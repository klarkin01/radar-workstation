# Open Questions

*Unresolved design questions that need answers before or during implementation of the
relevant subsystem. When a question is resolved, move it to the "Resolved" section at
the end of this document — with the decision and where it landed — rather than deleting
it. Record the decision in an ADR as well if it is architecturally significant.*

---

## Critical — Must Resolve Before Implementation

None outstanding.

---

## Architecture — Resolve Before the Relevant Subsystem

**Q5: How are multiple instances coordinated, if at all?**
Each instance is designed to be fully independent. Is there any case where instances
should share resources — for example, a shared on-disk tile cache to avoid redundant
downloads when multiple instances are monitoring sites in the same geographic area?
If yes, this requires a cache access coordination mechanism. If no, document explicitly.

**Q6: What is the placefile format support scope?**
GRLevelX placefile format support is planned. How complete does this implementation need
to be at v1.0? The format has a broad feature set. Define a minimum viable subset that
covers the most widely used placefiles (warnings, storm reports, METARs, lightning) and
defer less common features.

**Q7: How is the disk tile cache managed?**
Tile caching requires a cache eviction policy, a maximum size, and a directory location.
Define: default cache location (XDG cache dir convention on Linux), maximum cache size
(configurable?), eviction policy (LRU by last access time is standard), and whether
cache is shared across instances or per-instance.

**Q15: How is shapefile geometry loaded for production overlays?**
ADR-0006 designates `shapefile`, `geo`, and `lyon` for production overlay loading. That
puts a 0.x single-maintainer binary parser (plus `dbase` 0.5 and `time` 0.3) on a startup
path that must not panic, per Principle 2 (Stability as Ethics). The alternative: since
ADR-0006 already pre-projects overlays at load time, pre-project them at **build** time
into a flat bundled format the app `mmap`s — removing `shapefile`, `dbase`, `time`, and
`geo` from the shipped binary, eliminating a class of startup panic, and helping the
< 2 s first-render target. Resolving this means either accepting those dependencies under
a recorded rationale or superseding ADR-0006's parser clause. **Blocks:** overlay loading
implementation. Analysis: `docs/dependency-inventory.md` E-07.

**Q16: What HTTP client serves ADR-0007's tile providers?**
ADR-0007 requires a user-supplied URL template against an **arbitrary** host, which in
practice means redirect following, `ETag` / `If-None-Match` for the disk cache, and
possibly HTTP/2. ADR-0014 lists all of those as explicit non-goals of `http-ingest` and
says a need like this is "a signal to reopen this ADR, not to grow the crate." Three
options, assessed in `docs/dependency-inventory.md`: generalize `http-ingest`; add a
second, separate client crate scoped as best-effort and structurally unable to affect the
radar path; or reintroduce a third-party client for tiles only. The inventory recommends
the second option and rates the third worst. The answer determines whether
`http-ingest`'s compile-time host allowlist is a permanent asset or a temporary one, so it
must be settled **before** any tile code is written, and recorded in its own ADR.
**Blocks:** the entire tile subsystem. Analysis: `docs/dependency-inventory.md` E-09.

---

## Data and Products — Resolve During Decoder Implementation

**Q14: Backup data source?**
ADR-0011 partially resolves this: assembled volume files (`unidata-nexrad-level2`) are
the designated secondary source if the real-time chunk stream is unavailable. What
remains open is the failover mechanics — detecting chunk-stream unavailability,
switching over, and recovering back to the chunk stream — and whether a further
fallback beyond Unidata's AWS infrastructure (e.g., Iowa State Mesonet) is warranted.

---

## Rendering — Resolve During Rendering Subsystem Design

None outstanding at this stage. Q11 and Q17 (below) were the two in this category;
both resolved in Stage 3.

---

## Distribution — Resolve Before First Public Release

**Q12: What is the Linux distribution strategy?**
Options: AppImage (broadest compatibility, self-contained), Flatpak (sandboxed, good
for desktop Linux users), native packages (deb/rpm — higher maintenance burden),
or direct binary download. AppImage is the lowest-friction starting point. Flatpak
has security sandbox implications worth evaluating given the government use case.

**Q13: What are the minimum system requirements?**
Define minimum: Linux kernel version, GPU requirements (OpenGL version for the wgpu
GL backend fallback), RAM, and CPU. Users on older hardware or headless servers with
software rendering are explicitly out of scope — document this clearly.

---

## Resolved

Recovered from `git log -p docs/open-questions.md`, added 2026-07-30. Q1–Q3 and Q10
were deleted outright rather than moved when they were originally closed — this section
exists so that doesn't happen again (see the preamble). The numbering gaps this leaves
(Q1–Q3, Q10) are expected; do not reassign them.

**Q1: What is the project name?** — Resolved circa 2026-04-29. The project is named
"Radar Workstation, Meteorological." No ADR: naming is not architecturally significant.
Removed from this document 2026-06-26 (commit `f0f8bba`).

**Q2: What license specifically?** — Resolved: Apache License, Version 2.0. Recorded in
[ADR-0009](adr/0009-open-source.md). Removed from this document 2026-06-26 (commit
`f0f8bba`).

**Q3: What is the NEXRAD data source / polling endpoint?** — Resolved: the real-time
chunk stream (`unidata-nexrad-level2-chunks`) is the primary source; assembled volume
files (`unidata-nexrad-level2`) are the secondary source. Recorded in
[ADR-0011](adr/0011-chunk-stream-data-source.md). Removed from this document 2026-06-26
(commit `f0f8bba`) — three days *before* ADR-0011 itself was added (`eef06f8`,
2026-06-29). The question was deleted on the strength of an answer that had not yet been
recorded anywhere; ADR-0011 is retroactive documentation of a decision already made in
this document's removal, which is exactly the failure mode the preamble's new "move, don't
delete" instruction is meant to prevent.

**Q10: What projection is used for the display?** — Resolved: azimuthal equidistant
projection, centered on the active radar site. Documented in
[rendering.md](architecture/rendering.md). Removed from this document 2026-07-28
(commit `a3c323c`).

**Q4: Exact shared state structure?** — Resolved 2026-07-31 (Stage 2, S2-W1):
`Arc<AppState>` with an interior `RwLock<RadarState>` scoped to radar data only — not
the outer `Arc<RwLock<AppState>>` this document originally specified. View state
(pan/zoom/active product/window geometry) is owned outright by the render loop and
never enters `AppState`; ingest health is read through the `watch::Receiver
<IngestStatus>` `S3Poller::status()` already publishes, not copied into a second
structure. `AppState::snapshot()` is the only read API, returning owned data so holding
a lock guard across a frame is impossible by construction. Full rationale, retention
policy, and alternatives considered in
[ADR-0018](adr/0018-shared-application-state.md). `overview.md`, `data-flow.md`, and
`CLAUDE.md` corrected in the same change.

**Q8: Which derived products are in scope for v1.0?** — Resolved 2026-08-05 (Stage 3,
S3-b): reflectivity, velocity, spectrum width, Echo Tops, VIL, plus ZDR and CC
(dual-pol). KDP, PHI, CFP, and storm-relative velocity remain deferred — KDP needs a
real filtering algorithm (differentiating PHI over range), PHI/CFP are diagnostic
quantities of low value to a general operator, and storm-relative velocity needs a
storm-motion input mechanism that does not exist before Stage 4's UI. The revision from
`REQUIREMENTS.md`'s original conservative default (deferring all dual-pol) is because
the cost calculus changed under ADR-0020's R8+LUT representation: ZDR and CC cost one
palette and a few megabytes each once gridding is generic across moments, not a new
algorithm or new decoder work (FR-ND-4 already decoded them). `REQUIREMENTS.md` FR-RP-3
loses its `[OPEN]` marker; FR-RP-4 (storm-relative velocity) stays deferred but is no
longer blocked on this question specifically.

**Q9: Velocity dealiasing — implement or defer?** — Resolved 2026-08-05 (Stage 3,
S3-e): deferred, with both fold conditions made legible instead. Range folding (ICD raw
value 1) gets its own palette entry (`RF:`) so it renders visually distinct from
no-echo. Velocity aliasing is bounded by the sweep's Nyquist velocity
(`Sweep::nyquist_velocity_mps`), carried onto `SweepGrid::nyquist_velocity_mps` so a
future status bar/legend can state the fold limit. A dealiasing algorithm that unfolds
wrongly during a warning shows a couplet that is not there; a visible fold with the
Nyquist stated is read correctly by the operator this application is built for.
Documented as a known limitation, not silently absent. `REQUIREMENTS.md` FR-RP-5 loses
its `[OPEN]` marker (deferred, not implemented).

**Q11: How is color table / palette support handled?** — Resolved 2026-08-05 (Stage 3,
S3-c): a documented subset of the GRLevelX `.pal` format
(`compute::palette::parse`), bundled defaults compiled in with `include_str!`, user
overrides from `paths::data_dir()/palettes/<product>.pal`. Unknown directives are
skipped and reported, never fatal. Full directive table, the fuzz corpus, and a
recorded gap (the directive table was not cross-checked against real, currently
circulating community `.pal` files — no network access in the implementing session) in
[ADR-0021](adr/0021-colour-table-format.md). `REQUIREMENTS.md` FR-CT-1 loses its
`[OPEN]` marker.

**Q17 (narrowed 2026-07-31, S1-W4d; resolved 2026-08-05, Stage 3 S3-d):** neither one
shared texture format nor two per-resolution formats — **per-sweep native dimensions**,
carried as grid metadata (`SweepGrid::{azimuth_count, gate_count, first_gate_m,
gate_width_m}`) the shader takes as uniforms. Once the representation is R8 + a 256-entry
LUT (ADR-0020, closed in the same stage), the premise that motivated "one format vs.
two" no longer applies: the range dimension varies far more across measured tilts (688
to 1832 gates) than azimuth resolution does (360 vs 720), so no fixed format is
efficient regardless of the resolution question, and a texture array (the one
representation actually requiring uniform dimensions) is unnecessary since only one
(product, sweep) pair is drawn at a time. Full rationale in
[ADR-0020](adr/0020-product-texture-representation.md).
