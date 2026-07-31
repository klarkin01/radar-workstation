# Radar Workstation, Meteorological

A single-site NEXRAD Level II radar analysis application for Linux, written in Rust.
It is built for people who use radar seriously during active severe weather — storm
chasers, National Weather Service staff, and emergency managers — and it targets the
same use case as [GR2Analyst](https://www.grlevelx.com/) (Windows-only, $250/license).

## Status

**There is no runnable application yet.** `crates/radar-workstation/src/main.rs` is a
four-line stub that prints a startup banner and exits. This is early, not abandoned —
here is what exists and what doesn't:

- **Implemented and tested:** the NEXRAD decoder (Message 31 parsing,
  `crates/nexrad-decoder`), the workspace-local HTTP/1.1 client
  (`crates/http-ingest`, [ADR-0014](docs/adr/0014-http-ingest-own-the-boundary.md)),
  and the chunk ingest layer (S3 chunk-stream polling, chunk detection, BZ2
  decompression, in `crates/radar-workstation`).
- **Design-only, not yet code:** the volume assembly state machine
  ([ADR-0012](docs/adr/0012-volume-assembly-state-machine.md)), the compute layer,
  shared application state, and the render loop (egui/wgpu).

For something you can actually run today, see `utility/` — `fetch-sample` and
`decode-sample` (fetch and decode real chunk data from S3) and `radar-viz` (render a
decoded scan to a PNG). These are development tools with no stability guarantee, not
part of the product; see [`utility/README.md`](utility/README.md).

## Build and test

```
cargo build --release
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
cargo audit
```

The bare `cargo build` / `cargo test` (without `--workspace`) are scoped by
`default-members` to the three production crates (`radar-workstation`,
`nexrad-decoder`, `http-ingest`), excluding the `utility/` dev tools — deliberate,
but worth knowing when a command doesn't touch a file you just changed. `cargo-deny`
and `cargo-audit` are not part of the toolchain and need a one-time install:

```
cargo install --locked cargo-deny --version 0.20.2
cargo install --locked cargo-audit --version 0.22.2
```

## Repository layout

| Path | Contents |
|---|---|
| `crates/radar-workstation` | The application: chunk ingest, S3 polling, and (eventually) volume assembly, compute, and render |
| `crates/nexrad-decoder` | Custom NEXRAD Level II Message 31 decoder, zero third-party dependencies |
| `crates/http-ingest` | Workspace-local HTTP/1.1 client purpose-built for the S3 acquisition path ([ADR-0014](docs/adr/0014-http-ingest-own-the-boundary.md)) |
| `utility/` | Development-only tools: not part of the product, no stability guarantee — see its own README |
| `docs/` | Design philosophy, requirements, architecture, and ADRs — see [`docs/README.md`](docs/README.md) |

## Documentation map

Start with [`docs/PHILOSOPHY.md`](docs/PHILOSOPHY.md) — it predates and supersedes every
architectural decision in this repository. From there:

- [`docs/REQUIREMENTS.md`](docs/REQUIREMENTS.md) — functional and non-functional requirements
- [`docs/architecture/overview.md`](docs/architecture/overview.md) — technology stack, project structure, subsystem overview
- [`docs/adr/`](docs/adr/) — architectural decision records, one per significant technical choice
- [`docs/open-questions.md`](docs/open-questions.md) — design questions still open, and what they block

The full index, including one-line descriptions of every document, is in
[`docs/README.md`](docs/README.md).

## Security posture

- No telemetry, and no network connection the user has not explicitly configured.
- The S3 client has a compile-time host allowlist — it cannot be pointed at an
  arbitrary host.
- Pinned toolchain (`rust-toolchain.toml`) and a tracked `Cargo.lock`.
- `cargo-deny` and `cargo-audit` are gated in CI.
- CI uses no third-party GitHub Actions beyond `checkout`, pinned by commit SHA.
- Memory-safe by construction: `ring` (pulled in transitively for TLS) is the only
  non-Rust code in the production dependency graph.

See [`SECURITY.md`](SECURITY.md) for the vulnerability disclosure process and full
threat-model scope.

## License

Apache License, Version 2.0. See [`LICENSE`](LICENSE).
