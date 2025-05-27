use core::net::SocketAddr;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::Mutex as SyncMutex;
use std::sync::{Arc, OnceLock};

use common::protocol;
use tokio::{net::TcpListener, sync::Mutex};
use websocket::{
    Client, WsRecv, WsRecvHalf, WsSend, WsSendHalf, WsStream, handshake::IntoWebsocket,
    message::Message,
};

// turns out writing a thread safe static iterator is significantly more tedious than... not
type RandomNames = std::iter::Cycle<std::array::IntoIter<&'static str, 5>>;
static RANDOM_NAMES: OnceLock<SyncMutex<RandomNames>> = OnceLock::new();
fn random_names_init() -> SyncMutex<RandomNames> {
    SyncMutex::new(
        [
            "Anonymous Cat",
            "Anonymous Dog",
            "Anonymous Lion",
            "Anonymous Fox",
            "Anonymous Bear",
        ]
        .into_iter()
        .cycle(),
    )
}

type RandomColors = std::iter::Cycle<std::array::IntoIter<protocol::Color, 6>>;
static RANDOM_COLORS: OnceLock<SyncMutex<RandomColors>> = OnceLock::new();
fn random_colors_init() -> SyncMutex<RandomColors> {
    SyncMutex::new(
        [
            protocol::Color::Truecolor(255, 0, 0),
            protocol::Color::Truecolor(0, 255, 0),
            protocol::Color::Truecolor(0, 0, 255),
            protocol::Color::Truecolor(255, 255, 0),
            protocol::Color::Truecolor(0, 255, 255),
            protocol::Color::Truecolor(255, 0, 255),
        ]
        .into_iter()
        .cycle(),
    )
}

struct ClientData {
    tx: WsSendHalf<Client>,
    name: String,
    color: protocol::Color,
}

impl From<&ClientData> for protocol::MessageSender {
    fn from(value: &ClientData) -> Self {
        Self {
            name: value.name.to_string(),
            color: value.color,
        }
    }
}

impl From<&mut ClientData> for protocol::MessageSender {
    fn from(value: &mut ClientData) -> Self {
        Self {
            name: value.name.to_string(),
            color: value.color,
        }
    }
}

impl ClientData {
    fn new(tx: WsSendHalf<Client>) -> Self {
        let mut names_lock = RANDOM_NAMES.get().unwrap().lock().unwrap();
        let mut colors_lock = RANDOM_COLORS.get().unwrap().lock().unwrap();
        ClientData {
            tx,
            name: names_lock.next().unwrap().to_string(),
            color: colors_lock.next().unwrap(),
        }
    }
}

struct Clients {
    pub addr_map: HashMap<SocketAddr, ClientData>,
    pub token_map: HashMap<protocol::Token, SocketAddr>,
}

impl Clients {
    pub fn new() -> Self {
        Clients {
            addr_map: HashMap::new(),
            token_map: HashMap::new(),
        }
    }

    pub fn by_addr(&self, address: SocketAddr) -> Option<&ClientData> {
        self.addr_map.get(&address)
    }

    pub fn by_addr_mut(&mut self, address: SocketAddr) -> Option<&mut ClientData> {
        self.addr_map.get_mut(&address)
    }

    pub fn by_token(&self, token: protocol::Token) -> Option<&ClientData> {
        self.token_map
            .get(&token)
            .and_then(|addr| self.by_addr(*addr))
    }

    pub fn by_token_mut(&mut self, token: protocol::Token) -> Option<&mut ClientData> {
        self.token_map
            .get(&token)
            .cloned()
            .and_then(|addr| self.by_addr_mut(addr))
    }

    // TODO: Move these into whoever owns Clients in the future.
    pub fn generate_token(&self, address: SocketAddr) -> protocol::Token {
        address.to_string()
    }

    pub async fn try_connect(
        &mut self,
        address: SocketAddr,
        client: ClientData,
    ) -> Option<protocol::Token> {
        if self.addr_map.values().any(|c| *c.name == client.name) {
            None
        } else if self.addr_map.insert(address, client).is_none() {
            let token = self.generate_token(address);
            self.token_map.insert(token.clone(), address);
            Some(token)
        } else {
            None
        }
    }

    pub async fn disconnect(&mut self, address: SocketAddr) {
        self.addr_map.remove(&address);
        self.token_map.retain(|_, v| *v != address);
    }
    //

    pub async fn send_to_addr(
        &mut self,
        address: SocketAddr,
        message: Message,
    ) -> std::io::Result<()> {
        self.by_addr_mut(address)
            .ok_or::<std::io::Error>(std::io::ErrorKind::NotFound.into())?
            .tx
            .send(message)
            .await
    }

    pub async fn broadcast(&mut self, message: Message) -> std::io::Result<()> {
        for client in self.addr_map.values_mut() {
            client.tx.send(message.clone()).await?;
        }
        Ok(())
    }

    pub async fn broadcast_except_one(
        &mut self,
        address: SocketAddr,
        message: Message,
    ) -> std::io::Result<()> {
        for (addr, client) in self.addr_map.iter_mut() {
            if *addr == address {
                continue;
            }
            client.tx.send(message.clone()).await?;
        }
        Ok(())
    }
}

async fn on_disconnect(address: SocketAddr, clients: Arc<Mutex<Clients>>) {
    let mut lock = clients.lock().await;
    let client_name = lock.by_addr(address).map(|client| client.name.clone());
    if let Some(name) = client_name {
        println!("{name} ({address}) has disconnected.");
        _ = lock
            .broadcast_except_one(
                address,
                protocol::ServerMessage::ServerNotification(format!("{name} has disconnected.",))
                    .into(),
            )
            .await;
        lock.disconnect(address).await;
    }
}

async fn on_connect(socket: WsStream<Client>, clients: Arc<Mutex<Clients>>) -> std::io::Result<()> {
    let (mut rx, mut tx) = socket.into_split();
    let addr = rx.0.peer_addr()?;

    loop {
        let result = handle_auth(&mut rx, tx, Arc::clone(&clients)).await;
        match result {
            Ok(None) => break,
            Ok(Some(_tx)) => tx = _tx,
            Err(_) => return Err(ErrorKind::InvalidData)?,
        }
    }

    loop {
        match rx.receive().await {
            Ok(msg) => match protocol::ClientMessage::try_from(&msg) {
                Ok(message) => {
                    handle_client_message(message, Arc::clone(&clients)).await?;
                }
                Err(e) => {
                    println!("Received unknown message {msg:?} {e:?}")
                }
            },
            Err(_) => {
                on_disconnect(addr, clients).await;
                break Ok(());
            }
        }
    }
}

async fn handle_auth(
    rx: &mut WsRecvHalf<Client>,
    tx: WsSendHalf<Client>,
    clients: Arc<Mutex<Clients>>,
) -> std::io::Result<Option<WsSendHalf<Client>>> {
    let addr = tx.0.peer_addr()?;
    let client_msg = rx
        .receive()
        .await
        .map(|msg| protocol::ClientMessage::try_from(&msg).ok())
        .ok()
        .flatten();

    if client_msg.is_none() {
        return Ok(Some(tx));
    }

    let new_sender: protocol::MessageSender;
    let maybe_token: Option<protocol::Token>;
    match client_msg.unwrap() {
        protocol::ClientMessage::SimpleAuth => {
            let data = ClientData::new(tx);
            new_sender = (&data).into();
            maybe_token = clients.lock().await.try_connect(addr, data).await;
        }
        protocol::ClientMessage::Auth(sender) => {
            new_sender = sender;
            maybe_token = clients
                .lock()
                .await
                .try_connect(
                    addr,
                    ClientData {
                        tx,
                        name: new_sender.name.clone(),
                        color: new_sender.color,
                    },
                )
                .await;
        }
        _ => return Ok(Some(tx)),
    }
    let auth_success = maybe_token.is_some();

    let mut lock = clients.lock().await;
    lock.send_to_addr(
        addr,
        protocol::ServerMessage::AuthSuccess(maybe_token).into(),
    )
    .await?;

    if auth_success {
        println!("{} ({addr}) has connected.", new_sender.name);
        lock.broadcast_except_one(
            addr,
            protocol::ServerMessage::ServerNotification(format!(
                "{} has connected.",
                new_sender.name
            ))
            .into(),
        )
        .await?;
    }
    Ok(None)
}

async fn handle_client_message(
    message: protocol::ClientMessage,
    clients: Arc<Mutex<Clients>>,
) -> std::io::Result<()> {
    match message {
        protocol::ClientMessage::SendMessage { token, text } => {
            let maybe_sender: Option<protocol::MessageSender> = clients
                .lock()
                .await
                .by_token(token.clone())
                .map(|client| client.into());
            match maybe_sender {
                Some(sender) => {
                    clients
                        .lock()
                        .await
                        .broadcast(protocol::ServerMessage::PropagateMessage(sender, text).into())
                        .await?;
                }
                None => println!("Unknown sender with token `{token}`"),
            }
            Ok(())
        }
        msg => {
            println!("Unhandled message {msg:?}");
            Ok(())
        }
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    RANDOM_NAMES.get_or_init(random_names_init);
    RANDOM_COLORS.get_or_init(random_colors_init);

    let listener = TcpListener::bind("127.0.0.1:1337").await?;
    let clients = Arc::new(Mutex::new(Clients::new()));

    loop {
        if let Ok((socket, _)) = listener.accept().await {
            let mut socket = WsStream::<Client>::from_stream(socket);
            if socket.try_upgrade().await.is_ok() {
                tokio::spawn(on_connect(socket, Arc::clone(&clients)));
            }
        }
    }
}
