use core::net::SocketAddr;
use std::sync::Mutex as SyncMutex;
use std::sync::{Arc, OnceLock};

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

type RandomColors = std::iter::Cycle<std::array::IntoIter<(u8, u8, u8), 6>>;
static RANDOM_COLORS: OnceLock<SyncMutex<RandomColors>> = OnceLock::new();
fn random_colors_init() -> SyncMutex<RandomColors> {
    SyncMutex::new(
        [
            (255, 0, 0),
            (0, 255, 0),
            (0, 0, 255),
            (255, 255, 0),
            (0, 255, 255),
            (255, 0, 255),
        ]
        .into_iter()
        .cycle(),
    )
}

struct ClientData {
    tx: WsSendHalf<Client>,
    addr: SocketAddr,
    name: &'static str,
    color: (u8, u8, u8),
}

impl ClientData {
    fn new(tx: WsSendHalf<Client>, address: SocketAddr) -> Self {
        let mut names_lock = RANDOM_NAMES.get().unwrap().lock().unwrap();
        let mut colors_lock = RANDOM_COLORS.get().unwrap().lock().unwrap();
        ClientData {
            tx,
            addr: address,
            name: names_lock.next().unwrap(),
            color: colors_lock.next().unwrap(),
        }
    }
}

struct Clients(pub Vec<ClientData>);

impl Clients {
    pub fn new() -> Self {
        Clients(vec![])
    }

    pub fn by_addr(&self, address: SocketAddr) -> Option<&ClientData> {
        self.0.iter().find(|&client| client.addr == address)
    }

    pub fn by_addr_mut(&mut self, address: SocketAddr) -> Option<&mut ClientData> {
        self.0.iter_mut().find(|client| client.addr == address)
    }

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
        for client in &mut self.0 {
            client.tx.send(message.clone()).await?;
        }
        Ok(())
    }

    pub async fn broadcast_except_one(
        &mut self,
        address: SocketAddr,
        message: Message,
    ) -> std::io::Result<()> {
        for client in &mut self.0 {
            if client.addr == address {
                continue;
            }
            client.tx.send(message.clone()).await?;
        }
        Ok(())
    }
}

async fn on_connect(socket: WsStream<Client>, clients: Arc<Mutex<Clients>>) {
    let addr = socket.rx.0.peer_addr().unwrap();

    let (mut rx, tx) = socket.into_split();
    let client = ClientData::new(tx, addr);
    println!("{} ({addr}) has connected.", client.name);

    let mut lock = clients.lock().await;
    _ = lock
        .broadcast(Message::Text(format!("{} has connected.", client.name)))
        .await;
    lock.0.push(client);
    drop(lock);

    _ = tokio::join!(receive_messages(&mut rx, clients));
}

async fn on_disconnect(address: SocketAddr, clients: Arc<Mutex<Clients>>) {
    let mut lock = clients.lock().await;
    let client_name = lock.by_addr(address).unwrap().name;
    println!("{client_name} ({address}) has disconnected.");
    _ = lock
        .broadcast_except_one(
            address,
            Message::Text(format!("{client_name} has disconnected.",)),
        )
        .await;
    if let Some(pos) = lock.0.iter().position(|socket| socket.addr == address) {
        lock.0.remove(pos);
    }
}

async fn receive_messages(
    rx: &mut WsRecvHalf<Client>,
    clients: Arc<Mutex<Clients>>,
) -> std::io::Result<()> {
    let sender_addr = rx.0.peer_addr()?;
    let sender_name = clients
        .lock()
        .await
        .by_addr(sender_addr)
        .ok_or(std::io::ErrorKind::NotFound)?
        .name;

    loop {
        match rx.receive().await {
            Ok(Message::Text(text)) => {
                println!("{sender_name}: `{text}`");
                _ = clients
                    .lock()
                    .await
                    .broadcast_except_one(
                        sender_addr,
                        Message::Text(format!("{sender_name}: {text}")),
                    )
                    .await;
                _ = clients
                    .lock()
                    .await
                    .send_to_addr(sender_addr, Message::Text(format!("You: {text}")))
                    .await;
            }
            Err(_) => {
                tokio::spawn(on_disconnect(sender_addr, clients));
                break Ok(());
            }
            _ => (),
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
