# ADR-0026: The Tile HTTP Boundary — One Engine, Two Policy Crates

## Status
Accepted (2026-08-28) — **implementation deferred to post-v1.0**, except the `Bucket`
enum (below), which Stage 5 implemented 2026-09-02
(`docs/plans/stage-5-map-underlays.md` §11): `Client` renamed `S3Client` inside
`http-ingest`, `S3Client::new`/`with_config` take `Bucket` (not a hostname) and are
infallible, and `host.rs`'s `Host::parse`/`ALLOWED_HOSTS` and their ten unit tests are
deleted — the host-allowlist property moved from "checked at runtime" to
"unrepresentable." Everything else below stays deferred with the tile subsystem.

[ADR-0027](0027-tile-image-decoding.md), later the same day, defers the tile subsystem
out of v1.0 on the codec question this ADR raised as Q18. The transport design below is
unchanged and unsuperseded; it is simply not built yet.

Two practical consequences, both from ADR-0027 §4:

- **The three-crate split is deferred with the subsystem.** Its motivation was hosting a
  second policy crate, and with `tile-fetch` deferred it would leave a two-crate structure
  with one consumer. This ADR's own finding — that the seam already exists inside
  `http-ingest` — is what makes deferring the split cheap rather than costly.
- **`S3Client::new(bucket: Bucket)` is taken up now**, inside the existing crate. Making
  the radar path's host guarantee compiler-checked is worth having on its own merits, and
  it does not depend on tiles.

Deferred with the subsystem: `crates/tile-fetch`, the `UrlTemplate` parser, `ETag` /
`If-None-Match`, the N-worker concurrency model, and the tile `ClientConfig` budget.

Resolves [Q16](../open-questions.md). Amends the scope boundaries of
[ADR-0014](0014-http-ingest-own-the-boundary.md) — specifically the clauses excluding
arbitrary URL parsing, conditional requests, and *"serving as a general-purpose HTTP
client for other crates in the workspace."* ADR-0014's substantive decisions (own the
HTTP/1.1 implementation, `ring` over `aws-lc-rs`, keep `tls12`, no HTTP/2, bounded
framing) are unchanged and are what this ADR builds on. Implements the transport half of
[ADR-0007](0007-tile-providers.md).

## Context

ADR-0014 made "the destination host is a compile-time constant" a load-bearing property
of the acquisition path. ADR-0007 requires a destination host that comes from user
configuration. `dependency-inventory.md` E-09 raised the collision; Q16 carried it
forward as the live question. It is not a dependency-selection question — it is a
question about which of those two properties survives, and what the actual invariant is
underneath the one that gives way.

### The premises were not measured, and three of them are wrong

Q16 asserts tile fetching needs redirect following, `ETag` / `If-None-Match`, and
"possibly HTTP/2." Probed against the live providers on 2026-08-28 with
`curl --http1.1`:

| Claim | Measured |
|---|---|
| HTTP/2 required | **False.** `basemap.nationalmap.gov` (the ADR-0007 default) serves `HTTP/1.1 200`, `Connection: keep-alive`, `Content-Length: 27858`, `Content-Type: image/jpeg`. Same for `tile.openstreetmap.org` and `server.arcgisonline.com`. HTTP/2 is an ALPN preference these hosts offer, never a requirement they impose. |
| Redirects required | **Not observed.** `curl -L` against all three: `num_redirects=0`. |
| `ETag` / conditional GET | **True, and nearly free.** USGS returns `etag: "197d3e867e8"` and `cache-control: max-age=86400`, and answers `If-None-Match` with a clean `304 Not Modified`. `crates/http-ingest/src/response.rs` **already** frames 304 correctly as bodiless; only the `is_2xx` gate in `connection.rs` rejects it. |
| Arbitrary host | **True.** This is the whole question. |

Q16 also omits the one requirement that genuinely strains ADR-0014: **concurrency.**
ADR-0014 chose a single keepalive connection and no pool, on the reasoning that *"polling
cadence does not justify pool complexity."* A 5-second poll does not. A viewport of 20–40
tiles fetched serially at ~50 ms each does — one to two seconds of basemap latency per
pan.

### The invariant that actually matters

BC-1 requires that the application never initiate a network connection the user has not
sanctioned. The compile-time allowlist is *one implementation* of that property, not the
property itself. The property is:

> Every destination host traces to an explicit, auditable, user-controlled decision —
> never to data received over the network.

A tile URL in the XDG config file satisfies this completely; it is the user sanctioning
the connection, in the most direct form available. A redirect does not: following one
means opening a connection to a host chosen by a remote server. That single distinction
settles most of the sub-decisions below, and it is why the allowlist can be relaxed for
tiles without weakening BC-1 at all — the relaxation is *from config*, not *from the
wire*.

### The existing crate is already layered for this

`crates/http-ingest/src/lib.rs` is a thin S3 policy layer — a host allowlist and two
request shapes — sitting on a fully generic HTTP/1.1 engine (`connection.rs`,
`response.rs`, `request.rs`, `tls.rs`, `encode.rs`). Nothing in the engine knows about
S3. The decision below makes an existing internal seam explicit rather than introducing
a new one.

## Decision

Split `http-ingest` by layer: one HTTP/1.1 engine, two sibling policy crates that depend
on it. Three crates under `crates/`, per ADR-0010's structure.

### 1. `crates/http-ingest` — the engine (keeps its name)

Retains the framing, chunked decoding, TLS setup, session resumption, four-phase
timeouts, bounded limits, the single idempotent retry on a reused-connection close, and
the fuzz corpus. **One copy of all of it.**

It gains a generic request entry point and loses its S3 specifics:

```rust
pub struct Endpoint { /* syntactically validated host + port */ }
impl Endpoint {
    /// Validates hostname *syntax* only — ASCII, label rules, length,
    /// no userinfo/port/slash/trailing dot. Which hosts are permissible
    /// is the caller's policy, not the engine's.
    pub fn parse(host: &str, port: u16) -> Result<Self, Error>;
}

pub struct Engine { /* endpoint, tls config, one connection */ }
impl Engine {
    pub fn new(endpoint: Endpoint, cfg: ClientConfig) -> Self;
    pub async fn get(&mut self, path_and_query: &str, extra: &[Header])
        -> Result<Response, Error>;
}

pub struct Response { pub status: u16, pub body: Bytes, pub etag: Option<String> }
```

Two deliberate changes of responsibility:

- **Host *policy* leaves the engine.** `host.rs`'s allowlist moves to `s3-fetch`
  (§2). The engine validates that a hostname is syntactically well-formed and nothing
  more. The security property is not weakened; it moves to where it can be stated more
  strongly.
- **The `is_2xx` gate leaves the engine.** `connection.rs:124` currently converts any
  non-2xx into `Error::Http`. The engine now returns the status and lets each policy
  crate decide, which is what makes 304 usable without touching the parser.

The name `http-ingest` is retained despite the crate no longer being the ingest client,
to preserve ADR-0014's identity, the `crates/http-ingest/fuzz` paths, the root
`Cargo.toml` `exclude` line, and the audit trail in `dependency-inventory.md`. This is a
continuity choice, not a claim that the name is ideal.

### 2. `crates/s3-fetch` — the radar path

Owns today's `Client`, renamed `S3Client`, with the same two methods and the same
behaviour. `s3_poll.rs` changes its import and nothing else.

The host allowlist becomes stronger than it is today. `Host::parse(&str)` is replaced by
an enum, so no string ever reaches host selection:

```rust
pub enum Bucket { Chunks, Archive }   // the two ADR-0011 sources, and only those
impl S3Client { pub fn new(bucket: Bucket) -> Self; }
```

**`S3Client` has no constructor and no method that accepts a hostname.** The guarantee
that the radar path cannot be pointed at another host is a property of the type, checked
by the compiler, not an invariant a reviewer must confirm by auditing call sites. Port is
fixed at 443. No conditional requests, no extra headers.

### 3. `crates/tile-fetch` — the basemap path

```rust
pub struct TileClient { /* one Engine, one provider */ }
impl TileClient {
    pub fn new(template: &UrlTemplate, cfg: ClientConfig) -> Result<Self, Error>;
    pub async fn fetch(&mut self, z: u32, x: u32, y: u32, etag: Option<&str>)
        -> Result<TileOutcome, Error>;
}
pub enum TileOutcome { Tile { bytes: Bytes, etag: Option<String> }, NotModified }
```

`UrlTemplate` parses the configured `https://host[:port]/path/{z}/{x}/{y}.ext` once, at
config load, into `(Endpoint, path template)`. Hand-rolled, ~120 lines, **ASCII hosts
only** — `url` and `idna` stay out of the graph, keeping ADR-0014's D-07 finding
resolved. A user with an internationalised tile host enters punycode. A template missing
any of `{z}`, `{x}`, `{y}`, or carrying a scheme other than `https`, is rejected at
config load with a named error, not at first pan.

### 4. Sub-decisions

| Decision | Choice | Rationale |
|---|---|---|
| **Redirects** | **Never followed. Permanent.** 3xx (other than 304) is a typed error; the tile renders absent and the status bar names the provider. | Measured zero across all three major providers, so this costs nothing today. More importantly it is the one capability that would let a remote server choose a destination host, which is precisely what BC-1 exists to prevent. This is a decision, not a deferral: a provider that requires redirects is a reason to configure a different provider, not to add redirect following. |
| **HTTP/2** | No. | Measured unnecessary. ADR-0014's exclusion stands unchanged. |
| **Conditional requests** | `ETag` / `If-None-Match` only. No `Last-Modified` / `If-Modified-Since`. | One optional request header plus letting 304 through the status gate. The parser already frames 304 (`response.rs:140`). Pairs directly with Q7's cache-revalidation policy. |
| **Scheme** | `https` only; `http` templates rejected at config load. | The engine is TLS-only by construction, and plaintext basemap tiles would be a real regression in the deployment context ADR-0014's `tls12` clause describes. |
| **Port** | Explicit non-443 port permitted for tiles; fixed at 443 for S3. | Self-hosted and agency-internal tile servers on non-standard ports are a realistic air-gapped-deployment case (ADR-0007). |
| **Concurrency** | N independent `TileClient`s as N worker tasks, N = 4 by default. **Zero pool code.** | Each client owns one engine owning one connection — ADR-0014's "no connection pool" decision is preserved literally. Concurrency comes from task count, not from connection multiplexing. |
| **Tile limits** | `max_body_bytes` = 4 MB (vs. 64 MB for S3); timeouts 5 s connect / 5 s TLS / 5 s headers / 10 s body (vs. 30 s body for S3). | A tile that takes 30 seconds is useless; a 64 MB tile is an attack, not an image. Per-crate `ClientConfig` defaults make each path's budget explicit. |
| **Failure posture** | A tile failure produces a missing tile and a status-bar line, and nothing else, ever. | No shared connection state, no shared error values, no shared task supervision with the radar path. This preserves the part of E-09's option 2 that was worth having. |

### 5. Auth, headers, and what tiles may not send

`TileClient` sends `Host`, `User-Agent`, `Accept`, `Connection: keep-alive`, and
optionally `If-None-Match`. It sends no cookies, no `Authorization`, and no API keys —
ADR-0007's "no API key" property is enforced in code, not merely assumed of the default
provider. A provider requiring authentication is out of scope; that is a scope decision
consistent with Restraint is a Feature, and it is recorded here so a future request to
add a keyed provider reopens this ADR rather than quietly adding a header.

## Consequences

### Positive

- **One HTTP/1.1 parser, one fuzz corpus, one place to fix a framing bug.** The
  alternative that E-09 recommended would have duplicated ~1,065 lines of
  `connection.rs` + `response.rs`, the 31-file corpus, and the seeded mutator — divergence
  risk concentrated on the most security-sensitive code in the workspace, and a direct
  violation of `CLAUDE.md`'s DRY instruction.
- **Zero new dependencies.** The production graph is unchanged at 78 lockfile packages.
  Combined with ADR-0025 (five crates removed, zero added), Stage 5's total dependency
  delta is negative.
- **The radar path's host guarantee gets stronger, not weaker.** `Bucket` as an enum is a
  tighter statement than `Host::parse` against a string allowlist.
- **~300 lines of new code**: URL template parser (~120), `TileClient` (~150), conditional
  request plumbing (~30). The engine's own code is moved, not rewritten.
- **BC-1 remains auditable in one sitting.** The complete set of destinations is: two S3
  buckets named in an enum, one tile URL from config, and Stage 6's placefile URLs from
  config. No destination is derivable from network data.
- **`http-ingest`'s design is confirmed as a permanent asset.** Q16 asked whether the
  allowlist was permanent or temporary; the answer is permanent for the path it guards.

### Negative

- **A three-crate refactor of working, tested code.** `http-ingest`'s 68 unit tests and
  the fuzz target must keep passing across the split, and the fuzz target's path to
  `parse_head` (currently a `#[doc(hidden)] pub mod response`) has to survive it. The
  engine's behaviour does not change, which makes this mechanical, but it is not free.
- **ADR-0014's scope-boundary list is now partly false as written.** It says a need like
  this is *"a signal to reopen this ADR, not to grow the crate."* That is exactly what has
  happened, and the outcome is that two of its exclusions (arbitrary URL parsing,
  general-purpose use by other crates) no longer hold at the workspace level. An erratum
  is added to ADR-0014 pointing here.
- **Two `ClientConfig` default sets to keep straight.** Mitigated by their living in
  different crates with different names, but a reviewer must now check which budget
  applies where.
- **No redirect following is a real constraint**, not a costless one. A provider that
  moves behind a redirect stops working until the user updates the template. Accepted
  deliberately; the status bar naming the provider is what makes it diagnosable.

### Neutral

- HTTP/2 is still absent, as it has been since ADR-0013's lockfile.
- `bytes::Bytes` remains the body handoff type across all three crates (ADR-0017).
- Q5 and Q7 (cache sharing, cache sizing) are untouched and are answered with the tile
  cache, not here. This ADR decides the transport only.

## Rejected alternatives

- **Generalize `http-ingest` into one runtime-configurable client** (E-09 option 1). One
  implementation, which is right, but `Client::new(host)` accepting any string means the
  radar path's guarantee degrades from a compiler-checked property to a call-site
  convention. Every future reader would have to audit call sites to know S3 cannot be
  redirected elsewhere. The drift risk ADR-0014 names — the crate growing back toward a
  general HTTP client until the reqwest argument reappears with the maintenance burden now
  internal — is real, and a single type with two personalities is how it starts.
- **A second, fully independent client crate** (E-09 option 2, the inventory's
  recommendation). The right goal — structural isolation of the tile path — reached by the
  wrong mechanism. It buys type-level separation at the price of a second copy of the
  framing and chunked-decoding code, which is the one thing in this workspace that most
  needs a single source of truth. This ADR achieves the same isolation for ~300 lines
  instead of ~1,300, because the seam it splits on already existed.
- **Reintroduce a third-party client for tiles only** (E-09 option 3). Re-imports the
  `url` → `idna` → ICU4X chain and most of what ADR-0014 removed, for the *less* important
  data path. E-09 rated it worst; nothing found here changes that.
- **Follow redirects, bounded (same-host, HTTPS, max 1 hop).** The bounded form is
  defensible and was seriously considered. Rejected because it was measured unnecessary
  against every provider tested, and because "same-host" is a check on data the server
  supplies — a weaker statement than "the host came from the config file," which is what
  BC-1 wants to be able to say without qualification.
- **Support `Last-Modified` / `If-Modified-Since` alongside `ETag`.** A second date-parsing
  path (three legal HTTP date formats, RFC 9110 §5.6.7) for no measured gain — USGS,
  OSM, and ArcGIS all serve `ETag`.

## Open questions this ADR does not answer

- **[Q18](../open-questions.md) — what decodes the tile image bytes.** Tile bodies are
  JPEG and PNG (USGS returned `image/jpeg`), and decoding them is a new untrusted-input
  parser on a network path. That is a larger surface than everything decided here, it is
  a dependency question ADR-0025 has direct bearing on, and it must be settled before the
  tile subsystem is built. Raised as Q18 by this ADR.
- **Q5, Q7** — tile cache sharing across instances, and cache size / eviction policy.
  Answered with the cache implementation.
