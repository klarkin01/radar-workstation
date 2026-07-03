# ADR-0013: HTTP Client Dependency for S3 Data Acquisition

## Status
Accepted

## Context
The real-time data pipeline established in ADR-0011 pulls NEXRAD Level II chunks from
the `unidata-nexrad-level2-chunks` S3 bucket over HTTPS. The Rust standard library
provides no HTTP client. An external crate is required on the critical path.

The `unidata-nexrad-level2-chunks` bucket is public — no AWS SigV4 authentication is
required. Every fetch is a plain HTTPS GET against a known URL. This constrains what the
HTTP client actually needs to do: issue authenticated-but-unsigned GETs, stream response
bodies, follow redirects, and maintain connection pools for sustained polling.

Three options were evaluated:

**Option A — `reqwest` (batteries-included HTTP client)**
The dominant HTTP client in the Rust ecosystem. Wraps the `hyper` crate and handles TLS,
connection pooling, redirect following, and response streaming behind an ergonomic
async/await API. Default TLS backend on Linux is OpenSSL (a C dependency). An
alternative pure-Rust TLS backend (`rustls`) is available via feature flags.
~50M downloads/month; ~38,000 downstream dependents; maintained by Sean McArthur
(also the `hyper` maintainer). Pulls in 14+ required transitive crates, though
significant overlap exists with the `tokio` tree already in use.

**Option B — `hyper` directly**
The lower-level HTTP library that `reqwest` wraps. Using it directly would trade
ergonomics for one fewer layer of abstraction but would not materially reduce the
transitive dependency count. The code surface required to replicate reqwest's
connection pooling, redirect handling, and response streaming from hyper primitives
would be substantial and custom-maintained.

**Option C — hand-rolled HTTPS over `tokio`**
Constructing HTTP/1.1 GET requests directly over `tokio::net::TcpStream` with a
Rust TLS crate (e.g., `rustls`). Technically viable for a simple GET-only client
against a known host. In practice this would require reimplementing chunked transfer
encoding, connection keepalive, and any redirect the S3 infrastructure issues. The
maintenance burden is not justified by the savings.

## Decision
`reqwest` is used as the HTTP client, declared with `default-features = false` and
the `rustls-tls` and `stream` features explicitly enabled:

```toml
reqwest = { version = "0.13", default-features = false, features = ["rustls", "webpki-roots", "stream"] }
```

The `rustls` feature selects the rustls TLS backend in place of the default OpenSSL
binding. In reqwest 0.13, rustls uses `aws-lc-rs` as its cryptographic provider;
this pulls in `aws-lc-sys`, a C binding to AWS LibCrypto (a maintained fork of
BoringSSL). The build therefore requires a C toolchain. This is accepted — see
Rationale. The `webpki-roots` feature bundles Mozilla's CA root certificates so the
binary does not depend on the system certificate store — consistent with the goal of
reproducible, self-contained builds. The `stream` feature enables response body
streaming, which is necessary for chunk bodies that may be several megabytes in size.
The `query` feature enables URL query parameter serialization used by the S3
ListObjectsV2 API.

No other reqwest features (JSON, cookies, multipart, encoding detection, form
serialization) are enabled. The S3 use case requires none of them.

## Rationale

**An HTTP client is a hard format requirement, not a comfort dependency.**
The data source is an HTTPS endpoint. There is no alternative transport. The decision
is not whether to take an HTTP client dependency but which one and how to constrain it.
This is structurally identical to the bzip2 decision (ADR-0012): the format mandates
the capability; the only question is how cleanly the dependency is managed.

**reqwest is the appropriate level of abstraction.**
`hyper` directly would save nothing meaningful — the transitive dependency graph is
nearly identical, and the custom adapter code required to replicate reqwest's connection
pooling and streaming support would be a permanent maintenance burden with no benefit
to auditability or security. Option C (hand-rolled) would be worse on both dimensions.

**`rustls` avoids OpenSSL; `aws-lc-rs` is an acceptable crypto provider.**
The default `native-tls` feature on Linux links against OpenSSL. OpenSSL is excluded
on the grounds of dependency hygiene, audit surface, and the absence of FIPS
validation guarantees in the version typically shipped by Linux distributions.
`aws-lc-rs` is a different C library entirely: it is a Rust-bindgen wrapper around
AWS LibCrypto, itself a maintained fork of BoringSSL. Critically, aws-lc-rs holds
FIPS 140-3 validation, which OpenSSL on Linux does not. A C toolchain is required to
build it, but the result is an audited, FIPS-validated cryptographic implementation —
a stronger posture than the alternative. The `rustls-no-provider` path (which would
allow substituting the pure-assembly `ring` crate) was considered but adds a direct
dependency and requires explicit provider initialization at startup for no practical
security benefit over aws-lc-rs.

**Tokio overlap limits incremental cost.**
reqwest's transitive dependency tree is substantial in isolation but shares extensive
overlap with `tokio`, which is already present. The net new dependency surface is
significantly smaller than a raw crate count implies. In any case, the dependency is
justified: this is not a convenience import.

**Feature minimization limits attack surface.**
Declaring `default-features = false` and enabling only what is used means that JSON
parsing, cookie handling, multipart body construction, and encoding detection are not
compiled into the binary. This is consistent with the project's security posture and
reduces the auditable surface of the network layer.

## Consequences
- HTTPS GET against the S3 chunk bucket is handled correctly, including connection
  reuse across the polling loop established in ADR-0011.
- The binary contains no OpenSSL binding. TLS is provided by `rustls` backed by
  `aws-lc-rs` (AWS LibCrypto). This is FIPS 140-3 validated and does not require
  OpenSSL to be present on the target system. A C toolchain is required at build time.
- reqwest's transitive tree is relatively large. Any future contributor adding a
  reqwest feature flag (e.g., `json`, `cookies`) should treat that as a scope
  change requiring review, not a minor convenience addition.
- If the S3 endpoint ever requires authenticated access (e.g., if the bucket becomes
  private), SigV4 signing would need to be implemented. reqwest does not provide this
  natively. The `aws-sigv4` crate provides signing support and is compatible with
  reqwest. This is not an anticipated requirement for the current public bucket.
- The HTTP client is an implementation detail of the ingest layer; `reqwest` types do
  not surface into the decoder, the compute layer, or the UI.
