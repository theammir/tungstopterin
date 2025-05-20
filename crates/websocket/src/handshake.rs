use std::io::ErrorKind;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rand::Rng;
use sha1::Digest;
use sha1::Sha1;

use crate::Client;
use crate::Server;
use crate::WsRecv;
use crate::WsSend;
use crate::WsStream;

const SEC_WS_MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

fn generate_sec_key() -> String {
    let nonce: [u8; 16] = rand::rng().random();
    STANDARD.encode(nonce)
}

fn generate_response_key(key: String) -> String {
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

// FIX: Now IntoWebsocket is implemented by a WsStream, which is supposed to be an already
// established upgraded connection.
// Proper semantics would be upgrading a TcpStream into a WsStream.
// Tungstenite achieves this by leaving the method at module scope.
#[allow(async_fn_in_trait)]
pub trait IntoWebsocket: WsSend + WsRecv {
    async fn try_upgrade(&mut self) -> std::io::Result<()>;
}

impl IntoWebsocket for WsStream<Server> {
    async fn try_upgrade(&mut self) -> std::io::Result<()> {
        let sec_key = generate_sec_key();
        self.send_raw(
            format!(
                "\
GET / HTTP/1.1\r
Host: {host}\r
Upgrade: websocket\r
Connection: upgrade\r
Sec-Websocket-Key: {key}\r
Sec-Websocket-Version: 13\r\n\r\n",
                host = "idk",
                key = sec_key
            )
            .as_bytes(),
        )
        .await?;
        let response =
            String::from_utf8(self.read_http_bytes().await?).map_err(|_| ErrorKind::InvalidData)?;

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

impl IntoWebsocket for WsStream<Client> {
    async fn try_upgrade(&mut self) -> std::io::Result<()> {
        let request = String::from_utf8(self.read_http_bytes().await?.to_vec())
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
Sec-Websocket-Accept: {key}\r\n\r\n",
            key = generate_response_key(sec_key.to_string())
        );

        self.send_raw(response.as_bytes()).await?;
        Ok(())
    }
}
