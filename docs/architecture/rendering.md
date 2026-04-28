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
| 6 | Radar data | Derived product textures | The primary content. Alpha-composited over map layers. |
| 7 | Placefile overlays | Parsed placefile data | Drawn per-placefile in user-configured order. |
| 8 | Radar site markers | Bundled site list | Small icon + ICAO label. Clickable for site selection. |
| 9 | City labels | Bundled label data | Appear above a zoom threshold only. |

egui UI chrome is composited on top of all wgpu layers by the egui render pass.

---

## Radar Data Rendering

Radar data is the most performance-critical rendering path. The approach:

### Texture-Based Rendering

Derived products are pre-computed as RGBA textures by the compute layer (rayon) and
uploaded to the GPU once per new scan. The render loop draws the radar texture as a
full-screen quad (clipped to the 230km radar range ring), alpha-composited over the
map layers below.

This means **no per-frame color mapping.** Color mapping happens once in the compute
layer when a new scan arrives, not every frame. The GPU simply draws a pre-colored
texture. This is the primary reason the render loop is fast.

### Polar Grid Representation

The radar texture is generated on a polar coordinate grid matching the native NEXRAD
resolution: 1km range gates × 1° azimuth bins × 230km range. This grid is mapped to
the azimuthal equidistant projection coordinate space by the vertex shader.

### Transparency

Radar data below the minimum displayable threshold (typically 0 dBZ for reflectivity)
is rendered as fully transparent, allowing the map layers below to show through. This
is encoded in the alpha channel of the pre-computed texture.

### Multi-Tilt Display

Each elevation tilt is a separate texture. The active tilt is selected by the user.
Switching tilts is a GPU state change (swap the active texture) — it does not require
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
- The active product or tilt is changed
- A placefile updates
- The window is resized

In all of these cases, the view transform is preserved. The user's spatial context
is never disrupted by data updates.

---

## Performance Targets

These are design targets, not benchmarks. They should be validated during development.

| Metric | Target |
|---|---|
| Frame rate (steady state) | 60 fps |
| Frame rate (new scan upload) | No perceptible drop |
| Time to first render after launch | < 2 seconds |
| Time to display after site change | < 5 seconds on normal connection |
| Memory per instance (steady state) | < 200MB |
| GPU memory per instance | < 128MB |

---

## What the Render Loop Does Not Do

- Does not fetch data from any network source.
- Does not decode NEXRAD files.
- Does not compute derived products or perform color mapping.
- Does not write to shared application state.
- Does not block on any lock for more than a frame.
