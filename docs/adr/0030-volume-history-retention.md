# ADR-0030: Volume History Retention

## Status
Accepted (2026-09-04)

Resolves [Q25](../open-questions.md). Stage 6a Part B
(`docs/plans/stage-6a-part-b-retain-history.md`), executing Part B (B1-B4) of
`docs/plans/stage-6a-time-handling.md`. Supersedes ADR-0018's retention
paragraph (dated erratum added there in this change) and amends
`REQUIREMENTS.md` §4.1's memory target and §6's scope boundary.

## Context

There is no history and nothing to animate. `RadarState` (ADR-0018) holds one
merged `BTreeMap<u8, DisplaySweep>` — the newest closed sweep per elevation
number, carried across volume boundaries so a closing volume never blanks the
display (FR-DA-3, ADR-0012) — plus one derived-product set and one
`last_complete` summary. Nothing survives a sweep's replacement. A timeline
(Part C) needs a *loop*: more than one volume's worth of grids, addressable by
which volume they came from.

**A volume costs ~28-40 MB of grids — measured, not estimated.** Measured
2026-09-04 with a new `utility/radar-viz --path budget` harness (added by this
change, W1.1), which grids every base product per sweep
(`compute::grid::grid_all_base_products`) and derives Echo Tops/VIL from the
volume's reflectivity grids (`compute::derived::compute_derived`) — the same
two calls `compute::handle_event` makes, so the measured number is the number
the application actually allocates. Bytes come from `SweepGrid::byte_len()`,
never a recomputed `azimuth × gates`.

| Site | VCP | Elevations | Base grids | Derived | Frame total |
|---|---|---|---|---|---|
| KDOX | 35 (clear-air) | 16 | 37.29 MB | 2.52 MB | **39.80 MB** |
| KFWS | 12 (precipitation) | 12 | 26.54 MB | 1.64 MB | **28.17 MB** |
| KHGX | 212 (precipitation) | 14 | 28.30 MB | 2.52 MB | **30.82 MB** |

The parent plan's estimate ("plausibly 20-35 MB") was low for the clear-air
pattern and about right for the two precipitation patterns measured. The
concern the parent plan raised — that VCP 12/212 fly more super-resolution
cuts and might measure materially larger than 35 — did **not** materialize in
these two live volumes: VCP 35's 16 elevation cuts (clear-air surveillance
flies more tilts than either precipitation mode measured here) outweigh the
per-tilt gate-count difference. This is a property of the two volumes
actually captured, not a bound on what any VCP can produce, but it means
§1's defaults are sized against the conservative (larger) end of what was
measured, with margin rather than a shortfall.

A 20-frame loop of whole volumes is therefore **~560-800 MB** — the hard part
of this design, not the ring's data structure. Two consequences:

- **CPU memory is the binding constraint, not GPU memory.** The newest frame
  fully resident on the GPU is ~30-40 MB; the ADR-0029 overlay buffers are
  11.46 MB; a 12-frame tail of one selected `(product, elevation)` is
  ~15 MB. That is comfortably under the 128 MB GPU target. The same frames
  cost an order of magnitude more on the CPU.
- **The 200 MB per-instance target (`REQUIREMENTS.md` §4.1) cannot survive a
  useful loop.** §"Decision" 5 amends it rather than exceeding it silently.

The GPU grid cache (`render::radar`) also re-uploads everything on every
`revision` bump today: `upload_grid` took `&SweepGrid` and stored
`Arc::new(grid.clone())` — a deep copy under a fresh `Arc` — so
`plan_sync`'s `Arc::ptr_eq(existing, grid)` identity test could never be true
for anything `sync` itself had uploaded. With history, "re-upload everything
on every revision" becomes "re-upload every retained frame's grid on every
revision," which this ADR must not ship.

## Decision

### 1. A frame is one volume, and it holds only its own sweeps

A volume is the only unit at which "the next frame" is well-defined —
animating a tilt means stepping the same elevation number across volumes —
and Echo Tops/VIL are only defined at a volume boundary. Critically, a frame
does **not** inherit today's merge-across-volumes behavior: it holds only the
sweeps its own volume actually closed. If it inherited the merge, a played-back
frame would show a tilt the radar did not scan at that time, presented as if
it had — the failure `PHILOSOPHY.md` forbids in plain terms. The merged live
view (what Stage 5 always displayed) becomes a **read-time fold** over the
ring instead: newest-first, first occurrence of each elevation number wins,
stopping at the first frame whose VCP differs from the newest frame's. One
source of truth; the merge is a property of the view, not of stored data.

### 2. The ring stores `Arc<Frame>`; the applier writes through `Arc::make_mut`

Making the *frame* the `Arc`'d unit is what makes `AppState::snapshot()`
affordable: cloning the ring is N refcount bumps regardless of what each
frame holds, so `snapshot()` hands back every retained frame in full and the
render loop reaches into whichever it wants. No selector parameter on
`snapshot()`, no second read API, no metadata type kept in step with the
grids it describes — ADR-0018's "`snapshot()` is the only read API"
invariant is unchanged.

Frames accumulate sweeps while their volume is open, so the newest frame is
mutated in place through `Arc::make_mut` — copy-on-write triggers only while a
snapshot is outstanding, and a `Frame` clone is a `BTreeMap` walk plus a few
dozen refcount bumps, once per closed sweep (~every 20 s).

### 3. Retention is whole frames, evicted oldest-first, under two bounds

Whole frames — not a per-product depth, not the operator's selected product
alone, not compressed chunk bytes re-gridded on demand (see Alternatives).
Two bounds, in the house style `RETAINED_ELEVATION_CAP`/`LUT_CACHE_CAP`
already set:

- `history.frames` — what the operator asks for, in the unit they think in.
- `history.budget_mb` — the hard ceiling, because volume size varies with VCP
  and site (§Context) and a frame count alone is an unbounded memory
  commitment driven by incoming data.

Whichever binds first wins. When the *budget* is what bit, that is reported
once per transition (`Event::HistoryBudgetBound`, edge-triggered — not once
per eviction), so the operator learns their loop is shorter than they asked
for rather than counting ticks. **The newest frame is never evicted**,
whatever the bounds say — a budget too small for one volume degrades to "no
history," never to "no display." `RetentionPolicy::DISABLED` (`frames: 1,
budget_bytes: 0`) is the Stage 5 footprint, and is a first-class, tested
configuration, not a special case bolted onto the general path.

### 4. A VCP change is retained, not cleared; a site change clears

**VCP change:** the ring keeps the old pattern's frames — deleting them would
throw away the loop exactly when the radar switched to a precipitation
pattern, i.e. when weather started and the loop matters most. The
live-view fold's VCP-boundary stop reproduces today's
`sweeps.clear()`-on-VCP-change behavior exactly, without deleting anything;
Part C's timeline gets each frame's `vcp_number` and can mark or refuse
crossing that boundary.

**Site change:** `RadarState::reset` clears the ring. A frame's grids are
polar grids around a specific site; there is nothing in the old ring that
could be correctly displayed under a new one.

### 5. The 200 MB target is amended, deliberately and visibly

At ~30-40 MB per frame there is no arrangement in which a useful loop fits
inside the existing 200 MB target. The target is restated, not exceeded:

| Metric | Amended target |
|---|---|
| Memory per instance, history disabled (`history.budget_mb = 0`) | **< 200 MB** (unchanged — the Stage 5 application) |
| Memory per instance, default history budget | **< 200 MB + `history.budget_mb`** |
| GPU memory per instance | **< 128 MB** (unchanged; §Context shows history does not threaten it) |

NFR-P-1 (four simultaneous instances) gets an erratum: resource scaling per
instance is now an operator-set number with a documented way back to the
Stage 5 footprint, and it still scales linearly, as NFR-P-1 requires.

**Chosen defaults:** `history.frames = 12`, `history.budget_mb = 320`. At the
measured worst case (KDOX VCP 35, ~40 MB/frame) this buys ~8 frames — a
~56-minute loop at a ~20 s volume cadence — before the byte budget binds
ahead of the frame count; at the two precipitation-mode volumes measured
(~28-31 MB/frame) it buys the full 12. A four-instance deployment is then
roughly `4 × (200 + 320)` MB ≈ 2.1 GB. `history.budget_mb = 96` restores a
~300 MB instance with a ~2-3 frame loop; `0` restores the Stage 5 footprint
exactly. This is the one number in this ADR the operator should be able to
override without a rebuild, which is why it is configuration (§8) rather than
a compiled-in constant.

**Baseline measured, this development machine (2026-09-04):** a windowed,
Stage-5-shaped build (history disabled by construction — this measurement
predates this ADR's code) against live KDOX data reached **VmHWM ≈ 393 MB**
(402,284 kB) after an 18+ minute run. This is well above both the 200 MB
target and ADR-0018's earlier ~147 MB headless-adjacent figure, and the gap
is GPU-driver/Vulkan memory-mapping overhead this machine's discrete NVIDIA
adapter charges against RSS, not application-owned heap growth — `overlay:
... 11486000 GPU bytes` is the only large allocation this run's own log
shows. It is recorded here rather than smoothed over: the "< 200 MB, history
disabled" row above is a target for the application's own footprint, and this
number says a live windowed run on real hardware can already sit well past
it before a single history frame exists. Confirming whether this is
driver/GPU-vendor-specific (measure again on the ADR-0024 hybrid-GPU box —
see the `dev-machine-hybrid-gpu` note) is follow-up work this ADR does not
resolve.

### 6. `revision` keeps its meaning; the frame set needs no second counter

The selected frame *is* `ViewState`, so `RadarState` cannot speak about it,
and the render loop can already tell a new frame from the same frames by
comparing the newest frame's identity against what it last drew. What does
change, on the render side: the texture-upload gate is no longer `revision`
alone but `(revision, product, elevation_number)`, because the resident set
(§7) depends on the selection.

### 7. GPU residency is an explicit, bounded, pure plan — not an LRU

> Resident = every grid of the newest frame, plus the selected
> `(product, elevation_number)` grid of every other retained frame, oldest
> frames dropping out first until the set fits the GPU budget. The newest
> frame is never dropped.

Keeping the newest frame whole preserves FR-RP-7 ("product and sweep
switches are GPU state changes only") for the live display, unchanged.
Keeping one grid per older frame is exactly what playback reads and nothing
more (~1.2 MB per frame instead of ~30-40 MB). This is a pure function of
`(frames, selection, budget)` — `render::radar::residency` — that
`render::radar::plan_sync` diffs against the cache; nothing else decides GPU
contents, which is what makes "playback does zero uploads in steady state" a
property provable by a pure test rather than something merely hoped to hold.
An LRU was considered and rejected: it would add a second, opaque eviction
policy on top of a rule that already fully determines the cache.

**Uploads are rate-limited per frame** (`MAX_UPLOADS_PER_FRAME = 4`, ≈5 MB):
switching product with a full history tail resident would otherwise upload
the whole tail (~15 MB) in one frame. `plan_sync` returns whether work
remains; the render loop requests an immediate redraw while it does, so the
tail fills over a few frames instead of stalling one.

**The §2.2 defect above is fixed and made unrepresentable**: `upload_grid`
now takes `Arc<SweepGrid>` and stores the very `Arc` it was handed, so
`Arc::ptr_eq` is sound by construction on every subsequent call.

### 8. Retention policy is configuration, reaching `AppState` at construction

`RetentionPolicy { frames, budget_bytes }` (`state::history`) is built from
`Config` in `main.rs` and passed to `AppState::new`, the same shape
`ingest.poll_interval_seconds` already has. Two keys, `history.frames` and
`history.budget_mb`, read-only in the ADR-0019 sense (loaded, never written
back — like `ingest.poll_interval_seconds`, unlike `view.*`). It never flows
backward from the render loop: the selection stays exactly where ADR-0018
put it.

## Alternatives Considered

- **Retain only some products per frame** (e.g. reflectivity/velocity deep,
  others shallow). Halves the cost — DREF+DVEL is roughly half of a measured
  frame — and is *probably* true that few operators loop spectrum width.
  Rejected: the Instrument Principle cuts the other way — the software does
  not decide which moments the operator is allowed to look at over time on
  the strength of a guess about typical usage.
- **Retain the operator-selected product, deeper than the rest.** The
  efficient answer, and unavailable: the selection is `ViewState`, and
  ADR-0018 forbids it entering `AppState` — not incidentally, but because
  FR-NI-4's spatial-stability guarantee is held by the type system, not by
  discipline. A "retention hint" flowing render-loop → state is that
  backward edge with a friendlier name.
- **Retain compressed chunk bytes and re-grid on demand.** ~5x smaller and
  genuinely attractive, but it puts decoding and gridding on the scrub path,
  which the one-way data flow (`overview.md`) forbids, and re-gridding a
  volume is too slow for playback. Recorded for Part D to revisit for the
  archive path, where the tradeoff differs.
- **An LRU over GPU residency, expressed in bytes.** See §7 — a bounded
  residency plan already fully determines the cache's contents; an LRU adds
  a second, opaque policy over the same question.

## Consequences

- **Two new config keys**: `history.frames` (1-64), `history.budget_mb`
  (0-4096, `0` = disabled). Out-of-range values report
  `Event::ConfigValueInvalid` and fall back to the default (reject, not
  clamp — the operator asked for a specific memory commitment).
- **`RadarState` shape change**: `sweeps`/`derived`/`derived_volume`/
  `current_vcp`/`last_complete` are replaced by one `history::FrameRing`.
  Every one of the replaced fields is now derivable from it.
  `AppState::new` and `RadarState::new` both take a `RetentionPolicy`.
- **`StateSnapshot` gains one field**: `frames: Vec<Arc<history::Frame>>`,
  oldest → newest. Every existing field keeps its meaning; no consumer of
  `sweeps`/`derived`/`last_complete` changes.
- **`state::apply` returns `(bool, Vec<Event>)`**, not a bare `bool`: two new
  observability events (`HistoryBudgetBound`, `LateVolumeDiscarded`) can now
  originate inside the pure applier and are reported by `AppState::apply_event`,
  the same split `StateUpdate::Info` already used.
- **`render::radar`'s `GridKey` gains a `VolumeId`**: `(VolumeId,
  DisplayProduct, Option<u8>)`. `VolumeId` gains `Hash`.
- **ADR-0018's retention paragraph is superseded** (dated erratum added
  there, pointing here). `REQUIREMENTS.md` §4.1 and §6 are amended in the
  same change (FR-DA-10, Q25).
- **Part C (the timeline) consumes `StateSnapshot::frames` and needs no
  further state-layer work** — this ADR is the entire architectural
  prerequisite Part C needs.
- **No new dependencies.** A `VecDeque` of `Arc<Frame>` and two pure folds
  (`live_sweeps`, `live_derived`).
