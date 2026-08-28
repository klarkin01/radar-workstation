# Rendering

*How the application draws to the screen. For how data arrives at the render loop,
see [data-flow.md](data-flow.md). For the principles governing these choices, see
[PHILOSOPHY.md](../PHILOSOPHY.md).*

---

## Overview

Every frame, the render loop reads from shared application state and produces a complete
image. It never blocks on I/O, never initiates a network request, and never performs
product computation. It reads, it draws, it presents. That is all.

The render loop is split between two systems that share a window but not a render
pipeline:

- **wgpu** renders all geospatial content — map imagery, vector overlays, and radar data.
- **egui** renders all UI chrome — menus, toolbars, panels, labels, and controls.

egui is drawn last, on top of wgpu output, every frame.

---

## Frame Lifecycle

```
1. Begin frame
      │
      ▼
2. Acquire read lock on AppState (brief)
      │
      ▼
3. Upload any new textures to GPU
   (new radar scan, new map tiles — only when state has changed)
      │
      ▼
4. wgpu render pass — geospatial content
   (see Layer Rendering Order below)
      │
      ▼
5. egui render pass — UI chrome
      │
      ▼
6. Present frame to display
      │
      ▼
7. Release read lock, sleep until next frame
```

Texture uploads (step 3) only occur when new data has arrived. During steady-state
display between scans, the GPU renders from already-uploaded textures. CPU involvement
per frame is minimal.

---

## GPU Adapter Selection

*Added 2026-08-28, Stage 4 follow-up ([ADR-0024](../adr/0024-gpu-adapter-selection.md)).
Supersedes S4-a's `PowerPreference::LowPower`.*

The render loop must run on the GPU that **drives the display**. On a hybrid-GPU Linux
box the lowest-power adapter is the integrated one, which frequently drives no connected
output; a compositor that scans out only on the discrete GPU rejects the integrated
adapter's swapchain dmabufs asynchronously, and the Wayland connection is torn down
several frames after a "successful" init.

So `render/gpu.rs` does not express a power preference. It enumerates every Vulkan/GL
adapter, then `render/adapter.rs` **ranks** them: an adapter whose PCI address (or, on the
GL backend, whose vendor/device id) matches a `connected` DRM connector discovered from
`/sys/class/drm` wins; with no display information discoverable, a presentable discrete
GPU is the tiebreak; a non-presentable adapter and (unless nothing else can present) a
software rasteriser are excluded. Ranking is a prediction, so it is **verified by
bring-up**: the first candidate that requests a device, configures the surface without a
validation error, and yields one frame is kept. Exhausting the list is a clean
`NoPresentableAdapter` error naming every adapter tried.

A post-init presentation loss (the dmabuf case, which bring-up cannot catch) is turned
into an accurate `PresentationLost` message — by a guard on the reconfigure path, and, as
a fallback, by mapping any event-loop error that lands within 10 s of the first frame to
the same cause. Never a bare `Exit Failure: 1`, and never a mid-session adapter switch.

**Override.** `RADAR_GPU` bypasses ranking (not the bring-up check): a PCI address
(`0000:01:00.0`), a case-insensitive adapter-name substring (`nvidia`), or `discrete` /
`integrated`. An unmatched value lists the available adapters and exits non-zero.
`WGPU_BACKEND` still pins the backend; nothing else in the environment affects selection.

The startup line names the adapter and why it was chosen, e.g.
`GPU: NVIDIA GeForce RTX 4070 SUPER (Vulkan, 0000:01:00.0) — matched connected display`.

---

## Coordinate System

All geospatial rendering uses a single coordinate space: **azimuthal equidistant
projection centered on the active radar site.** This is the conventional projection
for single-site radar display. It preserves distance and direction from the site,
which is the correct reference frame for radar interpretation.

The coordinate transform pipeline is:

```
NEXRAD polar coords          Geographic coords (WGS84)
(range, azimuth, elevation)  (lat, lon)
         │                         │
         ▼                         ▼
    Cartesian (km)     →    Azimuthal equidistant (km)
    centered on site         centered on site
              │
              ▼
         Screen pixels
         (zoom + pan)
```

Vector overlay data (shapefiles) is pre-projected from WGS84 into azimuthal equidistant
coordinates at load time, not at render time. The GPU receives already-projected geometry.

Map imagery tiles are fetched in Web Mercator (the XYZ tile standard) and reprojected
on the GPU via a vertex shader. Tile reprojection is a one-time cost per tile, cached
with the tile texture.

---

## Layer Rendering Order

Layers are composited by wgpu in this order, back to front:

| Order | Layer | Source | Notes |
|---|---|---|---|
| 1 | Background | Solid color | Dark by default. Always present. |
| 2 | Terrain imagery | XYZ tile cache | Optional, toggleable. Transparent until tiles load. |
| 3 | County boundaries | Bundled shapefile | Always visible. |
| 4 | State / country boundaries | Bundled shapefile | Always visible. Slightly thicker line weight than counties. |
| 5 | Major highways | Bundled shapefile | Toggleable. Shown at all zoom levels. |
| 6 | Radar data | Selected gridded product texture | The primary content. Alpha-composited over map layers. |
| 6a | Reference geometry | Range rings, azimuth spokes, site marker (Stage 4) | `LineList` in world metres through the view uniform. Toggled by `R`. Chrome of the same kind as the radar image; drawn with it. Ring labels drawn by egui. |
| 7 | Placefile overlays | Parsed placefile data | Drawn per-placefile in user-configured order. |
| 8 | Radar site markers | Bundled site list | Small icon + ICAO label. Clickable for site selection (Stage 7). Stage 4 draws only the active site's marker, as part of layer 6a. |
| 9 | City labels | Bundled label data | Appear above a zoom threshold only. |

egui UI chrome is composited on top of all wgpu layers by the egui render pass.

**Stage 4 status:** only layers 1, 6, 6a, and 8 (active-site marker) have a data source
today. Layers 2–5, 7, and 9 are gated on Stages 5–6 (Q15, Q16, Q6). The two-pass frame
(pass 1: clear + scene; pass 2: egui) does not change as those layers land — they slot
into pass 1 in this order.

---

## Radar Data Rendering

Radar data is the most performance-critical rendering path. The approach:

### Texture-Based Rendering

<!-- corrected 2026-08-05 (ADR-0020, Stage 3, S3-a): this section previously said
products are "pre-computed as RGBA textures." Measured against the 128 MB per-instance
GPU budget (§4.1) for the full seven-moment v1.0 set, RGBA does not fit (~200 MB);
R8 + a 256-entry palette LUT does (~50 MB). See ADR-0020 for the full arithmetic. -->

Each `(sweep, product)` is gridded by the compute layer as a single-channel 8-bit
texture (`compute::grid::SweepGrid`) — the grid cell *is* the raw NEXRAD value — plus a
256-entry RGBA palette lookup table (`compute::palette::ColorLut`, one per product) that
the fragment shader samples once per pixel. Both are uploaded to the GPU once per new
scan; the render loop draws the radar texture as a full-screen quad, alpha-composited
over the map layers below.

This means **no per-frame color mapping.** Colour mapping happens once in the compute
layer, at most 256 times per product per scan (`compute::palette::compile_lut`), not
once per gate and not once per frame. The GPU's fragment shader does one 1D texture
lookup per pixel; this is the primary reason the render loop is fast.

### Polar Grid Representation

<!-- corrected 2026-07-30: this section previously stated the grid as "1km range gates
× 1° azimuth bins × 230km range," described as "matching the native NEXRAD resolution."
That figure was off by a factor of four in range and two in azimuth against confirmed
data, and it contradicted FR-ND-3 (both resolution variants must be supported) by
discarding super-resolution at the render stage. Corrected below with figures measured
directly from the decoder and its fixtures, per
docs/plans/documentation-remediation.md W7. See also
docs/architecture/nexrad-binary-format.md for the underlying byte-level layout. -->

The radar texture is generated on a polar coordinate grid. Measured from the five KDOX
VCP 35 (super-resolution) fixtures in `crates/nexrad-decoder/tests/fixtures/`, using
`ProductData::{gate_count, first_gate_m, gate_width_m}` and `Radial::{azimuth_deg,
azimuth_number}` (see `crates/nexrad-decoder/src/types/{product,radial}.rs`):

- **Gate width:** 0.25 km, uniform across every moment and every tilt observed.
- **First gate:** 2.125 km, uniform across every moment and every tilt observed.
- **Azimuthal spacing:** ~0.5° (measured 0.508° between consecutive azimuth numbers in
  the fixture set), consistent with super-resolution.
- **Maximum range varies by moment and by tilt**, not by a single fixed figure:

  | Tilt (elevation) | Reflectivity | Velocity / spectrum width | Dual-pol (ZDR/PHI/RHO/CFP) |
  |---|---|---|---|
  | 1 (~0.39°, surveillance split-cut) | 460.125 km (1832 gates) | absent on this cut | 300.125 km (1192 gates) |
  | 2 (~0.26°, Doppler split-cut) | 300.125 km (1192 gates) | 300.125 km (1192 gates) | absent on this cut |
  | 16 (~6.37°, highest measured tilt) | 174.125 km (688 gates) | 174.125 km (688 gates) | 174.125 km (688 gates) |

  The 230 km figure in the document this replaced does not match any measured
  moment/tilt combination.

**Standard-resolution geometry, measured 2026-07-31 (S1-W4d).** A real KTLH VCP 212
volume gave the decoder its first standard-resolution fixtures
(`crates/nexrad-decoder/tests/fixtures/ktlh_vcp212_*.bin`). Measured directly against
consecutive `az_angle` values across the full volume (also cross-checked against a full
KDOX VCP 35 volume in `downloads/KDOX_20260629_1811/`):

- **Gate width and first gate are identical to super-resolution** on the same site/VCP:
  0.25 km gates, 2.125 km first gate. Resolution does not change gate geometry.
- **Azimuthal spacing is the only thing that differs:** 1.0° (360 radials per 360°
  sweep) for standard-resolution elevations, vs. 0.5° (720 radials per sweep) for
  super-resolution ones — both measured directly, not assumed from the ICD.
- This also corrected `docs/architecture/nexrad-binary-format.md` §6.1: the `az_spacing`
  field's code meaning was previously documented backwards (code 1 was stated as 1.0°
  and code 2 as super-resolution 0.5°; measurement shows the reverse — code 1 is
  super-resolution, code 2 is standard-resolution).

FR-ND-3's requirement that the decoder support both variants is now verified against
real data. **Q17 is resolved (2026-08-05, Stage 3, S3-d):** neither a shared format nor
two per-resolution formats — each `SweepGrid` carries **its own native dimensions**
(`azimuth_count`, `gate_count`, `first_gate_m`, `gate_width_m`), taken as shader
uniforms, never padded, never upsampled. Once the representation is R8 + a 256-entry
palette LUT (ADR-0020), the premise behind "one format vs. two" stops applying: the
range dimension varies far more across measured tilts (688 to 1832 gates) than azimuth
resolution does (360 vs 720), so no fixed format is efficient regardless of resolution,
and a texture *array* — the one representation that would actually require uniform
dimensions — isn't needed, since only one (product, sweep) pair is drawn at a time.
Upsampling standard-resolution to super-resolution was rejected on a stronger ground
than memory: it would fabricate radials the antenna never measured. Full rationale in
[ADR-0020](../adr/0020-product-texture-representation.md).

The polar grid is mapped to the azimuthal equidistant projection coordinate space by the
vertex shader — that mechanism does not depend on the specific gate/azimuth resolution.

<!-- corrected 2026-08-28 (Stage 4, S4-b / ADR-0023): the mapping is done in the
**fragment** shader, not the vertex shader. The radar pass draws one full-screen
triangle; per covered pixel the fragment shader inverse-maps screen -> (ground range,
azimuth), converts ground range to slant range with the 4/3-earth model
(`compute::geometry::slant_range_and_height`'s closed form, skipped for the ground-range
derived products), computes the gate index and the azimuth slot
(`floor(az / spacing)`, matching `compute::grid::azimuth_slot`), does one `textureLoad`
against the `R8Uint` grid and one lookup into a 256-entry palette LUT uniform. There is
no mesh and no per-vertex projection; a pre-projected polar mesh was rejected because it
would re-introduce per-sweep CPU tessellation and break FR-RP-7. See ADR-0023 for the
rejected alternatives and their failure modes. -->

### Frame pacing (Stage 4, S4-c / ADR-0022)

FR-DR-5's "target 60 fps" protects two things: that *interaction* is smooth, and that a
*new scan does not stutter*. Rendering 60 frames a second of an unchanged image in four
processes for a multi-hour session serves neither the operator nor the machine. So the
render loop is **redraw-on-demand plus a 2 Hz idle tick**: `ControlFlow::WaitUntil(now +
500 ms)`, with a redraw requested on any input event, on resize, on egui's own repaint
request (`viewport_output[..].repaint_delay`, honoured when shorter than the idle tick),
and on an idle tick when `AppState::snapshot().revision` changed or the time-derived
chrome text (data age) changed. `PresentMode::Fifo` (vsync) caps an interaction burst at
the display rate. 60 fps under sustained pan/zoom and no stutter on scan arrival are met
by measurement (plan §14), not by burning cycles when nothing is happening.

### Transparency

Radar data below the minimum displayable threshold (typically 0 dBZ for reflectivity)
is rendered as fully transparent, allowing the map layers below to show through. This
is encoded in the alpha channel of the pre-computed texture.

### Multi-Sweep Display

Each elevation sweep is a separate texture. The active sweep is selected by the user.
Switching sweeps is a GPU state change (swap the active texture) — it does not require
re-fetching or re-computing data.

---

## Vector Overlay Rendering

County, state, country, and highway geometry is loaded from bundled shapefiles at
startup and tessellated into GPU vertex buffers by `lyon`. These buffers are uploaded
to the GPU once and held for the lifetime of the process.

At render time, vector overlays are drawn as line primitives from the pre-uploaded
vertex buffers. Pan and zoom are applied via a uniform transform matrix — the geometry
itself does not change, only the view transform.

This makes vector overlay rendering essentially free at runtime — a matrix multiply
and a draw call per layer.

---

## Placefile Rendering

Placefiles contain a mix of geometry types: polygons (warning outlines), polylines
(storm tracks), icons (LSR markers), and text labels. Each is rendered as follows:

- **Polygons** — tessellated at parse time, rendered as filled or stroked primitives
- **Polylines** — rendered as line primitives
- **Icons** — rendered as textured quads from a bundled icon spritesheet
- **Text labels** — rendered via egui's text rendering, composited in the egui pass

Placefile geometry is re-tessellated when new placefile data arrives (typically every
60–300 seconds). This is infrequent and not performance-sensitive.

---

## Pan, Zoom, and Spatial Stability

Pan and zoom are implemented as a 2D view transform matrix applied as a uniform to all
geospatial render passes. The transform is applied on the GPU — no geometry is moved,
no textures are re-generated, no data is re-fetched when the user pans or zooms.

**Spatial stability** means the display does not jump, reflow, or reset when:
- A new scan arrives and replaces the previous one
- The active product or sweep is changed
- A placefile updates
- The window is resized

In all of these cases, the view transform is preserved. The user's spatial context
is never disrupted by data updates.

---

## Performance Targets

These are design targets, not benchmarks. They should be validated during development.

| Metric | Target | Stage 4 measurement |
|---|---|---|
| Frame rate (steady state / interaction) | 60 fps | Not measured on-device: the build environment is a nested compositor without Vulkan surface support (see plan §16). Offscreen render verified; on-screen frame timing is a gap for a real-display session. |
| Frame rate (new scan upload) | No perceptible drop | `revision`-gated re-upload; `plan_sync` uploads only changed grids. Not timed on-device. |
| Frame rate (idle) | ~2 fps by design (S4-c) | By construction: `ControlFlow::WaitUntil(now + 500 ms)`. |
| Time to first render after launch | < 2 seconds | Not measured on-device. Palette load is < 50 ms (regression-guarded); pipeline compilation is the remaining cost. |
| Memory per instance (steady state) | < 200 MB | Not measured on-device; Stage 3 headless was ~147 MB. |
| GPU memory per instance | < 128 MB | Stage 3 measured 37.28 MB grids + 2.52 MB derived; the surface, LUT uniforms (4 KB each, ≤32), and egui atlas add little. Not measured on-device. |
| Texture uploads across a product/sweep switch | 0 (FR-RP-7) | **0** — `plan_sync` unit tests assert an empty upload list for both. |

---

## What the Render Loop Does Not Do

- Does not fetch data from any network source.
- Does not decode NEXRAD files.
- Does not compute derived products or perform color mapping.
- Does not write to shared application state.
- Does not block on any lock for more than a frame.
