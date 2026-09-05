# Architecture Overview

*This document describes what the system is made of and how the parts relate. It is the
entry point to the architecture directory. For the principles that govern every decision
made here, see [PHILOSOPHY.md](../PHILOSOPHY.md).*

> **Implementation status:** Stage 3 complete (`docs/plans/
> stage-3-compute-layer.md`). The NEXRAD decoder (Message 31), the workspace-local
> HTTP/1.1 client (`crates/http-ingest`), the chunk ingest layer (S3 chunk-stream
> polling, chunk detection, BZ2 decompression), the volume assembly state machine
> (ADR-0012), the compute layer (gridding, colour tables, Echo Tops/VIL — ADR-0020,
> ADR-0021), shared application state (ADR-0018), the runtime/supervision skeleton
> (`pipeline.rs`), the bundled site list (`sites.rs`), and configuration persistence
> (ADR-0019) are all implemented and tested — `cargo run --release -- KDOX` polls,
> decodes, assembles, grids every in-scope product, and derives Echo Tops/VIL, all
> visible in the headless output. **Stage 4 (2026-08-28):** the render loop described
> below is now implemented (winit + wgpu + egui, ADR-0022/ADR-0023,
> `crates/radar-workstation/src/render/`) — `cargo run --release -- KDOX` opens a window
> and draws the radar. `--headless` runs the terminal loop above instead. View state
> (pan, zoom, active product/sweep) is genuinely owned by `render::ViewState` and never
> enters `AppState`. Map underlays and placefiles (layers 3–5, 7, 9) remain unbuilt —
> Stages 5–6. Layer 2 (terrain imagery tiles) is **deferred out of v1.0 entirely**
> (ADR-0027, 2026-08-28): v1.0 ships a vector-only basemap. See the root
> [README.md](../../README.md) for the full status statement.

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
| Async I/O | tokio | Non-blocking network I/O for radar data polling and placefile retrieval. Tile fetching is post-v1.0 (ADR-0027). |
| HTTP client | Custom HTTP/1.1 implementation (`crates/http-ingest`) | Workspace-local; no third-party HTTP client. Purpose-built for the S3 acquisition path, with a compile-time host allowlist. See ADR-0014. |
| Data parallelism | rayon (deferred pending measurement — see ADR-0005's erratum) | `tokio::task::spawn_blocking` runs gridding and derived-product computation off the async runtime's workers today; rayon is the next lever if a future workload crosses the measured trigger. |
| Map vector data | Baked overlay bundle | Natural Earth and Census TIGER/Line geometry plus Natural Earth city labels, baked at build time into a flat blob compiled into the binary. No runtime map API dependency, no shapefile parser. ~6.41 MB of bundle over 727,620 points; primary roads simplified at ε = 30 m, the rest at native density. See ADR-0006, ADR-0025, ADR-0028 and ADR-0029. |
| Map imagery | Pluggable XYZ tile providers | **Deferred to post-v1.0 (ADR-0027).** USGS National Map by default; fetched on demand, cached to disk; optional and toggleable when built. v1.0 ships a vector-only basemap. |
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
assembles them into volume scans (ADR-0012). Also responsible for placefile retrieval
(Stage 6); map tile fetching is deferred to post-v1.0 (ADR-0027). All network I/O is non-blocking. The render loop is never waiting
on the data pipeline. See [data-flow.md](data-flow.md) for detail.

The NEXRAD acquisition path (chunk discovery and fetch) speaks HTTP through
`crates/s3-fetch`, over the first-party HTTP/1.1 engine in `crates/http-ingest` — a
separately-audited implementation with its own fuzz corpus and threat model (ADR-0014).
See [ADR-0014](../adr/0014-http-ingest-own-the-boundary.md).

<!-- corrected 2026-08-28 (ADR-0026, resolving Q16): this paragraph previously said
`http-ingest` was "not a general-purpose library shared with the tile or placefile
paths." It now is the shared engine, deliberately — one framing implementation and one
fuzz corpus rather than two. What is *not* shared is policy: `s3-fetch` reaches only the
two ADR-0011 buckets, named by a `Bucket` enum with no hostname in its API, while
`tile-fetch` takes its host from user config. Neither can affect the other's connection
state, error values, or task supervision, and neither follows redirects. See
[ADR-0026](../adr/0026-tile-http-boundary.md).

superseded in part 2026-08-28 (ADR-0027, resolving Q18): the tile subsystem is deferred
to post-v1.0, so `crates/tile-fetch` does not exist and the engine/policy split is
deferred with it — `http-ingest` remains one crate with the S3 policy inside it. The
`Bucket` enum is still taken up now, so the sentence above about `s3-fetch` reaching only
the two ADR-0011 buckets describes the guarantee being built; the crate boundary it
describes does not exist yet. See ADR-0027 §4. -->

### Compute Layer
**Implemented** (Stage 3, `crates/radar-workstation/src/compute/`). A fourth pipeline
stage between assembly and the applier: grids every in-scope product on each closed
sweep (reflectivity, velocity, spectrum width, ZDR, CC — `compute::grid`) and derives
Echo Tops/VIL from the accumulating volume's reflectivity grids at volume close
(`compute::derived`). Colour mapping is a 256-entry palette LUT compiled per product
(`compute::palette`, GRLevelX `.pal` format, ADR-0021), not per-gate arithmetic
(ADR-0020). Runs on `tokio::task::spawn_blocking`, not rayon, at this stage — gridding
measured close to a `memcpy` (1.4 ms/sweep average), well under rayon's justification
threshold; the derived-products pass measured above it, and is the recorded trigger to
revisit (ADR-0005's erratum). Results are written into shared application state through
the same applier the assembler used to write to directly. Computation never blocks the
UI (which does not exist yet) or the poller — a live end-to-end test asserts
`IngestStatus` stays `Polling` throughout.

### NEXRAD Decoder
Parses raw NEXRAD Level II archive files into an internal volume scan representation.
Implemented against the NCEI format specification. Treated as an internal library with
its own test suite. The decoder is the foundation — everything else depends on it being
correct.

### Shared Application State
The single source of truth for radar data — **not** user settings or view state, which
the render loop owns outright and never shares (see [ADR-0018](../adr/0018-shared-application-state.md),
Q4's resolution). Holds a bounded ring of the most recently completed volumes, each as
its own gridded sweeps and derived products (Echo Tops/VIL) — not raw radials, released
once gridded (S3-g); not a per-tilt merge, since a retained volume must show only what
that volume actually scanned (Stage 6a Part B,
[ADR-0030](../adr/0030-volume-history-retention.md)). The merged live view Stage 5
displayed is now a read-time fold over that ring. Written by the data pipeline and
compute layer through a narrow apply/report API; read by the rendering subsystem every
frame through a single `snapshot()` call that returns owned data. `Arc<AppState>`
internally holds `RwLock<RadarState>` — one lock, scoped to radar data only, not an
outer lock over the whole application — multiple readers, exclusive writers, no data
races by construction. See [data-flow.md](data-flow.md) for the full structure.

### Basemap Data
Vector overlay geometry — counties, states, country boundaries, coastlines (Natural Earth
10m) and major highways (Census TIGER/Line Primary Roads) — is baked at build time into a
single flat binary bundle compiled into the executable, and projected into azimuthal
equidistant coordinates once per site load (ADR-0025). The same bundle carries **city
labels** (Natural Earth 10m `populated_places`, ~27 KiB) as a label index plus a UTF-8
string table; they are drawn by egui and selected by a screen-space declutter pass rather
than uploaded as GPU geometry (ADR-0028). The label source is deliberately provisional and
known-sparse — 19 labels inside a KDOX 230 km PPI — and the runtime representation is
source-agnostic so a denser source is a bundle regeneration. No shapefile parser, DBF reader, or
tessellator is in the production graph, and nothing is parsed at startup. NEXRAD site
locations are compiled
directly into the binary as a generated `const` Rust table (`crates/radar-workstation/
src/sites_generated.rs`, `crate::sites`) derived from the NOAA site registry — not a
JSON file parsed at startup. See the two ADR-0006 errata. None of this data requires a
network connection.

---

## Layer Rendering Order

From bottom to top, as composited by the rendering subsystem each frame:

1. Background (solid dark color; optional terrain imagery tiles are post-v1.0, ADR-0027)
2. County boundaries
3. State and country boundaries
4. Major highways
5. Radar data (polar grid, color-mapped to active product)
6. Placefile overlays (warnings, storm reports, lightning, etc.)
7. Radar site markers and labels
8. City labels (bundled label data; screen-space declutter pass, drawn by egui at
   `Order::Background` — see ADR-0028)
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
