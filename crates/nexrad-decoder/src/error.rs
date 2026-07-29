use std::fmt;

#[derive(Debug)]
pub enum DecodeError {
    Truncated { context: &'static str },
    UnknownRadialStatus { got: u8 },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { context } => write!(f, "truncated data in {context}"),
            Self::UnknownRadialStatus { got } => {
                write!(f, "unknown radial status code: {got}")
            }
        }
    }
}

impl std::error::Error for DecodeError {}
