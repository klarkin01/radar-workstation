# Implementation Plan — Stage 6a Part C: Timeline

**Status:** Implemented 2026-11-22 — see §10 Results. Code, tests, documentation and
Phase 0 are done. The §6 windowed validation and the §11.3 VmHWM run are **not yet
performed** (they need an operator at a display).
**Date:** 2026-11-22
**Parent:** `docs/plans/stage-6a-time-handling.md` §3 Part C (C1–C6), with the
decisions `docs/plans/stage-6a-checkpoint.md` §5.2 requires (CK-20, CK-22 to CK-26).
This document is the executable form of Part C and nothing else.
**Precondition:** Parts A and B are on the branch you are working on (`git log` shows
`c83c049` and `343c44a`, or the merge of them). If they are not, stop and tell the
developer.
**Scope:** the operator-visible time axis over the ring ADR-0030 already retains: live
and paused modes, step, play/pause, loop length, an unmissable LIVE / NOT-LIVE state, and
the tilt-identity rule that keeps a loop honest across VCP changes and SAILS.
**Out of scope:** Part D (backfill, archive, jump-to-time). Its v1.0 scope is
checkpoint CK-21 and is not decided here. Nothing in this plan fetches data.

> **Do not commit.** The developer handles every `git add`, `git commit`, `git push`,
> branch and PR. Leave the work in the working tree. **No new dependencies.**

---

## 1. What "done" means

1. FR-TL-1 to FR-TL-10 are in `REQUIREMENTS.md`, ADR-0031 is written, and every one of
   them has a named test or a recorded validation outcome.
2. A loop never shows one angle under a selection meant for another, and never shows data
   a volume did not scan. A frame that lacks the selection says so and is skipped.
3. Playback uploads zero textures once the tail is resident. A stopped instance paces
   exactly as it does today.
4. `view_state_is_unchanged_by_any_sequence_of_state_updates` still passes, extended to
   cover the timeline.
5. `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo deny check` and `cargo audit` are clean.

---

## 2. Decisions

These are settled. Each is argued from `PHILOSOPHY.md` and GR2Analyst's behaviour. Where
GR2Analyst has no convention this repository can cite, that is stated, and the choice is
made on our principles instead.

**D-1 (CK-20). Playback steps volumes, not sweeps.** A loop position is one retained
frame (ADR-0030). A SAILS/MRLE re-scan is not a separate position: within a frame, the
selected tilt shows that volume's newest closure at that angle. This matches GR2Analyst's
volume-based loop. Per-cut SAILS animation would need an uneven time base and a second
frame concept, so it is a non-goal (§7).

**D-2 (CK-20). A tilt is identified by its commanded cut angle, not `elevation_number`.**
`elevation_number` means different angles in different VCPs. It also splits one tilt in
two: a split cut's surveillance scan (REF) and its Doppler scan (VEL) get different
numbers at the same angle. Measured angles drift up to ~0.08° from the commanded angle
(`nexrad-decoder` `VcpElevationCut` doc), and VCP 31's 2.5° sits 0.1° from VCP 35's 2.4°,
so no tolerance on the measured angle is safe. The Message 5 commanded angle is exact and
identical across patterns. Each sweep therefore carries
`cut_deg = Message 5 cut[elevation_number − 1].elevation_deg`. That value is used when
Message 5 is present and lies within 0.25° of the measured angle. Otherwise the sweep
falls back to its measured angle. Identity is `CutKey = round(cut_deg × 100)` (the 0.01°
bucket `compute::derived` already uses). A fallback sweep therefore almost never matches
a commanded one across frames, so it fails conservatively: the frame is marked as
lacking the tilt rather than showing a different angle. Measured `elevation_deg` stays
the input to all beam geometry.

**D-3. One grid-selection rule, used everywhere.** Among the grids at a `CutKey` that
carry the product, take the greatest `(range_extent_m, volume, elevation_number)`.
`range_extent_m = first_gate_m + gate_count × gate_width_m`. The rule picks the
full-range surveillance REF over a split cut's range-limited Doppler REF. It picks the
newest SAILS closure among equals. It picks the only VEL. The rule lives once, in
`compute::cut`. `compute::derived` (Echo Tops/VIL tilt dedupe), the live view, each frame
and GPU residency all use it.

**D-4. Live vs. frame resolution.** `Live` shows the live fold (`history::live_sweeps` /
`live_derived`), so it looks exactly like today. A paused or playing position shows
**only that frame's own sweeps**. Pausing from `Live` pins the frame that holds the
displayed sweep, so the image does not change on pause. A frame where D-3 finds nothing
shows no radar data. The status bar says "No VEL at 0.5° in the 21:04Z volume". Stepping
and playback skip that frame, and its scrubber tick is drawn hollow. Nothing is ever
substituted.

**D-5 (CK-22). Residency follows the focus frame.** The fully resident frame is the
pinned frame while `Paused`, and the newest frame while `Live` or `Playing`. Every other
frame keeps its D-3 selected grid in the tail, as ADR-0030 §3.7 already does. The grid
actually displayed is always included explicitly. So a product or tilt switch while
paused uploads nothing, and playback does not pull in whole frames.

**D-6 (CK-23). Keys.** No timeline keyboard convention for GR2Analyst is recorded in this
repository. Its loop controls are toolbar buttons. So the choice is made by
non-collision with the existing map and by mnemonic, and recorded as ours in `input.rs`:

| Key | Action |
|---|---|
| `Space` | Play / pause |
| `[` / `]` | Step back / forward one frame (stops playback; `]` past the newest frame → Live) |
| `End` | Jump to Live (pairs with `Home`, which resets the view) |
| `,` / `.` | Loop one frame shorter / longer (2 to 64) |
| Click / drag on scrubber | Pin a frame / scrub |

Speed and dwell get no keys: they are set-once preferences (Restraint), so they are config
only. `Shift+[`/`]` cannot be used: `action_for_key` matches the physical key first, so
both would also step.

**D-7. Playback semantics.** The loop window is the newest `loop_frames` retained frames.
Effective length is `min(loop_frames, retained)`. Play starts from the pinned frame if it
is in the window, otherwise from the oldest frame in the window. Each position holds
`frame_ms`, and the newest one holds `frame_ms + dwell_ms` before wrapping. A newly
arrived volume extends the window and does not interrupt the pass. If fewer than two frames
in the window satisfy D-3, Play does nothing and the status bar states the count. Steps
cover the whole ring and do not wrap. Playback wraps. Every launch starts `Live`, and mode
is never persisted.

**D-8. Evicted pinned frame.** The resolver does not mutate `ViewState`. It resolves a
`Paused(v)` whose `v` is gone to the oldest frame that satisfies D-3, and sets
`pinned_evicted`. The status bar shows "Pinned 20:12Z volume aged out; showing oldest
retained" until the next timeline action. The display stays paused.

**D-9 (CK-25). One age readout, plus the indicator.** The status bar's `age` always
measures the **live feed**: the Live resolution's sweep, with the existing two-VCP-cycle
alert rule. In Live this is exactly today's value. When not Live it is labelled
`live age`. The scan-time field always shows the **displayed** volume's time. A timeline
badge reads `LIVE`, or `PAUSED` / `LOOP` with the displayed volume's full UTC time and its
offset behind live (`−18 min`). It uses a dedicated colour that is distinct from the
stale alert. Being in the past on purpose never fires the alert.

**D-10 (CK-26). Placefile hook.** `render::timeline::time_context(&Resolved) ->
TimeContext { Live, At(unix_secs) }` is the single pure accessor a future layer 7 reads.
Nothing placefile-specific is built.

**D-11. Pacing.** While `Playing`, `about_to_wait` sets `WaitUntil(next_step)`, which is
one redraw per frame step rather than continuous redraw. When not playing, today's
pacing is unchanged. The playback clock is the render loop's, not a task's.

**D-12. ViewState rule, restated.** `ViewState` changes on operator input, and also on
the playback clock while `Playing`. It never changes on a state update. The guard test
is extended accordingly.

**D-13. Config keys** (FR-CP-1, ADR-0019 conventions):

| Key | Range | Default | Written back |
|---|---|---|---|
| `timeline.loop_frames` | 2–64 | `12` | yes (changed by `,` `.`) |
| `timeline.frame_ms` | 50–2000 | `250` | no |
| `timeline.dwell_ms` | 0–10000 | `1500` | no |

**D-14. A new ADR-0031** records D-1 to D-5 and D-9. Rejected alternatives:
`elevation_number`, measured-angle tolerance, per-sweep playback, and bounding the loop
at the VCP boundary. That last one would hide retained history that is honest to display
under D-2.

---

## 3. Phases and steps

### Phase 0 — Confirm the data facts D-2 and D-3 rest on

0.1 Add an `#[ignore]`d live test in `assembly` that assembles one live KDOX volume and asserts, per sweep, that `cut[elevation_number − 1]` exists and is within 0.25° of the measured angle.
0.2 In the same test, assert that at every split-cut angle the surveillance REF's range extent is ≥ the Doppler REF's.
0.3 Run it against KDOX and against any site currently in VCP 212/215 (SAILS). Record the per-sweep table in §10. A failure means the D-2 fallback covers it; it does not reopen the decision.

### Phase 1 — Requirements and ADR

1.1 Add FR-TL-1 to FR-TL-10 (§5) as `REQUIREMENTS.md` §2.11 "Timeline", and point FR-DA-10's last sentence at it.
1.2 Add "Timeline over the retained live history (FR-TL-*)" to §6 In Scope.
1.3 Write `docs/adr/0031-timeline-semantics.md` from D-1 to D-5, D-9 and D-14, and add it to `docs/README.md`.

### Phase 2 — Cut identity, library side

2.1 Add `assembly::cut_angle(vcp: Option<&VcpDefinition>, elevation_number, measured) -> f32` (pure, D-2 rule), with unit tests for present, absent, out-of-range-index and >0.25° cases.
2.2 Carry `cut_deg` on `AssemblyEvent::SweepClosed`, `StateUpdate::SweepGridded`, `SweepGrid` and `DisplaySweep`.
2.3 Add `compute/cut.rs`: `CutKey`, `cut_key(f32)`, `range_extent_m(&SweepGrid)`, and `best_at_cut(iter of (VolumeId, &Arc<SweepGrid>), product, CutKey)` (D-3).
2.4 Replace `derived::distinct_tilts_ascending`'s bucketing with `compute::cut`. Update `repeated_sails_cuts_contribute_one_tilt_not_two` and add a split-cut test (the surveillance REF wins).
2.5 Add `Frame::grid_at_cut(product, CutKey)` and a fold-side `history::live_grid_at_cut`, both thin wrappers over `best_at_cut`.

### Phase 3 — `render/timeline.rs` (pure, no call sites yet)

3.1 Define `Mode { Live, Paused(VolumeId), Playing { at, next_step } }` and `Timeline { mode, loop_frames, frame_interval, dwell }`, all `Copy`.
3.2 `resolve(&Timeline, product, Option<CutKey>, &StateSnapshot) -> Resolved`: the displayed grid and sweep, a per-frame position list (`satisfies`, `in_loop`), and `pinned_evicted` (D-4, D-8).
3.3 Add `step`, `toggle_play`, `jump_to_live`, `adjust_loop` and `tick(&mut Timeline, &Resolved, now) -> Option<Instant>`. They take a `Resolved`, never a snapshot (D-7, D-11, D-12).
3.4 Add `time_context(&Resolved) -> TimeContext` (D-10).
3.5 Unit-test every row of §4's timeline block before wiring anything.

### Phase 4 — Rewire `ViewState` and selection

4.1 Replace `ViewState::elevation_number` with `cut: Option<CutKey>` and add `timeline: Timeline`. `initial` takes the timeline prefs.
4.2 Make `step_elevation` walk the distinct `CutKey`s of the current resolution's source, ascending. PageUp no longer visits a split cut's second half.
4.3 Make `resolve_selection`, `displayed_scan` and the cursor readout all read one `Resolved` per frame. Keep the first-sweep default (lowest cut) as today.
4.4 Extend `view_state_is_unchanged_by_any_sequence_of_state_updates` to `Live`, `Paused` and `Playing` (no tick), and fix the tests that set `elevation_number`.

### Phase 5 — Residency (CK-22)

5.1 Change `radar::residency` to take `focus: Option<VolumeId>` (None → newest), `CutKey`, and the displayed grid. The tail uses `Frame::grid_at_cut` (D-5).
5.2 Key `last_uploaded_selection` on `(revision, product, cut, focus)`.
5.3 Add tests: a product/tilt switch while paused on an older frame uploads nothing; walking playback across the window uploads nothing once resident; focus changes never evict the newest frame's selected grid.

### Phase 6 — Actions and keys

6.1 Add `PlayPause`, `StepBackward`, `StepForward`, `JumpToLive`, `LoopShorter`, `LoopLonger` to `Action`, bound per D-6, with `input.rs` tests. Note the non-GR provenance in the module doc.
6.2 Route each through `apply_action` to the Phase 3 functions, using this frame's `Resolved`.
6.3 Add the rows to `help_overlay` and `QUICK-START.md`'s hotkey table. Nothing else holds a copy.

### Phase 7 — Playback pacing

7.1 Call `timeline::tick` once per `redraw`. When it returns a deadline, `about_to_wait` uses `WaitUntil(min(deadline, existing))` (D-11).
7.2 Add a test that a non-playing timeline yields no deadline, so idle pacing is provably unchanged.

### Phase 8 — Timeline UI

8.1 Add a scrubber row above the status bar, shown only when ≥ 2 frames are retained. It has one tick per frame, hollow when the frame fails D-3, a shaded loop window and a marked selection.
8.2 A click on a tick sets `Paused(v)` and a drag scrubs. The egui response is consumed so neither pans the map.
8.3 Add the mode badge to the status bar (D-9), with one new colour constant in `ui.rs`.
8.4 Relabel age as `live age` when not Live and source it from the Live resolution. Add the absent-at-frame, evicted and "loop needs ≥ 2" messages (D-4, D-7, D-8).
8.5 Add `ui.rs` tests: live age ignores the pinned frame's age; the alert never fires on a paused fresh feed; the badge text in each mode.

### Phase 9 — Config

9.1 Add the three D-13 keys to `config/mod.rs` with `parse_in_range`, tests matching the `history.*` tests, and `loop_frames` write-back in `save.rs`.
9.2 Thread the prefs into `ViewState::initial`. Headless ignores them.

### Phase 10 — Documentation sweep

10.1 `QUICK-START.md`: keys (6.3), status-bar badge and `live age`, the three config rows, the PageUp-by-distinct-angle note, and delete "No timeline yet".
10.2 Update the `rendering.md` residency section for the focus rule. `README.md` status: Part C complete.
10.3 Parent plan: a Part C "Done" note mirroring A and B, and answer §5's remaining playback questions with pointers to ADR-0031.
10.4 Checkpoint: mark §5.2 resolved by this plan. Update this plan's status line and the `docs/README.md` row (checkpoint §3.2 convention).
10.5 `CLAUDE.md`: add only the ADR-0031 index line and the `timeline.rs` module-map line.

### Phase 11 — Validation and results

11.1 Run `cargo test --workspace`, clippy, `cargo deny check` and `cargo audit`, plus `--ignored` GPU and live tests (allow ~20 min).
11.2 Windowed KDOX for at least 4 volumes: walk through §6's checklist and record each outcome in §10.
11.3 Record VmHWM with defaults while playing for 10 min, against the < 520 MB target.

---

## 4. Test matrix

| Area | Cases |
|---|---|
| `cut_angle` | Message 5 present / absent; index past cut list; commanded vs measured > 0.25° → measured |
| `best_at_cut` | split cut → surveillance REF, Doppler VEL; SAILS → newest closure; 2.4° never matches 2.5°; fallback sweep doesn't match commanded |
| `resolve` | Live = live fold byte-for-byte; paused shows own sweeps only; frame lacking cut → no grid + hollow; pinned evicted → oldest satisfying + flag; derived product per frame |
| `step` / `play` / `tick` | `]` past newest → Live; `[` at oldest stays; pause from Live pins displayed sweep's frame; play from Live starts at window's oldest; dwell on last; wrap; new frame extends without jump; < 2 satisfying → no-op; step stops play |
| VCP change | loop across 35 → 212 at 0.5° plays both sides; at 8.0° the VCP 35 frames are hollow and skipped |
| Residency | §5.3's three tests |
| ViewState guard | §4.4 |
| Keys / config | §6.1, §9.1 |

---

## 5. Requirements text (Phase 1.1)

- **FR-TL-1** Modes: Live (default at every launch, never persisted), Paused on a frame, Playing.
- **FR-TL-2** A non-live position shows only data its own volume scanned. Nothing is carried from another volume.
- **FR-TL-3** The tilt selection is a commanded elevation angle. A frame without it shows no radar data, says so, and is skipped by step and play.
- **FR-TL-4** Step back and forward one frame. Forward past the newest frame returns to Live, and one key returns to Live.
- **FR-TL-5** Play/pause loops the newest N retained frames. N, frame interval and end dwell are configurable, and N is adjustable from the keyboard.
- **FR-TL-6** The mode is always visible. When not Live, the displayed volume's full UTC time and its offset behind live are shown.
- **FR-TL-7** Live-feed age stays visible in every mode, and its stale alert depends only on the live feed.
- **FR-TL-8** If the paused frame is evicted, the display moves to the oldest retained frame and says so.
- **FR-TL-9** Playback performs no texture uploads once resident. A non-playing instance adds no redraws.
- **FR-TL-10** Product and tilt switches while paused upload nothing for the paused frame.

---

## 6. Validation checklist (Phase 11.2)

- Live display is unchanged from Stage 5 / Part B, including carry-forward during early sweeps.
- `Space` loops. The badge reads `LOOP` with the time and offset, `live age` stays green, and the end dwell is visible.
- `[` `]` `End` `,` `.` behave per D-6/D-7. The scrubber click pins and the drag scrubs without panning.
- PageUp from 0.5° goes to the next distinct angle, and VEL at 0.5° is present (the split-cut fix).
- With `history.budget_mb = 0` there is no scrubber, `Space` reports the count, and nothing else changes.
- The CPU stays idle while paused (same as today), and frame steps are smooth while playing.

---

## 7. Non-goals

- Per-cut SAILS animation (D-1), per-sweep timestamps (the readout is volume start time,
  as today), and speed/dwell keys.
- Velocity dealiasing, and any change to ADR-0030 retention or its defaults.
- Any Part D work and any placefile code beyond `time_context`.

## 8. Risks

- **Message 5 indexing differs under SAILS** (Phase 0). Contained by the D-2 fallback:
  the frame shows as missing, never wrong.
- **Choosing a grid by range changes Echo Tops/VIL** where the split-cut Doppler REF was
  being used. This is intended. Call it out in §10 with before/after values on one
  volume.
- **egui scrubber input vs. map drag.** Covered by 8.2 and checked in validation.

## 9. Commit policy

Do not commit, push or branch. Leave everything in the working tree for the developer.

## 10. Results

Filled in by the implementing session, 2026-11-22. Nothing was committed. The working
tree holds the change.

### 10.1 Phase 0: the data facts D-2 and D-3 rest on

`tests/assembly_live.rs::cut_identity_facts_hold_on_a_live_volume` (`#[ignore]`d,
`RADAR_LIVE_SITE` picks the site). It ran live against KATX (VCP 215) twice and KDOX
(VCP 35) once. The same table was produced offline, with a temporary harness that was
deleted afterwards, for the three local volumes in `downloads/` (not committed): KDOX
2026-06-29 18:11Z (VCP 35), KFWS 2026-09-03 03:20Z (VCP 12) and KHGX 2026-09-05 01:19Z
(VCP 212). **Both facts held on all six volumes.** The second KATX run after the median
change below:

| el | first radial | median | commanded | cut | REF range | VEL |
|---|---|---|---|---|---|---|
| 1 | 0.2664 | 0.4834 | 0.4834 | 0.4834 | 460125 m | no |
| 2 | 0.4834 | 0.4834 | 0.4834 | 0.4834 | 300125 m | yes |
| 3 | 0.7910 | 0.8789 | 0.8789 | 0.8789 | 460125 m | no |
| 4 | 0.8789 | 0.8789 | 0.8789 | 0.8789 | 300125 m | yes |
| 5 | 1.1920 | 1.2744 | 1.2744 | 1.2744 | 430125 m | no |
| 6 | 1.2744 | 1.2744 | 1.2744 | 1.2744 | 300125 m | yes |
| 7 | 1.8265 | 1.8018 | 1.8018 | 1.8018 | 364125 m | yes |
| 8 | 2.2275 | 2.3730 | 2.4170 | 2.4170 | 336125 m | yes |
| 9 | 2.9333 | 3.0322 | 3.0762 | 3.0762 | 297125 m | yes |
| 10 | 3.8177 | 3.9551 | 3.9990 | 3.9990 | 250125 m | yes |
| 11 | 4.9796 | 5.0537 | 5.0977 | 5.0977 | 209125 m | yes |
| 12 | 6.2732 | 6.3721 | 6.4160 | 6.4160 | 173125 m | yes |

- **Fact 1 (Message 5 index and tolerance) holds, but only against the median radial
  angle.** `Sweep::elevation_deg` is the *first* radial's angle, and on surveillance cuts
  the first radials are still settling. The first-radial gap to the commanded angle was
  up to **0.217°** (KATX el 1, above), 0.198° (live KDOX el 1: 0.1099 vs 0.3076), 0.200°
  (KFWS el 7: 1.6013 vs 1.8018) and 0.19° (KFWS el 1). That is not the ~0.08° the decoder
  docs record, and it passes the 0.25° tolerance with as little as 0.033° to spare. The
  **median** radial angle is never more than 0.044° from commanded on any of the six
  volumes. Doppler halves, which start with the antenna already settled, read the
  commanded angle exactly. So `assembly::cut_angle` is fed the sweep's median radial
  elevation (`assembly::median_elevation_deg`, see §10.5 item 1). Without that, a
  slightly worse settle would have pushed the lowest tilt onto the fallback and made it
  hollow in every loop.
- **Fact 2 (split cuts) holds everywhere:** the surveillance REF reaches 460 km (430 km
  at the third split cut) against the Doppler REF's 300 km, on every split cut of every
  volume.
- **SAILS / MRLE.** No live VCP 212/215 volume on 2026-11-22 carried inserted cuts (the
  KATX VCP 215 volume had 12 sweeps, none repeated). The local KDOX VCP 35 volume does:
  elevations 7/8 repeat 1/2 at 0.3076°. D-3 resolves it as designed: equal 460 km range,
  so el 7 (the newer closure) wins for REF and el 8 for VEL. This is covered by
  `compute::cut::tests::sails_re_closures_select_the_newest`.
- Commanded lowest cuts differ by site: KDOX flies 0.3076° and the others 0.4834°. This
  is harmless, since identity only has to match within one site's ring.

### 10.2 Tests and tooling

| Check | Result |
|---|---|
| `cargo test --workspace` | all pass: library 219 → 244, binary 82 → 120, other crates unchanged |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo deny check` | **advisories FAILED**, bans/licenses/sources ok. Pre-existing: `Cargo.lock` is unchanged by this work (checkpoint A1 open item) |
| `cargo audit` | **7 vulnerabilities, 2 unsound warnings**, all pre-existing: rustls (RUSTSEC-2026-0285, -0346, -0348), rustls-webpki (-0382), webpki-roots (-0592), plus rustix and tokio unsoundness (-0381, -0386, -0486) |
| `--ignored` (GPU + live, `--no-fail-fast`) | all pass (http-ingest live S3 6, `s3_poll` live 7, GPU offscreen 6, `assembly_live` 2, `pipeline_live` 2), except `nexrad-sample`'s `download_sample_writes_a_file_for_a_successful_response`, which hits a stale hard-coded key (§10.6). It is pre-existing and unrelated |

Named tests by requirement: FR-TL-1/4/5/8 `render::timeline::tests` (step, play, tick,
eviction, mode); FR-TL-2/3 `a_paused_frame_shows_only_its_own_sweeps`,
`a_cut_only_the_new_pattern_flies_is_hollow_in_the_old_patterns_frames`; FR-TL-6/7
`render::ui::tests::{badge_text_in_each_mode, live_age_ignores_the_pinned_frames_age,
the_stale_alert_never_fires_on_a_paused_fresh_feed}`; FR-TL-9
`render::radar::tests::walking_playback_across_the_window_uploads_nothing_once_resident`,
`render::tests::a_stopped_timeline_wakes_exactly_as_before`,
`timeline::tests::a_stopped_timeline_yields_no_deadline`; FR-TL-10
`a_product_or_tilt_switch_while_paused_on_an_older_frame_uploads_nothing_for_it`. The
guard `view_state_is_unchanged_by_any_sequence_of_state_updates` now runs in Live,
Paused and Playing.

### 10.3 Echo Tops / VIL before and after (§8 risk)

Before means the old dedupe (bucket by the first-radial angle, later closure wins).
After means D-3 over commanded cuts. Both came from the same gridded REF of one volume.
"Filled" counts cells with data.

| Volume | Tilts before → after | Echo Tops filled | VIL filled | Notes |
|---|---|---|---|---|
| KDOX VCP 35 | 15 → 10 | 14 454 → 6 346 | 220 907 → 196 585 | mean ET over shared cells 2.34 → 2.30 kft; max ET 60.6 both; max \|Δ\| 7.2 kft |
| KHGX VCP 212 | 14 → 11 | 97 400 → 74 810 | 275 026 → 249 168 | mean ET over shared cells 4.47 → 4.27 kft; max VIL 13.6 → 14.2 kg/m²; max \|ΔET\| 13.0 kft |
| KFWS VCP 12 | 12 → 9 | 127 463 → 120 074 | 451 939 → 437 433 | **geometry changed: 300 km → 460 km** (below) |

- "Before" counted a split cut's Doppler REF and a SAILS re-scan as extra tilts, because
  their first radials read different angles. ET/VIL then OR-ed two noisy samples of the
  same angle, which inflates the fill near the 18 dBZ threshold. "After" has exactly one
  REF per commanded angle, the surveillance one at split cuts.
- **KFWS shows a real defect in the old rule, now fixed.** There the Doppler half's first
  radial (0.4504°) read *lower* than the surveillance half's (0.6729°). The old "lowest
  tilt" was therefore the range-limited Doppler REF, and Echo Tops/VIL were built on its
  300 km geometry, dropping 300–460 km. After, both are on the surveillance REF's 460 km
  geometry.

### 10.4 §6 validation checklist and §11.3 memory: not performed

These need an operator watching a window for at least four live volumes. This session
did not do that. Every item below is open, for the developer:

- Live display unchanged from Stage 5 / Part B, including carry-forward: **not checked
  on screen.** It is pure-tested (`live_resolves_to_the_live_fold`).
- `Space` loop, `LOOP` badge with time and offset, `live age` green, visible end dwell:
  **not checked on screen.**
- `[` `]` `End` `,` `.`, scrubber click pins / drag scrubs without panning: **not
  checked on screen.** The scrubber senses click and drag but is deliberately not
  focusable (§10.5 item 9).
- PageUp from 0.5° to the next distinct angle, VEL present at 0.5°: pure-tested
  (`page_up_walks_distinct_cuts_…`), **not checked on screen.**
- `history.budget_mb = 0`: no scrubber (it needs ≥ 2 positions) and `Space` reports the
  count. **Not checked on screen.**
- CPU idle while paused, smooth steps while playing: pacing is pure-tested
  (`a_stopped_timeline_…`). **Not measured.**
- VmHWM with defaults while playing for 10 min against < 520 MB: **not measured.**

### 10.5 Where the implementation departs from, or settles, the plan text

1. **D-2's "measured angle" is the sweep's median radial elevation**, not
   `Sweep::elevation_deg` (the first radial). This follows from §10.1.
   `Sweep::elevation_deg` itself is unchanged and is still the geometry input.
2. **The first-sweep default is resolved, not stored.** `ViewState.cut` is `None` until
   the first PageUp/PageDown, and `timeline::resolve` reads `None` as the lowest live
   cut. This keeps D-12 strict (no state update ever writes `ViewState`). Because the
   default comes from the live fold, pausing never changes which cut is shown.
3. **The upload key also includes the displayed grid's `GridKey`:**
   `(revision, product, cut, focus, displayed)`. While playing, the displayed grid moves
   each step with `focus` unchanged. Re-planning then costs a `plan_sync` that uploads
   nothing, but it guarantees a tail grid the budget dropped is still uploaded.
4. **`plan_sync` uploads in `residency`'s priority order:** displayed grid, focus frame,
   the newest frame's selected grid, then the tail. This replaces "newest volume first",
   which would make a paused older frame wait.
5. **`timeline.loop_frames` write-back is in `main.rs::run_render`**, beside every other
   write-back. `config/save.rs` is a generic line-preserving writer and needed no change.
6. **`render::run` takes an `InitialView` struct.** One more argument would have passed
   clippy's argument limit.
7. **`Space` from Live plays** (D-7: from the window's oldest). "Pause from Live" (D-4)
   is `timeline::pause`. It is reached by `Space` while playing and by a step that
   stops playback, and it is unit-tested from Live directly. No extra key was added.
8. **Playing when fewer than two frames remain playable** (the selection changed
   mid-loop) pauses in place. `pinned_evicted` is reported only for `Paused`. A playing
   position that aged out falls back silently, and the next tick moves on.
9. **The scrubber is not focusable.** `Sense::click_and_drag()` includes `FOCUSABLE`,
   and a focused egui widget makes egui-winit consume every key press. One click would
   have silently disabled the keyboard map. It senses `CLICK | DRAG` only.
10. **The status bar's tilt field** shows the commanded cut and the measured angle of
    the drawn grid (`0.5° (0.48° meas)`), and `volume` for Echo Tops/VIL. The
    absent-at-frame message names the product in capitals (`No VEL at 0.5° in the
    21:04Z volume`).
11. **`DisplayProduct::is_derived()`** replaces the four scattered
    `matches!(EchoTops | Vil)` checks. The fold functions (`live_sweeps`,
    `live_derived`, the new `live_derived_source`) accept the ring's `VecDeque` or a
    snapshot's `Vec`. `render/test_support.rs` is the one set of frame and snapshot
    builders for the render-side tests.

### 10.6 Found along the way, outside this plan's scope

- **`Sweep::elevation_deg` (the first radial) feeds all beam geometry.** That includes
  the shader's slant correction, Echo Tops heights and the cursor's beam height, and it
  is up to 0.22° off on surveillance cuts (§10.1). At 200 km that is ~0.75 km
  (~2.5 kft) of Echo Top height. Using the median, as `cut_angle` now does, is a
  one-line change in `SweepBuilder::close`, but it changes geometry everywhere, so it
  should be its own decision.
- `utility/nexrad-sample/tests/data_acquisition.rs::download_sample_writes_a_file_for_a_successful_response`
  (`#[ignore]`d) fails with 404: it hard-codes a 2026-07-27 chunk key, long past the
  bucket's ~48 h retention.
- `CLAUDE.md`'s one-line status still says there is no timeline. Step 10.5 of this plan
  limited `CLAUDE.md` edits to the ADR-index and module-map lines.
