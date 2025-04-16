pub mod frame;
pub mod handshake;
pub mod message;

use std::{io::ErrorKind, marker::PhantomData};
use tokio::{
    io::AsyncReadExt,
    net::{
        TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
};

use crate::{
    frame::Frame,
    message::{Message, StatusCode},
};

async fn read_from_stream<R>(stream: &mut R) -> std::io::Result<Vec<u8>>
where
    R: AsyncReadExt + Unpin,
{
    loop {
        let mut buf = [0_u8; 4096];
        match stream.read(&mut buf).await {
            Ok(n) => break Ok(buf[..n].to_vec()),
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => continue,
            Err(e) => break Err(e),
        }
    }
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
    async fn read_raw(&mut self) -> std::io::Result<Vec<u8>>;
    async fn receive(&mut self) -> Result<Message, StatusCode>;
}

impl WsRecv for WsRecvHalf<Server> {
    async fn read_raw(&mut self) -> std::io::Result<Vec<u8>> {
        read_from_stream(&mut self.0).await
    }

    async fn receive(&mut self) -> Result<Message, StatusCode> {
        let data = self
            .read_raw()
            .await
            .map_err(|_| StatusCode::InternalServerError)?;

        {
            let frame: Frame = data.try_into().map_err(|_| StatusCode::ProtocolError)?;
            frame.try_into()
        }
    }
}

impl WsSend for WsSendHalf<Server> {
    async fn send_raw(&mut self, data: &[u8]) -> std::io::Result<()> {
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
        self.send_raw(&binary).await
    }
}

impl WsRecv for WsRecvHalf<Client> {
    async fn read_raw(&mut self) -> std::io::Result<Vec<u8>> {
        read_from_stream(&mut self.0).await
    }

    async fn receive(&mut self) -> Result<Message, StatusCode> {
        let data = self
            .read_raw()
            .await
            .map_err(|_| StatusCode::InternalServerError)?;

        {
            let mut frame: Frame = data.try_into().map_err(|_| StatusCode::ProtocolError)?;
            frame.mask();
            frame.try_into()
        }
    }
}

impl WsSend for WsSendHalf<Client> {
    async fn send_raw(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.0.writable().await?;
        self.0.try_write(data)?;
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
    async fn read_raw(&mut self) -> std::io::Result<Vec<u8>> {
        self.rx.read_raw().await
    }

    async fn receive(&mut self) -> Result<Message, StatusCode> {
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
    async fn read_raw(&mut self) -> std::io::Result<Vec<u8>> {
        self.rx.read_raw().await
    }

    async fn receive(&mut self) -> Result<Message, StatusCode> {
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
