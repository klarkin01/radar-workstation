# Implementation Plan — Stage 6a Part B: Retain history

**Status:** implemented in the working tree (2026-09-04), pending developer review and
commit — see §10 for the measurements and the §3.5 default the developer should confirm.
**Date:** 2026-09-04
**Parent:** `docs/plans/stage-6a-time-handling.md` §2 (what does not exist), §3
Part B (B1–B4), and §5 (the open questions B1 must answer). This document is
the executable form of Part B and nothing else.
**Precondition:** Part A is landed (`c83c049`, branch `time_handling`). The
poller is wrap-aware, `radar_workstation::time` and `radar_workstation::vcp`
exist library-side, and the status bar reports the displayed scan's own age.
Nothing below re-opens any of that.
**Scope:** give the application a history of completed volumes — the
architectural prerequisite for the timeline. A retention model recorded in an
ADR, a measured cap, a frame ring inside `RadarState`, and a GPU-side residency
rule that keeps playback upload-free.
**Out of scope:** Part C (timeline state, keys, playback, scrubber UI) and Part
D (chunk backfill, archive reads, jump-to-time). **Nothing in this plan is
user-visible except the `--headless` diagnostic line and two config keys.** No
`Timeline`, no `ViewState` field, no key binding, no egui widget.

> **Do not commit.** This plan is executed by an implementation session; the
> developer handles every `git add`, `git commit`, `git push`, branch and PR.
> Leave the work in the working tree. This is repeated at §9.

---

## 1. What "done" means

1. `RadarState` retains the **N most recent volumes** as whole frames, bounded
   by both a frame count and a byte budget, and the binding constraint is
   reported as a typed `Event` when the budget is what bites.
2. A frame holds only the sweeps its own volume scanned. The carry-forward
   behaviour FR-DA-3 depends on (a closing volume must never blank the display)
   is preserved **as a read-time fold**, not by copying tilts between frames —
   so a retained frame is an honest record of one scan, which is the whole
   point of retaining it.
3. `AppState::snapshot()` costs O(retained frames) refcount bumps, not
   O(retained grids). Every existing field of `StateSnapshot` keeps its current
   meaning; `frames` is additive.
4. Every existing `state::apply` test passes **unchanged in intent**, including
   `vcp_change_drops_elevations_from_the_old_pattern`,
   `stale_sweep_does_not_overwrite_newer` and
   `same_vcp_incomplete_volume_does_not_drop_other_elevations`.
5. The GPU grid cache is keyed by frame identity, bounded in bytes against the
   128 MB target, and **stores the `Arc` it was handed** — the identity defect
   in §2.2 is fixed and made unrepresentable.
6. A synthetic walk of the selection across every retained frame — what
   playback will do in Part C — performs **zero uploads** once the tail is
   resident, proven by a pure test on the residency planner.
7. The retention model, its measured numbers, and the memory-target amendment
   they force are recorded in **ADR-0030**, with `REQUIREMENTS.md` §4.1 and §6
   corrected in the same change.
8. `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D
   warnings` are clean. **No new dependencies** (CLAUDE.md).

---

## 2. What is already known

### 2.1 A volume costs ~40 MB of grids — measured, not estimated

Measured 2026-09-04 directly from the committed-adjacent sample volume
`downloads/KDOX_20260629_1811` (KDOX, VCP 35, 79 chunks, one complete volume),
by decoding every Message 31 radial and computing each product's grid footprint
as `azimuth_count × gate_count`, exactly as `compute::grid` allocates it:

| Elevations | az | Products present | Bytes |
|---|---|---|---|
| 1, 3, 5, 7 (surveillance halves) | 720 | DREF 1832g, DRHO 1192g, DZDR 1192g | 2.90 MB each |
| 2, 4, 6, 8 (Doppler halves) | 720 | DREF 1192g, DSW 1192g, DVEL 1192g | 2.46 MB each |
| 9 | 720 | DREF 1712g, DRHO, DZDR | 2.82 MB |
| 10 | 720 | DREF 1192g, DSW, DVEL | 2.46 MB |
| 11–16 | 360 | all five | 0.41–2.17 MB each |

```
per product:  DREF 12.55 MB   DVEL 6.18 MB   DSW 6.18 MB   DZDR 6.18 MB   DRHO 6.18 MB
base total:   37.28 MB across 16 elevations
derived:      Echo Tops + VIL at the lowest tilt's geometry (720 × 1832) = 2.52 MB
FRAME TOTAL:  ~39.8 MB
```

The parent plan's estimate ("plausibly 20–35 MB") was low. **A 20-frame loop of
whole volumes is ~800 MB.** That number, not the ring's data structure, is the
hard part of Part B, and it is why W1 re-measures across VCP 12 and 212 before
W2 writes the ADR: 12 and 212 fly more super-resolution cuts than 35 does.

Two consequences that shape every decision below:

- **CPU memory is the binding constraint, not GPU memory.** The newest frame
  fully resident on the GPU is ~40 MB; the ADR-0029 overlay buffers are
  11.46 MB; a 12-frame tail of one selected `(product, elevation)` is ~15 MB.
  That is ~67 MB against a 128 MB GPU target — comfortable. The same 12 frames
  cost ~480 MB of CPU.
- **The 200 MB per-instance target cannot survive a useful loop.** This is a
  requirements amendment, not a number to quietly exceed. §3.5 decides it and
  W2 writes it down.

### 2.2 The GPU grid cache re-uploads everything, every revision

`render::radar::upload_grid` ends with:

```rust
CachedGrid { source: Arc::new(grid.clone()), texture_view: ... }
```

`grid` arrives as `&SweepGrid` (deref-coerced from the snapshot's
`Arc<SweepGrid>`), so this **deep-copies every cell** into a fresh allocation
under a fresh `Arc`. `plan_sync` decides "already uploaded" with
`Arc::ptr_eq(existing, grid)`, which can therefore never be true for anything
`sync` itself uploaded. Two live consequences:

- Every `revision` bump — i.e. every closed sweep, roughly every 20 s —
  re-uploads **all ~70 grids**, ~37 MB, in one frame. FR-DR-5 ("new scan
  arrivals must not cause a perceptible frame rate drop") is currently held only
  by the transfer being fast enough to hide.
- The cache holds a **second full CPU copy** of every grid — ~37 MB of pure
  duplication, which is very nearly one frame of the history this plan is
  trying to afford.

`plan_sync`'s own unit tests pass because they construct the cache map by hand
from the same `Arc`s; nothing exercises the path `sync` actually takes. Fixing
this is W7's first item and it is not optional — with history, "re-upload
everything on every revision" becomes "re-upload every retained frame on every
revision."

### 2.3 What the parent plan's §5 questions need answered

B1 must answer, and ADR-0030 must record: what a frame is; what is retained and
how much; what a VCP change does to the loop; and whether history survives a
site change. §3 answers all four. Whether *archive* playback (Part D) is v1.0
stays open and is explicitly **not** answered here.

---

## 3. Decisions taken in this plan

### 3.1 (B-a) A frame is one volume, and it holds only its own sweeps

The parent plan proposed this and it is right, for the reason it gave — Echo
Tops and VIL are only defined at a volume boundary — plus a second one it did
not: animating a tilt means stepping *the same elevation number* across
volumes, so the volume is the only unit at which "the next frame" is
well-defined.

The consequence the parent plan did not name is the important one. Today
`RadarState.sweeps` is a *merged* map: a tilt that closed in volume *k* stays
displayed through volume *k+1* until it is re-scanned, which is exactly what
FR-DA-3 and ADR-0012's partial-scan rendering require. If frames inherited that
merge, a played-back frame would show a tilt the radar did not scan at that
time, presented as if it had. That is the failure `PHILOSOPHY.md` forbids in
plain terms.

So: **a frame holds only the sweeps its own volume closed, and the merged live
view becomes a read-time fold over the ring** (newest frame first, first
occurrence of each elevation number wins). One source of truth, no memo to
drift, and the merge is a property of the *view* rather than of the stored data.

### 3.2 (B-b) The ring stores `Arc<Frame>` and the applier writes through `Arc::make_mut`

The parent plan's B3 worried that a snapshot must not "clone a whole history",
and proposed splitting `StateSnapshot` into cheap per-frame metadata plus the
grids of one selected frame. That split needs `snapshot()` to take a selector,
which needs the selector to come from `ViewState`, which is the boundary
ADR-0018 exists to hold.

Making the *frame* the `Arc`'d unit dissolves the problem: cloning the ring is
N refcount bumps regardless of what each frame holds, so `snapshot()` can hand
back every frame in full and the render loop reaches into whichever it wants.
No selector parameter, no second read API, no metadata type that has to be kept
in step with the grids it describes.

Frames accumulate sweeps while their volume is open, so the newest frame is
mutated in place through `Arc::make_mut` — the copy-on-write path is taken only
while a snapshot is outstanding, and a `Frame` clone is a `BTreeMap` walk plus a
few dozen refcount bumps, once per closed sweep (~every 20 s). This is the
idiomatic Rust answer to "shared immutable history, one mutable head", and it
keeps ADR-0018's "`snapshot()` is the only read API" invariant exactly as
written.

### 3.3 (B-c) Retention is whole frames, evicted oldest-first, under two bounds

Rejected alternatives, with the reason each fails:

- **Retain only some products per frame** (e.g. reflectivity and velocity deep,
  ZDR/CC/SW shallow). Halves the cost — the measurement in §2.1 says DREF+DVEL
  is 18.7 of 37.3 MB — and it is *probably* true that nobody loops spectrum
  width. "Probably true about what operators do" is not a basis for silently
  making a product un-loopable. The Instrument Principle cuts the other way: the
  operator looks through the software, and the software does not decide which
  moments they are allowed to look at over time.
- **Retain the product the operator has selected, deeper.** This is the
  efficient answer and it is unavailable: the selection is `ViewState`, and
  ADR-0018 forbids it entering `AppState` — not incidentally, but because
  FR-NI-4's spatial-stability guarantee is held by the type system rather than
  by discipline. A "retention hint" flowing render-loop → state is that
  backward edge with a friendlier name.
- **Retain compressed chunk bytes and re-grid on demand.** 5.4× smaller
  (7.4 MB vs 39.8 MB for the measured volume) and genuinely attractive, but it
  puts decoding and gridding on the scrub path, which the one-way data flow
  forbids, and re-gridding a volume is far too slow for playback. Recorded in
  ADR-0030's Alternatives because Part D will have to revisit it for the archive
  path.

So: **whole frames, oldest evicted first.** Two bounds, each with a distinct
job, in the house style that already governs `RETAINED_ELEVATION_CAP`,
`LUT_CACHE_CAP`, the 64-entry event log and every channel in the tree:

- `history.frames` — what the operator asks for, in the unit they think in.
- `history.budget_mb` — the hard ceiling, because volume size varies with VCP
  and site and a frame count alone is an unbounded memory commitment driven by
  incoming data.

Whichever binds first wins. When the *budget* is what bit, that is reported once
per transition (edge-triggered, not per eviction) so the operator learns their
loop is shorter than they asked for, rather than counting ticks and wondering.

**The newest frame is never evicted**, whatever the bounds say — a budget too
small for one volume must degrade to "no history", never to "no display".

### 3.4 (B-d) A VCP change is retained, not cleared; a site change clears

**VCP change:** the ring keeps the old pattern's frames. Deleting them would
throw away the loop at the exact moment the radar switched to a precipitation
pattern — i.e. when weather started, which is when the loop matters most. The
misleading-mixture problem the parent plan raised is real but belongs to the
*view*, and §3.1's fold already solves it: the live merged view folds
newest-first and **stops at the first frame whose `vcp_number` differs from the
newest frame's**, which reproduces today's `sweeps.clear()`-on-VCP-change
behaviour exactly, without deleting anything. Part C's timeline gets each
frame's `vcp_number` and can mark or refuse the boundary; Part B does not
pre-empt that.

**Site change:** `RadarState::reset` clears the ring. A frame's grids are polar
grids around a specific site; there is nothing to keep and nothing that could be
correctly displayed. This also answers the parent plan's fourth §5 question.

### 3.5 (B-e) The 200 MB target is amended, deliberately and visibly

At ~40 MB per frame there is no arrangement in which a useful loop fits inside
`REQUIREMENTS.md` §4.1's 200 MB per-instance target. The target is therefore
restated rather than exceeded:

| Metric | Amended target |
|---|---|
| Memory per instance, history disabled (`history.budget_mb = 0`) | **< 200 MB** (unchanged — this is the Stage 5 application) |
| Memory per instance, default history budget | **< 200 MB + `history.budget_mb`** |
| GPU memory per instance | **< 128 MB** (unchanged; §2.1 shows history does not threaten it) |

NFR-P-1 (four simultaneous instances) gets an erratum: resource scaling per
instance is now an operator-set number with a documented way back to the Stage 5
footprint, and it scales linearly in exactly the way NFR-P-1 requires.

> **The one number in this plan the developer should confirm in review.**
> The proposed defaults are `history.frames = 12` and `history.budget_mb = 320`
> — at the measured VCP-35 frame size, ~8 frames, a ~56 min loop, and a
> ~470 MB instance. Four instances is then ~1.9 GB. If that is the wrong
> trade, the fix is one constant, not a redesign: `history.budget_mb = 96`
> restores a ~300 MB instance with a ~2-frame loop, and `0` restores Stage 5
> exactly. W2 records whichever number is chosen; the implementation session
> should proceed with the proposed defaults and flag the choice in its report.

### 3.6 (B-f) `revision` keeps its meaning; the frame set needs no second counter

The parent plan's B3 asked whether `revision` must distinguish "a new frame
arrived" from "the selected frame changed". It must not, and no second counter
is needed: the selected frame *is* `ViewState`, so state cannot speak about it,
and the render loop can already tell a new frame from the same frames by
comparing `snapshot.frames.last()`'s `VolumeId` against what it drew last — data
it holds anyway. Adding a `frames_revision` field would be a second encoding of
something the frame list already says.

What *does* change is render-side and belongs there: the texture-upload gate
stops being `revision` alone and becomes `(revision, product, elevation_number)`,
because the residency set (§3.7) depends on the selection.

### 3.7 (B-g) GPU residency is an explicit, bounded, pure plan — not an LRU

The parent plan's B4 asked for "an explicit LRU bound expressed in bytes". A
bounded **residency set** is better here, and the difference matters:

> Resident = every grid of the newest frame, plus the selected
> `(product, elevation_number)` grid of every other retained frame, oldest
> frames dropping out first until the set fits the GPU budget. The newest frame
> is never dropped.

Keeping the newest frame whole is what preserves FR-RP-7 ("product and sweep
switches are GPU state changes only") for the live display, unchanged. Keeping
one grid per older frame is exactly what playback reads and nothing more:
~1.26 MB per frame instead of ~40 MB.

An LRU on top of this would add a second, opaque eviction policy over a rule
that already fully determines the cache's contents, and it would turn "playback
does zero uploads in steady state" from a property you can prove with a pure
function into one you hope holds. The residency plan is a pure function of
`(frames, selection, budget)`; `plan_sync` diffs it against the cache; nothing
else decides what lives on the GPU.

**Uploads are rate-limited per frame.** Switching product with a 12-frame tail
resident would otherwise upload ~15 MB in one frame. `plan_sync` takes a
`max_uploads` cap (proposed 4, ≈5 MB, well under a 16.6 ms budget) and returns
whether work remains; the render loop requests an immediate redraw while it
does, so the tail fills over ~3 frames instead of stalling one.

### 3.8 (B-h) Retention policy is configuration, and reaches `AppState` at construction

`RetentionPolicy { frames, budget_bytes }` is built from `Config` in `main.rs`
and passed to `AppState::new`, alongside the site — the same shape
`ingest.poll_interval_seconds` already has. It is operator configuration, not
view state, and it never flows backwards from the render loop. The two keys are
read-only in the ADR-0019 sense: loaded, never written back, exactly like
`ingest.poll_interval_seconds` and unlike `view.highways`.

### 3.9 (B-i) A new ADR, not an erratum

Part A corrected a *format finding* and took errata. Part B changes ADR-0018's
retention paragraph, amends a non-functional requirement, and adds a v1.0 scope
item. That is a decision, and it gets **ADR-0030**, with ADR-0018 gaining a
pointer to it in the style of its own 2026-08-05 erratum.

### 3.10 (B-j) The measurement harness lives in `radar-viz`, not in a new crate

`utility/radar-viz` already has `load_volume`, `group_by_elevation`,
`build_sweep`, and calls into `compute::grid` and `compute::derived` — the whole
path a footprint measurement needs. A new `--path budget` variant of its
existing `RenderPath` enum reuses all of it and adds no crate, no workspace
member, and no dependency. Writing a second chunk-directory loader to measure
the first one would be the exact drift `utility/README.md` warns about.

---

## 4. Work items

Sequential. Each is expected to leave `cargo test --workspace` green before the
next begins. Mapping to the parent plan: **W1 = B2**, **W2 = B1**,
**W3–W6 = B3**, **W7–W8 = B4**, **W9 = documentation**.

### W1 — Measure the frame footprint across VCPs (parent B2)

**W1.1 — `--path budget` in `utility/radar-viz`.**

In `utility/radar-viz/src/main.rs`: add `Budget` to `RenderPath`, accept
`budget` in `parse_path`, and branch to a new `run_budget_path(args,
all_radials)` beside `run_radial_path`/`run_grid_path`. It reuses
`group_by_elevation` + `build_sweep` verbatim, then per sweep calls
`grid::grid_all_base_products`, and finally `derived::compute_derived` over the
reflectivity grids — the same two calls `compute::handle_event` makes, so the
number measured is the number the application allocates.

Output, to stdout:

```
el  angle   az    product   gates   bytes
 1   0.48  720    ref        1832   1318 KiB
 ...
per-product totals, in DisplayProduct order
derived: echo_tops <az>x<gates> <n> KiB, vil ...
FRAME <site> vcp=<n> elevations=<n> base=<n> KiB derived=<n> KiB total=<n> KiB
```

The last line is deliberately one machine-readable line — it is what gets pasted
into the ADR table.

Take the bytes from `SweepGrid::byte_len()`, never from a recomputed
`az × gates`: the whole point is to measure what the type allocates.

**W1.2 — Get one complete volume per VCP.**

`downloads/KDOX_20260629_1811` (VCP 35) is already present. Fetch a VCP 12 and a
VCP 212 volume with `utility/nexrad-sample`'s `fetch_sample`, into
`downloads/<SITE>_<YYYYMMDD>_<HHMM>/`. Pick sites in active precipitation — VCP
12 and 212 are precipitation patterns and will not be flying in clear air. The
committed decoder fixtures (`ktlh_vcp212_*`, `ktlh_vcp121_*`) are single chunks,
**not** whole volumes, and cannot be used for this.

`downloads/` is not committed; leave it that way.

**W1.3 — Baseline the running application's RSS.**

`cargo run --release -- KDOX`, leave it for at least two complete volume cycles,
then `grep VmHWM /proc/$(pgrep -f 'radar-workstation KDOX')/status`. This is the
"Stage 5 application, no history" number the amended target in §3.5 is stated
against. Record it. Stage 3 measured ~147 MB with no renderer; the renderer,
egui, and the 11.46 MB of overlay buffers are on top of that.

**W1.4 — Record.**

Write the three frame totals and the RSS baseline into this plan document under
a new `## 10. Measured results` section, and into ADR-0030's table in W2.
Numbers first, ADR second — the order that settled Q18/Q19/Q20.

### W2 — ADR-0030 and the requirements amendment (parent B1)

**W2.1 — `docs/adr/0030-volume-history-retention.md`.** Accepted. Follow
ADR-0029's shape: state the measurement, then the decision, then what it costs.

Context: parent plan §2 (there is no history and nothing to animate), §2.1's
measured 39.8 MB frame, ADR-0018's `ViewState`-never-in-`AppState` boundary, and
the 200 MB target.

Decision, one paragraph each: §3.1 (frame = volume, own sweeps only, merged view
is a fold), §3.2 (`Arc<Frame>` + `Arc::make_mut`), §3.3 (whole frames,
oldest-first, two bounds, newest never evicted), §3.4 (VCP retained / site
cleared), §3.5 (the amended memory target and the chosen defaults), §3.6
(`revision` unchanged), §3.7 (bounded residency, not LRU), §3.8 (policy is
configuration).

Alternatives Considered: the three rejected options in §3.3, each with the
number or the boundary that killed it.

Consequences: the config keys; the `RadarState` shape change; ADR-0018's
retention paragraph superseded; `REQUIREMENTS.md` §4.1 and §6 amended; Part C
consumes `StateSnapshot::frames` and needs no further state work.

**W2.2 — ADR-0018.** Add a dated erratum (2026-09-04, Stage 6a Part B) in the
style of its existing 2026-08-05 one: the "one merged sweep per elevation, plus
the last complete volume" retention paragraph is superseded by ADR-0030; the
merged map is now a read-time fold and `last_complete` is derived from the ring.
Do not rewrite the decision body.

**W2.3 — `REQUIREMENTS.md`.**

- §2.1: add **FR-DA-9** — the application must retain the most recent completed
  volumes in memory, bounded by an operator-configurable frame count and byte
  budget, and must report when the byte budget rather than the frame count is
  the binding constraint. Note that the timeline that consumes this is FR-TL-*
  (Part C, not yet written).
- §4.1: replace the single memory row per §3.5's table, and add the NFR-P-1
  erratum sentence.
- §6 In Scope: add "Volume history retention over the live chunk stream
  (resolved 2026-09-04, Q25 — see FR-DA-9 and ADR-0030)".
- §6 Explicitly Deferred: the existing line reads "Animation / loop playback of
  archived scans". Leave it deferred but make it precise — it is *arbitrary-time
  archive playback* (parent plan Part D) that is deferred, and it must not read
  as though looping the retained live volumes is deferred too, because Part C
  will deliver exactly that.

**W2.4 — `docs/open-questions.md`.** Add **Q25** to the Resolved section
(question text: what is retained for the time axis, and how much), resolved
2026-09-04 by ADR-0030, answering all four of the parent plan's §5 questions
except the archive-scope one, which stays open for Part D.

> **Numbering:** `CLAUDE.md` already reserves Q21–Q24 for Stage 6 even though
> `open-questions.md` has not yet been updated with them. Do not renumber and
> do not reuse 21–24.

### W3 — `state::history`: the frame and the ring (pure, no call sites yet)

New file `crates/radar-workstation/src/state/history.rs`; `mod history;` plus
the re-exports in `state/mod.rs`. Nothing in this file does I/O, reads a clock,
or knows what a `ViewState` is; `now` is injected exactly as `state::apply`
already injects it.

Module doc carries the §2.1 measurement, the reason a frame holds only its own
sweeps (§3.1), and a pointer to ADR-0030.

```rust
/// One volume's own gridded output — the unit of history (ADR-0030).
///
/// A frame holds **only** the sweeps its volume actually closed; it never
/// borrows a tilt from its predecessor. Carry-forward for the live display
/// is a read-time fold ([`live_sweeps`]), not a mutation, so a frame that
/// is played back shows what the radar saw at that time and nothing else.
#[derive(Debug, Clone)]
pub struct Frame {
    pub volume: VolumeId,
    pub vcp_number: u16,
    /// When this frame's first sweep was applied.
    pub first_applied: Instant,
    /// This volume's own closed sweeps, by elevation number.
    pub sweeps: BTreeMap<u8, DisplaySweep>,
    /// Echo Tops / VIL, once this volume closed `Complete`.
    pub derived: BTreeMap<DisplayProduct, Arc<SweepGrid>>,
    /// `Some` once this volume closed `VolumeStatus::Complete`.
    pub complete: Option<VolumeSummary>,
    bytes: usize,
}
```

| Item | Behaviour |
|---|---|
| `fn bytes(&self) -> usize` | maintained incrementally on every insert — the ring's budget arithmetic must not be O(grids) per applied event |
| `fn insert_sweep(&mut self, DisplaySweep)` | replaces the entry for that elevation number, adjusting `bytes` by the difference; a repeated elevation *number* within one volume is a re-closure, not a second tilt |
| `fn set_derived(&mut self, Vec<Arc<SweepGrid>>)` | replaces wholesale (ADR-0012's rule, unchanged), adjusting `bytes` |
| `fn grid(&self, DisplayProduct, Option<u8>) -> Option<&Arc<SweepGrid>>` | one lookup rule for both base (`Some(elevation)`) and derived (`None`) — the render side keys the same way, so define it once here |

```rust
/// Operator-set retention bounds (ADR-0030 §3.3). Configuration, not view
/// state: it reaches `AppState` at construction and never flows backwards
/// from the render loop (ADR-0018).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy { pub frames: usize, pub budget_bytes: usize }
```

`Default` = the §3.5 numbers, expressed as named constants
`DEFAULT_HISTORY_FRAMES` and `DEFAULT_HISTORY_BUDGET_BYTES` with the measured
justification in their doc comments. `RetentionPolicy::DISABLED` (`frames: 1`,
`budget_bytes: 0`) is the "Stage 5 footprint" setting and must be a first-class,
tested configuration — see §3.3's never-evict-the-newest invariant.

```rust
/// The retained volumes, oldest first. `Arc<Frame>` so a snapshot costs one
/// refcount bump per frame whatever each frame holds (ADR-0030 §3.2); the
/// newest frame is mutated through `Arc::make_mut`, which copies only while
/// a snapshot is outstanding.
pub struct FrameRing { frames: VecDeque<Arc<Frame>>, policy: RetentionPolicy, bytes: usize, budget_bound: bool }
```

| Item | Behaviour |
|---|---|
| `fn new(RetentionPolicy) -> Self` | |
| `fn head_mut(&mut self, volume, vcp_number, now) -> Result<&mut Frame, LateVolume>` | the newest frame if its `volume` matches; a **new** frame pushed if `volume` is newer; the matching existing frame if one is retained; `Err(LateVolume)` if `volume` is older than the newest and no frame matches — creating a frame behind the head would break the ordering invariant, and the caller reports it |
| `fn trim(&mut self) -> Option<Event>` | evict from the front while `len > policy.frames \|\| bytes > policy.budget_bytes`, **never** below one frame; returns `Some(Event::HistoryBudgetBound { .. })` on the rising edge of "the budget, not the count, is what bit" and clears the flag on the falling edge |
| `fn recompute_bytes(&mut self)` | called by `trim`; `bytes` is the sum of frame `bytes` |
| `fn newest(&self) -> Option<&Arc<Frame>>` / `oldest` / `len` / `bytes` / `iter` | |
| `fn snapshot_frames(&self) -> Vec<Arc<Frame>>` | oldest → newest; the whole cost of `AppState::snapshot`'s history |
| `fn clear(&mut self)` | site change (§3.4) |

Two pure read-side folds, in this module because they are the definition of the
live view and must have exactly one definition:

```rust
/// The live merged view: newest sweep per elevation number, folding the ring
/// newest-first and **stopping at the first frame whose VCP differs from the
/// newest frame's**. That stop is what reproduces Stage 2's
/// `sweeps.clear()`-on-VCP-change without deleting any history (ADR-0030
/// §3.4). Sorted by elevation number, as `RadarState.sweeps` always was.
pub fn live_sweeps(frames: &VecDeque<Arc<Frame>>) -> Vec<DisplaySweep>;

/// Echo Tops / VIL from the newest frame that has them, within the newest
/// frame's VCP. Same stop rule, same reason.
pub fn live_derived(frames: &VecDeque<Arc<Frame>>) -> Vec<Arc<SweepGrid>>;
```

Tests (same module):

- `a_frames_byte_count_tracks_its_grids` — insert, replace, and confirm `bytes`
  matches a recomputed sum; the incremental accounting is the thing most likely
  to drift
- `replacing_a_sweep_in_a_frame_does_not_double_count`
- `the_newest_frame_is_never_evicted_even_under_a_zero_budget` —
  `RetentionPolicy::DISABLED`
- `the_frame_count_bound_evicts_the_oldest_first`
- `the_byte_budget_evicts_before_the_frame_count_when_frames_are_large`
- `the_budget_bound_event_is_edge_triggered` — a steady-state ring at the
  budget reports once, not once per eviction
- `a_volume_older_than_the_head_and_not_retained_is_rejected`
- `a_late_sweep_for_a_retained_frame_lands_in_that_frame_not_the_head`
- `live_sweeps_folds_newest_first_and_carries_elevations_forward` — frame *k*
  has elevations 1 and 2, frame *k+1* has only 1; the fold yields 1 from *k+1*
  and 2 from *k*. This is FR-DA-3, restated as a property of the fold
- `live_sweeps_stops_at_a_vcp_boundary` — the direct analogue of
  `vcp_change_drops_elevations_from_the_old_pattern`
- `a_vcp_change_does_not_evict_the_old_patterns_frames`
- `live_derived_comes_from_the_newest_frame_that_has_it`

### W4 — `Event` arms for history

`crates/radar-workstation/src/event.rs`, beside `RetainedGridSetBounded`:

```rust
/// The byte budget, not the requested frame count, is what bounds the
/// history — the loop is shorter than the operator asked for. Edge-
/// triggered: reported when the constraint starts binding, not per eviction.
HistoryBudgetBound { frames_retained: usize, requested_frames: usize, bytes: usize },
/// A sweep or derived set arrived for a volume older than every retained
/// frame. Discarded: inserting behind the head would break the ring's
/// ordering. Observability, not data (ADR-0012's rule table).
LateVolumeDiscarded { volume: VolumeId },
```

Both get a `Display` arm and both go into `event.rs`'s existing
`display_formats_are_human_readable` test.

### W5 — Rewire `RadarState` and `state::apply` onto the ring

**`state/mod.rs`:**

1. `RadarState`'s `sweeps`, `derived`, `derived_volume`, `current_vcp` and
   `last_complete` fields are **replaced by** `history: FrameRing`. Every one of
   them is now derivable: `current_vcp` is `newest().vcp_number`;
   `derived_volume` is the newest frame with a non-empty `derived`; the
   out-of-order guards those fields existed to enforce are structural now, since
   a sweep can only ever land in its own volume's frame.
2. `RadarState::new(site, policy)`; `reset(site)` calls `history.clear()` and
   bumps `revision` as it does today.
3. `AppState::new(site, ingest, policy)`. Five call sites — `main.rs`,
   `headless`'s tests, `state`'s tests, `render/mod.rs`'s tests,
   `tests/pipeline_live.rs`. Add the parameter rather than a second constructor:
   a hidden default retention in production is exactly the kind of thing this
   codebase makes explicit everywhere else.
4. `StateSnapshot` gains **one** field and every existing field keeps its
   meaning:
   ```rust
   /// Every retained frame, oldest → newest (ADR-0030). Cloning this is one
   /// refcount bump per frame, whatever each frame holds.
   pub frames: Vec<Arc<Frame>>,
   ```
   `sweeps` becomes `history::live_sweeps(..)`, `derived` becomes
   `history::live_derived(..)`, and `last_complete` becomes the newest frame's
   `complete` searching backwards. No consumer of `StateSnapshot` changes.
5. `snapshot()` keeps its signature. Its cost is now: N `Arc<Frame>` clones,
   plus the live fold (bounded by retained frames × elevations, ≤ 12 × 40 map
   probes) and the same ~14 `DisplaySweep` clones it already did. Say so in the
   doc comment, with the numbers.

**`state/apply.rs`:** the four `StateUpdate` arms become routing into the ring.
Every rule in the current doc comments survives; state where each now lives.

- `SweepGridded` → `history.head_mut(volume, vcp_number, now)?` then
  `insert_sweep`, then `trim`, then bump `revision`. `Err(LateVolume)` returns
  `false` and the event is reported by `AppState::apply_event` (the same split
  `StateUpdate::Info` already uses: pure `apply` touches only `RadarState`;
  `AppState` owns the event log). The VCP-clear branch **disappears** — §3.4's
  fold replaces it, and `vcp_change_drops_elevations_from_the_old_pattern` must
  still pass against `live_sweeps`.
- `DerivedComputed` → the frame for that `VolumeId`; `set_derived`; `trim`;
  bump. The `derived_volume` staleness guard disappears with its field.
- `VolumeClosed` → `Complete` sets `frame.complete`; `TimedOut`/`Superseded`
  change nothing, exactly as now. If no frame matches (a volume that closed
  without a single gridded sweep) it is a no-op returning `false`.
- `Info` → unchanged.

**Tests.** Every existing test in `apply.rs` stays, adapted to read through
`history::live_sweeps(...)` instead of `state.sweeps`, and **their assertions
must not weaken**. Add:

- `a_second_volume_starts_a_second_frame`
- `sweeps_from_one_volume_all_land_in_one_frame`
- `a_stale_sweep_lands_in_its_own_frame_and_does_not_disturb_the_live_view` —
  the replacement for `stale_sweep_does_not_overwrite_newer`, now a structural
  property rather than a guarded comparison
- `a_completed_volume_marks_its_own_frame_complete`
- `reset_clears_the_history_ring`
- `history_depth_never_exceeds_the_policy`

### W6 — Config keys and wiring

`crates/radar-workstation/src/config/mod.rs`, following the `view.highways` and
window-geometry patterns already there:

```rust
/// FR-DA-9 / ADR-0030. Read-only in the ADR-0019 sense: loaded, never
/// written back — like `ingest.poll_interval_seconds`, unlike `view.*`.
pub const HISTORY_FRAMES_KEY: &str = "history.frames";
pub const HISTORY_BUDGET_MB_KEY: &str = "history.budget_mb";
pub const HISTORY_FRAMES_RANGE: RangeInclusive<usize> = 1..=64;
pub const HISTORY_BUDGET_MB_RANGE: RangeInclusive<usize> = 0..=4096;
```

`Config` gains `history_frames: Option<usize>` and `history_budget_mb:
Option<usize>`; out-of-range values report `ConfigValueInvalid` and fall back to
the default, matching the window-geometry precedent (clamping is right for the
poll interval, where every in-range value is safe; it is wrong here, where the
operator has asked for a specific memory commitment). `main.rs` builds
`RetentionPolicy` from the two and passes it to `AppState::new`. `budget_mb = 0`
is valid and means `RetentionPolicy::DISABLED`.

Tests: the two keys parse; out-of-range reports and falls back; `0` is accepted
for the budget and rejected for the frame count; a file with neither key yields
`RetentionPolicy::default()`.

### W7 — `render::radar`: identity, frame-keyed cache, residency (parent B4)

**W7.1 — Make the §2.2 defect unrepresentable.** `upload_grid` takes
`Arc<SweepGrid>` (not `&SweepGrid`) and stores it directly:

```rust
fn upload_grid(device: &wgpu::Device, queue: &wgpu::Queue, grid: Arc<SweepGrid>) -> CachedGrid
```

After this the function has no `SweepGrid` to deep-copy and the identity test
`plan_sync` performs is sound by construction — the same repair shape Part A used
for `VolumeSeq`. `sync`'s call site passes `Arc::clone(grid)`.

**W7.2 — Key the cache by frame.**

```rust
/// A cache key: which frame, which product, and — for a base product — which
/// elevation. Derived products (Echo Tops / VIL) are one per volume and key
/// as `None`.
pub type GridKey = (VolumeId, DisplayProduct, Option<u8>);
```

`VolumeId` gains `Hash` (one derive in `assembly/mod.rs`). `grid_key` becomes
`fn grid_key(volume: VolumeId, grid: &SweepGrid) -> GridKey` and keeps its
derived-product rule — which is the same rule as `Frame::grid`'s lookup, so
express one in terms of the other rather than writing the match twice.

**W7.3 — The residency planner.** Delete `snapshot_entries` and replace it:

```rust
/// What must be GPU-resident this frame (ADR-0030 §3.7): every grid of the
/// newest frame — so a product or elevation switch still uploads nothing
/// (FR-RP-7) — plus the selected `(product, elevation_number)` grid of every
/// other retained frame, so playback of that selection runs upload-free.
/// Oldest frames drop out first when `budget_bytes` binds; the newest frame
/// is never dropped. Pure: no device, no queue, no `ViewState`.
pub fn residency(
    frames: &[Arc<Frame>],
    product: DisplayProduct,
    elevation_number: u8,
    budget_bytes: usize,
) -> Vec<(GridKey, Arc<SweepGrid>)>
```

`HISTORY_GPU_BUDGET_BYTES` is a module constant, not a config key — the 128 MB
figure is a hardware target, not an operator preference. Its doc comment carries
the arithmetic from §2.1: 128 MB target, less the ADR-0029 overlay's 11.46 MB,
less the newest frame's ~40 MB, leaves ~76 MB for the tail — ~60 frames of one
super-resolution reflectivity grid, i.e. the GPU is not what bounds the loop.

**W7.4 — Rate-limit uploads.** `plan_sync(cached, entries, max_uploads)` returns

```rust
pub struct SyncPlan { pub to_upload: Vec<GridKey>, pub to_evict: Vec<GridKey>, pub more_pending: bool }
```

ordering `to_upload` newest-frame-first so the displayed frame is never the one
that waits. `MAX_UPLOADS_PER_FRAME = 4` with the §3.7 justification in its doc
comment. Evictions are never rate-limited — freeing is free, and a bounded cache
must be allowed to shrink promptly.

Build the eviction diff against a `HashSet<GridKey>` of the residency keys
rather than the current `entries.iter().any(..)` linear scan; the set is now up
to ~80 entries and grows with history depth.

**Tests** (pure, no GPU):

- `residency_holds_every_grid_of_the_newest_frame`
- `residency_holds_only_the_selected_grid_of_older_frames`
- `residency_drops_the_oldest_frames_first_under_the_budget`
- `residency_never_drops_the_newest_frame` — even at `budget_bytes = 0`
- `residency_of_a_derived_product_selects_it_from_every_frame` — Echo Tops/VIL
  key with `None` and must still form a loop
- `a_selection_absent_from_an_older_frame_is_simply_absent` — a tilt a VCP
  change removed; no panic, no placeholder
- `walking_the_selection_across_every_frame_uploads_nothing_once_resident` —
  §1's acceptance criterion 6: build the cache from one `residency` call, then
  call `plan_sync` once per frame of a synthetic playback pass and assert
  `to_upload` is empty every time
- `a_product_switch_uploads_the_tail_at_the_rate_limit_and_reports_more_pending`
- `the_newest_frames_grids_are_uploaded_before_the_tail`

**GPU test** (in the existing `#[ignore]`d offscreen suite): `sync` twice over
the same snapshot and assert the second call's plan is empty — the direct
regression for §2.2, which no pure test can catch because it is a property of
what `sync` puts in the cache.

`fake_snapshot` in that suite needs a `frames` field; give it one frame built
from the same grids so the helper stays a single definition.

### W8 — Render-loop wiring

`crates/radar-workstation/src/render/mod.rs`:

1. The upload gate becomes `(revision, product, elevation_number)` (§3.6) —
   store it as one `Option<(u64, DisplayProduct, u8)>` replacing
   `last_uploaded_revision`.
2. `radar.sync(...)` takes the residency selection and returns
   `more_pending`; when true, request an immediate redraw so the tail fills over
   the next few frames rather than waiting for the 2 Hz idle tick.
3. `resolve_selection` and `displayed_scan` keep reading `snapshot.sweeps` —
   the live merged view — so the live display is byte-for-byte what it is today.
   **No timeline, no pinning, no frame selection.** That is Part C.
4. `radar.draw` needs the selected grid's `GridKey`, which now needs a
   `VolumeId`; take it from the `DisplaySweep` the selection resolved from
   (derived products from the newest frame that has them). Do not reconstruct
   it — `resolve_selection` already has the sweep in hand.
5. `view_state_is_unchanged_by_any_sequence_of_state_updates` must still pass
   untouched. If it needs editing, something has gone wrong in W5 or W8; stop
   and re-read ADR-0018 §"View state is not shared at all".

`crates/radar-workstation/src/headless.rs`: extend `format_state_line` with
` frames=<n>/<policy> hist=<n>MiB` after `rev=`. This is Part B's only
user-visible surface and it is what makes the ring diagnosable before Part C
draws anything.

### W9 — Documentation

| File | What to change |
|---|---|
| `CLAUDE.md`, "Shared State" | `RadarState` retains a bounded ring of per-volume frames (ADR-0030); the merged live view is a read-time fold; `ViewState` is still never in `AppState` |
| `CLAUDE.md`, "Status" | Stage 6a Part B: history retention lands; still no timeline, no placefiles |
| `CLAUDE.md`, ADR index | add `0030` |
| `CLAUDE.md`, "Performance Targets" | the §3.5 memory rows |
| `docs/architecture/data-flow.md` | the applier now routes into the ring; snapshot cost restated |
| `docs/architecture/overview.md` | same, wherever it describes `RadarState`'s retention |
| `docs/architecture/rendering.md` | the residency rule and the amended FR-RP-7 statement: a switch uploads nothing for the *displayed* frame; the history tail fills at a bounded rate |
| `docs/plans/stage-6a-time-handling.md` | mark Part B done and correct its B2 estimate ("plausibly 20–35 MB") against §2.1's measurement. This is the live parent plan, so it does get corrected |

Do **not** edit completed plan documents under `docs/plans/` (stage-0-1 through
stage-5, and `stage-6a-part-a-stale-data.md` once it is landed). They are records
of what was known when written.

---

## 5. Test matrix

| Property | Where |
|---|---|
| A frame holds only its own volume's sweeps | `apply`: `sweeps_from_one_volume_all_land_in_one_frame` |
| Carry-forward across a volume boundary still works (FR-DA-3) | `history`: `live_sweeps_folds_newest_first_and_carries_elevations_forward` |
| A VCP change hides the old pattern without deleting it | `history`: `live_sweeps_stops_at_a_vcp_boundary`, `a_vcp_change_does_not_evict_the_old_patterns_frames`; `apply`: the existing `vcp_change_drops_elevations_from_the_old_pattern` |
| An out-of-order sweep cannot corrupt the live view | `apply`: `a_stale_sweep_lands_in_its_own_frame_and_does_not_disturb_the_live_view` |
| Retention never exceeds either bound | `history`: `the_frame_count_bound_evicts_the_oldest_first`, `the_byte_budget_evicts_before_the_frame_count_when_frames_are_large`; `apply`: `history_depth_never_exceeds_the_policy` |
| The display survives a budget too small for one volume | `history`: `the_newest_frame_is_never_evicted_even_under_a_zero_budget` |
| Byte accounting does not drift | `history`: `a_frames_byte_count_tracks_its_grids`, `replacing_a_sweep_in_a_frame_does_not_double_count` |
| A shortened loop is reported, once | `history`: `the_budget_bound_event_is_edge_triggered` |
| A site change clears history | `apply`: `reset_clears_the_history_ring` |
| `ViewState` is still untouched by any state update | `render`: the existing `view_state_is_unchanged_by_any_sequence_of_state_updates`, unedited |
| A synced grid is never re-uploaded | `radar` (GPU, `#[ignore]`d): the double-`sync` test |
| Playback does zero uploads once resident | `radar`: `walking_the_selection_across_every_frame_uploads_nothing_once_resident` |
| A switch never stalls a frame on uploads | `radar`: `a_product_switch_uploads_the_tail_at_the_rate_limit_and_reports_more_pending` |
| GPU residency is bounded | `radar`: `residency_drops_the_oldest_frames_first_under_the_budget`, `residency_never_drops_the_newest_frame` |
| Config bounds are enforced, not clamped | `config`: the range tests |

---

## 6. Validation

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check && cargo audit          # no dependency change expected; confirm
cargo run -p radar-viz -- --path budget downloads/KDOX_20260629_1811
cargo test -p radar-workstation --bins -- --ignored --nocapture   # GPU offscreen
cargo test -p radar-workstation -- --ignored --nocapture          # live, network
cargo run --release -- KDOX --headless < /dev/null
cargo run --release -- KDOX
```

In the headless run, confirm across at least three volume boundaries: `frames=`
climbs to the policy and stops; `hist=` tracks it; `HistoryBudgetBound` appears
at most once.

In the windowed run, confirm the Stage 5 behaviour is **unchanged** — this is
the main risk of Part B and the thing to actually look at:

- the displayed product/tilt, its colours, and the cursor readout are identical
  to before the change;
- a tilt scanned in the previous volume is still displayed during the next
  volume's early sweeps (FR-DA-3);
- product and elevation switching is still instant;
- `grep VmHWM /proc/<pid>/status` after several volumes matches
  `<W1.3 baseline> + hist=` within a few tens of MB. A large discrepancy means
  a grid is being retained somewhere the ring does not account for — most
  likely a stale `Arc` in the GPU cache (W7.1) or in `compute::RetainedTilts`.

---

## 7. What this plan deliberately does not do

- **No timeline, no playback, no keys, no UI.** `ViewState` gains no field,
  `input::Action` gains no variant, `ui.rs` gains no widget. If a work item
  seems to want one, it is Part C leaking; stop.
- **No selective retention by product or elevation.** §3.3 rejected it on
  principle, not on effort. Do not add a "while we're in here" product filter.
- **No archive reads, no backfill.** `Bucket::Archive` stays unconstructed;
  the ring fills only from the live stream.
- **No new dependencies.** A `VecDeque` of `Arc<Frame>` and two pure folds.
- **No change to gridding, palettes, or the shader.** `compute::RetainedTilts`
  stays exactly as it is: it holds `Arc` clones of the accumulating volume's
  reflectivity grids, which is not duplication — the same allocations the ring
  holds — and it is on the compute side of a one-way boundary.
- **No lock-shape change.** ADR-0018's `RwLock<RadarState>` and its
  poison-recovery helpers are untouched. The `ArcSwap` refinement that ADR
  anticipated is still a later, measured change.

---

## 8. Risks

- **The default budget is a product decision wearing a constant's clothes.**
  §3.5 flags it. Get it confirmed rather than assuming; it is the one number
  here whose wrongness the operator feels directly, in either direction.
- **W5 rewrites the most heavily tested pure function in the tree.** Every
  `state::apply` rule is currently enforced by an explicit guard; several become
  structural. A structural guarantee is stronger *if it actually holds* — so the
  existing tests must be adapted, never weakened, and any test that becomes
  "trivially true" should be read carefully before it is kept: it may have
  become trivially true because the property was lost, not because it was
  absorbed into the type.
- **VCP 12 and 212 may measure materially larger than 35.** They fly more
  super-resolution cuts. If W1 returns 55–60 MB per frame, the default budget
  buys fewer frames than §3.5 assumes and the number should be revisited before
  W2 writes it into the ADR — that is the whole reason W1 precedes W2.
- **`Arc::make_mut` is a silent copy if a snapshot is outstanding.** At one
  closed sweep per ~20 s and a `Frame` clone costing a `BTreeMap` walk, this is
  nothing — but it is worth a comment at the call site so a future contributor
  who moves this onto a hot path knows what they are paying.
- **The residency rule assumes the newest frame is the one being displayed.**
  True in Part B by construction (there is no pinning). Part C must revisit
  `residency` when `Pinned` exists — leave a doc-comment note saying so rather
  than pre-building for it.

---

## 9. Commit policy

**Do not create commits, branches, tags, or pull requests. Do not run
`git add`, `git commit`, `git push`, or `git checkout -b`.** Leave all changes
in the working tree and report what was changed, what passed, and anything left
undone — including the W1 measurements and the §3.5 default the developer needs
to confirm. The developer reviews and commits.

---

## 10. Measured results

*(Filled in by W1 before W2 writes ADR-0030. The KDOX VCP 35 row is already
measured — §2.1 — and is repeated here in the harness's own output format so all
three rows are comparable.)*

| Site | VCP | Elevations | Base grids | Derived | Frame total |
|---|---|---|---|---|---|
| KDOX | 35 | 16 | 37.29 MB | 2.52 MB | **39.80 MB** |
| KFWS | 12 | 12 | 26.54 MB | 1.64 MB | **28.17 MB** |
| KHGX | 212 | 14 | 28.30 MB | 2.52 MB | **30.82 MB** |

KFWS (`downloads/KFWS_20260903_0320`, 55 chunks, one complete volume) and KHGX
(`downloads/KHGX_20260905_0119`, 61 chunks, one complete volume) were located by
probing live `-S` chunks for their Message 5 VCP number across several
precipitation-mode sites, then fetched with `aws s3 sync --no-sign-request`. Both
`downloads/` directories are left uncommitted, per §4 W1.2.

Contrary to the §8 risk ("VCP 12 and 212 may measure materially larger than 35"),
**both measured smaller than VCP 35's 39.80 MB** — KDOX's 16 clear-air elevation cuts
outweigh the two precipitation volumes' higher per-tilt gate density at 12 and 14 cuts
respectively. This is a property of the two live volumes actually captured, not a
ceiling on any VCP's cost, but it means the chosen defaults (§3.5) are sized against the
larger, not the smaller, end of what was measured — margin, not a shortfall.

Baseline RSS, Stage 5 application, no history (W1.3): **VmHWM ≈ 393 MB (402,284 kB)**,
windowed, live KDOX data, 18+ minutes of run time, this development machine (NVIDIA
RTX 4070 SUPER, Vulkan backend). This is well above both the 200 MB target and
ADR-0018's earlier ~147 MB (headless-adjacent) figure; the gap is GPU-driver/Vulkan
memory-mapping overhead this machine's discrete adapter charges against RSS, not
apparent application-owned heap growth — the run's own log shows only the expected
11.46 MB overlay allocation as a large one-time cost. Recorded as measured rather than
reconciled; see ADR-0030 §"Decision" 5 for the full note and the follow-up (re-measure on
the ADR-0024 hybrid-GPU box) this plan does not resolve.
