use std::io::ErrorKind;
use tokio::net::TcpStream;

const UPGRADE_REQUEST: &str = "\
GET / HTTP/1.1\r
Host: {host}\r
Upgrade: websocket\r
Connection: upgrade\r
Sec-Websocket-Key: {key}\r
Sec-Websocket-Version: 13\r\n";

pub struct WSClient {
    stream: TcpStream,
}

impl WSClient {
    pub fn new(stream: TcpStream) -> WSClient {
        WSClient { stream }
    }

    async fn read_from_socket(&mut self) -> std::io::Result<Vec<u8>> {
        loop {
            self.stream.readable().await?;
            let mut buf = [0_u8; 4096];
            match self.stream.try_read(&mut buf) {
                Ok(n) => break Ok(buf[..n].to_vec()),
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => continue,
                Err(e) => break Err(e),
            }
        }
    }
    async fn write_to_socket(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.stream.writable().await?;
        self.stream.try_write(data)?;
        Ok(())
    }
    pub async fn upgrade(&mut self) -> std::io::Result<()> {
        self.write_to_socket(UPGRADE_REQUEST.as_bytes()).await?;
        let response: Vec<u8> = self.read_from_socket().await?;
        println!("{}", String::from_utf8(response).unwrap());
        Ok(())
    }
}
