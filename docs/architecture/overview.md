# Architecture Overview

*This document describes what the system is made of and how the parts relate. It is the
entry point to the architecture directory. For the principles that govern every decision
made here, see [PHILOSOPHY.md](../PHILOSOPHY.md).*

> **Implementation status:** Stage 2 complete (`docs/plans/
> stage-2-make-the-application-exist.md`). The NEXRAD decoder (Message 31), the
> workspace-local HTTP/1.1 client (`crates/http-ingest`), the chunk ingest layer (S3
> chunk-stream polling, chunk detection, BZ2 decompression), the volume assembly state
> machine (ADR-0012), shared application state (ADR-0018), the runtime/supervision
> skeleton (`pipeline.rs`), the bundled site list (`sites.rs`), and configuration
> persistence (ADR-0019) are all implemented and tested — `cargo run --release -- KDOX`
> is a real, runnable program. The compute layer and render loop described below are
> still architecture, not yet code; `main.rs`'s headless placeholder loop is what Stage 4
> replaces. See the root [README.md](../../README.md) for the full status statement.

---

## Project Structure

```
radar-workstation/
├── Cargo.toml                     ← workspace manifest (virtual, see ADR-0010)
├── crates/
│   ├── radar-workstation/         ← application crate (binary)
│   ├── nexrad-decoder/            ← decoder library crate
│   └── http-ingest/               ← workspace-local HTTP/1.1 client — ADR-0014
├── utility/                       ← dev-only tooling, not part of the product
│   ├── nexrad-inspect/            ← Python cross-validation scripts (MetPy-based)
│   ├── nexrad-sample/             ← fetch/decode sample chunks from S3
│   ├── nexrad-sites/              ← generate the bundled WSR-88D site table
│   └── radar-viz/                 ← render a decoded scan to PNG for visual checks
└── docs/
    ├── README.md                  ← documentation index
    ├── PHILOSOPHY.md
    ├── REQUIREMENTS.md
    ├── dependency-inventory.md
    ├── documentation-inventory.md
    ├── architecture/
    │   ├── overview.md            ← this document
    │   ├── data-flow.md
    │   ├── rendering.md
    │   ├── nexrad-binary-format.md
    │   └── nexrad-data-types.md
    ├── adr/
    │   ├── 0001-use-rust.md
    │   ├── 0002-use-egui.md
    │   └── ...
    ├── plans/                     ← executable work plans, retained as a record after execution
    └── open-questions.md
```

---

## System Summary

A single-site NEXRAD Level II radar analysis application. Each running instance is
independent — monitoring one radar site, maintaining its own data pipeline, and rendering
to its own window. Multiple instances run simultaneously without shared state or resource
contention. There is no server component, no database, and no background service. The
application starts, runs, and exits cleanly as a normal user process.

---

## Technology Stack

| Concern | Choice | Rationale |
|---|---|---|
| Language | Rust | Memory safety by construction, native performance, no GC, strong concurrency model. See ADR-0001. |
| UI framework | egui | Immediate mode, pure Rust, GPU-accelerated, minimal overhead. Does not need to match OS widget style. See ADR-0002. |
| GPU rendering | wgpu | Cross-platform Vulkan/OpenGL abstraction. Radar data rendered directly to GPU surface, bypassing egui's renderer. |
| Async I/O | tokio | Non-blocking network I/O for radar data polling, tile fetching, and placefile retrieval. |
| HTTP client | Custom HTTP/1.1 implementation (`crates/http-ingest`) | Workspace-local; no third-party HTTP client. Purpose-built for the S3 acquisition path, with a compile-time host allowlist. See ADR-0014. |
| Data parallelism | rayon | CPU-bound product computation distributed across cores without blocking the render loop. |
| Map vector data | Bundled shapefiles | Census TIGER/Line and Natural Earth data shipped with the binary. No runtime map API dependency. |
| Map imagery | Pluggable XYZ tile providers | USGS National Map by default. Fetched on demand, cached to disk. Optional and toggleable. |
| NEXRAD decoding | Custom implementation | Written against the NCEI Level II format specification. Owned entirely by this project. |

---

## Subsystem Overview

### UI Layer (egui)
Owns the application window, menus, toolbars, product selector, color scale legend, site
selector, and status bar. Hosts the wgpu render surface as an embedded panel. Does not
render radar data directly — it delegates that entirely to the rendering subsystem.

### Rendering Subsystem (wgpu)
Renders all geospatial content: map underlay, radar data, vector overlays, and placefile
content. Operates as a custom wgpu render pass embedded within the egui frame. Reads from
shared application state every frame. Never blocks on I/O or computation. See
[rendering.md](rendering.md) for detail.

### Data Pipeline (tokio)
Polls the Unidata AWS NEXRAD real-time chunk stream (ADR-0011) for new chunks and
assembles them into volume scans (ADR-0012). Also responsible for map tile fetching and
placefile retrieval. All network I/O is non-blocking. The render loop is never waiting
on the data pipeline. See [data-flow.md](data-flow.md) for detail.

The NEXRAD acquisition path (chunk discovery and fetch) speaks HTTP through
`crates/http-ingest` — a separately-audited, first-party HTTP/1.1 client with its own
fuzz corpus and threat model (ADR-0014), not a general-purpose library shared with the
tile or placefile paths. See [ADR-0014](../adr/0014-http-ingest-own-the-boundary.md).

### Compute Layer (rayon)
Receives decoded volume scans and derives products: Echo Tops, VIL, VILD, dual-pol
products, and others. Work is distributed across CPU cores via rayon's thread pool.
Results are written into shared application state when complete. Computation never
blocks the UI or render loop.

### NEXRAD Decoder
Parses raw NEXRAD Level II archive files into an internal volume scan representation.
Implemented against the NCEI format specification. Treated as an internal library with
its own test suite. The decoder is the foundation — everything else depends on it being
correct.

### Shared Application State
The single source of truth for radar data — **not** user settings or view state, which
the render loop owns outright and never shares (see [ADR-0018](../adr/0018-shared-application-state.md),
Q4's resolution). Holds the newest closed sweep per elevation, the last complete volume
scan, and (from Stage 3 on) derived product textures. Written by the data pipeline and
compute layer through a narrow apply/report API; read by the rendering subsystem every
frame through a single `snapshot()` call that returns owned data. `Arc<AppState>`
internally holds `RwLock<RadarState>` — one lock, scoped to radar data only, not an
outer lock over the whole application — multiple readers, exclusive writers, no data
races by construction. See [data-flow.md](data-flow.md) for the full structure.

### Basemap Data
Census TIGER/Line shapefiles (counties, states, highways) and Natural Earth data
(country boundaries, coastlines) are bundled with the binary. Loaded once at startup,
tessellated into GPU geometry, and held in memory. NEXRAD site locations are compiled
directly into the binary as a generated `const` Rust table (`crates/radar-workstation/
src/sites_generated.rs`, `crate::sites`) derived from the NOAA site registry — not a
JSON file parsed at startup; see the ADR-0006 erratum. None of this data requires a
network connection.

---

## Layer Rendering Order

From bottom to top, as composited by the rendering subsystem each frame:

1. Background (solid dark color, or optional terrain imagery tiles)
2. County boundaries
3. State and country boundaries
4. Major highways
5. Radar data (polar grid, color-mapped to active product)
6. Placefile overlays (warnings, storm reports, lightning, etc.)
7. Radar site markers and labels
8. City labels (at sufficient zoom)
9. egui UI chrome (drawn last, on top of everything)

---

## What Each Instance Does Not Do

- No telemetry or callbacks to any external server beyond configured data sources.
- No elevated privileges. Runs entirely as a normal user process.
- No shared state with other running instances.
- No background service or daemon.
- No installation of system files outside the application directory.

---

## Related Documents

- [PHILOSOPHY.md](../PHILOSOPHY.md) — the principles that govern all decisions made here
- [data-flow.md](data-flow.md) — how radar data moves from NOAA to the display
- [rendering.md](rendering.md) — how the GPU render pipeline is structured
- [../adr/](../adr/) — records of significant architectural decisions and their rationale
- [../open-questions.md](../open-questions.md) — unresolved design questions
