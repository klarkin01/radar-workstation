# Implementation Plan — Stage 0 and Stage 1: Close the Acquisition Path

**Status:** Drafted, not started
**Drafted:** 2026-07-30
**Implements:** `docs/project-inventory.md` §6, Stage 0 (item 1) and Stage 1 (items 2–5)
**Baseline commit:** `668b1ca` (working tree: `.gitignore` modified, `.github/` untracked,
`docs/project-inventory.md` untracked)
**Toolchain:** rustc 1.95.0 / cargo 1.95.0, `x86_64-unknown-linux-gnu`

This plan is written to be executed in a later session. It carries every decision already
taken so implementation does not need to re-derive them from the ADRs. Where a decision is
still open it is marked **DECIDE** and states a recommendation and the reasoning behind it.
Nothing in this plan requires answering any of the twelve open questions in
`docs/open-questions.md` — that is the defining property of Stage 1 and the reason it comes
first.

**Scope boundary:** this plan stops at the point where a `VolumeScan` exists in memory and
sweeps are announced as they close. It does not touch `AppState`, `main.rs`, the compute
layer, or anything that renders. Those are Stage 2 and later.

---

## 1. What "done" means

At the end of this plan:

| Claim | How it is demonstrated |
|---|---|
| CI actually runs | A green run on `main` for `build / test / clippy / deny / audit` |
| The chunk stream produces `VolumeScan`s | `VolumeAssembler` unit tests over a recorded chunk sequence, plus one `#[ignore]`d live end-to-end test |
| Sweeps are usable before the volume closes | `SweepClosed` events emitted at end-of-elevation, verified in tests against a fixture volume with a known sweep count |
| Every ADR-0012 exit path works | One test per path: `-E` → `Complete`, early `-S` → `Superseded`, watchdog → `TimedOut` |
| A skipped volume sequence no longer stalls the poller | Unit test over a synthetic folder listing with a gap; live re-anchor test |
| Errors are observable | A typed status value readable from outside the poller, with the age of the last success |
| The decoder does not panic on hostile bytes | A committed corpus + seeded mutator running under plain `cargo test`, mirroring `http-ingest` |
| FR-ND-8 fixture breadth improves measurably | Fixtures for a second site, a precipitation VCP, a standard-resolution cut, and a non-dual-pol era; `TESTING.md` table updated |

**Requirements closed or advanced:** FR-DA-9 (closed), FR-DA-5 (closed except the status-bar
surface, which has no UI yet), FR-ND-6 / BC-6 / NFR-ST-2 (materially advanced), FR-ND-8
(advanced, not closed), FR-ND-3 (verified against real standard-resolution data for the
first time).

**Not closed, deliberately:** FR-DA-3's "display each sweep" half — there is no display.
The assembly layer emits the signal; nothing consumes it until Stage 2.

---

## 2. Stage 0 — Commit what is already done

One work item. Do it first and in its own commit; everything after this depends on CI
existing to catch it.

### S0-W1 — Commit `.gitignore` and the CI workflow

The `.gitignore` change and `.github/workflows/ci.yml` are both already written and both
sitting uncommitted. `cargo deny` has already caught a live advisory pair (E-12 in
`docs/plans/dependency-inventory-remediation.md`), so this gate is load-bearing, not
ceremonial.

**Steps**

1. Before committing, run the full gate locally exactly as CI will, because CI runs tests
   in **release** mode and the release profile sets `overflow-checks = true` — a decoder
   arithmetic overflow that passes `cargo test` can still fail `cargo test --release`:

   ```
   cargo build --release --workspace
   cargo test --release --workspace
   cargo clippy --workspace --all-targets -- -D warnings
   cargo deny check
   cargo audit
   ```

2. Confirm `crates/http-ingest/fuzz/corpus/` is tracked (it is, as of the baseline commit)
   — `response.rs`'s `fuzz_corpus_never_panics` panics on an empty corpus directory, so an
   accidental ignore rule there turns into a CI failure with a confusing message.

3. Commit `.gitignore` and `.github/workflows/ci.yml` together. `docs/project-inventory.md`
   may go in the same commit or its own; it is documentation and blocks nothing.

4. Open the PR against `main` so the workflow's `pull_request` trigger proves itself before
   the `push` trigger does.

**Two pre-existing inconsistencies to resolve while here (both small, both optional):**

- `radar_project.code-workspace` is **tracked** but matched by the `*.code-workspace` ignore
  rule. `.gitignore` does not untrack files, so this is currently inert, but it is a
  contradiction that will confuse the next person. **DECIDE (S0-a):** either `git rm
  --cached` the file, or drop the `*.code-workspace` rule. *Recommendation:* drop the ignore
  rule. The file is genuinely useful to a contributor and is already in history; the rule is
  the thing that is wrong.
- The new `downloads/` and `utility/radar-viz/data/` rules match nothing tracked, so they are
  clean additions. No action needed — noted only so this is not re-checked later.

**Verification:** a green Actions run on the PR, then on `main` after merge.

---

## 3. Stage 1 — Close the acquisition path

Four work items. **S1-W1 is the spine**; S1-W2 and S1-W3 attach to it; S1-W4 is independent
of all three and can be done in parallel or by a different session.

Recommended order: **S1-W1 → S1-W4 → S1-W2 → S1-W3.**

The reordering from the inventory's numbering is deliberate. S1-W4 (fixture breadth) is
listed last in the inventory but produces two inputs that S1-W1 and S1-W2 need in order to
be *correct* rather than merely *plausible*:

- Whether `elevation_number` repeats within a single volume on a SAILS/MRLE precipitation
  VCP. ADR-0012's late-data discard rule keys on elevation number. If a SAILS supplemental
  cut reuses the elevation number of an already-closed sweep, that rule silently discards a
  legitimate cut — the exact low-level velocity data an operator is watching during a
  tornado warning. This cannot be answered from the current fixtures, all of which are
  VCP 35 clear-air with no SAILS.
- Whether the RVOL block is populated on every radial or only on the start-of-volume radial.
  `docs/architecture/nexrad-binary-format.md` §6.1 says code 3 is "the only radial that
  carries a populated RVOL block"; the code comment on `Radial::site_parameters`
  ([radial.rs:46-48](../../crates/nexrad-decoder/src/types/radial.rs#L46-L48)) says the
  opposite for observed KDOX data. ADR-0012's missing-`-S` fallback ("the VCP number and
  site calibration constants are present in every Message 31 radial's RVOL block") depends
  on the code comment being the true one. One of these two statements is wrong and needs to
  be corrected in place.

If schedule pressure forces S1-W4 later, do at least the precipitation-VCP fixture first and
treat the rest of S1-W4 as its own item.

---

### S1-W1 — Volume assembly state machine (ADR-0012)

**Requirement:** FR-DA-9. **Blocks:** everything above the acquisition layer.

New module `crates/radar-workstation/src/assembly/`. This is the piece that turns the
existing `Vec<Radial>` stream into the `VolumeScan` every layer above consumes.

#### 1.1 Shape: a synchronous core with time injected

The state machine must be a **pure, synchronous struct** with no `async`, no I/O, no clock
access, and no channels inside it:

```rust
pub struct VolumeAssembler { /* … */ }

impl VolumeAssembler {
    pub fn new(config: AssemblyConfig) -> Self;

    /// Feed one decompressed, decoded chunk. Returns the events it produced.
    pub fn on_chunk(&mut self, kind: ChunkKind, radials: Vec<Radial>, now: Instant)
        -> Vec<AssemblyEvent>;

    /// Drive the watchdog. Called on a timer by the owning task.
    pub fn on_tick(&mut self, now: Instant) -> Vec<AssemblyEvent>;

    pub fn state(&self) -> AssemblyState;
}
```

`now` is a parameter, not `Instant::now()` read internally. This is the single most important
design decision in this work item: it makes the watchdog timeout — the one ADR-0012 exit path
that is otherwise untestable without a ten-minute test — an ordinary unit test. It also keeps
the entire state machine testable offline with no tokio runtime, no network, and no fixture
I/O beyond what the decoder already needs.

The async wrapper (`assembly::run(rx, tx, ...)`, a `tokio::select!` over the chunk channel and
a `tokio::time::interval`) is a thin shell around this. Keep the shell under ~40 lines; if it
grows logic, that logic belongs in the core.

#### 1.2 States and events

States mirror ADR-0012 exactly — `Idle`, `AwaitingData`, `Accumulating`. Do not invent a
fourth. The ADR's "tentatively closed sweep" state was explicitly considered and rejected
(ADR-0012 §Considered Alternatives); do not reintroduce it.

```rust
pub enum AssemblyEvent {
    SweepClosed { sweep: Arc<Sweep> },
    VolumeClosed { volume: VolumeScan },
    /// A radial arrived for an elevation number whose sweep is already closed.
    LateRadialsDiscarded { elevation_number: u8, count: usize },
    /// A `-S` chunk was never seen for the volume now accumulating.
    MissingStartChunk,
}
```

Emitting events rather than calling callbacks keeps the core testable and keeps the ordering
guarantee explicit: within one `on_chunk` call, every `SweepClosed` precedes the
`VolumeClosed` that contains those sweeps.

**DECIDE (S1-a): does `VolumeScan.sweeps` become `Vec<Arc<Sweep>>`?**
A sweep at super-resolution is roughly 720 radials × ~1832 gates × up to seven moments —
single-digit megabytes. ADR-0012 requires each sweep to be handed to the compute layer at
closure *and* to appear in the closed `VolumeScan`, so without `Arc` every sweep is copied
once per hand-off, against a < 200 MB memory target and a "lightweight by design" principle.
*Recommendation: yes.* `Arc` is `std`, so `nexrad-decoder` stays dependency-free, and
sweep closure is permanent per ADR-0012 — the data is immutable after hand-off, which is
exactly what `Arc` without a lock expresses. Change `Sweep`'s consumers accordingly; there
are none yet, which is why this is the cheapest moment to make the change.

**DECIDE (S1-b): does `Sweep` gain `elevation_deg`?**
`Sweep` currently carries only `elevation_number`, `radials`, `complete`. The elevation
*angle* is what every layer above cares about — the compute layer for Echo Tops and VIL, the
render loop for the sweep selector, the UI for the label. It is available on every radial.
*Recommendation: yes* — add `elevation_deg: f32`, populated from the first radial of the
sweep, plus `nyquist_velocity_mps: Option<f32>` and `unambiguous_range_km: Option<f32>`
hoisted from the same radial (both are per-sweep constants in practice, and velocity
rendering will need Nyquist for the color table range). Record in `TESTING.md` or a code
comment that these are hoisted from the first radial, not independently decoded.

#### 1.3 Behaviors to implement, each with a named test

| ADR-0012 rule | Implementation note | Test |
|---|---|---|
| `-S` → `AwaitingData`, init `VolumeContext` | Context from Message 5 if S1-W2 has landed; otherwise VCP number from the first radial's RVOL | `start_chunk_initializes_context` |
| First `-I` → `Accumulating` | | `first_intermediate_begins_accumulating` |
| Sweep closes on `EndOfElevation` | Also on `EndOfVolume`, which implies end of the final elevation | `end_of_elevation_closes_sweep` |
| Sweep closes on new elevation number without `EndOfElevation` | Previous sweep closed with `complete = false` | `elevation_change_closes_previous_sweep_incomplete` |
| Late radials discarded | Keyed on a set of closed elevation numbers; emit `LateRadialsDiscarded`, never mutate | `late_radials_for_closed_sweep_are_discarded` |
| `-E` → `Complete` → `Idle` | Decode the `-E` radials *before* closing | `end_chunk_completes_volume` |
| `-S` before `-E` → `Superseded` → `AwaitingData` | New volume begins immediately in the same call | `early_start_chunk_supersedes_volume` |
| Watchdog → `TimedOut` → `Idle` | Threshold from `AssemblyConfig`, not a literal | `watchdog_times_out_stalled_volume` |
| Missing `-S` entirely | Emit `MissingStartChunk`, carry forward previous statics, proceed | `intermediate_without_start_chunk_still_accumulates` |
| Missing `-I` | Nothing to do — absent azimuths are simply absent | `missing_intermediate_leaves_azimuth_gap` |

**Watchdog threshold:** ADR-0012 says "approximately 10–15 minutes (well beyond the longest
VCP cycle)". Use **12 minutes** as the `AssemblyConfig` default, in a named constant with a
comment citing the ADR. Do not make it configurable by the user at this stage.

**Site-change / reset:** `VolumeAssembler::reset()` returning to `Idle` and dropping all
in-flight state. FR-DA-4 consumes this in Stage 7; adding the method now is one line and
avoids retrofitting it into a struct whose invariants have hardened.

#### 1.4 Test fixtures for assembly

The existing five fixtures in `crates/nexrad-decoder/tests/fixtures/` are **single Message 31
records**, not chunk streams — they cannot exercise sweep closure. Two options:

**DECIDE (S1-c): recorded chunks or synthetic radials for assembly tests?**
*Recommendation: synthetic radials, built by a test helper.* A `radial(elevation_number,
azimuth, status)` builder in the assembly module's test module produces every ordering the
state machine must handle — including orderings that are rare or impossible to capture live
(late radials, a missing `-S`, a volume with no `-E`) — in a few lines each, with no new
committed binaries. Reserve real recorded chunks for one `#[ignore]`d end-to-end test
(S1-W1 step below) that proves the synthetic model matches reality. Committing a full volume
of chunks (~79 files, tens of megabytes) to exercise what a 20-line builder covers is the
wrong trade, and `downloads/` is now gitignored precisely because bulk sample data does not
belong in the repository.

**One live end-to-end test** (`#[ignore]`d, mirroring the existing `s3_poll.rs` live tests):
poll a real site until one `VolumeClosed` with status `Complete` is produced; print sweep
count, per-sweep radial counts, elevation angles, and wall-clock time to first `SweepClosed`.
That last number is the first real measurement of FR-DA-3's "displayable within 30–60
seconds" claim and should be recorded in this plan's Results section.

---

### S1-W4 — Decoder hardening toward FR-ND-8

**Requirements:** FR-ND-6, FR-ND-8, BC-6, NFR-ST-2. Also retires half of Q17.

Done second, for the reasons in §3's ordering note. Four independent sub-items.

#### 4a. Corpus and mutator on stable `cargo test`

Mirror the pattern `http-ingest` already established
([response.rs:525-590](../../crates/http-ingest/src/response.rs#L525)): a committed corpus
directory, a `corpus_never_panics` test that runs every file through the parser, and a
`mutated_inputs_never_panic` test using a seeded xorshift64 mutator so a failure is
reproducible rather than a flake.

Corpus seeds, in `crates/nexrad-decoder/tests/fixtures/corpus/`:

- The five existing fixtures, unmodified (golden inputs).
- Truncation at each structural boundary: mid-CTM-header, mid-message-header, mid-fixed-header,
  mid-pointer-table, mid-moment-block, mid-gate-data.
- Hostile pointer values: block pointers past the end of the body, pointing at each other,
  pointing back into the fixed header, `0xFFFFFFFF`, and zero for a block the header claims
  exists.
- Hostile counts: `num_data_blks` of 0, 3, and 65535; `gate_count` far exceeding the
  remaining bytes; `word_size` values other than 8 and 16.
- Hostile framing: `size_hw` of 0 with `msg_type == 31`; `size_hw` smaller than the header;
  a message claiming a size that runs past the buffer; an unknown `radial_status` code.
- A stream of thousands of zero bytes, and a stream of `0xFF` bytes.

**DECIDE (S1-d): where does the shared mutator live?**
The xorshift64 mutator is ~30 lines currently inside `http-ingest`'s `#[cfg(test)]` module.
Duplicating it into `nexrad-decoder` violates the project's DRY instruction; sharing it needs
a home. *Recommendation:* a new dev-only workspace crate `crates/fuzz-support`, with no
external dependencies, listed in `[workspace] members` but **not** in `default-members`, and
consumed as a `[dev-dependencies]` path dependency by both crates. It ships in no binary, adds
nothing to `Cargo.lock`'s production graph, and gives the mutator one place to improve. Refactor
`http-ingest` to use it in the same commit, so the two do not drift.

#### 4b. Decompression bomb bound in `chunk.rs`

Found while reading for this plan; not in the inventory. `decompress_chunk` and
`decompress_blocks` ([chunk.rs:107-166](../../crates/radar-workstation/src/chunk.rs#L107-L166))
call `BzDecoder::read_to_end` with **no bound on the decompressed size**. bzip2's worst-case
expansion ratio is several thousand to one, and the input is attacker-influenceable network
data on a path whose governing principle is that the application must not fall over during an
event. Today the peer is S3 over TLS, which makes exploitation unlikely — but "unlikely given
the current peer" is not the standard `http-ingest` was held to, and `http-ingest` has a
`Limits` type for exactly this class of problem while `chunk.rs` has none.

Fix: read through `std::io::Read::take(limit)` and return a typed
`ChunkError::DecompressedTooLarge { limit }` when the limit is hit. Size the limit from
observed data — an `-I` chunk decompresses to roughly 120 radials of a few kilobytes each —
with generous headroom (recommend **32 MiB** per chunk, and a separate, larger bound is not
needed because chunks are decompressed one at a time). Same treatment for the multi-block
loop, bounding the *total* across blocks rather than per block, so a `-S` chunk with many
small blocks cannot slip past a per-block check.

Add the limit to the corpus work above: a small, hand-built bzip2 bomb as a test input.

#### 4c. Unknown radial status codes must not destroy the chunk

`RadialStatus::from_code` returns `None` for any code above 5, and `parse_message31` turns
that into `DecodeError::UnknownRadialStatus`, which propagates out of `parse_radial_stream`
and **discards all 120 radials in the chunk** — roughly 100° of coverage — because one radial
carried a code this build does not recognize.

That is the wrong failure shape under FR-ND-7 ("a decode failure must not crash or freeze the
application; the most recently successfully decoded scan must remain displayed") and under
Stability as Ethics generally. Newer RDA builds have added status codes for SAILS and MRLE
variants; the repo's own status table (`nexrad-binary-format.md` §6.1) and MetPy's decoder do
not agree on the meaning of code 5, which is itself evidence that this list is not closed.

Fix: add `RadialStatus::Unknown(u8)`, keep the radial with its geometry and moment data
intact, and treat it as `Intermediate` for sweep-closure purposes — an unknown code is by
definition not a closure signal we can act on, and elevation-number change still closes the
sweep. Log a count rather than per-radial. Verify the true meaning of codes 5 and above
against ICD 2620002 during S1-W2's inspection work and correct §6.1's table in the same pass.

#### 4d. Fixture breadth

Four new fixture sets. Note the sourcing constraint: the chunk bucket retains only 24 hours,
so anything historical must come from the **archive** bucket
(`unidata-nexrad-level2`, `YYYY/MM/DD/SITE/…` layout) — already on `http-ingest`'s allowlist
([host.rs:3-6](../../crates/http-ingest/src/host.rs#L3-L6)) and already fetchable with
`utility/nexrad-sample`. Archive files are whole volumes with a volume header and internally
BZ2-compressed Message 32 sub-blocks, a different envelope from the chunk path; extracting
Message 31 records from them is a `utility/nexrad-inspect` job, not a production-code job.

| Fixture | Source | What it settles |
|---|---|---|
| Second site | Any operational site, current chunk stream | Multiple-sites half of FR-ND-8; catches anything accidentally KDOX-specific |
| Precipitation VCP (12, 212, or 215) with SAILS | Chunk stream during active weather, or archive | **Whether `elevation_number` repeats within a volume** (blocks S1-W1's discard rule); multiple-scan-modes half of FR-ND-8 |
| Standard-resolution cut | An upper tilt of a super-res VCP, or an older archive volume | FR-ND-3 verified against real data; **retires half of Q17** |
| Non-dual-pol era | Archive bucket, pre-2013 volume | Non-dual-pol half of FR-ND-8; exercises the "moment block absent" path with real data |

For each: extend `utility/nexrad-inspect/gen_fixtures.py` rather than writing a second script,
follow the existing `<site>_<vcp>_<status>.bin` naming, and add per-fixture assertions in
`tests/decode_radial.rs` in the same style as the KDOX ones.

**Documentation to update when the measurements land:**

- `crates/nexrad-decoder/TESTING.md` — the coverage table is the point of that file.
- `docs/architecture/rendering.md` — the Polar Grid Representation section, with the
  standard-resolution geometry it currently cannot state.
- `docs/open-questions.md` — Q17, narrowed to the remaining texture-format question.
- `CLAUDE.md` — the "Confirmed Test File Values" table if any value there proves site-specific.

---

### S1-W2 — Decode `-S` metadata messages

**Requirement:** ADR-0012's `VolumeContext`; closes the gap named in `TESTING.md` §"`-S` chunk
metadata is not decoded". Minimum viable scope is **Message 5 (VCP)**.

#### 2.1 Establish the format first

Message 5's byte layout is **not** in `docs/architecture/nexrad-binary-format.md` — that
document covers the chunk envelopes, the volume header, the message header, and Message 31
only. Do not write parsing code against a guess.

1. Inspect a real `-S` chunk with `utility/nexrad-inspect` (extend `inspect_messages.py`;
   `downloads/KDOX_20260629_1801/20260629-180100-001-S` is a known-good input).
2. Cross-check field-by-field against MetPy's `Level2File`, as was done for Message 31 — the
   existing `metpy_inspect_*.py` scripts are the pattern.
3. Cross-check against ICD 2620002, and settle the radial-status-code question from S1-W4c in
   the same sitting.
4. **Write the confirmed layout into `nexrad-binary-format.md` as a new section before writing
   Rust.** That ordering is what made the Message 31 work reliable; keep it.

**Segmentation is a real risk here, not a theoretical one.** The message header carries
`num_segments` and `segment_num` (§5 of the format doc), and `parse_radial_stream` currently
advances every non-Message-31 record by a flat `LEGACY_MSG_SIZE` (2432 bytes) with no regard
for either field. Message 5 with a long VCP, and certainly Message 18, span multiple segments.
Determine during inspection whether Message 5 for the VCPs in scope fits in one segment; if it
does not, segment reassembly is part of this work item, keyed on `(msg_type, seq_num,
segment_num)`.

#### 2.2 API, and the DRY constraint

`nexrad-decoder` gains:

- `types::vcp::VcpDefinition` — VCP number, pattern type, and the per-elevation cut table
  (elevation angle, waveform type, super-res flags, PRF/Doppler parameters as far as the
  compute layer will plausibly need them). Resist decoding fields nobody will read.
- `parse::message5::parse_vcp(record: &[u8]) -> Result<VcpDefinition, DecodeError>`.
- A new public entrypoint for metadata streams.

**DECIDE (S1-e): one entrypoint or two?**
*Recommendation: two entrypoints over one shared framing walk.* Keep
`parse_radial_stream(data) -> Vec<Radial>` exactly as it is — its contract is good and every
existing test depends on it — and add `parse_metadata_stream(data) -> VolumeMetadata`. To
satisfy the DRY instruction, extract the message-framing loop currently inlined in
`parse::mod.rs` into a private `fn for_each_message(data, f: impl FnMut(MessageHeader, &[u8]))`
and implement both public functions over it. The framing walk — legacy-size skipping, 4-byte
alignment, the size-halfwords convention, and whatever segment handling §2.1 turns up — is
precisely the logic that must not exist in two places, since it is also where the hostile-input
risk lives.

Do **not** merge the two into one function returning both radials and metadata. Chunk kinds are
disjoint in practice (`-S` carries no Message 31; `-I`/`-E` carry nothing else), and a single
function would force every caller to handle a case that cannot occur.

#### 2.3 `VolumeContext` and the fallback path

`VolumeContext` lives in `crates/radar-workstation/src/assembly/context.rs`, **not** in the
decoder — the decoder parses formats, the assembly layer holds session state. It carries the
decoded `VcpDefinition`, the site parameters, and the carried-forward statics ADR-0012
describes (Messages 15 and 18, when those are decoded).

The ADR's missing-`-S` fallback — VCP number and calibration constants recovered from a
Message 31 RVOL block — must work whether or not S1-W2 lands, and is already required by
S1-W1. Its correctness depends on resolving the RVOL-on-every-radial contradiction flagged in
§3. If RVOL turns out to be populated only on the start-of-volume radial, the fallback still
works (that radial is in the first `-I`), but the code must not assume any arbitrary radial
will carry it.

**Messages 2, 3, 15, 18 — scope:** decode Message 2 (RDA Status) if the format inspection is
cheap, because it feeds the status bar later. Messages 3, 15, and 18 are explicitly **out of
scope for Stage 1** — nothing above consumes them until clutter filtering and calibration
display exist, neither of which is in v1.0 scope. Record that as a note in `TESTING.md` so the
gap stays visible rather than looking like an oversight.

---

### S1-W3 — Poller robustness

**Requirements:** FR-DA-5, plus the known stall documented at
[s3_poll.rs:143-150](../../crates/radar-workstation/src/ingest/s3_poll.rs#L143-L150) and in
`data-flow.md`.

#### 3a. Skipped volume-sequence recovery

The failure: `poll_once` always targets `last_completed_volume + 1`. Sequence gaps are real
and observed live (79→90, 92→165, 195→268 — inventory §3). When the next number never
materializes, the poller polls a nonexistent prefix forever and the display silently stops
updating. During an event, a display that has quietly stopped updating is worse than one that
has visibly failed.

A second, related stall shares the same fix: a volume whose `-E` chunk never arrives leaves
the poller draining a dead directory indefinitely.

**Design.** The poller cannot currently distinguish "this volume directory does not exist yet"
from "it exists but has no new keys since `start-after`" — both return zero keys. Add that
distinction and a re-anchor:

1. Track `seen_any_key_in_current_volume: bool`, reset when the target volume advances.
2. Track `consecutive_empty_polls: u32`.
3. When `consecutive_empty_polls` exceeds a threshold **and** `!seen_any_key_in_current_volume`,
   re-run `list_volume_folders` and re-anchor to `max(newest_folder - 1, current_target)`,
   emitting a status event. Never re-anchor backwards past a volume already delivered.
4. Separately, when `consecutive_empty_polls` exceeds a larger threshold **and**
   `seen_any_key_in_current_volume` (the stuck-mid-volume case), close the current volume by
   advancing past it, letting the assembly layer's watchdog mark it `TimedOut`.

**Thresholds.** A volume takes 4–6 minutes in clear air and 1–2 in precipitation, at a 5-second
poll interval — so a normal gap between chunks is a handful of empty polls, and a genuinely
absent directory produces an unbounded run of them. Recommend **12 empty polls (~60 s)** for
the never-saw-a-key re-anchor and **60 empty polls (~5 min)** for the stuck-mid-volume advance.
Both as named constants with the arithmetic in a comment. Re-listing costs a measured ~196 ms
for 480 folders (inventory §2.1), so a re-anchor once a minute in the failure case is
affordable; re-listing on *every* poll is not, and is not proposed.

**Tests.** Factor the decision into a pure function — `fn next_target(state: &PollState,
listing: Option<&[u64]>) -> PollAction` — in the same spirit as `cold_start_baseline`, which is
already a pure, offline-testable function for exactly this reason. Then: gap-in-listing
re-anchors; normal empty polls do not re-anchor; re-anchor never moves backwards; stuck
mid-volume advances after the longer threshold. Add one `#[ignore]`d live test that forces a
target past the newest real volume and asserts recovery within the threshold.

#### 3b. FR-DA-5 — graceful failure and an observable status

The poller currently swallows every error into `eprintln!`
([s3_poll.rs:85-88](../../crates/radar-workstation/src/ingest/s3_poll.rs#L85-L88)). FR-DA-5
requires the last successful scan to remain displayed, the error to be indicated, and the
**age of the displayed data** to be shown. Nothing above the poller can currently observe any
of that.

**Design:** the poller publishes to a `tokio::sync::watch::Sender<IngestStatus>` alongside its
existing `mpsc` chunk channel.

```rust
pub struct IngestStatus {
    pub state: IngestState,          // Polling | Retrying { attempts } | Stalled | ReAnchoring
    pub last_success: Option<Instant>,
    pub last_error: Option<IngestErrorKind>,   // typed, not a String
    pub current_volume: Option<u64>,
}
```

`watch` rather than `mpsc`: the status bar wants the *latest* status every frame, not a queue
of every status transition, and a `watch` receiver cannot cause backpressure on the poller if
nothing is reading it. `last_success` gives the status bar the data-age display FR-DA-5 asks
for, computed at read time.

`IngestErrorKind` must be a typed enum, not a formatted string — the status bar will want to
distinguish "no network" from "S3 returned 503" from "malformed listing", and a `String`
forces that decision to be re-made by parsing text.

Also in scope: classify `PollError` into transient (retry, keep polling) versus persistent
(surface prominently). `http-ingest` already does one idempotent retry at the connection layer
([lib.rs:70-88](../../crates/http-ingest/src/lib.rs#L70-L88)); the poller's job is the layer
above — repeated failures over time, not a single dropped connection. Do not add a second
retry loop that duplicates the client's.

#### 3c. The logging question

`eprintln!` appears in `s3_poll.rs` in four places, each with a `TODO` pointing at "once a
logging crate is added". This plan's other work items add error paths faster than any of them
add somewhere for those errors to go.

**DECIDE (S1-f): what replaces `eprintln!`?** This one needs the user, per CLAUDE.md's "do not
add dependencies without asking first". Three options:

1. **`tracing`** — the tokio-ecosystem default, structured, span-aware, pulls in
   `tracing-core` plus a subscriber and its dependencies.
2. **`log` + a hand-written sink** — one tiny facade crate, no subscriber tree, ~50 lines of
   our own code to write records to stderr.
3. **No crate.** A workspace-local `event` module: a typed enum of everything worth reporting,
   published over the same `watch`/`mpsc` seams as `IngestStatus`, with a single stderr sink
   in `main.rs`.

*Recommendation: option 3 for Stage 1, revisited at Stage 4.* Every error Stage 1 produces
needs to reach the status bar as **typed data** anyway — a string log line cannot drive a UI —
so the typed-event path must exist regardless, and options 1 and 2 would then be a *second*
mechanism carrying the same information. It also keeps the 78-package production graph
untouched. The cost is no free `RUST_LOG` filtering, which matters more once there are ten
subsystems than it does with three. Ask before implementing either of the others.

---

## 4. What this plan deliberately does not do

Recorded so a later session does not read these as oversights:

- **No `AppState`, no `main.rs` wiring.** Q4 is unanswered and both are Stage 2. The assembly
  layer's output is an event stream; who consumes it is Stage 2's decision.
- **No compute layer, not even a stub.** A stub would encode an answer to Q8/Q11/Q17 before
  those are asked.
- **No `SweepClosed` consumer.** The event is emitted and tested; nothing subscribes yet. This
  is intentional — it is the seam Stage 2 attaches to.
- **No Message 15 or 18 decoding.** Nothing consumes them in v1.0 scope.
- **No archive-bucket failover** (FR-DA-8) — blocked on Q14, and the primary path works.
- **No user-facing configuration** of poll interval, watchdog, or thresholds. All are named
  constants in Stage 1; FR-CP-1 makes them configurable in Stage 2 if any of them turns out
  to be worth exposing, which most will not be.

---

## 5. Risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| SAILS/MRLE reuses `elevation_number` within a volume, breaking ADR-0012's discard rule | Medium | High — silently drops legitimate low-level cuts | Ordering: S1-W4's precipitation fixture lands before S1-W1's discard rule is finalized. If confirmed, key closed-sweep tracking on `(elevation_number, sweep_ordinal)` and amend ADR-0012 with a dated erratum |
| Message 5 is segmented, and the flat 2432-byte advance mis-frames the `-S` stream | Medium | Medium — bad VCP data, or a decode error on every `-S` | §2.1 resolves this by inspection *before* any Rust is written; segment reassembly is scoped as part of S1-W2 if needed |
| The RVOL-on-every-radial contradiction resolves against ADR-0012's fallback | Low | Medium — missing-`-S` volumes lose their VCP number | Verified in S1-W4; the fallback reads from the first `-I`'s start-of-volume radial either way |
| `Arc<Sweep>` (S1-a) ripples into types not yet written | Low | Low | There are no consumers today; this is the cheapest possible moment |
| The re-anchor threshold is wrong for precipitation-mode cadence | Medium | Low | Thresholds are named constants; the live test prints observed empty-poll runs, which calibrates them against real cadence |
| Design drift from ADR-0012 during implementation | Medium | Medium | The inventory §7 names this as the failure mode to watch. Any deviation gets a dated erratum in ADR-0012, following ADR-0014's established pattern — not a silent rewrite |

---

## 6. Suggested commit sequence

Each line is one reviewable commit; each keeps `cargo test --release --workspace` and
`clippy -D warnings` green.

1. Stage 0 — `.gitignore` + CI workflow (+ the S0-a `.code-workspace` fix)
2. `crates/fuzz-support` extracted; `http-ingest` refactored onto it *(S1-d)*
3. Decoder corpus + `corpus_never_panics` + `mutated_inputs_never_panic` *(S1-W4a)*
4. `RadialStatus::Unknown`; unknown codes no longer discard the chunk *(S1-W4c)*
5. Decompression size bound in `chunk.rs` *(S1-W4b)*
6. New fixtures + assertions + `TESTING.md` / `rendering.md` / `open-questions.md` updates *(S1-W4d)*
7. `Sweep` / `VolumeScan` type changes *(S1-a, S1-b)*
8. `VolumeAssembler` core + unit tests *(S1-W1)*
9. Async assembly task + live end-to-end test *(S1-W1)*
10. Message 5 format documented in `nexrad-binary-format.md` *(S1-W2, docs only)*
11. `for_each_message` extraction + `parse_metadata_stream` + `VcpDefinition` *(S1-W2)*
12. `VolumeContext` wired into the assembler *(S1-W2)*
13. `next_target` + skipped-sequence recovery *(S1-W3a)*
14. `IngestStatus` watch channel + typed error classification *(S1-W3b)*
15. `eprintln!` replaced per the S1-f decision *(S1-W3c)*
16. `data-flow.md` known-gap paragraph removed; implementation-status notes updated

---

## 7. Open decisions summary

| # | Decision | Recommendation | Needs the user? |
|---|---|---|---|
| S0-a | Tracked-but-ignored `radar_project.code-workspace` | Drop the ignore rule | No |
| S1-a | `Vec<Arc<Sweep>>` in `VolumeScan` | Yes | No |
| S1-b | `Sweep` gains `elevation_deg`, Nyquist, unambiguous range | Yes | No |
| S1-c | Assembly test inputs | Synthetic radials + one live end-to-end test | No |
| S1-d | Shared mutator location | New dev-only `crates/fuzz-support` | Worth confirming — it adds a workspace crate |
| S1-e | Metadata parsing entrypoint | Two entrypoints over one shared framing walk | No |
| S1-f | Logging mechanism | Typed events, no new crate, revisit at Stage 4 | **Yes** — options 1 and 2 add dependencies |

---

## 8. Results

All of Stage 0 and Stage 1 (S1-W1 through S1-W4) were implemented and are green on
`cargo build --release --workspace`, `cargo test --release --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny check`, and
`cargo audit`. Measured numbers below, not impressions — same convention as
`docs/plans/dependency-inventory-remediation.md` §9.

- **Wall-clock time from poller start to first `SweepClosed`:** measured twice against
  live KDOX (VCP 35): **1.535 s** and **1.367 s**. Both far under FR-DA-3's 30–60 s
  claim. (`crates/radar-workstation/tests/assembly_live.rs`.)
- **Sweeps per volume / radials per sweep:**
  - KDOX, VCP 35 (live, 2026-07-31): 14 sweeps. Elevations 1–8 super-resolution (720
    radials each), elevations 9–14 standard-resolution (360 radials each). Full volume
    time (poller start to `VolumeClosed`): ~310–355 s.
  - KTLH, VCP 212 (recorded volume 998, precipitation with SAILS/MRLE): 16 sweeps, 8,640
    radials total. Elevations 1–6, 9, 10 standard... — see the az_spacing correction
    below; this volume mixes both resolutions across its 16 elevations.
- **Does `elevation_number` repeat within a SAILS/MRLE volume?** **No.** Confirmed
  against the same KTLH VCP 212 volume: the SAILS/MRLE-inserted low-level cuts repeat an
  earlier elevation *angle* (elevation 9 ≈0.66°, matching elevation 1's ≈0.65°; elevation
  10 ≈0.53°, matching elevation 2's ≈0.53°) but each gets a new, incrementing
  `elevation_number` — never a reused one. ADR-0012's late-data discard rule (keyed on
  elevation number) is safe as designed. See
  `crates/nexrad-decoder/tests/fixtures/ktlh_vcp212_sails_repeated_low_elevation.bin` and
  `nexrad-binary-format.md` §15.2.
- **Standard-resolution gate count/width and azimuthal spacing (Q17):** gate width
  (0.25 km) and first-gate range (2.125 km) are **identical** between standard- and
  super-resolution cuts on the same site/VCP — only the azimuthal radial count differs
  (360 vs. 720 per 360° sweep). This also surfaced that `nexrad-binary-format.md` §6.1's
  `az_spacing` code table had the 1/2 meaning **backwards**: code 1 measures 0.5°
  (super-resolution), code 2 measures 1.0° (standard-resolution) — corrected in the same
  pass, confirmed across two independent sites/VCPs (KDOX VCP 35, KTLH VCP 212).
- **Is RVOL populated on every radial, or only on the start-of-volume radial?** **Every
  radial**, in all observed KDOX and KTLH data — confirming the code comment on
  `Radial::site_parameters` was the true statement and correcting
  `nexrad-binary-format.md` §6.1's original claim that code 3 (Start of Volume) is "the
  only radial that carries a populated RVOL block." (That line predates this plan and
  was already superseded in practice; ADR-0012's missing-`-S` fallback works regardless
  of which is true, since it reads from whichever radial happens to carry the block.)
- **Radial status code 5:** empirically **not** "SAILS supplemental low-level cut" as
  previously documented. Appeared exactly once in the KTLH VCP 212 volume, on the single
  highest elevation (16, angle ≈9.84°) — matching MetPy's
  `START_ELEVATION | LAST_ELEVATION`. `RadialStatus::SailsCut` renamed to
  `StartOfLastElevation`; `nexrad-binary-format.md` §6.1 corrected.
- **Observed empty-poll run lengths in steady state:** not measured live this pass — the
  one live test that would calibrate this
  (`does_not_stall_when_forced_past_the_newest_real_volume`, `s3_poll.rs`) is written and
  compiles but takes up to 20 minutes of real VCP cycling to complete and was not run in
  this session. The `REANCHOR_EMPTY_POLLS` (12, ~60 s) and
  `STUCK_MID_VOLUME_EMPTY_POLLS` (60, ~5 min) thresholds are therefore still
  reasoned-from-cadence estimates, not measured; run that test manually to calibrate
  before relying on the exact numbers operationally.
- **Peak decompressed chunk size across the corpus:** the corpus itself is small,
  hand-crafted fixtures (a few KB to ~10 KB each) plus a deliberate decompression-bomb
  fixture (64 MiB of zeros, 79 compressed bytes) used specifically to prove the 32 MiB
  bound rejects it. For a real chunk: a full KDOX `-S` chunk decompressed to **325,912
  bytes** during Message 5 inspection — comfortably under the bound, consistent with the
  "an `-I` chunk decompresses to roughly 120 radials of a few kilobytes each" estimate
  the bound was sized from.

**Additional finding not anticipated by the plan:** while scanning ~35 currently active
sites to find a precipitation VCP for S1-W4d, VCP 12 (KGRK) and VCP 215 (multiple Gulf
Coast/Southeast sites) were also observed live but not captured as fixtures — noted in
`TESTING.md` as a remaining gap, not closed here.
