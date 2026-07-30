use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub connect: Duration,
    pub tls_handshake: Duration,
    pub response_headers: Duration,
    pub body: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(5),
            tls_handshake: Duration::from_secs(5),
            response_headers: Duration::from_secs(10),
            body: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_head_bytes: usize,
    pub max_header_count: usize,
    pub max_body_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_head_bytes: 64 * 1024,
            max_header_count: 128,
            max_body_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ClientConfig {
    pub timeouts: Timeouts,
    pub limits: Limits,
}
