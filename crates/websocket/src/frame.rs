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

#[derive(Debug, Clone)]
pub struct Frame {
    pub fin: bool,
    /// Only 3 least significant bits count.
    /// Should really remain all 0s for the purposes of this lib.
    pub rsv: u8,
    pub opcode: Opcode,
    pub payload_len: u64,
    pub masking_key: Option<u32>,
    pub payload: Vec<u8>,
}

impl Frame {
    /// Creates a new `Frame` with an initialized masking key.
    /// No actual masking is done, and is the responsibility of the caller,
    /// see [Frame::mask].
    pub fn new(fin: bool, opcode: Opcode, payload: Vec<u8>) -> Self {
        Frame {
            fin,
            rsv: 0,
            opcode,
            payload_len: payload.len() as u64,
            masking_key: Some(rand::rng().next_u32()),
            payload,
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

impl From<Frame> for Vec<u8> {
    fn from(value: Frame) -> Self {
        const U16_MAX: u64 = u16::MAX as u64;
        let mut result = Vec::with_capacity(
            value.payload_len as usize + 2 + if value.masking_key.is_some() { 4 } else { 0 },
        );

        let first_bit =
            ((value.fin as u8) << 7) | ((value.rsv & 0b00000111) << 4) | value.opcode as u8;
        result.push(first_bit);

        let mut second_bit = (value.masking_key.is_some() as u8) << 7;
        match value.payload_len {
            0..=125 => {
                second_bit |= value.payload_len as u8;
                result.push(second_bit);
            }
            126..=U16_MAX => {
                second_bit |= 126;
                result.push(second_bit);
                result.extend_from_slice(&((value.payload_len as u16).to_be_bytes()));
            }
            _ => {
                second_bit |= 127;
                result.push(second_bit);
                result.extend_from_slice(&value.payload_len.to_be_bytes());
            }
        }

        if let Some(key) = value.masking_key {
            result.extend_from_slice(&key.to_be_bytes());
        }

        result.extend_from_slice(&value.payload);

        result
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    TooShort,
    InvalidOpcode,
    LengthParsing,
    MaskingKeyParsing,
}

impl TryFrom<Vec<u8>> for Frame {
    type Error = FrameError;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        if value.len() < 2 {
            return Err(FrameError::TooShort);
        }

        let mut last_len_byte: usize = 1;
        let payload_len: u64 = {
            let len_header = value[1] & 0b01111111;
            match len_header {
                0..=125 => len_header as u64,
                126 => {
                    const U16_LEN: usize = 2;
                    last_len_byte += U16_LEN;
                    u16::from_be_bytes(
                        value[2..][..U16_LEN]
                            .try_into()
                            .map_err(|_| FrameError::LengthParsing)?,
                    ) as u64
                }
                127 => {
                    const U64_LEN: usize = 8;
                    last_len_byte += U64_LEN;
                    u64::from_be_bytes(
                        value[2..][..U64_LEN]
                            .try_into()
                            .map_err(|_| FrameError::LengthParsing)?,
                    )
                }
                _ => unreachable!(),
            }
        };

        const U32_LEN: usize = 4;
        let mut last_key_byte: usize = last_len_byte + 1;
        let masking_key = (value[1] >> 7 != 0).then(|| {
            // TODO: FrameError::MaskingKeyParsing
            last_key_byte += U32_LEN;
            u32::from_be_bytes(
                value[last_key_byte - U32_LEN..last_key_byte]
                    .try_into()
                    .unwrap(),
            )
        });

        Ok(Self {
            fin: (value[0] >> 7) != 0,
            rsv: (value[0] & 0b01110000) >> 4,
            opcode: Opcode::try_from(value[0] & 0b00001111)
                .map_err(|_| FrameError::InvalidOpcode)?,
            payload_len,
            masking_key,
            payload: {
                let mut payload = value[last_key_byte..].to_vec();
                payload.truncate(payload_len as usize);
                payload
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Frame, Opcode};

    #[test]
    fn unmasked_64bit_frame_into_bytes() {
        let unmasked_long = Frame {
            fin: false,
            rsv: 0,
            opcode: Opcode::Binary,
            payload_len: 69420,
            masking_key: None,
            payload: vec![0xde, 0xad, 0xbe, 0xef],
        };
        let bytes: Vec<u8> = unmasked_long.clone().into();

        println!("Unmasked 64-bit length: {:?}", unmasked_long);
        for byte in &bytes {
            print!("{:08b} ", byte);
        }
        println!("\n");

        assert_eq!(bytes[0] >> 7, 0, "incorrect FIN bit");
        assert_eq!((bytes[0] & 0b01110000) >> 4, 0, "incorrect RSV bits");
        assert_eq!(bytes[1] >> 7, 0, "incorrect masked bit");
        assert_eq!(bytes[1] & 0b01111111, 127, "incorrect payload length");
    }

    #[test]
    fn masked_7bit_frame_into_bytes() {
        let mut masked_7bit = Frame {
            fin: true,
            rsv: 3,
            opcode: Opcode::Continue,
            payload_len: 3,
            masking_key: Some(12345),
            payload: vec![0xff, 0x00, 0xff],
        };
        let bytes: Vec<u8> = masked_7bit.clone().into();

        println!("Yet to be masked 7-bit length: {:?}", masked_7bit);
        for byte in &bytes {
            print!("{:08b} ", byte);
        }
        println!();

        assert_eq!(masked_7bit.payload[2], 0xff);

        masked_7bit.mask();
        let bytes: Vec<u8> = masked_7bit.clone().into();
        println!("Masked:");
        for byte in &bytes {
            print!("{:08b} ", byte);
        }
        println!();

        assert_eq!(masked_7bit.payload[2], 0xcf, "invalid masked payload");

        assert_eq!(bytes[0] >> 7, 1, "incorrect FIN bit");
        assert_eq!((bytes[0] & 0b01110000) >> 4, 3, "incorrect RSV bits");
        assert_eq!(bytes[1] >> 7, 1, "incorrect masked bit");
        assert_eq!(bytes[1] & 0b01111111, 3, "incorrect payload length");
    }

    #[test]
    fn unmasked_64bit_raw_into_frame() {
        let unmasked_long_bytes = vec![2_u8, 127, 0, 0, 0, 0, 0, 1, 15, 44, 222, 173, 190, 239];
        println!("Raw unmasked 64-bit length:");
        for byte in &unmasked_long_bytes {
            print!("{:08b} ", byte);
        }
        println!();

        let frame: Frame = unmasked_long_bytes.try_into().unwrap();
        println!("Reconstructed: {:?}\n", frame);

        assert!(!frame.fin, "incorrect FIN bit");
        assert_eq!(frame.rsv, 0, "incorrect RSV bits");
        assert!(frame.masking_key.is_none(), "incorrect masked bit");
        assert_eq!(frame.payload_len, 69420, "incorrect payload length");
    }

    #[test]
    fn masked_7bit_raw_into_frame() {
        let masked_7bit_bytes = vec![176, 131, 0, 0, 48, 57, 255, 0, 207];
        println!("Raw masked 7-bit length:");
        for byte in &masked_7bit_bytes {
            print!("{:08b} ", byte);
        }
        println!();

        let mut frame: Frame = masked_7bit_bytes.try_into().unwrap();

        assert_eq!(frame.payload[2], 0xcf, "invalid masked payload");
        frame.mask();
        assert_eq!(frame.payload[2], 0xff, "invalid unmasked payload");

        println!("Unmasked 7-bit: {:?}", frame);

        assert!(frame.fin, "incorrect FIN bit");
        assert_eq!(frame.rsv, 3, "incorrect RSV bits");
        assert!(frame.masking_key.is_some(), "incorrect masked bit");
        assert_eq!(frame.payload_len, 3, "incorrect payload length");
    }
}
