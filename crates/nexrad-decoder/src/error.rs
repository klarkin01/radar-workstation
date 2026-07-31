use std::fmt;

#[derive(Debug)]
pub enum DecodeError {
    Truncated { context: &'static str },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { context } => write!(f, "truncated data in {context}"),
        }
    }
}

impl std::error::Error for DecodeError {}
