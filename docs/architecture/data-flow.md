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
      └──────► Shared App State (Arc<AppState>)
                    │
                    ▼
              Render Loop (wgpu)
                    │
                    ▼
               Display
```

> **Implementation status:** Stage 2 complete. The chunk ingest layer (fetch + BZ2
> decompression), the NEXRAD decoder, the volume assembly state machine (ADR-0012),
> shared application state (ADR-0018), the runtime/supervision skeleton, and
> configuration persistence (ADR-0019) are implemented and tested
> (`crates/radar-workstation`, `crates/nexrad-decoder`) — see
> `crates/radar-workstation/src/{assembly,state,pipeline.rs,config}`. `main.rs` consumes
> the assembler's `SweepClosed`/`VolumeClosed` events end to end, through `AppState`,
> today. The compute layer and render loop described below are still architecture, not
> yet code.

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

**Skipped-sequence recovery (S1-W3a):** if a volume-sequence directory never
appears — an RDA restart that skips sequence numbers, observed live (79→90, 92→165,
195→268) — the poller no longer stalls waiting on a directory that will never exist. It
tracks empty polls since the last key seen for the current target; past a threshold
(~60s) with no key ever seen, it re-lists the bucket's volume folders and re-anchors
forward if a genuine gap exists. A volume that produced real data but then stalls mid-
volume (past a longer, ~5 minute threshold) is abandoned and the poller advances to the
next one, leaving the assembly layer's own watchdog to mark that volume `TimedOut`. See
`S3Poller::apply_recovery` / `next_target` in `s3_poll.rs`.

**Observable poller health (S1-W3b):** `S3Poller::status()` returns a `watch::Receiver`
publishing `IngestState` (`Polling` / `Retrying{attempts}` / `Stalled` /
`ReAnchoring`), the last success time, and a typed `IngestErrorKind` — the seam a status
bar attaches to for FR-DA-5 once shared application state exists to read it from.

On site change, the current polling task is cancelled and a new one is spawned for
the new site. The shared state is cleared and the display resets to the new site's
most recent available data.

### Volume Assembly (ADR-0012)

**Implemented and tested** — `crates/radar-workstation/src/assembly/`. The chunk stream
offers no atomic "volume complete" guarantee, so an explicit state machine tracks
assembly: `Idle → AwaitingData → Accumulating`. Each sweep closes independently as its
end-of-elevation signal arrives (or is inferred from the next elevation number), and the
volume itself closes on `-E` receipt, on the next `-S` arriving early (`Superseded`), or
on a watchdog timeout (`TimedOut`). `VolumeScan` carries this completion status
explicitly so downstream layers can indicate incomplete data without withholding it — a
visible gap is preferable to silently modifying data already rendered. `VolumeAssembler`
is a pure, synchronous core (no async, no I/O, no clock access — `now` is a parameter),
with a thin `assembly::run` wrapper driving it from a chunk channel and a watchdog timer.
Message 5 (VCP definition) is decoded and wired into the assembler's `VolumeContext`,
falling back to the RVOL block on whichever radial carries one when a `-S` chunk was
missed or unreadable. See ADR-0012 for the full state machine, per-chunk-type
missing-data handling, and the rationale against a late-data waiting window; see
`docs/plans/stage-0-1-close-the-acquisition-path.md` for the measured 1.5s wall-clock
time from poller start to first `SweepClosed` (against FR-DA-3's 30-60s target).

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
The decoder has a dedicated test suite spanning two sites (KDOX, KTLH), three scan
modes (VCP 35 clear-air, VCP 212 precipitation with SAILS/MRLE, VCP 121 legacy),
two eras (current and a 2010 pre-dual-pol archive file), both resolution variants, and
a committed hostile-input corpus (structural truncation, hostile pointers/counts/framing)
run through a seeded-mutator fuzz test on plain `cargo test` — mirroring the pattern
`http-ingest` established. See `crates/nexrad-decoder/TESTING.md` for the current
coverage table and the gaps that remain (only two sites; no VCP 12 fixture; no
mid-volume dropped-moment fixture).

Remaining target coverage (FR-ND-8):
- More sites, to sample regional hardware/firmware variation
- Additional VCPs beyond the three currently covered

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

**Implemented** (Stage 2, S2-W1) — `crates/radar-workstation/src/state/`. Resolved as
Q4; full rationale in [ADR-0018](../adr/0018-shared-application-state.md). Not the
single outer `Arc<RwLock<AppState>>` this document previously described: `Arc<AppState>`
holds an interior `RwLock<RadarState>` scoped to radar data only, plus the
`watch::Receiver<IngestStatus>` `S3Poller::status()` already publishes and a bounded
event log behind its own `Mutex`. View state — pan, zoom, active product/sweep, window
geometry — is owned outright by the render loop and never enters `AppState` at all;
nothing in the data pipeline can reach it even by mistake. `AppState::snapshot()` is the
only read API: it takes the read lock, clones `Arc`s and `Copy` fields, and drops the
lock before returning, so holding a lock guard across a frame is impossible by
construction rather than a rule to remember.

### Contents (of `RadarState`, the one locked structure)
- The active site (`&'static Site`, from the bundled table — S2-W3)
- Newest closed sweep per elevation number (`BTreeMap<u8, DisplaySweep>`), carried
  across volume boundaries so a closing volume never blanks the display (ADR-0012)
- The last successfully completed `VolumeScan` (FR-DA-5)
- A `revision: u64` counter, incremented on every applied change, that the render loop
  will use (Stage 4) to skip GPU texture re-upload when unchanged (FR-DR-5)

Derived product textures, the tile cache index, and placefile data are Stage 3/5/6
concerns not yet designed in detail; ADR-0018 notes that adding them is expected to be
additive to this structure, not a redesign of it. User settings and application status
in the sense of "what the render loop currently has selected" are view state (above),
not part of `RadarState`.

### Access Pattern
- **Writer:** the data pipeline, through `AppState::apply_event` (assembly events) and
  `AppState::report` (typed events, to both the stderr sink and the bounded in-memory
  log)
- **Reader:** the render loop (Stage 4), once per frame, through `AppState::snapshot()`
- Write locks are held only long enough to apply one event. The render loop never
  blocks waiting for a long-running write, and a poisoned lock (a panic while holding
  it) is recovered rather than propagated, so a task restart (S2-W2's supervision)
  cannot leave every subsequent `snapshot()` panicking too.

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
