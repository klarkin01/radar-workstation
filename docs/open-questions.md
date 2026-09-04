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

**Q6: What is the placefile format support scope?**
GRLevelX placefile format support is planned. How complete does this implementation need
to be at v1.0? The format has a broad feature set. Define a minimum viable subset that
covers the most widely used placefiles (warnings, storm reports, METARs, lightning) and
defer less common features.

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

**Q15: How is shapefile geometry loaded for production overlays?** — Resolved
2026-08-28 (before Stage 5): it is not loaded at runtime at all. A dev-only generator
(`utility/map-bake/`) bakes the geometry at build time into one flat little-endian blob
of `i32` coordinates in units of 1e-7 degrees, which the binary `include_bytes!`s; the
runtime projects it into azimuthal equidistant coordinates once per site load. This
removes `shapefile`, `dbase`, `time`, `geo`, and `lyon` from the production graph — five
crates, zero added — and removes the startup parse step entirely, which is the same
reasoning that moved the site list to a generated `const` table in the ADR-0006 erratum.

The question as posed contained an error worth recording: "pre-project at **build**
time" is impossible, because azimuthal equidistant is centred on the active site and the
site is a runtime choice (FR-SS-2). The bundle ships *geographic* coordinates. Measuring
the projection is what settled the question — 2,000,000 points in 57.4 ms single-threaded
(~29 ns/point), and only 446,219 points survive bake-time filtering to within 700 km of a
bundled site, so a site change costs ~13 ms off-thread. Projection was never the cost;
parsing and DBF attribute filtering were, and both moved to build time.

Sources decided alongside: Natural Earth 10m for counties (a deliberate departure from
FR-MU-1's TIGER wording — TIGER county geometry carries far more vertices than a 230 km
PPI resolves), states/provinces, and coastline; TIGER/Line **Primary Roads** only for
highways. The generated bundle is committed; the source shapefiles are not, with
verified SHA-256 digests and a committed manifest standing in for diff review. `lyon` is
not adopted — layers 3–5 are strokes, not fills. Full rationale, bundle format, and
measurements in [ADR-0025](adr/0025-bundled-overlay-geometry.md). `REQUIREMENTS.md`
FR-MU-1 and FR-MU-2 lose their `[OPEN]` markers and are amended;
`dependency-inventory.md` E-07 is closed by this.

**Q16: What HTTP client serves ADR-0007's tile providers?** — Resolved 2026-08-28
(before Stage 5): neither of the two clients the question assumed. `http-ingest` splits
by layer into one HTTP/1.1 **engine** plus two sibling policy crates that depend on it —
`s3-fetch` (`S3Client`, the radar path) and `tile-fetch` (`TileClient`, the basemap
path). One framing implementation, one fuzz corpus, zero new dependencies, ~300 lines of
new code. The seam this splits on already existed inside the crate:
`http-ingest/src/lib.rs` was always a thin S3 policy layer over a generic engine.

`dependency-inventory.md` E-09 recommended a second, independent client crate. That
reached the right goal — structural isolation of the tile path — by a mechanism that
would have duplicated ~1,065 lines of `connection.rs` + `response.rs` and the 31-file
fuzz corpus, concentrating divergence risk on the workspace's most security-sensitive
code and contradicting `CLAUDE.md`'s DRY instruction. The layer split gets the same
isolation for a quarter of the code.

Three of the question's four premises did not survive measurement (2026-08-28, `curl
--http1.1` against `basemap.nationalmap.gov`, `tile.openstreetmap.org`, and
`server.arcgisonline.com`): HTTP/2 is not required (all three serve HTTP/1.1 keep-alive
with `Content-Length`), redirects were not observed on any of them (`num_redirects=0`),
and `ETag`/`If-None-Match` is nearly free because `response.rs` already frames 304 as
bodiless. The question also **omitted** the one requirement that genuinely strained
ADR-0014 — concurrency, since a viewport is 20–40 tiles and ADR-0014 chose a single
connection with no pool. That is solved by N independent `TileClient`s as N worker tasks,
which preserves "no connection pool" literally.

The invariant identified as the real one: BC-1's property is not "the host is a
compile-time constant" but *"every destination host traces to an explicit, auditable,
user-controlled decision — never to data received over the network."* A tile URL in the
config file satisfies it; a redirect does not. **Redirect following is therefore rejected
permanently, not deferred** — a provider that requires it is a reason to configure a
different provider. The radar path's guarantee gets *stronger*: `Host::parse(&str)` is
replaced by `S3Client::new(bucket: Bucket)`, a two-variant enum, so no string reaches
host selection and no `S3Client` method accepts a hostname at all.

Full rationale, the engine/policy API sketch, the sub-decision table (scheme, port,
limits, timeouts, concurrency, failure posture, no auth headers), and the rejected
alternatives in [ADR-0026](adr/0026-tile-http-boundary.md). ADR-0014 gains an erratum;
its scope-boundary list is amended, not discarded. `REQUIREMENTS.md` FR-DA-6 and FR-MU-4
lose their `[OPEN]` markers; `dependency-inventory.md` E-09 is closed by this. ADR-0026
raises **Q18** (tile image decoding) in the process — the transport question is settled,
the codec question is not.

**Q18: What decodes tile image bodies?** — Resolved 2026-08-28 (before Stage 5), and it
resolved the scope rather than the codec: **the tile subsystem is deferred to post-v1.0**
and v1.0 ships a vector-only basemap. Recorded in
[ADR-0027](adr/0027-tile-image-decoding.md).

The question's first candidate answer — own a minimal PNG decoder and restrict v1.0 to
PNG providers — turned out not to exist as an option. Four of the five USGS National Map
services serve **both** JPEG and PNG from a single URL template, interleaved by tile and
not only for blank tiles, so no configuration of the ADR-0007 default yields PNG only and
format cannot be pinned anywhere but per response.

The measurement that decided the ownership question cut the opposite way from how it
first looked. All 56 JPEGs sampled are one profile — SOF0 baseline, 8-bit, 3 components,
4:4:4, no restart markers, single scan, no progressive anywhere — which makes an owned
decoder only ~600 lines. But that profile is the setting of a cache built years ago by an
agency that never promised it. ADR-0008 can own the NEXRAD decoder because ICD 2620002
*is* a contract; a provider's encoder settings are a private implementation detail, and a
decoder scoped to them takes a silent dependency on someone else's unstated
configuration. **Specified-and-stable versus observed-and-unpromised** is the asymmetry
that keeps ADR-0008's reasoning from extending to tile codecs, and it is the first thing
to re-read if this is revisited.

Neither cost nor containment turned out to be the hard part. Decoding is 0.32 ms per tile
(mean, 73 tiles), so ~13 ms for a 40-tile viewport, off-thread. The decompression-bomb
bound is trivial because a tile has exactly one legal size: gating on declared dimensions
at the header rejects a 545-byte PNG claiming 30000×30000 (3.4 GB decoded) before any
allocation.

So the deferral is not a dodge of a hard problem — ADR-0027 §2 records the complete
answer (`png` 0.18 + `jpeg-decoder` 0.3, magic-byte dispatch, dimension gate,
`catch_unwind` on `spawn_blocking`, no `zlib-rs`) for when the subsystem is built. It is a
scope decision: the tile layer is the one v1.0 item whose cost is a new untrusted-input
parser on a network path and whose benefit is an optional, off-by-default raster layer
beneath a vector reference map that ADR-0025 already made complete. `REQUIREMENTS.md` §6
moves map imagery and the tile cache to Explicitly Deferred; FR-DA-6, FR-MU-4, FR-MU-5,
and FR-MU-6 are marked deferred. ADR-0007 and ADR-0026 stand as written, unimplemented;
**no stub is written** (ADR-0027 §3), and the corpus behind these measurements is
kept at `crates/radar-workstation/tests/fixtures/tiles/`.

**Q19: What is the source and bundle representation for city labels (layer 9)?** —
Resolved 2026-08-30 (before Stage 5). Recorded in
[ADR-0028](adr/0028-city-labels.md). The question asked four things together; **three of
the four collapsed under measurement, and the fourth — the source — is the one whose
presumed answer the numbers contradict.**

Natural Earth 10m `populated_places` was named as the obvious candidate, "consistent with
ADR-0025's other three Natural Earth layers." Measured against the committed 163-site
table, it yields **19 labels inside a KDOX 230 km PPI and 2 inside 100 km** (KTLX 16/4,
KLOT 28/8, KGLD 7/1). It is a small-scale *world* cartography set, and the consistency
argument is precisely what makes the wrong answer look obvious. The two denser
alternatives were measured too: Census Gazetteer places (32,329 records; KDOX 1,991/312)
is public domain but US-only — it blanks LPLA, PGUA, RKJK, RKSG and RODN entirely, strips
28–56% of labels from border sites (KATX 56%, KBUF 54%, KCXX 45%), and has no honest
ranking field, since joining `sub-est2024` reaches 60.2% of rows and **0.0% of CDPs**.
GeoNames `cities1000` (170,860 records; KDOX 1,742/148) is the best data and global, but
CC BY 4.0 — the only non-public-domain data that would enter the binary, inside the
approval surface Principle 4 protects.

**Natural Earth is chosen for v1.0 and recorded as explicitly provisional**, because the
plumbing is worth more than the source right now: layer 9 has never existed in any form,
and a working bake → bundle → project → select → draw path turns a denser source into a
regeneration rather than a project. The mechanism that guarantees that is **rank
normalisation at bake time** — the runtime sees exactly `{ lon, lat, rank, name }` and
nothing source-specific, so swapping sources changes the generator and the bundle, not
the format or the runtime.

The other three sub-questions:

- **The format extension is not a version bump.** ADR-0025 is accepted but
  *unimplemented* — no `utility/map-bake/`, no `overlay/` module, no blob in the tree — so
  labels are designed into version 1. A label index and a UTF-8 string table are added,
  spending the header's `reserved u32`. This narrows ADR-0025 §3's element-counts-only
  invariant to the geometry sections; a string table needs byte offsets, and the property
  that matters (a checked `slice::get` range, never a panic) is preserved. A fixed-width
  name field would have kept the invariant literally and was rejected because it fits
  Natural Earth's 25-byte maximum and truncates Census (57) and GeoNames (97) — encoding
  the provisional source into the format.
- **The renderer question had no architectural weight.** Compositing layers **1–8 are all
  wgpu** and layer 10 is egui, so egui's lowest order *is* slot 9 — no ordering violation
  and no second egui pass. `render/ui.rs::ring_labels` already draws world-projected text
  this way. Measured at the pinned egui 0.36.1: 500 labels cost **0.108 ms** and panning
  costs the same as static, because the galley cache keys on text, not position.
- **The zoom-threshold policy is subsumed** by a screen-space, rank-ordered greedy
  declutter pass, which is needed regardless: it self-limits output to ~250–360 labels
  independent of source density (measured at KDOX against the dense source: 2,997
  candidates → 254 placed at 230 km). The pass is pure, render-loop owned, never in
  `AppState` (ADR-0018), and covered by FR-NI-4's spatial-stability test.

Two costs accepted and recorded rather than discovered later: the v1.0 basemap names
major population centres, not every settlement (so two labels inside 100 km is the
design, not a bug); and at this density the declutter pass will essentially never reject
a candidate, so it **must be unit-tested with synthetic dense input** or it compiles
without ever being exercised.

`REQUIREMENTS.md` gains **FR-MU-7** — before this, city labels appeared only in FR-DR-3's
compositing list and had no functional requirement at all, so the requirement set and the
compositing order disagreed. ADR-0025 is amended (§1 source row, §2 filter counts, §3
format, §4 runtime path), not superseded.

**Q7: How is the disk tile cache managed?** — Closed 2026-08-28 by deferral: there is no
tile cache in v1.0 ([ADR-0027](adr/0027-tile-image-decoding.md)). Cache location, maximum
size, and eviction policy return unanswered with the tile subsystem, and are to be decided
with it rather than in advance. FR-MU-5 is marked deferred.

**Q5: How are multiple instances coordinated, if at all?** — Closed 2026-08-28. The
answer for v1.0 is **not at all**, which BC-4 already required ("running instances must
never communicate with each other"); this question existed because a shared on-disk tile
cache was the one plausible exception, and with tiles deferred
([ADR-0027](adr/0027-tile-image-decoding.md)) there is no candidate shared resource left.
Every other resource is already per-instance by construction: config and palettes are
read-only, and the overlay bundle is shared as read-only pages of the executable, not as
coordinated state (ADR-0025). The shared-cache case returns with the tile subsystem.
FR-MU-6 is marked deferred.

**Q20: Does TIGER Primary Roads need a bake-time simplification tolerance?** — Resolved
2026-09-02 (Stage 5). **Yes: Douglas–Peucker at ε = 30 m**, recorded in
[ADR-0029](adr/0029-primary-roads-simplification.md), which amends ADR-0025 §2 (the layer
was unmeasured there) and §6 (the manifest field the tolerance fills).

The measurement is the answer. TIGER Primary Roads is **3,589,114 points across 17,500
parts — 8.0× the three Natural Earth layers combined** — at a mean vertex spacing of
87.7 m over 313,078 km of road, and the 700 km site-footprint filter keeps **100%** of it,
because the road network and the radar network cover the same ground. Unsimplified that is
29.13 MB of bundle and **57.29 MB of GPU buffers — 45% of the 128 MB per-instance target
before a single radar texture**. At ε = 30 m it is 281,401 points, 2.67 MB, and 4.36 MB of
GPU: *smaller than the three layers it joins*, bringing the whole basemap to 6.41 MB of
bundle, 11.46 MB of GPU, a 21.1 ms per-site projection, and a ~24 MB binary.

Two corrections to this question as it was originally posed, both recorded in ADR-0029
because they outlast the number: (1) "justified against what a 230 km PPI resolves" is the
wrong yardstick — `view::MIN_M_PER_PX = 60.0` lets the user zoom 7× past the default view,
and 30 m is half a pixel there; (2) the more durable bound is that **the map is context for
a 250 m radar gate**, so 30 m is already 8.3× finer than the data it sits under. Projection
cost, which ADR-0025 implied would be the problem, is **not** a constraint at any tolerance:
117 ms unsimplified against a 5 s site-change budget.

`MIN_M_PER_PX` is deliberately **not** made load-bearing — ADR-0029 §2. The tolerance is a
calibration, not a contract; if that constant is ever lowered and the roads look faceted,
the repair is a bundle regeneration with a smaller ε, not a design change.

Two things recorded rather than left to be discovered: **13 of 163 sites have no
primary-road geometry within 230 km** (the five overseas DoD sites, roadless interior
Alaska, outer Hawaii) — TIGER is a US product with no public-domain global counterpart, and
layer 5 is toggleable, so this degrades rather than breaks; and dropping sub-kilometre
ramp/connector parts is worthless for bytes (0.7% of points) but may be worth doing for
**visual clutter**, which is deliberately deferred until layer 5 has been drawn and looked
at rather than raised as a new numbered question.
