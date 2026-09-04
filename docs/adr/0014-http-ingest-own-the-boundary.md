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
| `bytes`              | Zero-copy body handoff                   | Response bodies are handed to the framing layer without a copy (ADR-0017).            |

This table matches `crates/http-ingest/Cargo.toml` `[dependencies]` exactly, row for row.
Header and chunk-framing parsing (status line, headers, `Content-Length` /
`Transfer-Encoding: chunked` bodies) is hand-rolled in `src/response.rs`, not delegated to
`httparse` (see Erratum, item 2).

The `ring` crypto provider is selected over `aws-lc-rs`. Ring is not strictly C-free (Perl-generated assembly, a small C shim), but its trusted surface is roughly two orders of magnitude smaller than AWS-LC, and the FIPS validation that justified accepting AWS-LC does not apply to the declared configuration in any case. This trade — smaller trusted surface for absence of FIPS validation — is deliberate. Should FIPS-validated cryptography become a procurement requirement, `aws-lc-rs` with the `fips` feature enabled may be reintroduced under a superseding ADR; the crate boundary makes the swap local.

### TLS protocol version policy (`tls12`)

`rustls` and `tokio-rustls` both enable the `tls12` feature, alongside TLS 1.3 (which
`rustls` enables unconditionally and cannot be turned off). This is a recorded decision,
not a default left in place:

- **Live evidence.** Direct measurement against both S3 hosts (`curl -vI`, 2026-07-31)
  shows both `unidata-nexrad-level2-chunks.s3.amazonaws.com` and
  `unidata-nexrad-level2.s3.amazonaws.com` negotiate TLS 1.3
  (`TLS_AES_128_GCM_SHA256` / `X25519MLKEM768`) today. Dropping `tls12` would cost nothing
  against these two hosts as they currently stand.
- **Rationale for keeping it anyway.** The deployment context is an operator on an
  arbitrary network during a severe weather event — a hotel, a vehicle hotspot, an
  agency network behind a TLS-inspecting middlebox — not a controlled data-center path to
  S3. An endpoint or middlebox that only offers TLS 1.2 would turn a cosmetic protocol-
  surface reduction into total acquisition failure at the worst possible time. Under
  Principle 2 (Stability as Ethics), that trade goes against narrowing the surface:
  resilience of the acquisition path outweighs the marginal attack-surface reduction of
  dropping a still-current, still-secure TLS version.
- **Reversal condition.** If a future deployment profile can guarantee TLS 1.3 end to end
  (e.g. a locked-down agency network with a known-compliant egress path), dropping `tls12`
  is a one-line feature-flag change; record it as a superseding note here rather than
  silently flipping it.

The crate implements:

- Connection establishment with configurable connect / TLS handshake / headers / body timeouts (four distinct deadlines)
- A single long-lived keepalive connection per `(host, port)` pair, reopened on failure. No connection pool. Polling cadence does not justify pool complexity.
- Request formatting for `GET` with query parameters (percent-encoded per RFC 3986 for the S3 base64 continuation-token character set, which contains `=`, `+`, and `/`)
- Response parsing bounded by `Content-Length`, or by a bounded `Transfer-Encoding: chunked` decoder under `max_body_bytes` for the one accepted chunked shape — any other `Transfer-Encoding` value, a duplicate header, or `Transfer-Encoding` combined with `Content-Length` is rejected as a framing violation (see Erratum, item 1).
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

- The new crate lives at `crates/http-ingest`. It exposes a `Client` type constructed once per process, and two methods: `list_prefix(prefix: &str, start_after: Option<&str>, continuation_token: Option<&str>, delimiter: Option<&str>) -> Bytes` and `get_object(key: &str) -> Bytes` (see Erratum, items 3 and 9). It does not expose a general `request()` method.
- The `S3PollingSource` acquisition layer takes the `Client` by value in its constructor. This preserves the existing seam where the caller wires the transport, and matches the current pattern of `S3Poller::new` accepting its HTTP client from above.
- Timeout policy: 5 s connect, 5 s TLS handshake, 10 s response headers, 30 s body read. These are configurable but have opinionated defaults.
- Error taxonomy distinguishes at minimum: `Connect`, `Tls`, `Protocol` (framing violation), `Http { status }`, `Timeout { phase }`, `Closed` (peer closed keepalive connection). Framing layer maps `Closed` to a transparent retry; other errors bubble.
- A `src/test_server.rs` module provides a minimal plaintext HTTP/1.1 responder for integration tests. It is `#[cfg(test)]`-gated and does not ship in the release binary (see Erratum, item 4).

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

## Erratum (added during dependency-inventory remediation, 2026-07-31)

8. **Item 6, above, is itself wrong about the chunks bucket and is superseded.** Item 6
   asserted the chunks bucket's key layout is `SITE/YYYY/MM/DD/...`. Direct inspection of
   the live bucket (2026-07-31, while measuring cold-start latency for
   `docs/plans/dependency-inventory-remediation.md` W1) found the real layout is
   `SITE/<volume-sequence>/<timestamp>-<n>-<kind>`: an unpadded per-site volume counter as
   the first path segment, with the calendar timestamp embedded in the filename rather
   than the path. (2026-09-03: "monotonically increasing" here is itself wrong — the
   counter is a cyclic 1–999 counter; see ADR-0011's erratum and
   `crates/radar-workstation/src/ingest/volume_seq.rs`.) This is not a wording nit — `S3Poller`'s
   `current_hour_anchor` was built directly on the wrong assumption, and a live measurement
   against it returned 32,524 keys (a whole day's near-complete backlog) instead of a small
   hour-boundary backlog, because the constructed anchor string didn't correspond to any
   real prefix and unpadded lexical sort put most of the bucket's contents after it. Fixed
   in the same session: `S3Poller` now discovers volume-sequence folders via `delimiter=/`
   (item 9) and anchors on the numeric volume number, not a synthetic calendar path. See
   `crates/radar-workstation/src/ingest/s3_poll.rs` doc comments and the plan's W1 Results
   for the full account. Item 6's claim about the *archive* bucket's layout
   (`YYYY/MM/DD/SITE/...`) is unaffected — that bucket was not re-verified here and nothing
   found this session contradicts it.
9. **`list_prefix` gains a fourth parameter, `delimiter: Option<&str>`.** Added to support
   `S3Poller::list_volume_folders` (item 8, above): `Some("/")` requests S3's
   `<CommonPrefixes>` grouping instead of a flat key listing, which is what makes
   enumerating volume-sequence directories cheap (one small request instead of paging
   through every chunk in the retention window). Same endpoint, same host allowlist, same
   trust boundary as the existing `list-type=2` query — this is an additional optional
   query parameter, not a new capability, so it does not reopen the "no connection pool" /
   scope-boundary decisions above.

## Erratum (added by ADR-0026, 2026-08-28)

10. **The "Scope boundaries" section is amended, not discarded.** That section says any
    future need for one of its non-goals is *"a signal to reopen this ADR, not to grow the
    crate."* Q16 was exactly that signal, and
    [ADR-0026](0026-tile-http-boundary.md) is the reopening. Three of the listed non-goals
    change status at the workspace level:

    - *"Arbitrary URL parsing"* — still true of `http-ingest` itself, which validates
      hostname **syntax** only. Which hosts are permissible is now the caller's policy.
      `url` and `idna` remain absent from the graph; `tile-fetch` hand-rolls an
      ASCII-only URL template parser, so D-07 stays resolved.
    - *"Serving as a general-purpose HTTP client for other crates in the workspace"* — no
      longer holds. `http-ingest` becomes the shared HTTP/1.1 **engine** for two sibling
      policy crates, `s3-fetch` and `tile-fetch`. This was chosen over duplicating
      `connection.rs` + `response.rs` (~1,065 lines) and the fuzz corpus into a second
      client, which is what `dependency-inventory.md` E-09 had recommended.
    - *"Redirect following"* — unchanged and now **permanent**, promoted from an omission
      to a decision on BC-1 grounds. See ADR-0026 §4.

    HTTP/2, HTTP/3, cookies, proxies, request bodies, multipart, and HTTP-layer
    compression remain non-goals, unchanged.

11. **Two behaviours move out of the engine into the policy crates.** The compile-time
    host allowlist (`src/host.rs`) moves to `s3-fetch`, where it becomes a `Bucket` enum —
    a stronger statement than the current string match, since no hostname string reaches
    host selection at all. The `is_2xx` gate at `connection.rs:124` also moves out, so the
    engine returns the status and each policy crate decides; this is what makes `304 Not
    Modified` usable by `tile-fetch` without touching the response parser, which already
    frames 304 correctly (`response.rs:140`).

12. **The `tls12` decision (above) is reinforced by the tile path.** Its rationale — an
    operator on an arbitrary network, possibly behind a TLS-inspecting middlebox — applies
    with more force to user-configured tile hosts than to two known S3 endpoints. Keeping
    `tls12` is now load-bearing for a second reason.
