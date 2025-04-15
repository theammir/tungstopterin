use crate::frame::{Frame, Opcode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusCode {
    Normal = 1000,
    GoingAway = 1001,
    ProtocolError = 1002,
    UnsupportedData = 1003,

    InvalidPayloadData = 1007,
    PolicyViolated = 1008,
    MessageTooBig = 1009,
    UnsupportedExtension = 1010,
    InternalServerError = 1011,
}

impl From<u16> for StatusCode {
    fn from(value: u16) -> Self {
        match value {
            1000 => Self::Normal,
            1001 => Self::GoingAway,
            1002 => Self::ProtocolError,
            1003 => Self::UnsupportedData,

            1007 => Self::InvalidPayloadData,
            1008 => Self::PolicyViolated,
            1009 => Self::MessageTooBig,
            1010 => Self::UnsupportedExtension,
            1011 => Self::InternalServerError,

            _ => Self::UnsupportedData,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Represents a frame with valid *UTF-8* text.
    Text(String),
    /// Represents a frame with any binary data.
    Binary(Vec<u8>),
    /// Represents a *Close* frame with an optional `String`
    /// up to 123 bytes long.
    /// Converting this to a [Frame] will truncate the `String` if needed.
    Close(StatusCode, Option<String>),
    /// Represents a *Ping* frame with 125-byte payload.
    /// Converting this to a [Frame] will truncate the payload if needed.
    Ping(Vec<u8>),
    /// Represents a *Pong* frame with 125-byte payload.
    /// Converting this to a [Frame] will truncate the payload if needed.
    Pong(Vec<u8>),
}

impl From<&Message> for Opcode {
    fn from(value: &Message) -> Self {
        match value {
            Message::Text(_) => Opcode::Text,
            Message::Binary(_) => Opcode::Binary,
            Message::Close(_, _) => Opcode::Close,
            Message::Ping(_) => Opcode::Ping,
            Message::Pong(_) => Opcode::Pong,
        }
    }
}

impl TryFrom<Frame> for Message {
    type Error = StatusCode;

    fn try_from(value: Frame) -> Result<Self, Self::Error> {
        match value.opcode {
            Opcode::Continue => Err(StatusCode::ProtocolError),
            Opcode::Text => Ok(Message::Text(
                String::from_utf8(value.payload).map_err(|_| StatusCode::InvalidPayloadData)?,
            )),
            Opcode::Binary => Ok(Message::Binary(value.payload)),
            Opcode::Close => Ok(Message::Close(
                (u16::from_be_bytes(value.payload[0..2].try_into().unwrap())).into(),
                {
                    if let Ok(string) = String::from_utf8(value.payload[2..].to_vec()) {
                        if !string.is_empty() {
                            Some(string)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                },
            )),
            Opcode::Ping => Ok(Message::Ping(value.payload)),
            Opcode::Pong => Ok(Message::Pong(value.payload)),
        }
    }
}

impl TryFrom<Vec<Frame>> for Message {
    type Error = StatusCode;

    fn try_from(value: Vec<Frame>) -> Result<Self, Self::Error> {
        if value.is_empty() {
            return Err(StatusCode::UnsupportedData);
        }
        if value[0].fin {
            return value.into_iter().next().unwrap().try_into();
        }

        let mut first = value[0].clone();
        let buffer: Vec<u8> = value
            .into_iter()
            .map(|frame| frame.payload)
            .reduce(|mut acc, payload| {
                acc.extend_from_slice(&payload);
                acc
            })
            .unwrap();
        first.fin = true;
        first.payload_len = buffer.len() as u64;
        first.payload = buffer;
        first.try_into()
    }
}

impl From<Message> for Frame {
    fn from(value: Message) -> Self {
        let opcode: Opcode = (&value).into();
        let payload: Vec<u8> = match value {
            Message::Text(text) => text.into(),
            Message::Binary(binary) => binary,
            Message::Close(code, reason) => {
                let mut vector =
                    Vec::with_capacity(reason.as_ref().map_or(0, |s| usize::max(123, s.len()) + 2));
                vector.extend((code as u16).to_be_bytes().iter());
                if let Some(s) = reason {
                    let mut s = s.into_bytes();
                    s.truncate(123);
                    vector.extend(s.iter());
                }
                vector
            }
            Message::Ping(mut binary) => {
                binary.truncate(125);
                binary
            }
            Message::Pong(mut binary) => {
                binary.truncate(125);
                binary
            }
        };
        Frame::new(true, opcode, payload)
    }
}
