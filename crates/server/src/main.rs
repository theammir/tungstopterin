use std::{sync::Arc, time::Duration};

use tokio::{net::TcpListener, sync::RwLock};
use websocket::{
    Client, WsRecv, WsRecvHalf, WsSend, WsSendHalf, WsStream, handshake::IntoWebsocket,
    message::Message,
};

async fn on_connect(socket: WsStream<Client>, clients_tx: Clients) {
    let (mut rx, tx) = socket.into_split();
    {
        let mut lock = clients_tx.write().await;
        lock.push(tx);
    }
    _ = tokio::join!(
        receive_messages(&mut rx, clients_tx), /* send_messages(&socket) */
    );
}

async fn _send_messages(
    socket: &mut WsSendHalf<Client>,
    clients_tx: Clients,
) -> std::io::Result<()> {
    let mut counter = 1;
    loop {
        if (socket
            .send(Message::Text(format!("Server: This is message #{counter}")))
            .await)
            .is_err()
        {
            println!("The client has disconnected");
            let mut lock = clients_tx.write().await;
            if let Some(pos) = lock.iter().position(|client| std::ptr::eq(client, socket)) {
                lock.remove(pos);
            }
            break Ok(());
        }
        println!("Sent message #{counter}");
        counter += 1;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn receive_messages(
    socket: &mut WsRecvHalf<Client>,
    clients_tx: Clients,
) -> std::io::Result<()> {
    let addr = socket.0.peer_addr()?;
    loop {
        match socket.receive().await {
            Ok(Message::Text(text)) => {
                println!("Received message: `{text}`");
                let mut lock = clients_tx.write().await;
                for c in lock.iter_mut() {
                    let msg_text = {
                        let client_addr = c.0.peer_addr()?;
                        if addr == client_addr {
                            format!("You: {text}")
                        } else {
                            format!("{client_addr}: {text}")
                        }
                    };
                    _ = c.send(Message::Text(msg_text)).await;
                }
            }
            Err(_) => {
                println!("The client has disconnected");
                break Ok(());
            }
            _ => (),
        }
    }
}

type Clients = Arc<RwLock<Vec<WsSendHalf<Client>>>>;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:1337").await?;
    let clients_tx: Clients = Arc::new(RwLock::new(vec![]));

    loop {
        if let Ok((socket, _)) = listener.accept().await {
            let mut socket = WsStream::<Client>::from_stream(socket);
            if socket.try_upgrade().await.is_ok() {
                tokio::spawn(on_connect(socket, Arc::clone(&clients_tx)));
            }
        }
    }
}
