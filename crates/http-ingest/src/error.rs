#[derive(Debug)]
pub enum Error {
    HostNotAllowed(String),
    Connect(std::io::Error),
    Tls(std::io::Error),
    /// A framing violation. The `&'static str` names the specific rule that
    /// fired (e.g. `"bare LF"`, `"duplicate Content-Length"`), so tests and
    /// callers can match the exact rule rather than just the variant.
    Protocol(&'static str),
    Http { status: u16 },
    Timeout { phase: Phase },
    /// The peer closed a keepalive connection. Distinguished from `Connect` so
    /// callers can decide whether a retry is safe (see `Client`'s retry rule).
    Closed,
    BodyTooLarge { len: u64, limit: u64 },
    /// Rejected before any bytes were sent — e.g. CR/LF in a key.
    InvalidInput(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Connect,
    Tls,
    Headers,
    Body,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HostNotAllowed(host) => write!(f, "host not allowed: {host}"),
            Self::Connect(e) => write!(f, "connect failed: {e}"),
            Self::Tls(e) => write!(f, "TLS handshake failed: {e}"),
            Self::Protocol(rule) => write!(f, "protocol violation: {rule}"),
            Self::Http { status } => write!(f, "HTTP error status: {status}"),
            Self::Timeout { phase } => write!(f, "timed out during {phase:?}"),
            Self::Closed => write!(f, "connection closed by peer"),
            Self::BodyTooLarge { len, limit } => {
                write!(f, "response body too large: {len} bytes (limit {limit})")
            }
            Self::InvalidInput(reason) => write!(f, "invalid input: {reason}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Connect(e) | Self::Tls(e) => Some(e),
            _ => None,
        }
    }
}
