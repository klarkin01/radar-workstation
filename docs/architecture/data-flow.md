# Data Flow

*How data moves from external sources into the application and through to the display.
For the rendering pipeline specifically, see [rendering.md](rendering.md). For the
principles governing these design choices, see [PHILOSOPHY.md](../PHILOSOPHY.md).*

---

## Overview

Data flows in one direction. External sources are fetched asynchronously by the data
pipeline, decoded and computed by the compute layer, written into shared application
state, and read by the render loop. Nothing flows backward. The render loop never
initiates a fetch. The data pipeline never touches the renderer.

```
External Sources
      │
      ▼
 Data Pipeline (tokio)
      │
      ├── NEXRAD Volume Scans
      │         │
      │         ▼
      │   NEXRAD Decoder
      │         │
      │         ▼
      │   Compute Layer (rayon)
      │         │
      │         ▼
      └──────► Shared App State (Arc<RwLock<>>)
                    │
                    ▼
              Render Loop (wgpu)
                    │
                    ▼
               Display
```

---

## External Data Sources

### NEXRAD Level II — Primary Radar Data
- **Source:** AWS S3 public bucket (`s3://noaa-nexrad-level2`)
- **Access:** Public, no authentication required, no API key
- **Latency:** Files typically available within 30–60 seconds of scan completion
- **Update cadence:** Every 4–6 minutes in clear air mode, every 1–2 minutes in
  precipitation mode
- **Protocol:** HTTPS (S3 REST API, no AWS SDK required — plain HTTP GET)
- **Format:** NEXRAD Level II archive format (WSR-88D), bzip2 compressed

### Map Imagery Tiles — Background Terrain/Satellite
- **Source:** Pluggable XYZ tile provider (USGS National Map by default)
- **Access:** Public, no authentication required for USGS
- **Protocol:** HTTPS
- **Format:** PNG tiles, standard XYZ/TMS scheme
- **Caching:** Tiles written to on-disk LRU cache on first fetch

### Placefiles — Warnings, Storm Reports, Overlays
- **Source:** User-configured URLs (NWS, third-party providers)
- **Access:** Public HTTP/HTTPS endpoints
- **Update cadence:** Per-placefile polling interval, typically 60–300 seconds
- **Format:** GRLevelX placefile format

---

## Data Pipeline (tokio)

The data pipeline runs entirely within the tokio async runtime. Each data source is
an independent async task. Tasks do not block each other. The render loop is never
aware of or blocked by pipeline activity.

### NEXRAD Polling Task

```
1. On startup, resolve the selected radar site identifier (e.g. KTLX)
2. Query the S3 bucket listing for the current UTC date prefix
3. Identify the most recent volume scan file not yet downloaded
4. Download the file to a local buffer
5. Pass the buffer to the NEXRAD decoder (see below)
6. On completion, signal the compute layer via channel
7. Sleep for the configured polling interval (default: 30 seconds)
8. Repeat from step 2
```

On site change, the current polling task is cancelled and a new one is spawned for
the new site. The shared state is cleared and the display resets to the new site's
most recent available scan.

### Tile Fetching Task

```
1. Render loop signals required tile coordinates (z/x/y) not present in cache
2. Tile task receives coordinates via channel
3. Check disk cache — if present and fresh, load from disk and deliver
4. If not cached, fetch from configured tile provider URL
5. Write fetched tile to disk cache
6. Deliver tile texture to render state
```

Tile fetching is fire-and-forget from the render loop's perspective. Missing tiles
render as transparent until delivered. The display never waits on a tile fetch.

### Placefile Polling Task

```
1. For each configured placefile URL, spawn an independent polling subtask
2. Fetch the placefile on the configured interval
3. Parse the GRLevelX placefile format into internal representation
4. Write parsed placefile data into shared application state
5. Sleep for the polling interval and repeat
```

---

## NEXRAD Decoder

The decoder is an internal library, cleanly separated from the rest of the application,
with its own test suite. It accepts a raw byte buffer and returns a structured volume
scan representation or a typed error.

### Input
Raw NEXRAD Level II archive file bytes (bzip2 compressed, per-radial or
message-31 format).

### Output
A `VolumeScan` struct containing:
- Site identifier and metadata (lat/lon, elevation, scan time)
- A collection of `Tilt` structs, one per elevation angle
- Each `Tilt` contains moment data arrays: reflectivity, velocity, spectrum width,
  and dual-pol moments (ZDR, CC, KDP) where present
- All values in calibrated physical units (dBZ, m/s, dB, etc.)

### Error Handling
The decoder returns typed errors for all failure modes: truncated files, corrupt
headers, unsupported format versions, and decompression failures. The application
handles these gracefully — a failed decode is logged and the previous scan remains
displayed. The UI does not crash.

### Testing
The decoder has a dedicated test suite exercising:
- Known-good Level II files from NCEI archive (multiple sites, scan modes, eras)
- Corrupt and truncated input (must not panic, must return typed error)
- Dual-pol and non-dual-pol variants
- Super-resolution and standard-resolution variants

---

## Compute Layer (rayon)

When a new `VolumeScan` is decoded, it is handed to the compute layer. Product
derivation runs in parallel across rayon's thread pool.

### Products Derived (v1.0 scope — see open-questions.md Q8)
- **Base reflectivity** — all tilts (color-mapped directly from decoded moment data)
- **Base velocity** — all tilts
- **Storm-relative velocity** — all tilts (requires storm motion vector input)
- **Spectrum width** — all tilts
- **Echo Tops** — derived from multi-tilt reflectivity volume
- **VIL** — vertically integrated liquid, derived from reflectivity volume
- **Dual-pol moments** — ZDR, CC, KDP (where present in scan)

### Output
Derived products are written as pre-computed, color-mapped RGBA textures ready for
upload to the GPU. The render loop uploads these textures and draws them — it does
not perform color mapping or product computation at render time.

---

## Shared Application State

`Arc<RwLock<AppState>>` is the coordination point between the data pipeline, compute
layer, and render loop.

### Contents
- Current `VolumeScan` (most recently decoded)
- Derived product textures (indexed by product type and tilt)
- Active site configuration (identifier, lat/lon, elevation)
- Loaded placefile data
- Tile cache index (in-memory portion)
- User settings (active product, color table, zoom, pan position)
- Application status (polling state, last scan time, error messages)

### Access Pattern
- **Writers:** Data pipeline (new scans, new tiles, new placefiles), compute layer
  (derived products)
- **Readers:** Render loop (every frame)
- Write locks are held briefly. The render loop always acquires a read lock and
  proceeds — it never blocks waiting for a long-running write.

---

## Data Flow on Site Change

Site changes are the most disruptive event in the data flow. The sequence is:

```
1. User selects new site
2. Write to AppState: clear current scan, derived products, and site-specific data
3. Cancel existing NEXRAD polling task
4. Spawn new polling task for the new site
5. Render loop detects cleared state, displays loading indicator
6. First scan for new site arrives, decodes, computes, renders
```

This sequence is fast. The user should see the new site's most recent scan within
a few seconds of selection on a normal network connection.

---

## What the Data Pipeline Does Not Do

- Does not render anything.
- Does not modify user settings.
- Does not communicate with other running instances.
- Does not make any network connection not explicitly initiated by polling logic
  or user-configured placefile URLs.
- Does not write to any location outside the application's designated cache directory.
