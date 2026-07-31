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

> **Implementation status:** The chunk ingest layer (fetch + BZ2 decompression) and the
> NEXRAD decoder are implemented and tested (`crates/radar-workstation`,
> `crates/nexrad-decoder`). The volume assembly state machine (ADR-0012), compute layer,
> shared application state, and render loop described below are architecture, not yet
> code — `main.rs` is currently a stub.

---

## External Data Sources

### NEXRAD Level II — Primary Radar Data
- **Source:** Real-time chunk stream, `s3://unidata-nexrad-level2-chunks` — the primary
  source per ADR-0011. Assembled volume files, `s3://unidata-nexrad-level2`, are a
  secondary source for historical playback, testing, and chunk-stream fallback. The
  legacy `noaa-nexrad-level2` bucket stopped receiving updates September 1, 2025 and is
  not used.
- **Access:** Public, no authentication required, no API key
- **Latency:** Individual chunks (~100° of azimuthal coverage each) are available within
  seconds of the antenna completing that portion of the scan — the lowest sweep is
  typically displayable 30–60 seconds into a volume, well before the volume completes.
- **Update cadence:** A full volume completes every 4–6 minutes in clear air mode, every
  1–2 minutes in precipitation mode; individual chunks arrive continuously throughout.
- **Protocol:** HTTPS (S3 REST API — `ListObjectsV2` for chunk discovery, plain HTTP GET
  for chunk bodies; no AWS SDK required)
- **Format:** NEXRAD Level II real-time chunk format — `-S` (start), `-I`
  (intermediate), `-E` (end), each BZ2-compressed after a short envelope header. See
  [nexrad-binary-format.md](nexrad-binary-format.md) and ADR-0011/ADR-0012.

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

The chunk bucket keys objects as `SITE/<volume-sequence>/<timestamp>-<n>-<kind>`, where
`<volume-sequence>` is an unpadded, monotonically increasing per-site integer that does
not sort lexically in numeric order (`"78"` sorts after `"709"`). The poller therefore
tracks position as a volume-sequence number rather than a flat key scan; see
`crates/radar-workstation/src/ingest/s3_poll.rs` (`S3Poller::poll_once`) for the
authoritative implementation.

```
1. On startup, resolve the selected radar site identifier (e.g. KTLX) and list the site
   prefix with delimiter=/ (list_volume_folders) to enumerate volume-sequence directories
   as CommonPrefixes, parsed numerically
2. Anchor cold start one behind the newest known volume (cold_start_baseline), so the
   first poll fetches the current volume rather than replaying the full 24-hour
   retention window
3. Each poll, list the single volume-sequence directory last_completed_volume + 1 via
   ListObjectsV2, using start-after within that directory only — fixed-width
   <timestamp>-<n>-<kind> filenames make lexical order chronological there
4. Classify each new key by its -S / -I / -E suffix and fetch chunk bodies sequentially
   (ADR-0014: the client holds one keepalive connection, no connection pool)
5. Decompress each chunk (strip the volume header for -S; BZ2-decompress the block(s))
6. Hand the decompressed message stream to the NEXRAD decoder (see below)
7. Feed the decoded radials to the volume assembly state machine (ADR-0012), which
   accumulates them into the in-progress VolumeScan and signals the compute layer as
   each sweep closes
8. On seeing an -E chunk, advance to the next volume-sequence directory for the
   following poll
9. Sleep for the configured polling interval (implementation default: 5 seconds)
10. Repeat from step 3
```

**Known accepted gap:** if a volume-sequence directory never appears — for example an
RDA restart that skips sequence numbers, observed live — the poller stalls waiting on a
directory that will never exist. Recorded in `s3_poll.rs` and tracked as wanting its own
issue; not fixed here.

On site change, the current polling task is cancelled and a new one is spawned for
the new site. The shared state is cleared and the display resets to the new site's
most recent available data.

### Volume Assembly (ADR-0012)

The chunk stream offers no atomic "volume complete" guarantee, so an explicit state
machine tracks assembly: `IDLE → AWAITING_DATA → ACCUMULATING`. Each sweep closes
independently as its end-of-elevation signal arrives (or is inferred from the next
elevation number), and the volume itself closes on `-E` receipt, on the next `-S`
arriving early (`Superseded`), or on a watchdog timeout (`TimedOut`). `VolumeScan`
carries this completion status explicitly so downstream layers can indicate incomplete
data without withholding it — a visible gap is preferable to silently modifying data
already rendered. See ADR-0012 for the full state machine, per-chunk-type missing-data
handling, and the rationale against a late-data waiting window.

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
with its own test suite. It accepts a decompressed message stream and returns a
structured volume scan representation or a typed error.

### Input
A decompressed NEXRAD message stream, Message 31 format only (ADR-0011 — the chunk
stream carries no other format). Chunk classification and BZ2 decompression happen in
the ingest layer before the decoder sees any bytes; the decoder itself performs no
decompression.

### Output
A `VolumeScan` struct containing:
- Site identifier and metadata (lat/lon, elevation, scan time)
- A collection of `Sweep` structs, one per elevation angle
- Each `Sweep` contains moment data arrays: reflectivity, velocity, spectrum width,
  and dual-pol moments (ZDR, CC, KDP) where present
- All values in calibrated physical units (dBZ, m/s, dB, etc.)

### Error Handling
The decoder returns typed errors for all failure modes: truncated files, corrupt
headers, unsupported format versions, and decompression failures. The application
handles these gracefully — a failed decode is logged and the previous scan remains
displayed. The UI does not crash.

### Testing
The decoder has a dedicated test suite: 24 tests, all passing, against fixtures covering
one site (KDOX), one scan mode (VCP 35), one era, and super-resolution dual-pol data
only. The target coverage below is what FR-ND-8 specifies for v1.0, not what exists
today — see `crates/nexrad-decoder/TESTING.md` for the current-vs-target breakdown and
the fixture-coverage gap this leaves.

Target coverage:
- Known-good Level II files from NCEI archive (multiple sites, scan modes, eras)
- Corrupt and truncated input (must not panic, must return typed error)
- Dual-pol and non-dual-pol variants
- Super-resolution and standard-resolution variants

---

## Compute Layer (rayon)

When a new `VolumeScan` is decoded, it is handed to the compute layer. Product
derivation runs in parallel across rayon's thread pool.

### Products Derived

Confirmed v1.0 scope (REQUIREMENTS.md FR-RP-1/FR-RP-2):
- **Base reflectivity** — all sweeps (color-mapped directly from decoded moment data)
- **Base velocity** — all sweeps
- **Spectrum width** — all sweeps
- **Echo Tops** — derived from multi-sweep reflectivity volume
- **VIL** — vertically integrated liquid, derived from reflectivity volume

Open pending Q8/Q9, conservative default is deferred post-v1.0 (REQUIREMENTS.md
FR-RP-3/4/5):
- **Storm-relative velocity** — requires a storm motion vector input mechanism
- **Dual-pol moments** — ZDR, CC, KDP (decoded regardless per FR-ND-4; the compute/
  display pipeline for them is what's unresolved)
- **Velocity dealiasing**

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
- Derived product textures (indexed by product type and sweep)
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
