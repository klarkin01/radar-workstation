# Architecture Overview

*This document describes what the system is made of and how the parts relate. It is the
entry point to the architecture directory. For the principles that govern every decision
made here, see [PHILOSOPHY.md](../PHILOSOPHY.md).*

---

## Project Structure

```
~/dev/radar_project/
├── docs/
│   ├── PHILOSOPHY.md
│   ├── architecture/
│   │   ├── overview.md          ← this document
│   │   ├── data-flow.md
│   │   └── rendering.md
│   ├── adr/
│   │   ├── 0001-use-rust.md
│   │   ├── 0002-use-egui.md
│   │   └── ...
│   └── open-questions.md
└── src/
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
Polls the NOAA/AWS NEXRAD data feed for new volume scans. Downloads and queues incoming
scan files asynchronously. Also responsible for map tile fetching and placefile retrieval.
All network I/O is non-blocking. The render loop is never waiting on the data pipeline.
See [data-flow.md](data-flow.md) for detail.

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
The single source of truth for the running application. Holds the current volume scan,
derived products, site configuration, and user settings. Written by the data pipeline and
compute layer. Read by the rendering subsystem every frame. Access is coordinated via
Rust's `Arc<RwLock<>>` — multiple readers, exclusive writers, no data races by construction.

### Basemap Data
Census TIGER/Line shapefiles (counties, states, highways) and Natural Earth data
(country boundaries, coastlines) are bundled with the binary. Loaded once at startup,
tessellated into GPU geometry, and held in memory. NEXRAD site locations are loaded from
a bundled JSON file derived from the NOAA site list. None of this data requires a network
connection.

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
