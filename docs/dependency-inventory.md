# Dependency Inventory — post-ADR-0014

**Audited commit:** `74a1065` (working tree: `.gitignore` modified only)
**Date:** 2026-07-29
**Toolchain:** rustc 1.95.0 / cargo 1.95.0, `x86_64-unknown-linux-gnu`
**Scope:** dependency graph vs. `PHILOSOPHY.md` and the accepted ADRs
**Changes made:** none. This document is the only file added.

Finding IDs in this document use the `E-` namespace. The previous audit (against
`f512ea8`, cited in ADR-0014) used `D-`; cross-references to those IDs are preserved
so the ADR trail stays navigable.

> **Superseded — point-in-time audit (2026-07-30).** This document assesses the tree at
> `74a1065` (2026-07-29). Findings E-01…E-10 describe the **pre-remediation** tree and are
> retained as the audit trail, not as current state. The remediation that closed most of
> them is `docs/plans/dependency-inventory-remediation.md`; its §9 Results is the
> authoritative account of the current dependency posture. Findings E-11 and E-12 were
> appended 2026-07-31, after that remediation, and are the only content here written
> against the post-remediation tree.

---

## 1. Executive summary

The `reqwest` removal did what ADR-0014 said it would, and the measurements are not close:

| Metric | `f512ea8` (pre) | `74a1065` (post) | Change |
|---|---:|---:|---:|
| Packages in `Cargo.lock` | 222 | 78 | **−65%** |
| Compiled units, `-p radar-workstation --release` | 104 | **35** | **−66%** |
| Compiled units, workspace-wide release | 133 | 62 | −53% |
| Non-Rust LOC linked (C / asm / headers) | 1,345,961 | **151,501** | **−89%** |
| Native crypto stacks compiled in one build | 2 | **1** | −50% |
| Native static archive size | 7.26 MB | 5.03 MB | −31% |
| Native object files compiled | 365 | 90 | −75% |
| `fetch-sample` binary (links the network stack) | 4.80 MB | **3.07 MB** | −36% |
| Direct production deps without an ADR | 3 | **0** | resolved |

Every finding ADR-0014 set out to resolve is resolved, and I verified each one against
the code rather than the ADR's claim about it:

| Prior finding | Status | Verification |
|---|---|---|
| D-01 FIPS rationale unfounded | **Resolved** | `grep -c aws-lc Cargo.lock` → `0`. The claim is gone along with the dependency; ADR-0014 records the trade explicitly rather than restating it. |
| D-02 system trust store consulted | **Resolved** | `tls.rs:16-22` builds a `RootCertStore` from `webpki_roots::TLS_SERVER_ROOTS` and nothing else. No platform verifier in the graph. |
| D-03 two crypto stacks | **Resolved by construction** | One `http-ingest` crate; there is no second HTTP client to disagree with. |
| D-06 unused `stream` / `async-tokio` features | **Resolved** | `quick-xml = "0.37"` at audit time (now `0.41`, see E-12) with no features; no streaming API to leave dead. |
| D-07 ICU4X for one ASCII hostname | **Resolved** | No `url`, `idna`, or `icu_*` anywhere in the lockfile. Hostnames are a compile-time allowlist (`host.rs:3-6`). |
| D-08 reqwest 0.12/0.13 split | **Resolved** | Both gone. |
| D-11 `query` feature kept deliberately | **Superseded, correctly** | I previously argued *keep* `serde_urlencoded` because base64 continuation tokens contain `=`, `+`, `/`. `encode.rs` encodes strictly the RFC 3986 unreserved set, and `request.rs` has a test asserting the exact encoding of a real 100-char token. The concern was addressed rather than inherited. |
| D-12 `bytes` in the public shape | **Documented** | ADR-0017 now records it. |

Three of my prior findings are **still open** and are restated below as E-04, E-05, and
E-07 — none was in ADR-0014's scope.

The cost side is honest and was correctly predicted: the workspace now owns **1,845 lines**
of network code (1,395 production + 450 test-support) that no third party will patch.
That code is the single largest new risk in the tree, and §4 assesses it on its own terms.

---

## 2. What is actually declared

Six direct production dependencies across three first-party crates. All six now have an ADR
— the `CLAUDE.md` "ask first" gap identified in the prior audit is closed.

### `crates/nexrad-decoder`

```toml
[dependencies]
```

Empty, still. 700 lines of `std`-only Rust parsing NEXRAD Message 31. This remains the
reference standard in the workspace and is the precedent ADR-0014 explicitly builds on.

### `crates/http-ingest` — new

| Crate | Version | Features | Role | Rust-native? |
|---|---|---|---|---|
| `tokio` | 1 | `net`, `time`, `io-util`, `rt` (no defaults) | TCP, timers | yes |
| `rustls` | 0.23 | `ring`, `std`, `tls12` (no defaults) | TLS | **no** — see E-01 |
| `tokio-rustls` | 0.26 | `ring`, `tls12` (no defaults) | async TLS glue | yes |
| `webpki-roots` | 1 | — | Mozilla trust anchors | yes (data only) |
| `bytes` | 1 | — | zero-copy body handoff | yes |

Feature hygiene here is genuinely tight: `default-features = false` on all three crates
that support it, and the enabled sets are minimal. This is the crate that replaced ~100
transitive dependencies with five direct ones.

### `crates/radar-workstation`

| Crate | Version | ADR | Rust-native? |
|---|---|---|---|
| `nexrad-decoder` | path | 0008 | yes |
| `http-ingest` | path | 0014 | via `rustls` → no |
| `bzip2` | 0.6 | 0015 | **yes** — `libbz2-rs-sys`, pure Rust |
| `bytes` | 1 | 0017 | yes |
| `tokio` | 1 | 0004 | yes |
| `quick-xml` | 0.41 <!-- corrected 2026-07-30, was 0.37 at audit time; see E-12 --> | 0016 | yes |

### `utility/` — explicitly non-production

| Crate | Declared in | Notes |
|---|---|---|
| `http-ingest` | `nexrad-sample` | now shares the production client — this is what resolves D-03 |
| `tokio`, `tempfile` | `nexrad-sample` | `tempfile` is a dev-dependency |
| `image` 0.25 (`png` only) | `radar-viz` | see E-06 |
| `shapefile` 0.6 | `radar-viz` | see E-07 |

### Out-of-workspace

`crates/http-ingest/fuzz` is `exclude`d from the workspace and depends on `libfuzzer-sys`
0.4. It does not appear in `Cargo.lock` and does not affect any normal build. See E-08.

---

## 3. Non-Rust code in the graph

`ring` is now the **only** non-Rust code in the production graph. Everything else that
compiles is Rust.

| Component | Language | Vendored | Measured | Reached via |
|---|---|---:|---|---|
| `ring` 0.17.14 | C + perlasm-generated assembly | 8.2 MB | **151,501 LOC**<br>90 objects<br>5.03 MB `.a` | `rustls` (feature `ring`) |
| `libc` 0.2.186 | FFI declarations only | — | no compiled C | `tokio`, `mio`, `socket2` |
| `bzip2` 0.6.1 | **pure Rust** | — | — | → `libbz2-rs-sys` 0.2.5 |

Notes on each:

- **`ring` is the irreducible floor, not an oversight.** `rustls` has no production-quality
  pure-Rust crypto provider today; the two real options are `ring` and `aws-lc-rs`, and both
  carry C and assembly. Choosing `ring` cut the native surface from 1,345,961 lines to
  151,501 — and removed the `cmake` / `fs_extra` / `dunce` / `pkg-config` build-dependency
  cluster along with the host-probing build script that made `aws-lc-sys` a reproducibility
  hazard. `ring`'s build now uses `cc` only. This is about as good as memory-safety-by-
  construction gets while still speaking TLS.

- **`bzip2` remains a genuine win and is now documented.** `bzip2-sys` (the C binding)
  appears nowhere in the lockfile; `bzip2` 0.6 resolves to `libbz2-rs-sys`, a pure-Rust
  rewrite. This matters more than the TLS choice, because decompression processes attacker-
  influenced bytes on *every single chunk*, whereas TLS processes a framed, authenticated
  stream. ADR-0015 now records this as a decision.

- **`webpki-roots` is data, not code.** A compiled-in Mozilla root bundle. This is what
  makes the "no runtime dependency on the host trust store" claim true this time.

---

## 4. The new risk: 1,845 lines of first-party network code

This replaces a dependency risk with a code risk. That trade is the whole point of
ADR-0014, but it has to be assessed rather than assumed, so I read all of it.

| Module | Lines | What it does |
|---|---:|---|
| `response.rs` | 598 | status-line / header / chunk-size parsing — the security-critical core |
| `connection.rs` | 508 | keepalive, timeouts, body reads, chunked decoding |
| `request.rs` | 131 | request formatting |
| `test_server.rs` | 101 | `#![cfg(test)]` scripted plaintext server |
| `lib.rs` | 97 | `Client`, retry policy |
| `encode.rs` | 89 | percent-encoding |
| `host.rs` | 84 | compile-time host allowlist |
| `error.rs` | 53 | error taxonomy |
| `config.rs` | 43 | timeouts and limits |
| `tests/live_s3.rs` | 89 | live-network tests, all `#[ignore]`d |

**Verification status — measured, not asserted:**

- `cargo test --workspace`: **111 passed, 0 failed, 7 ignored**. 67 of those tests are in
  `http-ingest` alone — roughly one test per 20 lines of production code.
- `cargo clippy --workspace --all-targets`: **clean**, no warnings.
- The 7 ignored tests are all network-dependent (5 in `live_s3.rs`, 2 in
  `data_acquisition.rs`). Correctly gated — `cargo test` never touches the network, which
  matters for both CI determinism and the no-undisclosed-connections principle.
- A `cargo-fuzz` target exists for `parse_head`, with a **31-file corpus**.

**What the implementation gets right, and why it lowers the risk of owning this:**

- **Every framing rule is a named error with a test.** `Error::Protocol(&'static str)`
  carries the specific rule that fired, and the test matrix asserts on the rule name:
  bare LF, bare CR, duplicate `Content-Length`, duplicate `Transfer-Encoding`,
  `Content-Length` + `Transfer-Encoding` together, obs-fold, space-before-colon, non-tchar
  header names, `1xx`, chunk extensions. These are exactly the response-smuggling shapes.
- **The fuzz corpus is reachable from stable `cargo test`.**
  `response.rs:536` (`fuzz_corpus_never_panics`) and `:561` (`mutated_inputs_never_panic`,
  a seeded xorshift mutator over the corpus) mean a regression fails an ordinary test run,
  not just a nightly fuzz session someone remembers to launch. This is the single best
  decision in the crate — it converts fuzzing from a ritual into a gate.
- **Defense in depth beyond parsing.** ALPN pinned to `http/1.1` so h2 can never be
  negotiated (`tls.rs:27`); a compile-time host allowlist with tests for prefix, suffix,
  case-variant, embedded-port, and userinfo bypass attempts (`host.rs`); CR/LF rejected in
  every encoder input before a byte is sent (`encode.rs:7-12`); four independent timeout
  phases; every buffer bounded by an explicit limit.
- **The retry rule is argued, not guessed.** `lib.rs:72-84` retries once, only on a
  *reused* connection that closed before any response byte — which cannot double-deliver.
  Non-idempotent double-fetch is the classic bug here and it is closed by construction.
- **`test_server.rs` does not ship.** `#![cfg(test)]` at file scope; verified.

**E-02 — the ADR body and its erratum disagree about `httparse`.** `low`

**Resolved** by W2 of the remediation plan. Verify against: ADR-0014's Decision table vs.
`crates/http-ingest/Cargo.toml` — the erratum is now folded into the table body.

ADR-0014's Decision table lists `httparse` as a direct dependency with the rationale
*"Alternative to hand-rolling."* Erratum item 2 correctly states it was not taken. A
reader who consults the table — the part of the document that looks authoritative for
"what are the dependencies" — gets the wrong answer and has to reach the erratum to
correct it. Worth folding the erratum into the table body.

*Effort: 10 minutes. No code impact.*

**E-03 — chunk fetches are now serialized.** `medium — verify against the perf target`

**Resolved — measured, not over budget.** Fan-out was not applied. Verify against: the
remediation plan's §9.1 and §9.2.

The prior implementation fetched a batch concurrently via `tokio::task::JoinSet`. ADR-0014
chose a single keepalive connection with no pool, so `s3_poll.rs:104-108` now fetches
sequentially. The code comment is honest about why, and the error semantics actually got
*simpler* — batch-atomic retry now falls out of the first `?` rather than needing the
all-slots-filled dance.

But this is a real latency change on the path that `rendering.md` budgets at **<5 s to
display after site change**, and it is currently unmeasured. A cold start reads a full
hour-prefix listing and then fetches every chunk in it one at a time. TLS session
resumption and keepalive make each subsequent request cheap, so this may well be fine —
but "may well be fine" is not the standard this project sets for the tornado-warning case.

*Effort to measure: ~1 hour with the live tests. Effort to fix if needed: a small
connection pool, or two to four `Client` instances fanned out over the batch — the
`Client`-per-host shape makes that a local change. Recommend measuring before building
anything.*

**Assessment.** For code on an untrusted-input path, this is a well-run crate: the parser
is a pure function, the rejection rules are enumerated and individually tested, fuzz
findings are gated on stable, and the network surface is narrowed at four independent
layers. It does not eliminate the perpetual-maintenance cost ADR-0014 accepted, but it
substantially de-risks it. My prior audit argued the decoder's zero-dependency posture was
the standard the rest of the tree should meet; `http-ingest` meets it.

---

## 5. Open findings

### E-01 — `ring` is the sole remaining native dependency `informational`

151,501 lines of C and generated assembly, 90 objects, a 5.03 MB static archive. Not
removable while using `rustls` — no production-quality pure-Rust provider exists. Recorded
for the audit trail, not as an action item.

One available tightening: `rustls` and `tokio-rustls` both enable the `tls12` feature.
S3 supports TLS 1.3, and rustls enables 1.3 unconditionally, so dropping `tls12` would
narrow the protocol surface to 1.3-only. That trades a little resilience (no fallback if an
endpoint or a middlebox on the operator's network only offers 1.2) for a smaller attack
surface. Given the deployment context — operators on arbitrary networks during severe
weather — **keeping `tls12` is the defensible call**, and it should be a recorded decision
rather than a default. *Effort: one line either way, plus a sentence in ADR-0014.*

### E-04 — `default-members` is still unset `medium` *(was D-04)*

**Resolved** by W3 of the remediation plan. Verify against: root `Cargo.toml`'s
`default-members`, which now scopes the default build to the three production crates.

`CLAUDE.md`: *"The utility/ directory contains tools that are strictly not intended for
production."* The workspace root has `exclude` for the fuzz directory but no
`default-members`, so a bare `cargo build` still compiles **62 units instead of 35** —
pulling `image`, `shapefile`, `dbase`, `time`, `png`, `flate2`, `moxcms`, `pxfm` and
others, and letting their features unify with the production graph.

This is materially less dangerous than it was pre-refactor: `http-ingest` is now the only
HTTP client, so there is no cross-crate feature unification that can silently double a
crypto stack. It is now a build-time cost and a hygiene issue rather than a correctness
hazard. Still worth one line.

```toml
default-members = ["crates/radar-workstation", "crates/nexrad-decoder", "crates/http-ingest"]
```

*Effort: one line. Saves 27 compiled units off the default build.*

### E-05 — reproducibility scaffolding still absent `medium` *(was D-05)*

**Resolved** by W4 of the remediation plan — all four pieces. Verify against:
`rust-toolchain.toml`, `deny.toml`, the `[profile.release]` block in the root
`Cargo.toml`, and `.gitignore` (the `Cargo.lock` entry is gone).

Principle 7 names reproducible builds. Still missing (at the time of this audit):

- no `rust-toolchain.toml` — nothing pins the compiler
- no `deny.toml` / `cargo-deny`, no `cargo-vet`
- no `.cargo/config.toml`
- **no `[profile.release]` section anywhere** — so no LTO, no `codegen-units` tuning, and
  no `panic` policy. That last one is worth a deliberate decision given Principle 2: the
  default `panic = "unwind"` is almost certainly right for this application (a panic in one
  task should not take down a workstation mid-warning), but it should be a recorded choice.
- `cargo audit` is in the `CLAUDE.md` build commands but `cargo-audit` is not installed on
  this machine.

`Cargo.lock` is committed but is *also* still listed in `.gitignore`. Git ignores that for
tracked files, so nothing is broken — it remains a trap for a future contributor or a
`git clean -X`. The entry should be deleted; a binary workspace wants its lockfile tracked.

The refactor made this cheaper to fix, not harder: with `aws-lc-sys`'s host-probing build
script gone, a pinned toolchain plus a tracked lockfile gets genuinely close to
reproducible.

*Effort: ~2 hours total.*

### E-06 — `image` still pulls two 0.x single-author crates `low` *(was D-10)*

**Resolved** by W6 of the remediation plan — hand-rolled encoder. Verify against:
`utility/radar-viz/Cargo.toml` (no `image` dependency) and `src/png_out.rs`.

`radar-viz` declares (at the time of this audit) `image 0.25` with `default-features = false, features = ["png"]` —
correct hygiene — and still receives `moxcms` 0.8.1 (color management, single author) and
`pxfm` 0.1.29 (float math, single author, **0.1.x**, no `repository` field in its
manifest), plus `flate2`, `miniz_oxide`, `fdeflate`, `bytemuck`, `num-traits`,
`byteorder-lite`, `simd-adler32`, `crc32fast`, `adler2`.

The code uses `image` only as `ImageBuffer<Rgba<u8>, Vec<u8>>` with `put_pixel`
(`render.rs`, `overlay.rs`). Depending on `png` 0.18 directly and carrying a `Vec<u8>`
would drop ~8 crates for roughly 40 lines.

*Effort: ~1 hour. Priority: low on its own — it is a dev tool — but it should not be the
pattern that reaches `crates/` when the real render path needs a raster encoder.*

### E-07 — ADR-0006 still plans a single-maintainer parser for production `medium` *(was D-09)*

**Still open.** Now also tracked as `docs/open-questions.md` Q15 — that is the live version
of this finding.

`shapefile` 0.6 and `dbase` 0.5 are both 0.x, both from a single maintainer, and `dbase`
pulls `time` 0.3 for DBF date fields. Confined to `utility/radar-viz` today, which is fine.
ADR-0006 designates `shapefile` — alongside `geo` and `lyon` — for **production** overlay
loading.

That puts a 0.x single-maintainer binary parser on a startup path that must not panic. The
shapefile spec has been frozen since 1998, and the two geometry types needed (Polyline,
Polygon) are simple.

**The stronger move is to skip the parser entirely.** ADR-0006 already says overlays are
pre-projected at load time. Pre-project them at *build* time into a flat bundled format the
app `mmap`s: no `shapefile`, no `dbase`, no `time`, no `geo` in the shipped binary, a whole
class of startup panic removed, and faster startup against the <2 s target.

ADR-0014 has now established the precedent and the muscle memory for exactly this kind of
narrow-scope ownership. This is the natural next application of it.

*Effort: ~1 day for a build-time projection tool. Saves 4+ production crates.*

### E-08 — `libfuzzer-sys` needs nightly and a C++ toolchain `informational`

The fuzz crate is `exclude`d from the workspace and absent from `Cargo.lock`, so it costs
nothing in a normal build — correctly structured. Worth recording that actually *running*
it needs nightly Rust, `cargo-fuzz`, and LLVM's libFuzzer (C++). The corpus-regression
tests in `response.rs` are what make this acceptable: the safety property is enforced on
stable, and the nightly fuzzer is an amplifier rather than the only line of defense.

### E-11 — `S3Poller`'s cold-start anchor targeted a key layout the bucket doesn't use `high — correctness, found and fixed 2026-07-31`

Not in this audit's original scope; found while building the W1 timing harness in
`docs/plans/dependency-inventory-remediation.md` and fixed in the same session (see that
plan's §9 Results for the full account). Recorded here because it is more severe than
anything else in this document — a correctness bug on the primary data-acquisition path,
not a hygiene or dependency-shape issue.

`current_hour_anchor` constructed `SITE/YYYY/MM/DD/HH/` as the cold-start `start-after`
anchor, and ADR-0014 Erratum item 6 asserted the same layout for the archive-bucket
comparison. The real `unidata-nexrad-level2-chunks` layout, confirmed by direct bucket
inspection, is `SITE/<volume-sequence>/<timestamp>-<n>-<kind>`, where the volume-sequence
directory is an **unpadded** monotonically increasing integer — lexically, `"78"` sorts
after `"709"`. A live measurement against the unfixed code returned 32,524 keys (a near-
complete day's retention) instead of a small hour-boundary backlog, because the
constructed anchor matched no real prefix.

Fixed: `S3Poller` now enumerates volume-sequence folders via S3's `delimiter=/`
(`Client::list_prefix` gained a `delimiter` parameter — ADR-0014 Erratum item 9) and
anchors numerically on the newest volume, rather than constructing a calendar path.
CLAUDE.md's NEXRAD Format Findings now records the confirmed layout so the assumption
doesn't recur.

*Effort: found opportunistically; fix was ~2 hours including tests. No dependency impact.*

### E-12 — `quick-xml 0.37.5` carried a live RUSTSEC advisory pair, on the untrusted-XML path `high — found and fixed 2026-07-31`

Also not in this audit's original scope; found because W4/W5 of
`docs/plans/dependency-inventory-remediation.md` wired up `cargo deny check` for the first
time and it flagged something real on its first run. RUSTSEC-2026-0194 (quadratic-time
duplicate-attribute check in `BytesStart::attributes()`) and RUSTSEC-2026-0195 (unbounded
allocation in `NsReader`'s namespace resolution) both apply to `quick-xml 0.37.5`, which
`radar-workstation` uses to parse S3 `ListObjectsV2` responses — untrusted network input,
exactly the threat model both advisories describe.

Checked before acting: `parse_list_xml` never calls `.attributes()` and never uses
`NsReader`, so neither advisory's affected code path is reachable in this codebase as
written. Upgraded to `quick-xml 0.41` anyway (the fixed version) rather than filing an
exemption, since dropping a known-vulnerable pin without a reason not to is the
conservative move under Principle 4 (Security as First-Class). The upgrade was not
mechanical — 0.41 changed how entity references are surfaced from the event stream, which
required a corresponding fix in `parse_list_xml` (see the plan's §9.3 for the full
account) — but is otherwise inert. Confirmed live against S3 both before and after.

*Effort: ~1.5 hours, half of it the parser fix. Version bump, no new dependency.*

### E-09 — the client cannot serve ADR-0007 `medium — architectural, plan now`

**Closed 2026-08-28 by [ADR-0026](adr/0026-tile-http-boundary.md)**, which resolves Q16.
The finding was correct and the recommendation below (option 2, a second client crate) was
not taken. Option 2's *goal* — a tile path structurally unable to affect the radar path —
is exactly what shipped; its *mechanism* was rejected, because a second client means a
second copy of `connection.rs` + `response.rs` (~1,065 lines) and of the 31-file fuzz
corpus, concentrating divergence risk on the most security-sensitive code in the workspace
and contradicting `CLAUDE.md`'s DRY instruction. ADR-0026 splits `http-ingest` by layer
instead — one engine, two sibling policy crates (`s3-fetch`, `tile-fetch`) — reaching the
same isolation for ~300 lines and zero new dependencies, because the seam already existed
inside the crate. The answer to this finding's closing question is that `http-ingest`'s
allowlist design is a **permanent** asset for the path it guards: on the radar path it is
strengthened from a string match to a `Bucket` enum. Three of the capability premises
below (HTTP/2, redirects, and the difficulty of conditional requests) did not survive
measurement against the live providers — see ADR-0026's Context. The original text is
preserved below for the audit trail.

The successor finding is `docs/open-questions.md` **Q18**: tile bodies are JPEG/PNG, and
decoding them is a new untrusted-input parser on a network path — a larger dependency
surface than the transport question this finding raised.

This is the one genuinely *new* forward-looking gap, and it follows directly from
ADR-0014's own scope boundaries, which explicitly exclude arbitrary URL parsing, redirect
following, and *"serving as a general-purpose HTTP client for other crates in the
workspace."*

ADR-0007 (Pluggable XYZ tile providers) requires the opposite: a user-supplied URL template
against an **arbitrary** host, which in practice also means redirects, `ETag` /
`If-None-Match` for the disk cache, and probably HTTP/2. None of that fits behind a
compile-time host allowlist.

So the tile subsystem faces a three-way choice that should be decided before it is built:

1. **Generalize `http-ingest`** — runtime-configurable hosts, redirect policy, conditional
   requests. Directly contradicts ADR-0014's scope boundaries, which say a need like this
   is *"a signal to reopen this ADR, not to grow the crate."* Reopening is legitimate; the
   risk is that the crate drifts back toward being a general HTTP client, at which point
   the reqwest argument reappears with the maintenance burden now internal.
2. **A second, separate client crate for tiles** — keeps the S3 boundary hardened and
   auditable, at the cost of two HTTP implementations. Defensible if the tile path is
   treated as lower-trust: tiles are cosmetic, cached to disk, and a failure degrades the
   basemap rather than the radar product.
3. **Reintroduce a third-party client for tiles only** — worst option. It re-imports most
   of what ADR-0014 removed, for the *less* important data path.

My read: option 2, with the tile client explicitly scoped as best-effort and unable to
affect the radar path. But this is an architecture decision, not an audit finding, and it
deserves its own ADR before any tile code is written — the answer determines whether
`http-ingest`'s allowlist design is a permanent asset or a temporary one.

---

## 6. Version duplication

Down from seven duplicated crates to two, both benign:

| Crate | Versions | Cause | Compiles in a release build? |
|---|---|---|---|
| `getrandom` | 0.2.17, 0.4.3 | 0.2 via `ring` (production); 0.4 via `tempfile` (dev-dependency of `nexrad-sample`) | only 0.2 |
| `windows-sys` | 0.52.0, 0.61.2 | 0.52 via `ring`; 0.61 via `tokio`, `mio`, `socket2`, `rustix`, `errno`, `tempfile` | neither — Linux target |

Neither is worth acting on. The `getrandom` split is a production/dev boundary, not a
conflict, and the `windows-sys` split never leaves the lockfile on this platform.

Of the 78 lockfile packages, **21 never compile on this target**: eleven `windows-*`
crates, `wasi`, `r-efi`, `serde_core` / `serde_derive` (optional features of
`shapefile`/`dbase` that are never enabled), and the `tempfile` / `rustix` / `errno` /
`fastrand` / `linux-raw-sys` cluster which is dev-dependency-only. The real compiled
surface is 35 units for the application and 62 for the full workspace including dev tools.

This still matters for `cargo audit` and for anyone reviewing the vendored manifest, but at
78 packages the whole graph is now small enough to read in one sitting — which was the
actual goal.

---

## 7. Scorecard against `PHILOSOPHY.md`

| Principle | Pre (`f512ea8`) | Post (`74a1065`) | Evidence |
|---|---|---|---|
| No undisclosed network connections | holds | **holds, strengthened** | Compile-time host allowlist (`host.rs`); no redirects; no proxy support; no telemetry crate; all 7 network tests `#[ignore]`d so `cargo test` never dials out. |
| Custom decoder, no third-party (ADR-0008) | exemplary | **exemplary** | `nexrad-decoder` still has an empty `[dependencies]`. |
| Memory-safe by construction | **at risk** | **acceptable** | Non-Rust surface cut 89%, from 1,345,961 to 151,501 LOC. The remainder is irreducible for TLS. Decompression — the untrusted-input path — is now pure Rust. |
| Minimal dependencies, auditable | **at risk** | **holds** | 104 → 35 compiled units for the app. Every direct dependency has an ADR. At audit time: still no `cargo-deny` config (E-05) — **corrected 2026-07-30: `deny.toml` now exists and `cargo deny check` runs in CI; E-05 is resolved.** |
| Reproducible builds | gaps | **gaps, but smaller** | `aws-lc-sys`'s host-probing C build is gone. At audit time: no toolchain pin, no `[profile.release]`, lockfile still listed in `.gitignore` (E-05) — **corrected 2026-07-30: all three are now false; `rust-toolchain.toml`, `[profile.release]`, and a tracked `Cargo.lock` all exist; E-05 is resolved.** |
| Lightweight by design | unproven | **improving, still unproven** | `fetch-sample` 4.80 → 3.07 MB. `radar-workstation` is still 447 KB *only because `main.rs` is a 4-line stub* — the linker strips the network tree. Treat 3.07 MB as the floor before egui/wgpu. |
| Stability is a trust relationship | — | **strong on the new code** | 111 tests green, clippy clean, fuzz corpus gated on stable `cargo test`, four independent timeout phases, every buffer explicitly bounded. Untested: the serialized-fetch latency budget (E-03). |
| Dependencies chosen conservatively | gaps | **holds** | ADRs 0014–0017 close the three-dependency documentation gap. ADR-0013 correctly marked superseded, retained for continuity. |
| Clean, uncomplex code | — | **holds** | The one place ownership was rejected on principle is XML parsing (ADR-0016), which is the right call — XML edge cases are where hand-rolling actually loses. |

---

## 8. Recommended order of work

**Corrected 2026-07-30: items 1, 2, 3, 4, 7, and 8 below are complete** (E-02, E-04, E-05,
E-03, E-06, E-01 — see the inline markers in §5). Only E-07 (item 6) and E-09 (item 5)
remain, and both are now tracked live as `docs/open-questions.md` Q15 and Q16
respectively. The list below is preserved as originally written, for the audit trail.

Nothing here is urgent; the tree is in good shape. Ordered by value per unit of effort:

1. **E-02** — fold the erratum into ADR-0014's dependency table. *10 min.*
2. **E-04** — add `default-members`. *1 line.*
3. **E-05** — delete `Cargo.lock` from `.gitignore`; add `rust-toolchain.toml`,
   `[profile.release]` with an explicit `panic` policy, and a `deny.toml`. *~2 hours.*
4. **E-03** — measure cold-start batch fetch latency against the <5 s target before
   deciding whether serialization needs fixing. *~1 hour.*
5. **E-09** — write the ADR for the tile-provider HTTP boundary before writing tile code.
   *Design, not implementation.*
6. **E-07** — decide the shapefile question; prefer build-time pre-projection over a
   production parser dependency. *~1 day when the map layer starts.*
7. **E-06** — drop `image` for `png` in `radar-viz`. *~1 hour, low priority.*
8. **E-01** — record the `tls12` decision either way. *1 sentence.*

---

## 9. Method

All figures are reproducible on this checkout. The environment was offline, which is why
crates.io download counts and issue-tracker signals are absent from the risk assessment in
§4 and E-06/E-07 — those rest on graph position, version number, and authorship metadata
from the vendored manifests.

```sh
# compiled units: app-only vs. whole workspace
RUSTC_BOOTSTRAP=1 cargo build -p radar-workstation --release \
    --unit-graph -Z unstable-options        # -> 35 unique target names
RUSTC_BOOTSTRAP=1 cargo build --release \
    --unit-graph -Z unstable-options        # -> 62

# native surface
find ~/.cargo/registry/src/*/ring-0.17.14 \
    \( -name '*.c' -o -name '*.h' -o -name '*.S' -o -name '*.inl' \) \
    | xargs cat | wc -l                     # -> 151501
find target/release/build/ring-*/out -name '*.o' | wc -l          # -> 90
ls -la target/release/build/ring-*/out/libring_core_0_17_14_.a    # -> 5028386 bytes

# confirm aws-lc is gone
grep -c 'aws-lc' Cargo.lock                 # -> 0

# duplication and lock-only packages: parsed from Cargo.lock directly, since
# `cargo tree -i` hides deps not built for the host target

# verification
cargo test --offline --workspace            # -> 111 passed, 0 failed, 7 ignored
cargo clippy --offline --workspace --all-targets   # -> clean
cargo build --offline --release --workspace
ls -la target/release/{radar-workstation,fetch-sample}
    # -> 446600 / 3073288 bytes

# cert-store behavior (the D-02 recheck)
sed -n '13,33p' crates/http-ingest/src/tls.rs
```

One caveat on the unit counts: 35 and 62 are unique target names in the unit graph, which
include build scripts and count a crate's lib and bin separately. Read them as "about 30
third-party crates compile for the application" rather than as exact crate counts. The
pre-refactor figures (104 / 133) were measured the same way, so the comparison is
apples-to-apples.

Stale `target/release/build/aws-lc-sys-*` directories remain on disk from before the
refactor. They are build cache, not graph members — `cargo clean` removes them, and
`Cargo.lock` confirms the dependency is gone.

---

## Addendum — Stage 4 render stack (2026-08-28)

Written against the post-Stage-4 tree. ADR-0022 is the decision record; this is the
dependency-posture accounting the plan (§1, §12) requires be kept here rather than only
in the ADR.

**Five direct dependencies added** to `crates/radar-workstation`: `winit =0.30.13`,
`wgpu =30.0.1`, `egui =0.36.1`, `egui-wgpu =0.36.1`, `egui-winit =0.36.1`. Versions are
pinned with `=` and matched to `egui-wgpu`/`egui-winit`'s own manifests, not guessed
(ADR-0022's table). `winit` and `wgpu` carry `default-features = false` with an explicit
feature list — see below.

| Metric | Before (Stage 3) | After (Stage 4) |
|---|---|---|
| `Cargo.lock` package count | 67 | 337 |
| Release binary size (bytes) | 2,964,688 | 17,546,712 |
| Crates with a duplicate version | 2 (getrandom, windows-sys — both assessed benign) | 8: `getrandom`, `hashbrown`, `linux-raw-sys`, `rustc-hash`, `rustix`, `syn`, `thiserror`(+`-impl`) |
| `cargo audit` | clean | clean |
| `cargo deny check` | clean | clean (licence allowlist +4) |
| `unsafe` in first-party code | 0 | 0 (`render/` included, tests included) |

**This is the largest single dependency step the project will take** (roughly 5×). It is
recorded as a decision (NFR-SEC-2), not absorbed silently. The plan estimated
~230–260 packages; the real figure is 337, driven mostly by `naga` (the WGSL compiler),
`wgpu-hal`'s backend crates, and egui's text stack (`skrifa`, `harfrust`, `epaint`,
`vello_cpu`).

**Licence allowlist expansion** (`deny.toml`), one entry at a time, each commented with
the requiring crate: `BSD-2-Clause` (arrayref, via wgpu-hal), `Zlib` (foldhash, via
egui/ahash), `OFL-1.1` + `Ubuntu-font-1.0` (`epaint_default_fonts` — egui's bundled UI
fonts). All permissive. **Nothing copyleft beyond `MPL-2.0` appeared** — had it, ADR-0009
says stop and treat it as an open-source-scope question, not a `deny.toml` edit.

**`ttf-parser` (RUSTSEC-2026-0192, "unmaintained")** would have entered the tree via
`winit`'s default `wayland-csd-adwaita` feature → `sctk-adwaita` → `ab_glyph` →
`owned_ttf_parser`. `winit` is added with that feature omitted (the compositor draws our
window decorations; we do not need egui-drawn client-side ones), so the advisory does
**not** appear. `cargo audit` and `cargo deny check advisories` are both clean.

**Duplicate versions** (`[bans].multiple-versions = "warn"`, unchanged): the 8 duplicated
crates are proc-macro / `no_std`-helper splits deep in the build graph
(`syn` 1↔2, `thiserror` 1↔2, `rustix`/`linux-raw-sys` old↔new, `hashbrown` 0.16↔0.17,
`rustc-hash` 1↔2, `getrandom` 0.2/0.3/0.4). None is runtime-code divergence in the
application's own path. Assessed benign, consistent with §6's posture; not skipped in
`deny.toml`.

**Binary size** (+14.6 MB) is dominated by egui's bundled fonts (~1–2 MB compressed,
larger uncompressed in the binary) and the naga/wgpu code. The fonts stay for Stage 4 —
the status bar, legend, and cursor readout need real text. Font trimming (a reduced glyph
set, or a smaller face) is a Stage 8 packaging lever, not a Stage 4 optimisation.
