# Implementation Plan — ADR-0014: `crates/http-ingest`

**Status:** Draft, not yet implemented
**Drafted:** 2026-07-29
**Implements:** ADR-0014 (Own the HTTP boundary)
**Baseline commit:** `f512ea8`

This plan is written to be executed in a later session. It carries every decision
already made so that implementation does not need to re-litigate them.

---

## 1. Decisions taken (settled — do not reopen during implementation)

| # | Question | Decision | Consequence |
|---|---|---|---|
| D-a | Concurrent chunk fetch vs. single connection | **Serialize on one connection.** `JoinSet` is removed from `poll_once`; `get_object` takes `&mut self`. | Matches ADR's "no connection pool" literally. Startup backfill is slower; steady state (~1 chunk / 5 s poll) is unaffected. |
| D-b | Header parsing library | **Hand-rolled.** `httparse` is *not* taken as a dependency. | Deviates from the ADR dependency table. Requires the hostile-input test matrix in §6D to be treated as load-bearing, not optional. |
| D-c | Module layout | **Split modules**, not a single file. See §3. | |
| D-d | TLS in tests | **Plaintext loopback for the wire tests; real TLS only in `#[ignore]`d live tests.** No `rcgen`, no checked-in test cert. | The plaintext code path exists only under `#[cfg(test)]` and cannot be reached from a release build. TLS handshake correctness is verified manually against real S3, not in CI. |
| D-e | Fuzzing | **`fuzz/` excluded from the workspace (nightly, manual) + corpus replay and deterministic mutation as ordinary `#[test]`s.** | Default `cargo build` / `cargo test` never needs nightly. Corpus regressions still fail normal CI. |
| D-f | `nexrad-sample` CLI | **Keep `--sample-url <full URL>`**; add a ~30-line local `split_s3_url`. | CLI and `RADAR_SAMPLE_URL` stay compatible. Three of the four existing utility tests must be rewritten (§7.3). |

### 1.1 Recommendation carried inline (flag if you disagree)

**Retry on `Closed`.** ADR-0014 says the framing layer maps `Closed` to a transparent
retry. Put the retry *inside* `Client` instead, under a strict invariant:

> Retry exactly once, and only when **all three** hold: the request was a `GET`
> (always true — the crate has no other verb), the connection was **reused** rather
> than freshly opened, and **zero response bytes** were observed before the close.

That is the standard safe idempotent retry. It cannot double-deliver, because a
reused connection closing before any response byte means the server never processed
the request. Putting it in `Client` keeps `S3Poller` free of transport concerns and
means the framing layer only ever sees errors that are genuinely its problem. A
second failure bubbles.

---

## 2. Deviations from ADR-0014 as written

These must be recorded as an erratum on the ADR (Phase 7). Listing them here so the
implementing session does not read the ADR and conclude the code is wrong.

1. **`httparse` is not a dependency** (D-b). The ADR's table lists it with the note
   "alternative to hand-rolling"; we took the hand-rolled branch. Direct dependency
   count is unchanged at five, because `bytes` takes its slot.
2. **`list_prefix` returns `Bytes`, not `ListResponse`,** and gains a `start_after`
   parameter. `ListResponse` is a `radar-workstation` domain type; hoisting it into
   the transport crate would drag `quick-xml` in with it and grow the dependency list
   the ADR exists to shrink. XML parsing stays where it is today.
3. **`dev-server/` becomes `src/test_server.rs`, `#[cfg(test)]`-gated and plaintext**
   (D-d). The ADR describes a "rustls-fronted HTTP/1.1 responder"; the TLS decision
   made that a cost without a matching benefit.
4. **Wire tests live in `src/` unit tests, not `tests/`.** Integration tests cannot
   reach the `#[cfg(test)]` plaintext path without a feature flag that would then have
   to be passed on every `cargo test` invocation. Unit tests get it for free.
5. **LOC estimate revised.** The ADR estimates 500–900 lines. The module breakdown in
   §3 lands closer to **~1,000 lines of production code plus ~90 of test harness**,
   most of it in `response.rs`. Not a reason to change course, but the ADR's number
   should not be quoted as a target that implementation is failing to hit.

---

## 3. Crate layout

```
crates/http-ingest/
├── Cargo.toml
├── fuzz/                          # excluded from the workspace; nightly-only
│   ├── Cargo.toml
│   ├── fuzz_targets/parse_response.rs
│   └── corpus/parse_response/     # golden + hostile seeds, checked in
├── src/
│   ├── lib.rs          ~130 loc   # Client, send/retry, public API, re-exports
│   ├── config.rs        ~60       # ClientConfig, Timeouts, Limits
│   ├── error.rs         ~80       # Error, Phase
│   ├── host.rs          ~50       # compile-time allowlist, Host
│   ├── encode.rs        ~70       # RFC 3986 percent-encoding
│   ├── request.rs       ~90       # request serialization, two builders
│   ├── response.rs     ~260       # status line + header parsing  ← security core
│   ├── connection.rs   ~220       # Wire<S>, connect, round_trip, body read
│   ├── tls.rs           ~60       # rustls config, ALPN pin, session store
│   └── test_server.rs   ~90       # #[cfg(test)] scripted plaintext TCP server
└── tests/
    └── live_s3.rs                 # all #[ignore] — real network
```

`Wire<S>` in `connection.rs` is generic over `S: AsyncRead + AsyncWrite + Unpin`.
Production instantiates `Wire<TlsStream<TcpStream>>`; `#[cfg(test)]` code instantiates
`Wire<TcpStream>`. This one generic parameter is what makes D-d work without a feature
flag.

### 3.1 `Cargo.toml`

```toml
[dependencies]
tokio        = { version = "1",    default-features = false, features = ["net", "time", "io-util", "rt"] }
rustls       = { version = "0.23", default-features = false, features = ["ring", "std", "tls12"] }
tokio-rustls = { version = "0.26", default-features = false, features = ["ring", "tls12"] }
webpki-roots = "1"
bytes        = "1"
```

Notes for the implementing session:

- `rustls` **default features must be off** — the default set is
  `["aws-lc-rs", "logging", "std", "tls12"]`. Leaving them on reintroduces the exact
  1.35M-line C library this ADR removes.
- `webpki-roots` 1.0 changed `TLS_SERVER_ROOTS` to a slice of `TrustAnchor`; confirm
  the API shape against the version that resolves.
- Keep `tls12` **on** for now. AWS S3 supports TLS 1.3, but dropping 1.2 is a
  separate, verifiable decision — make it in Phase 3 with live evidence, not in
  Phase 1 on an assumption.

---

## 4. Design specification

### 4.1 Public API (`lib.rs`)

```rust
pub struct Client {
    host: Host,
    tls:  Arc<rustls::ClientConfig>,   // shared across reconnects → session resumption works
    cfg:  ClientConfig,
    conn: Option<Connection>,
}

impl Client {
    pub fn new(host: &str) -> Result<Self, Error>;
    pub fn with_config(host: &str, cfg: ClientConfig) -> Result<Self, Error>;

    pub async fn list_prefix(
        &mut self,
        prefix: &str,
        start_after: Option<&str>,
        continuation_token: Option<&str>,
    ) -> Result<Bytes, Error>;

    pub async fn get_object(&mut self, key: &str) -> Result<Bytes, Error>;
}
```

No public `request()`. No public verb other than GET. The `tls` field is an `Arc`
held across reconnects specifically so rustls' session cache survives a dropped
connection — that is the whole mechanism behind the ADR's resumption requirement.

### 4.2 Error taxonomy (`error.rs`)

```rust
pub enum Error {
    HostNotAllowed(String),
    Connect(std::io::Error),
    Tls(std::io::Error),
    Protocol(&'static str),          // framing violation; message names the rule
    Http { status: u16 },
    Timeout { phase: Phase },
    Closed,                          // peer closed a keepalive connection
    BodyTooLarge { len: u64, limit: u64 },
    InvalidInput(&'static str),      // CR/LF in a key, etc.
}

pub enum Phase { Connect, Tls, Headers, Body }
```

`Protocol` carries a `&'static str` naming the rule that fired (`"bare LF"`,
`"duplicate Content-Length"`, …). No allocation, and test assertions can match the
exact rule rather than just the variant.

### 4.3 Configuration (`config.rs`)

```rust
pub struct Timeouts { connect, tls_handshake, response_headers, body }  // 5s, 5s, 10s, 30s
pub struct Limits {
    max_head_bytes:  usize,  // 64 KiB
    max_header_count: usize, // 128
    max_body_bytes:  u64,    // 64 MiB
}
```

`max_body_bytes` is the OOM guard and is checked against `Content-Length` **before
any allocation**. A hostile or buggy origin advertising `Content-Length:
9999999999999` must cost zero bytes of heap.

### 4.4 Host allowlist (`host.rs`)

```rust
pub const ALLOWED_HOSTS: &[&str] = &[
    "unidata-nexrad-level2-chunks.s3.amazonaws.com",
    "unidata-nexrad-level2.s3.amazonaws.com",
];
```

Exact `==` match only. Not prefix, not suffix, not `contains`. Port is fixed at 443
and is not part of the input. Reject anything containing `@`, `:`, `/`, or a trailing
dot before the comparison.

> **Phase 0 gate:** if either bucket 301-redirects to a region-qualified hostname,
> the region-qualified name goes in the list instead. The ADR forbids redirect
> following, so an unnoticed redirect is a hard failure of the primary code path,
> not a degradation.

### 4.5 Request formatting (`request.rs`)

```
GET {path_and_query} HTTP/1.1\r\n
Host: {host}\r\n
User-Agent: radar-workstation/{CARGO_PKG_VERSION}\r\n
Accept: */*\r\n
Connection: keep-alive\r\n
\r\n
```

`Accept-Encoding` is deliberately absent — the ADR excludes HTTP-layer compression.
Omitting the header is a request, not a guarantee, so §4.6 rule 13 rejects any
non-identity `Content-Encoding` on the response side.

Two builders, nothing else:

- `list_query(prefix, start_after, continuation_token)`
  → `/?list-type=2&prefix=…[&start-after=…][&continuation-token=…]`
  Parameter order is fixed and `start-after` / `continuation-token` are mutually
  exclusive at the call site (S3 requires this; preserve today's `first_page` logic).
- `object_path(key)` → `/` + percent-encoded key.

### 4.6 Response parsing (`response.rs`) — the security core

```rust
#[doc(hidden)]  // public only so the out-of-workspace fuzz target can reach it
pub fn parse_head(buf: &[u8], limits: &Limits) -> Result<Option<Head>, Error>;

pub struct Head {
    status: u16,
    content_length: u64,
    connection_close: bool,
    head_len: usize,
}
```

`Ok(None)` = incomplete, need more bytes. `Ok(Some(_))` = `\r\n\r\n` found.
`Err(_)` = framing violation, connection is poisoned and must be dropped.

Each rule below gets a named test in §6D. This list *is* the specification.

1. Status line is exactly `HTTP/1.1 ` or `HTTP/1.0 `, then exactly three ASCII
   digits, then either `\r\n` or one SP + reason-phrase + `\r\n`. Reject `HTTP/2`,
   `HTTP/0.9`, two- or four-digit codes, non-digits, and any other protocol token.
2. **Line terminators are strictly `\r\n`.** A bare `\n` is not a line ending and is
   rejected. This is the single most important smuggling defense in the parser.
3. A bare `\r` anywhere inside a header line is rejected.
4. Header name is one or more RFC 7230 `tchar`. **A space before the colon is
   rejected** (classic smuggling vector).
5. Header value permits VCHAR, SP, HTAB only. Any other byte < 0x20, or 0x7F, is
   rejected.
6. Obsolete line folding (`obs-fold` — a continuation line beginning with SP or HTAB)
   is rejected.
7. `Content-Length` may appear **at most once**. A duplicate is rejected *even if the
   two values are identical*.
8. `Content-Length` value is 1–20 ASCII digits after optional OWS. No `+`, no `-`, no
   `0x`, no comma-separated list, no trailing garbage. Parse with overflow check.
9. **Any `Transfer-Encoding` header is rejected**, including `identity`. Per the ADR
   this is a hard error, not a parse branch.
10. Both `Content-Length` and `Transfer-Encoding` present → rejected (rule 9 fires
    first; assert the error anyway).
11. `content_length > limits.max_body_bytes` → `BodyTooLarge`, before any allocation.
12. `1xx` is rejected — we never send `Expect: 100-continue`. `204` and `304` are
    accepted with `content_length = 0` whether or not the header is present. Every
    other status **must** carry a valid `Content-Length`; its absence is a framing
    violation, not an "read until EOF" fallback.
13. `Content-Encoding` other than `identity` (or absent) → rejected.
14. Head size is capped at `max_head_bytes` and header count at `max_header_count`.
    Both are enforced **incrementally as bytes arrive**, so an origin that never
    sends `\r\n\r\n` cannot grow the read buffer without bound.
15. `Connection: close` (ASCII-case-insensitive, comma-list aware) sets
    `connection_close`, marking the connection non-reusable.
16. Header name comparison is `eq_ignore_ascii_case` on the raw slice. Never allocate
    a lowercased `String` to compare.

**Body read.** Exactly `content_length` bytes into a `BytesMut` reserved to that
size, then `freeze()`. Not one byte more — trailing bytes belong to the next response
on the keepalive connection and must remain in the read buffer. EOF before
`content_length` is satisfied → `Closed`, never a truncated success.

**Non-2xx handling.** If `content_length <= 8 KiB`, read and discard the body so the
connection stays reusable, then return `Http { status }`. Otherwise return the error
and drop the connection. This keeps a 404 (routine — S3 listings can race object
creation) from costing a full TCP + TLS reconnect.

### 4.7 Percent-encoding (`encode.rs`)

Unreserved set per RFC 3986: `A-Z a-z 0-9 - . _ ~`. Everything else becomes `%XX`
with uppercase hex.

- `encode_query_value` — encodes strictly the unreserved set. This is what makes the
  base64 continuation token (`=`, `+`, `/`) correct, which is the specific
  requirement the ADR calls out.
- `encode_path` — same, plus `/` passes through as a path separator. NEXRAD keys
  (`KDOX/2026/07/29/00/KDOX_20260729_000248_I`) are entirely unreserved plus `/`, so
  this is conservative and lossless for the real workload.

Both additionally hard-reject `\r` and `\n` in their input with
`InvalidInput`. Encoding already neutralizes them; the explicit rejection is
belt-and-braces against request splitting and makes the intent testable.

### 4.8 TLS (`tls.rs`)

- Provider: `rustls::crypto::ring::default_provider()`, selected explicitly rather
  than relying on a default.
- Roots: `webpki_roots::TLS_SERVER_ROOTS` only. **No `rustls-platform-verifier`, no
  `rustls-native-certs`** — this is the concrete fix for audit finding D-02.
- **`alpn_protocols = vec![b"http/1.1".to_vec()]`.** Pinning ALPN means the server
  can never negotiate h2 against a client that cannot speak it. Do not skip this.
- Resumption: set `config.resumption` to an explicit
  `ClientSessionMemoryCache`-backed store rather than relying on the default, so the
  behavior is legible at the call site.
- Built once in `Client::new`, stored as `Arc`, reused across every reconnect.

### 4.9 Connection lifecycle (`connection.rs`)

```rust
struct Wire<S> { stream: S, rbuf: BytesMut }
struct Connection { wire: Wire<TlsStream<TcpStream>>, reusable: bool }
```

- `connect`: `timeout(cfg.connect, TcpStream::connect)` → `Timeout { Connect }` /
  `Connect`; `set_nodelay(true)`; `timeout(cfg.tls_handshake, connector.connect(...))`
  → `Timeout { Tls }` / `Tls`.
- `round_trip`: `write_all` + `flush`, then read the head under
  `cfg.response_headers`, then the body under `cfg.body`. Four distinct deadlines, as
  the ADR requires.
- Any `Err` other than `Http { .. }` drops the connection. A poisoned read buffer is
  never reused.

---

## 5. Phasing

Each phase ends green on `cargo build --release && cargo test && cargo clippy -- -D
warnings`.

### Phase 0 — Pre-flight verification (no code)

Do this **first**. Two of its findings can invalidate design choices already baked in.

- [ ] `curl -sv https://unidata-nexrad-level2-chunks.s3.amazonaws.com/?list-type=2&prefix=KDOX/`
      — record: status, whether a 301 occurs, `Transfer-Encoding` vs `Content-Length`,
      `Connection` header, negotiated TLS version and ALPN.
      **If S3 uses `Transfer-Encoding: chunked` for `ListObjectsV2`, stop — the ADR's
      hard-error decision breaks the primary path and the ADR needs amending before
      any code is written.**
- [ ] Same against `unidata-nexrad-level2` (archive). This resolves ADR-0014 open
      question #2 with evidence.
- [ ] Save real response heads (both shapes) into `fuzz/corpus/parse_response/` as
      golden seeds and as the basis for the `request.rs` golden tests.
- [ ] Record baselines: `cargo tree -p radar-workstation | wc -l`,
      `cargo tree -p nexrad-sample | wc -l`, `ls -l target/release/fetch-sample`.

### Phase 1 — Pure modules, no I/O

`error.rs`, `config.rs`, `host.rs`, `encode.rs`, `request.rs`, `response.rs`.
Test groups **A–D**. Most of the crate's risk lives here and none of it needs a
socket. Do not start Phase 2 until group D is complete and green.

### Phase 2 — I/O

`tls.rs`, `connection.rs`, `test_server.rs`, `lib.rs` (`Client`, retry). Test group **E**.

### Phase 3 — Live validation

`tests/live_s3.rs`, group **F**. Prove the client works against real S3 **before**
touching any caller. Revisit the `tls12` feature here with live evidence.

### Phase 4 — Fuzzing

`fuzz/` target + corpus, plus the in-`cargo test` corpus replay and mutation harness.
Group **G**.

### Phase 5 — Migrate `crates/radar-workstation`

See §7.1. Group **H**.

### Phase 6 — Migrate `utility/nexrad-sample`

See §7.3.

### Phase 7 — Documentation

- ADR-0013 → `Status: Superseded by ADR-0014`.
- ADR-0014 → append an **Erratum** section carrying §2 of this plan verbatim.
- New ADRs: `0015-bzip2`, `0016-quick-xml`, `0017-bytes` — closing the "ask first"
  gap the audit identified. (Separable from the rest; can ship in its own commit.)
- `CLAUDE.md`: ADR index, the stack table's HTTP row, and the status paragraph.

---

## 6. Test plan

### A. `encode.rs` (unit)

- unreserved bytes pass through unchanged
- `=`, `+`, `/` in a query value → `%3D`, `%2B`, `%2F` (the continuation-token case)
- a real base64 S3 continuation token round-trips to the exact expected string
- path encoding preserves `/`, encodes space, `%`, `?`, `#`, and non-ASCII UTF-8 bytes
- `\r` or `\n` in input → `InvalidInput`
- hex digits are uppercase

### B. `host.rs` (unit)

- each allowlisted host is accepted
- rejected: case variants, trailing dot, `evil.com`,
  `unidata-nexrad-level2-chunks.s3.amazonaws.com.evil.com`,
  `evil.unidata-nexrad-level2-chunks.s3.amazonaws.com`, embedded port, userinfo (`@`),
  empty string, a host containing `/`

### C. `request.rs` (unit)

- byte-for-byte golden request for `list_prefix` in each of its four argument shapes
- byte-for-byte golden request for `get_object`
- assert `Accept-Encoding` is absent
- assert exactly one `Host` header and `Connection: keep-alive`
- a key with characters requiring encoding produces a correctly encoded request line

### D. `response.rs` (unit) — the hostile matrix

Table-driven. **One case per numbered rule in §4.6**, asserting the specific
`Protocol` message, not just the variant:

| Input | Expected |
|---|---|
| bare-LF line endings | `Protocol("bare LF")` |
| `Content-Length` twice, identical values | `Protocol` |
| `Content-Length: 5, 5` | `Protocol` |
| `Content-Length: +5` / `" 5 "` / `0x5` / `5abc` / empty | `Protocol` |
| `Content-Length: 99999999999999999999999` | `Protocol` (overflow) |
| `Transfer-Encoding: chunked` | `Protocol` |
| `Transfer-Encoding: identity` | `Protocol` |
| CL + TE together | `Protocol` |
| `Content-Length : 5` (space before colon) | `Protocol` |
| `obs-fold` continuation line | `Protocol` |
| header name containing NUL / space / `(` | `Protocol` |
| header value containing an embedded `\r` | `Protocol` |
| `Content-Encoding: gzip` | `Protocol` |
| `HTTP/2 200` / `HTTP/1.1 20` / `HTTP/1.1 2000` / `ICAP/1.0 200` | `Protocol` |
| `HTTP/1.1 100 Continue` | `Protocol` |
| `204` / `304` with no `Content-Length` | `Ok`, body empty |
| `Content-Length` one byte over `max_body_bytes` | `BodyTooLarge`, no allocation |
| head one byte over `max_head_bytes` | `Protocol` |
| 129 headers | `Protocol` |
| `Connection: close` / `Connection: keep-alive, close` / `Connection: CLOSE` | `connection_close == true` |
| well-formed 200 | `Ok`, correct `status` / `content_length` / `head_len` |

Plus one **property test**: for a fixed valid head of N bytes, feed every prefix
`1..N` and assert `Ok(None)` for all of them, and an identical `Head` at N. Cheap,
and it is the test that catches incremental-parse bugs the table cannot.

### E. `connection.rs` + `test_server.rs` (unit, plaintext loopback)

`test_server.rs` provides a scripted server: takes a list of canned response byte
blobs, records every request it receives and how many TCP connections it accepted.

- single round-trip returns the body; recorded request bytes match the golden
- **two sequential requests, server accepted exactly one connection** — the keepalive proof
- server closes after response 1 → `Client` reconnects and the second request
  succeeds (the D-a/§1.1 retry path), and the server records two accepts
- server closes *mid-body* → `Closed`, not a short success
- server sends head then stalls → `Timeout { phase: Body }`
- server accepts TCP then sends nothing → `Timeout { phase: Headers }`
- server writes response 1 *and* the head of response 2 in one `write` → both parse
  correctly (read-buffer boundary correctness)
- connect to a closed port → `Connect`
- body larger than `max_body_bytes` → `BodyTooLarge`, connection dropped
- non-2xx with a small body → `Http { status }` **and the connection is still reusable**
- non-2xx with a body over 8 KiB → `Http { status }` and the connection is dropped

### F. `tests/live_s3.rs` — all `#[ignore]`

Run with `cargo test -p http-ingest -- --ignored`. Not part of default CI.

- `list_prefix` against the chunks bucket for a live site returns XML with ≥ 1 key
- `get_object` on the first key returns bytes that `detect_chunk_kind` accepts
- two sequential `get_object` calls succeed — real keepalive against real S3
- a `list_prefix` with a `continuation-token` from a truncated page succeeds
  (proves the encoding of `=` / `+` / `/` against the real service — this is the one
  test that can catch an encoding bug the unit tests would agree with)
- the archive bucket answers the same two request shapes

### G. Fuzzing

- `[workspace] exclude = ["crates/http-ingest/fuzz"]` so the default build never sees it.
- Target `parse_response`: arbitrary bytes → `parse_head`. Asserts no panic, and on
  `Ok(Some(head))` asserts `head.head_len <= input.len()` and
  `head.content_length <= limits.max_body_bytes`.
- Seed corpus: the Phase 0 real responses plus every hostile input from group D.
- `#[test] fn fuzz_corpus_never_panics()` in `response.rs` walks
  `$CARGO_MANIFEST_DIR/fuzz/corpus/parse_response/` and asserts each entry parses
  without panicking — so corpus regressions fail plain `cargo test`.
- `#[test] fn mutated_inputs_never_panic()` — xorshift with a **fixed seed**, 5,000
  iterations of bit flips / byte splices / truncations over the corpus. Fixed seed
  means a failure is reproducible rather than a flake.

### H. `radar-workstation` regressions

- The four existing `s3_poll.rs` unit tests (`chunk_kind_from_known_suffixes`,
  `hour_anchor_sorts_before_real_keys`, `unix_to_utc_known_values`,
  and both `parse_list_xml` tests) must pass **verbatim, unmodified**. That they
  survive untouched is the signal that the migration disturbed only the transport.

**Known gap, accepted:** `poll_once` remains untested. Making it testable requires a
trait seam abstracting `Client`, which reintroduces exactly the indirection ADR-0014
exists to remove. The batch-atomicity property (§7.2) is preserved by construction
and reviewed by reading, not asserted by a test. Flagging rather than hiding it.

---

## 7. Migration detail

### 7.1 `crates/radar-workstation/Cargo.toml`

```diff
-reqwest = { version = "0.13", default-features = false, features = ["rustls", "webpki-roots", "stream", "query"] }
-quick-xml = { version = "0.37", features = ["async-tokio"] }
+http-ingest = { path = "../http-ingest" }
+quick-xml = "0.37"
```

`async-tokio` removal is safe: `parse_list_xml` is already synchronous over `&[u8]`.

### 7.2 `crates/radar-workstation/src/ingest/s3_poll.rs`

- `PollError::Http(reqwest::Error)` → `PollError::Http(http_ingest::Error)`.
- **`PollError::Task` is deleted** along with the `JoinSet` import — with fetches
  serialized there is no spawned task and no `JoinError`.
- `S3Poller::new(site_id, client: http_ingest::Client)`; the `client` field becomes
  owned and mutable.
- `poll_once`: replace the `JoinSet` block with a sequential loop.
  **Preserve the batch-atomicity invariant** — `last_seen_key` advances only after
  every chunk in the batch has been fetched successfully. With sequential fetching
  this falls out naturally: the first `?` aborts before the assignment. Keep the
  comment explaining *why*, since the mechanism is no longer visible in the code
  shape.
  The `fetched: Vec<Option<Bytes>>` scaffolding and the
  `.expect("every slot filled…")` disappear.
- `fetch_bytes` free function is deleted; its doc comment explaining why it was a free
  function no longer applies.
- `list_keys_after`: drop the `Vec<(&str, Cow<str>)>` params and the `Cow` import;
  call `client.list_prefix(prefix, first_page.then_some(start_after), token.as_deref())`.
  Keep the `first_page` logic — S3 still forbids combining `start-after` with
  `continuation-token`.
- `BUCKET_BASE` becomes a bare hostname constant, not a URL.

Pre-existing behavior worth restating (**not** a regression introduced here): a key
that permanently 404s blocks the stream, because `last_seen_key` never advances past
it. True today. Out of scope for this ADR; worth an issue.

### 7.3 `utility/nexrad-sample`

- New `src/url.rs`: `split_s3_url(&str) -> Result<(&str, &str), AcquisitionError>` —
  require an `https://` prefix (reject `http://` explicitly with a distinct message),
  split at the first `/` after the authority, require a non-empty host and key.
  Host *allowlist* validation is delegated to `Client::new`, not duplicated here.
- `data_acquisition.rs`: `split_s3_url` → `Client::new(host)?` → `get_object(key)`,
  then the existing tempfile write + atomic rename, unchanged. `AcquisitionError`
  gains a `NotAllowed(String)` variant mapping from `Error::HostNotAllowed`.
- `bin/fetch_sample.rs`: drop `use reqwest::Url`; derive the filename from the last
  `/`-segment of the key.
- `Cargo.toml`: drop `reqwest`, add `http-ingest` path dependency. **This is the fix
  for audit finding D-03** — with reqwest gone from both crates there is no longer a
  version for them to disagree on.

**Test impact — a real reduction, called out rather than buried.** Three of the four
tests in `tests/data_acquisition.rs` drive a plaintext loopback server over
`http://127.0.0.1:PORT`. The new client is HTTPS-only with a host allowlist, so they
cannot be ported:

- `download_sample_writes_a_file_for_a_successful_response` → becomes `#[ignore]`d,
  hitting a real S3 key
- `download_sample_returns_an_error_for_a_non_success_status` → becomes an `#[ignore]`d
  live test against a known-absent key
- `download_sample_returns_an_error_for_an_empty_body` → **lost**; keep the
  `EmptyResponse` branch and cover it by a direct unit test of the check if it is
  extracted into a testable function
- `download_sample_returns_an_error_for_invalid_urls` → survives, retargeted at
  `split_s3_url`, and gains cases: `http://` scheme, allowlisted-host-with-no-key,
  non-allowlisted host

New `src/url.rs` unit tests cover the split logic thoroughly, which is where the
utility's real remaining logic lives. `utility/` is dev-only per `CLAUDE.md`, so
trading integration coverage for unit coverage here is an acceptable exchange — but
it *is* an exchange.

---

## 8. Verification gates

Run at the end of Phase 5 and again at the end of Phase 6. Record actual numbers
against the ADR's claims — the ADR was written on an audit, and the audit's
predictions should be checked, not assumed.

```bash
cargo build --release
cargo test
cargo test -p http-ingest -- --ignored     # live, manual
cargo clippy --all-targets -- -D warnings
cargo audit
```

| Gate | Expected |
|---|---|
| `grep -rn reqwest --include=*.toml --include=*.rs .` | no hits |
| `cargo tree -i aws-lc-rs` | "package ID not found" |
| `cargo tree -i idna` | "package ID not found" (kills the ICU4X stack — D-07) |
| `cargo tree -i ring -e no-dev` | exactly one crypto provider (D-03) |
| `cargo tree -i rustls-platform-verifier` | not found (D-02) |
| `cargo tree -p radar-workstation \| wc -l` | ADR predicts ~100 → < 15; **record the actual** |
| `ls -l target/release/fetch-sample` | ADR predicts well under 4.8 MB; **record the actual** |
| `cargo tree -p http-ingest --depth 1` | exactly the five crates in §3.1 |

---

## 9. Open items carried forward

- **ADR-0014 open question 1** (connection reuse across multiple hostnames) is
  answered by D-a for now: one `Client` per host, serialized. A second concurrent
  source means a second `Client`, which is fine and needs no new machinery.
- **ADR-0014 open question 2** (archive bucket deviation) is resolved by evidence in
  Phase 0, not by reasoning.
- Whether to drop the `tls12` rustls feature — decide in Phase 3 against live
  negotiation data.
- The permanently-404ing-key stall in `poll_once` (§7.2) predates this work and
  should get its own issue.
