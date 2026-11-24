# ADR-0031: Timeline Semantics

## Status
Accepted (2026-11-22)

Stage 6a Part C (`docs/plans/stage-6a-part-c-timeline.md`), executing Part C (C1–C6)
of `docs/plans/stage-6a-time-handling.md`. Resolves the playback questions that
`docs/plans/stage-6a-checkpoint.md` §5.2 required: CK-20 (what a loop position is and
what identifies a tilt), CK-22 (residency), CK-25 (age and the LIVE indicator) and
CK-26 (the placefile time hook). Adds `REQUIREMENTS.md` §2.11 (FR-TL-1 to FR-TL-10).

## Context

ADR-0030 retains a bounded ring of per-volume frames, each holding only the sweeps
its own volume closed. Nothing displays it: the render loop draws the live fold, and
the operator's tilt selection is an `elevation_number`.

`elevation_number` is not a tilt identity. It names different angles in different
VCPs: elevation 3 is 1.3° in VCP 35 and 1.8° in another pattern. It also splits one
tilt in two: a split cut's surveillance scan (REF, full range) and its Doppler scan
(VEL, range-limited REF) get consecutive numbers at the same angle. Stepping the same
number across a VCP change would play one angle under a label meant for another.

The measured angle is not a safe identity either. Measured angles drift up to ~0.08°
from the commanded angle (`nexrad_decoder::VcpElevationCut`'s doc comment, measured on
KDOX VCP 35). VCP 31's 2.5° sits 0.1° from VCP 35's 2.4°, so a tolerance wide enough
to absorb drift is too wide to keep those two apart.

The Message 5 commanded angle is exact, and it is identical across patterns that fly
the same cut.

## Decision

### 1. A loop position is a volume, not a sweep (CK-20)

A position is one retained frame (ADR-0030). A SAILS/MRLE re-scan is not a separate
position: within a frame, the selected tilt shows that volume's newest closure at that
angle. This matches GR2Analyst's volume-based loop.

### 2. A tilt is identified by its commanded cut angle (CK-20)

Each sweep carries `cut_deg = Message 5 cut[elevation_number − 1].elevation_deg`
(`assembly::cut_angle`). That value is used when Message 5 is present and lies within
0.25° (`CUT_ANGLE_TOLERANCE_DEG`) of the measured angle. Otherwise the sweep falls back
to its measured angle. Identity is `CutKey = round(cut_deg × 100)`
(`compute::cut::CutKey`, the 0.01° bucket `compute::derived` already used).

A fallback sweep almost never matches a commanded one across frames, so it fails
conservatively: the frame is marked as lacking the tilt rather than showing a
different angle. The measured `elevation_deg` stays the input to all beam geometry.
`cut_deg` is never used for geometry.

The GPU cache key stays keyed by `elevation_number` (`history::key_elevation`). Two
grids can share a cut within one frame, and a cache key must name exactly one grid.

### 3. One grid-selection rule, used everywhere

Among the grids at a `CutKey` that carry the product, take the greatest
`(range_extent_m, volume, elevation_number)`, where
`range_extent_m = first_gate_m + gate_count × gate_width_m`. This picks the full-range
surveillance REF over a split cut's range-limited Doppler REF. It picks the newest
SAILS closure among equals, and it picks the only VEL.

The rule is `compute::cut::best_at_cut`, defined once. It is used by
`compute::derived` (the Echo Tops/VIL tilt dedupe), the live view
(`history::live_grid_at_cut`), each frame (`Frame::grid_at_cut`) and GPU residency.

### 4. Live vs. frame resolution

`Live` shows the live fold (`history::live_sweeps` / `live_derived`), so it looks as
it did before. A paused or playing position shows **only that frame's own sweeps**.
Pausing from `Live` pins the frame that holds the displayed sweep, so the image does
not change on pause.

A frame where §3 finds nothing shows no radar data. The status bar names what is
missing, for example "No VEL at 0.5° in the 21:04Z volume". Step and play skip that
frame, and its scrubber tick is drawn hollow. Nothing is ever substituted.

A pinned frame that ages out of the ring resolves, without mutating `ViewState`, to
the oldest frame that satisfies §3. The status bar says so until the next timeline
action. The resolution is `render::timeline::resolve`, called once per frame. Every
consumer reads its one `Resolved`.

### 5. Residency follows the focus frame (CK-22)

The fully resident frame is the pinned frame while `Paused`, and the newest frame
while `Live` or `Playing` (`timeline::residency_focus`). Every other frame keeps its
§3-selected grid in the tail, as ADR-0030 §3.7 already did. The displayed grid and the
newest frame's selected grid are always included explicitly and are never budgeted
away. So a product or tilt switch while paused uploads nothing for the paused frame,
and playback does not pull in whole frames. Uploads go in residency's priority order:
displayed grid, focus frame, then tail.

### 6. One age readout, plus the indicator (CK-25)

The status bar's age always measures the **live feed**: the Live resolution's sweep,
with the existing two-VCP-cycle stale alert. In Live this is exactly the earlier
value. When not Live it is labelled `live age`. The scan-time field always shows the
**displayed** volume's time.

A timeline badge reads `LIVE`, or `PAUSED` / `LOOP` with the displayed volume's full
UTC time and its offset behind live (`−18 min`). It is drawn in a dedicated timeline
colour, distinct from the stale-alert accent. Being in the past on purpose never fires
the alert.

### 7. The placefile hook (CK-26)

`render::timeline::time_context(&Resolved) -> TimeContext { Live, At(unix_secs) }` is
the single pure accessor a future layer 7 reads. Nothing placefile-specific is built.

## Considered Alternatives

- **Identify a tilt by `elevation_number`.** Rejected: it names different angles in
  different VCPs and splits one split cut into two numbers, so a loop across a VCP
  change would show one angle under a selection meant for another.
- **Identify a tilt by measured angle within a tolerance.** Rejected: measured drift
  (~0.08°) and the 0.1° gap between VCP 31's 2.5° and VCP 35's 2.4° leave no tolerance
  that is both wide enough and narrow enough.
- **Per-sweep playback (animate SAILS cuts individually).** Rejected for v1.0: it
  needs an uneven time base and a second frame concept. Volume stepping matches
  GR2Analyst.
- **Bound the loop at the VCP boundary.** Rejected: under §2 a cut both patterns fly
  is honestly comparable across the change, and frames lacking the selected cut are
  already shown as lacking it. Bounding would hide retained history that is honest to
  display.

## Consequences

- The tilt selection is a `CutKey`. PageUp/PageDown walk distinct commanded angles, so
  they no longer visit a split cut's second half, and VEL at the lowest cut is reached
  from the default selection.
- Echo Tops and VIL now dedupe tilts by commanded cut with the §3 rule. At a split cut
  they use the surveillance REF, not the range-limited Doppler REF. This is an
  intended change in their values wherever the Doppler REF was previously
  contributing.
- Every `SweepGrid` and `DisplaySweep` carries `cut_deg`. Volume-derived grids carry
  `0.0`, as they do for `elevation_deg`.
- Keys are recorded as this project's own choice, not a GR2Analyst convention (none is
  recorded here): `Space`, `[` `]`, `End`, `,` `.`. Speed and dwell are config only
  (`timeline.frame_ms`, `timeline.dwell_ms`). `timeline.loop_frames` is written back.
- The mode is never persisted. Every launch starts `Live`.
