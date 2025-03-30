use tokio::net::TcpListener;
use websocket::server::WSServer;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:1337").await?;
    let mut server = WSServer::new(listener);

    server.listen().await?;

    Ok(())
}
