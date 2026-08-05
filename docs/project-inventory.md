# Project Inventory — status against the v1.0 objective

**Audited commit:** `668b1ca` (working tree: `.gitignore` modified, `.github/` untracked)
**Date:** 2026-07-30
**Toolchain:** rustc 1.95.0 / cargo 1.95.0, `x86_64-unknown-linux-gnu`
**Scope:** every crate under `crates/` and `utility/`, every tracked document, read against
`docs/REQUIREMENTS.md` §6 (the authoritative v1.0 scope boundary)
**Changes made:** none. This document is the only file added.

This is an inventory, not a plan. It records what exists, what does not, and the order in
which the missing work should be taken up. It does not decide any open question, propose a
design, or estimate effort in hours.

> **Superseded for current state, 2026-07-31:** Stage 2 (items 6–9 below) is complete —
> see `docs/plans/stage-2-make-the-application-exist.md` §12 for what was built and
> measured. This document's numbers (test counts, LOC, open-question counts) reflect the
> `668b1ca` snapshot and are not re-audited here; the plan's own Results section is the
> current source of truth for Stage 2, the same relationship `dependency-inventory.md`
> has to its own remediation plan.

---

## 1. Executive summary

**The project has one complete vertical slice — bytes off the network, through
decompression, into decoded radials — and nothing above it.** Everything from the volume
assembly state machine upward (assembly, compute, shared state, render, UI, map, tiles,
placefiles, config) is documented in detail and unwritten in code. `main.rs` is a
four-line stub.

Measured against the fourteen bullets of the v1.0 In-Scope list in `REQUIREMENTS.md` §6:
**zero are user-visible today**, because there is no application to see them in. That is
the honest headline. It is also less discouraging than it sounds — the parts that are done
are the parts that are hardest to get right *quietly* (binary format parsing, an
own-the-boundary HTTP client on an untrusted-input path), they are tested to a standard
well above typical, and the design work above them is unusually complete.

| Dimension | State |
|---|---|
| Production code | 3,291 lines of `src/` across three crates — `http-ingest` 1,760, `nexrad-decoder` 676, `radar-workstation` 855 — plus 492 lines in `tests/`. Inline `#[cfg(test)]` modules are a substantial share of the `src/` figures, particularly `http-ingest`. |
| Tests | 123 passing, 12 `#[ignore]`d live-network, 0 failing. `clippy -D warnings`, `cargo deny check`, `cargo audit` all clean |
| Architecture documentation | Complete for every planned subsystem — 17 ADRs, 5 architecture documents, a 526-line requirements spec |
| Open design questions | 12 open (Q4–Q9, Q11–Q17), 4 resolved (Q1–Q3, Q10). Six of the twelve block a subsystem that has not started |
| Working-tree debt | CI workflow (`.github/workflows/ci.yml`) is written but **untracked** — the supply-chain gates it runs are not actually running anywhere |

The single largest risk to the schedule is not any of the unwritten code. It is that
**the entire rendering half of the application is still gated on unanswered questions**
(Q4, Q11, Q15, Q16, Q17), and two of those (Q15, Q16) call accepted ADRs into question
rather than merely filling them in.

---

## 2. Complete

### 2.1 `crates/http-ingest` — HTTP/1.1 client for the S3 boundary

Implements ADR-0014 in full. Owns TLS setup, connection lifecycle, request encoding,
response framing, and a compile-time host allowlist.

- `Client::list_prefix` (with `delimiter` support for `CommonPrefixes`) and
  `Client::get_object` — the only two operations S3 acquisition needs
- Single keepalive connection, deliberately no pool; single idempotent retry, correctly
  scoped to reused-connection-closed-before-response only ([lib.rs:70-88](../crates/http-ingest/src/lib.rs#L70-L88))
- Typed `Error` with a `Phase` discriminant; configurable `Limits` and `Timeouts`
- 68 unit tests, including a 31-file fuzz corpus plus a seeded mutator that run on
  **stable `cargo test`**, not only under `cargo fuzz`
- 6 `#[ignore]`d live-S3 integration tests
- A `cargo-fuzz` target exists out-of-workspace (`crates/http-ingest/fuzz/`)

Measured live 2026-07-31: 196 ms to list 480 volume folders; 51 ms first fetch, 33–90 ms
subsequent. Adequate by two orders of magnitude against the 5 s site-change budget.

### 2.2 `crates/nexrad-decoder` — Message 31 decoding

Implements ADR-0008. Parses a decompressed message stream into `Vec<Radial>`.

- `parse_radial_stream` — message framing, legacy-record skipping, 4-byte record alignment
- RVOL / RELV / RRAD constant blocks, including both RRAD versions distinguished by
  `block_size`, with correct unit scaling (1/8 km, 0.01 m/s)
- All seven moment blocks (DREF, DVEL, DSW, DZDR, DPHI, DRHO, DCFP), 8- and 16-bit word
  sizes, `(raw − offset) / scale` conversion to physical units
- ICD reserved codes (below threshold, range folded) return `None`, not a bogus value
- Typed `DecodeError`; no `unwrap` on radar bytes; `overflow-checks = true` in release
- 24 tests against 5 real KDOX fixtures, one per chunk kind
- Byte-level format reference (`docs/architecture/nexrad-binary-format.md`) matches the code

### 2.3 `crates/radar-workstation` — chunk ingest layer

- `chunk.rs` — chunk kind detection (`-S` / `-I` / `-E`, including the signed-negative
  length that marks end-of-volume) and BZ2 decompression of the length-prefixed envelope
- `ingest/s3_poll.rs` — `S3Poller`: volume-folder discovery via delimiter listing with
  numeric prefix parsing, cold-start anchoring one volume behind newest, `start-after`
  scoped correctly to within a single volume directory, `-E` detection for volume advance,
  `ListObjectsV2` XML parsing, `ChunkEnvelope` delivery over an mpsc channel
- 16 unit tests + 4 `#[ignore]`d live tests

This layer satisfies FR-DA-1 and FR-DA-2 as written.

### 2.4 Supply-chain and build posture

`Cargo.lock` committed; `rust-toolchain.toml` pinning 1.95.0; `deny.toml`;
`[profile.release]` with an explicit and reasoned `panic = "unwind"` / `overflow-checks`
policy; `default-members` scoping bare `cargo build` to the three production crates.
78 packages in the lock file, down from 222 pre-ADR-0014. `ring` (via `rustls`) is the
only non-Rust code in the production graph.

### 2.5 Documentation

`PHILOSOPHY.md`, `REQUIREMENTS.md` (all FR/BC/NFR IDs, with open items explicitly marked
and cross-linked), five architecture documents, seventeen ADRs, root `README.md`,
`docs/README.md`, `CONTRIBUTING.md`, `SECURITY.md`, `utility/README.md`. Two point-in-time
audits (`documentation-inventory.md`, `dependency-inventory.md`) and their two remediation
plans, all with Results sections filled in.

### 2.6 Developer utilities (explicitly not production)

- `utility/nexrad-sample` — `fetch-sample` / `decode-sample` binaries
- `utility/radar-viz` — CPU PPI renderer to PNG, with a color table module, a shapefile
  overlay module, and a hand-rolled PNG encoder. **This is a de-risking asset**: polar-to-
  Cartesian projection, color mapping, and vector overlay drawing have each been proven
  once against real data, in throwaway form, before the GPU versions are written.
- `utility/nexrad-inspect` — Python inspection tooling that produced the format findings

---

## 3. Partially complete

| Item | What exists | What is missing |
|---|---|---|
| Decoder vs. FR-ND-8 | 24 tests, 5 fixtures | One site, one VCP, one era, super-res only, dual-pol only, one truncation case, **no corrupt-input tests, no fuzz corpus**. `TESTING.md` states this plainly. |
| `-S` chunk decoding | Chunk detected, decompressed | Messages 2, 3, 5, 15, 18 are silently skipped. ADR-0012's `VolumeContext` initialization depends on decoding Message 5 (VCP) at minimum. |
| `VolumeScan` / `Sweep` / `VolumeStatus` types | Declared in `nexrad-decoder/src/types/` with the ADR-0012 status variants | **Nothing constructs them.** No code path anywhere produces a `Sweep` or a `VolumeScan`. |
| CI | `.github/workflows/ci.yml` written, pinned by SHA, covers build/test/clippy/deny/audit | Untracked — not committed, therefore not running. |
| Poller robustness | Cold start, steady state, volume advance all correct and measured | Known accepted gap: a skipped volume-sequence number (observed live: 79→90, 92→165, 195→268) **stalls the poller indefinitely**. FR-DA-5 (graceful network failure, status surfacing) is not implemented. |

---

## 4. Not started

Every item below is designed in `docs/` and has zero implementation.

**Data pipeline**
- Volume assembly state machine (ADR-0012) — IDLE → AWAITING_DATA → ACCUMULATING, sweep
  closure, late-data discard, watchdog timeout, `Superseded` / `TimedOut` closure
- Site-change cancellation and state clearing (FR-DA-4)
- Chunk-stream → assembled-volume failover (FR-DA-8, blocked on Q14)
- Placefile fetching (FR-DA-7) and tile fetching (FR-DA-6, blocked on Q16)

**Compute layer** — nothing exists
- rayon parallel product derivation; Echo Tops; VIL
- Color table parsing and application (blocked on Q11)
- RGBA texture generation (blocked on Q17, Q8)

**Shared application state** — nothing exists
- `AppState` struct, lock granularity, writer/reader discipline (blocked on Q4)

**Render loop** — nothing exists
- wgpu device/surface/pipeline setup; egui integration; window and event loop
- Azimuthal equidistant projection in shaders; pan/zoom transform; spatial-stability
  guarantees (FR-NI-4, NFR-UX-2)
- Ten-layer compositing order; transparency below threshold; multi-sweep switching as a
  GPU state change only (FR-RP-7)

**Map underlays** — nothing exists in production (POC only in `radar-viz`)
- County / state / country / highway geometry loading and pre-projection (blocked on Q15)
- Bundled NEXRAD site list (FR-MU-3, FR-SS-1) — not blocked on anything
- Tile subsystem and LRU disk cache (blocked on Q16, Q7, Q5)

**Placefiles** — nothing exists
- GRLevelX parser, polling, ordering, toggling (scope blocked on Q6)

**UI and interaction** — nothing exists
- Status bar (FR-DR-7, NFR-ST-3), loading indicator, site picker, clickable site markers
- Keyboard-first control of site / product / sweep / navigation (NFR-UX-1)

**Configuration** — nothing exists
- XDG config load/save, defaults on missing-or-corrupt (FR-CP-1…3)

**Release** — nothing exists
- Packaging (Q12), minimum system requirements (Q13), reproducible-build verification
  (NFR-SEC-4), multi-instance and long-run validation (NFR-P-1, NFR-ST-4)

---

## 5. Open questions, by what they block

Six of the twelve open questions gate work that cannot sensibly start without them.

| Q | Blocks | Note |
|---|---|---|
| **Q4** — `AppState` structure | Shared state, and therefore compute *and* render | Not blocked on anything else; can be answered now |
| **Q8** — v1.0 product set | Compute layer scope | Conservative default already stated |
| **Q9** — velocity dealiasing | Compute layer, velocity product | Algorithmically the largest single unknown |
| **Q17** — texture grid dimensions | Compute layer texture generation | Half-answerable today; standard-res geometry unmeasurable from current fixtures |
| **Q11** — color table format | Compute layer color mapping, bundled defaults | GRLevelX-compatible is the strong preference |
| **Q15** — shapefile parser on the startup path | Map overlay loading | May supersede ADR-0006's parser clause |
| **Q16** — HTTP client for tiles | The **entire** tile subsystem | FR-DA-6 and FR-MU-4 are currently *unimplementable* against the accepted ADR set. Needs its own ADR. |
| Q14 | Data-source failover | Secondary; the primary path works |
| Q6 | Placefile scope | Answer before writing the parser |
| Q5, Q7 | Tile cache sharing and sizing | Answer with Q16 |
| Q12, Q13 | Distribution and minimum requirements | Answer before first release, not before |

---

## 6. Remaining work, in sequence

Ordering rationale: finish the acquisition path that already exists before starting new
layers; answer a question immediately before the work it gates, not long before; and get
one end-to-end pixel on screen as early as possible, because every performance target and
half the requirements cannot be validated until something renders.

### Stage 0 — Commit what is already done
1. Commit `.gitignore` and `.github/workflows/ci.yml`. The supply-chain gates
   (`cargo deny`, `cargo audit`) are inert until this lands — and `cargo deny` has already
   caught one live advisory pair (E-12), so this is not ceremonial.

### Stage 1 — Close the acquisition path (no new questions needed)
2. **Volume assembly state machine** (ADR-0012). This is the correct next code item: it is
   fully specified, unblocked, and it is the piece that turns the existing `Vec<Radial>`
   stream into the `VolumeScan` every layer above consumes. Includes sweep closure on
   end-of-elevation and on elevation change, permanent closure, late-data discard,
   watchdog, and the `Superseded` / `TimedOut` paths.
3. **Decode `-S` metadata messages** — at minimum Message 5 (VCP), for `VolumeContext`.
   Messages 2, 3, 15, 18 as scope allows.
4. **Poller robustness** — skipped-volume-sequence recovery (the known stall), plus FR-DA-5
   error handling and the status signal the status bar will later consume.
5. **Decoder hardening toward FR-ND-8** — corrupt-input tests and a fuzz corpus on stable
   `cargo test`, mirroring the pattern `http-ingest` already established. Broaden fixtures:
   a second site, a precipitation-mode VCP, a standard-resolution cut, a non-dual-pol cut.
   The standard-res fixture also retires half of Q17.

### Stage 2 — Make the application exist
6. **Answer Q4** (`AppState` structure and lock granularity).
7. **Runtime skeleton** — replace the `main.rs` stub: tokio runtime, task supervision,
   channel wiring from `S3Poller` → assembly → (compute stub) → `AppState`, clean shutdown.
8. **Bundled site list** (FR-MU-3, FR-SS-1) — unblocked, small, and needed by everything
   that selects a site.
9. **Configuration persistence** (FR-CP-1…3) — needed before the UI has settings worth
   remembering, and its must-not-crash-on-corrupt requirement is easier to satisfy when
   the config surface is still small.

### Stage 3 — Compute layer
10. **Answer Q8, Q9, Q11, Q17** as one batch — they jointly define what the compute layer
    produces and in what format.
11. **Color table support** — parser, bundled defaults for all in-scope products
    (FR-CT-2), user palettes from the XDG data directory (FR-CT-3).
12. **Base product textures** — reflectivity, velocity, spectrum width → pre-computed RGBA
    (FR-RP-1, FR-RP-6). `radar-viz`'s color table and PPI code is the reference.
13. **Derived products** — Echo Tops and VIL (FR-RP-2).

### Stage 4 — First pixels
14. **Render loop foundation** — window, event loop, wgpu device/surface, egui integration,
    render/present at 60 fps against a placeholder.
15. **Radar texture rendering in azimuthal equidistant projection** (FR-DR-1, FR-DR-2),
    with transparency below threshold (FR-DR-4).
16. **Pan, zoom, and spatial stability** (FR-NI-1, FR-NI-2, FR-NI-4, NFR-UX-2) — build the
    inviolability guarantee in from the start; retrofitting it is far harder.
17. **Product and sweep switching as pure GPU state changes** (FR-RP-7, FR-NI-3).
18. **Status bar and loading indicator** (FR-DR-6, FR-DR-7, NFR-ST-3) — every error path
    written in Stages 1–3 has been waiting for somewhere to surface.

*At the end of Stage 4 the application is usable for its core purpose. Everything after
this point is context around the radar image.*

### Stage 5 — Map underlays
19. **Answer Q15**, then implement vector overlay loading and pre-projection (FR-MU-1,
    FR-MU-2) and layers 3–5 and 8–9 of the compositing order.
20. **Answer Q16 and record it as an ADR**, then implement the tile subsystem and the LRU
    disk cache (FR-MU-4, FR-MU-5), answering Q5 and Q7 alongside it.

### Stage 6 — Placefiles
21. **Answer Q6**, then implement the GRLevelX parser, per-placefile polling, ordering,
    toggling, and fetch-failure tolerance (FR-PF-1…6).

### Stage 7 — Site switching and multi-instance
22. **Runtime site change** (FR-DA-4, FR-SS-2, FR-SS-3) against the < 5 s target.
23. **Multi-instance validation** (NFR-P-1) — four concurrent instances, resource scaling.

### Stage 8 — Validate the non-functional requirements
24. Measure every target in `REQUIREMENTS.md` §4.1 (60 fps, < 2 s first render, < 200 MB,
    < 128 MB GPU). These are stated as design targets to be validated, not assumed.
25. Long-run soak for memory stability (NFR-ST-4); audit for `unwrap`/`expect` on
    untrusted data (NFR-ST-2); confirm no unsanctioned network connections (BC-1, BC-2).
26. Reproducible-build verification (NFR-SEC-4).

### Stage 9 — Release
27. **Answer Q12 and Q13**, package, and document minimum system requirements.

---

## 7. Cross-cutting observations

**The documentation is ahead of the code, deliberately and usefully.** Requirements carry
explicit `[OPEN — Qn]` markers, ADRs carry dated errata rather than silent rewrites, and
both audit documents carry supersession banners. The failure mode to watch for now is the
opposite of the usual one: not stale docs, but *design drift* — implementing something
subtly different from the ADR and not amending the ADR. ADR-0014's erratum pattern is the
established remedy.

**Two accepted ADRs are under real pressure.** Q15 questions ADR-0006's parser clause and
Q16 questions whether ADR-0014's host allowlist can survive contact with ADR-0007's
arbitrary-host tile providers. Neither is a documentation problem; both need a decision
and an ADR before their subsystem starts.

**Test rigor is uneven across the boundary layers, in the wrong direction.**
`http-ingest` has a fuzz corpus gated on stable `cargo test`. `nexrad-decoder` — the other
untrusted-input parser, and the one whose failure modes are named directly in BC-6 —
rests on a single truncation test. The pattern to copy already exists in-tree.

**`utility/radar-viz` is worth more than its label suggests.** It has already exercised
color mapping, polar-to-Cartesian projection, and vector overlay drawing against real
KDOX data. It is not production code and should not become production code, but it is
where a rendering question can be answered cheaply before the GPU pipeline exists.
