# ADR-0029: TIGER Primary Roads — Bake-Time Simplification at 30 m

## Status
Accepted (2026-09-02)

Resolves [Q20](../open-questions.md). Amends [ADR-0025](0025-bundled-overlay-geometry.md)
§2 (which recorded this layer as unmeasured) and §6 (which reserves a manifest field for
the tolerance this ADR sets). Supersedes nothing.

## Context

ADR-0025 baked four overlay layers into one `include_bytes!` bundle and measured three of
them — 5,865 parts / 446,219 points / ~3.6 MB of bundle / ~7 MB of GPU buffers / ~13 ms of
per-site projection. The fourth, Census TIGER/Line Primary Roads, was **not in the tree**
and was recorded as unmeasured, "expected to be the largest single layer and the one most
likely to want a bake-time simplification tolerance." [ADR-0027](0027-tile-image-decoding.md)
raised that gap as a formal question when it deferred the tile subsystem and made the
vector basemap the whole basemap.

Q20 is a measurement before it is a decision. The source was downloaded and measured.

### The framing had to be corrected first

Q20 and ADR-0025 both say a tolerance must be "justified against what a 230 km PPI
actually resolves." That yardstick is wrong twice over, and both corrections matter more
than the number they produce.

1. **The application is not a fixed-scale PPI.** `view::MIN_M_PER_PX = 60.0` lets the user
   zoom to 60 m/px; the default 230 km view is ~426 m/px at 1080p. A tolerance justified
   against the default view is 7× too coarse for a zoom the application already permits.
2. **The map is context for the radar, and the radar resolves 250 m.** Map geometry far
   finer than the data it sits under has diminishing value regardless of what the display
   can address. This is the more durable of the two bounds, because it does not move when
   a rendering constant does.

### Provenance of the measured source

`tl_2025_us_primaryroads` (TIGER2025), retrieved 2026-09-02 from
`https://www2.census.gov/geo/tiger/TIGER2025/PRIMARYROADS/tl_2025_us_primaryroads.zip`.
U.S. federal government data, public domain. 38,379,400-byte archive; 58.4 MB `.shp`.

```
zip  400453e97b9e6693dfecb7362ce7a6cf260d27050f7d84d2a024ba0710b94c07
shp  0a71f09e16325e815961e5486b71e825c1da31e9d80fd58fe5b5da0c01ed313b
dbf  4b9e2a05d259c73ced83eb6769db225b717945b52509444635869dfdb29dfce6
```

The shapefile lives in the `.gitignore`d `utility/radar-viz/data/`, per ADR-0025 §6: the
generated bundle is committed, the sources are not. These digests are what
`utility/map-bake/` will verify before it emits anything.

## The measurements

Read with stdlib-only SHP and DBF readers — no third-party parser was installed to answer
a question about not shipping one. Cost model is ADR-0025's own format: part index 24 B,
point 8 B, GPU vertex `2×f32`, GPU index `2×u32` per segment, projection at the 29 ns/point
ADR-0025 measured.

**Measurement 1 — the raw layer.**

| | value |
|---|---|
| Records / parts (≥2 points) | 17,500 / 17,500 |
| Points | **3,589,114** |
| Points per part | mean 205, median 120, p90 503, max 2,826 |
| Total road length | 313,078 km |
| **Mean vertex spacing** | **87.7 m** |
| MTFCC | `S1100` × 17,500 (uniform) |

**8.0× the three Natural Earth layers combined.** ADR-0025's expectation was right, and
low.

**Measurement 2 — the 700 km site-footprint filter does nothing here.**

**17,500 / 17,500 parts and 100.0% of points kept.** The same outcome as counties, for the
same reason: the primary-road network and the radar network cover the same ground. The
filter that cut coastline to 24.1% and states to 15.3% has no headroom on this layer, so
the raw figure is the shipped figure. This is a fact about the layer, not a defect in the
filter — it stays, because it is what excludes nothing at 700 km and everything beyond it.

**Measurement 3 — cost, unsimplified.**

| | roads alone | with the other three layers + labels |
|---|---|---|
| Bundle | **29.13 MB** | 32.87 MB |
| GPU buffers | **57.29 MB** | **64.38 MB** |
| Projection | 104.1 ms | 117.0 ms |
| Resulting binary | — | ~50.5 MB |

Only two of those three are constraints:

- **GPU memory is the binding one.** 64 MB of line geometry against the 128 MB
  per-instance target in `rendering.md` — half the budget before a single radar texture,
  and NFR-P-1 expects four instances running at once.
- **Binary size clears ADR-0006's 30–80 MB band** at ~50.5 MB, but map data would be 65%
  of the executable.
- **Projection is not a constraint.** 117 ms against a 5 s site-change budget is 2.3%, on
  a blocking task, off the render thread. ADR-0025's guess that this layer would be
  "expensive" is right about bytes and wrong about milliseconds; recorded so the number is
  not cited later as though it mattered.

**Measurement 4 — Douglas–Peucker sweep.** Local equirectangular metres, endpoints always
preserved.

| ε | points | % kept | bundle MB | GPU MB | proj ms | max deviation @ 60 m/px |
|---|---|---|---|---|---|---|
| none | 3,589,114 | 100.0% | 29.13 | 57.29 | 104.1 | 0 |
| **0.5 m** | 2,123,928 | **59.2%** | 17.41 | 33.84 | 61.6 | 0.008 px |
| 5 m | 676,919 | 18.9% | 5.84 | 10.69 | 19.6 | 0.08 px |
| 15 m | 392,162 | 10.9% | 3.56 | 6.13 | 11.4 | 0.25 px |
| **30 m** | **281,401** | **7.8%** | **2.67** | **4.36** | **8.2** | **0.50 px** |
| 60 m | 202,964 | 5.7% | 2.04 | 3.11 | 5.9 | 1.00 px |
| 100 m | 159,928 | 4.5% | 1.70 | 2.42 | 4.6 | 1.67 px |
| 200 m | 115,374 | 3.2% | 1.34 | 1.71 | 3.3 | 3.33 px |

**A half-metre tolerance deletes 41% of the vertices.** That single row is the finding:
TIGER carries sub-metre positional detail, and this is a mismatch between source fidelity
and display purpose, not a rendering trade-off. Deviation in pixels is exact, not
sampled — Douglas–Peucker bounds perpendicular error by ε, so the column is ε divided by
metres-per-pixel.

**Measurement 5 — visual check at maximum zoom.** KRLX (Charleston WV — I-64/I-77/I-79
through the Alleghenies, about the most curvature primary roads have anywhere in the
country), rendered at 60 m/px, 1200×800, unsimplified against each tolerance:

- **ε = 30 m — indistinguishable from unsimplified.** No curve reads as faceted.
- **ε = 100 m — visible faceting**, long chords across the mountain curves.
- **ε = 200 m — obviously polygonal.**

Flat-country interstates show nothing at any of these tolerances; KRLX was chosen
adversarially.

**Measurement 6 — attribute structure.** All 17,500 records are `MTFCC = S1100`.

| RTTYP | parts | points | % of points | length km |
|---|---|---|---|---|
| I (Interstate) | 5,618 | 1,702,373 | 47.4% | 158,238 |
| M (common name) | 4,939 | 750,803 | 20.9% | 56,917 |
| U (US route) | 3,781 | 672,216 | 18.7% | 59,819 |
| S (State route) | 3,130 | 461,035 | 12.8% | 37,912 |
| O, C | 32 | 2,687 | 0.1% | 191 |

Short parts — the ramps and connectors — are 15.0% of parts but **0.7% of points** (under
1 km); 22.3% of parts and 1.6% of points (under 2 km).

**Measurement 7 — coverage.** 13 of the 163 sites in `sites_generated.rs` have **no
primary-road geometry within 230 km**: `LPLA PABC PACG PAEC PAIH PAKC PAPD PGUA PHKM PHWA
RKJK RKSG RODN` — the five overseas DoD sites plus roadless interior Alaska and the outer
Hawaiian sites. TIGER is a United States product and has no global counterpart in the
public domain.

## Decision

### 1. Bake TIGER Primary Roads with a Douglas–Peucker tolerance of ε = 30 m

281,401 points, 17,500 parts, 2.67 MB of bundle, 4.36 MB of GPU buffers, 8.2 ms of
projection. **The national road layer becomes smaller than the three Natural Earth layers
it joins** (281k points against 446k), and the complete basemap costs 6.41 MB of bundle,
11.46 MB of GPU (9% of the 128 MB target), a 21.1 ms per-site projection, and a ~24 MB
binary.

The tolerance has two independent justifications, and it is deliberate that neither is a
number chosen by eye — that is the failure mode ADR-0025 avoided when it moved counties
off TIGER, and the one ADR-0028 rejected a ranking proxy to avoid:

- **30 m is 8.3× finer than the 250 m gate the map is context for.** The map exists so the
  operator can place an echo against the ground; resolving the ground an order of
  magnitude finer than the radar resolves the storm buys nothing.
- **30 m is half a pixel at `view::MIN_M_PER_PX`** — 0.07 px at the 230 km default view,
  0.01 px fully zoomed out. Below the width of the line that draws it, at every zoom the
  application permits.

### 2. `MIN_M_PER_PX` is explicitly **not** made load-bearing

The tolerance is a **calibration, not a contract.** Nothing in the code, the bundle, or
the manifest asserts a relationship between `view::MIN_M_PER_PX` and ε, and no test
enforces one.

This is a deliberate choice against the tighter coupling, and the reasoning is recorded
because the tighter coupling is the more obvious move. Tying them means a future change to
a rendering constant silently invalidates a committed data artifact — a long-range,
non-local failure whose symptom (roads look slightly faceted when zoomed all the way in)
is far milder than the machinery needed to prevent it. If `MIN_M_PER_PX` is ever lowered
and the roads look wrong, the repair is to re-run the generator with a smaller ε. That is
a bundle regeneration, which is a cost this design already pays for data updates.

The 250 m gate justification in §1 is the durable one precisely because it does not move
when a rendering constant does.

### 3. Attribute narrowing is rejected; ramp pruning is deferred to sight of the drawn layer

Filtering by `RTTYP` was considered as an alternative to simplification and is rejected on
Measurement 6: dropping everything but Interstates halves the points and costs every US
route on the display, where ε = 30 m removes 92% of them at no visible cost. Simplification
strictly dominates.

Dropping sub-kilometre parts is separately worthless **for bytes** — 0.7% of points — but
may still be worth doing for **visual clutter**, since interchange ramps are unresolvable
below roughly 400 m/px and add nothing but ink. That is a cartography question, not a
budget one, and it is **deferred until layer 5 has actually been drawn and looked at.**
It does not reopen this ADR: it is a generator filter and a bundle regeneration.

### 4. The coverage limit is stated, not engineered around

13 sites get no roads (Measurement 7). Layer 5 is toggleable and every other overlay layer
is global, so this degrades rather than breaks — an Alaskan or Okinawan site draws
coastline, boundaries, and labels with an empty highway layer. No fallback source is
adopted: the candidates are not public domain, and this is not a defect worth a licence
dependency inside a government/defense approval surface (Principle 4).

### 5. `bundle.manifest.txt` records the tolerance

ADR-0025 §6 already reserves the field. It records `simplification: douglas-peucker,
epsilon 30 m, applied to primary_roads only` alongside the per-layer part and point counts,
so a reviewer reading the manifest sees what a regeneration changed. The three Natural
Earth layers are **not** simplified — they were measured as adequate at their native
density and there is no budget reason to touch them.

## Consequences

- **The whole basemap fits in 9% of the GPU budget** — 11.46 MB against 128 MB, leaving
  the headroom for radar textures across four simultaneous instances that NFR-P-1 wants.
- **The binary lands near 24 MB**, comfortably inside ADR-0006's 30–80 MB band rather than
  filling two thirds of it with map data.
- **The bundle is now lossy, and says so.** Overlay geometry is no longer a faithful
  reproduction of its source. The manifest is what makes that reviewable; ADR-0025 already
  accepted that a binary blob cannot be reviewed in a diff, and this adds a second thing
  the manifest must state.
- **~30 lines of Douglas–Peucker in `utility/map-bake/`.** Dev-only Python. **Zero
  dependency delta** — no crate, no build-time dependency, nothing in the production graph.
  Stage 5's overlay work still has no dependency delta at all.
- **A source refresh is now a decision, not a copy.** Regenerating from a newer TIGER
  vintage re-runs the tolerance; the manifest's counts are what reveal whether the source
  changed materially.

## Rejected alternatives

- **Ship unsimplified.** Nothing hard-fails: the binary fits and 117 ms of projection is
  irrelevant. Rejected because it spends 45% of the per-instance GPU budget on vertices
  spaced 88 m apart, which is not a defensible reading of Lightweight by Design, and
  because the cost buys detail that neither the display nor the radar can resolve.
- **ε tuned to the 230 km default view (~100–200 m).** The literal reading of Q20's own
  wording, and the reason the wording is corrected above. Measurement 5 shows visible
  faceting at 100 m and polygonal roads at 200 m, at zoom levels the application permits
  and where roads matter most.
- **ε = 15 m.** A quarter-pixel at maximum zoom, at 0.89 MB more bundle and 1.77 MB more
  GPU. Strictly more conservative and entirely defensible; rejected because half a pixel on
  a one-pixel line is already unresolvable, and because §2 declines to treat
  `MIN_M_PER_PX` as a constant worth buying insurance against.
- **`RTTYP = 'I'` filtering instead of simplification.** See §3 — dominated on every axis.
- **Two-LOD bake with runtime selection by `m_per_px`.** ADR-0025 §3 bakes part bounding
  boxes and notes they exist "for LOD and draw-range selection later," so this is the
  option the format anticipates. Rejected for v1.0: it buys back exactly the runtime
  complexity ADR-0025 spent itself removing — a second buffer, an upload policy, a switch
  threshold, and a new class of "the roads changed shape while I panned" bug — to save a
  few megabytes of a budget that ε = 30 m already brings inside 9%. The bounding boxes stay
  for whenever a layer genuinely needs this; primary roads does not.
- **A denser or global road source for the 13 uncovered sites.** No public-domain global
  road dataset exists at usable quality; OpenStreetMap is ODbL, which is a share-alike
  licence on a bundled data artifact inside a government/defense approval surface.

## Open questions this ADR does not answer

None. Q20 is closed.

The ramp/stub clutter question in §3 is deliberately **not** raised as a numbered open
question: it cannot be answered without looking at the drawn layer, it gates nothing, and
its answer is a generator filter. It is recorded in Stage 5's sequence instead.
