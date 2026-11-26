# Radar Workstation, Meteorological

A single-site NEXRAD Level II radar analysis application for Linux, written in Rust.
It is built for people who use radar seriously during active severe weather — storm
chasers, National Weather Service staff, and emergency managers — and it targets the
same use case as [GR2Analyst](https://www.grlevelx.com/) (Windows-only, $250/license).

## Status

**Stage 5 complete** (`docs/plans/stage-5-map-underlays.md`), and **Stage 6a Parts A–C
complete** (`docs/plans/stage-6a-time-handling.md`): ingest is wrap-safe across the chunk
bucket's 999 → 1 volume counter, the app keeps a bounded history of recent volumes, and a
timeline loops, steps and pauses over it (`Space`, `[` `]`, `End`) behind an unmissable
LIVE / PAUSED / LOOP badge, with the status bar's age always measuring the live feed
([ADR-0031](docs/adr/0031-timeline-semantics.md)). Part C's windowed validation run is
recorded in its plan's §10. **Part D1, launch backfill, is done**: once the live volume
has arrived, the loop is filled from the chunk bucket in about a minute rather than over
40–60 minutes of watching ([ADR-0033](docs/adr/0033-launch-backfill.md)). Archive
playback (D2–D4) is not started.
`cargo run --release -- KDOX` opens a window and draws the radar over a map: it polls the
live chunk stream, decodes and assembles volumes, **grids every in-scope product for
every sweep and derives Echo Tops/VIL for every completed volume**, and draws the
selected product in azimuthal-equidistant projection over county, state/country,
coastline and (toggleable, `H`) primary-road geometry, range rings, every bundled site's
marker, and declutter-selected city/site labels — all from a bundle baked into the
binary, no network, no runtime parser. Pan and zoom (drag / arrows / wheel) never lose
spatial context; product (`1`–`7`) and sweep (`PageUp`/`PageDown`) switch as pure GPU
state changes; the cursor reads out range/azimuth/beam-height/value; a status bar
surfaces every pipeline event. It draws **no tiles, no placefiles** yet — tiles are
deferred past v1.0 ([ADR-0027](docs/adr/0027-tile-image-decoding.md)) and placefiles are
Stage 6. `--headless` runs the terminal state-transition loop instead (for a server or
CI). Here is what exists and what doesn't:

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
  [ADR-0023](docs/adr/0023-radar-sampling-in-screen-space.md)), the map underlay bundle
  and its reader/projection ([ADR-0025](docs/adr/0025-bundled-overlay-geometry.md),
  [ADR-0028](docs/adr/0028-city-labels.md), [ADR-0029](docs/adr/0029-primary-roads-simplification.md)),
  wrap-aware volume selection and the scan-age readout
  ([`stage-6a-part-a-stale-data.md`](docs/plans/stage-6a-part-a-stale-data.md)),
  volume history retention ([ADR-0030](docs/adr/0030-volume-history-retention.md)), the
  timeline over it ([ADR-0031](docs/adr/0031-timeline-semantics.md)), launch backfill
  of that history ([ADR-0033](docs/adr/0033-launch-backfill.md)), the
  tokio runtime/task-supervision skeleton, the bundled WSR-88D site list, and
  configuration persistence ([ADR-0019](docs/adr/0019-config-format.md)) — all in
  `crates/radar-workstation`.
- **Design-only, not yet code:** archive playback (Stage 6a Part D2–D4), map
  imagery tiles (deferred post-v1.0, [ADR-0027](docs/adr/0027-tile-image-decoding.md)),
  placefiles (Stage 6), runtime site change (Stage 7).

History retention is bounded by two config keys, `history.frames` (default 12) and
`history.budget_mb` (default 320). Retained volumes cost 28–40 MB each, so the default
adds up to 320 MB per instance. `history.budget_mb = 0` brings back the Stage 5
footprint. The loop plays the newest `timeline.loop_frames` (default 12) of them. At launch, the
history is backfilled from the bucket on a second connection; `history.backfill = false`
turns that off. Every config key is listed in
[`QUICK-START.md`](QUICK-START.md#configuration).

For something else you can run today, see `utility/` — `fetch-sample` and
`decode-sample` (fetch and decode real chunk data from S3) and `radar-viz` (render a
decoded scan to a PNG). These are development tools with no stability guarantee, not
part of the product; see [`utility/README.md`](utility/README.md).

## Running it

```
cargo run --release -- KDOX          # open the radar window for site KDOX
cargo run --release -- KDOX --headless   # terminal state-transition loop, no window
```

[`QUICK-START.md`](QUICK-START.md) is the operator guide. It has the keyboard map, what
the status bar shows, and every configuration key. A machine with no display or no
working GPU adapter exits non-zero and tells you to use `--headless`. It never silently
falls back to the terminal loop.

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
| `crates/fuzz-support` | Dev-only seeded mutator for parser hardening tests. A `[dev-dependencies]` path dependency only, never in a shipped binary |
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
