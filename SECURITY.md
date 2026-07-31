# Security Policy

## Reporting a vulnerability

Report vulnerabilities through GitHub's private vulnerability reporting feature for
this repository (repository **Settings → Security → Advisories → Report a
vulnerability**, or the **Security** tab → **Report a vulnerability**). This creates a
private advisory visible only to the maintainers until a fix is ready — no public issue,
no email address to manage.

Do not open a public GitHub issue for a suspected vulnerability.

## Supported versions

This project is pre-1.0 with no released versions. Only `main` is supported; there is no
backport policy.

## Scope

This is a single-site NEXRAD Level II radar analysis application. The paths that process
untrusted or adversary-influenced input are the ones worth reporting against:

- **NEXRAD chunk bytes fetched from S3** — parsed by `crates/nexrad-decoder`
  (uncompressed and BZ2-decompressed) and by the BZ2 decompression step itself. This is
  the highest-value target: it is the primary data path and runs continuously.
- **S3 `ListObjectsV2` XML responses** — parsed by `quick-xml` in `crates/radar-workstation`.
- **HTTP response framing** (status line, headers, chunked transfer encoding) — parsed by
  `crates/http-ingest`.

Everything else in the render/UI layer is currently design-only (see the root
[`README.md`](README.md) for implementation status) and out of scope until it exists.
`utility/` is explicitly non-production tooling with no stability guarantee — see
[`utility/README.md`](utility/README.md) — and is out of scope. The
`crates/http-ingest/fuzz` crate is excluded from the workspace and does not ship.

## Security posture

- **Owned HTTP boundary.** `crates/http-ingest` is a workspace-local HTTP/1.1 client
  purpose-built for the S3 acquisition path, with a compile-time host allowlist, ALPN
  pinned to HTTP/1.1 (so HTTP/2 can never be negotiated), ~1,845 lines of first-party
  code with dedicated parser tests and a fuzz corpus gated on stable `cargo test`. See
  [ADR-0014](docs/adr/0014-http-ingest-own-the-boundary.md).
- **Zero-dependency NEXRAD decoder.** `crates/nexrad-decoder` has no third-party
  dependencies at all — the entire attack surface on the primary, continuously-running
  data path is first-party code. See [ADR-0008](docs/adr/0008-custom-decoder.md).
- **Pure-Rust decompression on the attacker-influenced path.** BZ2 decompression of
  chunk bytes uses `libbz2-rs-sys`, a pure-Rust implementation, not a C binding. See
  [ADR-0015](docs/adr/0015-bzip2.md).
- **Overflow checks enabled in release builds** (`overflow-checks = true` in the root
  `Cargo.toml`'s `[profile.release]`) — arithmetic overflow panics rather than wrapping
  silently, even outside debug builds.
- **No telemetry, no undisclosed network connections.** The only network connections are
  those the user explicitly configures (radar site, tile provider, placefile URLs).
- **`cargo-deny` and `cargo-audit` run in CI** against every push and pull request.

## What is out of scope

- `utility/` — explicitly non-production, no stability guarantee.
- `crates/http-ingest/fuzz` — workspace-excluded, does not appear in `Cargo.lock`, does
  not ship.
- The render/UI layer — does not exist yet.
