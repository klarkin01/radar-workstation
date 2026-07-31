# Documentation Inventory — code vs. docs

**Audited commit:** `d46042c` (working tree: `.gitignore` modified, `.github/` untracked)
**Date:** 2026-07-30
**Toolchain:** rustc 1.95.0 / cargo 1.95.0, `x86_64-unknown-linux-gnu`
**Scope:** every tracked document in `docs/`, `utility/`, and the repository root, read
against the code actually present in `crates/` and `utility/`
**Changes made:** none. This document is the only file added.

Finding IDs use the `DOC-` namespace. `E-` IDs refer to `docs/dependency-inventory.md`;
`W-` IDs to `docs/plans/dependency-inventory-remediation.md`.

---

## 1. Executive summary

**The design documentation is in unusually good condition. The gaps are almost entirely
at the two edges: the repository has no front door, and the audit/plan documents that
drove the last two work sessions were not swept after the work landed.**

The architecture set (`PHILOSOPHY.md`, `REQUIREMENTS.md`, `architecture/`, seventeen ADRs)
is coherent, cross-referenced, and — where it describes subsystems that exist — accurate.
ADR-0014 in particular is maintained to a standard most projects never reach: its body was
rewritten in place to match shipped code (W2), and two errata sections record what changed
and on what evidence. `docs/architecture/nexrad-binary-format.md` matches the decoder
field for field. Verified by reading, not assumed:

| Check | Result |
|---|---|
| `cargo test --workspace` | **123 passed, 0 failed, 12 ignored** — matches the remediation plan's final metrics table exactly |
| ADR-0014 Decision table vs. `crates/http-ingest/Cargo.toml` | matches, row for row (the W2 acceptance criterion) |
| `nexrad-binary-format.md` §8 RVOL layout vs. `parse/blocks.rs` | matches, field for field including the `major`/`minor` prefix |
| `nexrad-binary-format.md` §1 detection rules vs. `chunk.rs::detect_chunk_kind` | matches, including the E-before-I ordering rule |
| ADR-0012 `VolumeStatus` states vs. `types/volume_scan.rs` | matches (`InProgress`/`Complete`/`Superseded`/`TimedOut`) |
| Confirmed KDOX fixture values vs. `tests/decode_radial.rs` | matches |

Against that, eleven findings. Two are significant, and both are consequences of the
project's own success rather than of neglect: the last two sessions moved fast and left
their audit trail pointing at a tree that no longer exists.

| ID | Finding | Severity |
|---|---|---|
| DOC-01 | No `README.md`, and `CLAUDE.md` — the de facto project index — is gitignored | **high** |
| DOC-02 | `dependency-inventory.md` reads as current but describes a pre-remediation tree | **high** |
| DOC-03 | `utility/README.md` publishes the chunk-bucket key layout that E-11 disproved | medium |
| DOC-04 | `data-flow.md` documents the deleted `current_hour_anchor` polling behavior | medium |
| DOC-05 | `REQUIREMENTS.md` never absorbed Q15/Q16 from W8 | medium |
| DOC-06 | `overview.md` predates `crates/http-ingest` in both its structure tree and stack table | medium |
| DOC-07 | ADR-0016 pins `quick-xml = "0.37"`; the tree ships 0.41 | medium |
| DOC-08 | `rendering.md`'s polar grid spec contradicts confirmed super-resolution geometry | medium |
| DOC-09 | No document records the decoder test suite's distance from FR-ND-8 | medium |
| DOC-10 | `open-questions.md` has an empty Critical section and no resolution log | low |
| DOC-11 | ADR-0010 and ADR-0011 carry stale crate names / crate counts | low |

Nothing here is a correctness defect in shipped code. Every finding is a document that
would mislead someone acting on it.

---

## 2. What is in good shape

Recorded so the findings below are read in proportion.

- **ADR discipline is real.** All seventeen ADRs exist, all are `Accepted`, ADR-0013 is
  correctly marked superseded and retained. Every direct production dependency has one
  (verified: `nexrad-decoder` empty; `http-ingest`'s five all in ADR-0014's table;
  `radar-workstation`'s `bzip2`/`bytes`/`tokio`/`quick-xml` in ADRs 0015/0017/0004/0016).
- **ADR-0014 is maintained rather than archived.** The W2 sweep rewrote four body
  passages to match shipped code and left the erratum intact as a change log. Erratum
  item 8 goes further and corrects an earlier erratum item on new evidence. This is the
  behavior DOC-02 and DOC-07 are asking for elsewhere.
- **`docs/plans/dependency-inventory-remediation.md` is the best document in the tree.**
  Per-item acceptance status, measured before/after metrics, an honest footnote about
  figures that did not reproduce, and two unplanned findings (E-11, E-12) written up in
  full with the reasoning that produced them. Its §9 Results is the authoritative account
  of the current tree's state.
- **Code comments carry design rationale at the right density.** `s3_poll.rs:41-52`
  explains the volume-sequence key layout and why lexical ordering is unsafe across it;
  `Cargo.toml`'s `[profile.release]` explains each setting as a decision; `ci.yml`
  explains why there are no third-party actions. These are load-bearing and correct.
- **The binary format reference is production-grade.** 486 lines of confirmed byte
  offsets that the decoder demonstrably implements.

---

## 3. Findings

### DOC-01 — The repository has no front door, and its index is gitignored `high`

There is no `README.md` at the repository root. There is also no `CONTRIBUTING.md`,
`SECURITY.md`, or `docs/README.md`. `git ls-files` returns exactly four documentation
files outside `docs/`: `LICENSE`, `utility/README.md`, `utility/radar-viz/README.md`, and
`crates/http-ingest/fuzz/README.md`.

Separately and more sharply: **`CLAUDE.md` is excluded by `.gitignore:5`.** That file is
where the ADR index, the build commands, the current implementation status, the NEXRAD
format findings summary, and the "do not add dependencies without asking" instruction
actually live. A contributor cloning this repository receives none of it.

Most of the *content* survives elsewhere — `nexrad-binary-format.md` carries the format
findings, the ADRs carry their own decisions — so this is a discoverability failure, not
a knowledge loss. But it is the one that matters most here, because ADR-0009 and
Principle 8 make public auditability the stated reason the project is open source at all:

> *"A public codebase can be audited by security reviewers, evaluated by government
> procurement officers, studied by the meteorological community, and contributed to by
> domain experts."*

A security reviewer landing on this repository today finds a virtual Cargo manifest and
an undifferentiated `docs/` directory, with no statement of what the software is, how to
build it, what its security posture is, or where to start reading. The philosophy that
justifies the license is not reachable from the root.

What is missing, in priority order:

1. **`README.md`** — what this is, project status (decoder + ingest implemented, render
   layer design-only, `main.rs` a stub), build/test commands, a map into `docs/`, and the
   license. The status paragraph matters most: without it a reader sees a 4-line `main.rs`
   and cannot tell whether the project is abandoned or early.
2. **`docs/README.md`** — an index. Nine top-level documents, seventeen ADRs, and two
   plans currently have no entry point, and `docs/plans/` is referenced from no tracked
   document at all.
3. **`SECURITY.md`** — for a project explicitly courting government and defense review,
   a vulnerability disclosure path is table stakes. The supply-chain posture (`deny.toml`,
   pinned toolchain, SHA-pinned CI actions, no telemetry, host allowlist) is a genuine
   asset and is currently invisible.
4. **`CONTRIBUTING.md`** — ADR-0009 names community contribution as a goal. The rules
   that would govern it (ADR-before-dependency, DRY, the `utility/` boundary) exist only
   in the gitignored file.

Deciding whether `CLAUDE.md` itself should be tracked is a separate call — many projects
keep agent instructions private on purpose. The finding is not "track `CLAUDE.md`"; it is
that the repository has no tracked substitute for it.

*Effort: ~3 hours for all four documents. No code impact.*

### DOC-02 — `dependency-inventory.md` describes a tree that no longer exists `high`

The document is headed *"Audited commit `74a1065`, Date 2026-07-29"* and its own §8
correctly calls itself *"a point-in-time audit [that] will age out."* That framing would
make staleness acceptable. But the document **was subsequently edited** — findings E-11
and E-12 were appended on 2026-07-31, after the remediation work, and both are written in
the present tense of that later session. The result is a document that carries a 07-29
header, 07-31 content, and no marker separating them. A reader has no way to tell which
parts were refreshed.

They were not refreshed. Against the current tree:

| Passage | Claims | Actually |
|---|---|---|
| §2, `radar-workstation` table | `quick-xml` **0.37** | **0.41** — upgraded by E-12, in the same document |
| §2 executive summary line | `quick-xml = "0.37"` with no features | same |
| **E-04** "`default-members` is **still unset**" | bare build compiles 62 units | `Cargo.toml:8-12` sets it; W3 landed |
| **E-05** "reproducibility scaffolding **still absent**" | no `rust-toolchain.toml`, no `deny.toml`, **no `[profile.release]` anywhere**, `Cargo.lock` in `.gitignore` | all four shipped in W4. `rust-toolchain.toml` pins 1.95.0; `deny.toml` exists; `[profile.release]` has an argued `panic`/`lto`/`codegen-units`/`strip`/`overflow-checks` block; `Cargo.lock` removed from `.gitignore` |
| **E-06** "`image` still pulls two 0.x single-author crates" | `radar-viz` declares `image 0.25` | removed in W6; `utility/radar-viz/Cargo.toml` declares only `shapefile`. `png_out.rs` is the hand-rolled encoder |
| §7 scorecard, *Minimal dependencies* | "Still no `cargo-deny` config (E-05)" | `deny.toml` exists and `cargo deny check` is a CI step |
| §7 scorecard, *Reproducible builds* | "no toolchain pin, no `[profile.release]`, lockfile still listed in `.gitignore`" | none of the three is true |
| §8 recommended order of work | eight open items | **E-02, E-04, E-05, E-03, E-06, E-01 are done.** Only E-07 and E-09 remain, and both are now tracked as Q15/Q16 |

The severity is not that the facts are old — it is that E-04, E-05, and E-06 are written
as **open findings with recommended fixes**, and someone acting on §8 would re-do work
that has already landed, or file the same findings a second time. The E-11/E-12 additions
actively reinforce the misreading by making the document look live.

Two clean resolutions, either acceptable:

- **Freeze it.** Add a banner: *"Point-in-time audit of `74a1065`. Superseded by
  `docs/plans/dependency-inventory-remediation.md` §9 Results (2026-07-31). E-01…E-10
  reflect the pre-remediation tree."* Mark E-02/E-03/E-04/E-05/E-06 **Resolved** inline
  with a pointer to the work item, leave the analysis intact as the audit trail, and move
  E-11/E-12 into the remediation plan where they chronologically belong.
- **Re-audit.** Re-run §9's method against `d46042c` and publish an inventory of the
  current tree, retaining the old one as history.

The first is ~1 hour and preserves the record. Recommended. The second is the honest
choice only if the numbers are wanted fresh.

*Effort: ~1 hour (freeze) or ~4 hours (re-audit). No code impact.*

### DOC-03 — `utility/README.md` publishes the disproven chunk key layout `medium`

`utility/README.md:100` tells the reader that real-time chunks live at:

```
s3://unidata-nexrad-level2-chunks/<SITE>/<YYYY>/<MM>/<DD>/<HH>/
```

This is precisely the calendar layout that E-11 disproved by direct bucket inspection on
2026-07-31. The real layout is `SITE/<volume-sequence>/<YYYYMMDD-HHMMSS>-<n>-<kind>`. The
assumption was expensive the first time — it produced a shipping correctness bug in
`S3Poller` and a live measurement that returned 32,524 keys instead of a small backlog.

Every other location was corrected: `s3_poll.rs`, ADR-0014 erratum item 8, the remediation
plan §9.0, `CLAUDE.md`'s format findings, and `live_s3.rs`'s comments. This file was
missed, and it is the one a *new* contributor reads first when trying to fetch sample data
by hand — the exact path by which the assumption would recur. E-11's stated purpose was
*"so the assumption doesn't silently recur."*

Fix: correct the path, and state the volume-sequence property (unpadded, monotonic,
lexical ≠ numeric ordering) in one sentence so the reader knows why it matters.

*Effort: 10 minutes.*

### DOC-04 — `data-flow.md` documents deleted polling behavior `medium`

`docs/architecture/data-flow.md:91-103` describes the NEXRAD polling task:

> *"1. On startup, resolve the selected radar site identifier (e.g. KTLX) and **anchor the
> chunk listing to the current UTC hour**, so startup does not replay earlier chunks
> 2. Query the chunk bucket's ListObjectsV2 listing for **keys newer than the last seen key**"*

Step 1 describes `current_hour_anchor`, which was **deleted** in the E-11 fix as dead code
once the calendar assumption was gone. Step 2 describes a flat lexical `start-after` scan,
which `s3_poll.rs:41-52` explicitly documents as unsafe across the volume-sequence
directory.

What the code does: enumerate first-level `CommonPrefixes` with `delimiter=/`, parse them
numerically, anchor at `newest − 1` (`cold_start_baseline`), then use `start-after`
*within* one volume directory only. Sequence-numbered rather than clock-anchored.

This matters more than DOC-03 because `data-flow.md` is where someone goes to understand
the acquisition layer's design before reading its code, and the passage is confidently
wrong rather than merely vague. The document is otherwise well maintained — its
implementation-status callout at lines 40-44 is accurate and exactly the right convention.

Note the file already got one thing right that the code has not: line 101's "5 seconds"
matches `POLL_INTERVAL`.

*Effort: 20 minutes.*

### DOC-05 — `REQUIREMENTS.md` never absorbed Q15 and Q16 `medium`

W8 added two blocking open questions and its acceptance criterion was *"Q15 and Q16 in
`docs/open-questions.md` and reflected in `CLAUDE.md`."* Both were done. `REQUIREMENTS.md`
was not in scope and was not touched — but it is the document that maintains the
requirement-to-question mapping, and its §7 table is described as authoritative:

> *"The following requirements are explicitly incomplete pending resolution of open design
> questions… These must be resolved before implementation of the relevant subsystem begins."*

The table lists ten entries against Q5–Q14. Q15 and Q16 appear nowhere in the document,
and the requirements they block carry no `[OPEN]` marker:

| Requirement | Text today | Blocked by |
|---|---|---|
| FR-MU-1 | boundaries and highways "must be sourced from bundled … shapefiles" | **Q15** — whether a 0.x parser is on the startup path at all, or geometry is pre-projected at build time |
| FR-MU-2 | "must be pre-projected … at load time" | **Q15** — build-time pre-projection changes *when* this happens |
| FR-DA-6 | "must fetch map imagery tiles via HTTPS from the configured XYZ tile provider" | **Q16** — no client in the workspace can do this; `http-ingest`'s allowlist forbids it by design |
| FR-MU-4 | tile provider URL "must be user-configurable to any XYZ-scheme HTTPS tile source" | **Q16** — same |

FR-DA-6 and FR-MU-4 are the sharper half. They are stated as flat requirements, and
ADR-0014's scope boundaries make them **currently unimplementable** — arbitrary hosts,
redirects, and conditional requests are all explicit non-goals. An implementer working
from `REQUIREMENTS.md` alone would discover this only after starting.

Fix: add both rows to §7, and mark the four requirements `[OPEN — Q15]` / `[OPEN — Q16]`
in the house style already used elsewhere in the document.

*Effort: 20 minutes.*

### DOC-06 — `overview.md` predates `crates/http-ingest` `medium`

`docs/architecture/overview.md` is the stated entry point to the architecture directory.
Two sections are pre-ADR-0014:

- **Project structure tree (lines 11-35):** lists `crates/radar-workstation` and
  `crates/nexrad-decoder` only. `crates/http-ingest` — a 1,845-line first-party crate that
  is the subject of the workspace's longest ADR — is absent. The `docs/` subtree likewise
  omits `plans/` and `dependency-inventory.md`.
- **Technology stack table (lines 51-60):** eight rows, no HTTP client row. `CLAUDE.md`'s
  equivalent table has one; this, the tracked document, does not. A reader concludes the
  project has no HTTP boundary decision — the opposite of the truth.

The **Subsystem Overview** section has the same shape of gap: it describes UI, rendering,
data pipeline, compute, decoder, shared state, and basemap. The acquisition/HTTP boundary
is folded into "Data Pipeline" with no mention that it is an owned, separately-audited
crate.

Also worth a line while in the file: `overview.md` is the only architecture document with
no implementation-status callout. `data-flow.md` has an excellent one (lines 40-44).
Adding the equivalent here would put the single most useful fact — decoder and ingest
built, render layer design-only — at the top of the entry-point document.

*Effort: 30 minutes.*

### DOC-07 — ADR-0016 pins a version the tree does not ship `medium`

`docs/adr/0016-quick-xml.md:21` states the decision as `quick-xml = "0.37"`.
`crates/radar-workstation/Cargo.toml` declares `quick-xml = "0.41"`.

The upgrade was not cosmetic. E-12 raised it in response to RUSTSEC-2026-0194 and
-2026-0195, and 0.41 changed how entity references are surfaced from the event stream,
forcing a real fix in `parse_list_xml` (the `GeneralRef` accumulation path at
`s3_poll.rs:300-365`, with two regression tests guarding it).

This is exactly the E-02 failure mode, in a fresh instance: the authoritative section of
an ADR contradicts shipped code. The security reasoning that *drove* the bump — a
vulnerable pin dropped under Principle 4 even though the affected paths were unreachable —
is the kind of decision an ADR exists to record, and it is currently recorded only in an
audit finding and a plan's results section.

Fix: update the version in the Decision, and add a short note recording the advisory pair,
the reachability analysis, and the parser change the upgrade forced. Follow ADR-0014's
pattern.

*Effort: 30 minutes.*

### DOC-08 — `rendering.md`'s polar grid contradicts confirmed scan geometry `medium`

`docs/architecture/rendering.md:126`:

> *"The radar texture is generated on a polar coordinate grid matching the native NEXRAD
> resolution: **1km range gates × 1° azimuth bins** × 230km range."*

The confirmed KDOX data — read from the fixtures, asserted in `decode_radial.rs`, and
recorded in `CLAUDE.md` — is **0.250 km gate width** and **0.5° azimuthal spacing**. The
document's own claim of "matching the native NEXRAD resolution" is false against a factor
of four in range and two in azimuth.

Two documents contradict this one:

- **FR-ND-3** requires support for "both standard-resolution and super-resolution scan
  variants." A 1 km × 1° texture grid discards super-resolution at the render stage — the
  decoder would preserve it and the renderer would throw it away.
- **`nexrad-data-types.md`** describes ~120 radials per chunk covering ~100°, which is
  0.83°/radial and only consistent with super-resolution.

The renderer does not exist yet, so this costs nothing today. It costs a lot the day
someone implements the compute layer's texture generation from this line — the texture
dimensions are among the first decisions made and among the most expensive to revisit.
This is the clearest instance in the tree of a design document that would actively cause a
defect if implemented as written.

Fix: state the grid as 0.25 km × 0.5° for super-resolution with the standard-resolution
case as the fallback, and say which sweeps carry which (super-res is typically the lowest
cuts only). Q8's product-set resolution is the natural moment to settle it.

*Effort: 30 minutes, plus confirming the standard-resolution cut geometry against a fixture.*

### DOC-09 — nothing records the decoder test suite's distance from FR-ND-8 `medium`

FR-ND-8 and `data-flow.md`'s Testing section both specify what the decoder suite must
exercise. Measured against `crates/nexrad-decoder/tests/decode_radial.rs` (24 tests, all
green) and the five fixtures in `tests/fixtures/`:

| FR-ND-8 requires | Present |
|---|---|
| Known-good files, **multiple sites** | one site — KDOX only |
| **multiple scan modes** | one — VCP 35 (clear air) only |
| **multiple eras** | one — 2026-06-29 only |
| **Corrupt** input | none. One truncation test (`truncated_msg31_record_returns_error`) |
| Truncated input | partial — one case |
| **Dual-pol and non-dual-pol** variants | dual-pol only; no non-dual-pol fixture |
| **Super-res and standard-res** variants | super-res only |

The tests that exist are good — physical-value conversion, 16-bit product handling,
reserved-code handling, per-status geometry, gate geometry per moment. The gap is fixture
breadth, not test quality.

The finding is documentary, not a demand to write tests now. FR-ND-8 is a v1.0 requirement
and the decoder is early. What is missing is any document saying so. `CLAUDE.md` states
the decoder is "implemented and tested" without qualification; `data-flow.md`'s Testing
section is written in the present tense for a suite that does not yet meet it. Someone
reading either would reasonably believe FR-ND-8 is satisfied.

This matters against Principle 2 specifically. "Must never panic on malformed input"
(FR-ND-6, BC-6, NFR-ST-2) is currently supported by exactly one truncation test — whereas
`http-ingest`, on a comparable untrusted-input path, has a 31-file fuzz corpus gated on
stable `cargo test`. That asymmetry is worth naming: the precedent for how to test a
parser against hostile input already exists in this workspace, one crate over.

Fix: a status note in `data-flow.md`'s Testing section, or a short
`crates/nexrad-decoder/TESTING.md`, stating current fixture coverage and what FR-ND-8
still wants. Note also that `parse_radial_stream` silently skips non-Message-31 records —
so the `-S` chunk's metadata messages (2, 3, 5, 15, 18) are **not decoded**, meaning
ADR-0012's `VolumeContext` initialization and `nexrad-data-types.md`'s "Role in this
application" for `-S` chunks are unimplemented. That is expected at this stage and
correctly scoped, but it is stated nowhere.

*Effort: ~1 hour to write the status note. Fixture expansion is its own effort.*

### DOC-10 — `open-questions.md` has an empty Critical section and no resolution log `low`

Two small things in a document that is otherwise doing its job:

- **Lines 9-12:** the heading *"Critical — Must Resolve Before Implementation"* is
  followed by nothing. Either genuinely empty (say so — "None outstanding" is
  information) or something was removed without a trace.
- **Q1, Q2, Q3, and Q10 are absent** with no record of their resolution. The document's own
  preamble sanctions this (*"Remove a question when it is resolved"*), and Q2 (license →
  ADR-0009) and Q10 (projection → `rendering.md`) did land in ADRs as instructed. But
  Q1 and Q3 left no trace at all, and the numbering gaps invite the exact confusion W8
  already hit once — it planned to add "Q14 and Q15," found Q14 taken, and shipped Q15/Q16
  instead, leaving its own text needing a correction note.

Fix: a short "Resolved" section listing each closed question and where its decision lives.
Cheap, and it makes the numbering self-explanatory.

*Effort: 20 minutes.*

### DOC-11 — stale crate names and counts in ADR-0010 and ADR-0011 `low`

- **ADR-0011:100** — *"The decoder crate (`radar-decoder`)…"*. No such crate; it is
  `nexrad-decoder`. A rename that predates ADR-0010's naming.
- **ADR-0010** — the Decision lists exactly two crates under `crates/` and states *"All
  crates live under `crates/`."* Both are now out of date: `crates/http-ingest` is a third,
  and the workspace also has `utility/nexrad-sample` and `utility/radar-viz` as members
  (root `Cargo.toml:2-8`), plus an `exclude`d fuzz crate. The `utility/` split is a real
  structural decision — reinforced by `default-members` in W3 and by `utility/README.md`'s
  "not part of the product" boundary — and ADR-0010 is where it belongs.

Neither would mislead anyone badly. Both are one-line fixes and worth doing while the
neighboring documents are open. Note ADR-0014 already sets the precedent for how: correct
the body, keep the history.

*Effort: 20 minutes for both.*

---

## 4. Not findings

Assessed and deliberately excluded, so the absence is a judgment rather than an oversight.

- **`nexrad-data-types.md` and ADR-0012 reference a `VolumeContext` type that does not
  exist.** Both describe the volume assembly layer, which is design-only by CLAUDE.md's
  own status statement. Naming a type before building it is what a design document is for.
- **`overview.md`, `rendering.md`, and `data-flow.md` describe UI, GPU, compute, and
  shared-state layers that are not implemented.** Correctly framed as architecture.
  `data-flow.md`'s status callout is the model; DOC-06 asks for the same in `overview.md`.
- **`.github/workflows/ci.yml` is untracked.** A working-tree state, not a documentation
  gap — though note the remediation plan's §8 flags that it has never run on a real
  runner, so the first push is worth watching.
- **`utility/radar-viz/README.md` is three bare command lines with no trailing newline.**
  Genuinely trivial, and `utility/README.md` explicitly disclaims polish for dev tools
  (*"no stability guarantee… written for a single session's purpose"*). Left alone.
- **`docs/plans/0014-http-ingest-implementation.md` describes a completed effort.** Plans
  are historical by nature and this one is superseded by the shipped code plus ADR-0014's
  errata. Unlike DOC-02, it does not present itself as a current assessment.
- **NFR-SEC-4 (byte-identical reproducible builds) is unverified.** Real, but a
  verification gap, not a documentation one — the requirement is stated clearly and the
  scaffolding (pinned toolchain, tracked lockfile) is in place.

---

## 5. Recommended order of work

Ordered by the cost of leaving each one in place.

| # | Item | Why first | Effort |
|---|---|---|---|
| 1 | **DOC-03** — fix the chunk key layout in `utility/README.md` | The only finding that would reintroduce a *known, already-paid-for* bug. E-11's stated purpose was preventing exactly this recurrence. | 10 min |
| 2 | **DOC-02** — freeze `dependency-inventory.md` with a supersession banner and inline Resolved markers | Highest chance of causing duplicated work today; someone acting on §8 re-does W3/W4/W6. | 1 h |
| 3 | **DOC-04** — correct `data-flow.md`'s polling steps | Confidently wrong in the document an implementer reads before touching the acquisition layer. | 20 min |
| 4 | **DOC-01** — `README.md`, then `docs/README.md`, `SECURITY.md`, `CONTRIBUTING.md` | Largest gap, but nothing downstream breaks while it is open. Do the three quick corrections first so the README points at a consistent tree. | 3 h |
| 5 | **DOC-05, DOC-07, DOC-11** — requirement/ADR sweep | Small, mechanical, done together in one pass. | 1 h |
| 6 | **DOC-06** — `overview.md` structure, stack table, and a status callout | Natural companion to the README. | 30 min |
| 7 | **DOC-08** — resolve the polar grid geometry | No cost until the compute layer starts; high cost the day it does. Settle alongside Q8. | 30 min |
| 8 | **DOC-09** — record decoder test coverage vs. FR-ND-8 | Documents a known gap rather than closing one. | 1 h |
| 9 | **DOC-10** — resolution log in `open-questions.md` | Pure hygiene. | 20 min |

Total for items 1-3: **~1.5 hours**, and they remove every finding that could cause
someone to act on wrong information.

---

## 6. Method

Every tracked document in `docs/`, `utility/`, and the repository root was read in full
and checked against the code it describes. Claims about the code were verified by reading
the code, not by trusting a second document.

```sh
# what documentation exists, and what is tracked
git ls-files | grep -iE 'readme|contributing|security|\.md$'
git check-ignore -v CLAUDE.md .claude/      # -> .gitignore:5, :6

# test counts, against the remediation plan's final metrics table
cargo test --workspace --offline            # -> 123 passed, 0 failed, 12 ignored

# ADR-0014 Decision table vs. the manifest (the W2 acceptance check)
diff <(...ADR table crates...) <(...http-ingest/Cargo.toml [dependencies]...)

# stale calendar-layout claims, repository-wide
grep -rn -E 'YYYY/MM/DD|<YYYY>|current UTC hour|current_hour' docs/ utility/ crates/

# version drift between ADRs and manifests
grep -n 'quick-xml' docs/adr/0016-quick-xml.md crates/radar-workstation/Cargo.toml

# decoder coverage vs. FR-ND-8
ls crates/nexrad-decoder/tests/fixtures/
grep -E 'fn ' crates/nexrad-decoder/tests/decode_radial.rs
```

Binary-format claims were spot-checked structure by structure: `nexrad-binary-format.md`
§1 against `chunk.rs::detect_chunk_kind`, §8 (RVOL) against `parse/blocks.rs::parse_rvol`
including the `major`/`minor` version prefix, and the moment-block layout against
`parse/product.rs`. All matched.

One limitation: this audit did not dial out. Claims about live S3 behavior — the chunk
bucket's key layout, the 6 min 18 s volume production time, TLS 1.3 negotiation — are
taken from the remediation plan's §9 Results, which records them as measured on
2026-07-31. DOC-03 and DOC-04 rest on that record being correct; it is corroborated
independently by the shipped `S3Poller` code and by ADR-0014's erratum item 8.
