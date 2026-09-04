# ADR-0025: Bundled Overlay Geometry — Build-Time Bake, No Runtime Parser

## Status
Accepted (2026-08-28)

Resolves [Q15](../open-questions.md). Supersedes the parser clause of
[ADR-0006](0006-bundle-shapefiles.md) — the sentence naming `geo`, `shapefile`, and
`lyon` as the production loading path. ADR-0006's *bundling* decision (vector data ships
with the binary; no runtime map API) is unchanged and is what this ADR implements.

**Amended 2026-09-02 by [ADR-0029](0029-primary-roads-simplification.md)** (resolving Q20):
§2's unmeasured TIGER Primary Roads row is measured and gains a bake-time Douglas–Peucker
tolerance of ε = 30 m for that layer only; §6's reserved manifest tolerance field is filled
in. This ADR is not superseded; ADR-0029 completes the one measurement it deferred.

**Amended 2026-08-30 by [ADR-0028](0028-city-labels.md)** (resolving Q19, city labels):
§1 gains a populated-places source row, §2 gains the label filter counts, §3 gains a
label index and a string table — and narrows this ADR's element-counts-only invariant to
the geometry sections — and §4 gains the label runtime path and the screen-space
declutter pass. This ADR is not superseded; ADR-0028 extends it.

## Context

ADR-0006 designated `shapefile` + `dbase` + `geo` + `lyon` for production overlay
loading. `docs/dependency-inventory.md` E-07 objected: `shapefile` 0.6 and `dbase` 0.5
are both 0.x, both single-maintainer, and `dbase` pulls `time` 0.3 — five crates on a
startup path that must not panic (Principle 2, Stability as Ethics) and inside the
government/defense approval surface (Principle 4). Q15 carried that objection forward as
the live question and proposed pre-projecting at build time instead.

### The question as posed contained an error

"Pre-project at **build** time" cannot mean what it says. Azimuthal equidistant is
centred on the *active site*, and the site is a runtime choice (FR-SS-2, Stage 7). No
site-independent bundle can ship az-eq coordinates. A build-time artifact must therefore
carry **geographic** coordinates, and a projection pass happens at runtime, per site
change.

Whether that matters was measured rather than assumed. `rustc -O -C target-cpu=native`,
the `az_eq_project` function already proven in `utility/radar-viz/src/overlay.rs`,
including the output allocation:

```
2,000,000 points projected in 57.4 ms   (~29 ns/point, single-threaded)
```

**Projection is not the cost.** Against a 2 s first-render and 5 s site-change budget it
is noise, and it runs off the render thread. The costs that actually motivated Q15 are
*parsing* and *attribute filtering* — and those are exactly the parts that do not need to
exist in the shipped binary.

### Two further facts that constrain the answer

1. **Attribute filtering needs DBF.** "Major highways" is a selection on TIGER's `MTFCC`
   / route class; city labels (layer 9) need place *names*. Doing that at runtime puts
   `dbase` → `time` on the startup path. Doing it at build time is a dozen lines of
   Python.
2. **The source shapefiles are not in the repository.** `utility/radar-viz/data/` is
   `.gitignore`d. Any answer has to say what is actually committed.

### The precedent already exists in this ADR

ADR-0006's own erratum (2026-07-31) moved the NEXRAD site list from "bundled JSON parsed
at startup" to a generated `const` Rust table produced by `utility/nexrad-sites/
generate.py`, for precisely this reason: a startup path that can fail to parse data the
project ships itself, for no benefit. **Q15 is that same decision one layer up.**

## Decision

Bake the overlay geometry at build time into one flat binary artifact compiled into the
executable. No shapefile parser, no DBF reader, and no tessellator ships.

### 1. Sources

| Layer | Source | Note |
|---|---|---|
| County boundaries | Natural Earth 10m `admin_2_counties_lakes` | 3,646 parts / 149,269 points measured. **Deliberate departure from FR-MU-1's "Census TIGER/Line" for counties.** TIGER's county file carries roughly an order of magnitude more vertices than a 230 km PPI can resolve, and the NE geometry is what `radar-viz` has already drawn against real KDOX data. FR-MU-1 is amended accordingly. |
| State / province boundaries | Natural Earth 10m `admin_1_states_provinces` | |
| Country boundaries / coastline | Natural Earth 10m `coastline` | |
| Major highways | Census TIGER/Line **Primary Roads** (`tl_<year>_us_primaryroads`) | Interstates and US routes only. Secondary roads are deferred — "major" means primary (Restraint is a Feature). |
| City labels (layer 9) | Natural Earth 10m `populated_places` | **Added 2026-08-30 by [ADR-0028](0028-city-labels.md).** Public domain and global, at ~27 KiB of bundle. **Explicitly provisional and known-sparse** — 19 labels inside a KDOX 230 km PPI, 2 inside 100 km (measured). The runtime representation is source-agnostic by construction, so a denser source is a regeneration, not a redesign. See ADR-0028 §1–§2 and §6. |

### 2. Bake-time filtering to the site footprint

The generator keeps only parts whose bounding box falls within **700 km** of any site in
`crates/radar-workstation/src/sites_generated.rs` — it reads the *committed generated
site table*, so the two artifacts cannot drift apart, and the overseas DoD sites (Kadena,
Kunsan, Osan, Lajes) are covered for free. 700 km is the 460 km maximum radar range plus
a generous pan margin.

Measured effect on the three Natural Earth layers:

| Layer | parts kept | points kept |
|---|---|---|
| `admin_1_states_provinces` | 1,438 / 8,646 | 197,952 / 1,295,319 (15.3%) |
| `admin_2_counties_lakes` | 3,646 / 3,646 | 149,269 / 149,269 (100%) |
| `coastline` | 781 / 4,133 | 98,998 / 410,957 (24.1%) |
| **total** | **5,865** | **446,219 (24.0%)** |

Measured effect on the label layer (added 2026-08-30, ADR-0028): **1,216 of 7,342**
`populated_places` records kept — ~16.6 KiB of label index plus ~10.3 KiB of strings,
~27 KiB in all. No population or `SCALERANK` floor is applied at bake time; at this
density the whole surviving set is worth keeping, and the declutter pass (§4) decides
what is actually drawn.

**~3.6 MB** of bundle for those three layers, ~7 MB of GPU buffers, and a ~13 ms
projection per site change.

**TIGER Primary Roads, measured 2026-09-02 by [ADR-0029](0029-primary-roads-simplification.md)
(resolving Q20).** The expectation stated here was right and low: **3,589,114 points across
17,500 parts, 8.0× the three Natural Earth layers combined**, at a mean vertex spacing of
87.7 m — and the 700 km filter keeps **100%** of it, because the road network and the radar
network cover the same ground. Unsimplified: 29.13 MB of bundle and 57.29 MB of GPU buffers,
45% of the 128 MB per-instance target. **A Douglas–Peucker tolerance of ε = 30 m is adopted**
for this layer only, giving 281,401 points / 2.67 MB / 4.36 MB of GPU — smaller than the
three Natural Earth layers, which stay at native density. Full measurement tables, the two
justifications for 30 m, and the decision *not* to couple it to `view::MIN_M_PER_PX` are in
ADR-0029.

| Layer | parts | points | bundle |
|---|---|---|---|
| three Natural Earth layers (above) | 5,865 | 446,219 | ~3.71 MB |
| `primaryroads` @ ε = 30 m | 17,500 | 281,401 | ~2.67 MB |
| `populated_places` labels (ADR-0028) | — | 1,216 labels | ~27 KiB |
| **complete basemap** | **23,365** | **727,620** | **~6.41 MB** |

~11.46 MB of GPU buffers and a ~21 ms per-site projection for the whole set. The generator
reports per-layer counts.

### 3. Bundle format

One little-endian blob, `include_bytes!`'d into the binary. All cross-references are
**element counts, not byte offsets**, so no arithmetic can address outside an array
without failing a bounds check.

```
header      magic [u8;8] = "RWMOVL01" | version u32 | layer_count u32
            | part_count u32 | point_count u32
            | label_count u32 | string_bytes u32
layer table layer_count × { kind u32, first_part u32, part_count u32 }
part index  part_count  × { first_point u32, point_count u32,
                            min_lon i32, min_lat i32, max_lon i32, max_lat i32 }
points      point_count × { lon i32, lat i32 }

label index label_count × { lon i32, lat i32, rank u16,
                            name_off u32, name_len u16 }
strings     one contiguous UTF-8 blob
```

The last two sections were added 2026-08-30 by [ADR-0028](0028-city-labels.md), which
resolves Q19. `magic` stays `RWMOVL01` and `version` stays **1**: this ADR was accepted
but never implemented, so labels are designed into the first version of the format rather
than migrated into a second one. A layer `kind` discriminant identifies a labelled-point
layer. The header's former `reserved u32` becomes `label_count`, and `string_bytes` is
appended — the reserved word existed for exactly this kind of extension, and spending it
before anything is baked costs nothing. `rank` is a dense `u16`, ascending, `0` = draw
first, normalised at bake time from whatever importance signal the source carries — the
runtime never interprets it beyond ordering, which is what keeps the source swappable.

**The element-counts-only invariant below is narrowed by that extension, deliberately.**
It continues to hold for the layer table, part index, and points. The string table
necessarily uses byte offsets — there is no representation that avoids it — but the
property that matters is unchanged, because `slice::get(off .. off + len)` is still a
checked range that yields `None` rather than panicking (§4). Names are stored as UTF-8;
49 of the 1,216 kept records are non-ASCII. A fixed-width name field would have preserved
the invariant literally and was rejected: it fits Natural Earth's 25-byte maximum and
truncates both denser candidate sources, encoding the provisional source into the format.

Coordinates are `i32` in units of 1e-7 degrees — 1.1 cm resolution, and ±180° fits `i32`
exactly (1.8e9 < 2.147e9). Integers, so there is no float parsing and no rounding
question.

Read with `u32::from_le_bytes` / `i32::from_le_bytes` over 4-byte subslices: **no
`unsafe`, no alignment assumption, no zero-copy cast.** The per-point decode cost
disappears into the projection pass that has to touch every point anyway.

Part bounding boxes are stored but **not used for culling in v1.0** — at 13 ms the whole
set is projected at site load, which avoids an "overlays vanish when panned far" class of
bug entirely. They are there for LOD and draw-range selection later.

### 4. Runtime path (`crates/radar-workstation/src/overlay/`)

- Accessors over `&'static [u8]` returning layers → parts → points. Every access is
  `slice::get`, never indexing; an inconsistent bundle yields an empty layer, not a
  panic. There is no untrusted input here — these are bytes the project generated and
  validated — so this is belt-and-braces, not a parser.
- Projection at site load, on `spawn_blocking`, using the same az-eq function the radar
  path and `radar-viz` use (DRY — one projection implementation).
- Upload as a `LineList` vertex buffer plus a runtime-derived index buffer (two indices
  per segment, trivially generated from the part index — the indices are *not* baked into
  the bundle, since they are exactly derivable from it). Drawn through the same view
  uniform `render/reference.rs` already uses for range rings. The CPU-side projected copy
  is dropped after upload.
- **Labels (added 2026-08-30, ADR-0028).** Label points are projected in the same site-load
  pass as the geometry. They are *not* uploaded as GPU geometry: a screen-space,
  rank-ordered greedy declutter pass selects which labels are drawn, and egui paints them
  in the existing egui pass at `Order::Background` — which is exactly compositing slot 9,
  since layers 1–8 are all wgpu and layer 10 is egui chrome. The pass is pure
  (`(labels, &ViewState, viewport) -> Vec<PlacedLabel>`), render-loop owned, never in
  `AppState` (ADR-0018), and unit-tested without a window like `view.rs` and `input.rs`.
  See ADR-0028 §5.
- A test in `crates/radar-workstation` walks the entire bundle — every layer, every part,
  every point — and asserts internal consistency. That test, not CI regeneration, is the
  gate: CI has neither the source shapefiles nor network access.

### 5. No tessellator

`lyon` is not adopted. Layers 3–5 are strokes, not fills. Reassess only if Stage 6
placefile polygons need filled interiors (FR-PF-3); even then, ear-clipping a warning
polygon is not a dependency.

### 6. Provenance and regeneration

`utility/map-bake/` (dev-only Python, the shape of `utility/nexrad-sites/generate.py`;
never invoked by `cargo build`, `cargo test`, or the binary — see `utility/README.md`'s
boundary).

- **The generated bundle is committed. The source shapefiles are not.** Natural Earth and
  TIGER sources total hundreds of megabytes; committing them would dominate the
  repository for data that changes on a multi-year cadence.
- `utility/README.md` records, per source: download URL, retrieval date, licence, and
  SHA-256. The generator **verifies those digests** before it emits anything, so a
  regeneration from substituted inputs fails loudly.
- A `bundle.manifest.txt` is committed beside the blob: source digests, per-layer part and
  point counts, **label count and string-table bytes** (ADR-0028), filter radius, and
  simplification tolerance — which [ADR-0029](0029-primary-roads-simplification.md) fills in
  as `simplification: douglas-peucker, epsilon 30 m, applied to primary_roads only`. A binary
  blob is not reviewable in a diff; the manifest is, and it is what a reviewer reads to see
  what a regeneration actually changed. Since ADR-0029 the bundle is **lossy**, which makes
  the manifest the only place that fact is visible.

## Consequences

- **Five crates removed from the production plan, zero added** — `shapefile`, `dbase`,
  `time`, `geo`, `lyon`. Stage 5's overlay work has no dependency delta at all.
- **A class of startup failure ceases to exist.** No file I/O, no binary parsing, no
  attribute decoding before first pixel.
- **Instances share one physical copy.** `include_bytes!` data lives in the executable's
  read-only segment, demand-paged from the same file by every process (NFR-P-1, four
  simultaneous instances). Parsed-into-`Vec` geometry would be per-process heap.
- **Binary grows by the bundle size** — ~3.6 MB measured for the three Natural Earth
  layers, plus ~27 KiB of labels (ADR-0028), plus TIGER Primary Roads, comfortably inside
  ADR-0006's stated 30–80 MB budget.
- **FR-MU-1 is amended**: counties come from Natural Earth, not TIGER; TIGER supplies
  primary roads only.
- **FR-MU-2 is amended**: geometry is *bundled* in geographic coordinates and projected
  once at site load, not at build time. "No projection per frame" is unchanged and remains
  the property that matters.
- **Data updates now require a regeneration step** and cannot be reviewed by reading the
  diff. The manifest and the verified source digests are the mitigation; this is a real
  cost, accepted.
- Runtime code for Stage 5 overlays is roughly 150 lines instead of roughly 500.

## Rejected alternatives

- **Implement ADR-0006 literally (runtime `shapefile`/`dbase`/`geo`/`lyon`).** Five 0.x
  crates on the must-not-panic startup path, parsing and DBF decoding before first pixel,
  and per-process heap for every instance — to avoid a generator script.
- **Own a minimal SHP/DBF reader in-workspace** (the `http-ingest` / `nexrad-decoder`
  move). Answers the dependency objection but not the startup one: still parses at
  startup, still needs DBF for attributes, still per-process. It would add ~500 lines and
  a fuzz corpus to own a boundary that build-time baking deletes outright. Owning a
  boundary is right when the boundary must exist; this one need not.
- **Sidecar data file, mmap'd instead of `include_bytes!`'d.** Cheaper data updates and
  a smaller binary, but it reintroduces exactly the startup I/O failure mode this ADR
  removes (missing, truncated, wrong version), and a single self-contained binary is
  friendlier to Q12 packaging and to defense-environment review. The bundle format is
  unchanged if this is ever revisited.
- **Project in the vertex shader** (upload lon/lat, transform per vertex per frame). Would
  remove even the 13 ms, but contradicts `rendering.md`'s "the GPU receives already-
  projected geometry" for no measurable gain, and puts trig in a per-frame path to save
  work that happens once per site change.
- **Per-site pre-baked az-eq bundles.** Would make build-time projection literally
  possible, at the cost of 163 copies of the geometry and a bundle that must be
  regenerated whenever the site list changes.

## Erratum (added 2026-09-02, Stage 5 / S5-c, S5-e)

Two corrections, both from implementing this ADR (`docs/plans/stage-5-map-underlays.md`):

- **§4's claim that the projection function is "the same az-eq function the radar path
  and `radar-viz` use (DRY — one projection implementation)" was wrong about the code.**
  No production az-eq implementation existed anywhere in the tree: the radar path never
  converts geographic coordinates (`compute::grid` works in polar range/azimuth,
  `render/view.rs` works in metres from the site, `shaders/radar.wgsl` inverse-maps
  screen pixels to ground range/azimuth). The only az-eq code was
  `utility/radar-viz/src/overlay.rs::az_eq_project`, dev-only per `utility/README.md`'s
  own boundary. Stage 5 adds the first production implementation,
  `compute::geometry::az_eq_project`, and deletes `radar-viz`'s copy in favour of calling
  it — which is what makes the DRY claim true, rather than assumed.
- **Projection runs synchronously at renderer init, not on `spawn_blocking`.** At Stage 5
  the active site is fixed for the process's lifetime, so there is exactly one projection
  ever, costing ~25 ms (measured, KDOX, release build — the ADR's own §4 predicted
  ~13 ms/~21 ms depending on which count it was run against; both are well under the
  ~50 ms "revisit before shipping" threshold this plan set) against a < 2 s first-render
  budget. `overlay::project(bundle: &Bundle, site: &Site) -> (Projected, Vec<Event>)` is a
  plain, pure function; Stage 7 (runtime site change) moves the *call* onto
  `spawn_blocking` without changing this function, which is the guarantee this ADR
  actually needs — nothing reachable from `App::redraw` takes a `&Bundle`.
