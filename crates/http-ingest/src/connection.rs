use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

use crate::config::{ClientConfig, Limits, Timeouts};
use crate::error::{Error, Phase};
use crate::host::Host;
use crate::response::{parse_chunk_size_line, parse_head, BodyFraming, Head};
use crate::tls;

/// Bodies of non-2xx responses are read and discarded (to keep the
/// connection reusable) only up to this size; a larger error body isn't
/// worth the read, so the connection is dropped instead.
const DISCARD_CAP: u64 = 8 * 1024;
const READ_CHUNK: usize = 8 * 1024;

/// Read buffer plus the underlying stream. Generic so production can
/// instantiate `Wire<TlsStream<TcpStream>>` while `#[cfg(test)]` code
/// instantiates `Wire<TcpStream>` against a plaintext loopback server —
/// the one seam that lets D-d avoid a feature flag.
pub(crate) struct Wire<S> {
    stream: S,
    rbuf: BytesMut,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Wire<S> {
    fn new(stream: S) -> Self {
        Self { stream, rbuf: BytesMut::new() }
    }

    /// Reads more bytes from the socket into `rbuf`. `Ok(0)` means EOF.
    async fn read_more(&mut self) -> std::io::Result<usize> {
        let mut tmp = [0u8; READ_CHUNK];
        let n = self.stream.read(&mut tmp).await?;
        if n > 0 {
            self.rbuf.extend_from_slice(&tmp[..n]);
        }
        Ok(n)
    }
}

pub(crate) struct Connection<S> {
    wire: Wire<S>,
    /// Whether this connection may be reused for another request. Set to
    /// `false` on any error other than a discarded small non-2xx body.
    pub(crate) reusable: bool,
}

/// Outcome of one request attempt, as `Client` needs it to implement the
/// D-a/1.1 retry rule without `Connection` knowing anything about retries.
pub(crate) enum Attempt {
    Success(Bytes),
    /// The peer closed the connection before any response byte arrived.
    /// Always safe to retry on a fresh connection — the request was never
    /// processed.
    ClosedEmpty,
    Failed(Error),
}

/// Internal outcome of a read step, before it's known whether any bytes were
/// observed for *this* response (which determines `ClosedEmpty` eligibility).
enum Internal {
    Timeout(Phase),
    /// `bool` is whether any response bytes had already been observed when
    /// this I/O error occurred.
    Io(bool),
    /// An error with its own well-defined `Error` variant (protocol
    /// violation, body-too-large, ...).
    Fatal(Error),
}

impl Connection<TlsStream<TcpStream>> {
    pub(crate) async fn connect(
        host: &Host,
        tls_config: &Arc<rustls::ClientConfig>,
        cfg: &ClientConfig,
    ) -> Result<Self, Error> {
        let addr = format!("{}:443", host.as_str());
        let tcp = timeout(cfg.timeouts.connect, TcpStream::connect(addr))
            .await
            .map_err(|_| Error::Timeout { phase: Phase::Connect })?
            .map_err(Error::Connect)?;
        tcp.set_nodelay(true).map_err(Error::Connect)?;

        let connector = TlsConnector::from(tls_config.clone());
        let server_name = tls::server_name(host.as_str())?;
        let tls_stream = timeout(cfg.timeouts.tls_handshake, connector.connect(server_name, tcp))
            .await
            .map_err(|_| Error::Timeout { phase: Phase::Tls })?
            .map_err(Error::Tls)?;

        Ok(Self { wire: Wire::new(tls_stream), reusable: true })
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Connection<S> {
    pub(crate) async fn round_trip(
        &mut self,
        request: &[u8],
        limits: &Limits,
        timeouts: &Timeouts,
    ) -> Attempt {
        let mut any_bytes = false;

        if self.wire.stream.write_all(request).await.is_err()
            || self.wire.stream.flush().await.is_err()
        {
            self.reusable = false;
            return Attempt::ClosedEmpty;
        }

        let head = match self.read_head(limits, timeouts.response_headers, &mut any_bytes).await {
            Ok(head) => head,
            Err(internal) => return self.finish_with_internal(internal),
        };
        self.wire.rbuf.advance(head.head_len);

        let is_2xx = (200..300).contains(&head.status);
        let cap = if is_2xx { limits.max_body_bytes } else { DISCARD_CAP.min(limits.max_body_bytes) };

        match self.read_body(head.framing, cap, timeouts.body, &mut any_bytes).await {
            Ok(bytes) => {
                self.reusable = !head.connection_close;
                if is_2xx {
                    Attempt::Success(bytes)
                } else {
                    Attempt::Failed(Error::Http { status: head.status })
                }
            }
            // A non-2xx body bigger than the discard cap: not worth reading
            // further. Drop the connection but surface the real HTTP
            // status, not BodyTooLarge — the cap here is a reuse heuristic,
            // not the actual protocol limit.
            Err(Internal::Fatal(Error::BodyTooLarge { .. })) if !is_2xx => {
                self.reusable = false;
                Attempt::Failed(Error::Http { status: head.status })
            }
            Err(internal) => self.finish_with_internal(internal),
        }
    }

    fn finish_with_internal(&mut self, internal: Internal) -> Attempt {
        self.reusable = false;
        match internal {
            Internal::Timeout(phase) => Attempt::Failed(Error::Timeout { phase }),
            Internal::Io(false) => Attempt::ClosedEmpty,
            Internal::Io(true) => Attempt::Failed(Error::Closed),
            Internal::Fatal(e) => Attempt::Failed(e),
        }
    }

    async fn read_head(
        &mut self,
        limits: &Limits,
        dur: Duration,
        any_bytes: &mut bool,
    ) -> Result<Head, Internal> {
        loop {
            if let Some(head) = parse_head(&self.wire.rbuf, limits).map_err(Internal::Fatal)? {
                return Ok(head);
            }
            let n = timeout(dur, self.wire.read_more())
                .await
                .map_err(|_| Internal::Timeout(Phase::Headers))?
                .map_err(|_| Internal::Io(*any_bytes))?;
            if n == 0 {
                return Err(Internal::Io(*any_bytes));
            }
            *any_bytes = true;
        }
    }

    async fn read_body(
        &mut self,
        framing: BodyFraming,
        max_body_bytes: u64,
        dur: Duration,
        any_bytes: &mut bool,
    ) -> Result<Bytes, Internal> {
        match framing {
            BodyFraming::None => Ok(Bytes::new()),
            BodyFraming::ContentLength(len) => {
                if len > max_body_bytes {
                    return Err(Internal::Fatal(Error::BodyTooLarge { len, limit: max_body_bytes }));
                }
                self.read_exact_body(len as usize, dur, any_bytes).await
            }
            BodyFraming::Chunked => self.read_chunked_body(max_body_bytes, dur, any_bytes).await,
        }
    }

    async fn fill_to(&mut self, needed: usize, dur: Duration, any_bytes: &mut bool) -> Result<(), Internal> {
        while self.wire.rbuf.len() < needed {
            let n = timeout(dur, self.wire.read_more())
                .await
                .map_err(|_| Internal::Timeout(Phase::Body))?
                .map_err(|_| Internal::Io(*any_bytes))?;
            if n == 0 {
                return Err(Internal::Io(*any_bytes));
            }
            *any_bytes = true;
        }
        Ok(())
    }

    async fn read_exact_body(
        &mut self,
        len: usize,
        dur: Duration,
        any_bytes: &mut bool,
    ) -> Result<Bytes, Internal> {
        self.fill_to(len, dur, any_bytes).await?;
        // Exactly `len` bytes; trailing bytes belong to the next response on
        // a keepalive connection and must stay in the read buffer.
        Ok(self.wire.rbuf.split_to(len).freeze())
    }

    async fn read_chunked_body(
        &mut self,
        max_body_bytes: u64,
        dur: Duration,
        any_bytes: &mut bool,
    ) -> Result<Bytes, Internal> {
        let mut out = BytesMut::new();
        loop {
            let (size, consumed) = loop {
                match parse_chunk_size_line(&self.wire.rbuf) {
                    Ok(Some(v)) => break v,
                    Ok(None) => {
                        let n = timeout(dur, self.wire.read_more())
                            .await
                            .map_err(|_| Internal::Timeout(Phase::Body))?
                            .map_err(|_| Internal::Io(*any_bytes))?;
                        if n == 0 {
                            return Err(Internal::Io(*any_bytes));
                        }
                        *any_bytes = true;
                    }
                    Err(e) => return Err(Internal::Fatal(e)),
                }
            };
            self.wire.rbuf.advance(consumed);

            if size == 0 {
                // No chunk trailers supported (see response.rs) — the
                // terminator is exactly one more CRLF.
                self.fill_to(2, dur, any_bytes).await?;
                if &self.wire.rbuf[..2] != b"\r\n" {
                    return Err(Internal::Fatal(Error::Protocol("chunk trailers not supported")));
                }
                self.wire.rbuf.advance(2);
                return Ok(out.freeze());
            }

            let new_total = out.len() as u64 + size;
            if new_total > max_body_bytes {
                return Err(Internal::Fatal(Error::BodyTooLarge { len: new_total, limit: max_body_bytes }));
            }

            self.fill_to(size as usize + 2, dur, any_bytes).await?;
            let chunk = self.wire.rbuf.split_to(size as usize);
            if &self.wire.rbuf[..2] != b"\r\n" {
                return Err(Internal::Fatal(Error::Protocol("malformed chunk terminator")));
            }
            self.wire.rbuf.advance(2);
            out.extend_from_slice(&chunk);
        }
    }
}

#[cfg(test)]
impl Connection<TcpStream> {
    /// Plaintext connect for `#[cfg(test)]` loopback tests only (D-d). This
    /// path is compiled out of release builds — `Client` only ever
    /// constructs the TLS variant above.
    pub(crate) async fn connect_plaintext(addr: std::net::SocketAddr, cfg: &ClientConfig) -> Result<Self, Error> {
        let tcp = timeout(cfg.timeouts.connect, TcpStream::connect(addr))
            .await
            .map_err(|_| Error::Timeout { phase: Phase::Connect })?
            .map_err(Error::Connect)?;
        tcp.set_nodelay(true).map_err(Error::Connect)?;
        Ok(Self { wire: Wire::new(tcp), reusable: true })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_server::{Action, TestServer};

    fn cfg() -> ClientConfig {
        ClientConfig::default()
    }

    fn short_timeouts() -> ClientConfig {
        ClientConfig {
            timeouts: Timeouts {
                connect: Duration::from_millis(200),
                tls_handshake: Duration::from_millis(200),
                response_headers: Duration::from_millis(200),
                body: Duration::from_millis(200),
            },
            limits: Limits::default(),
        }
    }

    fn ok_response(body: &[u8]) -> Vec<u8> {
        let mut resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
        resp.extend_from_slice(body);
        resp
    }

    #[tokio::test]
    async fn single_round_trip_returns_body_and_records_request() {
        let server = TestServer::start(vec![vec![vec![Action::Write(ok_response(b"hello"))]]]).await;
        let cfg = cfg();
        let mut conn = Connection::connect_plaintext(server.addr, &cfg).await.unwrap();

        let req = b"GET / HTTP/1.1\r\nHost: x\r\n\r\n";
        match conn.round_trip(req, &cfg.limits, &cfg.timeouts).await {
            Attempt::Success(bytes) => assert_eq!(&bytes[..], b"hello"),
            _ => panic!("expected success"),
        }
        assert_eq!(server.requests()[0], req);
    }

    #[tokio::test]
    async fn keepalive_reuses_one_connection_for_two_requests() {
        let server = TestServer::start(vec![vec![
            vec![Action::Write(ok_response(b"one"))],
            vec![Action::Write(ok_response(b"two"))],
        ]])
        .await;
        let cfg = cfg();
        let mut conn = Connection::connect_plaintext(server.addr, &cfg).await.unwrap();

        let req = b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n";
        assert!(matches!(conn.round_trip(req, &cfg.limits, &cfg.timeouts).await, Attempt::Success(_)));
        assert!(conn.reusable);
        assert!(matches!(conn.round_trip(req, &cfg.limits, &cfg.timeouts).await, Attempt::Success(_)));

        assert_eq!(server.accept_count(), 1);
    }

    #[tokio::test]
    async fn server_close_after_response_surfaces_as_closed_empty_on_next_attempt() {
        let server = TestServer::start(vec![
            vec![vec![Action::Write(ok_response(b"one")), Action::Close]],
            vec![vec![Action::Write(ok_response(b"two"))]],
        ])
        .await;
        let cfg = cfg();
        let mut conn = Connection::connect_plaintext(server.addr, &cfg).await.unwrap();
        let req = b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n";
        assert!(matches!(conn.round_trip(req, &cfg.limits, &cfg.timeouts).await, Attempt::Success(_)));

        // Reusing the now-dead connection should surface ClosedEmpty, which
        // is what `Client` uses to trigger the retry-on-fresh-connection path.
        assert!(matches!(conn.round_trip(req, &cfg.limits, &cfg.timeouts).await, Attempt::ClosedEmpty));
        assert!(!conn.reusable);
    }

    #[tokio::test]
    async fn closes_mid_body_is_closed_not_short_success() {
        let mut partial = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n".to_vec();
        partial.extend_from_slice(b"short");
        let server = TestServer::start(vec![vec![vec![Action::Write(partial), Action::Close]]]).await;
        let cfg = cfg();
        let mut conn = Connection::connect_plaintext(server.addr, &cfg).await.unwrap();
        let req = b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n";
        match conn.round_trip(req, &cfg.limits, &cfg.timeouts).await {
            Attempt::Failed(Error::Closed) => {}
            other => panic!("expected Failed(Closed), got a different outcome ({})", matches!(other, Attempt::Success(_))),
        }
    }

    #[tokio::test]
    async fn head_then_stall_times_out_on_body() {
        let mut partial = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n".to_vec();
        partial.extend_from_slice(b"short");
        let server = TestServer::start(vec![vec![vec![
            Action::Write(partial),
            Action::StallThenClose(Duration::from_secs(5)),
        ]]])
        .await;
        let cfg = short_timeouts();
        let mut conn = Connection::connect_plaintext(server.addr, &cfg).await.unwrap();
        let req = b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n";
        match conn.round_trip(req, &cfg.limits, &cfg.timeouts).await {
            Attempt::Failed(Error::Timeout { phase: Phase::Body }) => {}
            _ => panic!("expected Timeout on Body phase"),
        }
    }

    #[tokio::test]
    async fn accepts_but_sends_nothing_times_out_on_headers() {
        let server = TestServer::start(vec![vec![vec![Action::StallThenClose(Duration::from_secs(5))]]]).await;
        let cfg = short_timeouts();
        let mut conn = Connection::connect_plaintext(server.addr, &cfg).await.unwrap();
        let req = b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n";
        match conn.round_trip(req, &cfg.limits, &cfg.timeouts).await {
            Attempt::Failed(Error::Timeout { phase: Phase::Headers }) => {}
            _ => panic!("expected Timeout on Headers phase"),
        }
    }

    #[tokio::test]
    async fn two_responses_in_one_write_both_parse_correctly() {
        let mut combined = ok_response(b"first");
        combined.extend_from_slice(&ok_response(b"second"));
        let server = TestServer::start(vec![vec![
            vec![Action::Write(combined)],
            vec![], // second request gets no fresh write — its response was already sent
        ]])
        .await;
        let cfg = cfg();
        let mut conn = Connection::connect_plaintext(server.addr, &cfg).await.unwrap();
        let req = b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n";

        match conn.round_trip(req, &cfg.limits, &cfg.timeouts).await {
            Attempt::Success(bytes) => assert_eq!(&bytes[..], b"first"),
            _ => panic!("expected success"),
        }
        match conn.round_trip(req, &cfg.limits, &cfg.timeouts).await {
            Attempt::Success(bytes) => assert_eq!(&bytes[..], b"second"),
            _ => panic!("expected success"),
        }
    }

    #[tokio::test]
    async fn connect_to_closed_port_fails_with_connect_error() {
        // Bind then drop immediately to obtain a port nothing is listening on.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let cfg = short_timeouts();
        let result = Connection::connect_plaintext(addr, &cfg).await;
        assert!(matches!(result, Err(Error::Connect(_))));
    }

    #[tokio::test]
    async fn body_larger_than_limit_is_body_too_large_and_drops_connection() {
        let body = vec![b'x'; 100];
        let server = TestServer::start(vec![vec![vec![Action::Write(ok_response(&body))]]]).await;
        let mut cfg = cfg();
        cfg.limits.max_body_bytes = 10;
        let mut conn = Connection::connect_plaintext(server.addr, &cfg).await.unwrap();
        let req = b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n";
        match conn.round_trip(req, &cfg.limits, &cfg.timeouts).await {
            Attempt::Failed(Error::BodyTooLarge { .. }) => {}
            _ => panic!("expected BodyTooLarge"),
        }
        assert!(!conn.reusable);
    }

    #[tokio::test]
    async fn non_2xx_small_body_is_http_error_and_reusable() {
        let mut resp = b"HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\n\r\n".to_vec();
        resp.extend_from_slice(b"no");
        let server = TestServer::start(vec![vec![vec![Action::Write(resp)]]]).await;
        let cfg = cfg();
        let mut conn = Connection::connect_plaintext(server.addr, &cfg).await.unwrap();
        let req = b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n";
        match conn.round_trip(req, &cfg.limits, &cfg.timeouts).await {
            Attempt::Failed(Error::Http { status: 404 }) => {}
            _ => panic!("expected Http(404)"),
        }
        assert!(conn.reusable);
    }

    #[tokio::test]
    async fn non_2xx_large_body_is_http_error_and_drops_connection() {
        let body = vec![b'x'; (DISCARD_CAP as usize) + 1];
        let mut resp = format!("HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
        resp.extend_from_slice(&body);
        let server = TestServer::start(vec![vec![vec![Action::Write(resp)]]]).await;
        let cfg = cfg();
        let mut conn = Connection::connect_plaintext(server.addr, &cfg).await.unwrap();
        let req = b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n";
        match conn.round_trip(req, &cfg.limits, &cfg.timeouts).await {
            Attempt::Failed(Error::Http { status: 500 }) => {}
            _ => panic!("expected Http(500)"),
        }
        assert!(!conn.reusable);
    }

    #[tokio::test]
    async fn chunked_response_decodes_correctly() {
        let body = b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let mut resp = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        resp.extend_from_slice(body);
        let server = TestServer::start(vec![vec![vec![Action::Write(resp)]]]).await;
        let cfg = cfg();
        let mut conn = Connection::connect_plaintext(server.addr, &cfg).await.unwrap();
        let req = b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n";
        match conn.round_trip(req, &cfg.limits, &cfg.timeouts).await {
            Attempt::Success(bytes) => assert_eq!(&bytes[..], b"hello world"),
            _ => panic!("expected success"),
        }
    }
}
