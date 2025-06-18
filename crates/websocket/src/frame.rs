use rand::RngCore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    Continue = 0,
    Text = 1,
    Binary = 2,

    Close = 8,
    Ping = 9,
    Pong = 10,
}

pub struct InvalidOpcode;

impl TryFrom<u8> for Opcode {
    type Error = InvalidOpcode;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Continue),
            1 => Ok(Self::Text),
            2 => Ok(Self::Binary),
            8 => Ok(Self::Close),
            9 => Ok(Self::Ping),
            10 => Ok(Self::Pong),
            _ => Err(InvalidOpcode),
        }
    }
}

/// A length of [Frame] payload. Due to the header being possibly partially parsed,
/// can hold not only actual len, but also hints to parse next u16 or u64.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadLen {
    /// The value of the 7 length bits
    ExactU8(u8),
    /// The value of the 2 extra length bytes
    ExactU16(u16),
    /// The value of the 8 extra length bytes
    ExactU64(u64),
    /// Corresponds to length bits set to 126.
    /// Contains no actual length info.
    HintU16,
    /// Corresponds to length bits set to 127.
    /// Contains no actual length info.
    HintU64,
}

impl From<u64> for PayloadLen {
    fn from(value: u64) -> Self {
        const U8_MAX: u64 = 125u64;
        const U16_MAX: u64 = u16::MAX as u64;
        const U64_MAX: u64 = u64::MAX;
        #[allow(clippy::cast_possible_truncation)]
        #[allow(clippy::match_overlapping_arm)] // idk it's kinda readable
        // in case it's not, the smallest possible variant is prioritized
        match value {
            0..=U8_MAX => Self::ExactU8(value as u8),
            0..=U16_MAX => Self::ExactU16(value as u16),
            0..=U64_MAX => Self::ExactU64(value),
        }
    }
}

/// [Frame] header that can be parsed from the first 2 bytes of it.
/// If the length is 126 or 127, respective [`PayloadLen`] hint will be assigned.
/// Enough bytes in the slice will convert to instance with exact length of the smallest possible
/// unsigned int size.
#[derive(Debug, Clone, Copy)]
pub struct FrameHeader {
    pub fin: bool,
    /// Only 3 rightmost bits count: RSV1 RSV2 RSV3 in BE order.
    // Honestly it just sounds like a better idea to use 3 bools now.
    /// Should really remain all 0s for the purposes of this lib.
    pub rsv: u8,
    pub opcode: Opcode,
    // Super Rustacean API of bool + Option
    pub masked: bool,
    pub payload_len: PayloadLen,
}

/// WebSocket Frame consisting of a [`FrameHeader`], a payload, and an optional masking key.
#[derive(Debug, Clone)]
pub struct Frame {
    pub header: FrameHeader,
    pub masking_key: Option<u32>,
    pub payload: Vec<u8>,
}

impl FrameHeader {
    /// Creates a new [`FrameHeader`] with an initialized masking key.
    /// No actual masking is done, and is the responsibility of the caller,
    /// see [`Frame::mask`].
    #[must_use]
    pub fn new(fin: bool, opcode: Opcode, masked: bool, payload_len: u64) -> Self {
        FrameHeader {
            fin,
            rsv: 0,
            opcode,
            masked,
            payload_len: payload_len.into(),
        }
    }
}

impl Frame {
    /// Creates a new [Frame] with an initialized masking key.
    /// No actual masking is done, and is the responsibility of the caller,
    /// see [`Frame::mask`].
    #[must_use]
    pub fn new(fin: bool, opcode: Opcode, payload: Vec<u8>) -> Self {
        Frame {
            header: FrameHeader::new(fin, opcode, true, payload.len() as u64),
            payload,
            masking_key: Some(rand::rng().next_u32()),
        }
    }

    /// Masks the payload.
    /// The operation is *involutory*, meaning that unmasking is done
    /// through this method as well.
    /// # Panics
    /// Panics if `masking_key` is *None*.
    pub fn mask(&mut self) {
        let key = self.masking_key.unwrap();

        for (index, byte) in self.payload.iter_mut().enumerate() {
            *byte ^= key.to_be_bytes()[index % 4];
        }
    }
}

impl From<FrameHeader> for Vec<u8> {
    fn from(value: FrameHeader) -> Self {
        let mut result = Vec::with_capacity(2 + if value.masked { 4 } else { 0 });

        let first_bit =
            (u8::from(value.fin) << 7) | ((value.rsv & 0b0000_0111) << 4) | value.opcode as u8;
        result.push(first_bit);

        let mut second_bit = u8::from(value.masked) << 7;
        match value.payload_len {
            PayloadLen::ExactU8(len) => {
                second_bit |= len;
                result.push(second_bit);
            }
            PayloadLen::ExactU16(len) => {
                second_bit |= 126;
                result.push(second_bit);
                result.extend_from_slice(&((len).to_be_bytes()));
            }
            PayloadLen::ExactU64(len) => {
                second_bit |= 127;
                result.push(second_bit);
                result.extend_from_slice(&len.to_be_bytes());
            }
            PayloadLen::HintU16 => {
                second_bit |= 126;
                result.push(second_bit);
            }
            PayloadLen::HintU64 => {
                second_bit |= 127;
                result.push(second_bit);
            }
        }

        result
    }
}

// TEST: these
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    FrameTooShort,
    InvalidOpcode,
    LengthParsing,
    MaskingKeyParsing,
    PayloadTooShort,
}

impl TryFrom<&[u8]> for FrameHeader {
    type Error = FrameError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        if value.len() < 2 {
            return Err(FrameError::FrameTooShort);
        }

        let payload_len = {
            let len_header = value[1] & 0b0111_1111;
            match len_header {
                0..=125 => PayloadLen::ExactU8(len_header),
                126 => {
                    const U16_LEN: usize = 2;
                    value
                        .get(2..2 + U16_LEN)
                        .map_or(PayloadLen::HintU16, |slice| {
                            PayloadLen::ExactU16(u16::from_be_bytes(slice.try_into().unwrap()))
                        })
                }
                127 => {
                    const U64_LEN: usize = 8;
                    value
                        .get(2..2 + U64_LEN)
                        .map_or(PayloadLen::HintU64, |slice| {
                            PayloadLen::ExactU64(u64::from_be_bytes(slice.try_into().unwrap()))
                        })
                }
                _ => unreachable!(),
            }
        };

        Ok(Self {
            fin: (value[0] >> 7) != 0,
            rsv: (value[0] & 0b0111_0000) >> 4,
            opcode: Opcode::try_from(value[0] & 0b0000_1111)
                .map_err(|_| FrameError::InvalidOpcode)?,
            masked: (value[1] >> 7) != 0,
            payload_len,
        })
    }
}

impl From<Frame> for Vec<u8> {
    fn from(value: Frame) -> Self {
        let mut header: Vec<u8> = value.header.into();
        if let Some(key) = value.masking_key {
            header.extend_from_slice(&key.to_be_bytes());
        }
        header.extend(value.payload);
        header
    }
}

impl TryFrom<Vec<u8>> for Frame {
    type Error = FrameError;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        const MASKING_KEY_LEN: usize = 4;
        let header: FrameHeader = value.as_slice().try_into()?;
        let masking_key_index = match header.payload_len {
            PayloadLen::ExactU8(_) => 2,
            PayloadLen::ExactU16(_) => 4,
            PayloadLen::ExactU64(_) => 10,
            _ => Err(FrameError::LengthParsing)?,
        };
        let masking_key = (header.masked)
            .then(|| {
                value
                    .get(masking_key_index..masking_key_index + MASKING_KEY_LEN)
                    .ok_or(FrameError::MaskingKeyParsing)
                    .map(|bytes| <[u8; 4]>::try_from(bytes).unwrap())
                    .map(u32::from_be_bytes)
            })
            .transpose()?;
        Ok(Frame {
            header,
            masking_key,
            payload: value
                .get(masking_key_index + MASKING_KEY_LEN..)
                .ok_or(FrameError::PayloadTooShort)?
                .to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::frame::{Frame, PayloadLen};

    use super::{FrameHeader, Opcode};

    #[test]
    fn unmasked_64bit_frame_into_bytes() {
        let unmasked_long = Frame {
            header: FrameHeader {
                fin: false,
                rsv: 0,
                opcode: Opcode::Binary,
                masked: false,
                payload_len: PayloadLen::ExactU64(69420),
            },
            masking_key: None,
            payload: vec![0xde, 0xad, 0xbe, 0xef],
        };
        let bytes: Vec<u8> = unmasked_long.clone().into();

        println!("Unmasked 64-bit length: {unmasked_long:?}");
        for byte in &bytes {
            print!("{byte:08b} ");
        }
        println!("\n");

        assert_eq!(bytes[0] >> 7, 0, "incorrect FIN bit");
        assert_eq!((bytes[0] & 0b0111_0000) >> 4, 0, "incorrect RSV bits");
        assert_eq!(bytes[1] >> 7, 0, "incorrect masked bit");
        assert_eq!(bytes[1] & 0b0111_1111, 127, "incorrect payload length");
    }

    #[test]
    fn masked_7bit_frame_into_bytes() {
        let mut masked_7bit = Frame {
            header: FrameHeader {
                fin: true,
                rsv: 3,
                opcode: Opcode::Continue,
                masked: true,
                payload_len: PayloadLen::ExactU8(3),
            },
            masking_key: Some(12345),
            payload: vec![0xff, 0x00, 0xff],
        };
        let bytes: Vec<u8> = masked_7bit.clone().into();

        println!("Yet to be masked 7-bit length: {masked_7bit:?}");
        for byte in &bytes {
            print!("{byte:08b} ");
        }
        println!();

        assert_eq!(masked_7bit.payload[2], 0xff);

        masked_7bit.mask();
        let bytes: Vec<u8> = masked_7bit.clone().into();
        println!("Masked:");
        for byte in &bytes {
            print!("{byte:08b} ");
        }
        println!();

        assert_eq!(masked_7bit.payload[2], 0xcf, "invalid masked payload");

        assert_eq!(bytes[0] >> 7, 1, "incorrect FIN bit");
        assert_eq!((bytes[0] & 0b0111_0000) >> 4, 3, "incorrect RSV bits");
        assert_eq!(bytes[1] >> 7, 1, "incorrect masked bit");
        assert_eq!(bytes[1] & 0b0111_1111, 3, "incorrect payload length");
    }

    #[test]
    fn unmasked_64bit_raw_into_frame() {
        let unmasked_long_bytes = vec![2_u8, 127, 0, 0, 0, 0, 0, 0, 0, 4, 222, 173, 190, 239];
        println!("Raw unmasked 64-bit length:");
        for byte in &unmasked_long_bytes {
            print!("{byte:08b} ");
        }
        println!();

        let frame: Frame = unmasked_long_bytes.try_into().unwrap();
        println!("Reconstructed: {frame:?}\n");

        assert!(!frame.header.fin, "incorrect FIN bit");
        assert_eq!(frame.header.rsv, 0, "incorrect RSV bits");
        assert!(!frame.header.masked, "incorrect masked bit");
        assert_eq!(
            frame.header.payload_len,
            PayloadLen::ExactU64(4),
            "incorrect payload length"
        );
    }

    #[test]
    fn masked_7bit_raw_into_frame() {
        let masked_7bit_bytes = vec![176, 131, 0, 0, 48, 57, 255, 0, 207];
        println!("Raw masked 7-bit length:");
        for byte in &masked_7bit_bytes {
            print!("{byte:08b} ");
        }
        println!();

        let mut frame: Frame = masked_7bit_bytes.try_into().unwrap();

        assert_eq!(frame.payload[2], 0xcf, "invalid masked payload");
        frame.mask();
        assert_eq!(frame.payload[2], 0xff, "invalid unmasked payload");

        println!("Unmasked 7-bit: {frame:?}");

        assert!(frame.header.fin, "incorrect FIN bit");
        assert_eq!(frame.header.rsv, 3, "incorrect RSV bits");
        assert!(!frame.header.masked, "incorrect masked bit");
        assert_eq!(
            frame.header.payload_len,
            PayloadLen::ExactU8(3),
            "incorrect payload length"
        );
    }
}
