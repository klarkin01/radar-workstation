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

**Q4: Exact shared state structure?**
`Arc<RwLock<AppState>>` is the chosen pattern. The structure of `AppState` needs to be
defined: what it holds, how it is partitioned, and whether a single lock or multiple
finer-grained locks better serves the read/write patterns of the render loop vs. the
data pipeline.

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

**Q8: Which derived products are in scope for v1.0?**
GR2Analyst derives Echo Tops, VIL, VILD, POSH, and MEHS from Level II reflectivity.
Define the v1.0 product set. A conservative starting point: base reflectivity, base
velocity, storm-relative velocity, spectrum width (all sweeps), plus Echo Tops and VIL
as derived products. Dual-pol products (ZDR, CC, KDP) are high value but add decoder
and rendering complexity.

**Q9: Velocity dealiasing — implement or defer?**
Velocity aliasing is a known limitation of raw Doppler data that significantly affects
usability of the velocity product. GR2Analyst implements dealiasing. This is
algorithmically non-trivial. Decide whether v1.0 ships with dealiasing, ships with
a known limitation notice, or ships with a simple range-folding indicator only.

**Q14: Backup data source?**
ADR-0011 partially resolves this: assembled volume files (`unidata-nexrad-level2`) are
the designated secondary source if the real-time chunk stream is unavailable. What
remains open is the failover mechanics — detecting chunk-stream unavailability,
switching over, and recovering back to the chunk stream — and whether a further
fallback beyond Unidata's AWS infrastructure (e.g., Iowa State Mesonet) is warranted.

---

## Rendering — Resolve During Rendering Subsystem Design

**Q11: How is color table / palette support handled?**
GR2Analyst supports user-supplied color tables in a documented format, and a large
community ecosystem of custom palettes exists. Supporting GRLevelX-compatible color
table format would give immediate access to this ecosystem. Define: which palette format
to support, where user palettes are stored, and how defaults are shipped with the
application.

**Q17: What are the radar texture's grid dimensions, for standard-res and super-res
alike?** `docs/architecture/rendering.md`'s Polar Grid Representation section records
measured super-resolution geometry (0.25 km gates, ~0.5° azimuthal spacing, maximum
range varying by moment and tilt from 174 km to 460 km) from the KDOX VCP 35 fixtures in
`crates/nexrad-decoder/tests/fixtures/` — but all five fixtures are super-resolution
only, so standard-resolution cut geometry is not yet confirmed against sample data
(overlaps DOC-09's fixture-coverage gap, see decoder `TESTING.md`). FR-ND-3 requires the
decoder to support both variants. Open: whether standard-res and super-res sweeps share
one texture format or two, and what the shared/differing dimensions are. **Blocks:** the
compute layer's texture generation. Related: Q8 (v1.0 product set — dual-pol moments
have their own, narrower range at some tilts, per the measured table).

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
