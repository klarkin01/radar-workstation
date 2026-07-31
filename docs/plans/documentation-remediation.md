# Implementation Plan — Documentation Remediation

**Status:** Implemented, 2026-07-30 (all work items W1–W9 landed in one session, plus one
unplanned finding fixed opportunistically — see §9 Results). Not yet committed to git;
see §9 for why.
**Drafted:** 2026-07-30
**Addresses:** `docs/documentation-inventory.md`, findings DOC-01 … DOC-11 (all eleven)
**Baseline commit:** `d46042c` (working tree: `.gitignore` modified, `.github/` untracked,
`docs/documentation-inventory.md` untracked)
**Toolchain:** rustc 1.95.0 / cargo 1.95.0, `x86_64-unknown-linux-gnu`

This plan is written to be executed in a later session. It carries the decisions already
taken so implementation does not need to re-litigate them. Where a decision is still open
it is marked **DECIDE** and states a recommendation — those, and only those, need the
project owner's input before the affected work item can proceed.

**Net code change across all nine work items: zero.** Every change is to a `.md` file.
That property is the primary verification gate (§7).

---

## 0. Before you start

Three things, in order. None takes more than a few minutes and all three have bitten
previous sessions on this repository.

1. **Read `docs/documentation-inventory.md` first.** It carries the evidence for every
   finding this plan remediates. This plan carries the fix; the inventory carries the
   argument for why it is a fix.

2. **Re-verify before editing. Do not trust this plan's line numbers.** They were
   accurate at `d46042c`. Every work item below states the *content* to look for, not
   just a line number — match on content. If a passage this plan describes is already
   corrected, mark that item **Not needed** in the §8 acceptance table and move on. Do
   not manufacture a change to close a row.

3. **Check whether `docs/documentation-inventory.md` and `.github/` are still
   untracked.** Both were at drafting time. Neither is this plan's business (§6), but a
   session that mistakes untracked files for missing ones will waste effort.

A note on standard, since this plan touches the documents that define it: this repository
holds documentation to the same bar as code. Claims are measured, not asserted; when a
document is corrected, the history of what it used to say is preserved rather than
deleted (ADR-0014's erratum pattern). Apply that bar to your own edits.

---

## 1. Scope

The inventory's §5 recommended order was **DOC-03, DOC-02, DOC-04, DOC-01, then the
sweep**. This plan adopts that order unchanged, and groups the remaining findings into
work items by the file they touch rather than by finding ID, so each commit is one
coherent diff.

| In scope | Findings |
|---|---|
| W1 — chunk key layout in `utility/README.md` | DOC-03 |
| W2 — freeze `dependency-inventory.md` | DOC-02 |
| W3 — `data-flow.md` polling steps | DOC-04 |
| W4 — the four missing root/index documents | DOC-01 |
| W5 — requirement and ADR sweep | DOC-05, DOC-07, DOC-11 |
| W6 — `overview.md` | DOC-06 |
| W7 — polar grid geometry | DOC-08 |
| W8 — decoder test coverage status | DOC-09 |
| W9 — open-questions resolution log | DOC-10 |

**Nothing is out of scope.** All eleven findings are addressed. Two work items (W4, W7)
contain a decision the project owner must make first — see §2.

### 1.1 Why this order

W1, W2, and W3 are first because they are the only findings that would cause someone to
*act on wrong information*: reintroducing a known bug (W1), redoing landed work (W2), or
building the acquisition layer against deleted behavior (W3). Together they are ~1.5
hours and they retire the entire "actively misleading" category.

W4 (the README set) is the largest gap but nothing downstream breaks while it is open,
and it should be written *after* W1–W3 so the new README points at a tree that is already
internally consistent. Writing the front door first means writing it twice.

W5–W9 are hygiene and can be done in any order, or dropped into a later session, without
affecting anything else.

---

## 2. DECIDE — questions for the project owner

Two work items are blocked until these are answered. Everything else can proceed
immediately. If you want to unblock the whole plan in one pass, answer all four.

### D-a (W4) — Should `CLAUDE.md` be tracked, or should a tracked substitute be written?

`CLAUDE.md` is excluded by `.gitignore:5`. It currently holds the ADR index, build
commands, implementation status, the NEXRAD format findings summary, and the
"ask before adding dependencies" instruction. Three options:

1. **Keep it ignored; write tracked substitutes** (README carries build commands + status,
   `docs/README.md` carries the index). Agent-specific instructions stay private. This is
   what the rest of W4 assumes.
2. **Track `CLAUDE.md` as-is.** One line removed from `.gitignore`. Cheapest, and makes
   the format findings and dependency rule public — but it also publishes instructions
   written for an AI agent as if they were contributor documentation, which they are not.
3. **Split it:** track the durable project knowledge (format findings, dependency rule,
   ADR index) as normal documentation; keep the agent-facing framing in an ignored file.

**Recommendation: option 1.** It is the only one where each document has a single
audience. The content that matters for public audit — format findings, ADRs, philosophy —
is already tracked; what is missing is an entry point, and an entry point should be
written for humans arriving at the repository, not adapted from agent instructions.

*Note there is no wrong answer here that this plan can detect. If you pick 2 or 3, W4's
`docs/README.md` step shrinks and its `README.md` step is unchanged.*

### D-b (W4) — What is the security disclosure contact for `SECURITY.md`?

A vulnerability disclosure path needs an address. Options: a dedicated address, your
existing contact address, or GitHub's private security advisory feature (repository
Settings → Security → private vulnerability reporting), which requires no address at all.

**Recommendation: enable GitHub private vulnerability reporting and point `SECURITY.md`
at it,** with no email in the file. It keeps a personal address out of a public repository
that is explicitly courting government and defense review, and it is the mechanism such a
reviewer will expect to find.

**Do not invent a contact address.** If this is unanswered when the session runs, write
`SECURITY.md` with everything except the reporting channel, leave a clearly marked
`<!-- TODO: reporting channel — see plan D-b -->`, and flag it in the §8 table.

### D-c (W2) — Freeze `dependency-inventory.md`, or re-audit it?

The inventory (DOC-02) recommends freezing: banner the document as point-in-time, mark
E-02/E-03/E-04/E-05/E-06 **Resolved** inline with pointers to the work item that closed
them, keep the analysis intact as the audit trail. ~1 hour. The alternative is re-running
`dependency-inventory.md` §9's method against the current tree and publishing a fresh
inventory, retaining the old one as history. ~4 hours.

**Recommendation: freeze.** The remediation plan's §9 Results already *is* the current-
state document, with measured numbers. A second inventory would duplicate it. Re-audit is
the right call only if you specifically want fresh dependency-graph numbers, which nothing
currently depends on.

W2 below is written for the freeze path. If you choose re-audit, W2 becomes "write
`docs/dependency-inventory-2.md` following the existing document's §9 method" and the
freeze banner is still needed on the original.

### D-d (W7) — How far does the polar grid correction go?

`rendering.md:126` states the radar texture grid as "1km range gates × 1° azimuth bins ×
230km range," described as "matching the native NEXRAD resolution." The confirmed data is
0.25 km gates and 0.5° azimuthal spacing. Two scopes:

1. **Correct the factual claim only.** State the measured super-resolution geometry,
   note that standard-resolution cuts differ, and record the *texture grid sizing*
   decision as a new open question rather than inventing an answer. ~30 min plus
   measurement.
2. **Correct it and decide the texture grid now.** Requires settling how super-res and
   standard-res sweeps share (or don't share) a texture, which interacts with Q8's
   product set and with the compute layer that does not exist yet.

**Recommendation: option 1.** Restraint is a feature: the renderer does not exist, and
fixing a texture format before the compute layer's requirements are concrete is the same
mistake the previous plan's §6 declined to make for the bundled overlay format. Correct
the wrong number now; decide the grid when someone is actually building it.

---

## 3. Decisions already taken (settled — do not reopen)

| # | Question | Decision |
|---|---|---|
| P-a | One commit per work item? | **Yes**, so a `git log` on `docs/` reads as one finding per commit and a revert is surgical. W4 may be two commits (root documents, then `docs/README.md`) if that reads better. |
| P-b | Delete superseded text, or correct in place with a pointer? | **Correct in place, preserve the history.** ADR-0014's erratum pattern is the house standard: the authoritative sections are made correct, and a dated note records what changed and why. Applies to W2, W5, W6, W7. |
| P-c | May work items change code? | **No.** Every finding is documentary. If you believe a code change is required, stop and record it as a new finding rather than folding it into a documentation commit. §7's first gate exists to catch this. |
| P-d | Should the README claim the application runs? | **No.** `main.rs` is a 4-line stub. The README states implementation status plainly. Overselling status in the front door of a project whose first principle is stability would be a poor trade for a project this early. |
| P-e | Scope of `CONTRIBUTING.md` | **Minimal and honest.** Build/test commands, the ADR-before-dependency rule, the `utility/` boundary, the DRY expectation, and where to start reading. Not a code-of-conduct, not a PR template, not a style guide — there is no `rustfmt.toml` and adopting one is a separate decision already deferred (previous plan, W5). |
| P-f | Does `docs/documentation-inventory.md` get committed? | **Yes**, in W1's commit or its own. It is the evidence base this plan cites throughout; a plan referencing an untracked document is not reviewable. |

---

## 4. Order of work

| # | Item | Findings | Effort | Blocked on |
|---|---|---|---:|---|
| **W1** | Chunk key layout in `utility/README.md` | DOC-03 | 10 min | — |
| **W2** | Freeze `dependency-inventory.md` | DOC-02 | 1 h | **D-c** |
| **W3** | `data-flow.md` polling steps | DOC-04 | 20 min | — |
| **W4** | `README.md`, `docs/README.md`, `SECURITY.md`, `CONTRIBUTING.md` | DOC-01 | 3 h | **D-a, D-b**; W1–W3 first |
| **W5** | Requirement and ADR sweep | DOC-05, DOC-07, DOC-11 | 1 h | — |
| **W6** | `overview.md` structure, stack table, status callout | DOC-06 | 30 min | W4 (consistency) |
| **W7** | Polar grid geometry | DOC-08 | 30 min + measurement | **D-d** |
| **W8** | Decoder test coverage status | DOC-09 | 1 h | — |
| **W9** | `open-questions.md` resolution log | DOC-10 | 20 min | W7 (may add a question) |

Only three orderings are real: W1–W3 before W4 (so the README describes a consistent
tree), W4 before W6 (so `overview.md`'s status callout and the README agree), and W7
before W9 (W7 may add an open question that W9's log should reflect). Everything else is
independent.

**Total: ~7 hours.** Items 1–3 are ~1.5 hours and retire every finding that could cause
someone to act on wrong information; they are worth doing even if the rest slips.

---

## 5. Work items

### W1 — Chunk key layout in `utility/README.md` (DOC-03)

**Why first:** this is the only finding that would reintroduce a bug the project has
already paid for. E-11 cost a shipping correctness defect in `S3Poller` and a live
measurement that returned 32,524 keys instead of a small backlog; its stated purpose was
*"so the assumption doesn't silently recur."* Every other location was corrected. This one
was missed, and it is what a new contributor reads when fetching sample data by hand.

**File:** `utility/README.md`, "Data files" section, the Unidata bullet (~lines 96-102).

Current text asserts:

```
s3://unidata-nexrad-level2-chunks/<SITE>/<YYYY>/<MM>/<DD>/<HH>/ for real-time chunks
```

Replace with the confirmed layout, and state the property that makes it non-obvious:

- Chunks: `s3://unidata-nexrad-level2-chunks/<SITE>/<volume-sequence>/<YYYYMMDD-HHMMSS>-<n>-<kind>`,
  e.g. `KDOX/166/20260728-095259-001-S`.
- `<volume-sequence>` is an **unpadded**, monotonically increasing per-site integer, one
  per volume scan. It is not derivable from wall-clock time, and its lexical order does
  not match numeric order across digit widths (`"78"` sorts after `"709"`) — so listing
  the bucket flatly and sorting by key does not give chronological order.
- Chunks persist 24 hours.
- The archive bucket's layout is different and unrelated: `YYYY/MM/DD/SITE/...`. Leave
  the existing archive line alone — it is correct.

Cross-reference `docs/architecture/nexrad-binary-format.md` for the file-level format and
ADR-0014 erratum item 8 for the provenance of the key-layout correction.

**Verification:** `grep -n 'YYYY' utility/README.md` returns only the archive-bucket line.

**Also in this commit (P-f):** `git add docs/documentation-inventory.md`.

---

### W2 — Freeze `dependency-inventory.md` (DOC-02) — *blocked on D-c*

Written for the freeze path. If D-c chose re-audit, see §2.

The document is headed `74a1065` / 2026-07-29 and calls itself point-in-time, but findings
E-11 and E-12 were appended on 07-31 with no marker separating them. It therefore reads as
live while describing a pre-remediation tree, and three of its findings are written as
open with recommended fixes that have already shipped.

**Three edits, in this order.**

**W2a — Banner.** Immediately after the existing header block (audited commit / date /
toolchain / scope), add a clearly-set-off note:

> **Superseded — point-in-time audit.** This document assesses the tree at `74a1065`
> (2026-07-29). Findings E-01…E-10 describe the **pre-remediation** tree and are retained
> as the audit trail, not as current state. The remediation that closed most of them is
> `docs/plans/dependency-inventory-remediation.md`; its §9 Results is the authoritative
> account of the current dependency posture. Findings E-11 and E-12 were appended
> 2026-07-31, after that remediation, and are the only content here written against the
> post-remediation tree.

That last sentence is the load-bearing one — it is what resolves the ambiguity a reader
currently cannot resolve.

**W2b — Mark resolved findings inline.** For each of the five findings below, add a
one-line status marker directly beneath the finding's heading. Do **not** rewrite the
analysis bodies; they are the record of what was true and why it mattered.

| Finding | Marker to add | Verify against |
|---|---|---|
| E-02 (ADR/erratum `httparse` disagreement) | **Resolved** by W2 of the remediation plan | ADR-0014's Decision table vs. `crates/http-ingest/Cargo.toml` |
| E-03 (serialized chunk fetches) | **Resolved — measured, not over budget.** Fan-out not applied | remediation plan §9.1, §9.2 |
| E-04 (`default-members` unset) | **Resolved** by W3 | root `Cargo.toml` `default-members` |
| E-05 (reproducibility scaffolding) | **Resolved** by W4 — all four pieces | `rust-toolchain.toml`, `deny.toml`, `[profile.release]`, `.gitignore` |
| E-06 (`image` pulls 0.x crates) | **Resolved** by W6 — hand-rolled encoder | `utility/radar-viz/Cargo.toml`, `src/png_out.rs` |

**Check each one against the tree before marking it**, per §0.2 — do not copy this table's
verdicts on faith.

E-01, E-07, E-08, E-09 stay open and unmarked. E-07 and E-09 are now also tracked as Q15
and Q16; add a pointer to that effect so a reader knows where the live version lives.

**W2c — Correct the two stale facts and the scorecard.** These are factual errors rather
than stale findings, so correct them in place with a dated inline note (P-b):

- §2 `radar-workstation` dependency table: `quick-xml` **0.37 → 0.41**.
- §1 executive summary, D-06 row: the same `quick-xml = "0.37"` reference.
- §7 scorecard, *Minimal dependencies, auditable* row: "Still no `cargo-deny` config
  (E-05)" is false — `deny.toml` exists and `cargo deny check` runs in CI.
- §7 scorecard, *Reproducible builds* row: "no toolchain pin, no `[profile.release]`,
  lockfile still listed in `.gitignore`" — all three are false.
- §8 recommended order of work: add a line stating that items 1, 2, 3, 4, 7, and 8 are
  complete and only E-07 / E-09 remain (now Q15 / Q16).

**Optional, and a judgment call:** the inventory suggests moving E-11/E-12 into the
remediation plan where they chronologically belong. The banner (W2a) already resolves the
ambiguity, and moving them breaks inbound references from the plan's §7 acceptance table.
**Recommend leaving them in place.**

**Verification:** no sentence in the document asserts a fact contradicted by the tree at
HEAD. Mechanically: every version number named in the document matches the corresponding
`Cargo.toml`.

---

### W3 — `data-flow.md` polling steps (DOC-04)

**File:** `docs/architecture/data-flow.md`, "NEXRAD Polling Task" numbered block
(~lines 89-103).

Steps 1 and 2 describe `current_hour_anchor` — deleted as dead code during the E-11 fix —
and a flat lexical `start-after` scan, which `s3_poll.rs:41-52` documents as unsafe across
the volume-sequence directory.

Rewrite steps 1–2 to describe what `S3Poller::poll_once` actually does. The authoritative
source is `crates/radar-workstation/src/ingest/s3_poll.rs` — **read it, do not paraphrase
this plan.** The shape:

1. On startup, resolve the site identifier and list the site prefix with `delimiter=/`
   (`list_volume_folders`) to enumerate volume-sequence directories as `CommonPrefixes`,
   parsed **numerically**.
2. Anchor cold start at `newest − 1` (`cold_start_baseline`), so the first poll fetches
   the current volume rather than replaying the 24-hour retention window.
3. Each poll, list the single volume directory `last_completed_volume + 1`, using
   `start-after` **within that directory only** — where fixed-width filenames make lexical
   order chronological.
4. Classify each key by its `-S` / `-I` / `-E` suffix and fetch bodies sequentially
   (ADR-0014: one keepalive connection, no pool).
5. On seeing an `-E`, advance to the next volume-sequence directory.

Steps 4–8 of the existing block (decompress, decode, feed the assembler, sleep) are
correct — the decompress/decode/assemble description matches `chunk.rs` and
`parse_radial_stream`, and the 5-second interval matches `POLL_INTERVAL`. Leave them.

**Two things to preserve while editing:**

- The document's implementation-status callout (~lines 40-44) is accurate and is the
  convention W6 will copy into `overview.md`. Do not disturb it.
- `s3_poll.rs`'s known accepted gap — if a volume-sequence directory never appears (an RDA
  restart skipping numbers, observed live), the poller stalls waiting on it. This is
  recorded in the code and in the previous plan's §8 as wanting its own issue. Add one
  sentence to `data-flow.md` noting it, so the design document does not describe a
  failure-free loop that the code knows is not failure-free.

**Verification:** no reference to hour-anchoring survives in `data-flow.md`; the described
sequence matches `poll_once` step for step.

---

### W4 — The four missing documents (DOC-01) — *blocked on D-a, D-b; do W1–W3 first*

The largest item. Four documents; write them in this order, since each informs the next.

Written assuming **D-a option 1** (keep `CLAUDE.md` ignored, write tracked substitutes).

#### W4a — `README.md` (repository root)

The front door. ADR-0009 and Principle 8 make public auditability the stated reason this
project is open source; today a reviewer arrives at a virtual Cargo manifest and an
undifferentiated `docs/` directory.

Sections, in order:

1. **Name and one-line description.** "Radar Workstation, Meteorological — a single-site
   NEXRAD Level II radar analysis application for Linux." One short paragraph on who it is
   for (storm chasers, NWS staff, emergency managers, during active severe weather) and
   what the reference application is (GR2Analyst).
2. **Status — and be plain about it (P-d).** This is the most useful paragraph in the
   document. Something close to:
   - Implemented and tested: the NEXRAD decoder (Message 31), the workspace-local HTTP/1.1
     client (`crates/http-ingest`), the chunk ingest layer (S3 polling, chunk detection,
     BZ2 decompression).
   - Design-only: volume assembly (ADR-0012), compute layer, shared app state, render loop.
   - **`main.rs` is a stub. There is no runnable application yet.** Say it in those words.
     A reader who sees a 4-line `main.rs` and no status statement cannot tell early from
     abandoned.
   - Point at `utility/` for what *is* runnable today (`fetch-sample`, `decode-sample`,
     `radar-viz`), noting they are dev tools with no stability guarantee.
3. **Build and test.** Lift verbatim from `CLAUDE.md`'s Build Commands — and verify each
   one runs clean before committing:
   ```
   cargo build --release
   cargo test --workspace
   cargo clippy --workspace --all-targets -- -D warnings
   cargo deny check
   cargo audit
   ```
   Note the `--workspace` requirement explicitly: `default-members` scopes the bare
   commands to the three production crates, which is deliberate but surprising.
   `cargo-deny` and `cargo-audit` need `cargo install --locked` — say so, with the
   versions CI pins.
4. **Repository layout.** Short table: `crates/radar-workstation`, `crates/nexrad-decoder`,
   `crates/http-ingest`, `utility/`, `docs/`. One line each. Must include `http-ingest` —
   its omission is exactly DOC-06's failure mode.
5. **Documentation map.** Four or five links, not a directory listing: `docs/PHILOSOPHY.md`
   (start here), `docs/REQUIREMENTS.md`, `docs/architecture/overview.md`, `docs/adr/`,
   `docs/open-questions.md`. Defer the full index to `docs/README.md`.
6. **Security posture.** Three or four bullets, then a link to `SECURITY.md`. The posture
   is a genuine asset and is currently invisible: no telemetry; no network connection the
   user has not configured; compile-time host allowlist on the S3 client; pinned toolchain
   and tracked lockfile; `cargo-deny` and `cargo-audit` gated in CI; SHA-pinned CI actions
   with no third-party actions beyond `checkout`; memory-safe by construction with `ring`
   the only non-Rust code in the production graph.
7. **License.** Apache-2.0, link to `LICENSE`.

**Do not** include screenshots, badges, or a roadmap. There is nothing to screenshot, CI
has not run on a real runner yet, and the roadmap is `docs/open-questions.md`.

#### W4b — `docs/README.md`

An index. Nine top-level documents, seventeen ADRs, and two plans currently have no entry
point, and `docs/plans/` is referenced from no tracked document at all.

- A short "start here" ordering: PHILOSOPHY → REQUIREMENTS → architecture/overview → ADRs.
- One-line description per document in `docs/` and `docs/architecture/`.
- **The ADR index** — number, title, status. Port it from `CLAUDE.md` and verify each
  entry against the file, including ADR-0013's superseded status. Under D-a option 1 this
  index exists nowhere tracked today; that is the single most valuable thing in this file.
- `docs/plans/` — explain what a plan document is (an executable work plan, retained after
  execution as the record of what was done and measured) and list the two, plus this one.
- `docs/dependency-inventory.md` and `docs/documentation-inventory.md` — describe both as
  point-in-time audits and note W2's supersession banner.

#### W4c — `SECURITY.md` — *see D-b*

Root of the repository, where GitHub looks for it.

- **Reporting channel** (D-b). If unanswered: write everything else and leave a marked
  TODO.
- **Scope:** what is in the threat model. The untrusted-input paths are worth naming
  precisely, because they are where a reviewer will look: NEXRAD chunk bytes from S3
  (decoder, BZ2 decompression), S3 `ListObjectsV2` XML responses, HTTP response framing.
- **Posture summary**, cross-referencing ADR-0014 (owned HTTP boundary, compile-time host
  allowlist, ALPN pinned to HTTP/1.1), ADR-0008 (zero-dependency decoder), ADR-0015
  (pure-Rust BZ2 on the attacker-influenced path), and the `[profile.release]`
  `overflow-checks = true` decision.
- **Supported versions:** pre-1.0, no released versions, `main` only. Say so.
- **What is out of scope:** the `utility/` directory is explicitly non-production
  (`utility/README.md`), and the fuzz crate is workspace-excluded.

#### W4d — `CONTRIBUTING.md`

Minimal and honest (P-e).

- How to build and test (link to README rather than duplicating — DRY applies to prose).
- **The dependency rule**, stated prominently: no new dependency without an ADR. This is
  the project's most distinctive constraint, it currently lives only in an ignored file,
  and a contributor who violates it has to redo their work.
- **The `utility/` boundary:** nothing in `utility/` is imported by any crate in
  `crates/`; logic that belongs in the product gets reimplemented in Rust there.
  `utility/README.md` already states this well — link, don't restate.
- **DRY**, per Principle 4.
- **What to read before proposing an architectural change:** the relevant ADR, then
  `PHILOSOPHY.md`.
- **No formatting requirement.** There is no `rustfmt.toml`, adopting one is deliberately
  deferred (previous plan, W5), and the existing code intentionally departs from default
  rustfmt in places. Say this — otherwise a well-meaning contributor runs `cargo fmt` and
  produces a tree-wide diff.
- **Note that CI has never run on a real GitHub Actions runner.** Honest, and it sets
  expectations for the first external PR.

**Verification for W4:** every relative link resolves; every command in the README runs
clean; the ADR index matches `ls docs/adr/` exactly; no document claims the application
runs.

---

### W5 — Requirement and ADR sweep (DOC-05, DOC-07, DOC-11)

Three small corrections across four files. One commit.

**W5a — `REQUIREMENTS.md` absorbs Q15 and Q16 (DOC-05).**

W8 of the previous plan added both questions to `open-questions.md` and `CLAUDE.md` but
not to `REQUIREMENTS.md`, which is the document that maintains the requirement-to-question
mapping and describes its §7 table as authoritative.

Add `[OPEN — Qn]` markers, in the existing house style, to four requirements:

| Requirement | Marker | Because |
|---|---|---|
| FR-MU-1 | `[OPEN — Q15]` | whether a 0.x shapefile parser is on the startup path at all, or geometry is pre-projected at build time |
| FR-MU-2 | `[OPEN — Q15]` | build-time pre-projection changes *when* projection happens |
| FR-DA-6 | `[OPEN — Q16]` | no client in the workspace can fetch from arbitrary hosts; ADR-0014 makes it an explicit non-goal |
| FR-MU-4 | `[OPEN — Q16]` | same |

Then add two rows to the §7 Open Requirements table: `FR-MU-1` / `FR-MU-2` → Q15,
`FR-DA-6` / `FR-MU-4` → Q16.

FR-DA-6 and FR-MU-4 are the sharper half — they are currently stated as flat requirements
that are **unimplementable** against the accepted ADR set. Make sure the marker text says
so rather than merely pointing at a question number.

**W5b — ADR-0016 version (DOC-07).**

`docs/adr/0016-quick-xml.md` states the decision as `quick-xml = "0.37"`;
`crates/radar-workstation/Cargo.toml` declares `0.41`. This is the E-02 failure mode in a
fresh instance: an ADR's authoritative section contradicting shipped code.

- Update the version in the Decision.
- Add a short dated note (ADR-0014's pattern) recording *why*: RUSTSEC-2026-0194 and
  RUSTSEC-2026-0195 against 0.37.5; the reachability analysis that found neither advisory's
  affected path reachable (`parse_list_xml` never calls `.attributes()`, never uses
  `NsReader`); the decision to upgrade anyway under Principle 4 rather than file an
  exemption; and that 0.41 changed entity-reference handling, forcing the `GeneralRef`
  accumulation fix in `parse_list_xml` with two regression tests guarding it.

Full account is in the previous plan's §9.3 — read it rather than working from this
summary.

**W5c — Stale crate names and counts (DOC-11).**

- `docs/adr/0011-chunk-stream-data-source.md`, Consequences, final paragraph: *"The decoder
  crate (`radar-decoder`)"* → `nexrad-decoder`.
- `docs/adr/0010-workspace-structure.md`, Decision: lists two crates under `crates/` and
  states *"All crates live under `crates/`."* Both are now wrong. Add `crates/http-ingest`
  (ADR-0014), and record the `crates/` vs. `utility/` split as the structural decision it
  has become — reinforced by `default-members` and by `utility/README.md`'s
  "not part of the product" boundary. Mention the workspace-`exclude`d fuzz crate in a
  clause. Correct in place with a dated note (P-b).

**Verification:** every version pinned in an ADR Decision matches the corresponding
`Cargo.toml`; no ADR names a crate that does not exist; §7's table in `REQUIREMENTS.md`
lists every `[OPEN — Qn]` marker in the document, and vice versa.

---

### W6 — `overview.md` (DOC-06) — *after W4*

`docs/architecture/overview.md` is the stated entry point to the architecture directory
and is pre-ADR-0014 in three places.

- **Project structure tree** (~lines 11-35): add `crates/http-ingest/` with a one-line
  gloss ("workspace-local HTTP/1.1 client — ADR-0014"). Add `docs/plans/` and
  `docs/dependency-inventory.md`; add `docs/documentation-inventory.md` if W1 committed it.
- **Technology stack table** (~lines 51-60): add an HTTP client row —
  *Custom HTTP/1.1 implementation (`crates/http-ingest`), ADR-0014; no third-party HTTP
  client.* `CLAUDE.md`'s equivalent table has this row; the tracked document does not, so a
  reader concludes there is no HTTP boundary decision.
- **Subsystem Overview:** the acquisition/HTTP boundary is currently folded into "Data
  Pipeline" with no mention that it is an owned, separately-audited crate. Add a short
  subsection, or extend the Data Pipeline entry, noting it is first-party code with its own
  fuzz corpus and threat model.
- **Add an implementation-status callout** near the top. `data-flow.md`'s (~lines 40-44) is
  the model — copy its shape. This puts the single most useful fact in the entry-point
  document. Keep it consistent with W4a's status paragraph; if they drift, the README wins.

**Verification:** the structure tree matches `ls crates/` and `ls docs/`; the stack table
has a row for every technology named in the ADR index.

---

### W7 — Polar grid geometry (DOC-08) — *blocked on D-d*

Written for **D-d option 1** (correct the factual claim; defer the texture-grid decision).

`rendering.md:126` states the radar texture grid as "1km range gates × 1° azimuth bins ×
230km range," described as "matching the native NEXRAD resolution." That is off by a factor
of four in range and two in azimuth against confirmed data, and it contradicts FR-ND-3
(both resolution variants) and `nexrad-data-types.md` (~120 radials per ~100°, which is
only consistent with super-resolution).

**W7a — Measure, don't assert.** This is a documentation fix, but the numbers going in must
be measured, matching the standard the previous plan set. Derive per-sweep geometry from
the fixtures and the decoder rather than from memory or from `CLAUDE.md`'s summary table:

- `crates/nexrad-decoder/tests/fixtures/` has five KDOX VCP 35 chunks.
- `ProductData` exposes `gate_count`, `first_gate_m`, `gate_width_m` per moment;
  `Radial::azimuth_deg` and `azimuth_number` give azimuthal spacing.
- `utility/nexrad-sample`'s `decode-sample` binary, or a throwaway test, will print them.
  If you write a throwaway, do not commit it.

Record: gate width, first gate, gate count, and implied maximum range **per moment**
(reflectivity and velocity differ), plus azimuthal spacing. Note that
`1832 gates × 0.25 km` puts reflectivity well beyond the document's stated 230 km — so the
range figure needs checking too, not just the resolution figures.

**Known limitation, state it in the edit:** all five fixtures are KDOX VCP 35, super-
resolution. There is **no standard-resolution fixture in the repository**, so the
standard-res geometry cannot be measured here. Do not assert it from memory. Either cite
ICD 2620002 explicitly, or write the standard-res case as "not yet confirmed against
sample data" — the second is preferable and is the honest option. This overlaps DOC-09's
fixture-coverage gap; note the connection.

**W7b — Correct `rendering.md`.** Replace the grid sentence with the measured
super-resolution geometry, state that standard-resolution cuts differ and are not yet
confirmed here, and remove or correct the "matching the native NEXRAD resolution" claim
and the 230 km figure. Cross-reference `nexrad-binary-format.md`.

**W7c — Record the deferred decision.** The *texture grid sizing* question — how super-res
and standard-res sweeps share, or don't share, a texture — is now explicit and unanswered.
Add it to `docs/open-questions.md` under **Rendering**, as the next free number (**Q17**
at drafting time — **verify this before using it**; W8 of the previous plan was burned by
exactly this assumption and had to ship a correction note). Frame it as blocking the
compute layer's texture generation, and cross-reference Q8 (product set) and FR-ND-3.

**Verification:** every number in the edited passage traces to a measurement recorded in
§9 of this plan, or to a cited ICD section. No unattributed figures.

---

### W8 — Decoder test coverage status (DOC-09)

Nothing currently records how far the decoder suite is from FR-ND-8. `CLAUDE.md` says
"implemented and tested" without qualification, and `data-flow.md`'s Testing section is
written in the present tense for a suite that does not yet meet it.

**This item documents a gap. It does not close it.** Do not write fixtures or tests here —
that is its own effort with its own sample-data acquisition problem.

**W8a — Re-measure the coverage table.** The inventory's DOC-09 table was accurate at
`d46042c`, but re-derive it from `ls crates/nexrad-decoder/tests/fixtures/` and the test
names rather than copying. Axes: sites, VCPs/scan modes, eras, corrupt input, truncated
input, dual-pol vs. non-dual-pol, super-res vs. standard-res.

**W8b — Write the status note.** **DECIDE — location.** Either a short
`crates/nexrad-decoder/TESTING.md`, or a status callout in `data-flow.md`'s Testing
section. **Recommend `TESTING.md`**, with a one-line pointer from `data-flow.md`: it
lives next to the tests it describes, so it is more likely to be updated when fixtures are
added, and `data-flow.md` is about data flow rather than test inventory.

Contents:

- What the 24 tests cover, honestly — physical-value conversion, 16-bit product handling,
  reserved-code handling, per-status geometry, per-moment gate geometry. The tests are
  good; the gap is fixture breadth.
- The FR-ND-8 delta, as a table.
- **The asymmetry worth naming:** "must never panic on malformed input" (FR-ND-6, BC-6,
  NFR-ST-2) is currently supported by one truncation test, while `http-ingest` — a
  comparable untrusted-input path, one crate over — has a 31-file fuzz corpus gated on
  stable `cargo test` (`response.rs`'s `fuzz_corpus_never_panics` and
  `mutated_inputs_never_panic`). The precedent for how to test a parser against hostile
  input already exists in this workspace. Point at it.
- **That `parse_radial_stream` silently skips non-Message-31 records**, so the `-S` chunk's
  metadata messages (2, 3, 5, 15, 18) are not decoded — meaning ADR-0012's `VolumeContext`
  initialization and `nexrad-data-types.md`'s "Role in this application" for `-S` chunks
  are unimplemented. Correctly scoped for this stage, but currently stated nowhere.

**W8c — Qualify the status claims.** Add a clause to `data-flow.md`'s Testing section
noting current coverage and pointing at `TESTING.md`. If D-a resulted in `CLAUDE.md` being
tracked, qualify its "implemented and tested" line too.

**Verification:** the table matches the fixtures and test names on disk; no claim that
FR-ND-8 is satisfied survives anywhere.

---

### W9 — `open-questions.md` resolution log (DOC-10) — *after W7*

Two small things in a document that is otherwise doing its job.

- **The empty "Critical — Must Resolve Before Implementation" section** (~lines 9-12): a
  heading followed by nothing. Either write "None outstanding." — which is information —
  or, if something was removed, restore it. Check `git log -p docs/open-questions.md`
  before assuming it was always empty.
- **Add a "Resolved" section.** Q1, Q2, Q3, and Q10 are absent with no record. The
  document's preamble sanctions removal, and Q2 (license → ADR-0009) and Q10 (projection →
  `rendering.md`) did land where instructed — but Q1 and Q3 left no trace at all, and the
  numbering gaps invite exactly the confusion the previous plan's W8 hit when it assumed
  Q14 was free, found it taken, and shipped Q15/Q16 with a correction note.

  Recover what you can from `git log -p docs/open-questions.md`. For questions whose
  resolution cannot be recovered, say so — "Q1: removed before 2026-07-29; resolution not
  recorded" is more useful than silence, and it is honest.

  Add a line to the preamble instructing that closed questions move to this section rather
  than being deleted.

**If W7 added Q17**, make sure it appears under Rendering and that the resolution log's
numbering narrative accounts for it.

**Verification:** every question number from Q1 to the highest in use appears either as an
open question or in the resolution log.

---

## 6. What this plan does not do

Stated so the boundaries are explicit rather than inferred.

- **Does not change any code.** All nine items are `.md` edits. See §7's first gate.
- **Does not write decoder tests or fixtures** (W8 documents the gap; closing it is its
  own effort, gated on acquiring sample data across sites, VCPs, and eras).
- **Does not decide the radar texture grid** (W7, per D-d option 1). It records the
  question.
- **Does not resolve Q15 or Q16.** W5 makes `REQUIREMENTS.md` reflect that they block four
  requirements. Answering them is design work with its own ADRs.
- **Does not commit `.github/`** or run CI. The workflow is untracked at baseline and its
  first real run is tracked in the previous plan's §8.
- **Does not adopt `rustfmt`.** Deferred with reasons in the previous plan's W5. W4d
  documents the absence as a choice.
- **Does not re-audit the dependency graph** (D-c chose freeze; if it chose re-audit, that
  is W2's scope and this line does not apply).
- **Does not restructure `docs/`.** No files move. The information architecture is fine;
  what is missing is an index.

---

## 7. Verification gates

Run at the end of every work item.

```bash
# GATE 1 — the defining property of this plan: no code changed.
git diff --stat <baseline>..HEAD
#   Expect: only *.md files. Any .rs, .toml, .yml, or .lock in the diff means
#   a work item exceeded its scope (P-c). Stop and reassess.

# GATE 2 — nothing broke, which given GATE 1 should be tautological.
cargo test --workspace                 # expect 123 passed / 0 failed / 12 ignored
cargo clippy --workspace --all-targets -- -D warnings

# GATE 3 — stale-pattern sweep.
grep -rn -E 'YYYY/MM/DD|<YYYY>/<MM>/<DD>|current UTC hour|current_hour_anchor' \
    docs/ utility/ --include='*.md'
```

**GATE 3 will not return zero hits, and must not.** Surviving hits are legitimate and
must be individually confirmed as historical:

| Expected surviving hit | Why it is correct |
|---|---|
| `docs/adr/0014-*.md` erratum items 6, 8 | Item 6 is the historical record; item 8 corrects it. Deleting either destroys the audit trail. Item 6's *archive*-bucket claim (`YYYY/MM/DD/SITE/...`) is separately still true. |
| `docs/plans/dependency-inventory-remediation.md` | Historical record of the bug and its fix. |
| `docs/dependency-inventory.md` E-11 | Same. |
| `docs/documentation-inventory.md` DOC-03/DOC-04 | Quotes the stale text as evidence. |

The gate fails only if a hit survives in `utility/README.md` or
`docs/architecture/data-flow.md` — the two documents W1 and W3 correct.

```bash
# GATE 4 — link integrity (W4 and W6 onward).
#   Every relative markdown link resolves to a file that exists.
#   Grep out link targets and test each with `test -e`; no external tool needed.

# GATE 5 — ADR index accuracy (W4b onward).
ls docs/adr/                           # must match docs/README.md's index exactly

# GATE 6 — version agreement (W2, W5 onward).
#   Every version pinned in an ADR Decision matches the corresponding Cargo.toml.
grep -rn 'quick-xml\|bzip2\|rustls\|tokio\|bytes' docs/adr/*.md
```

### Metrics to record

Fill in as work lands.

| Metric | Baseline `d46042c` | Final |
|---|---:|---:|
| Tracked `.md` files outside `docs/` | 4 | 4 committed at HEAD, unchanged — plus `README.md`, `SECURITY.md`, `CONTRIBUTING.md` written this session and not yet staged/committed (see §9) |
| `README.md` at repository root | absent | present |
| Documents in `docs/` with no inbound link from any tracked file | `plans/` (both), `dependency-inventory.md` | none — `docs/README.md` now links every top-level document, every ADR, and every plan |
| Findings in `dependency-inventory.md` written as open but already resolved | 5 (E-02…E-06) | 0 — all five marked Resolved inline; E-01 and E-08 confirmed still accurately open |
| Requirements blocked by an open question with no `[OPEN]` marker | 4 (FR-DA-6, FR-MU-1, FR-MU-2, FR-MU-4) | 0 — all four marked, and added to §7's table |
| ADRs whose Decision contradicts a shipped manifest | 1 (ADR-0016) | 0 — ADR-0016 corrected to 0.41; one further contradiction found and fixed opportunistically (ADR-0015's C-dependency claim, see below) |
| ADRs naming a crate that does not exist | 1 (ADR-0011) | 0 |
| `cargo test --workspace` | 123 / 12 | 123 / 12 — unchanged, as expected for a documentation-only change |
| Lines of `.rs` changed | 0 | 0 — **confirmed via `git diff --stat HEAD -- '*.rs'`, empty** |

---

## 8. Per-item acceptance

Fill in the Status column as work lands. `Not needed` is a valid outcome — see §0.2.

| Item | Findings | Passes when | Status |
|---|---|---|---|
| W1 | DOC-03 | `utility/README.md` states the volume-sequence key layout and the unpadded-lexical-ordering caveat; `grep 'YYYY' utility/README.md` returns only the archive line; `documentation-inventory.md` is tracked. | **Done.** |
| W2 | DOC-02 | Supersession banner present and dates the E-11/E-12 additions; E-02…E-06 marked Resolved with pointers; every version and scorecard claim in the document matches the tree. | **Done.** |
| W3 | DOC-04 | Polling steps match `poll_once` step for step; no hour-anchoring reference survives; the missing-volume-folder stall is noted; the existing status callout is intact. | **Done.** |
| W4 | DOC-01 | Four documents exist; every command in `README.md` runs clean; status paragraph states plainly that `main.rs` is a stub; ADR index matches `ls docs/adr/`; all relative links resolve; D-b answered or a marked TODO recorded. | **Done.** D-a option 1, D-b GitHub private vulnerability reporting — both answered by the project owner before this session started (see §9). |
| W5 | DOC-05, DOC-07, DOC-11 | Four requirements carry `[OPEN — Q15/Q16]` markers and appear in §7's table; ADR-0016 states 0.41 with the advisory rationale; no ADR names `radar-decoder`; ADR-0010 lists three `crates/` members and the `utility/` split. | **Done.** Also fixed an unplanned finding in ADR-0015 — see §9. |
| W6 | DOC-06 | Structure tree matches `ls crates/` and `ls docs/`; stack table has an HTTP client row; status callout present and consistent with `README.md`. | **Done.** |
| W7 | DOC-08 | Grid geometry figures are measured from fixtures and recorded in §9; the standard-res case is either ICD-cited or explicitly marked unconfirmed; the texture-grid question is recorded in `open-questions.md` at a **verified-free** number. | **Done.** D-d option 1. Q17 confirmed free before use. |
| W8 | DOC-09 | Coverage table re-derived from disk; the `http-ingest` fuzz-corpus precedent is named; the unparsed `-S` metadata messages are recorded; no surviving claim that FR-ND-8 is satisfied. | **Done.** `crates/nexrad-decoder/TESTING.md` written; `data-flow.md` qualified with a pointer to it. |
| W9 | DOC-10 | Critical section is non-empty or explicitly "None outstanding"; every number from Q1 up appears as open or resolved; the preamble instructs future closures to move rather than delete. | **Done.** Q1–Q3 and Q10 recovered from `git log -p` and written up in a new "Resolved" section. |

---

## 9. Results

All nine work items landed in one session, 2026-07-30. §0's re-verification note held:
line numbers cited throughout this plan had drifted from `d46042c` slightly (the working
tree had moved since drafting), so every edit was made by matching content, not by
trusting a line number.

### 9.1 DECIDE items — answered by the project owner before implementation began

All four were put to the project owner as a single batch of questions at the start of
the session, and all four were answered with this plan's own recommendation:

- **D-a (W4):** Option 1 — keep `CLAUDE.md` ignored, write tracked substitutes
  (`README.md`, `docs/README.md`).
- **D-b (W4c):** GitHub private vulnerability reporting. `SECURITY.md` points to it; no
  email address is published.
- **D-c (W2):** Freeze `dependency-inventory.md` with a supersession banner, rather than
  re-auditing.
- **D-d (W7):** Option 1 — correct the factual claim in `rendering.md` only; record the
  texture-grid sizing decision as a new open question (Q17) rather than deciding it.

### 9.2 W7 — measured geometry (super-resolution, KDOX VCP 35)

Measured with a throwaway test against the five fixtures in
`crates/nexrad-decoder/tests/fixtures/`, using `ProductData::{gate_count, first_gate_m,
gate_width_m}` and `Radial::{azimuth_deg, azimuth_number}`. The test was deleted after
recording these numbers, per the plan's own instruction not to commit it.

- **Gate width:** 0.25 km, uniform across every moment and every tilt observed.
- **First gate:** 2.125 km, uniform across every moment and every tilt observed.
- **Azimuthal spacing:** 0.508° between consecutive azimuth numbers (`az_num` 1→2 in the
  `start_of_volume`/`intermediate` fixtures, 226.247°→226.755°) — consistent with the
  0.5° super-resolution spacing already recorded in `CLAUDE.md`.
- **Maximum range, by moment and tilt** (`first_gate_m + gate_count × gate_width_m`):

  | Tilt (elevation, fixture) | Reflectivity | Velocity / spectrum width | Dual-pol (ZDR/PHI/RHO/CFP) |
  |---|---|---|---|
  | 1 (~0.39°, `start_of_volume`/`intermediate`) | 460.125 km (1832 gates) | absent (surveillance-only cut) | 300.125 km (1192 gates) |
  | 2 (~0.26°, `start_of_elevation`) | 300.125 km (1192 gates) | 300.125 km (1192 gates) | absent (Doppler-only cut) |
  | 16 (~6.37°, `end_of_volume`) | 174.125 km (688 gates) | 174.125 km (688 gates) | 174.125 km (688 gates) |

  This is VCP 35's split-cut structure: the lowest elevation is split into a long-range
  surveillance pass (reflectivity + dual-pol, no Doppler) and a shorter-range Doppler
  pass (reflectivity + velocity + spectrum width, no dual-pol); higher tilts carry all
  seven moments at a shorter common range. The document's original "230 km" figure
  matches none of these.

Standard-resolution geometry remains unmeasured — all five fixtures are super-resolution
KDOX VCP 35, so there is nothing to measure it from in this repository. Recorded as
Q17's open half rather than asserted.

### 9.3 Unplanned finding: ADR-0015 misdescribed the `bzip2` backend

Found during W5's ADR sweep, not in `documentation-inventory.md`'s original eleven.
ADR-0015's Decision and Consequences asserted `bzip2` is "a safe Rust wrapper around
`bzip2-sys`, which vendors the reference C implementation (`libbzip2`)," and counted
that C dependency as an accepted tradeoff. `Cargo.lock` shows `bzip2 0.6.1` depending on
`libbz2-rs-sys 0.2.5` — a pure-Rust rewrite, not `bzip2-sys` — and
`docs/dependency-inventory.md` §3 already independently recorded this correctly ("bzip2
0.6.1 | pure Rust | ... | → libbz2-rs-sys 0.2.5"). The `bzip2` crate's default backend
changed to the pure-Rust implementation at some point before ADR-0015 was written, and
the ADR's Context/Decision were never re-verified against the resolved dependency graph.

Net effect is strictly better than what was decided: BZ2 decompression of
attacker-influenced chunk bytes has **no C dependency at all**, and `ring` (via
`rustls`) is the only non-Rust code anywhere in the production graph. Fixed in place
with a dated erratum, following the ADR-0014 pattern (P-b) — the original Decision and
Consequences text is preserved in the erratum rather than deleted. `SECURITY.md` (W4c),
written after this fix, states the corrected fact directly and needed no further change.

### 9.4 Not committed

No commits were created. This plan's own convention (P-a: one commit per work item) was
written assuming the implementing session would commit as it went; the agent executing
this plan operates under a standing instruction to commit only when the user explicitly
asks, and the user's request was to implement the plan, not to commit it. All nine work
items are complete and verified against the working tree (§7's gates were run against
`git diff`/`git status` rather than `git diff <baseline>..HEAD`, with the same result).
`docs/documentation-inventory.md` (P-f) and `docs/plans/documentation-remediation.md`
remain untracked, same as at baseline. Whoever commits this work should decide whether to
follow P-a's one-commit-per-work-item convention or land it as one commit — the diff is
small enough (thirteen modified files, six new) that either is reasonable.

### 9.5 §7 metrics table

Completed inline in §7, above — all metrics moved from baseline to final value.
`cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo deny check`, and `cargo audit` were all run and are clean, matching baseline
exactly (123 passed / 0 failed / 12 ignored), confirming this session made no
code-visible change.
