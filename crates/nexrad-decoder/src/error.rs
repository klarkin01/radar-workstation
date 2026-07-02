use std::fmt;

#[derive(Debug)]
pub enum DecodeError {
    Truncated { context: &'static str },
    InvalidMagic { got: [u8; 4] },
    UnsupportedMessageType { got: u8 },
    InvalidBlockName { got: [u8; 4] },
    DecompressionFailed(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { context } => write!(f, "truncated data in {context}"),
            Self::InvalidMagic { got } => {
                write!(f, "invalid magic bytes: {got:02x?}")
            }
            Self::UnsupportedMessageType { got } => {
                write!(f, "unsupported message type: {got}")
            }
            Self::InvalidBlockName { got } => {
                write!(f, "invalid data block name: {got:02x?}")
            }
            Self::DecompressionFailed(msg) => write!(f, "decompression failed: {msg}"),
        }
    }
}

impl std::error::Error for DecodeError {}
