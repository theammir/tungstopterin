use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha1::Digest;
use sha1::Sha1;
use std::io::ErrorKind;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;

use crate::frame::Frame;
use crate::message::Message;
use crate::message::StatusCode;

const SEC_WS_MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

pub fn generate_response_key(key: String) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key + SEC_WS_MAGIC);
    let result: Vec<u8> = hasher.finalize().iter().cloned().collect();
    STANDARD.encode(result)
}

fn validate_upgrade_headers(request: &str) -> bool {
    let lines: Vec<_> = request.lines().collect();

    lines
        .iter()
        .any(|l| l.eq_ignore_ascii_case("upgrade: websocket"))
        && lines
            .iter()
            .any(|l| l.eq_ignore_ascii_case("connection: upgrade"))
        && lines
            .iter()
            .any(|l| l.eq_ignore_ascii_case("sec-websocket-version: 13"))
        && lines
            .iter()
            .any(|l| l.to_ascii_lowercase().starts_with("host:"))
        && lines
            .iter()
            .any(|l| l.to_ascii_lowercase().starts_with("sec-websocket-key:"))
}

pub struct WsServer {
    listener: TcpListener,
}

// TODO: The entire thing is basically a wrapper around `&TcpStream`.
// As a server, we should probably accept listener connections in terms of plain TCP,
// and provide a wrapper with implemented WS stuff.
// `WSClient` already consumes the stream so I guess that's fine?
impl WsServer {
    pub fn new(listener: TcpListener) -> WsServer {
        WsServer { listener }
    }

    async fn read_from_socket<R>(socket: &mut R) -> std::io::Result<Vec<u8>>
    where
        R: AsyncReadExt + Unpin,
    {
        loop {
            let mut buf = [0_u8; 4096];
            match socket.read(&mut buf).await {
                Ok(n) => break Ok(buf[..n].to_vec()),
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => continue,
                Err(e) => break Err(e),
            }
        }
    }
    async fn write_to_socket<W>(socket: &mut W, data: &[u8]) -> std::io::Result<()>
    where
        W: AsyncWriteExt + Unpin,
    {
        // turns out i had convenience methods all along
        // still probably needs proper handling
        socket.write_all(data).await
    }

    pub async fn try_upgrade(socket: &mut TcpStream) -> std::io::Result<()> {
        let request = String::from_utf8(WsServer::read_from_socket(socket).await?.to_vec())
            .map_err(|_| ErrorKind::InvalidData)?;

        validate_upgrade_headers(&request);

        let sec_key = request
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("sec-websocket-key:"))
            .unwrap()
            .split_once(": ")
            .unwrap()
            .1;

        let response = format!(
            "\
HTTP/1.1 101 Switching Protocols\r
Upgrade: websocket\r
Connection: upgrade\r
Sec-Websocket-Accept: {key}\r\n",
            key = generate_response_key(sec_key.to_string())
        );

        WsServer::write_to_socket(socket, response.as_bytes()).await?;
        Ok(())
    }

    pub async fn send<W>(socket: &mut W, message: Message) -> std::io::Result<()>
    where
        W: AsyncWriteExt + Unpin,
    {
        let binary: Vec<u8> = {
            let frame: Frame = message.into();
            frame.into()
        };
        WsServer::write_to_socket(socket, &binary).await
    }

    pub async fn receive<R>(socket: &mut R) -> Result<Message, StatusCode>
    where
        R: AsyncReadExt + Unpin,
    {
        let data = WsServer::read_from_socket(socket)
            .await
            .map_err(|_| StatusCode::InternalServerError)?;

        {
            let mut frame: Frame = data.try_into().map_err(|_| StatusCode::ProtocolError)?;
            if frame.masking_key.is_some() {
                frame.mask();
            }
            frame.try_into()
        }
    }

    pub async fn listen<F, T: Fn(TcpStream) -> F>(&mut self, on_connect: T) -> std::io::Result<()>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        loop {
            let (mut socket, _) = self.listener.accept().await?;
            WsServer::try_upgrade(&mut socket).await?;

            tokio::spawn(on_connect(socket));
        }
    }
}
