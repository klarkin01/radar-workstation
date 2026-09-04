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

> **Implementation status:** Stage 3 complete. The chunk ingest layer (fetch + BZ2
> decompression), the NEXRAD decoder, the volume assembly state machine (ADR-0012),
> the compute layer (gridding, colour tables, Echo Tops/VIL — ADR-0020, ADR-0021),
> shared application state (ADR-0018), the runtime/supervision skeleton, and
> configuration persistence (ADR-0019) are implemented and tested
> (`crates/radar-workstation`, `crates/nexrad-decoder`) — see
> `crates/radar-workstation/src/{assembly,compute,state,pipeline.rs,config}`. `main.rs`
> runs the full poller → assembly → compute → applier quartet end to end, through
> `AppState`, today. **Stage 4 (2026-08-28):** the render loop described below is
> implemented (`crates/radar-workstation/src/render/`, ADR-0022/ADR-0023). It reads
> `AppState` once per frame through `snapshot()`, holds no lock, and owns view state
> (`render::ViewState`) outright — never writing back. Map and placefile layers are still
> unbuilt (Stages 5–6); the tile layer is deferred out of v1.0 (ADR-0027).

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

**Deferred to post-v1.0 ([ADR-0027](../adr/0027-tile-image-decoding.md)).** No
tile-fetching code exists and none is stubbed; v1.0 ships a vector-only basemap and
compositing layer 2 is unpopulated. The design below stands as written for when the
subsystem returns — it is recorded, not implemented.

- **Source:** Pluggable XYZ tile provider (USGS National Map by default)
- **Access:** Public, no authentication. `TileClient` sends no `Authorization` header
  and no API key — enforced in code, not assumed of the default provider (ADR-0026)
- **Protocol:** HTTPS only, HTTP/1.1, no redirects ever, `ETag`/`If-None-Match` for
  revalidation. Transport is `crates/tile-fetch` over the `http-ingest` engine (ADR-0026)
- **Format:** standard XYZ/TMS scheme. Both JPEG and PNG must be decoded — format is a
  **per-tile** property, not a per-provider one: four of five USGS services serve both
  from one URL template (measured 2026-08-28, ADR-0027 Measurement 1), so dispatch is on
  magic bytes, never `Content-Type`. Q18 is closed: take `png` + `jpeg-decoder`, gate on
  declared dimensions at the header before allocating, and contain panics with
  `catch_unwind` on `spawn_blocking` (ADR-0027 §2)
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
`<volume-sequence>` is an unpadded per-site **cyclic counter over 1–999** — it rolls
`999 → 1` (observed 2026-09-03) and is *not* monotonically increasing, and it also does
not sort lexically in numeric order (`"78"` sorts after `"709"`). The poller tracks
position as a `VolumeSeq` (`crates/radar-workstation/src/ingest/volume_seq.rs`), which
carries no `Ord`; ordering across a possibly-wrapped listing comes from `VolumeWindow`.
See `crates/radar-workstation/src/ingest/s3_poll.rs` (`S3Poller::poll_once`) for the
authoritative implementation.

```
1. On startup, resolve the selected radar site identifier (e.g. KTLX) and list the site
   prefix with delimiter=/ (list_volume_folders) to enumerate volume-sequence directories
   as CommonPrefixes, parsed numerically
2. Anchor cold start to the newest retained volume (cold_start_target, wrap-aware via
   VolumeWindow), so the first poll fetches the current volume rather than replaying the
   full retention window (observed ~48 h, not contractual)
3. Each poll, list the single volume-sequence directory `target` via ListObjectsV2, using
   start-after within that directory only — fixed-width <timestamp>-<n>-<kind> filenames
   make lexical order chronological there
4. Classify each new key by its -S / -I / -E suffix and fetch chunk bodies sequentially
   (ADR-0014: the client holds one keepalive connection, no connection pool)
5. Decompress each chunk (strip the volume header for -S; BZ2-decompress the block(s))
6. Hand the decompressed message stream to the NEXRAD decoder (see below)
7. Feed the decoded radials to the volume assembly state machine (ADR-0012), which
   accumulates them into the in-progress VolumeScan and signals the compute layer as
   each sweep closes
8. On seeing an -E chunk, advance `target` to the next volume-sequence directory
   (successor function, rolls 999 → 1) for the following poll
9. Sleep for the configured polling interval (implementation default: 5 seconds)
10. Repeat from step 3
```

**Skipped-sequence recovery (S1-W3a):** if a volume-sequence directory never
appears — an RDA restart that skips sequence numbers, observed live (79→90, 92→165,
195→268) — the poller no longer stalls waiting on a directory that will never exist. It
tracks empty polls since the last key seen for the current target; past a threshold
(~60s) with no key ever seen, it re-lists the bucket's volume folders and re-anchors
forward *in time* if a genuine gap exists — "forward in time" rather than "forward
numerically" because across the 999 → 1 wrap the live volume is numerically smaller, so
the comparison is `VolumeWindow::is_after` (position in the retained arc), not `>`. A
volume that produced real data but then stalls mid-
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

**Post-v1.0** — deferred with the subsystem (ADR-0027). Recorded, not implemented.

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

Step 4 runs on N independent `TileClient`s (N = 4 by default), each owning one engine
owning one connection — concurrency comes from task count, not from a connection pool,
which preserves ADR-0014's "no pool" decision literally. A tile failure produces a
missing tile and a status-bar line and nothing else: no shared connection state, no
shared error values, and no shared task supervision with the radar path (ADR-0026).

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

## Compute Layer

**Implemented** (Stage 3) — `crates/radar-workstation/src/compute/`. A fourth pipeline
stage, between assembly and the applier: `poller → assembly → compute → applier →
AppState` (`pipeline::run_ingest_pipeline`). As each sweep closes
(`AssemblyEvent::SweepClosed`), the compute task grids every in-scope product present on
it (`compute::grid::grid_all_base_products`) on `tokio::task::spawn_blocking` — the
runtime has two workers (S2-c), and awaiting the blocking job inline (rather than
spawning and moving on) keeps at most one grid job running at a time, which is what
keeps §3.6's "no rayon yet" decision honest. Gridded results are sent onward as
`compute::StateUpdate` and are also what the compute task retains (as cheap `Arc`
clones) for the accumulating volume's reflectivity, needed to derive Echo Tops/VIL when
the volume closes `Complete`.

### Products Derived

v1.0 scope, resolved 2026-08-05 (Q8, ADR-0020's §3.1 sketch; REQUIREMENTS.md
FR-RP-1/2/3):
- **Base reflectivity, base velocity, spectrum width** — all sweeps
- **ZDR (differential reflectivity), CC (correlation coefficient)** — all sweeps
  carrying them; decoded regardless per FR-ND-4, gridded identically to the base three
  once gridding is generic across moments
- **Echo Tops, VIL** — derived from the accumulating volume's retained reflectivity
  grids at volume close (`compute::derived`)

Deferred post-v1.0 (Q8/Q9; REQUIREMENTS.md FR-RP-4/5):
- **Storm-relative velocity** — needs a storm motion vector input mechanism that
  doesn't exist before Stage 4's UI
- **KDP, PHI, CFP** — KDP needs a real differentiation-over-range algorithm; PHI/CFP
  are diagnostic quantities of low value to a general operator
- **Velocity dealiasing** — deferred with fold indicators instead: range folding gets
  its own palette entry, and the Nyquist velocity is carried on every velocity grid for
  a future legend/status-bar readout

### Output

**Amended from the original RGBA sketch (ADR-0020, S3-a).** Each `(sweep, product)`
becomes a `compute::grid::SweepGrid` — a single-channel 8-bit polar grid where the cell
*is* the raw NEXRAD value (`0`=no-data, `1`=range-fold, `2..=255`=`(raw−offset)/scale`)
— plus a `compute::palette::ColorLut`, a 256-entry RGBA table compiled once per product
from a GRLevelX-format `.pal` file (`compute::palette`, ADR-0021; bundled defaults or a
user override from `paths::data_dir()`). RGBA for the full seven-moment set exceeds the
128 MB per-instance GPU budget (~200 MB); R8+LUT fits comfortably (~50 MB) — see
ADR-0020's memory table. The render loop (Stage 4) uploads both and does one 1D LUT
lookup per pixel in the fragment shader — it still performs no per-gate colour mapping
or product computation at render time, which is the property this section's original
RGBA wording was protecting.

### Retention

Once a sweep is gridded, its raw radials are released — `RadarState` holds grids, not
`Sweep`s (ADR-0018's erratum). The last `Arc<Sweep>` anywhere in the process drops when
the grid job's `spawn_blocking` closure returns.

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
- Newest closed sweep per elevation number, **as gridded products**
  (`BTreeMap<u8, DisplaySweep>`, `DisplaySweep.grids: Vec<Arc<SweepGrid>>` — S3-g),
  carried across volume boundaries so a closing volume never blanks the display
  (ADR-0012)
- Volume-derived products (Echo Tops, VIL), replaced wholesale per completed volume
  (`BTreeMap<DisplayProduct, Arc<SweepGrid>>`)
- Metadata for the last successfully completed volume (`VolumeSummary`, not the
  `VolumeScan` itself — S3-g: its sweeps are already gridded and released) (FR-DA-5)
- A `revision: u64` counter, incremented on every applied change, that the render loop
  will use (Stage 4) to skip GPU texture re-upload when unchanged (FR-DR-5)

The tile cache index and placefile data are Stage 5/6 concerns not yet designed in
detail; ADR-0018 notes that adding them is expected to be additive to this structure,
not a redesign of it. User settings and application status in the sense of "what the
render loop currently has selected" are view state (above), not part of `RadarState`.

### Access Pattern
- **Writer:** the data pipeline, through `AppState::apply_event` (`compute::StateUpdate`,
  Stage 3) and `AppState::report` (typed events, to both the stderr sink and the bounded
  in-memory log)
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
