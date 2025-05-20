use serde::{Deserialize, Serialize};
use websocket::message::Message;

pub type Token = String;

#[non_exhaustive]
#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
pub enum ClientMessage {
    /// An auth request with a user's display name and its color.
    Auth(MessageSender),
    /// The `name` and `color` are provided automatically
    /// by the server.
    SimpleAuth,
    /// Token provided by [ServerMessage::AuthSuccess] and message text.
    /// Does not imply that the message will *actually* be sent.
    /// The client should only rely on [ServerMessage::PropagateMessage].
    SendMessage { token: Token, text: String },
}

impl From<ClientMessage> for Message {
    fn from(val: ClientMessage) -> Self {
        let mut buf = vec![];
        val.serialize(&mut rmp_serde::Serializer::new(&mut buf))
            .unwrap();
        Self::Binary(buf)
    }
}

impl TryFrom<Message> for ClientMessage {
    type Error = ();

    fn try_from(value: Message) -> Result<Self, Self::Error> {
        match value {
            Message::Binary(buf) => Ok(rmp_serde::from_slice(&buf).map_err(|_| ())?),
            _ => Err(()),
        }
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
pub enum ServerMessage {
    /// Whether the server accepts [ClientMessage::Auth].
    // TODO: Provide specific errors (invalid nickname, already occupied)
    AuthSuccess(Option<Token>),
    /// A chat message from either this client or any other.
    PropagateMessage(MessageSender, String),
    /// A message from server.
    ServerNotification(String),
    // TODO: Utilize these
    //
    /// A message about a new client being connected.
    ClientConnected(MessageSender),
    /// A message about a client being disconnected.
    ClientDisconnected(MessageSender),
}

impl From<ServerMessage> for Message {
    fn from(val: ServerMessage) -> Self {
        let mut buf = vec![];
        val.serialize(&mut rmp_serde::Serializer::new(&mut buf))
            .unwrap();
        Self::Binary(buf)
    }
}

impl TryFrom<Message> for ServerMessage {
    type Error = ();

    fn try_from(value: Message) -> Result<Self, Self::Error> {
        match value {
            Message::Binary(buf) => Ok(rmp_serde::from_slice(&buf).map_err(|_| ())?),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
pub struct MessageSender {
    pub name: String,
    pub color: Color,
}

#[non_exhaustive]
#[derive(Debug, Default, Clone, Copy, Hash, Serialize, Deserialize)]
pub enum Color {
    #[default]
    Text,
    Truecolor(u8, u8, u8),
    Red,
    Yellow,
    Green,
    Cyan,
    Blue,
    Magenta,
}
