use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha1::Digest;
use sha1::Sha1;
use std::io::ErrorKind;
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

pub struct WSServer {
    listener: TcpListener,
}

// TODO: The entire thing is basically a wrapper around `&TcpStream`.
// As a server, we should probably accept listener connections in terms of plain TCP,
// and provide a wrapper with implemented WS stuff.
// `WSClient` already consumes the stream so I guess that's fine?
impl WSServer {
    pub fn new(listener: TcpListener) -> WSServer {
        WSServer { listener }
    }

    async fn read_from_socket(socket: &TcpStream) -> std::io::Result<Vec<u8>> {
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
    async fn write_to_socket(socket: &TcpStream, data: &[u8]) -> std::io::Result<()> {
        socket.writable().await?;
        socket.try_write(data)?;
        Ok(())
    }

    pub async fn try_upgrade(socket: &TcpStream) -> std::io::Result<()> {
        let request = String::from_utf8(WSServer::read_from_socket(socket).await?.to_vec())
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

        WSServer::write_to_socket(socket, response.as_bytes()).await?;
        Ok(())
    }

    pub async fn send(socket: &TcpStream, message: Message) -> std::io::Result<()> {
        let binary: Vec<u8> = {
            let frame: Frame = message.into();
            frame.into()
        };
        WSServer::write_to_socket(socket, &binary).await
    }

    pub async fn receive(socket: &TcpStream) -> Result<Message, StatusCode> {
        let data = WSServer::read_from_socket(socket)
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

    pub async fn listen(&mut self) -> std::io::Result<()> {
        loop {
            let (socket, _) = self.listener.accept().await?;
            WSServer::try_upgrade(&socket).await?;

            tokio::spawn(async move {
                loop {
                    let message = WSServer::receive(&socket).await;
                    if let Ok(msg) = message {
                        println!("Got message: {:?}", msg);
                        _ = WSServer::send(&socket, msg).await;
                        println!("Send message back.");
                    } else {
                        break;
                    }
                }
            });
        }
    }
}
