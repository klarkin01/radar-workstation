# Implementation Plan — Stage 6a Part A: Stop displaying stale data

**Status:** proposed, not started
**Date:** 2026-09-03
**Parent:** `docs/plans/stage-6a-time-handling.md` §1 (findings) and §3 Part A
(A1–A5). This document is the executable form of Part A and nothing else.
**Scope:** the poller anchors to the wrong volume across the chunk bucket's
sequence-counter wrap, cannot recover from it, and the status bar reports the
poll clock as if it were the data clock. Fixing all three.
**Out of scope:** Parts B, C and D of the parent plan (history retention,
timeline controls, archive reads). Nothing here retains a second frame.

> **Do not commit.** This plan is executed by an implementation session; the
> developer handles every `git add`, `git commit`, `git push`, branch and PR.
> Leave the work in the working tree. This is repeated at §8.

---

## 1. What "done" means

1. A cold start at any point in the ~48 h after a sequence wrap anchors to the
   **live** volume, not to a pre-wrap volume — verified by a unit test over the
   measured KDOX listing from the parent plan §1.1.
2. A running poller that reaches the end of the sequence advances `999 → 1`
   rather than to a directory that can never exist. The deadlock in parent §1.3
   is structurally impossible: `1000` is unrepresentable.
3. Re-anchoring is expressed as "never backwards **in time**" and therefore
   still refuses a stale listing while permitting a numerically backwards move
   across the wrap.
4. The status bar's primary freshness number is **the displayed scan's own age**
   in wall-clock UTC, coloured as an alert past two nominal VCP cycles. The poll
   clock stays, relabelled as poll health.
5. Every doc and doc comment asserting "monotonically increasing" or a 24-hour
   retention contract is corrected against the measurement.
6. `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D
   warnings` are clean. **No new dependencies** (CLAUDE.md).

---

## 2. Decisions taken in this plan

### 2.1 (A-a) The cycle gets a type, and that type deliberately has no `Ord`

The bug is a `max()` over a cyclic sequence. The repair is not a smarter `max()`
call at that one site — it is making the wrong call impossible to write. A new
`ingest::volume_seq` module introduces `VolumeSeq`, a newtype over `1..=999`
with a checked constructor, `succ`, `pred`, and `Display`. It derives
`Debug, Clone, Copy, PartialEq, Eq, Hash` and **does not** derive or implement
`PartialOrd`/`Ord`: two sequence numbers have no ordering without a reference
point, and the original defect was precisely the belief that they do.

### 2.2 (A-b) Ordering comes from the retained arc, found by its largest gap

Ordering is supplied by `VolumeWindow`, built from one listing. The retained set
always occupies a contiguous arc of the cycle with holes in it, so:

> the newest volume is the element immediately **before** the largest cyclic gap;
> the oldest is the element immediately **after** it.

Against the parent plan's measured KDOX listing — runs `[(1,199), (659,659),
(749,999)]`, 451 prefixes — the cyclic gaps are `199→659 = 460`, `659→749 = 90`,
`999→1 = 1`, so the arc runs `659 … 999, 1 … 199`: newest **199** (live), oldest
**659**. The stray `659` singleton is absorbed as an old leftover rather than
mistaken for the newest, and no `LastModified` tiebreak is needed. Parent A2
left that tiebreak open pending a test; this rule closes it — decided by the
fixture, not by inspection.

The rule degenerates correctly: an unwrapped single run yields exactly the
numeric max (so the existing cold-start behaviour is preserved verbatim), and a
one-element listing yields that element.

**Why the arc is always gapped**, i.e. why this is sound rather than lucky: the
cycle is 999 volumes; retention is ~48 h. At the *fastest* VCP (12, ~4.2 min per
volume) 48 h is ~686 volumes, leaving a boundary gap of ~313. The largest gap
between *skipped* sequence numbers observed live is 73 (`92→165`). Those are an
order of magnitude apart. `MIN_TRUSTED_BOUNDARY_GAP = 180` sits between them; a
listing whose largest gap falls below it is still resolved deterministically but
is **reported** as an event, because the assumption above has stopped holding
and the operator should learn that from the application rather than from a
wrong picture.

### 2.3 (A-c) The poller tracks the target, not the baseline

`last_completed_volume: Option<u64>` currently serves double duty: it is the
cold-start anchor *and* the "last fully delivered" number published in
`IngestStatus`, which is why `poll_once` has to re-assign it on every branch and
why `± 1` appears at five separate sites. Splitting it into

```rust
target: Option<VolumeSeq>,                 // the directory being drained
last_completed_volume: Option<VolumeSeq>,  // last volume whose -E was seen
```

removes every `± 1` except the two inside `succ`/`pred`, deletes the
`last_completed_volume = Some(baseline)` re-assignments outright, and makes the
published number honest (today a re-anchor publishes `new_target - 1` as
"fully delivered" when that volume was never delivered at all; the cold start
publishes the `0` sentinel).

### 2.4 (A-d) Recovery *policy* does not change — only its arithmetic

`REANCHOR_EMPTY_POLLS`, `STUCK_MID_VOLUME_EMPTY_POLLS`, the
listing-only-when-due throttle, and the choice to re-anchor to `pred(newest)`
rather than `newest` (so recovery lands on a complete volume, where the cold
start deliberately accepts a partial one for latency) are all preserved exactly.
This is a bug-fix plan; the only semantic changes are those §2.3 names.

### 2.5 (A-e) `render/time.rs` moves library-side to `src/time.rs`

The data-age readout needs NEXRAD-time → Unix-seconds, and the live regression
test (W6) needs the same conversion from the *library* crate, where
`render/` — binary-side by ADR-0022 — is not reachable. The module is pure
calendar arithmetic with no render dependency; it becomes `radar_workstation::time`
and gains `days_from_civil`, `unix_secs_from_civil` and `unix_secs_from_nexrad`.
One import in `render/ui.rs` changes. Nothing else uses it.

### 2.6 (A-f) VCP cadence is operational knowledge and gets its own module

"Two VCP cycles" needs a nominal volume duration per VCP, which is not in the
decoded bytes and therefore does not belong in `nexrad-decoder::types::vcp`. A
new `radar_workstation::vcp` module holds one table and one function. Unknown
VCPs fall back to the longest defined cycle (10 min), so an unrecognised pattern
under-warns rather than cries wolf.

### 2.7 (A-g) Errata, not a new ADR

Part A contradicts a *format finding* (the counter's behaviour), not a decision.
ADR-0011 and ADR-0014 get errata blocks in the style of ADR-0005 and ADR-0018.
The retention ADR the parent plan calls for belongs to Part B.

---

## 3. Work items

Sequential. Each is expected to leave `cargo test --workspace` green before the
next begins. Mapping to the parent plan: **W1+W2 = A2+A3**, **W3+W4+W5 = A4**,
**W6 = A1+A5**.

### W1 — `ingest::volume_seq`: the cyclic type (pure, no call sites yet)

New file `crates/radar-workstation/src/ingest/volume_seq.rs`; add
`pub mod volume_seq;` to `crates/radar-workstation/src/ingest/mod.rs`.

Module doc comment carries the measurement: the 2026-09-04T00:55Z KDOX listing,
its three runs, the observed `999 → 1` roll at ~2026-09-03T03:13Z, and the
statement that the bucket's numbering is *observed behaviour, not a documented
contract*.

```rust
/// A chunk-bucket volume-sequence number: a cyclic counter over 1..=999,
/// **not** a monotonically increasing integer (see the module doc).
/// Deliberately no `Ord`/`PartialOrd` — see `VolumeWindow`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VolumeSeq(u16);
```

API:

| Item | Behaviour |
|---|---|
| `const MIN: VolumeSeq` / `const MAX: VolumeSeq` | `1` / `999` |
| `const CYCLE: u32` | `999`, private |
| `VolumeSeq::new(n: u64) -> Option<Self>` | `None` outside `1..=999` |
| `fn get(self) -> u16` | the raw number |
| `fn succ(self) -> Self` / `fn pred(self) -> Self` | wrap `999↔1` |
| `impl Display` | the bare integer, so existing log text is unchanged |

```rust
/// The retained arc of the cycle, recovered from one listing: everything
/// between `oldest` and `newest` going forward, wrap included. The arc's
/// ends are found as the two sides of the largest cyclic gap (plan §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeWindow { oldest: VolumeSeq, newest: VolumeSeq, largest_gap: u32 }
```

| Item | Behaviour |
|---|---|
| `pub const MIN_TRUSTED_BOUNDARY_GAP: u32 = 180` | justified in its doc comment by the 73-vs-313 numbers in §2.2 |
| `VolumeWindow::from_listing(&[VolumeSeq]) -> Option<Self>` | sorts and dedups a copy; `None` for an empty slice; a one-element slice yields `oldest == newest` and `largest_gap == CYCLE` |
| `fn newest(&self) -> VolumeSeq` / `fn oldest(&self)` | |
| `fn largest_gap(&self) -> u32` | so the caller can report ambiguity without the pure function doing I/O |
| `fn position(&self, v: VolumeSeq) -> u32` | `(v - oldest) mod CYCLE`; total — defined for values outside the listing, including the not-yet-created next volume |
| `fn is_after(&self, a: VolumeSeq, b: VolumeSeq) -> bool` | `position(a) > position(b)` |

Tests (same module, `#[cfg(test)]`). Build the measured listing once, from the
runs, so the fixture *is* the measurement:

```rust
fn measured_kdox_wrap() -> Vec<VolumeSeq> {
    (1..=199).chain(std::iter::once(659)).chain(749..=999)
        .map(|n| VolumeSeq::new(n).unwrap()).collect()
}
```

- `successor_of_the_last_sequence_number_is_the_first`
- `predecessor_of_the_first_sequence_number_is_the_last`
- `out_of_range_sequence_numbers_are_rejected` (`0`, `1000`, `u64::MAX`)
- `the_measured_listing_has_the_prefix_count_that_was_observed` — asserts `451`,
  which is what makes the fixture a transcription of the measurement rather
  than an invention
- `window_over_the_measured_kdox_wrap_picks_the_post_wrap_volume` — `newest == 199`,
  `oldest == 659`
- `window_over_a_single_unwrapped_run_agrees_with_numeric_max` — `100..=500`
- `window_of_one_folder_is_that_folder`
- `window_of_an_empty_listing_is_none`
- `skipped_sequence_numbers_inside_the_arc_do_not_split_it` — the live-observed
  `92→165` gap of 73 must not become the boundary
- `order_is_by_position_in_the_arc_not_by_integer` — over the measured fixture,
  `is_after(1, 999)` is true and `is_after(999, 1)` is false
- `a_volume_that_does_not_exist_yet_is_after_the_newest_retained_one` —
  `is_after(newest.succ(), newest)`
- `a_listing_with_no_trusted_boundary_gap_is_still_resolved_deterministically` —
  a near-full cycle; `largest_gap() < MIN_TRUSTED_BOUNDARY_GAP` and
  `from_listing` still returns `Some`

### W2 — Rewire `S3Poller` onto the cycle

File: `crates/radar-workstation/src/ingest/s3_poll.rs`.

1. **Fields** (§2.3): replace `last_completed_volume: Option<u64>` with the two
   `Option<VolumeSeq>` fields; update `S3Poller::new`. Update the `S3Poller`
   struct doc comment: strike "monotonically increasing", state the cyclic
   1–999 behaviour, and point at `volume_seq`'s module doc for the evidence
   (the wrong assumption was written here first and then coded to — parent A1).
2. **`parse_volume_folder`** returns `Option<VolumeSeq>` by parsing to `u64` and
   passing through `VolumeSeq::new`, so an out-of-range directory flows into the
   existing `Event::UnrecognizedVolumeFolder` report instead of silently
   corrupting the ordering. `list_volume_folders` returns `Vec<VolumeSeq>`.
3. **`cold_start_baseline` → `cold_start_target`**:
   ```rust
   /// The volume directory a cold start should begin draining: the newest
   /// retained volume (partial, and therefore the lowest-latency choice —
   /// see plan §2.4), or `VolumeSeq::MIN` when the site has no data at all.
   fn cold_start_target(folders: &[VolumeSeq]) -> VolumeSeq {
       VolumeWindow::from_listing(folders).map_or(VolumeSeq::MIN, |w| w.newest())
   }
   ```
4. **`poll_once`**: resolve `self.target` (listing on the cold path only, then
   store it), drop the `baseline + 1` computation and both
   `last_completed_volume = Some(baseline)` re-assignments. On `saw_end`:
   `self.last_completed_volume = Some(target); self.target = Some(target.succ());`
   plus the existing `last_seen_key`/`seen_any_key_in_current_volume` resets.
   Publish `current_volume` from `last_completed_volume`.
   Update the `(E-10)` comment: it says "the full 24-hour retention window".
5. **Ambiguity report**: wherever a fresh listing is taken (`poll_once` cold
   path and `apply_recovery`), if the resulting window's `largest_gap()` is
   below `MIN_TRUSTED_BOUNDARY_GAP`, report a new event (W2.7) once per listing.
   Do this in the poller, not inside the pure function.
6. **`next_target`**: `PollState.current_target` becomes `VolumeSeq`. The
   re-anchor branch becomes
   ```rust
   if let Some(folders) = listing {
       if let Some(window) = VolumeWindow::from_listing(folders) {
           // Recovery lands on a *complete* volume, one behind the newest —
           // unlike the cold start, which takes the partial newest for
           // latency (plan §2.4). Never re-anchor backwards *in time*:
           // across a wrap the correct move is backwards numerically, which
           // is why this compares positions in the retained arc, not integers.
           let candidate = window.newest().pred();
           if window.is_after(candidate, state.current_target) {
               return PollAction::ReAnchor { new_target: candidate };
           }
       }
   }
   ```
   and `AdvancePastStuckVolume { new_target: state.current_target.succ() }`.
   `PollAction`'s `new_target` fields become `VolumeSeq`.
7. **`apply_recovery`**: both arms set `self.target = Some(new_target)` and no
   longer touch `last_completed_volume` or publish a `new_target - 1` as
   delivered (§2.3). Keep the `IngestState::ReAnchoring` publish and both
   `Event` reports.
8. **`event.rs`**: `ReAnchored`/`AdvancedPastStalledVolume`'s `from_volume` and
   `to_volume` become `VolumeSeq` (`Display` keeps the message text identical —
   `event.rs`'s `display_formats_are_human_readable` must keep
   passing with its literals updated to `VolumeSeq::new(79).unwrap()`). Add:
   ```rust
   /// The retained volume-sequence folders did not contain a gap large
   /// enough to identify the retention boundary confidently, so "newest"
   /// was resolved from a smaller gap than the bucket's observed behaviour
   /// predicts. Reported, not fatal — the poller still picks deterministically.
   VolumeSequenceOrderAmbiguous { largest_gap: u32, folder_count: usize },
   ```
   with a `Display` arm.
9. **`IngestStatus.current_volume`** becomes `Option<VolumeSeq>`.

Tests. Port the existing `next_target` tests to `VolumeSeq` unchanged in intent
(`normal_empty_polls_below_threshold_do_not_reanchor`,
`gap_in_listing_reanchors_forward` — still expects `164` from `92..=165` —,
`no_gap_in_listing_does_not_reanchor`,
`reanchor_never_moves_backwards_past_current_target`,
`stuck_mid_volume_advances_only_after_the_longer_threshold`,
`stuck_mid_volume_does_not_trigger_the_reanchor_path`), rename the three
`cold_start_baseline_*` tests to `cold_start_target_*` with targets one higher
than the old baselines, and add:

- `cold_start_target_across_the_measured_wrap_is_the_live_volume` — the W1
  fixture; asserts `199`, **not** `999`. This is parent §1.2 verbatim.
- `cold_start_target_of_an_empty_listing_is_the_first_sequence_number`
- `the_volume_after_the_last_sequence_number_is_the_first` — `succ` at the
  poller's advance site; parent §1.3's `1000` can no longer be formed
- `advance_past_stuck_volume_wraps_at_the_end_of_the_cycle` — stuck on `999`
  yields `1`
- `reanchor_does_not_move_backwards_in_time_across_a_wrap` — `current_target`
  just after a wrap, listing still holding the pre-wrap tail; must be `Continue`
- `reanchor_crosses_the_wrap_to_the_live_volume` — `current_target = 998` with
  the measured fixture must re-anchor to `198`, i.e. numerically backwards but
  forwards in time. This is the failure mode that had no recovery path at all.

### W3 — Move `time` library-side and add Unix-seconds conversion

1. `git mv`-equivalent: move `crates/radar-workstation/src/render/time.rs` to
   `crates/radar-workstation/src/time.rs`; drop `pub mod time;` from
   `render/mod.rs:26`; add `pub mod time;` to `lib.rs` (alphabetical, after
   `sites`/`state` per the existing ordering — keep the list sorted).
2. `render/ui.rs:17` becomes
   `use radar_workstation::time::{format_utc, unix_secs_from_nexrad, utc_from_nexrad};`.
3. Add to the module, expressed so `utc_from_nexrad` and the new functions share
   one definition of the epoch offset rather than repeating `julian_date - 1`:
   ```rust
   /// Hinnant's `days_from_civil` — the exact inverse of `civil_from_days`.
   fn days_from_civil(y: i32, m: u32, d: u32) -> i64;
   /// Seconds since the Unix epoch for a broken-down UTC civil time.
   pub fn unix_secs_from_civil(t: CivilTime) -> i64;
   /// Seconds since the Unix epoch for a NEXRAD (julian_date, scan_time_ms).
   pub fn unix_secs_from_nexrad(julian_date: u16, scan_time_ms: u32) -> i64;
   ```
   `unix_secs_from_nexrad` clamps `scan_time_ms` the same way
   `utc_from_nexrad` already does; keep that clamp in **one** place and have
   both call it (Stability as Ethics — these bytes are attacker-influenceable).
4. Tests, alongside the existing ones:
   - `days_from_civil_inverts_civil_from_days` over a multi-decade day sweep
   - `unix_secs_from_nexrad_matches_the_kdox_fixture_scan_time` — the
     2026-06-29 fixture date already used by `hand_computed_2026_06_29`
   - `unix_secs_from_nexrad_clamps_a_malformed_time_of_day`

### W4 — `vcp::nominal_volume_duration`

New file `crates/radar-workstation/src/vcp.rs`; `pub mod vcp;` in `lib.rs`.
Module doc: *operational cadence, deliberately not in `nexrad-decoder` — it is
not decoded from any message, it is knowledge about how the WSR-88D is
operated.*

```rust
/// Nominal time to complete one volume for `vcp_number`. Nominal: SAILS and
/// MRLE insert extra cuts and can extend a real volume by ~1 min per added
/// cut, which is why callers compare against a *multiple* of this rather
/// than against it directly. An unrecognised VCP falls back to the longest
/// defined pattern, so an unknown pattern under-warns rather than crying wolf.
pub fn nominal_volume_duration(vcp_number: u16) -> Duration
```

Table: `12 → 4.2 min`, `212 → 4.5`, `112 → 5.5`, `21 → 6.0`, `121 → 6.0`,
`215 → 6.0`, `35 → 7.0`, `31 → 10.0`, `32 → 10.0`, default `10.0`.

Tests: `known_vcps_have_their_published_cycle_times` (spot-check 12, 35, 212,
31) and `an_unknown_vcp_falls_back_to_the_longest_defined_cycle`.

### W5 — The status bar tells the truth about data age

File: `crates/radar-workstation/src/render/ui.rs`, plus the `ChromeInput`
construction in `render/mod.rs`.

1. Replace `ChromeInput`'s `displayed_volume: Option<VolumeId>` with one struct
   so the identity and its VCP cannot drift apart:
   ```rust
   /// Identity of the sweep actually on screen — the only honest source for
   /// a data-age readout (plan §1.4). Both fields come from the same
   /// `DisplaySweep`, in one `find`.
   #[derive(Debug, Clone, Copy)]
   pub struct DisplayedScan { pub volume: VolumeId, pub vcp_number: u16 }
   ```
   `render/mod.rs:316` becomes one
   `.map(|s| DisplayedScan { volume: s.volume, vcp_number: s.vcp_number })`.
2. Add `pub now_unix: i64` to `ChromeInput`, filled beside the existing
   `now: Instant` from `SystemTime::now().duration_since(UNIX_EPOCH)`, with a
   pre-epoch clock mapping to `0`. Keep `now: Instant` — poll health is a
   monotonic-clock question and must not become wall-clock-sensitive.
3. Three pure functions, next to the existing `age_secs`:
   ```rust
   /// Age of the data on screen, in seconds of wall-clock UTC. `None` when
   /// nothing is displayed. A volume timestamped in the future (clock skew,
   /// or a corrupt header) clamps to 0 rather than reading as negative.
   fn data_age_secs(displayed: Option<DisplayedScan>, now_unix: i64) -> Option<i64>;
   /// "42s", "7m 12s", "21h 03m" — coarser as the number gets larger,
   /// because at 21 hours the seconds are noise.
   fn format_age(secs: i64) -> String;
   /// Past two nominal VCP cycles the display is no longer current in any
   /// operationally useful sense (plan §2.6).
   fn age_is_alarming(secs: i64, vcp_number: u16) -> bool;
   ```
4. In `status_bar`, replace the current single readout with, in order:
   - the scan time (unchanged, `format_utc(utc_from_nexrad(..))`);
   - `age {format_age(..)}`, in `ACCENT` when `age_is_alarming`, plain
     otherwise, and `"no data yet"` when nothing is displayed;
   - `poll {n}s ago` from the existing `age_secs(snapshot.ingest.last_success,
     now)` — kept, relabelled, and no longer the freshness number;
   - the existing ingest-state label and recent-event slot, unchanged.
5. Add the two new keys to nothing — no key bindings change in Part A.
6. Tests in `ui.rs`'s existing `mod tests`:
   - `data_age_is_measured_from_the_displayed_scan_not_the_last_poll` — the
     parent §1.4 regression: a volume scanned at 2026-09-03T03:08Z read at
     2026-09-04T00:55Z reports ~21 h and alarms, while the poll readout would
     still say 3 s
   - `data_age_of_a_future_timestamped_volume_clamps_to_zero`
   - `data_age_is_none_when_no_scan_is_displayed`
   - `age_is_alarming_past_two_nominal_vcp_cycles` / `..._is_not_alarming_within_one_cycle`
   - `format_age_switches_units_at_the_right_magnitudes`

### W6 — Errata, doc corrections, and the live regression test

**Doc edits** (parent A1). Correct the claim, cite the measurement date, and say
plainly that the bucket's layout is observed rather than contractual:

| File | What to change |
|---|---|
| `CLAUDE.md`, "Chunk bucket key layout" | "unpadded, monotonically increasing per-site integer" → unpadded **cyclic counter over 1–999**, with the measured runs, the 451-prefix listing, and the ~2026-09-03T03:13Z roll. Keep the existing lexical-ordering warning — it is still true. |
| `CLAUDE.md`, "Data Sources" | "Chunks persist for a maximum of 24 hours" → observed ~48 h (`KDOX/749/` still serving 2026-09-02T00:05Z objects at 2026-09-04T00:55Z); not a contract in either direction. |
| `docs/adr/0011-chunk-stream-data-source.md` (:20, :95) | Erratum block (ADR-0018's style): retention observed ~48 h, and the sequence counter is cyclic. Do not rewrite the decision body. |
| `docs/adr/0014-http-ingest-own-the-boundary.md` (:208) | Erratum: strike "monotonically increasing". |
| `docs/architecture/data-flow.md` (:113, :124) | Same two corrections. |
| `docs/architecture/nexrad-data-types.md` (:15–17) | The three "24 hours" retention cells. |
| `docs/dependency-inventory.md` (:371) | Same correction. |
| `crates/radar-workstation/src/ingest/s3_poll.rs` | Done in W2.1/W2.4. |

Do **not** edit completed plan documents under `docs/plans/` (stage-0-1,
stage-2, dependency-inventory-remediation, documentation-remediation). They are
records of what was known when they were written; correcting them retroactively
destroys the audit trail. The parent plan `stage-6a-time-handling.md` is the
live one and already states the finding.

**Live regression test** (parent A5), in `s3_poll.rs`'s `#[ignore]`d live suite
beside `does_not_stall_when_forced_past_the_newest_real_volume`:

```
#[tokio::test] #[ignore]
async fn cold_start_anchor_is_within_one_vcp_cycle_of_now()
```

- list the real volume folders for `LIVE_TEST_SITE`;
- assert every parsed folder is in `1..=999` (this is the tripwire for the
  §2.2 assumption — if the bucket's modulus ever changes, this fails loudly
  instead of the poller quietly misbehaving);
- print `folder_count`, the window's `oldest`/`newest`/`largest_gap`;
- take `cold_start_target(&folders)`, list that directory's keys, parse the
  `YYYYMMDD-HHMMSS` prefix of the newest key with a `#[cfg(test)]`-local helper
  (fixed-width, so `last()` on the sorted listing is the newest) and convert it
  via `time::unix_secs_from_civil`;
- assert the result is within `2 × nominal_volume_duration(35)` of
  `SystemTime::now()` — a generous bound that still fails hard on a 21-hour
  anchor — and print the measured age either way.

The key timestamp is used rather than `LastModified` because the XML parser
deliberately does not materialise `LastModified` text, and adding it for a
test-only path would be work the production path never uses.

---

## 4. Test matrix

| Property | Where |
|---|---|
| The counter wraps and `succ`/`pred` follow it | `volume_seq`: `successor_of_the_last…`, `predecessor_of_the_first…` |
| The newest volume across a wrap is the post-wrap one | `volume_seq`: `window_over_the_measured_kdox_wrap…`; `s3_poll`: `cold_start_target_across_the_measured_wrap…` |
| An unwrapped listing behaves exactly as before | `volume_seq`: `window_over_a_single_unwrapped_run_agrees_with_numeric_max`; the ported `cold_start_target_*` tests |
| A skipped sequence number is not mistaken for the wrap | `volume_seq`: `skipped_sequence_numbers_inside_the_arc_do_not_split_it` |
| The `1000`-target deadlock cannot recur | `volume_seq`: `out_of_range_sequence_numbers_are_rejected`; `s3_poll`: `the_volume_after_the_last_sequence_number_is_the_first` |
| Re-anchor still refuses a stale listing | `s3_poll`: `reanchor_never_moves_backwards_past_current_target`, `reanchor_does_not_move_backwards_in_time_across_a_wrap` |
| Re-anchor now recovers *across* a wrap | `s3_poll`: `reanchor_crosses_the_wrap_to_the_live_volume` |
| Recovery policy is unchanged | the six ported `next_target` tests |
| An ambiguous listing is reported, not guessed at | `volume_seq`: `a_listing_with_no_trusted_boundary_gap…`; the new `Event` arm |
| Displayed age is the scan's, not the poll's | `ui`: `data_age_is_measured_from_the_displayed_scan_not_the_last_poll` |
| A stale display alarms | `ui`: `age_is_alarming_past_two_nominal_vcp_cycles` |
| Calendar arithmetic round-trips | `time`: `days_from_civil_inverts_civil_from_days` |
| The bucket's numbering hasn't changed under us | live: `cold_start_anchor_is_within_one_vcp_cycle_of_now` |

---

## 5. Validation

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check && cargo audit          # no dependency change expected; confirm
cargo test -p radar-workstation -- --ignored --nocapture   # live, network
cargo run --release -- KDOX
```

In the running app, confirm: the scan time is within minutes of UTC now; the
`age` readout reads in seconds or minutes and is not accented; `poll Ns ago`
stays small and separate. If the counter has not wrapped recently, the wrap path
is covered only by the fixture tests — that is expected and is exactly why the
fixture exists.

To re-measure the listing independently (read-only, no credentials):

```
aws s3api list-objects-v2 --bucket unidata-nexrad-level2-chunks \
  --prefix KDOX/ --delimiter / --no-sign-request \
  --query 'CommonPrefixes[].Prefix' --output text
```

---

## 6. What this plan deliberately does not do

- **No history, no second frame.** `RadarState` still holds one sweep per
  elevation. Parts B–D of the parent plan are untouched, and nothing here
  should acquire a "while we're in there" ring buffer.
- **No new dependencies.** The calendar arithmetic, the cyclic type and the
  VCP table are all a few dozen lines of stdlib (CLAUDE.md; the same call
  `paths.rs` made on `dirs` and `event.rs` on a logging crate).
- **No recovery-policy tuning.** Thresholds and the re-anchor target choice are
  preserved; if they turn out to be wrong that is a separate, measured change.
- **No archive-bucket work.** `Bucket::Archive` stays unconstructed.
- **No new ADR.** Errata only (§2.7).

---

## 7. Risks

- **The modulus is an observation.** If the counter is ever not 1–999, folders
  outside the range become `UnrecognizedVolumeFolder` reports and the poller
  degrades to polling `1`. That is loud and recoverable-by-rebuild, and the
  live test fails first. It is a deliberate trade against silently mis-ordering.
- **`MIN_TRUSTED_BOUNDARY_GAP` is a calibration, not a contract** — in the
  spirit of ADR-0029's note about `MIN_M_PER_PX`. It is reported when violated
  and never load-bearing for correctness of the pick, only for confidence in it.
- **W2 touches the one code path with no offline integration coverage.** Run
  the live suite before considering W2 done, and keep the poller's diff
  mechanical: every `± 1` that disappears should be accounted for by `succ`,
  `pred`, or §2.3's field split.

---

## 8. Commit policy

**Do not create commits, branches, tags, or pull requests. Do not run
`git add`, `git commit`, `git push`, or `git checkout -b`.** Leave all changes
in the working tree and report what was changed, what passed, and anything left
undone. The developer reviews and commits.
