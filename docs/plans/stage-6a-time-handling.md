# Plan — Time handling: staleness, history, and timeline controls

**Status:** proposed, not started
**Date:** 2026-09-04
**Scope:** why displayed data is hours stale today, and the sequence of work
needed to give the application a time axis — history retention, animation/loop,
step forward/back, pause, and jump-to-time.
**Relationship to other plans:** this is proposed to run *before* Stage 6
(placefiles). Nothing here depends on Q6/Q21–Q24. It does touch ADR-0018
(shared state) and ADR-0011 (chunk stream data source), and will need an ADR of
its own for history retention.

---

## 1. Why the data is stale — root cause, confirmed live

The displayed volume is not merely lagging. The poller is **permanently
anchored to a pre-wrap volume and will never advance again** without a restart,
and even a restart re-anchors to the same wrong place.

### 1.1 The chunk bucket's volume-sequence counter wraps at 999

`CLAUDE.md` and `ingest/s3_poll.rs` both describe `<volume-sequence>` as
"unpadded, monotonically increasing." The unpadded part is right. The
monotonic part is wrong: it is a **cyclic counter over 1–999**.

Measured against `unidata-nexrad-level2-chunks`, prefix `KDOX/`,
`delimiter=/`, at 2026-09-04T00:55Z — 451 `CommonPrefixes`, in two runs:

```
contiguous runs: [(1, 199), (659, 659), (749, 999)]

KDOX/999/  newest object  2026-09-03T03:08:14Z   <- last volume before the wrap
KDOX/1/    newest object  2026-09-03T03:13:46Z   <- first volume after the wrap
KDOX/199/  newest object  2026-09-04T00:55:43Z   <- live, seconds old
```

The counter rolled 999 → 1 at ~03:13Z on 2026-09-03. Both sides of the wrap are
present in the listing simultaneously because objects outlive the wrap.

### 1.2 `cold_start_baseline` takes a numeric max over a cyclic sequence

```rust
fn cold_start_baseline(folders: &[u64]) -> u64 {
    folders.iter().copied().max().unwrap_or(0).saturating_sub(1)
}
```

`max()` returns 999 whenever any pre-wrap folder is still retained. Baseline
998, target 999 — a volume whose data is from 03:08Z. That is exactly the
"~3Z data at 20:25Z" symptom, and it is reproducible from a cold start at any
time in the ~day-and-a-half after a wrap.

### 1.3 The recovery path then deadlocks, so it never self-heals

`poll_once` drains volume 999, sees its `-E`, and advances the target to
**1000**. That directory can never exist. After `REANCHOR_EMPTY_POLLS` the
re-anchor path re-lists and calls `next_target`, which computes:

```rust
let newest    = folders.iter().copied().max().unwrap_or(...);  // 999
let candidate = newest.saturating_sub(1).max(state.current_target); // max(998, 1000) = 1000
if candidate > state.current_target { ... }                    // 1000 > 1000 -> false
```

→ `PollAction::Continue`, forever. The "never re-anchor backwards" guard, which
is correct for a monotonic counter, is precisely what prevents recovery across a
wrap, where the correct move *is* backwards numerically. The poller polls
`KDOX/1000/` indefinitely while publishing `IngestState::Polling` and a healthy
`last_success` every 5 seconds.

### 1.4 Two secondary findings

- **The status bar's age readout is measured from the wrong clock.**
  `ui.rs:183` renders `age_secs(snapshot.ingest.last_success, now)` as
  "updated Ns ago". `last_success` is the last *successful poll*, which
  succeeds every 5 s while stuck. The one honest staleness number — now minus
  the displayed volume's own scan time — is computable (`displayed_volume` is
  already in `ChromeInput`, `time::utc_from_nexrad` already exists) and is not
  shown. A 21-hour-stale display currently reads "updated 3s ago".
- **Retention is longer than documented.** `CLAUDE.md` says chunks persist a
  maximum of 24 hours; `KDOX/749/` was still serving objects from
  2026-09-02T00:05Z at 2026-09-04T00:55Z — ~49 hours. This widens the window in
  which both sides of a wrap coexist, and it is also good news for §4: a
  multi-hour loop can be backfilled from the chunk bucket alone.

---

## 2. What does not exist yet for time controls

- **No history.** `RadarState` keeps exactly one `DisplaySweep` per elevation
  number and one set of derived grids, each replaced wholesale on arrival.
  A previous volume's grids are dropped the moment the next one is applied.
  There is nothing to animate.
- **No time in the view.** `ViewState` carries `center_m`, `m_per_px`,
  `product`, `elevation_number`, and layer toggles. There is no notion of a
  selected frame, live-vs-pinned, or playback.
- **No time actions.** `input::Action` has no play/pause/step/scrub variants,
  and no keys are free-form reserved for them.
- **No archive reader.** `Bucket::Archive` exists in `http-ingest/src/host.rs`
  and is never constructed. `chunk.rs` handles `-S`/`-I`/`-E` envelopes only;
  nothing reads an `AR2V` archive volume file's type-32 BZ2-wrapped records.
  Jump-to-an-arbitrary-past-time needs that path.

---

## 3. Roadmap

Sequenced. Part A is a bug fix and is independently shippable. Part B is the
architectural prerequisite for everything after it. Part C is the user-visible
feature. Part D extends the reach of the time axis beyond the chunk bucket's
retention.

### Part A — Stop displaying stale data

**A1. Record the wrap as a format finding.**
Correct `CLAUDE.md`'s "monotonically increasing per-site integer" to "cyclic
1–999 counter", correct the 24-hour retention claim to "observed ~48 h, not
contractual", and add the measured evidence from §1.1. Fix the same claim in
`ingest/s3_poll.rs`'s `S3Poller` doc comment, which is where the wrong
assumption was written down and then coded to.

**A2. Make newest-volume selection wrap-aware.**
Replace `cold_start_baseline`'s `max()` with a function that picks the newest
volume from a possibly-wrapped folder set. The listing alone is sufficient:
split the sorted set into contiguous runs, and when there is more than one run
separated by a large gap, the *lowest-numbered* run is the newer one. Keep it a
pure function so the §1.1 measurement becomes a fixture-backed unit test. A
tiebreak against object `LastModified` may be needed for the ambiguous case of a
single run — decide with a test, not by inspection.

**A3. Make target advance and re-anchor wrap-aware.**
`volume + 1` must become a successor function that rolls 999 → 1, and
`next_target`'s "never re-anchor backwards" guard must be re-expressed as
"never re-anchor backwards *in time*" — comparing positions on the cycle, not
integers. This is the change that makes the failure self-healing rather than
terminal, and it is the one to write tests against first: cold start mid-wrap,
`-E` seen on 999, and a stuck target of 1000 must all converge on the live
volume.

**A4. Show honest data age.**
Add a second, primary age readout derived from the displayed volume's own
`VolumeId` (`julian_date`, `scan_time_ms`) against wall-clock UTC, and colour it
as an alert past a threshold (a volume older than ~2 VCP cycles is a real
problem). Keep the poll-health readout, but label it as poll health, not as data
freshness. This is the guard that would have surfaced §1.3 immediately, and it
is worth having independently of A2/A3.

**A5. A live regression test for the wrap.**
Add an `#[ignore]`d live test alongside the existing ones in
`ingest/s3_poll.rs`'s live suite that lists real volume folders and asserts the
chosen anchor's newest object is within one VCP cycle of now. Cheap, and it
catches any future change in the bucket's numbering scheme rather than trusting
a comment.

### Part B — Retain history

**Done (2026-09-04).** Executed as
`docs/plans/stage-6a-part-b-retain-history.md`, which is the record of what
was actually decided and built — read it, not the four sketches below, which
are left as originally written per this document's own preamble. In one
line: B1 became [ADR-0030](../adr/0030-volume-history-retention.md); B2's
"plausibly 20–35 MB" estimate was measured directly (below); B3 and B4 are
`state::history::FrameRing` and `render::radar`'s `residency`/`plan_sync`
respectively.

**B1. Decide and record the retention model (new ADR; amends ADR-0018).**
The question is what a "frame" is. Proposed: a frame is one *volume*, and the
history is a ring of per-volume grid sets keyed by `VolumeId` — not a ring of
sweeps, because animating a tilt means stepping the same elevation number across
volumes, and the volume is the only boundary at which Echo Tops and VIL are
defined. Decide the eviction rule (frame count, byte budget, or both), whether
history survives a VCP change (it must not silently mix elevation sets), and
whether it survives a site change (no — `RadarState::reset` clears it).

**B2. Measure the memory cost before choosing the cap.**
A super-res reflectivity grid is 720 × 1832 ≈ 1.32 MB; a velocity/SW/ZDR/CC cut
is ~0.86 MB each. A VCP-35 volume is plausibly 20–35 MB of grids, which puts a
20-frame loop far past the 200 MB per-instance target. Measure it for real
across VCP 12/35/212 with a small harness before the cap is a number in a
config file. The likely outcomes are a per-product history (only the selected
product is retained deep) or a byte-budget ring — pick after measuring, not now.

> **Corrected 2026-09-04**, against the measurement Part B actually ran
> (`utility/radar-viz --path budget`, three live volumes): a whole volume is
> **28–40 MB**, not 20–35 — the low end was about right, the high end was
> low. A 20-frame loop is therefore ~560–800 MB, not "far past 200 MB" by a
> vague margin but by a specific, now-recorded one
> ([ADR-0030](../adr/0030-volume-history-retention.md)). The two outcomes this
> paragraph guessed at were both considered and both rejected — see the ADR's
> Alternatives — in favor of whole frames under a frame-count-and-byte-budget
> pair.

**B3. Implement the history ring in `RadarState`.**
Add the ring behind the existing private-field discipline, mutated only through
`apply`. Extend `StateSnapshot` with the frame list — identities and cheap
metadata for every retained frame, plus the grids of the selected frame only, so
per-frame snapshot cost stays a handful of refcount bumps and the render loop
never clones a whole history. `revision` semantics need re-examining: the render
loop currently treats it as "the texture may have changed", and with history it
must distinguish "a new frame arrived" from "the selected frame changed".

**B4. Bound the GPU-side cache.**
`render::radar`'s grid-texture cache is keyed for a single live frame today.
Animation will cycle through N frames per second and must not re-upload each
one every pass. Give the cache an explicit LRU bound expressed in bytes against
the 128 MB GPU target, and confirm that playback at the target frame rate does
zero uploads in steady state once the loop is resident.

### Part C — Timeline controls

**C1. Add timeline state to `ViewState`.**
A `Timeline` field holding the selection mode (`Live` — always the newest frame,
the current behaviour and the default — or `Pinned(VolumeId)`), the playback
state (stopped/playing), the loop length in frames, and the playback rate. It
belongs in `ViewState`, not `AppState`: it is operator view state, and ADR-0018's
boundary plus the `view_state_is_unchanged_by_any_sequence_of_state_updates`
test both apply unchanged. Resolving a `Timeline` plus a `StateSnapshot` to a
concrete frame is a pure function and should be unit-tested as one, including
the cases where the pinned frame has been evicted or the newest frame lacks the
selected product/elevation.

**C2. Add the actions and key bindings.**
Extend `input::Action` with `PlayPause`, `StepForward`, `StepBackward`,
`JumpToLive`, `LoopLonger`/`LoopShorter`, and `SpeedUp`/`SpeedDown`. Follow
GR2Analyst's conventions where it has them, as `input.rs`'s module doc already
requires. Space for play/pause and left/right bracket for stepping are the
obvious candidates; arrows are taken by pan and must stay taken. `StepForward`
past the newest frame should return to `Live` rather than dead-ending. Update
the `F1` help overlay and `CLAUDE.md`'s keyboard map in the same change.

**C3. Drive playback from the frame clock.**
Playback advances on wall-clock time inside the render loop's existing pacing —
not on a timer task, and not on data arrival. Hold the last frame for a
configurable dwell so the loop's end is readable, and define what a newly
arrived volume does mid-playback (it extends the loop; it does not interrupt
the current pass). While playing, the render loop must request redraws
continuously; while stopped it must return to the current on-demand pacing so
an idle instance still costs nothing.

**C4. Build the timeline UI.**
A scrubber above the status bar: one tick per retained frame, the selected one
marked, the scan time of the selected frame shown in full, and an unmissable
`LIVE` indicator when the mode is `Live`. Clicking a tick pins that frame;
dragging scrubs. Keep the egui surface minimal — the Instrument Principle
applies, and this is chrome that earns its place only by making the time axis
legible at a glance during a warning.

**C5. Persist what should persist.**
Loop length, playback rate, and dwell are operator preferences and belong in the
config file alongside the existing `H` toggle (FR-CP-1, ADR-0019). The selected
frame and playback state are not preferences and must not persist — every launch
starts `Live`.

**C6. Requirements.**
There is no FR covering animation today. Add the FR-TL-* group to
`REQUIREMENTS.md` §6's in-scope list and state plainly that a loop over retained
volumes is v1.0 scope. Without this the work has no requirement to be measured
against, and the inventory has nowhere to record it.

### Part D — Reach beyond the live stream

**D1. Backfill the loop from the chunk bucket.**
The cheapest path to a useful loop at startup, and it needs no new format
support: the chunk bucket retains ~48 h, so the previous N volume directories
are usually still there. On launch, walk backwards from the anchor volume
(wrap-aware, per A3) fetching and assembling N-1 prior volumes, then start live
polling. This must be cancellable, must not delay first render (the live volume
still arrives first), and must respect the same history cap as B1.

**D2. Read archive volume files.**
Jump-to-an-arbitrary-time beyond the chunk window needs the archive bucket's
`YYYY/MM/DD/SITE/` layout and its `AR2V` file format — a 24-byte volume header
plus type-32 messages wrapping internally BZ2-compressed sub-blocks, which
`chunk.rs` does not handle. `Bucket::Archive` already exists unused;
`nexrad-decoder::parse_radial_stream` already consumes a decompressed message
stream. The new work is the file envelope and the key enumeration, plus fixtures
— this is a decoder-adjacent change and gets decoder-grade hardening tests.

**D3. Jump-to-time as a pipeline mode.**
A `Historical { start, end }` mode alongside the live mode: the pipeline stops
polling, fetches the archive volumes covering the window, feeds them through the
existing assembly → compute → state path, and fills the history ring; the
timeline then behaves identically to the live case. This wants an explicit,
visible mode indicator — a historical display that looks like a live one during
a warning is exactly the failure `PHILOSOPHY.md` forbids.

**D4. Entering a time.**
The operator-facing half of D3: a way to type a UTC time, and a way to get back
to live. Small, but it is what makes D2/D3 usable, and it is the last piece of
"specify a specific time."

---

## 4. Sequencing notes

- **A is independent and should go first.** It is a correctness fix to shipped
  behaviour, it is small, and every part of B–D would otherwise be built on a
  poller that silently serves day-old data.
- **B1/B2 gate everything after them.** The retention cap is a measured number,
  and picking it wrong makes either the loop useless (too short) or the instance
  over-budget (too long). Do not start C until B2 has a number.
- **C is the whole user-visible feature** and is worth shipping on chunk-bucket
  data alone, before D exists at all.
- **D1 is much cheaper than D2–D4** and delivers most of the practical value
  (a loop that is populated at launch rather than after 40 minutes of watching).
  Treat D2–D4 as a separable follow-on.
- **Interaction with Stage 6 (placefiles):** placefiles are time-varying too
  (warnings expire, reports have timestamps). If the timeline lands first, the
  placefile layer can be written against it from the start instead of being
  retrofitted. That is an argument for this plan preceding Stage 6, not merely
  interleaving with it.

---

## 5. Open questions this raises

- **What is a frame?** (B1) Volume is proposed; per-sweep animation is the
  alternative and is what some operators expect for a fast tilt.
- **What is retained, and how much?** (B1/B2) All products for N volumes, or
  the selected product deeper? The answer changes both memory and how product
  switching behaves mid-loop.
- **What happens to the loop on a VCP change?** (B1) Elevation sets differ
  between patterns; a loop that silently mixes them is misleading.
- **Does the timeline survive a site change?** (B1, interacts with Stage 7's
  FR-DA-4.)
- **Is historical mode v1.0 scope, or post-v1.0?** (D2–D4) The loop (C) is
  clearly v1.0. Arbitrary-time archive playback is a larger surface and might
  reasonably be deferred — but it should be deferred deliberately, in an ADR,
  the way tiles were.
