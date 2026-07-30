# ADR-0014: Own the HTTP boundary — replace reqwest with a workspace-local client

**Status:** Accepted
**Date:** 2026-07-29
**Supersedes:** ADR-0013 (Use `reqwest` for S3 access)
**Related:** ADR-0008 (Custom NEXRAD decoder), ADR-0011 (Chunked S3 as primary source)

---

## Context

ADR-0013 accepted `reqwest` for the S3 acquisition layer, on three load-bearing arguments: (1) `aws-lc-rs` provides FIPS 140-3 validated cryptography for government deployability; (2) the `webpki-roots` feature makes the binary self-contained with respect to the system trust store; (3) the `stream` feature is required for chunk bodies that may be several megabytes. A dependency audit against the tree at commit `f512ea8` established that none of the three claims holds against the code and lockfile actually produced:

- **FIPS is not enabled.** FIPS validation applies to `aws-lc-fips-sys`, gated behind the `fips` feature of `aws-lc-rs`. The declared configuration resolves to the default `aws-lc-sys` path. `aws-lc-fips-sys` does not appear in `Cargo.lock`. The 1,345,961-line C dependency is present; the property it was accepted to secure is not.
- **The system trust store is consulted.** In `reqwest 0.13` the `rustls` feature unconditionally pulls `rustls-platform-verifier`, and the default client construction path selects `rustls_platform_verifier::Verifier::new()`. The bundled Mozilla roots are compiled in but unreached. Obtaining the documented behavior requires opting in via `ClientBuilder::tls_certs_only()`; no caller does.
- **Response bodies are buffered, not streamed.** `s3_poll.rs` uses `.bytes()` in both call sites. No `bytes_stream()` call exists in the workspace. The `stream` feature is dead code.

Two further findings from the same audit are relevant:

- The `reqwest → url → idna` chain pulls the ICU4X Unicode normalization stack (14 crates plus two embedded data tables) to punycode a hostname (`unidata-nexrad-level2-chunks.s3.amazonaws.com`) that is a compile-time ASCII constant.
- Version drift between the workspace's two `reqwest` pins causes `rustls` feature unification to compile both `aws-lc-rs` and `ring` — two independent C/assembly cryptography stacks — into any root-level `cargo build`.

The actual HTTP surface required by the acquisition layer is:

- HTTPS to a fixed set of compile-time-known S3 hostnames
- Two request shapes: `GET /?list-type=2&prefix=…&continuation-token=…` returning XML, and `GET /<key>` returning bytes
- No authentication (public bucket), no cookies, no redirect following, no HTTP-layer compression (BZ2 is decompressed by the framing layer above)
- Connection reuse across polls
- Buffered response bodies bounded by `Content-Length`

This is a strictly narrower surface than reqwest's design center. The transitive cost of using reqwest to satisfy it — approximately 100 third-party crates, the ICU4X stack, tower/tower-http middleware, and a C cryptography library — is disproportionate to what is delivered.

Precedent for owning code at this level of the stack exists in the workspace. `nexrad-decoder` (ADR-0008) implements a genuinely difficult binary format in 700 lines of `std`-only Rust with an empty `[dependencies]` section. HTTP/1.1 is a smaller and better-specified surface than NEXRAD Message 31.

## Decision

Replace `reqwest` with a workspace-local HTTP/1.1 client crate (`crates/http-ingest`) that assembles the minimum stack required for the acquisition layer's actual workload.

Direct dependencies of the new crate:

| Crate                | Role                                     | Rationale                                                                             |
|----------------------|------------------------------------------|---------------------------------------------------------------------------------------|
| `tokio`              | Async runtime, TCP I/O                   | Already a workspace dependency (ADR-0004).                                            |
| `rustls`             | TLS implementation                       | Memory-safe TLS. Configured with the `ring` provider (see below).                     |
| `tokio-rustls`       | Async glue for rustls                    | Minimal; maintained by the rustls project.                                            |
| `webpki-roots`       | Trust anchors                            | Mozilla root store, compiled in. No runtime dependency on the system certificate store. |
| `httparse`           | HTTP/1.1 response header parsing         | Single crate, no transitive dependencies, well-audited. Alternative to hand-rolling.  |

The `ring` crypto provider is selected over `aws-lc-rs`. Ring is not strictly C-free (Perl-generated assembly, a small C shim), but its trusted surface is roughly two orders of magnitude smaller than AWS-LC, and the FIPS validation that justified accepting AWS-LC does not apply to the declared configuration in any case. This trade — smaller trusted surface for absence of FIPS validation — is deliberate. Should FIPS-validated cryptography become a procurement requirement, `aws-lc-rs` with the `fips` feature enabled may be reintroduced under a superseding ADR; the crate boundary makes the swap local.

The crate implements:

- Connection establishment with configurable connect / TLS handshake / headers / body timeouts (four distinct deadlines)
- A single long-lived keepalive connection per `(host, port)` pair, reopened on failure. No connection pool. Polling cadence does not justify pool complexity.
- Request formatting for `GET` with query parameters (percent-encoded per RFC 3986 for the S3 base64 continuation-token character set, which contains `=`, `+`, and `/`)
- Response parsing bounded by `Content-Length`. Requests advertise `Connection: keep-alive` and do not accept `Transfer-Encoding: chunked` — chunked responses are rejected with a typed error rather than parsed.
- Rustls session resumption via a process-local `ClientSessionStore` to amortize TLS handshake cost across polls
- Structured errors distinguishing transport, TLS, protocol, and status-code failures

`quick-xml` is retained for parsing `ListObjectsV2` responses. XML has enough edge cases (entities, CDATA, encoding declarations) that hand-parsing is the one place in this stack where the ownership-cost tradeoff runs the other way. `quick-xml` is pure Rust, permanently 0.x but very widely deployed and stable, and the audit rates it low risk.

The `async-tokio` feature of `quick-xml` is removed. `parse_list_xml` is synchronous over `&[u8]`.

## Scope boundaries

The following are explicit non-goals of `crates/http-ingest`:

- HTTP/2, HTTP/3
- Cookie handling
- Redirect following
- Proxy support
- Arbitrary URL parsing (`url::Url` and the `idna` chain are not depended on; hostnames are validated against a compile-time allowlist)
- Request bodies of any kind
- Multipart, form encoding
- HTTP-layer compression (gzip, deflate, brotli)
- Serving as a general-purpose HTTP client for other crates in the workspace

Any future need for one of these is a signal to reopen this ADR, not to grow the crate.

## Consequences

### Positive

- The transitive dependency count for the acquisition path drops from approximately 100 crates to fewer than 15. The specific removals include the ICU4X stack (14 crates plus data tables), `tower`, `tower-http`, `tokio-util`, `rustls-platform-verifier`, `serde`/`serde_urlencoded`/`ryu` (query serialization is hand-rolled for the known parameter set), and either `aws-lc-rs` (68 MB vendored, 1.35M LOC of C) or the second copy of a crypto provider currently pulled through the version-split path.
- Every line of code that touches the network is legible from the framing layer down to the socket. This is the same posture `nexrad-decoder` achieves for the parsing layer, and it is the posture `PHILOSOPHY.md` names as a first-order goal.
- The findings D-01 (FIPS rationale), D-02 (cert-store behavior), D-03 (dual crypto stacks), D-06 (unused stream feature), D-07 (ICU4X for one hostname), and D-08 (0.12/0.13 baseline) from the dependency audit are all resolved in a single motion rather than patched individually.
- Reproducibility improves. The C build step in `aws-lc-sys` — which probes the host to choose between `cc` and CMake paths — is removed from the graph. `webpki-roots` is compiled in; no runtime dependency on the host trust store.
- Static binary size decreases meaningfully. The `fetch-sample` binary is currently 4.8 MB with the reqwest tree linked; the replacement stack is a small fraction of that.
- The crate boundary makes future substitution (FIPS-validated crypto, HTTP/2 if ever required, an alternative rustls provider) a local change.

### Negative

- The workspace takes on approximately 500–900 lines of code that must be maintained in perpetuity. Bugs in this code are not patched by third parties.
- Client-side HTTP/1.1 parsing has fewer landmines than server-side, but not zero. Response smuggling shapes, malformed headers, and `Content-Length` / `Transfer-Encoding` conflicts must be rejected explicitly. The decision to hard-error on `Transfer-Encoding: chunked` (rather than support it) is defensible for the S3 workload but is a real constraint that must not silently regress if the acquisition layer ever fetches from a different origin.
- Test burden is non-trivial. The crate requires unit tests for header parsing (well-formed and hostile inputs), integration tests against a local TLS server, and fuzz testing on the response parser. This is a one-time cost, but it is a real one.
- Loss of FIPS-validated cryptography as a claim. Given ADR-0013's FIPS rationale did not hold against the actual build, this is a nominal loss, not a real one — but it must be documented, because a procurement reviewer reading the ADR trail will look for it.
- `bytes::Bytes` remains part of `ChunkEnvelope`'s public shape. This was noted under D-12 as acceptable; it remains acceptable. The new client returns `Bytes` for zero-copy handoff to the framing layer.

### Neutral

- HTTP/2 is not lost. The current reqwest configuration has no `h2` in the lockfile; HTTP/2 was never on.
- The `bytes` and `bytes-utils` chain remains in the graph transitively. Zero incremental cost.
- `quick-xml` retention is unchanged from the current tree. Its ADR gap (previously unrecorded as a deliberate choice) is closed by this document.

## Implementation notes

- The new crate lives at `crates/http-ingest`. It exposes a `Client` type constructed once per process, and two methods: `list_prefix(prefix: &str, continuation_token: Option<&str>) -> ListResponse` and `get_object(key: &str) -> Bytes`. It does not expose a general `request()` method.
- The `S3PollingSource` acquisition layer takes the `Client` by value in its constructor. This preserves the existing seam where the caller wires the transport, and matches the current pattern of `S3Poller::new` accepting its HTTP client from above.
- Timeout policy: 5 s connect, 5 s TLS handshake, 10 s response headers, 30 s body read. These are configurable but have opinionated defaults.
- Error taxonomy distinguishes at minimum: `Connect`, `Tls`, `Protocol` (framing violation), `Http { status }`, `Timeout { phase }`, `Closed` (peer closed keepalive connection). Framing layer maps `Closed` to a transparent retry; other errors bubble.
- A `dev-server/` module under `crates/http-ingest` provides a minimal rustls-fronted HTTP/1.1 responder for integration tests. It is `#[cfg(test)]`-gated and does not ship in the release binary.

## Migration

- `crates/radar-workstation` drops `reqwest` and takes `http-ingest` as a path dependency.
- `utility/nexrad-sample` is migrated to the same client, resolving D-03 by construction — there is no longer a version to disagree on.
- ADR-0013 is marked superseded. Its file remains in `docs/architecture/adr/` for historical continuity; this ADR is the current authority on the HTTP boundary.
- The `bzip2`, `quick-xml`, and `bytes` dependencies acquire ADRs of their own in the same commit series, closing the gap identified in the audit against the `CLAUDE.md` "ask first" instruction.

## Open questions

- Whether to support connection reuse across multiple destination hostnames concurrently, or serialize acquisition against a single host at a time. Current polling behavior fetches from one hostname; the answer likely does not matter until a second source is wired in.
- Whether the archive S3 source (`unidata-nexrad-level2`) requires any deviation from the chunked-source client shape. To be verified during implementation of the archive `ChunkSource`.

## Rejected alternatives

- **Level 1: keep reqwest, swap `aws-lc-rs` for `ring`.** Removes the 1.35M-line C library but leaves ~100 transitive crates, the ICU4X stack, and the tower middleware graph in the build. Addresses D-01 and D-03 but not D-02, D-06, D-07, or D-08. Considered as a bridge if this ADR's implementation is delayed; not adopted as an endpoint.
- **Level 2: replace reqwest with `hyper` + `hyper-util`.** Sheds the tower/cookie/redirect surface and drops the transitive count to ~20–30 crates. However, `hyper`'s type surface still transitively pulls `url` and therefore `idna` in common configurations, and the ownership boundary has been moved without being collapsed. Rejected as the worst-of-both-worlds outcome: pays the cost of owning the HTTP boundary without the benefit of a fully legible stack.

## Erratum (added during implementation, 2026-07-29)

The implementation plan (`docs/plans/0014-http-ingest-implementation.md`) carries several
decisions made before or during implementation that this ADR's original text does not
reflect. Recorded here so the ADR and the shipped code agree.

1. **`Transfer-Encoding: chunked` is supported, not hard-rejected — this is a substantive
   change to the Decision, not a wording fix.** The ADR as originally written says
   "chunked responses are rejected with a typed error rather than parsed." Phase 0
   pre-flight verification against both live buckets (2026-07-29) found that
   `ListObjectsV2` — the primary chunk-discovery path, called every 5-second poll —
   always responds `Transfer-Encoding: chunked` with no `Content-Length`, on both
   `unidata-nexrad-level2-chunks` and `unidata-nexrad-level2`. A blanket rejection breaks
   the primary workload, not an edge case. `crates/http-ingest/src/response.rs` instead
   implements a bounded, spec-compliant chunked decoder: `Transfer-Encoding: chunked` is
   the one accepted value; any other value, a duplicate header, or `Transfer-Encoding`
   combined with `Content-Length` is still rejected as a framing violation. `get_object`
   (the other call shape) is unaffected — verified live to always carry `Content-Length`.
   The request-smuggling defenses this rejection existed to provide are proxy-chain
   concerns; as a direct client with no intermediary, they don't apply the same way here,
   and the chunked decoder is itself bounded by `max_body_bytes` exactly like the
   `Content-Length` path.
2. **`httparse` is not a dependency** (plan D-b). Hand-rolled instead. Direct dependency
   count is unchanged at five, because `bytes` takes its slot.
3. **`list_prefix` returns `Bytes`, not `ListResponse`,** and gains a `start_after`
   parameter. `ListResponse` is a `radar-workstation` domain type; hoisting it into the
   transport crate would drag `quick-xml` in with it. XML parsing stays where it is
   today, in `radar-workstation::ingest::s3_poll`.
4. **`dev-server/` became `src/test_server.rs`,** `#[cfg(test)]`-gated and plaintext
   (plan D-d), not the "rustls-fronted HTTP/1.1 responder" originally described. TLS
   handshake correctness is instead verified against real S3 in `tests/live_s3.rs`
   (all `#[ignore]`d).
5. **Wire tests live in `src/` unit tests** (`connection.rs`), not `tests/` — integration
   tests can't reach the `#[cfg(test)]` plaintext path without a feature flag.
6. **The archive bucket's key layout is `YYYY/MM/DD/SITE/...`**, the reverse of the
   chunks bucket's `SITE/YYYY/MM/DD/...`. Confirmed live (2026-07-29); resolves this
   ADR's second open question. Not yet consumed by any `ChunkSource` — that's future
   work — but worth recording now since it was learned in the course of writing
   `tests/live_s3.rs`.
7. **LOC estimate revised.** This ADR's original estimate of 500–900 lines and the
   plan's revised estimate of ~1,000 lines of production code both undershoot somewhat
   once the chunked decoder (item 1) is counted; not a reason to revisit the design.
