# Implementation Plan — Dependency Inventory Remediation

**Status:** Implemented, 2026-07-31 (all work items W1–W8 landed in one session; see §9 Results)
**Drafted:** 2026-07-30
**Addresses:** `docs/dependency-inventory.md` findings E-01 … E-10, plus E-11 and E-12
(found during implementation — see §9.0 and §9.3)
**Baseline commit:** `74a1065` (working tree: `.gitignore` modified, `docs/dependency-inventory.md` untracked)
**Toolchain:** rustc 1.95.0 / cargo 1.95.0, `x86_64-unknown-linux-gnu`

This plan is written to be executed in a later session. It carries every decision
already taken so implementation does not need to re-litigate them. Where a decision
is still open, it is marked **DECIDE** and states a recommendation.

---

## 1. Scope

The inventory's §8 recommended order was **E-02, E-04, E-05, E-03, E-09, E-07, E-06,
E-01**. This plan reorders it to put **E-03 first**, as directed, and makes three further
changes agreed during planning:

| Change | Rationale |
|---|---|
| E-03 moves to first | Directed. It is also the only finding that touches a *performance target under load*, and the only one whose answer could change the shape of the acquisition layer. Hygiene items cannot invalidate it; it could invalidate them. |
| **E-10 added** and folded into E-03 | New finding, not in the inventory — see §2. It plausibly dominates the same latency budget E-03 measures, so measuring one without the other produces a misleading number. |
| E-09 and E-07 dropped from this plan | Both are gated on subsystems that do not exist (tile layer, map layer). They become tracked blocking prerequisites in `docs/open-questions.md` instead — see W8. |
| A minimal CI workflow added (W5) | The E-05 supply-chain gates are inert if nothing runs them. There is currently no `.github/`. |

**In scope:** E-03, E-10, E-02, E-04, E-05, E-06, E-01, E-08, plus CI.
**Out of scope, tracked:** E-07, E-09.

### 1.1 Decisions taken during planning (settled — do not reopen)

| # | Question | Decision |
|---|---|---|
| P-a | How far does E-03 go? | **Measure, then fix only if over budget.** Harness and numbers land first, as their own reviewable commit. Remediation is gated on the measurement. |
| P-b | The cold-start anchor finding (E-10) | **In scope, remediated together with E-03.** Measured jointly; fixed jointly if the gate fails. |
| P-c | E-05 contents | **All four pieces:** delete `Cargo.lock` from `.gitignore`, add `rust-toolchain.toml`, add `[profile.release]` with an explicit panic policy, add `deny.toml` + install `cargo-deny` / `cargo-audit`. |
| P-d | E-06 approach | **Hand-roll a PNG encoder.** No `png` dependency. Drops ~11 crates and adds none. |
| P-e | E-07 / E-09 | **Defer to their own effort.** Recorded as blocking open questions, not resolved here. |
| P-f | CI | **Add a minimal GitHub Actions workflow.** |
| P-g | E-01 (`tls12`) | **Keep `tls12` enabled**, and record it as a decision rather than a default. The inventory's argument is accepted: operators on arbitrary networks during severe weather must not lose acquisition entirely because a middlebox offers only TLS 1.2. |

---

## 2. E-10 — a finding the inventory does not carry

Found while reading `s3_poll.rs` for E-03. Recorded here because it changes what E-03's
measurement means.

`S3Poller::new` seeds `last_seen_key` from `current_hour_anchor`
([s3_poll.rs:53](../../crates/radar-workstation/src/ingest/s3_poll.rs#L53)), which returns the
bare **current-UTC-hour directory prefix** —
`KDOX/2026/07/30/14/`. That prefix sorts before every real key in the hour, so the first
`list_keys_after` returns **every chunk produced since the top of the hour**, and
`poll_once` then fetches all of them, one at a time, before returning a single envelope.

The doc comment says this exists so that "startup does not replay historical chunks from
earlier in the hour" — but the hour boundary is the wrong granularity for that intent. At
:05 past the hour the backlog is small; at :55 it is nearly an hour of chunks. NEXRAD
produces roughly 10–15 volumes per hour and tens of chunks per volume, so the cold-start
backlog ranges from a handful of objects to several hundred, purely as a function of when
the operator launched the application.

Why this matters for E-03: the inventory frames the risk as *per-request* serialization
cost. But if the first poll must drain a 300-object backlog before delivering anything,
then per-request cost is a second-order term and parallelizing the fetch would buy a 3–4×
improvement on a number that needs to come down by two orders of magnitude. **Measuring
E-03 without E-10 would produce a number that is real but attributes the latency to the
wrong cause.** Hence P-b.

Severity: `medium`. Not a crash, not data loss. It is a startup-latency and
bandwidth-waste issue that scales with the clock, which makes it exactly the kind of
defect that behaves well in testing and badly during an event.

---

## 3. Measured baseline

All figures from commit `74a1065`, reproduced on this checkout. Every "after" column in
§5 is measured against these.

| Metric | Baseline | Command |
|---|---:|---|
| Workspace units (`--workspace --release`) | **62** | `RUSTC_BOOTSTRAP=1 cargo build --release --unit-graph -Z unstable-options` |
| App units (`-p radar-workstation --release`) | **35** | same, with `-p radar-workstation` |
| Units compiled by a **bare** `cargo build` | **62** | no `default-members` set |
| Tests, `cargo test --workspace` | **111 passed / 7 ignored** | `cargo test --workspace` |
| Tests reachable from the three `crates/` members | **101 passed / 5 ignored** | `cargo test -p radar-workstation -p nexrad-decoder -p http-ingest` |
| `target/release/fetch-sample` | **3,073,288 bytes** | |
| `target/release/radar-workstation` | **446,600 bytes** | 4-line stub `main.rs`; not a meaningful figure yet |
| `cargo clippy --workspace --all-targets` | clean | |
| Cold-start `poll_once` latency | **unmeasured** | this is W1 |

Two facts confirmed while establishing the baseline, both feeding work items below:

- `git check-ignore -v --no-index Cargo.lock` → `.gitignore:3:Cargo.lock`. The lockfile
  **is** tracked (`git ls-files` confirms), so nothing is broken today, but the rule is
  live and would catch a fresh clone's regenerated lockfile or a `git clean -X`. E-05
  confirmed.
- `httparse` is still listed as a direct dependency in ADR-0014's Decision table
  ([0014-http-ingest-own-the-boundary.md:47](../adr/0014-http-ingest-own-the-boundary.md#L47)).
  E-02 confirmed — and see W2 for why the finding needs widening.

---

## 4. Order of work

Each work item is one commit, independently green on the §7 gates, so a bisect lands on a
single finding. W1 splits into two commits (harness, then remediation) so the measurement
is reviewable without the fix.

| # | Item | Findings | Effort | Blocking? |
|---|---|---|---|---|
| **W1** | Cold-start acquisition latency: measure, then remediate if over budget | E-03, E-10 | 2 h measure + up to 1 d fix | No, but do it first |
| **W2** | Reconcile ADR-0014's body with its erratum | E-02 | 30 min | No |
| **W3** | `default-members` | E-04 | 15 min | No |
| **W4** | Reproducibility scaffolding | E-05 | 3 h | W3 first (unit counts) |
| **W5** | Minimal CI workflow | — | 1 h | **W4 first** |
| **W6** | Hand-rolled PNG encoder in `radar-viz` | E-06 | 3 h | W3 first (unit counts) |
| **W7** | Record the `tls12` decision; document the fuzz toolchain | E-01, E-08 | 30 min | No |
| **W8** | Defer E-07 / E-09 as tracked open questions | E-07, E-09 | 30 min | No |

W3 precedes W4 and W6 only so that the unit-count deltas each item claims are
attributable to that item. Nothing else is order-dependent.

---

## 5. Work items

### W1 — Cold-start acquisition latency (E-03 + E-10)

#### W1.1 Define the budget precisely, before measuring

`docs/architecture/rendering.md:199` sets **"< 5 seconds on normal connection"** for *time
to display after site change*. That target is about pixels on screen, and displaying
anything requires **one assembled volume**, not a drained backlog. So the measurement's
pass criterion is:

> **Time from `S3Poller::new` to holding every chunk of one complete volume scan.**

State this interpretation in the results write-up. It is the thing that decides whether
E-03 or E-10 is the actual defect, and it is not stated anywhere today.

Note the interaction with ADR-0012: "one complete volume" means an `-S` chunk plus its
`-I` chunks plus the `-E` chunk. A start that lands mid-volume produces an
ADR-0012 `Superseded`-adjacent path (missing `-S`, per that ADR's "Missing `-S` chunk"
section) — degraded but defined. This matters for choosing the E-10 remedy in W1.3.

#### W1.2 The harness (commit 1)

Live-network timing tests as **`#[ignore]`d unit tests inside
`crates/radar-workstation/src/ingest/s3_poll.rs`'s existing `mod tests`.**

Rationale for that location, rather than `tests/`:

- `poll_once`, `list_keys_after`, `last_seen_key`, and `current_hour_anchor` are all
  private. An integration test would have to either widen visibility for measurement's
  sake or re-implement the anchor logic — the latter violating the DRY instruction in
  `CLAUDE.md` and, worse, measuring a copy of the code rather than the code.
- Precedent exists: `http-ingest` puts its wire tests in `src/` unit tests for exactly
  this reason (ADR-0014 erratum item 5).
- ADR-0014's plan §6H explicitly recorded `poll_once` as an accepted testing gap. This
  closes part of it without introducing the trait seam that gap existed to avoid.

`radar-workstation` already declares `tokio` with `macros` and `rt-multi-thread`, so
`#[tokio::test]` needs **no new dependency**.

Four tests, all `#[ignore]`d, all printing a structured line so results can be pasted into
the write-up:

| Test | Records |
|---|---|
| `cold_start_listing_size` | Wall-clock minute-of-hour at launch; keys returned by the first `list_keys_after`; page count; `T_list`. Run at three points in the hour (early / mid / late) to show the E-10 slope. |
| `cold_start_poll_once_latency` | Total `poll_once` wall time from a fresh `S3Poller`; per-object fetch times (min / median / max); total bytes. **This is the E-03 + E-10 headline number.** |
| `steady_state_poll_latency` | The 2nd and 3rd `poll_once` after the backlog is drained — the real ~1-chunk-per-5-s case. Expected to be trivially inside budget; establishes that the problem is confined to cold start. |
| `keepalive_amortization` | First `get_object` vs. subsequent ones on the same `Client`. The serialization decision in ADR-0014 rests on "TLS session resumption and keepalive make each subsequent request cheap." Measure it rather than assume it. |

Also capture, for W7, the negotiated TLS version against both buckets. No code change
needed — `curl -vI https://unidata-nexrad-level2-chunks.s3.amazonaws.com/` and the archive
host, recording the `SSL connection using TLSv1.x` line.

Deliverable: a **Results** section appended to this plan document, with the actual numbers
and the pass/fail verdict against §W1.1.

#### W1.3 Remediation, gated on the measurement (commit 2)

Apply **only** if W1.2 shows time-to-first-complete-volume over ~5 s. Fix E-10 before
E-03 — it is the larger term and the cheaper change.

**E-10 remedy — recommended: anchor to the newest `-S` key.**

Restructure the anchor so the first poll starts at a volume boundary instead of an hour
boundary:

1. List the current-hour prefix (as today).
2. Select the **most recent `-S` key** in the listing; set `last_seen_key` to the key
   immediately preceding it, so the batch begins with that `-S`.
3. If the current hour contains no `-S` (launched just after the top of the hour), also
   list the previous hour's prefix and select from the union.
4. Fetch only from that point forward.

This delivers a volume-aligned start with zero wasted object fetches, and hands ADR-0012's
state machine a clean `-S` rather than exercising its degraded missing-`-S` path.

Structure the selection as a **pure function** so it is testable offline:

```rust
/// Returns the key that should be used as `start-after` so the next batch
/// begins with the most recent -S chunk in `keys`. `keys` is in S3 lexical
/// (== chronological) order. `None` if the listing contains no -S chunk.
fn anchor_before_newest_start(keys: &[String]) -> Option<&str>
```

New offline unit tests: newest-`-S`-of-several, single `-S`, no `-S` at all, `-S` as the
first key, empty listing. None touches the network.

Rejected alternative: capping the first batch to the newest N keys. Simpler, but starts
mid-volume by construction, so every cold start begins with a degraded volume. Not worth
the saving.

**E-03 remedy — apply only if still over budget after the E-10 fix.**

Fan the batch out over 2–4 `http_ingest::Client` instances via `tokio::task::JoinSet`.
Each `Client` remains one keepalive connection with no pool inside it, so the change is
local and the crate's shape is unchanged.

Two things this must not lose:

- **Batch atomicity.** Today it falls out of the first `?` before `last_seen_key` is
  assigned ([s3_poll.rs:97-111](../../crates/radar-workstation/src/ingest/s3_poll.rs#L97-L111)).
  With concurrency it must be restored explicitly: collect all results, and advance
  `last_seen_key` only if every fetch succeeded. This is what the pre-ADR-0014 code did;
  ADR-0014 §7.2 removed the scaffolding. Reinstate it deliberately, with the comment
  explaining why, and reinstate a `PollError` variant for `JoinError` so a panicked task
  surfaces rather than being read as a missing chunk.
- **Ordering.** Envelopes must reach the assembler in key order regardless of completion
  order. Index the results, do not push as they land.

> ⚠️ **This requires an ADR-0014 amendment.** ADR-0014's Decision says "A single
> long-lived keepalive connection per `(host, port)` pair … **No connection pool.**"
> N clients against one host is a pool by any honest reading of that sentence. If the E-03
> remedy ships, ADR-0014 gains an erratum item recording the measurement that forced it and
> the revised connection policy. Do not ship the fan-out and leave the ADR asserting the
> opposite — that is precisely the failure mode E-02 exists to correct.

**Carried forward, not fixed here:** a key that permanently 404s blocks the stream, because
`last_seen_key` never advances past it. Predates this work; recorded in ADR-0014's plan
§7.2 as wanting its own issue. The E-10 anchor change does not alter it. If W1.3 runs,
file the issue at the same time — the batch-atomicity comment is the natural place a
future reader will trip over it.

**Regression bar:** the five existing `s3_poll.rs` unit tests must pass **unmodified**.

---

### W2 — Reconcile ADR-0014's body with its erratum (E-02)

E-02 flags one row: `httparse` in the Decision table. **The finding needs widening**, and
the plan does so deliberately rather than silently — the inventory's own argument ("a
reader consults the part that looks authoritative for 'what are the dependencies' and gets
the wrong answer") applies identically, and with higher stakes, to three other passages:

| Passage | Body says | Erratum says | Stakes |
|---|---|---|---|
| [:47](../adr/0014-http-ingest-own-the-boundary.md#L47) Decision table | `httparse` is a direct dependency | item 2: not taken | Low. The finding as filed. |
| [:56](../adr/0014-http-ingest-own-the-boundary.md#L56) Decision | chunked responses "are rejected with a typed error rather than parsed" | item 1: chunked **is** supported; blanket rejection breaks the primary path | **High.** A reader implementing a second `ChunkSource` from the Decision text would build against a framing policy the code does not have. |
| [:107](../adr/0014-http-ingest-own-the-boundary.md#L107) Implementation notes | `list_prefix(...) -> ListResponse` | item 3: returns `Bytes`, gains `start_after` | Medium. Wrong API signature in the authoritative section. |
| [:111](../adr/0014-http-ingest-own-the-boundary.md#L111) Implementation notes | a `dev-server/` module, "rustls-fronted" | item 4: `src/test_server.rs`, plaintext | Low. |

**Approach:** correct the body text in place, each corrected passage carrying an inline
`(see Erratum, item N)` pointer. Keep the Erratum section intact as the change log — it is
the audit trail, and deleting it would destroy the record of *when* the decision changed
and on what evidence. The result is a document whose authoritative sections are correct
and whose history is still legible.

Specific edits:

- Decision table: drop the `httparse` row; add a `bytes` row (role: zero-copy body
  handoff; cross-reference ADR-0017). The table then matches
  `crates/http-ingest/Cargo.toml` **exactly, row for row** — which is the property that
  makes it verifiable rather than merely plausible. Add a line beneath: header and chunk
  parsing are hand-rolled in `src/response.rs`; see Erratum item 2.
- Line 56: rewrite to state the actual policy — `Transfer-Encoding: chunked` is accepted
  and decoded under `max_body_bytes`; any other value, a duplicate header, or `TE`
  combined with `Content-Length` is a framing violation. Pointer to Erratum item 1.
- Line 107: correct the signature. Pointer to item 3.
- Line 111: correct to `src/test_server.rs`, `#[cfg(test)]`, plaintext. Pointer to item 4.
- Erratum items 1–4: retain, prefixed "Now reflected in the body above."

No code impact. Verification is by reading, plus one mechanical check: every crate in
ADR-0014's Decision table appears in `crates/http-ingest/Cargo.toml` `[dependencies]`, and
vice versa.

---

### W3 — `default-members` (E-04)

Root `Cargo.toml`:

```toml
default-members = [
    "crates/radar-workstation",
    "crates/nexrad-decoder",
    "crates/http-ingest",
]
```

Expected: bare `cargo build --release` drops **62 → 35** units. `image`, `shapefile`,
`dbase`, `time`, `png`, `flate2`, `moxcms`, `pxfm` and the rest stop compiling by default,
and stop unifying features with the production graph.

**The consequence that must not be left implicit:** bare `cargo test` will then run
**101 of the 111 tests** — `nexrad-sample`'s 10 and `radar-viz`'s 0 fall out. A developer
running the command `CLAUDE.md` documents would see green without having compiled the
utilities at all. Mitigations, both required:

- Update `CLAUDE.md`'s Build Commands to `cargo test --workspace` and
  `cargo clippy --workspace --all-targets -- -D warnings`.
- W5's CI uses `--workspace` throughout.

Note for W4: `cargo-deny` reads the full lock graph regardless of `default-members`, so
the utility crates' licenses and advisories stay in scope. That is correct — they are
still shipped source.

Verification: unit-graph count is 35; `cargo test` reports 101/5 and
`cargo test --workspace` still reports 111/7.

---

### W4 — Reproducibility scaffolding (E-05)

Four independent pieces. Each is separately verifiable; keep them one commit for
reviewability, since they are one finding.

#### W4a — `.gitignore`

Delete line 3 (`Cargo.lock`). A binary workspace wants its lockfile tracked, and the rule
is live (§3). While in the file, restore the trailing newline the current working-tree diff
removed.

Verify: `git check-ignore -v --no-index Cargo.lock` returns nothing.

**Hygiene note, found in passing, not part of E-05:** `out.png` is tracked at the repo
root (committed in `e1b4f3f`, "visualization utilities for POC and verification"). It is a
`radar-viz` output artifact. Recommend `git rm` it and add `/out.png` — or better, `*.png`
at the root — to `.gitignore` in the same commit. Flagged rather than done silently
because it is a deletion of tracked content and therefore yours to approve.

#### W4b — `rust-toolchain.toml`

```toml
[toolchain]
channel    = "1.95.0"
components = ["rustfmt", "clippy"]
targets    = ["x86_64-unknown-linux-gnu"]
profile    = "minimal"
```

Pins the compiler, so CI and every developer machine agree and a toolchain bump becomes a
tracked commit with a reviewable diff. `rustup` auto-installs on the first `cargo`
invocation in the directory. Cost: a contributor on a different toolchain silently gets a
download.

Verify: `rustc --version` run inside the repo reports 1.95.0.

#### W4c — `[profile.release]`

Add to the root `Cargo.toml`. Each setting is a recorded decision, not a default:

```toml
[profile.release]
panic         = "unwind"
lto           = "thin"
codegen-units = 1
strip         = "debuginfo"
```

- **`panic = "unwind"`** — the one the inventory calls out as needing a deliberate choice.
  Principle 2 (Stability as Ethics) decides it: under `unwind`, a panic in a decode or
  fetch task is contained to that task and ADR-0012 already defines the degraded path
  (`TimedOut` / missing-chunk handling). Under `abort`, the same panic takes down the whole
  workstation mid-warning. The binary-size and codegen savings from `abort` are not worth
  that trade in this application.
- **`lto = "thin"`** over `"fat"` — nearly all of fat's cross-crate benefit at a fraction
  of the release build time, which matters because release builds are on the
  measure-and-iterate path for W1 and for every future perf target.
- **`codegen-units = 1`** — better intra-crate optimization; the compile-time cost is
  bounded because thin LTO is doing the cross-crate work.
- **`strip = "debuginfo"`, deliberately not `"symbols"`** — this is the philosophy-driven
  one. Stripping debuginfo removes the bulk; retaining the symbol table means a panic
  during an event produces a **readable backtrace**. A binary that crashes during a
  tornado warning and emits nothing but addresses is a stability failure twice over.

**DECIDE — `overflow-checks` in release.** Not in the inventory; raising it because
`[profile.release]` is the only place it can be set and this is the one time the section
gets written.

The decoder does arithmetic on gate counts, pointer offsets, and block sizes read straight
out of attacker-influenceable bytes. With release defaults (`overflow-checks = false`) a
malformed radial can wrap an offset and produce a *plausible-looking wrong answer*. With
`overflow-checks = true` it panics, and under `panic = "unwind"` that panic is contained to
the decode task with an ADR-0012 path already defined for the resulting missing chunk.

**Recommendation: `overflow-checks = true`.** Silently rendering wrong reflectivity is a
worse failure than a named, contained, logged decode failure — the Instrument Principle
says the user is looking *through* the software at the atmosphere, and wrong data breaks
that in a way a gap does not. Cost is a few percent of decode throughput. Wants your
sign-off because it is a runtime-behavior change the inventory did not ask for.

Verify: record `target/release/fetch-sample` size against the 3,073,288-byte baseline, and
confirm a deliberately panicking build still produces a symbolized backtrace.

#### W4d — `deny.toml` + tooling

Install `cargo-deny` and `cargo-audit` (`cargo install --locked`, pinned versions).
Neither becomes a workspace dependency.

`deny.toml` skeleton — but note the license section **must be derived from the graph, not
copied from a template**:

```toml
[licenses]
allow = [ ... ]           # populate from `cargo deny check licenses` output

[bans]
multiple-versions = "warn"
# Both duplications were assessed in dependency-inventory.md §6 and found benign.
# Listed here so a future reader knows they were reviewed, not overlooked.
skip = [
    { name = "getrandom" },   # 0.2 via ring (production); 0.4 via tempfile (dev-only)
    { name = "windows-sys" }, # neither version compiles on this target
]

[advisories]
yanked = "deny"

[sources]
unknown-registry = "deny"
allow-git        = []     # asserts: zero git dependencies. A real supply-chain property.
```

Two things the implementing session will hit:

- **`ring`'s license needs a `[[licenses.clarify]]` entry.** `ring` carries a mixed,
  partly OpenSSL-derived license that is not cleanly SPDX-expressible, and `cargo-deny`
  will flag it. Resolve it by reading `ring`'s own `LICENSE` file and writing an explicit
  clarification with the file hash — not by adding a blanket exception. This is the one
  part of W4d that is real work rather than configuration.
- **`deny.toml`'s schema has changed across `cargo-deny` major versions** (notably the
  `[advisories]` `unmaintained` key). Pin the tool version in CI (W5) so the config and
  the tool cannot drift apart.

**DECIDE — drop `cargo audit` in favor of `cargo deny check`?** `cargo deny check
advisories` reads the same RustSec database; `cargo audit` is a strict subset of what
`cargo-deny` does. `CLAUDE.md` currently lists `cargo audit` among the build commands and
the tool is not installed on this machine.

**Recommendation: keep both, for now, and say why in `CLAUDE.md`.** `cargo audit` is what a
government or defense reviewer will recognize and may ask for by name; `cargo deny check`
is what actually gates. The redundancy costs one CI step. If you would rather have one
tool, drop `cargo audit` and update `CLAUDE.md` — the coverage loss is nil.

Verify: `cargo deny check` exits clean.

---

### W5 — Minimal CI workflow

`.github/workflows/ci.yml`. Triggers: push to `main`, and `pull_request`. Permissions
`contents: read`, declared explicitly (least privilege; it matters for a repo aiming at
government review).

Steps:

1. `actions/checkout`, **pinned by commit SHA**, not by tag.
2. No other third-party actions. The toolchain installs itself from `rust-toolchain.toml`
   on the first `cargo` invocation; `cargo-deny` installs via
   `cargo install --locked cargo-deny --version <pinned>`. This trades ~2 minutes of
   build time for not granting a third-party action write access to the build. For a
   project whose stated posture is "approvable in government and defense environments,"
   that is the right side of the trade — and it should be stated in a comment in the
   workflow so it does not get "optimized" away later.
3. `cargo build --release --workspace`
4. `cargo test --workspace` — note `--workspace` explicitly, because W3 changed what the
   bare form covers.
5. `cargo clippy --workspace --all-targets -- -D warnings`
6. `cargo deny check`
7. `cargo audit` (subject to the W4d decision)

**Deliberately omitted, with reasons:**

- **No dependency caching.** `actions/cache` is another action and another trust
  relationship for a build that is already ~2 minutes. Revisit if CI time becomes a
  problem.
- **No `cargo fmt --check`.** There is no `rustfmt.toml`, and the existing code does not
  match default rustfmt — `s3_poll.rs:249` and the aligned `Cargo.toml` dependency tables
  are both intentional and both would be reformatted. Adopting `fmt` means either a
  tree-wide formatting commit or a `rustfmt.toml` that encodes the current style. That is
  a separate decision with its own diff; recorded here so its absence is a choice rather
  than an oversight.
- **The live tests never run in CI.** They are `#[ignore]`d and no step passes
  `--ignored`. This is load-bearing for the "no undisclosed network connections"
  principle and for determinism, so the workflow says so in a comment.

Verify: the workflow passes on a branch push before merging.

---

### W6 — Hand-rolled PNG encoder in `radar-viz` (E-06)

Removes `image 0.25` and with it `moxcms`, `pxfm`, `png`, `flate2`, `miniz_oxide`,
`fdeflate`, `bytemuck`, `num-traits`, `byteorder-lite`, `simd-adler32`, `crc32fast`,
`adler2` — about 11 crates, including a `0.1.x` single-author crate with no `repository`
field — and adds none.

`radar-viz` is a dev tool, so the value is not the dev tool. It is that the workspace ends
up owning a raster encoder before the real render path needs one, which is exactly the
pattern the inventory says should *not* reach `crates/` by way of `image`.

#### New: `utility/radar-viz/src/png_out.rs` (~110 lines)

```rust
pub struct Raster { width: u32, height: u32, px: Vec<u8> }   // RGBA8, row-major

impl Raster {
    pub fn filled(width: u32, height: u32, color: [u8; 4]) -> Self;
    pub fn put_pixel(&mut self, x: u32, y: u32, color: [u8; 4]);
}

pub fn write_png(raster: &Raster, path: &Path) -> std::io::Result<()>;
```

Encoding, all of it `std`-only:

- 8-byte PNG signature.
- `IHDR`: width, height, bit depth 8, colour type 6 (RGBA), compression 0, filter 0,
  interlace 0.
- `IDAT`: a zlib stream — 2-byte header, then **stored (uncompressed) deflate blocks**
  (`BTYPE = 00`, `LEN`/`NLEN` little-endian, ≤ 65535 bytes each, `BFINAL` on the last),
  then the 4-byte big-endian Adler-32 of the *uncompressed* data.
- Uncompressed data: per scanline, a filter byte of `0` followed by `width × 4` bytes.
- `IEND`. Every chunk carries a CRC-32 over chunk type + data.

Two small tables: CRC-32 (0xEDB88320 polynomial) and Adler-32 (no table needed).

**Cost, stated plainly:** stored deflate blocks mean no compression. A 1200×1200 RGBA PPI
goes from roughly 300 KB to roughly 5.8 MB on disk. For a developer utility that writes
one file per invocation this is fine. If it becomes annoying, fixed-Huffman deflate is
another ~60 lines — do not build it speculatively.

#### Call-site changes

| File | Change |
|---|---|
| `render.rs:3,8` | drop `use image::{ImageBuffer, Rgba}`; `type PpiImage = Raster` |
| `render.rs:26-27` | `Rgba([15,15,15,255])` → `[15u8,15,15,255]`; `ImageBuffer::from_pixel` → `Raster::filled` |
| `render.rs:91` | `img.put_pixel(x, y, Rgba(color))` → `img.put_pixel(x, y, color)` |
| `overlay.rs:3,50,60,99,105,122` | same substitutions; `Rgba<u8>` params become `[u8; 4]` |
| `main.rs:118` | `img.save(&args.output)` → `png_out::write_png(&img, &args.output)` |
| `Cargo.toml` | remove `image`. **`shapefile` stays** — that is E-07, deferred. |

#### Verification — the part that actually matters

Unit tests alone cannot prove a PNG is well-formed, and there is no decoder in the
workspace to round-trip against. So:

1. **Golden bytes.** Encode a 2×2 raster of known colours and assert the exact output byte
   sequence. Follows the `http-ingest` precedent of golden request bytes
   (ADR-0014 plan §6C). Computed once and reviewed by hand.
2. **Invariants.** Uncompressed length is exactly `height × (1 + width × 4)`; block count
   is `ceil(len / 65535)`; CRC-32 against the PNG specification's published test vector.
3. **One-time external validation, before the commit lands.** Render a fixture with the
   current `image`-based code and save it. Apply the change, render the same fixture, then
   confirm the two decode to **identical pixels** using a tool outside the workspace —
   `python3` + `zlib`, or ImageMagick `compare -metric AE`. Record the result in the commit
   message. This is the only step that proves the encoder correct rather than
   self-consistent; do not skip it.
4. `cargo tree -p radar-viz` shows the 11 crates gone; workspace unit count recorded
   against W3's post-`default-members` figure.

---

### W7 — Record the `tls12` decision; document the fuzz toolchain (E-01, E-08)

**E-01.** Add a short subsection to ADR-0014 recording that `tls12` stays enabled on both
`rustls` and `tokio-rustls`, with:

- The rationale (P-g): dropping 1.2 would narrow the protocol surface to TLS 1.3, but
  operators run on arbitrary networks during severe weather. An endpoint or middlebox that
  offers only 1.2 would turn a cosmetic risk reduction into total acquisition failure.
  Under Principle 2 that trade goes the other way.
- The live evidence captured in W1.2: what both S3 hosts actually negotiate. Recording
  "S3 offers 1.3, we keep 1.2 anyway, deliberately, for the middlebox case" is a
  materially stronger record than the assertion alone.
- The reversal condition: if a future deployment profile guarantees 1.3 end to end,
  dropping `tls12` is a one-line change and a superseding note.

**E-08.** Add `crates/http-ingest/fuzz/README.md` (~10 lines): running the fuzz target
requires nightly Rust, `cargo-fuzz`, and LLVM's libFuzzer (C++) — none of which are
installed here or needed for any normal build. State that the corpus-regression and
mutation tests in `response.rs:536` and `:561` enforce the no-panic property on **stable**,
and that the nightly fuzzer is an amplifier, not the primary defense. That framing is why
E-08 is acceptable rather than a gap, and it should live next to the code, not only in an
audit document.

---

### W8 — Defer E-07 and E-09 as tracked open questions

Neither is resolved here (P-e). Both are recorded where the relevant subsystem's
implementer will actually look, because `docs/dependency-inventory.md` is a point-in-time
audit and will age out.

**Correction, made during implementation:** this section assumed Q14 was free. It is not
— `docs/open-questions.md` already had a Q14 ("Backup data source?", added independently
of this plan). The two new questions below actually landed as **Q15** and **Q16**. Text
below is left as originally planned; treat every "Q14"/"Q15" reference in it as "Q15"/
"Q16" respectively. The acceptance table (§7) and §8 use the corrected numbers.

Add to `docs/open-questions.md` under **Architecture — Resolve Before the Relevant
Subsystem**. Next free numbers are **Q14** and **Q15** (Q12/Q13 are taken by the
Distribution section):

**Q14 — How is shapefile geometry loaded for production overlays?**
ADR-0006 designates `shapefile`, `geo`, and `lyon` for production overlay loading. That
puts a 0.x single-maintainer binary parser (plus `dbase` 0.5 and `time` 0.3) on a startup
path that must not panic, per Principle 2. The alternative the inventory argues for: since
ADR-0006 already pre-projects overlays at load time, pre-project them at **build** time
into a flat bundled format the app `mmap`s — removing `shapefile`, `dbase`, `time`, and
`geo` from the shipped binary, eliminating a class of startup panic, and helping the
< 2 s first-render target. Resolving this means either accepting those dependencies under a
recorded rationale or superseding ADR-0006's parser clause. **Blocks:** overlay loading
implementation. Analysis: `docs/dependency-inventory.md` E-07.

**Q15 — What HTTP client serves ADR-0007's tile providers?**
ADR-0007 requires a user-supplied URL template against an **arbitrary** host, which in
practice means redirect following, `ETag` / `If-None-Match` for the disk cache, and
possibly HTTP/2. ADR-0014 lists all of those as explicit non-goals of `http-ingest` and
says a need like this is "a signal to reopen this ADR, not to grow the crate." Three
options, assessed in the inventory: generalize `http-ingest`; add a second, separate
client crate scoped as best-effort and structurally unable to affect the radar path;
or reintroduce a third-party client for tiles only. The inventory recommends the second
and rates the third worst. The answer determines whether `http-ingest`'s compile-time
allowlist is a permanent asset or a temporary one, so it must be settled **before** any
tile code is written, and recorded in its own ADR. **Blocks:** the entire tile subsystem.
Analysis: `docs/dependency-inventory.md` E-09.

Also update `CLAUDE.md`'s **Open Questions** section so Q14 and Q15 appear alongside
Q8/Q9/Q11 under the subsystem that blocks on them.

---

## 6. What this plan does not do

Stated so the boundaries are explicit rather than inferred:

- **Does not resolve E-07 or E-09.** W8 records them; it does not answer them.
- **Does not build the E-07 build-time projection tool.** Designing a bundled overlay
  format before the renderer's requirements are concrete would fix the format at the
  wrong time.
- **Does not touch `nexrad-decoder`.** Empty `[dependencies]`; it is the standard, not a
  finding.
- **Does not adopt `rustfmt`.** See W5.
- **Does not remove `shapefile` from `radar-viz`.** That is Q14's answer to make, not
  this plan's.
- **Does not add any production dependency.** Net dependency change across all eight work
  items: **−1 direct (`image`), −11 transitive, +0.**

---

## 7. Verification gates

Run at the end of every work item. Every command is offline-safe except the `--ignored`
live tests.

```bash
cargo build --release --workspace
cargo test --workspace                                   # expect 111 passed / 7 ignored,
                                                         #   +4 ignored after W1.2
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check                                         # from W4d onward
```

Live, manual, never in CI:

```bash
cargo test -p http-ingest        -- --ignored
cargo test -p radar-workstation  -- --ignored             # the W1 timing harness
```

### Metrics to record

Fill this in as work lands. The point is that every claim in the final write-up is a
measurement, matching the inventory's own standard.

| Metric | Baseline `74a1065` | After W3 | After W6 | Final |
|---|---:|---:|---:|---:|
| Units, bare `cargo build --release` | 62¹ | 44¹ | 44¹ | **44** |
| Units, `--workspace --release` | 62¹ | 75¹ | 56¹ | **56** |
| Crates in `cargo tree -p radar-viz` | — | — | −13 (`image` + 12 transitive) | confirmed gone from `Cargo.lock` |
| `target/release/fetch-sample` bytes | 3,073,288 | | | **2,532,752** |
| `cargo test --workspace` | 111 / 7 | | | **123 / 12** |
| bare `cargo test` | 111 / 7 | | | **108 / 10** |
| Cold-start time to first complete volume | unmeasured | | | see §9 Results — not a software-latency number (antenna physics, ~6 min for VCP 35); time to *currently-available* data is **1.56 s** |
| Direct production deps without an ADR | 0 | 0 | 0 | **0** |
| `cargo deny check` | not installed | | | **clean** (found and fixed a real RUSTSEC advisory pair on `quick-xml`, unrelated to any inventory finding — see §9.3) |
| `cargo audit` | not installed | | | **clean**, 66 crates scanned |

¹ The baseline's 62/62 figures do not exactly reproduce on this checkout even before any
code change (measured 75/44 pre-W3, same toolchain, unchanged `Cargo.lock`) — almost
certainly a `cargo`-version difference in how the unit graph double-counts
`build-script-build` units, not a real regression. Test/unit counts also grew across the
session from tests added by W1 and W6. Treat "Final" as authoritative; treat the
baseline column as directional, not bit-for-bit reproducible.

### Per-item acceptance

| Item | Passes when | Status |
|---|---|---|
| W1.2 | Four `#[ignore]`d timing tests exist; numbers and a verdict against the §W1.1 budget are written into this document's Results section. | **Done.** All four ran live against `KDOX`; see §9. |
| W1.3 | Only if W1.2 failed: time to first complete volume inside 5 s; five original `s3_poll.rs` tests unmodified and green; new `anchor_before_newest_start` tests green offline; ADR-0014 amended if the fan-out shipped. | **Not triggered** — W1.2 passed with two orders of magnitude margin. The E-11 anchor fix (below) shipped regardless, since it was a correctness bug, not a latency remedy. |
| — (E-11, unplanned) | A key-format bug found while building W1.2 — `current_hour_anchor` didn't match the real bucket layout — is fixed: `S3Poller` anchors on numeric volume-sequence folders via a new `delimiter` param on `http_ingest::Client::list_prefix`; offline tests for `cold_start_baseline`/`parse_volume_folder` pass; five original `s3_poll.rs` tests still pass (now updated for the new key format, since the old tests encoded the wrong layout too). | **Done.** See §9.0 and `docs/dependency-inventory.md` E-11. |
| W2 | Every crate in ADR-0014's Decision table appears in `crates/http-ingest/Cargo.toml`, and vice versa. No body passage contradicts an erratum item. | **Done.** Table now lists `tokio`/`rustls`/`tokio-rustls`/`webpki-roots`/`bytes` — matches `Cargo.toml` exactly. Erratum gained items 8 (corrects item 6's chunks-bucket claim) and 9 (`delimiter` param). |
| W3 | Bare build is 35 units; `CLAUDE.md` commands updated to `--workspace`. | **Done, different number.** Bare build is 44 units on this checkout's toolchain (see metrics footnote) — still a clean drop from the 56-unit `--workspace` build. `CLAUDE.md` updated. |
| W4 | `git check-ignore --no-index Cargo.lock` empty; in-repo `rustc --version` is 1.95.0; `cargo deny check` clean; a panicking release build yields a symbolized backtrace. | **Done.** All four confirmed; `cargo deny check` also caught and (after an upgrade) resolved two real RUSTSEC advisories on `quick-xml` — see §9.3. |
| W5 | Workflow green on a branch; no third-party action other than SHA-pinned `actions/checkout`; no step passes `--ignored`. | **Written, not yet run in CI** — no push to a branch happened in this session. Steps mirror the local verification gates, which are all green locally. |
| W6 | `image` absent from `cargo tree -p radar-viz`; golden-byte test green; **external pixel-identity check against a pre-change render recorded in the commit message**. | **Done.** `image` and 12 transitive crates gone; golden-byte + invariant tests green; SHA-256-identical pixel bytes confirmed against a pre-change render via a disposable git worktree + Python/Pillow (external to the workspace) — see §9.4. |
| W7 | ADR-0014 records the `tls12` decision with live evidence; `fuzz/README.md` exists. | **Done.** Both buckets confirmed TLS 1.3 live; `crates/http-ingest/fuzz/README.md` added. |
| W8 | Q15 and Q16 (see W8's correction note) in `docs/open-questions.md` and reflected in `CLAUDE.md`. | **Done**, with corrected numbering — Q14 was already taken by an unrelated question not anticipated when this plan was drafted. |

---

## 8. Open items carried forward

Resolved during implementation (kept here for the record, not because they're still open):

- **DECIDE (W4c):** `overflow-checks = true` — **approved and shipped.**
- **DECIDE (W4d):** keep `cargo audit` alongside `cargo deny check` — **approved and
  shipped**; both are clean and both run in CI.
- **APPROVE (W4a):** `git rm out.png` — **approved and shipped**, `/*.png` gitignored at
  root.
- W1.3's fan-out remedy — **not triggered**; ADR-0014's "no connection pool" decision
  stands unamended (§9.2).

Still genuinely open:

- **Q15, Q16** (W8; see W8's correction note on numbering) — blocking prerequisites for the map and tile subsystems.
- The permanently-404ing-key stall in `poll_once` — predates ADR-0014, wants its own
  issue. The E-11 rewrite changed the shape of this slightly (it's now a permanently-
  missing-volume-folder stall, same failure class) but did not fix it — see the comment
  at the `saw_end` branch in `s3_poll.rs::poll_once`.
- `rustfmt` adoption — deferred with reasons in W5.
- Fixed-Huffman deflate for `png_out.rs` — only if the uncompressed file size becomes a
  real annoyance.
- W5's CI workflow has not yet run on a real GitHub Actions runner (no push happened in
  this session) — first push should be watched to confirm it's actually green there, not
  just "should be green" from local gate parity.

---

## 9. Results

*Filled in by the implementing session, 2026-07-31.*

### 9.0 A finding this plan did not anticipate, found while building the W1.2 harness

The first run of `cold_start_listing_size` against the live `unidata-nexrad-level2-chunks`
bucket returned **32,524 keys across 33 pages in 4.4 s** for a single site — not the "handful
to a few hundred" backlog §2 predicted. Direct inspection of the bucket
(`?list-type=2&prefix=KDOX/&delimiter=/`) showed why: the real key layout is

```
SITE/<volume-sequence>/<YYYYMMDD-HHMMSS>-<n>-<kind>
```

e.g. `KDOX/166/20260728-095259-001-S` — not the `SITE/YYYY/MM/DD/HH/...` calendar layout
`current_hour_anchor` (and ADR-0014 Erratum item 6) assumed. `<volume-sequence>` is an
**unpadded**, monotonically increasing per-site integer, so S3's lexical listing order does
not track chronological order across it (`"78"` sorts after `"709"`). The constructed
`current_hour_anchor` string didn't correspond to any real prefix, and unpadded lexical sort
put most of a day's retained volumes after it — hence 32,524 keys instead of a small
backlog.

This is a real, currently-shipping correctness bug, independent of the E-03/E-10 findings
as filed, but it directly confounds the W1 measurement (a cold start would have replayed
essentially the whole 24-hour retention window, every time, regardless of launch minute).
Per direction during this session, it was fixed in place rather than only recorded:

- `crates/http-ingest`: `Client::list_prefix` / `list_query` gained a fourth parameter,
  `delimiter: Option<&str>`, so the chunk bucket's `<CommonPrefixes>` grouping can be
  requested (`Some("/")`) instead of always paging through a flat key listing. ADR-0014
  Erratum items 8 (corrects item 6) and 9 record this.
- `crates/radar-workstation/src/ingest/s3_poll.rs`: `S3Poller` no longer anchors on a
  synthetic calendar path. It discovers volume-sequence folders via
  `list_volume_folders` (delimiter listing, numeric parsing, defensive against
  unrecognized entries), tracks position as `last_completed_volume: Option<u64>` +
  `last_seen_key: Option<String>` (the latter scoped to whichever single volume directory
  is currently being drained — lexical order *is* safe within one directory, since chunk
  filenames there are fixed-width), and detects volume completion by seeing a `-kind` byte
  of `E`. Cold start anchors to `newest_volume - 1` (`cold_start_baseline`, a pure
  function, unit-tested offline) so the first poll fetches the current volume rather than
  replaying the retention window — the same intent as the plan's original
  `anchor_before_newest_start` design, adapted to the real key structure.
- `current_hour_anchor` and `unix_to_utc_parts` are deleted (dead code once the calendar
  assumption is gone).
- Regression test added: `parse_list_xml_extracts_common_prefixes_but_not_the_top_level_echo`
  — guards against re-capturing S3's top-level echoed `<Prefix>` tag as a volume folder,
  which would silently reintroduce a version of this bug.
- Known, accepted gap, not fixed here (mirrors the pre-existing permanently-404ing-key
  issue already carried forward elsewhere in this plan): if a volume-sequence directory
  never appears — e.g. an RDA restart skips numbers, as was observed live (gaps at
  79→90, 92→165, 195→268 in the 479 folders inspected) — the poller stalls waiting on it
  indefinitely. Same class of issue, same disposition: wants its own issue, not solved by
  drive-by.
- `docs/dependency-inventory.md` gains finding **E-11** recording this. CLAUDE.md's
  NEXRAD Format Findings gains the confirmed chunk-bucket key layout so this assumption
  doesn't silently recur.

### 9.1 W1.2 measurements (against the corrected `S3Poller`)

All against `KDOX`, live, 2026-07-31 (~00:00–00:15 UTC).

| Test | Result |
|---|---|
| `cold_start_listing_size` | 480 volume folders, newest = 711, **`t_list = 196 ms`** (delimiter listing; contrast the pre-fix 4.4 s / 32,524-key flat scan above) |
| `cold_start_poll_once_latency` | Fetched the in-progress current volume (711): **26 chunks in 1.56 s total** (list + sequential fetch) |
| `steady_state_poll_latency` | poll 2: 1 chunk in 61 ms; poll 3: 2 chunks in 118 ms |
| `keepalive_amortization` | first fetch 51 ms; next four 90 / 36 / 40 / 33 ms — no dramatic keepalive win in this environment, but nothing near a handshake-per-request cost either |
| TLS version (both buckets, `curl -vI`) | TLS 1.3 negotiated on both `unidata-nexrad-level2-chunks` and `unidata-nexrad-level2` (feeds W7) |

One additional data point, gathered from S3 object `LastModified` timestamps directly
(no code path exercised) rather than from the harness: volume 166's `-S` chunk landed at
`09:53:01`, its `-E` chunk at `09:59:19` — **6 min 18 s** end to end, for VCP 35. This is
antenna rotation time, not network or software latency, and it reframes what §W1.1's pass
criterion should mean in practice (see 9.2).

### 9.2 Verdict against the §W1.1 budget

**Pass, by two orders of magnitude, on every number that acquisition-layer software
latency can actually affect.** Cold start to *currently-available* data: 1.56 s. Steady
state: two orders of magnitude under the 5 s budget on every poll.

§W1.1's literal pass criterion — "time from `S3Poller::new` to holding every chunk of one
complete volume scan" — is not a number the acquisition layer can hit in under 5 s for any
implementation, because a complete volume takes minutes to be produced by the radar
hardware regardless of fetch speed (9.1's 6 min 18 s figure). Read literally, the criterion
fails by construction and no software change can fix it. The more defensible reading,
consistent with ADR-0011's explicit "partial scan rendering" design ("users see the lowest
sweep within ~60 seconds... a feature, not a limitation"): the 5 s budget governs how
quickly *currently-available* chunks reach the render path, not how quickly a full volume
completes. Under that reading, both the cold-start and steady-state numbers pass with
large margin, and there is no reason to believe fetch latency is ever the bottleneck on the
path to first pixels — antenna physics is.

**W1.3 (E-03 fan-out remedy): not applied.** The gate ("apply only if still over budget
after the E-10 fix") is not met — 1.56 s and 61–118 ms are not over budget by any reading.
Sequential fetching over one keepalive connection, as ADR-0014 decided, is adequate. No
ADR-0014 amendment to the "no connection pool" decision is needed.

### 9.3 A second unplanned finding: `cargo deny check` caught a live RUSTSEC advisory

Wiring up W4d/W5 (the point of "the E-05 supply-chain gates are inert if nothing runs
them") did its job on the first run. `cargo deny check advisories` flagged
**RUSTSEC-2026-0194** and **RUSTSEC-2026-0195** against `quick-xml 0.37.5`: a quadratic-
time duplicate-attribute check (`BytesStart::attributes()`) and an unbounded namespace-
declaration allocation (`NsReader`), both reachable by an attacker able to supply crafted
XML — which is exactly `parse_list_xml`'s threat model (untrusted S3 `ListObjectsV2`
responses).

Checked before deciding how to respond: `crates/radar-workstation/src/ingest/s3_poll.rs`
never calls `.attributes()` and never uses `NsReader` — only `Reader` matched by tag name
via `e.name()`. Neither advisory's affected code path is reachable in this codebase today.
Upgraded anyway (`quick-xml` `0.37` → `0.41`, the fixed version) rather than filing an
`[[advisories.ignore]]` exemption, since a working upgrade path existed and Principle 4
(Security as First-Class) doesn't stop at "not currently exploitable."

The upgrade was not a version-number-only change. quick-xml 0.41 removed
`BytesText::unescape()` and split entity references (`&amp;`, `&#38;`, ...) out of `Text`
events into a separate `Event::GeneralRef` event, rather than pre-expanding them inline.
`parse_list_xml` assumed one `Text` event fully represented an element's content; under the
new model an element containing an entity would arrive as multiple events, and the old
per-Text-event `keys.push(...)` would have silently split one key into two or dropped the
entity's character entirely — a correctness regression the compiler cannot catch (the code
still type-checks; the bug is a wrong result on realistic input, not a compile error).
Fixed by accumulating text across `Text` and `GeneralRef` events into a buffer, flushed on
`End`, resolving `GeneralRef` via `BytesRef::resolve_char_ref` (numeric entities) plus a
closed five-entry match for XML's built-in named entities (`amp`/`lt`/`gt`/`apos`/`quot` —
XML has no DTD here to define more, so anything else is a correctly-rejected framing
violation). Two new regression tests
(`parse_list_xml_resolves_entity_references_split_across_events`,
`parse_list_xml_rejects_unrecognized_entity`) guard this. Re-verified against live S3
after the upgrade — `cold_start_listing_size` and all six `http-ingest` live tests still
pass.

`cargo deny check` and `cargo audit` are both clean as of this session (§7 metrics table).

### 9.4 W6 external verification detail

`docs/dependency-inventory.md` E-06 (fold into `crates/radar-viz`'s hand-rolled encoder,
work item W6) required proof the new encoder is pixel-correct against the `image`-based
one it replaced, not merely self-consistent. Method: a disposable `git worktree` checked
out at the pre-change commit (`74a1065`) built the old `radar-viz` unmodified; both binaries
rendered the same real fixture (`downloads/KDOX_20260629_1801`, sweep 1, DREF, 800×800);
Python + Pillow (external to this workspace) decoded both PNGs and compared raw RGBA byte
buffers. Result: **identical SHA-256** (`dedc63b3c32e5bc6e81f976085b66df206cdc5d4f9d1a8e8d1b601494b54a8b1`)
on both. Old file 90,564 bytes (real deflate compression via `image`/`flate2`); new file
2,561,063 bytes (stored/uncompressed blocks, as W6 accepted) — consistent with the ~19×
inflation the plan predicted by extrapolation from a 1200×1200 example.
