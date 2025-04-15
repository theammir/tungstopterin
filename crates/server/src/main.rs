use std::{sync::Arc, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        TcpListener, TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::RwLock,
};
use websocket::{message::Message, server::WsServer};

async fn on_connect(socket: TcpStream, clients_tx: Clients) {
    let (mut rx, tx) = socket.into_split();
    {
        let mut lock = clients_tx.write().await;
        lock.push(tx);
    }
    _ = tokio::join!(
        receive_messages(&mut rx, clients_tx), /* send_messages(&socket) */
    );
}

async fn send_messages(socket: &mut OwnedWriteHalf, clients_tx: Clients) -> std::io::Result<()> {
    let mut counter = 1;
    loop {
        if (WsServer::send(
            socket,
            Message::Text(format!("Server: This is message #{counter}")),
        )
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

async fn receive_messages(socket: &mut OwnedReadHalf, clients_tx: Clients) -> std::io::Result<()> {
    let addr = socket.peer_addr()?;
    loop {
        match WsServer::receive(socket).await {
            Ok(Message::Text(text)) => {
                println!("Received message: `{text}`");
                let mut lock = clients_tx.write().await;
                for c in lock.iter_mut() {
                    let msg_text = {
                        let client_addr = c.peer_addr()?;
                        if addr == client_addr {
                            format!("You: {text}")
                        } else {
                            format!("{client_addr}: {text}")
                        }
                    };
                    _ = WsServer::send(c, Message::Text(msg_text)).await;
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

type Clients = Arc<RwLock<Vec<OwnedWriteHalf>>>;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:1337").await?;
    let mut server = WsServer::new(listener);
    let clients_tx: Clients = Arc::new(RwLock::new(vec![]));

    server
        .listen(|socket: TcpStream| on_connect(socket, Arc::clone(&clients_tx)))
        .await?;

    Ok(())
}
