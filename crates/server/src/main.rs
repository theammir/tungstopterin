#![warn(clippy::pedantic)]
use core::net::SocketAddr;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::Arc;

use common::protocol;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::Mutex,
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    },
};
use websocket::{
    Client, WsRecv, WsRecvHalf, WsSend, WsSendHalf, WsStream,
    handshake::IntoWebsocket,
    message::{Message, MessageError},
};

type TlsStream = tokio_rustls::server::TlsStream<TcpStream>;

#[derive(Debug)]
struct ClientData {
    tx: WsSendHalf<Client, TlsStream>,
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

    pub fn by_token(&self, token: &protocol::Token) -> Option<&ClientData> {
        self.token_map
            .get(token)
            .and_then(|addr| self.by_addr(*addr))
    }

    #[allow(dead_code)]
    pub fn by_token_mut(&mut self, token: &protocol::Token) -> Option<&mut ClientData> {
        self.token_map
            .get(token)
            .copied()
            .and_then(|addr| self.by_addr_mut(addr))
    }

    // TODO: Move these into whoever owns Clients in the future.
    pub fn generate_token(address: SocketAddr) -> protocol::Token {
        address.to_string()
    }

    pub fn try_connect(
        &mut self,
        address: SocketAddr,
        client: ClientData,
    ) -> Result<protocol::Token, (protocol::AuthError, ClientData)> {
        if self.addr_map.values().any(|c| *c.name == client.name) {
            return Err((protocol::AuthError::NicknameUnavailable, client));
        }
        if client.name.len() > protocol::NICKNAME_MAX_LEN {
            return Err((protocol::AuthError::NicknameTooLong, client));
        }

        if let Some(client) = self.addr_map.insert(address, client) {
            Err((protocol::AuthError::AlreadyAuthorized, client))
        } else {
            let token = Clients::generate_token(address);
            self.token_map.insert(token.clone(), address);
            Ok(token)
        }
    }

    pub fn disconnect(&mut self, address: SocketAddr) {
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
        for (addr, client) in &mut self.addr_map {
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
    let maybe_sender = lock.by_addr(address).map(protocol::MessageSender::from);
    if let Some(sender) = maybe_sender {
        println!("{} ({address}) has disconnected.", sender.name);
        _ = lock
            .broadcast_except_one(
                address,
                protocol::ServerMessage::Notification(
                    protocol::ServerNotification::ClientDisconnected(sender),
                )
                .into(),
            )
            .await;
        lock.disconnect(address);
    }
}

async fn on_connect(
    socket: WsStream<Client, TlsStream>,
    addr: SocketAddr,
    clients: Arc<Mutex<Clients>>,
) -> std::io::Result<()> {
    let (mut rx, mut tx) = socket.into_split();

    loop {
        let result = handle_auth(&mut rx, tx, addr, Arc::clone(&clients)).await;
        match result {
            Ok(None) => break,
            Ok(Some(tx_)) => tx = tx_,
            Err(_) => {
                // currently has no effect, but is probably the
                // right thing to do
                on_disconnect(addr, clients).await;
                return Ok(());
            }
        }
    }

    loop {
        if let Ok(msg) = rx.receive().await {
            match protocol::ClientMessage::try_from(&msg) {
                Ok(message) => {
                    handle_client_message(message, Arc::clone(&clients)).await?;
                }
                Err(e) => {
                    println!("Received unknown message {msg:?} {e:?}");
                }
            }
        } else {
            on_disconnect(addr, clients).await;
            return Ok(());
        }
    }
}

async fn handle_auth(
    rx: &mut WsRecvHalf<Client, TlsStream>,
    tx: WsSendHalf<Client, TlsStream>,
    addr: SocketAddr,
    clients: Arc<Mutex<Clients>>,
) -> std::io::Result<Option<WsSendHalf<Client, TlsStream>>> {
    let client_msg = match rx.receive().await {
        Ok(msg) => protocol::ClientMessage::try_from(&msg).ok(),
        Err(MessageError::ProtocolViolated(websocket::message::StatusCode::CloseAbnormal)) => {
            return Err(ErrorKind::UnexpectedEof.into());
        }
        Err(_) => return Ok(Some(tx)),
    };

    let new_sender: protocol::MessageSender;
    let maybe_token = match client_msg.unwrap() {
        protocol::ClientMessage::Auth(sender) => {
            new_sender = sender;
            clients.lock().await.try_connect(
                addr,
                ClientData {
                    tx,
                    name: new_sender.name.clone(),
                    color: new_sender.color,
                },
            )
        }
        _ => return Ok(Some(tx)),
    };

    let mut lock = clients.lock().await;

    if let Err((err, client_data)) = maybe_token {
        let mut tx = client_data.tx;
        tx.send(protocol::ServerMessage::AuthSuccess(Err(err)).into())
            .await?;
        return Ok(Some(tx));
    }

    lock.send_to_addr(
        addr,
        protocol::ServerMessage::AuthSuccess(maybe_token.map_err(|(err, _)| err)).into(),
    )
    .await?;
    println!("{} ({addr}) has connected.", new_sender.name);
    lock.broadcast_except_one(
        addr,
        protocol::ServerMessage::Notification(protocol::ServerNotification::ClientConnected(
            new_sender,
        ))
        .into(),
    )
    .await?;

    Ok(None)
}

async fn handle_client_message(
    message: protocol::ClientMessage,
    clients: Arc<Mutex<Clients>>,
) -> std::io::Result<()> {
    match message {
        protocol::ClientMessage::SendMessage { token, text, image } => {
            let maybe_sender: Option<protocol::MessageSender> = clients
                .lock()
                .await
                .by_token(&token)
                .map(protocol::MessageSender::from);
            match maybe_sender {
                Some(sender) => {
                    clients
                        .lock()
                        .await
                        .broadcast(
                            protocol::ServerMessage::PropagateMessage(sender, text, image).into(),
                        )
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
    // TODO: clap
    let certs = CertificateDer::pem_file_iter("certs/cert.pem")
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_file("certs/cert.key.pem").unwrap();

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));

    let listener = TcpListener::bind("localhost:1337").await?;
    let clients = Arc::new(Mutex::new(Clients::new()));

    loop {
        if let Ok((socket, _)) = listener.accept().await {
            let Ok(socket) = acceptor.accept(socket).await else {
                continue;
            };

            let Ok(addr) = socket.get_ref().0.peer_addr() else {
                continue;
            };

            let mut socket = WsStream::<Client, _>::from_stream(socket);
            if socket.try_upgrade("localhost:1337").await.is_ok() {
                tokio::spawn(on_connect(socket, addr, Arc::clone(&clients)));
            }
        }
    }
}
