# Plan — Stage 6a checkpoint: catch up, clean up, and resume time handling

**Status:** in progress — Part (a) A1–A7 done 2026-11-21 (A1 open items: supply-chain advisories, an on-screen look, budget sign-off); A8 (merge) is the developer's
**Date:** 2026-11-21
**Audited:** branch `time_handling` at `343c44a`, against `main` at `baf2d2d`
(working tree: `QUICK-START.md` untracked; nothing else modified)
**Toolchain:** the pinned `rust-toolchain.toml` toolchain, `x86_64-unknown-linux-gnu`
**Scope:** where the project actually stands after Stage 5 and Stage 6a Parts A–B; what
the documentation gets wrong about that; and the ordered steps to (a) clean up, (b) resume
time handling at Part C, and (c) set up what follows it.
**Changes made in producing this document:** none. This file is the only addition.

> **Do not commit.** This plan is executed by an implementation session; the developer
> handles every `git add`, `git commit`, `git push`, branch and PR.

Finding IDs use the `CK-` namespace so they don't collide with `DOC-` (documentation
inventory) or `E-` (dependency inventory).

---

## 1. Summary

**The code is in good shape. The record of it is behind.** Stage 5 is complete and merged
(PR #17). Stage 6a Parts A (stale data / wrap handling) and B (volume history retention,
ADR-0030) are implemented, tested and committed on `time_handling`, but are **not on
`main`**. Part C (the timeline — the first time-handling work an operator can see) has not
been started. No Part C work has been staged locally; the only uncommitted file is
`QUICK-START.md`.

Three things matter most:

1. **`main` still carries the sequence-wrap bug that Part A fixed.** At every 999 → 1
   roll of the volume counter (every few days per site, depending on VCP cadence), a
   cold-started or long-running instance built from `main` anchors to a volume that is
   hours old, never recovers, and shows "updated 3s ago". Part A is a correctness fix to
   shipped behaviour, so it should not wait for Part C.
2. **The status line of almost every plan document is wrong**, and the docs index has no
   entry for Stage 6a or ADR-0030. Someone going by the docs alone would conclude
   Stages 3–5 and Part A never happened, and that time handling is "proposed, not
   started".
3. **Part C has one design problem nobody has named yet** (§5.2, CK-20): the view selects
   a tilt by `elevation_number`, and a loop that crosses a VCP change, which ADR-0030
   deliberately keeps, would quietly show a different elevation angle under the same
   selection.

"Stage 6" in this document means **placefiles**, as every existing plan and
`project-inventory.md` uses it. Time handling is **Stage 6a**, slotted in *before*
Stage 6 by `stage-6a-time-handling.md` §4. Step A7 gets `project-inventory.md` to say so.

---

## 2. Current state of the code

### 2.1 Branch versus `main`

| Commit | Content | Plan |
|---|---|---|
| `c83c049` "fixes stale times" | `ingest::volume_seq` (`VolumeSeq`, no `Ord`; `VolumeWindow` largest-gap ordering), wrap-aware poller, `time.rs` moved library-side, `vcp.rs`, status bar data-age readout, errata on ADR-0011/0014 | `stage-6a-part-a-stale-data.md` |
| `343c44a` "Time history" | `state::history` (`Frame`, `FrameRing`, `RetentionPolicy`, `live_sweeps`/`live_derived` folds), `apply.rs` rewired onto the ring, `history.frames`/`history.budget_mb` config, `render::radar` residency + rate-limited `plan_sync`, ADR-0030, FR-DA-10, the amended memory targets, Q25 resolved | `stage-6a-part-b-retain-history.md` |

Diff: 36 files, +4,709 / −560. Nothing is pushed beyond the local branch.

### 2.2 Build and test health (measured 2026-11-21, this audit)

| Check | Result |
|---|---|
| `cargo test --workspace` | **421 passed, 0 failed, 24 ignored** |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `#[ignore]`d GPU offscreen tests (`--bins -- --ignored`) | **6 / 6 pass**, including both overlay tests and the radar-pass test |
| `#[ignore]`d live poller tests (`--lib -- --ignored`) | **7 / 7 pass**, including Part A's A5 regression `cold_start_anchor_is_within_one_vcp_cycle_of_now`. `does_not_stall_when_forced_past_the_newest_real_volume` took ~590 s against live KDOX (forced 340 → reached 341). It is slow by design (20-minute deadline), not hung |
| `#[ignore]`d live integration tests (`tests/*_live.rs`) | **3 / 3 pass**: `polls_a_real_site_to_one_complete_volume` and both `pipeline_spawn_*` tests |
| `cargo deny check`, `cargo audit` | **not run** in this audit (step A1) |

### 2.3 What exists, by Stage 6a part

| Part | State | Evidence |
|---|---|---|
| A1 doc errata (wrap, retention) | done, except `utility/README.md` (CK-08) | ADR-0011/0014 errata, `data-flow.md`, `CLAUDE.md` |
| A2/A3 wrap-aware anchor and advance | done | `ingest/volume_seq.rs`, `s3_poll.rs`; `1000` is unrepresentable |
| A4 honest data age | done | `render/ui.rs` `data_age_is_measured_from_the_displayed_scan_not_the_last_poll`; poll readout relabelled "poll Ns ago" |
| A5 live wrap regression test | done, passes | see §2.2 |
| B1–B4 history ring, ADR, residency | done | ADR-0030, `state/history.rs`, `render/radar.rs` |
| B: §3.5 default budget sign-off | **unconfirmed on paper** (CK-11) | Part B §3.5 asked the developer to confirm `frames = 12`, `budget_mb = 320`. It was committed, but nothing records the decision |
| B: §6 windowed/headless validation | **unrecorded** (CK-10) | Part B §10 holds the W1 measurements only |
| C1–C6 timeline | **not started** | no `Timeline`, no new `Action`, no FR-TL-*; code has forward-pointing notes at `render/radar.rs:58` and `state/mod.rs:150` |
| D1 launch backfill | done (2026-11-25), in v1.0 per CK-21 | ADR-0033, FR-DA-11, `ingest/backfill.rs`, `FrameRing::behind_mut`; measured in ADR-0033 §8 |
| D2–D4 archive / jump-to-time | not started; v1.0 scope still undecided (CK-21) | `Bucket::Archive` still unconstructed |

### 2.4 Open measurement debt

- **Memory against target.** Part B measured the Stage 5 application, with no history, at
  **VmHWM ≈ 393 MB** on the RTX 4070 SUPER box, against a < 200 MB target. ADR-0030
  puts the gap down to driver/Vulkan mappings charged to RSS, but that is a hypothesis
  and has not been checked. With the default history budget the target becomes
  < 520 MB, so the default config already leaves little headroom on this machine. This
  is the one number in the project that is currently **over its own target**.
- **GPU memory.** `rendering.md` says "Not measured on-device" for the history residency
  set.
- **The 393 MB follow-up points at a file outside the repository.** ADR-0030 §Decision 5
  says to re-measure on "the ADR-0024 hybrid-GPU box — see the `dev-machine-hybrid-gpu`
  note". That note is a local Claude memory file, not a tracked document (CK-12).

---

## 3. State of the documentation

The architecture documents and ADRs written *during* Stage 6a are accurate. ADR-0030,
FR-DA-10, the ADR-0011 erratum, Q25 and the rendering.md residency section all match the
code. What's stale is the layer of **status and index metadata** around them, plus two
front-door documents that missed the Stage 5 sweep.

### 3.1 Findings

| ID | Document | Finding | Severity |
|---|---|---|---|
| CK-01 | `docs/plans/*.md` status lines | Seven plans carry a `**Status:**` line that contradicts their own Results section or the git history: `0014-http-ingest-implementation` "Draft, not yet implemented"; `stage-0-1` "Drafted, not started"; `stage-3`, `stage-4`, `stage-5` "Drafted — not yet implemented"; `stage-6a-part-a` "proposed, not started"; `stage-6a-part-b` "pending developer review and commit" | **high** |
| CK-02 | `docs/plans/stage-6a-time-handling.md` | The live parent plan says "proposed, not started". Part B is marked done in its body but **Part A is not**, and §5's open questions don't say which of them Q25 answered | **high** |
| CK-03 | `docs/README.md` | No rows for ADR-0030, `stage-6a-time-handling`, `stage-6a-part-a-stale-data`, `stage-6a-part-b-retain-history`, or (since Stage 4) `render-adapter-selection`. The "Plans" status column is the only place plan status is collected, and it's incomplete | **high** |
| CK-04 | `docs/architecture/overview.md` | The implementation-status banner says "Stage 3 complete … Map underlays and placefiles (layers 3–5, 7, 9) remain unbuilt — Stages 5–6". The Stage 5 sweep (Stage 5 §18 / §13) missed it. The project tree also lacks `crates/fuzz-support/` and `utility/map-bake/` | medium |
| CK-05 | `README.md` | Status still reads "Stage 5 complete". It says nothing about wrap handling, the data-age readout, or history retention and its two config keys. Accurate as of `main`, behind as of this branch | medium |
| CK-06 | `QUICK-START.md` (untracked) | Useful and mostly accurate, but it is a **third copy** of the run instructions and keyboard map: README "Running it", `CLAUDE.md` "Keyboard Map", and the in-app `F1` overlay hold the others. That breaks the project's DRY rule. Minor inaccuracy: "NOAA's public S3 archive" (the source is the Unidata-hosted chunk bucket, ADR-0011). It doesn't mention data age or the history keys | medium |
| CK-07 | `CLAUDE.md` (gitignored) | Accurate through Part B, but it has become a dense second status ledger that repeats README, `overview.md` and the ADR index, so every stage now updates four status paragraphs. The render module map omits `render/offscreen.rs`, which has existed since Stage 5. DOC-01 noted it is gitignored, and that is still true | medium |
| CK-08 | `utility/README.md` §sample files | Still says "chunks persist 24 hours" and "monotonically increasing per-site integer". It is a live document, not a record, and Part A's A1 missed it | medium |
| CK-09 | `docs/project-inventory.md` | Its "superseded" banner ends at Stage 5, and its stage list has no Stage 6a. Stage 6 / 7 / 8 / 9 there are the only written roadmap past this point | medium |
| CK-10 | `stage-6a-part-a-stale-data.md`, `stage-6a-part-b-retain-history.md` | Part A has **no Results section** at all. Part B's §10 records the W1 measurements but none of the §6 validation outcomes: headless `frames=` climbing across three volume boundaries, windowed FR-DA-3 carry-forward, VmHWM with history on, live and GPU tests | medium |
| CK-11 | ADR-0030 / Part B §3.5 | The default budget (`12` frames / `320` MB) was flagged as "the one number … the developer should confirm in review". It was committed, but there is no record that anyone confirmed it | low |
| CK-12 | ADR-0030 §Decision 5 | Cites a private, untracked memory note (`dev-machine-hybrid-gpu`) as the location of follow-up work | low |
| CK-13 | `ADR-0011` body | The Consequences paragraph "24-hour chunk retention. Chunks are purged after 24 hours" stands uncorrected inline. The erratum below it is correct, and the 2026-07-30 errata used inline `<!-- corrected -->` markers, which a reader skimming the body would see | low |
| CK-14 | Operator configuration | No single place lists the config keys (`site`, `ingest.poll_interval_seconds`, `window.*`, `view.product`/`highways`/`reference`, `history.frames`/`budget_mb`), their ranges, or their defaults. They are spread across ADR-0019, ADR-0030, `REQUIREMENTS.md` and `rendering.md` | low |
| CK-15 | `dependency-inventory.md`, `documentation-inventory.md`, `project-inventory.md` | Point-in-time audits, correctly bannered as such. No action needed. Listed so they aren't "fixed" by mistake | — |

### 3.2 A convention to settle first: may a completed plan's status line change?

Part B §W9 says completed plans "are records of what was known when written" and must not
be edited. That rule is right for plan **bodies**. Applied to the one-line **status
header**, though, it has caused CK-01: every executed plan permanently says it was never
executed. Proposed convention, to be written into `docs/README.md`'s Plans preamble:

> A plan's body is frozen once executed. Its `**Status:**` line is not: the implementing
> session updates it to `Implemented YYYY-MM-DD — see §N Results` (or `Superseded by …`)
> in the same change that fills in Results. `docs/README.md`'s plan table mirrors that
> line.

Every A-step below that edits a plan assumes this convention.

---

## 4. Part (a) — Get cleaned up

Ordered. A1–A3 come before anything else, because merging is blocked on them.

**A1. Close the validation gaps for Parts A and B (CK-10, CK-11).**
Run the steps Part B §6 lists and Part A §5 implies, and record the results:

```
cargo deny check && cargo audit
cargo test -p radar-workstation -- --ignored --nocapture      # live + GPU; allow ~20 min
cargo run --release -- KDOX --headless < /dev/null             # ≥ 3 volume boundaries
cargo run --release -- KDOX                                    # windowed
```

- Headless: `frames=` climbs to the policy and stops, `hist=` tracks it, and
  `HistoryBudgetBound` appears at most once.
- Windowed: Stage 5 behaviour is unchanged (product/tilt/colours/cursor readout; the
  previous volume's tilt still displays during the next volume's early sweeps; switching
  is instant). The data-age readout is plausible and turns to alert colour when the scan
  is stale.
- `grep VmHWM /proc/<pid>/status` after several volumes, with and without
  `history.budget_mb = 0`.

Write these into a new `§9 Results` in `stage-6a-part-a-stale-data.md` and append them to
`stage-6a-part-b-retain-history.md` §10. Record the decision on the §3.5 default in §10
and in ADR-0030 as one dated line: confirmed as-is, or the replacement number.

**A2. Fix the plan status lines and the parent plan (CK-01, CK-02).**
Adopt §3.2's convention, then make the seven status-line corrections. In
`stage-6a-time-handling.md`, mark Part A done with a pointer to its plan, mirroring the
existing Part B note. Set the header to "in progress — Parts A, B done; C next". Annotate
§5's open questions with which were answered by Q25 / ADR-0030 and which remain (frame
semantics for playback: §5.2 below; archive scope: CK-21).

**A3. Bring the index and front-door docs up to date (CK-03, CK-04, CK-05, CK-08, CK-09, CK-13).**

| File | Change |
|---|---|
| `docs/README.md` | add ADR-0030; add the three Stage 6a plans, `render-adapter-selection.md` and this document to the Plans table with correct status; add §3.2's convention to the Plans preamble |
| `docs/architecture/overview.md` | replace the status banner with a short current statement plus a pointer to README (see A5); add `fuzz-support/` and `map-bake/` to the tree |
| `README.md` | status: Stage 5 complete, Stage 6a Parts A–B complete (wrap-safe ingest, scan-age readout, history retention with no timeline yet). Mention the `history.*` keys and `budget_mb = 0` |
| `utility/README.md` | correct the 24 h / monotonic claims; point at ADR-0011's erratum rather than restating it |
| `docs/project-inventory.md` | extend the superseded banner through Stage 6a Part B, and record that Stage 6a sits before Stage 6 |
| `docs/adr/0011-…` | add an inline `<!-- corrected 2026-09-03, see Erratum -->` marker on the 24-hour Consequences paragraph, matching that ADR's existing convention |

**A4. Decide what `QUICK-START.md` is, then remove the duplication (CK-06).**
Recommended: commit it as the **operator-facing** entry point, and make it the single
written copy of the run instructions and keyboard map. README's "Running it" then
shrinks to the two command lines plus a link to it, and `CLAUDE.md`'s keyboard map
becomes a link too. The `F1` overlay stays the in-app copy; it's generated from code and
can't link. In the same change, fix "NOAA's public S3 archive", add one line on the
scan-age readout, and add one line on `history.budget_mb`.

**A5. Thin `CLAUDE.md` (CK-07).**
It is gitignored, and that is fine for a session-guidance file. The problem is that it
restates status that README, `overview.md` and `docs/README.md` also restate. Reduce its
Status section to one paragraph that points at README. Reduce its ADR index to the
one-line titles, with `docs/README.md` holding the long descriptions. Keep the content
that is unique to it: the NEXRAD format findings, design principles, data flow and render
module map. Add `offscreen.rs` to the module map. Stage completion then costs one status
edit, not four. (This is a developer-local file; the implementing session should propose
the diff rather than rewrite it unasked.)

**A6. Move the GPU re-measure follow-up into the repo (CK-12).**
State the follow-up in ADR-0030 itself, or in A1's Results: re-measure VmHWM on the
hybrid NVIDIA + AMD iGPU box with `RADAR_GPU=integrated` and `=discrete`. A tracked
document should not depend on an untracked one.

**A7. Write one configuration reference (CK-14).**
Add a single table of keys, types, ranges, defaults and the ADR that owns each one.
Recommended home: a "Configuration" section in `QUICK-START.md`, since it is
operator-facing, with ADR-0019 linking to it. Don't create a new top-level doc for a
twelve-row table. Part C adds keys (C5), so this table is where they go.

**A8. Merge Parts A + B to `main`.**
Open a PR from `time_handling` once A1–A3 are done. Reasons not to wait for Part C:
`main` has the wrap bug (§1); A and B are a coherent, tested, user-invisible unit;
and every earlier stage landed as one branch → PR → merge, so Part C should start from a
`main` that contains its prerequisites. Suggested PR title:
"Stage 6a Parts A–B: wrap-safe ingest, honest data age, volume history retention".
Branch Part C fresh from the merged `main`.

---

## 5. Part (b) — Get back on track with time handling

Part C is next. `stage-6a-time-handling.md` §3 Part C (C1–C6) is a sound outline, but it
was written before Part B fixed the data model. Before an executable
`stage-6a-part-c-timeline.md` is drafted, the following must be settled. Each one is a
decision for the plan document, not a new open question, unless noted.

### 5.1 Prerequisites already met

- A frame is one volume, holding only its own sweeps; the ring is `StateSnapshot::frames`
  (ADR-0030).
- Residency already keeps one selected grid per older frame resident, and the
  `walking_the_selection_across_every_frame_uploads_nothing_once_resident` test is the
  zero-upload playback property C3 needs.
- `ViewState` stays out of `AppState`. The
  `view_state_is_unchanged_by_any_sequence_of_state_updates` test carries over unchanged.

### 5.2 Decisions the Part C plan must make

**Resolved 2026-11-22** by `docs/plans/stage-6a-part-c-timeline.md` §2 and
[ADR-0031](../adr/0031-timeline-semantics.md): CK-20 → ADR-0031 §1–§3 (a loop position
is a volume; a tilt is its commanded cut angle; one grid-selection rule). CK-22 → §5
(residency follows the focus frame). CK-23 → plan D-6 (keys recorded as this project's
choice in `render/input.rs`). CK-24 → `REQUIREMENTS.md` §2.11, FR-TL-1 to FR-TL-10.
CK-25 → §6 (age always measures the live feed, beside a dedicated-colour badge).
CK-26 → §7 (`render::timeline::time_context`). The text below is left as written.

**CK-20 — What does "the selected tilt" mean across frames?**
`ViewState` stores `elevation_number`. Within one VCP that is stable. Across a VCP change,
which ADR-0030 §B-d deliberately **retains** in the ring, elevation number 3 is a
different angle in VCP 35 than in VCP 12. A loop would then step through frames showing
different tilts under one unchanged selection. That is the "silently misleading" failure
`PHILOSOPHY.md` forbids. Options for the plan to choose between, with tests:

- select by **elevation angle** (nearest cut within a tolerance) when resolving a frame,
  and show the actual angle per frame; or
- keep `elevation_number`, but bound the loop at the VCP boundary (the same rule the live
  fold already uses) and say so on the scrubber.

The same rule must settle **SAILS/MRLE**. Repeated low-level cuts get distinct
`elevation_number`s within one volume (`compute::derived` already dedupes them by angle).
Is a SAILS re-scan a separate loop frame at the lowest tilt, as operators of the
reference application expect, or invisible to the loop? Part B made "a frame is a
volume"; Part C must say whether playback steps volumes or sweeps at a fixed angle. This
is the §5 question "per-sweep animation is the alternative", and Part B left it open.

**CK-22 — `residency` assumes the newest frame is displayed.**
`render/radar.rs:58` already says Part C must revisit this. When the timeline is
`Pinned`, the pinned frame gets every-grid residency, or at least the selected product's
grids for all tilts, so tilt switching while paused stays instant. The plan needs a rule
and a residency test for it.

**CK-23 — Keys.**
C2 proposes `Space` for play/pause and `[` / `]` for stepping. Check them against the
existing map (`R`, `H`, `Home`, `PageUp`/`PageDown`, digits, arrows) and against the
reference application's conventions, as `input.rs`'s module doc requires. Settle loop
length and speed keys at the same time. The keymap edit lands in exactly one written
place (A4) plus the `F1` overlay.

**CK-24 — Requirements first (C6).**
Write FR-TL-* *before* the plan's work items, so the plan has something to be measured
against. Minimum content: live/pinned modes, step, play/pause, loop length, dwell, an
unmissable LIVE / NOT-LIVE indicator, start-`Live` on launch, and the evicted-pinned-frame
behaviour. FR-DA-10 already says "FR-TL-* (Stage 6a Part C, not yet written)".

**CK-25 — Data age when not live.**
Part A's scan-age readout is right for live display. When pinned or playing, the alert
colour must not fire for being in the past *on purpose*. But the operator must still be
able to see that the newest frame is fresh. Two readouts (selected frame time, live-feed
age) or one readout plus the LIVE indicator: the plan must choose one.

**CK-26 — Placefiles will need the timeline.**
Parent §4 already argues the timeline should exist before placefiles. Part C should
expose the selected frame's time as a single, pure accessor that a future layer 7 can
consume. It should not build anything placefile-specific.

### 5.3 Proposed Part C sequence

1. FR-TL-* in `REQUIREMENTS.md` (CK-24), plus a decision record for CK-20. CK-20 changes
   what a played-back image *means*, so this is probably an ADR (ADR-0031) and not just a
   plan decision.
2. `render::view::Timeline` plus the pure `resolve(timeline, snapshot) -> frame` function,
   with tests covering eviction, VCP boundaries, SAILS and an absent product (C1).
3. `Action` variants and key bindings (C2, CK-23).
4. Frame-clock playback with on-demand pacing when stopped (C3), and the residency rule
   for `Pinned` (CK-22).
5. Scrubber and LIVE indicator, plus the data-age rule (C4, CK-25).
6. Persisted preferences — loop length, rate, dwell — added to A7's table (C5).

Draft `stage-6a-part-c-timeline.md` in the house format (done-criteria, decisions, work
items, test matrix, validation, non-goals, risks, commit policy, Results). It's ready to
write once A8 has merged.

---

## 6. Part (c) — Set up for next steps

**CK-21 — Decide Part D's v1.0 scope, deliberately.**
Parent §5's last question is still open. The recommendation is to split it:

- **D1 (backfill the loop from the chunk bucket at launch)** goes **into v1.0**. It needs
  no new format support: the bucket retains ~48 h, it reuses the wrap-aware `VolumeWindow`
  walking backwards, and without it the loop is empty for the first ~40–60 minutes after
  every launch. Without D1, Part C works but is mostly useless during a fast-developing
  event. Plan it immediately after Part C.
- **D2–D4 (archive reader, historical mode, enter-a-time)** get an explicit
  deferral-or-not ADR, the way ADR-0027 handled tiles. D2 is decoder-adjacent, new
  untrusted-input surface (`AR2V` + type-32 BZ2 records), and needs decoder-grade
  hardening. `REQUIREMENTS.md` §6 already lists "arbitrary-time archive playback" as
  deferred, so the ADR may simply confirm that.

**Resume Stage 6 (placefiles) after Stage 6a.**
Nothing in Stage 6a touched Q6 or Q21–Q24, and they are as `CLAUDE.md` describes. Two of
them can be answered **now**, in parallel with Part C, because neither waits on the
directive histogram: **Q21** (the arbitrary-host HTTP client and the deferred ADR-0026
engine/policy split) and **Q24** (plural config keys and persisted enable/order). Q22
(HTTP vs HTTPS) is a requirements contradiction and should be answered before Q21's
design hardens. Q6's corpus session still comes before the parser, and Q23 (icons) follows
Q6.

**Pay down the memory question before Stage 8.**
§2.4 is the only target currently measured as exceeded. Before more retained state
arrives (D1 fills the ring at launch; placefiles add more), get a measurement that
separates application heap from driver mappings: VmHWM vs. `Pss_Anon` from
`/proc/<pid>/smaps_rollup`, on both adapters of the hybrid box. The result goes into
ADR-0030's follow-up line (A6) and decides whether `history.budget_mb = 320` stays the
default.

**Keep the record from going stale again.**
Two habits would have prevented most of §3:

- Make the "docs sweep" work item in every plan (Part B's W9, Stage 5's §13) include an
  explicit checklist: its own status line, `docs/README.md`'s row, the README status
  paragraph, and the `overview.md` banner. If A5 is done, the banner can simply point at
  README, which removes one item from that list.
- When a plan lands, its Results section records the *validation* outcomes (the §6 /
  §12 checklists), not only the measurements. That is the gap CK-10 describes.

---

## 7. Checklist

| Step | Covers | Blocks |
|---|---|---|
| A1 validate and record Parts A–B | CK-10, CK-11 | A8 |
| A2 plan status lines and parent plan | CK-01, CK-02 | A8 |
| A3 index and front-door docs | CK-03, -04, -05, -08, -09, -13 | A8 |
| A4 `QUICK-START.md` as single run/key reference | CK-06 | — |
| A5 thin `CLAUDE.md` (propose, don't impose) | CK-07 | — |
| A6 GPU follow-up into the repo | CK-12 | — |
| A7 configuration reference | CK-14 | Part C's C5 |
| A8 PR `time_handling` → `main` | the main-branch wrap bug | Part C |
| B: FR-TL-* and the CK-20 decision (ADR-0031?) — **done 2026-11-22** (ADR-0031, FR-TL-*) | CK-20, CK-24 | Part C plan |
| B: draft `stage-6a-part-c-timeline.md` — **done; implemented 2026-11-22** | CK-22, -23, -25, -26 | Part C |
| C: Part D scope ADR; D1 plan after Part C — **D1 done 2026-11-25** (ADR-0033); the D2–D4 ADR is still open | CK-21 | — |
| C: Q21, Q22, Q24 answered in parallel | Stage 6 | Stage 6 parser |
| C: memory measurement split heap / driver | §2.4 | Stage 8 |
