use std::io::ErrorKind;
use tokio::net::TcpListener;
use tokio::net::TcpStream;

pub const SEC_WS_MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const UPGRADE_RESPONSE: &str = "\
HTTP/1.1 101 Switching Protocols\r
Upgrade: websocket\r
Connection: Upgrade\r
Sec-WebSocket-Accept: {key}\r\n";

pub struct WSServer {
    listener: TcpListener,
}

impl WSServer {
    pub fn new(listener: TcpListener) -> WSServer {
        WSServer { listener }
    }

    async fn read_from_socket(&mut self, socket: &TcpStream) -> std::io::Result<Vec<u8>> {
        loop {
            socket.readable().await?;
            let mut buf = [0_u8; 4096];
            match socket.try_read(&mut buf) {
                Ok(n) => break Ok(buf[..n].to_vec()),
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => continue,
                Err(e) => break Err(e),
            }
        }
    }
    async fn write_to_socket(&mut self, socket: &TcpStream, data: &[u8]) -> std::io::Result<()> {
        socket.writable().await?;
        socket.try_write(data)?;
        Ok(())
    }

    pub async fn listen(&mut self) -> Result<(), std::io::Error> {
        let (socket, _) = self.listener.accept().await?;
        let data = self.read_from_socket(&socket).await?.to_vec();
        println!("{}", String::from_utf8(data).unwrap());
        self.write_to_socket(&socket, UPGRADE_RESPONSE.as_bytes())
            .await?;
        Ok(())
    }
}
