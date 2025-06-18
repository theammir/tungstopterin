#![warn(clippy::pedantic)]

pub mod frame;
pub mod handshake;
pub mod message;

use frame::{Frame, FrameHeader, PayloadLen};
use message::MessageError;
use std::{io::ErrorKind, marker::PhantomData};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};

use crate::message::{Message, StatusCode};

pub trait UnpinReader: AsyncReadExt + Unpin {}
impl<T: AsyncReadExt + Unpin> UnpinReader for T {}
pub trait UnpinWriter: AsyncWriteExt + Unpin {}
impl<T: AsyncWriteExt + Unpin> UnpinWriter for T {}
pub trait UnpinStream: UnpinReader + UnpinWriter {}
impl<T: UnpinReader + UnpinWriter> UnpinStream for T {}

/// Read HTTP headers separated by *\r\n*.
/// Stop when encountering an empty line.
async fn read_http_bytes<R>(stream: &mut R) -> std::io::Result<Vec<u8>>
where
    R: UnpinReader,
{
    // PERF: Look into [BufReader]
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    loop {
        let n = reader.read_line(&mut buf).await?;
        if n == 0 {
            Err(ErrorKind::UnexpectedEof)?;
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
    R: UnpinReader,
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
            n.into()
        }
        PayloadLen::HintU16 => {
            payload_len_bytes = 2;
            stream.read_exact(&mut payload_buf[..2]).await?;
            u16::from_be_bytes(payload_buf[..2].try_into().unwrap()).into()
        }
        PayloadLen::HintU64 => {
            payload_len_bytes = 8;
            stream.read_exact(&mut payload_buf).await?;
            u64::from_be_bytes(payload_buf)
        }
        _ => unreachable!(),
    };

    let frame_len: usize = 2 + payload_len_bytes + if header.masked { 4 } else { 0 };
    #[allow(clippy::cast_possible_truncation)]
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
pub struct WsStream<S: Side, T: UnpinStream> {
    pub rx: WsRecvHalf<S, T>,
    pub tx: WsSendHalf<S, T>,
}

impl<S: Side, T: UnpinStream> WsStream<S, T> {
    pub fn from_stream(stream: T) -> WsStream<S, T> {
        let (rx, tx) = tokio::io::split(stream);
        WsStream {
            rx: WsRecvHalf(rx, PhantomData::<S>),
            tx: WsSendHalf(tx, PhantomData::<S>),
        }
    }

    #[must_use]
    pub fn into_split(self) -> (WsRecvHalf<S, T>, WsSendHalf<S, T>) {
        (self.rx, self.tx)
    }
}

#[derive(Debug)]
pub struct WsRecvHalf<S: Side, T: UnpinStream>(pub ReadHalf<T>, PhantomData<S>);
#[derive(Debug)]
pub struct WsSendHalf<S: Side, T: UnpinStream>(pub WriteHalf<T>, PhantomData<S>);

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

// TODO: Fix essentially duplicate implementations. Can I make a default implementation
// of `read_http_bytes`, `read_frame_bytes`, `receive` and `send` somewhere else than module level
// to make minor changes in individual impls?

impl<T: UnpinStream> WsRecv for WsRecvHalf<Server, T> {
    async fn read_http_bytes(&mut self) -> std::io::Result<Vec<u8>> {
        read_http_bytes(&mut self.0).await
    }

    async fn read_frame_bytes(&mut self) -> std::io::Result<Vec<u8>> {
        read_frame_bytes(&mut self.0).await
    }

    async fn receive(&mut self) -> Result<Message, MessageError> {
        let mut frames: Vec<Frame> = vec![];
        loop {
            let data = self
                .read_frame_bytes()
                .await
                .map_err(|_| MessageError::ProtocolViolated(StatusCode::CloseAbnormal))?;
            let frame: Frame = data
                .try_into()
                .map_err(|_| MessageError::ProtocolViolated(StatusCode::ProtocolError))?;
            let fin = frame.header.fin;

            // avoid first allocation
            if frames.is_empty() && fin {
                return frame.try_into();
            }

            frames.push(frame);

            if fin {
                break;
            }
        }
        frames.try_into()
    }
}

impl<T: UnpinStream> WsSend for WsSendHalf<Server, T> {
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

impl<T: UnpinStream> WsRecv for WsRecvHalf<Client, T> {
    async fn read_http_bytes(&mut self) -> std::io::Result<Vec<u8>> {
        read_http_bytes(&mut self.0).await
    }

    async fn read_frame_bytes(&mut self) -> std::io::Result<Vec<u8>> {
        read_frame_bytes(&mut self.0).await
    }

    async fn receive(&mut self) -> Result<Message, MessageError> {
        let mut frames: Vec<Frame> = vec![];
        loop {
            let data = self
                .read_frame_bytes()
                .await
                .map_err(|_| MessageError::ProtocolViolated(StatusCode::CloseAbnormal))?;
            let mut frame: Frame = data
                .try_into()
                .map_err(|_| MessageError::ProtocolViolated(StatusCode::ProtocolError))?;
            frame.mask();
            let fin = frame.header.fin;

            // avoid first allocation
            if frames.is_empty() && fin {
                return frame.try_into();
            }

            frames.push(frame);

            if fin {
                break;
            }
        }
        frames.try_into()
    }
}

impl<T: UnpinStream> WsSend for WsSendHalf<Client, T> {
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

impl<T: UnpinStream> WsRecv for WsStream<Server, T> {
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

impl<T: UnpinStream> WsSend for WsStream<Server, T> {
    async fn send_raw(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.tx.send_raw(data).await
    }

    async fn send(&mut self, message: Message) -> std::io::Result<()> {
        self.tx.send(message).await
    }
}

impl<T: UnpinStream> WsRecv for WsStream<Client, T> {
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

impl<T: UnpinStream> WsSend for WsStream<Client, T> {
    async fn send_raw(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.tx.send_raw(data).await
    }

    async fn send(&mut self, message: Message) -> std::io::Result<()> {
        self.tx.send(message).await
    }
}
