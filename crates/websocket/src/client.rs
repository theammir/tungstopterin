use base64::{Engine as _, engine::general_purpose::STANDARD};
use rand::Rng;
use std::io::ErrorKind;
use tokio::net::TcpStream;

use crate::{
    frame::Frame,
    message::{Message, StatusCode},
    server::generate_response_key,
};

fn generate_sec_key() -> String {
    let nonce: [u8; 16] = rand::rng().random();
    STANDARD.encode(nonce)
}

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

    pub async fn try_upgrade(&mut self) -> std::io::Result<()> {
        let sec_key = generate_sec_key();
        self.write_to_socket(
            format!(
                "\
GET / HTTP/1.1\r
Host: {host}\r
Upgrade: websocket\r
Connection: upgrade\r
Sec-Websocket-Key: {key}\r
Sec-Websocket-Version: 13\r\n",
                host = "idk",
                key = sec_key
            )
            .as_bytes(),
        )
        .await?;
        let response = String::from_utf8(self.read_from_socket().await?)
            .map_err(|_| ErrorKind::InvalidData)?;

        let resp_key = response
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("sec-websocket-accept:"))
            .ok_or::<std::io::Error>(ErrorKind::InvalidData.into())?
            .split_once(": ")
            .unwrap()
            .1;

        if resp_key != generate_response_key(sec_key) {
            return Err(ErrorKind::InvalidData.into());
        }

        Ok(())
    }

    pub async fn send(&mut self, message: Message) -> std::io::Result<()> {
        let binary: Vec<u8> = {
            let mut frame: Frame = message.into();
            frame.mask();
            frame.into()
        };
        self.write_to_socket(&binary).await
    }

    pub async fn receive(&mut self) -> Result<Message, StatusCode> {
        let data = self
            .read_from_socket()
            .await
            .map_err(|_| StatusCode::InternalServerError)?;

        {
            let frame: Frame = data.try_into().map_err(|_| StatusCode::ProtocolError)?;
            frame.try_into()
        }
    }
}
