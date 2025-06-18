use serde::{Deserialize, Serialize};
use websocket::message::Message;

pub type Token = String;

pub const NICKNAME_MAX_LEN: usize = 16;

#[non_exhaustive]
#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
pub enum ClientMessage {
    /// An auth request with a user's display name and its color.
    Auth(MessageSender),
    /// Token provided by [`ServerMessage::AuthSuccess`] and message text.
    /// Does not imply that the message will *actually* be sent.
    /// The client should only rely on [`ServerMessage::PropagateMessage`].
    SendMessage { token: Token, text: String },
}

#[non_exhaustive]
#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
pub enum ServerMessage {
    /// Whether the server accepts [`ClientMessage::Auth`].
    AuthSuccess(Result<Token, AuthError>),
    /// A chat message from either this client or any other.
    PropagateMessage(MessageSender, String),
    /// Any kind of notification issued by the server.
    Notification(ServerNotification),
}

#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
pub enum AuthError {
    /// Nickname already used or otherwise unavailable.
    NicknameUnavailable,
    /// Nickname length exceeds the predefined maximum length.
    NicknameTooLong,
    /// The user sending [`ClientMessage::Auth`] is already authenticated.
    AlreadyAuthorized,
}

#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
pub enum ServerNotification {
    /// Literal message from the server.
    Literal(String),
    /// A message about a new client being connected.
    ClientConnected(MessageSender),
    /// A message about a client being disconnected.
    ClientDisconnected(MessageSender),
}

impl From<ClientMessage> for Message {
    fn from(val: ClientMessage) -> Self {
        let mut buf = vec![];
        val.serialize(&mut rmp_serde::Serializer::new(&mut buf))
            .unwrap();
        Self::Binary(buf)
    }
}

impl TryFrom<&Message> for ClientMessage {
    type Error = ();

    fn try_from(value: &Message) -> Result<Self, Self::Error> {
        match value {
            Message::Binary(buf) => Ok(rmp_serde::from_slice(buf).map_err(|_| ())?),
            _ => Err(()),
        }
    }
}

impl From<ServerMessage> for Message {
    fn from(val: ServerMessage) -> Self {
        let mut buf = vec![];
        val.serialize(&mut rmp_serde::Serializer::new(&mut buf))
            .unwrap();
        Self::Binary(buf)
    }
}

impl TryFrom<&Message> for ServerMessage {
    type Error = ();

    fn try_from(value: &Message) -> Result<Self, Self::Error> {
        match value {
            Message::Binary(buf) => Ok(rmp_serde::from_slice(buf).map_err(|_| ())?),
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
