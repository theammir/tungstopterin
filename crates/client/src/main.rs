use tokio::net::TcpStream;
use websocket::{client::WSClient, message::Message};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let stream = TcpStream::connect("127.0.0.1:1337").await?;
    let mut client = WSClient::new(stream);

    client.try_upgrade().await?;
    client.send(Message::Text("Hi!".to_string())).await?;
    println!("Sent message 'Hi!'");
    let message = client.receive().await;
    if let Ok(msg) = message {
        println!("Got message: {:?}", msg);
    }

    Ok(())
}
