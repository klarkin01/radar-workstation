use std::sync::Arc;

use rustls::client::Resumption;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};

use crate::error::Error;

/// Builds the shared TLS client configuration once per `Client::new`. Reused
/// across every reconnect (`Client::tls` is an `Arc`) so rustls' session
/// cache survives a dropped connection — this is the entire mechanism behind
/// session resumption.
pub(crate) fn build_config() -> Arc<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());

    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring provider supports its own default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();

    // Pinning ALPN to http/1.1 means the server can never negotiate h2
    // against a client that cannot speak it.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    // Explicit rather than relying on the default so the behavior is legible
    // at the call site.
    config.resumption = Resumption::in_memory_sessions(256);

    Arc::new(config)
}

pub(crate) fn server_name(host: &str) -> Result<ServerName<'static>, Error> {
    ServerName::try_from(host.to_string()).map_err(|_| Error::HostNotAllowed(host.to_string()))
}
