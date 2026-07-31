mod config;
mod connection;
mod encode;
mod error;
mod host;
mod request;
mod test_server;
mod tls;

/// Public only so the out-of-workspace fuzz target (`fuzz/fuzz_targets/parse_response.rs`)
/// can reach `parse_head` directly. Not part of the crate's supported API.
#[doc(hidden)]
pub mod response;

use std::sync::Arc;

use bytes::Bytes;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

use connection::{Attempt, Connection};
use host::Host;

pub use config::{ClientConfig, Limits, Timeouts};
pub use error::{Error, Phase};

pub struct Client {
    host: Host,
    /// Shared across reconnects so rustls' session cache survives a dropped
    /// connection — the whole mechanism behind session resumption.
    tls: Arc<rustls::ClientConfig>,
    cfg: ClientConfig,
    conn: Option<Connection<TlsStream<TcpStream>>>,
}

impl Client {
    pub fn new(host: &str) -> Result<Self, Error> {
        Self::with_config(host, ClientConfig::default())
    }

    pub fn with_config(host: &str, cfg: ClientConfig) -> Result<Self, Error> {
        let host = Host::parse(host)?;
        Ok(Self { host, tls: tls::build_config(), cfg, conn: None })
    }

    pub async fn list_prefix(
        &mut self,
        prefix: &str,
        start_after: Option<&str>,
        continuation_token: Option<&str>,
        delimiter: Option<&str>,
    ) -> Result<Bytes, Error> {
        let path = request::list_query(prefix, start_after, continuation_token, delimiter)?;
        self.send(&path).await
    }

    pub async fn get_object(&mut self, key: &str) -> Result<Bytes, Error> {
        let path = request::object_path(key)?;
        self.send(&path).await
    }

    async fn send(&mut self, path_and_query: &str) -> Result<Bytes, Error> {
        let req = request::format_request(self.host.as_str(), path_and_query);

        let reused = self.conn.is_some();
        if self.conn.is_none() {
            self.conn = Some(Connection::connect(&self.host, &self.tls, &self.cfg).await?);
        }

        match self.attempt(&req).await {
            Attempt::Success(bytes) => Ok(bytes),
            Attempt::Failed(e) => Err(e),
            // Retry exactly once, on a fresh connection, and only when the
            // connection that failed was reused rather than freshly opened
            // (see docs/plans/0014-http-ingest-implementation.md §1.1). A
            // reused connection closing before any response byte means the
            // server never processed the request, so this cannot double-deliver.
            Attempt::ClosedEmpty if reused => {
                self.conn = Some(Connection::connect(&self.host, &self.tls, &self.cfg).await?);
                match self.attempt(&req).await {
                    Attempt::Success(bytes) => Ok(bytes),
                    Attempt::Failed(e) => Err(e),
                    Attempt::ClosedEmpty => Err(Error::Closed),
                }
            }
            Attempt::ClosedEmpty => Err(Error::Closed),
        }
    }

    async fn attempt(&mut self, req: &[u8]) -> Attempt {
        let conn = self.conn.as_mut().expect("connection established by caller");
        let outcome = conn.round_trip(req, &self.cfg.limits, &self.cfg.timeouts).await;
        if !conn.reusable {
            self.conn = None;
        }
        outcome
    }
}
