# Radar Workstation, Meteorological

A single-site NEXRAD Level II radar analysis application for Linux, written in Rust.
It is built for people who use radar seriously during active severe weather — storm
chasers, National Weather Service staff, and emergency managers — and it targets the
same use case as [GR2Analyst](https://www.grlevelx.com/) (Windows-only, $250/license).

## Status

**Stage 4 complete** (`docs/plans/stage-4-first-pixels.md`).
`cargo run --release -- KDOX` opens a window and draws the radar: it polls the live
chunk stream, decodes and assembles volumes, **grids every in-scope product for every
sweep and derives Echo Tops/VIL for every completed volume**, and draws the selected
product in azimuthal-equidistant projection over range rings and a site marker. Pan and
zoom (drag / arrows / wheel) never lose spatial context; product (`1`–`7`) and sweep
(`PageUp`/`PageDown`) switch as pure GPU state changes; the cursor reads out
range/azimuth/beam-height/value; a status bar surfaces every pipeline event. It draws
**no map, no tiles, no placefiles** yet — those are Stages 5–6. `--headless` runs the
terminal state-transition loop instead (for a server or CI). Here is what exists and
what doesn't:

- **Implemented and tested:** the NEXRAD decoder (Message 31 parsing,
  `crates/nexrad-decoder`), the workspace-local HTTP/1.1 client
  (`crates/http-ingest`, [ADR-0014](docs/adr/0014-http-ingest-own-the-boundary.md)),
  the chunk ingest layer (S3 chunk-stream polling, chunk detection, BZ2
  decompression), the volume assembly state machine
  ([ADR-0012](docs/adr/0012-volume-assembly-state-machine.md)), the compute layer
  (gridding, colour tables, Echo Tops/VIL —
  [ADR-0020](docs/adr/0020-product-texture-representation.md),
  [ADR-0021](docs/adr/0021-colour-table-format.md)), shared application state
  ([ADR-0018](docs/adr/0018-shared-application-state.md)), the winit/wgpu/egui render
  loop ([ADR-0022](docs/adr/0022-render-loop-hosting.md),
  [ADR-0023](docs/adr/0023-radar-sampling-in-screen-space.md)), the tokio
  runtime/task-supervision skeleton, the bundled WSR-88D site list, and configuration
  persistence ([ADR-0019](docs/adr/0019-config-format.md)) — all in
  `crates/radar-workstation`.
- **Design-only, not yet code:** map underlay and vector overlays (Stage 5), map imagery
  tiles (Stage 5), placefiles (Stage 6), runtime site change (Stage 7).

For something else you can run today, see `utility/` — `fetch-sample` and
`decode-sample` (fetch and decode real chunk data from S3) and `radar-viz` (render a
decoded scan to a PNG). These are development tools with no stability guarantee, not
part of the product; see [`utility/README.md`](utility/README.md).

## Running it

```
cargo run --release -- KDOX          # open the radar window for site KDOX
cargo run --release -- KDOX --headless   # terminal state-transition loop, no window
```

| Key | Action |
|---|---|
| `1`–`7` | product: reflectivity, velocity, spectrum width, ZDR, CC, echo tops, VIL |
| `PageUp` / `PageDown` | next / previous elevation |
| Arrow keys / left-drag | pan |
| `+` / `-` / wheel | zoom (wheel zooms about the cursor) |
| `Home` | reset view (site-centred, 230 km) — product and tilt kept |
| `R` | toggle range rings and spokes |
| `F1` or `?` | key-help overlay |
| `Ctrl+Q` | quit |

Window geometry and the active product persist to the config file on a clean exit. A
machine with no display or no working GPU adapter exits non-zero and tells you to use
`--headless` — it never silently falls back to the terminal loop.

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
| `crates/radar-workstation` | The application: chunk ingest, S3 polling, volume assembly, compute layer, shared state, the winit/wgpu/egui render loop, runtime/supervision, site list, and configuration |
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
  compiled non-Rust code in the production dependency graph. The Stage 4 render stack
  (winit/wgpu/egui) is pure Rust; it loads the platform's Vulkan/GL/Wayland/X11
  libraries at runtime via `dlopen` (`*-dl` binding crates), not by compiling C.

See [`SECURITY.md`](SECURITY.md) for the vulnerability disclosure process and full
threat-model scope.

## License

Apache License, Version 2.0. See [`LICENSE`](LICENSE).
