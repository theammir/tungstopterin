use base64::{Engine as _, engine::general_purpose::STANDARD};
use rand::Rng;
use std::io::ErrorKind;
use tokio::net::{
    TcpStream,
    tcp::{OwnedReadHalf, OwnedWriteHalf},
};

use crate::{
    frame::Frame,
    message::{Message, StatusCode},
    server::generate_response_key,
};

fn generate_sec_key() -> String {
    let nonce: [u8; 16] = rand::rng().random();
    STANDARD.encode(nonce)
}

#[derive(Debug)]
pub struct WsClient {
    read_half: WsRecvHalf,
    write_half: WsSendHalf,
}

#[derive(Debug)]
pub struct WsRecvHalf(OwnedReadHalf);
#[derive(Debug)]
pub struct WsSendHalf(OwnedWriteHalf);

#[allow(async_fn_in_trait)]
pub trait WsSend {
    async fn write_to_socket(&mut self, data: &[u8]) -> std::io::Result<()>;
    async fn send(&mut self, message: Message) -> std::io::Result<()>;
}

#[allow(async_fn_in_trait)]
pub trait WsRecv {
    async fn read_from_socket(&mut self) -> std::io::Result<Vec<u8>>;
    async fn receive(&mut self) -> Result<Message, StatusCode>;
}

#[allow(async_fn_in_trait)]
pub trait Websocket: WsSend + WsRecv {
    async fn try_upgrade(&mut self) -> std::io::Result<()>;
}
impl<T: WsSend + WsRecv> Websocket for T {
    async fn try_upgrade(&mut self) -> std::io::Result<()> {
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
}

impl WsClient {
    pub fn from_stream(stream: TcpStream) -> WsClient {
        let (rx, tx) = stream.into_split();
        WsClient {
            read_half: WsRecvHalf(rx),
            write_half: WsSendHalf(tx),
        }
    }

    pub fn into_split(self) -> (WsRecvHalf, WsSendHalf) {
        (self.read_half, self.write_half)
    }
}

impl WsRecv for WsRecvHalf {
    async fn read_from_socket(&mut self) -> std::io::Result<Vec<u8>> {
        loop {
            self.0.readable().await?;
            let mut buf = [0_u8; 4096];
            match self.0.try_read(&mut buf) {
                Ok(n) => break Ok(buf[..n].to_vec()),
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => continue,
                Err(e) => break Err(e),
            }
        }
    }
    async fn receive(&mut self) -> Result<Message, StatusCode> {
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

impl WsSend for WsSendHalf {
    async fn write_to_socket(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.0.writable().await?;
        self.0.try_write(data)?;
        Ok(())
    }

    async fn send(&mut self, message: Message) -> std::io::Result<()> {
        let binary: Vec<u8> = {
            let mut frame: Frame = message.into();
            frame.mask();
            frame.into()
        };
        self.write_to_socket(&binary).await
    }
}

impl WsRecv for WsClient {
    async fn read_from_socket(&mut self) -> std::io::Result<Vec<u8>> {
        self.read_half.read_from_socket().await
    }

    async fn receive(&mut self) -> Result<Message, StatusCode> {
        self.read_half.receive().await
    }
}

impl WsSend for WsClient {
    async fn write_to_socket(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.write_half.write_to_socket(data).await
    }

    async fn send(&mut self, message: Message) -> std::io::Result<()> {
        self.write_half.send(message).await
    }
}
