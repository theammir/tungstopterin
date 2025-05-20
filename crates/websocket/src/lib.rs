pub mod frame;
pub mod handshake;
pub mod message;

use frame::{Frame, FrameHeader, PayloadLen};
use message::MessageError;
use std::{io::ErrorKind, marker::PhantomData};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{
        TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
};

use crate::message::{Message, StatusCode};

/// Read HTTP headers separated by *\r\n*.
/// Stop when encountering an empty line.
async fn read_http_bytes<R>(stream: &mut R) -> std::io::Result<Vec<u8>>
where
    R: AsyncReadExt + Unpin,
{
    // PERF: Look into [BufReader]
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    loop {
        let n = reader.read_line(&mut buf).await?;
        if n == 0 {
            Err(ErrorKind::UnexpectedEof)?
        }
        if &buf[buf.len() - 4..] == "\r\n\r\n" {
            break;
        }
    }
    Ok(buf.into_bytes())
}

/// Read first 2 bytes, determine length, read additional 0/2/8 bytes.
/// Read until exactly that many bytes are read + masking key.
async fn read_frame_bytes<R>(stream: &mut R) -> std::io::Result<Vec<u8>>
where
    R: AsyncReadExt + Unpin,
{
    let mut header_buf = [0u8; 2];
    stream.read_exact(&mut header_buf).await?;
    let header: FrameHeader = header_buf[..]
        .try_into()
        .map_err(|_| ErrorKind::InvalidData)?;

    let mut payload_buf = [0u8; 8];
    let payload_len_bytes: usize;
    let payload_len = match header.payload_len {
        PayloadLen::ExactU8(n) => {
            payload_len_bytes = 0;
            n as u64
        }
        PayloadLen::HintU16 => {
            payload_len_bytes = 2;
            stream.read_exact(&mut payload_buf[..2]).await?;
            u16::from_be_bytes(payload_buf[..2].try_into().unwrap()) as u64
        }
        PayloadLen::HintU64 => {
            payload_len_bytes = 8;
            stream.read_exact(&mut payload_buf).await?;
            u64::from_be_bytes(payload_buf)
        }
        _ => unreachable!(),
    };

    let frame_len: usize = 2 + payload_len_bytes + if header.masked { 4 } else { 0 };
    let mut frame_vec = vec![0u8; frame_len + payload_len as usize];

    let mut masked_key_buf = payload_buf;
    if header.masked {
        stream.read_exact(&mut masked_key_buf[..4]).await?;
        frame_vec[2 + payload_len_bytes..][..4].copy_from_slice(&masked_key_buf[..4]);
    }

    stream.read_exact(&mut frame_vec[frame_len..]).await?;
    frame_vec[..2].copy_from_slice(&header_buf);
    Ok(frame_vec)
}

pub trait Side {}
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct Server;
impl Side for Server {}
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct Client;
impl Side for Client {}

#[derive(Debug)]
pub struct WsStream<S: Side> {
    pub rx: WsRecvHalf<S>,
    pub tx: WsSendHalf<S>,
}

impl<S: Side> WsStream<S> {
    pub fn from_stream(stream: TcpStream) -> WsStream<S> {
        let (rx, tx) = stream.into_split();
        WsStream {
            rx: WsRecvHalf(rx, PhantomData::<S>),
            tx: WsSendHalf(tx, PhantomData::<S>),
        }
    }

    pub fn into_split(self) -> (WsRecvHalf<S>, WsSendHalf<S>) {
        (self.rx, self.tx)
    }
}

#[derive(Debug)]
pub struct WsRecvHalf<S: Side>(pub OwnedReadHalf, PhantomData<S>);
#[derive(Debug)]
pub struct WsSendHalf<S: Side>(pub OwnedWriteHalf, PhantomData<S>);

#[allow(async_fn_in_trait)]
pub trait WsSend {
    async fn send_raw(&mut self, data: &[u8]) -> std::io::Result<()>;
    async fn send(&mut self, message: Message) -> std::io::Result<()>;
}

#[allow(async_fn_in_trait)]
pub trait WsRecv {
    async fn read_http_bytes(&mut self) -> std::io::Result<Vec<u8>>;
    async fn read_frame_bytes(&mut self) -> std::io::Result<Vec<u8>>;
    async fn receive(&mut self) -> Result<Message, MessageError>;
}

impl WsRecv for WsRecvHalf<Server> {
    async fn read_http_bytes(&mut self) -> std::io::Result<Vec<u8>> {
        read_http_bytes(&mut self.0).await
    }

    async fn read_frame_bytes(&mut self) -> std::io::Result<Vec<u8>> {
        read_frame_bytes(&mut self.0).await
    }

    async fn receive(&mut self) -> Result<Message, MessageError> {
        let data = self
            .read_frame_bytes()
            .await
            .map_err(|_| MessageError::ProtocolViolated(StatusCode::InternalServerError))?;

        let frame: Frame = data
            .try_into()
            .map_err(|_| MessageError::ProtocolViolated(StatusCode::ProtocolError))?;
        frame.try_into()
    }
}

impl WsSend for WsSendHalf<Server> {
    async fn send_raw(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.0.write_all(data).await?;
        self.0.flush().await?;
        Ok(())
    }

    async fn send(&mut self, message: Message) -> std::io::Result<()> {
        let binary: Vec<u8> = {
            let mut frame: Frame = message.into();
            frame.mask();
            frame.into()
        };
        self.send_raw(&binary).await
    }
}

impl WsRecv for WsRecvHalf<Client> {
    async fn read_http_bytes(&mut self) -> std::io::Result<Vec<u8>> {
        read_http_bytes(&mut self.0).await
    }

    async fn read_frame_bytes(&mut self) -> std::io::Result<Vec<u8>> {
        read_frame_bytes(&mut self.0).await
    }

    async fn receive(&mut self) -> Result<Message, MessageError> {
        let data = read_frame_bytes(&mut self.0)
            .await
            .map_err(|_| MessageError::ProtocolViolated(StatusCode::InternalServerError))?;

        let mut frame: Frame = data
            .try_into()
            .map_err(|_| MessageError::ProtocolViolated(StatusCode::ProtocolError))?;
        frame.mask();
        frame.try_into()
    }
}

impl WsSend for WsSendHalf<Client> {
    async fn send_raw(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.0.write_all(data).await?;
        self.0.flush().await?;
        Ok(())
    }

    async fn send(&mut self, message: Message) -> std::io::Result<()> {
        let binary: Vec<u8> = {
            let frame: Frame = message.into();
            frame.into()
        };
        self.send_raw(&binary).await
    }
}

impl WsRecv for WsStream<Server> {
    async fn read_http_bytes(&mut self) -> std::io::Result<Vec<u8>> {
        self.rx.read_http_bytes().await
    }

    async fn read_frame_bytes(&mut self) -> std::io::Result<Vec<u8>> {
        self.rx.read_frame_bytes().await
    }

    async fn receive(&mut self) -> Result<Message, MessageError> {
        self.rx.receive().await
    }
}

impl WsSend for WsStream<Server> {
    async fn send_raw(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.tx.send_raw(data).await
    }

    async fn send(&mut self, message: Message) -> std::io::Result<()> {
        self.tx.send(message).await
    }
}

impl WsRecv for WsStream<Client> {
    async fn read_http_bytes(&mut self) -> std::io::Result<Vec<u8>> {
        self.rx.read_http_bytes().await
    }

    async fn read_frame_bytes(&mut self) -> std::io::Result<Vec<u8>> {
        self.rx.read_frame_bytes().await
    }

    async fn receive(&mut self) -> Result<Message, MessageError> {
        self.rx.receive().await
    }
}

impl WsSend for WsStream<Client> {
    async fn send_raw(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.tx.send_raw(data).await
    }

    async fn send(&mut self, message: Message) -> std::io::Result<()> {
        self.tx.send(message).await
    }
}
