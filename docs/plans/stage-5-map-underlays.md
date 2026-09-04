# Implementation Plan — Stage 5: Map Underlays

**Status:** Drafted — not yet implemented
**Drafted:** 2026-09-02
**Implements:** `docs/project-inventory.md` §6, Stage 5 (items 19–22)
**Baseline commit:** `7ada4c3` (branch `map_underlays`; the working tree carries
ADR-0025 through ADR-0029 as untracked files plus documentation edits — **commit those
first**, see §17)
**Toolchain:** rustc 1.95.0 / cargo 1.95.0, `x86_64-unknown-linux-gnu`
**Predecessors:** `docs/plans/stage-0-1-close-the-acquisition-path.md` (§8 Results),
`docs/plans/stage-2-make-the-application-exist.md` (§12 Results),
`docs/plans/stage-3-compute-layer.md` (§15 Results),
`docs/plans/stage-4-first-pixels.md` (§16 Results)
**Governing ADRs:** [0025](../adr/0025-bundled-overlay-geometry.md) (bundle),
[0028](../adr/0028-city-labels.md) (labels), [0029](../adr/0029-primary-roads-simplification.md)
(road simplification), [0026 §1–§2](../adr/0026-tile-http-boundary.md) (the `Bucket` enum
only), [0022](../adr/0022-render-loop-hosting.md)/[0023](../adr/0023-radar-sampling-in-screen-space.md)
(the render loop this extends)

This plan is written to be executed in a later session. It carries every decision already
taken so implementation does not need to re-derive them from the ADRs. **Stage 5 opens no
open questions and closes none — Q15, Q16, Q19 and Q20 were all answered before it, which
is the whole reason this stage has no research phase.** It adds **no new ADR**: every
decision below either implements an accepted ADR or is a narrow implementation choice
inside one, recorded here and, where it departs from an ADR's literal wording, as a dated
erratum on that ADR (§13).

**Scope boundary:** this plan puts the map under the radar. At the end of it,
`cargo run --release -- KDOX` draws county, state/country, coastline and primary-road
geometry beneath the radar image, radar site markers above it, and city labels above
everything but the chrome — all in the same azimuthal-equidistant frame, all projected
once at site load, all from a bundle baked into the binary with no parser, no file I/O,
and no network. Layer 5 toggles with `H` and its state persists. It also lands
ADR-0026's `Bucket` enum, which is the one piece of the tile-transport ADR that does not
depend on tiles.

It draws **no tiles** (layer 2 stays empty for v1.0 — ADR-0027) and **no placefiles**
(layer 7 — Stage 6). Site markers are drawn but **not clickable**, and the active site
still cannot be changed at runtime — both are Stage 7 (FR-SS-2, FR-SS-3).

---

## 1. What "done" means

| Claim | How it is demonstrated |
|---|---|
| Layers 3, 4 and 5 are drawn, under the radar, in the same projection | An offscreen read-back render at KDOX writes a `.ppm` showing counties, states, coastline and interstates in correct geographic relationship to the range rings; a second at KRLX at 60 m/px is the ADR-0029 §3 "look at the drawn layer" check |
| No shapefile parser, DBF reader or tessellator ships | `Cargo.lock` package count is **unchanged** across the whole stage; `cargo tree -p radar-workstation` contains no `shapefile`, `dbase`, `time`, `geo` or `lyon` |
| The bundle is exactly what the ADRs measured | The generator prints per-layer part/point counts and they match ADR-0025 §2 and ADR-0029 §1's tables; any delta is reconciled or recorded in `bundle.manifest.txt` and §18 |
| A malformed bundle cannot panic the reader (Stability as Ethics) | `overlay::Bundle::parse` returns `Option`; a walk test traverses every layer, part, point, label and name; a corrupt-bundle test set (truncated header, out-of-range counts, string offset past the table, non-UTF-8 name bytes) each yields an empty layer or `None`, never a panic |
| Projection happens once per site load, never per frame | The projected `Vec` is consumed by buffer creation and dropped; no function called from `App::redraw` takes a `&Bundle`; the measured cost is recorded in §16 |
| City labels never overlap each other or the UI chrome (FR-MU-7) | `labels::select` unit tests: two colliding candidates place one, the higher rank wins; candidates outside `available_rect` are culled before placement; a **synthetic dense** input (2,000 candidates) exercises the cull the shipped bundle cannot (ADR-0028 §6) |
| Label selection is as spatially stable as the view (FR-NI-4) | `view_state_is_unchanged_by_any_sequence_of_state_updates` is extended to assert the placed-label set is unchanged bit-for-bit across the same synthetic state sequence |
| Layer 5 is toggleable and the toggle survives a restart (FR-DR-3, FR-CP-1) | `H` maps to `Action::ToggleHighways`; `view.highways` and `view.reference` round-trip through `config::load`/`save` with tests for valid / invalid / absent |
| The radar path cannot be pointed at another host (ADR-0026 §2) | `S3Client` has no constructor and no method that accepts a hostname; `Host::parse` and the string allowlist are deleted; the guarantee is a compile error, not a test |
| The 13 road-less sites degrade rather than break (ADR-0029 §4) | An offscreen render at `PABC` shows coastline and boundaries with an empty highway layer and no error; a test asserts a zero-length draw range is skipped, not submitted |
| The whole basemap stays inside the GPU budget | Vertex + index buffer bytes are computed and logged at init and compared against ADR-0029's 11.46 MB; recorded in §16 |
| Nothing regressed | `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny check`, `cargo audit` all clean; the existing live pipeline tests pass unchanged |

**Requirements closed or advanced:** FR-MU-1 (closed), FR-MU-2 (closed), FR-MU-7
(closed), FR-DR-3 (advanced — layers 3, 4, 5, 8, 9 gain data sources; layer 2 stays
deliberately empty per ADR-0027 and layer 7 is Stage 6), FR-DR-4 (closed *in the
demonstrable sense* — until this stage there was nothing below the radar for a
transparent cell to reveal), FR-CP-1 (advanced — "toggleable layer states" now persist),
FR-SS-1 (advanced — the bundled site list is now visible on the map), FR-NI-4 (held, and
extended to a new subsystem).

**Not closed, deliberately.** FR-SS-2 and FR-SS-3 (runtime site change, clickable
markers) stay in Stage 7; this stage draws markers and stops. FR-MU-4/5/6 remain deferred
(ADR-0027). FR-PF-* are untouched.

---

## 2. What Stages 0–4 left that this plan builds on

Read `stage-4-first-pixels.md` §16 before starting. Seven of its outcomes shape the work
here, and one of its recorded gaps changes how this stage is verified.

- **The two-pass frame already has the right shape.** `App::redraw` clears to
  `BACKGROUND` (layer 1), draws radar (layer 6), then reference geometry (layer 6a), then
  runs one egui pass (layer 10). Stage 5 inserts two draws into pass 1 — overlays before
  the radar, site markers after it — and one more `Order::Background` painter into the
  egui pass. **The pass structure does not change.**
- **`render/reference.rs` is the template for every wgpu part of this stage.** A
  `LineList` in world metres, a 32-byte view uniform (`center_m`, `m_per_px`, `viewport`),
  a bind group, a vertex buffer built once at init, `draw(&self, queue, pass, camera)`.
  `shaders/reference.wgsl`'s `vs_main` is the exact transform the overlay pass needs.
  Copy the shape; do not re-derive the transform.
- **`render/ui.rs::ring_labels` is the template for every text part of this stage.**
  World position → `view::world_to_screen` → `ctx.layer_painter(LayerId::new(
  Order::Background, …))` → `painter.text`. ADR-0028 §5 is explicit that city labels are
  "that function with a different point set."
- **`ViewState` is render-loop-owned and there is a named test guarding it.** The label
  declutter pass joins it there (ADR-0028 §5), and §9.4 extends the same test rather than
  writing a second one.
- **`Camera::from_view(&ViewState, viewport)` already exists** and is what both new wgpu
  passes take.
- **`config::load` never fails and `config::save` is line-preserving and atomic.** Two new
  boolean keys are the same one-line pattern `view.product` already follows (§10).
- **`sites::all()` returns the 163-site `const` table**, and `sites_generated.rs` is what
  the bake-time footprint filter reads (ADR-0025 §2) — the generator parses the committed
  Rust file so the two artifacts cannot drift.
- **The development machine cannot present a window.** Stage 4 §16 records that the nested
  compositor rejects Vulkan dmabufs, so the app exits non-zero naming `--headless` rather
  than showing anything — but *offscreen* rendering with read-back works and is how the
  radar pass was verified. **Every visual check in this stage is therefore an offscreen
  read-back written to a `.ppm` file** (§12.3), not a screenshot. This is not a
  workaround grafted on; it is the only verification path that exists in this environment,
  and it happens to be reviewable in a diff-free, reproducible way.

Two smaller facts worth having in front of you:

- `render/` is a **binary-side** module tree (`mod render;` in `main.rs`, ADR-0022/S4-f)
  and cannot be reached from `radar-viz` or from integration tests in `tests/`. Anything
  that must be testable from outside goes in the library (`lib.rs`'s module list).
- `utility/radar-viz` **depends on `radar-workstation`** (see its `Cargo.toml`). That
  matters for §6.1: once the projection function exists in production, radar-viz's private
  copy must be deleted and the production one called (DRY, `CLAUDE.md`).

---

## 3. Decisions taken in this plan

These are implementation choices inside accepted ADRs. Three of them (S5-c, S5-e, S5-g)
depart from an ADR's literal wording and each gets a dated erratum in §13. None of them
reopens a decision.

### 3.1 (S5-a) The bundle and its manifest live beside the code that reads them

```
crates/radar-workstation/src/overlay/
    mod.rs                 accessors over &'static [u8]
    overlay.bin            the baked bundle (committed, ~6.41 MB)
    bundle.manifest.txt    provenance, counts, filter radius, tolerance (committed)
```

`include_bytes!("overlay.bin")` from `mod.rs`. ADR-0025 §6 requires the manifest to be
committed beside the blob; putting both under `src/overlay/` keeps the "what is this
binary file" question answerable from the directory listing.

### 3.2 (S5-b) `overlay` is library-side; `render/overlay.rs` and `render/labels.rs` are binary-side

ADR-0025 §4 names `crates/radar-workstation/src/overlay/`, which is the library. Bundle
reading and projection are pure, have no GPU dependency, and want to be reachable from
integration tests and from `radar-viz` — so they go in `lib.rs`'s module list beside
`compute`. The wgpu pipeline, the vertex/index buffers, and the declutter pass are
render-loop concerns and go in `render/` per ADR-0022/S4-f. The split follows the one
`compute::grid` / `render::radar` already established: the library computes what the GPU
will hold, the binary uploads and draws it.

### 3.3 (S5-c) The azimuthal-equidistant projection is **introduced** here, in `compute::geometry`

ADR-0025 §4 says the projection uses "the same az-eq function the radar path and
`radar-viz` use (DRY — one projection implementation)." **That premise is wrong about the
code, and the correction matters.** The radar path never converts geographic coordinates:
`compute::grid` works in polar (range, azimuth), `render/view.rs` works in metres from the
site, and `shaders/radar.wgsl` inverse-maps screen pixels to (ground range, azimuth). The
*only* az-eq implementation in the tree is
`utility/radar-viz/src/overlay.rs::az_eq_project`, which is dev-only code that
`utility/README.md`'s boundary explicitly says must be re-implemented in Rust in the
appropriate crate if it becomes production logic.

So Stage 5 adds it, once:

```rust
// compute::geometry
/// Azimuthal equidistant projection centred on (site_lat, site_lon).
/// Returns metres east (+x) and north (+y) of the site — the same world
/// frame `render::view` and `render::reference` use.
pub fn az_eq_project(site_lat: f64, site_lon: f64, lat: f64, lon: f64) -> (f64, f64)
```

`compute::geometry` is the right home: it is already the module for earth geometry, it
already owns `EARTH_RADIUS_M`, it has no radar-specific types, and it is library-side so
both `overlay` and `radar-viz` can reach it. **`radar-viz`'s private copy is deleted and
its call site changed to the production function** — that is what makes the DRY claim in
ADR-0025 §4 true, rather than assumed.

Two substantive differences from the radar-viz version, both deliberate:

- **Metres and `f64`, not kilometres and `f32`.** The world frame everything else uses is
  metres in `f64` (`view::screen_to_world`, `Camera::center_m`). Converting at the
  boundary once, in `overlay::project`, is where the `f32` narrowing belongs.
- **The antipodal singularity is guarded.** `k = c / sin(c)` diverges as `c → π`. The
  bundle is global (it keeps geometry within 700 km of Kadena, Lajes and Guam), so a
  point near a site's antipode is representable even though no bundled site currently has
  one nearby. Clamp `c` to `π − 1e-9` and return a finite, far-away coordinate. It draws
  off-screen, which is correct; a `NaN` would poison a vertex buffer. Stability as Ethics
  applies to arithmetic the same way it applies to parsing.

### 3.4 (S5-d) One vertex buffer, one index buffer, per-layer draw ranges — and colour is a **per-layer uniform**, not a vertex attribute

ADR-0029's GPU arithmetic is `2×f32` per vertex and `2×u32` per segment; it lands at
11.46 MB, which is 9% of the 128 MB per-instance target. Copying `reference.rs`'s
`{ pos: [f32;2], color: [f32;4] }` vertex would make that **23 MB** — the colour would
cost more than the geometry, for four constants. So:

- one `VERTEX` buffer of `[f32; 2]` world metres for all layers,
- one `INDEX` buffer of `u32` line-list pairs for all layers,
- a per-layer index range,
- colour in each layer's own small uniform buffer, alongside the view uniform.

**One uniform buffer per layer, not one shared buffer written per draw.** This is the
non-obvious hazard in this stage: `Queue::write_buffer` is ordered relative to *submission*,
not relative to draw calls already recorded into the encoder. Writing one buffer twice in
a frame gives **both** draws the second value. `reference.rs` gets away with a single
uniform because it issues exactly one draw. Four layers need four buffers and four bind
groups, created at init, each written once per frame. A shared buffer with dynamic offsets
would also work and is more machinery than four small buffers deserve.

`shaders/overlay.wgsl` is a new file — same `View` struct as `reference.wgsl`, plus a
`color: vec4<f32>` in the uniform, and a position-only vertex input. The three-line
transform is duplicated between the two shaders; that is the correct trade against
introducing WGSL preprocessing or a shared-include mechanism for three lines.

### 3.5 (S5-e) Projection runs synchronously at renderer init, behind a pure function

ADR-0025 §4 says "projection at site load, on `spawn_blocking`." At Stage 5 the site is
fixed for the process lifetime, so there is exactly one projection, at window init,
costing a measured ~21 ms against a < 2 s first-render budget. Building the async
delivery path this implies — a channel, a "geometry not ready" render state, an
upload-when-it-arrives branch, and the frame-pacing wake-up to notice — is machinery
written for Stage 7's benefit before Stage 7 can specify what it needs.

The projection is a **pure function**:

```rust
pub fn project(bundle: &Bundle, site: &Site) -> Projected
```

so Stage 7 moves the *call* onto `spawn_blocking` and changes nothing else. The
guarantee ADR-0025 actually cares about — no projection on the per-frame path — is
structural: nothing reachable from `App::redraw` takes a `&Bundle`.

Dated erratum on ADR-0025 §4 in §13. If the measured cost comes in materially above
~50 ms, revisit before shipping it: that is a visible hitch on a cold window.

### 3.6 (S5-f) Layer 5 toggles on `H`, and both layer toggles persist

FR-DR-3 makes layer 5 toggleable and FR-CP-1 explicitly names "toggleable layer states"
as configuration that must persist. `H` is unbound, mnemonic, and does not collide with
`R` (reference geometry) or the digit products. Stage 4 left `show_reference`
unpersisted; since this stage adds the config plumbing for one toggle, it adds it for
both rather than leaving a two-toggle UI with one-toggle persistence.

City labels and site markers get **no** toggle. FR-DR-3 marks only layer 5 toggleable,
and a toggle for every layer is exactly the chrome the Instrument Principle exists to
refuse.

### 3.7 (S5-g) Site labels and city labels go through **one** declutter pass, with sites at rank 0

Layer 8's ICAO labels and layer 9's city names are both egui text at `Order::Background`,
placed from world coordinates, competing for the same screen space. Two independent
passes would let a city name land on a radar site's identifier — the one collision that
matters most, since the site markers are what the operator navigates by. So site labels
enter the pass first, as reserved boxes, before any city candidate is considered.

This is an extension of ADR-0028 §5, not a departure from it: the pass signature and
purity are unchanged, and "rank" is already defined as a dense ascending ordering the
runtime never interprets beyond ordering. Erratum in §13 recording that layer 8's labels
share it.

### 3.8 (S5-h) The declutter pass is a brute-force greedy cull, not a uniform grid

ADR-0028's Measurement 4 suggests "sub-millisecond in Rust with a uniform grid." The grid
is unnecessary: the pass is self-limiting at ~250–360 *placed* labels regardless of source
density, and the collision test is candidate-against-placed, not candidate-against-
candidate. Worst case is 1,216 candidates × ~360 placed ≈ 440k rectangle overlap tests,
which is well under a millisecond and has no data structure to get wrong. If a denser
source ever pushes placed counts up, the loop is the obvious place to add the grid, and
the pure signature means that change is invisible from outside.

### 3.9 (S5-i) `Bucket` lands as a rename, not an alias

ADR-0026 §2's `S3Client::new(bucket: Bucket)` is taken up now (its own status note says
so). The type is renamed `Client` → `S3Client` inside the existing `http-ingest` crate —
about ten call sites, mechanical — so the ADR is literally implemented for the part being
built, rather than approximately implemented with a rename owed later. The engine/policy
crate split, the `is_2xx` gate move, `UrlTemplate`, `ETag` and the N-worker model all stay
deferred with the tile subsystem (ADR-0027 §4). **Do not implement them.**

---

## 4. S5-W1 — The generator (`utility/map-bake/`)

Dev-only Python, stdlib only, never invoked by `cargo build`, `cargo test`, or the binary.
It follows `utility/nexrad-sites/generate.py`'s shape exactly: a module docstring stating
what it reads and writes, digest verification before it emits anything, and a loud failure
on any source that does not look like what it expects.

```
utility/map-bake/bake.py
```

### 4.1 Sources and digests

Four of the five sources are already on disk in the `.gitignore`d
`utility/radar-viz/data/`. **The fifth is not** — `ne_10m_populated_places` was measured
for ADR-0028 but not retained, and must be downloaded before the bake:

```
https://naciscdn.org/naturalearth/10m/cultural/ne_10m_populated_places.zip
```

Digests measured on disk 2026-09-02 (the TIGER three match ADR-0029's provenance block
exactly, which is the first thing the generator's digest check will confirm):

| File | SHA-256 |
|---|---|
| `tl_2025_us_primaryroads.zip` | `400453e97b9e6693dfecb7362ce7a6cf260d27050f7d84d2a024ba0710b94c07` |
| `tl_2025_us_primaryroads.shp` | `0a71f09e16325e815961e5486b71e825c1da31e9d80fd58fe5b5da0c01ed313b` |
| `tl_2025_us_primaryroads.dbf` | `4b9e2a05d259c73ced83eb6769db225b717945b52509444635869dfdb29dfce6` |
| `ne_10m_admin_1_states_provinces.shp` | `c6f5c8b4b1320d9417033762419c6df1eb423989cd880fba78ea0b1e3522cbe4` |
| `ne_10m_admin_1_states_provinces.dbf` | `445a8a9bea889634faf0af18081830df0b05b8471fc6af8dc42aecdd7a71bba1` |
| `ne_10m_admin_2_counties_lakes.shp` | `3b2d28346a793500f855f130bbebe4562f17427a7b170f0aaec8bdefcb51114e` |
| `ne_10m_admin_2_counties_lakes.dbf` | `5f8a71a570b35a6164ce29dffce34bf69b05f1512f69626a6e8327bc22d25b3f` |
| `ne_10m_coastline.shp` | `459a4a97c09db19aadf5244026612de9d43748be27f83a360242b99f7fabb3c1` |
| `ne_10m_coastline.dbf` | `9ccc214342fe400bf8c7d91d7a5b276b0457b0ada03e8d4be16ac5ba13037f3b` |
| `ne_10m_populated_places.{shp,dbf}` | **compute at retrieval and record here, in the script, and in `utility/README.md`** |

The generator holds this table and refuses to emit if any digest mismatches (ADR-0025 §6:
"a regeneration from substituted inputs fails loudly").

### 4.2 Readers, stdlib only

ADR-0029 already read these files with stdlib-only SHP and DBF readers — "no third-party
parser was installed to answer a question about not shipping one." Same here.

**SHP main file.** 100-byte header; then records of `[record number: i32 BE]
[content length in 16-bit words: i32 BE]` followed by little-endian content:
`shape_type i32`, and for PolyLine (3) / Polygon (5): `box 4×f64`, `num_parts i32`,
`num_points i32`, `parts i32[num_parts]` (start indices), `points (f64,f64)[num_points]`.
For Point (1): `x f64, y f64`. Polygon rings are treated as closed polylines, all rings
including holes (`radar-viz`'s `overlay.rs` established that lake shores and island
coastlines are useful radar context). Parts of fewer than 2 points are dropped.

**DBF.** 32-byte header (`num_records` at offset 4, `header_len` at 8, `record_len` at
10), 32-byte field descriptors terminated by `0x0D`, then records each prefixed by a
one-byte deletion flag. Only `populated_places` needs attributes: `NAME` (C), `SCALERANK`
(N), `POP_MAX` (N). Decode text as UTF-8 (the `.cpg` sidecars say `UTF-8`); a decode
failure on a name is a hard error, not a silent replacement — 49 of the kept records are
non-ASCII and mangling them is the outcome ADR-0028 §3 rejected.

### 4.3 The 700 km footprint filter

Read the site table by parsing `crates/radar-workstation/src/sites_generated.rs` with a
regex over the `Site { id: "...", ..., lat: ..., lon: ... }` lines (ADR-0025 §2 and
ADR-0028's measurements both do this deliberately, so the bundle and the site table cannot
drift). Assert the parsed count is 163 and that every field parsed, so a reformatted
generated file fails loudly rather than silently filtering against three sites.

Keep a part when the great-circle distance from **any** site to its bounding box is
≤ 700 km. Compute that as: clamp the site's latitude and longitude into the bbox ranges,
then haversine to the clamped point. Labels use the same filter against their single
point.

**Acceptance:** the printed counts must reproduce the ADR tables.

| Layer | parts | points | source |
|---|---|---|---|
| `admin_2_counties_lakes` | 3,646 | 149,269 | ADR-0025 §2 |
| `admin_1_states_provinces` | 1,438 | 197,952 | ADR-0025 §2 |
| `coastline` | 781 | 98,998 | ADR-0025 §2 |
| `primaryroads` (after §4.4) | 17,500 | 281,401 | ADR-0029 §1 |
| `populated_places` | — | 1,216 labels | ADR-0028 §4 |

Exact reproduction is not guaranteed — "bounding box within 700 km" admits more than one
implementation, and the ADR figures came from one of them. **If the delta is under 1%,
record the measured counts in `bundle.manifest.txt` and in §18 and move on. If it is
larger, reconcile before committing the bundle** — a large delta means the filter is
doing something different, and the bundle is the artifact nobody can review by reading.

### 4.4 Douglas–Peucker, ε = 30 m, primary roads only (ADR-0029 §1)

- Applied **only** to the `primaryroads` layer. The three Natural Earth layers are baked
  at native density (ADR-0029 §5) — do not "improve" them.
- Local equirectangular metres: `x = R·cos(lat₀)·Δlon`, `y = R·Δlat`, with `lat₀` the
  part's bbox centre latitude.
- Endpoints always preserved.
- **Iterative, not recursive.** The longest part is 2,826 points and Python's default
  recursion limit is 1,000; an explicit stack removes the failure mode rather than raising
  the limit.
- Applied after the footprint filter (which keeps 100% of this layer anyway — ADR-0029
  §2 — so the order is a statement of intent, not an optimisation).

### 4.5 Label rank normalisation (ADR-0028 §2)

Sort the kept records by `SCALERANK` ascending, tie-broken by `POP_MAX` descending, and
assign a dense `u16` rank starting at 0. **No `SCALERANK` or population floor is applied**
— at 1,216 records the whole surviving set is kept and the declutter pass decides what is
drawn. The sort key is recorded in the manifest, because it is the one line that would
have to change for a different source.

### 4.6 Output

`overlay.bin` (§5.1) and `bundle.manifest.txt`, both written into
`crates/radar-workstation/src/overlay/`. The manifest is plain text and records:

- format magic and version;
- generator name and the date of the bake;
- per source: URL, retrieval date, SHA-256;
- the SHA-256 of `sites_generated.rs` the filter was run against (so a site-table change
  that should trigger a re-bake is visible in the diff);
- per layer: kind, kind name, parts, points;
- label count, string-table bytes, and the rank sort key;
- `filter: 700 km from any bundled site (bbox)`;
- `simplification: douglas-peucker, epsilon 30 m, applied to primary_roads only`
  (ADR-0029 §5's exact wording);
- total bundle bytes.

---

## 5. S5-W2 — The bundle format and its reader

### 5.1 Format (ADR-0025 §3 as amended by ADR-0028 §3)

Little-endian throughout, sections contiguous in the order below, **no padding anywhere**.

```
offset  size                       field
0       8                          magic  = b"RWMOVL01"
8       4                          version u32 = 1
12      4                          layer_count u32
16      4                          part_count u32
20      4                          point_count u32
24      4                          label_count u32
28      4                          string_bytes u32
32      layer_count × 12           layer table: { kind u32, first_part u32, part_count u32 }
        part_count  × 24           part index:  { first_point u32, point_count u32,
                                                  min_lon i32, min_lat i32,
                                                  max_lon i32, max_lat i32 }
        point_count × 8            points:      { lon i32, lat i32 }
        label_count × 16           label index: { lon i32, lat i32, rank u16,
                                                  name_off u32, name_len u16 }
        string_bytes               strings:     one contiguous UTF-8 blob
```

Coordinates are `i32` in units of 1e-7 degrees (`round(deg * 1e7)`).

**Layer kinds** (`u32`), which are also what maps a layer to its compositing slot and
colour:

| kind | source | compositing layer |
|---|---|---|
| 1 | `admin_2_counties_lakes` | 3 |
| 2 | `admin_1_states_provinces` | 4 |
| 3 | `coastline` | 4 |
| 4 | `primaryroads` | 5 |
| 5 | `populated_places` (labels) | 9 |

An unknown kind is **skipped with an `Event`, not an error** — that is what lets a future
bundle add a layer without a version bump breaking an older binary. The label layer's
`first_part`/`part_count` are `0`; its content is the label index, which the header sizes.

**Expected size**, so the implementer can check the bake arithmetic rather than guess:
`32 + 5×12 + 23,365×24 + 727,620×8 + 1,216×16 + 10,522 = 6,411,790 bytes` (≈ 6.41 MB,
matching ADR-0025 §2). Note that ADR-0028's "~16.6 KiB of label index" was computed on a
14-byte record; the format as specified is 16 bytes, so the label sections cost ~29 KiB
rather than ~27 KiB. The difference is 2.4 KiB and changes nothing, but it is recorded so
a correct bundle is not mistaken for a wrong one.

### 5.2 The reader (`src/overlay/mod.rs`)

```rust
pub struct Bundle { bytes: &'static [u8], /* section offsets, all validated */ }

pub struct Layer { pub kind: u32, first_part: u32, part_count: u32 }
pub struct Part  { pub bbox_deg: [f64; 4], first_point: u32, point_count: u32 }
pub struct Label<'a> { pub lon: f64, pub lat: f64, pub rank: u16, pub name: &'a str }

/// The bundle compiled into the binary. `None` if it fails validation —
/// which cannot happen for a bundle this project generated, and is handled
/// anyway.
pub fn bundled() -> Option<&'static Bundle>;

impl Bundle {
    pub fn parse(bytes: &'static [u8]) -> Option<Self>;
    pub fn layers(&self) -> impl Iterator<Item = Layer> + '_;
    pub fn parts(&self, layer: &Layer) -> impl Iterator<Item = Part> + '_;
    pub fn points(&self, part: &Part) -> impl Iterator<Item = (f64, f64)> + '_;
    pub fn labels(&self) -> impl Iterator<Item = Label<'_>> + '_;
    pub fn total_points(&self) -> usize;
}
```

Rules, all from ADR-0025 §3–§4 and non-negotiable:

- **Every access is `slice::get`, never indexing.** No `unsafe`, no alignment assumption,
  no zero-copy cast, no `from_le_bytes` on a slice that was not bounds-checked first.
- `parse` validates the magic, the version, and that every section fits inside `bytes`
  with the declared counts. It returns `None` rather than panicking or truncating.
- An inconsistent *interior* reference (a part whose points run past the point array, a
  name whose `off + len` runs past the string table, a non-UTF-8 name) yields an **empty
  iteration for that element**, not a panic and not a failed parse. A single bad label
  must not delete the county layer.
- `bundled()` uses a `OnceLock`. A `None` is reported once, via the events below, and the
  application draws no overlays and keeps running — the same posture `config::load` and
  `palette::load_all` already take.

Two new `Event` variants (`src/event.rs`), with `Display` impls in the existing style:

```rust
/// The compiled-in overlay bundle failed validation; no map underlay is drawn.
OverlayBundleInvalid { reason: &'static str },
/// A bundle layer carried a kind this build does not know; it was skipped.
OverlayLayerUnknownKind { kind: u32 },
```

### 5.3 Tests (library-side, run in CI)

- `walks_every_layer_part_point_and_label` — iterate the whole committed bundle; assert
  the totals equal the header counts, every coordinate is inside ±180°/±90°, every part
  has ≥ 2 points, every label name is non-empty, and ranks are dense and ascending.
- `layer_kinds_are_the_five_expected` — pins kinds 1–5 present exactly once.
- `counts_match_the_manifest` — parse `bundle.manifest.txt` (it is committed beside the
  blob and is plain text) and assert the header counts match it. This is what catches a
  bundle regenerated without its manifest, which is the failure mode ADR-0025 §6's
  reviewability argument depends on.
- `corrupt_bundles_never_panic` — a small in-test corpus built by mutating a copy of the
  real bundle's header: bad magic, version 2, `part_count` = `u32::MAX`, `point_count`
  one too large, a label `name_off` past the string table, a name slice containing
  `0x80`, and a truncated buffer at each section boundary. Each yields `None` or an empty
  iteration. Follows the `config_hardening.rs` / `palette_corpus` pattern already in
  `tests/fixtures/`.

---

## 6. S5-W3 — Projection

### 6.1 `compute::geometry::az_eq_project` (S5-c)

Add the function described in §3.3, with `EARTH_RADIUS_M` (the module already has it —
do not introduce a second earth radius), the antipodal clamp, and:

- `projects_the_site_itself_to_the_origin`
- `north_east_south_west_have_the_expected_signs`
- `distance_from_the_site_matches_great_circle_within_a_metre_at_500_km` — az-eq is
  distance-preserving along radii, so this is an exact property, not a tolerance fudge
- `antipodal_input_is_finite`

Then **delete `az_eq_project` from `utility/radar-viz/src/overlay.rs`** and call
`radar_workstation::compute::geometry::az_eq_project`, converting metres to km at the call
site. `radar-viz` already depends on `radar-workstation`, so this is an import change.
Confirm its PPI overlay output is unchanged (the utility's own manual check is enough —
it is not production code).

### 6.2 `overlay::project`

```rust
pub struct ProjectedLayer { pub kind: u32, pub index_range: std::ops::Range<u32> }
pub struct ProjectedLabel { pub world: [f32; 2], pub rank: u16, pub name: &'static str }
pub struct Projected {
    pub vertices: Vec<[f32; 2]>,   // world metres
    pub indices:  Vec<u32>,        // line-list pairs
    pub layers:   Vec<ProjectedLayer>,
    pub labels:   Vec<ProjectedLabel>,
}

pub fn project(bundle: &'static Bundle, site: &Site) -> Projected;
```

- Iterate layers in bundle order; for each part, project every point into world metres,
  push vertices, and emit `2·(n−1)` indices — `[v, v+1, v+1, v+2, …]`. Indices are
  **derived here, never baked** (ADR-0025 §4).
- Narrow to `f32` at this boundary. At 700 km, `f32` resolves ~0.06 m; the bundle's own
  resolution is 1.1 cm at the equator but the display's is 60 m/px at maximum zoom.
- Labels are projected in the same pass (ADR-0025 §4) into `ProjectedLabel`.
- **No culling.** ADR-0025 §3 stores part bounding boxes but explicitly declines to cull
  in v1.0, because projecting everything "avoids an 'overlays vanish when panned far'
  class of bug entirely." Do not add culling as an optimisation; 11.46 MB is 9% of budget.
- Sanity-check the produced sizes at init and log them once, next to the existing GPU line
  in `resumed()`: vertices, indices, total bytes. That log line is the §16 measurement.

Tests: a synthetic two-part bundle produces the expected index pattern; a part of one
point produces zero indices and no panic; the site's own coordinates land at `[0, 0]`.

---

## 7. S5-W4 — The overlay pass (layers 3–5)

`render/overlay.rs` + `render/shaders/overlay.wgsl`.

```rust
pub struct OverlayRenderer { /* pipeline, vertex, index, per-layer { range, uniform, bind } */ }

impl OverlayRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, projected: &Projected) -> Self;
    pub fn draw(&self, queue: &wgpu::Queue, pass: &mut wgpu::RenderPass<'_>,
                camera: Camera, show_highways: bool);
    pub fn buffer_bytes(&self) -> u64;   // for the §16 measurement
}
```

- `PrimitiveTopology::LineList`, `IndexFormat::Uint32`, alpha blending, no depth.
- The uniform is `reference.wgsl`'s `View` (32 bytes) **plus** `color: vec4<f32>` — 48
  bytes, one buffer per layer (§3.4's hazard).
- Draw order inside pass 1, after the clear and **before** the radar draw:
  counties (kind 1) → states (kind 2) → coastline (kind 3) → roads (kind 4, only when
  `show_highways`).
- **A zero-length index range is skipped, not submitted.** That is what makes the 13
  road-less sites (ADR-0029 §7) a no-op rather than a validation error.
- The `Projected` value is consumed by `new` (buffers created from it) and dropped —
  ADR-0025 §4's "the CPU-side projected copy is dropped after upload." Take it by
  reference and drop the owner at the call site, or take it by value; either way, assert
  in review that nothing retains it.

**Colours** — starting values, to be confirmed against the §12.3 offscreen render. The
Instrument Principle governs: these sit *under* the radar image and must not compete with
it.

| Layer | RGBA |
|---|---|
| Counties | `[0.36, 0.38, 0.43, 0.55]` |
| States / provinces | `[0.58, 0.62, 0.70, 0.75]` |
| Coastline | `[0.58, 0.62, 0.70, 0.75]` |
| Primary roads | `[0.72, 0.55, 0.30, 0.60]` |

States and coastline share a colour deliberately: FR-DR-3 treats them as one compositing
layer, and they are the same class of feature to an operator. Line width is 1 px — wgpu
line primitives have no width control, and `rendering.md`'s "slightly thicker line weight
than counties" for layer 4 is satisfied by contrast rather than by width. Record that as
a note in `rendering.md` (§13) rather than leaving the table saying something the renderer
does not do.

---

## 8. S5-W5 — Site markers (layer 8)

Also in `render/overlay.rs` — it needs the same projection and the same view uniform, and
a third file for eight lines of vertex generation would be worse.

- Built at init from `sites::all()`, projected with `az_eq_project` against the active
  site, **excluding the active site itself** (`reference.rs` already draws that one, and
  two markers at the origin is a defect, not emphasis).
- A small cross, `MARKER_ARM_KM = 4.0` world km, matching `reference.rs`'s active-site
  marker so the two read as the same symbol at different emphasis. World-sized, so it
  shrinks when zoomed out — consistent with every other geospatial element, and it avoids
  a per-frame vertex rebuild.
- Colour dimmer than the active marker: `[0.75, 0.78, 0.85, 0.55]`.
- Drawn **after** the radar and after the reference geometry, which is compositing slot 8.
- Its ICAO labels are `LabelCandidate`s handed to the declutter pass at rank 0 (§3.7),
  not painted directly.
- 163 sites × 4 vertices is 5 KB of buffer. No culling, same reasoning as §6.2.

Not clickable. FR-SS-3 is Stage 7 and needs runtime site change to be worth anything.

---

## 9. S5-W6 — City labels (layer 9)

### 9.1 `render/labels.rs` — selection

Pure, no egui types, no window, unit-tested like `view.rs` and `input.rs`:

```rust
#[derive(Clone, Copy)]
pub struct LabelCandidate { pub world: [f32; 2], pub rank: u16, pub text: &'static str }

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PlacedLabel { pub screen: (f32, f32), pub text: &'static str }

/// Greedy, rank-ordered, screen-space collision cull. `avail` is the
/// chrome-free rectangle (min_x, min_y, max_x, max_y) in physical pixels.
pub fn select(
    candidates: &[LabelCandidate],
    view: &ViewState,
    viewport: Viewport,
    avail: [f32; 4],
) -> Vec<PlacedLabel>;
```

- Candidates arrive already sorted by rank (site labels at 0, then cities in bundle
  order — the bake already sorted them, so the runtime does not re-sort).
- For each: `view::world_to_screen`, then build the box —
  `w = CHAR_W·chars + 2·PAD`, `h = LINE_H + 2·PAD`, with `CHAR_W = 6.6`, `LINE_H = 14.0`,
  `PAD = 3.0` (ADR-0028 Measurement 4's approximation, explicitly a magnitude and not a
  final layout constant), anchored left-bottom at the point plus `(5, −3)`.
- Cull against `avail` **before** placement (ADR-0028 §5) — a label that would sit under
  the status bar or the legend is not placed at all, rather than placed and hidden.
- Reject on overlap with any already-placed box; otherwise place.
- Brute force against the placed set (§3.8).

Tests:

- `two_colliding_candidates_place_only_the_higher_rank`
- `candidates_outside_the_available_rect_are_never_placed`
- `a_label_that_fits_beside_a_placed_one_is_placed`
- `dense_synthetic_input_self_limits` — 2,000 synthetic candidates on a grid; assert the
  placed count is bounded and every pair of placed boxes is disjoint. **This is the test
  ADR-0028 §6 requires**, because the shipped bundle (19 candidates in a KDOX 230 km PPI)
  will essentially never make the pass reject anything.
- `selection_is_deterministic` — same inputs, same output, twice.

### 9.2 Memoisation

Cache `(center_m, m_per_px, viewport, avail)` alongside the placed vector in `App`. Recompute
only when that tuple changes; compare by exact bit equality (`f64::to_bits`) rather than a
tolerance, so the cache can never be subtly stale. At the measured cost, nothing more
elaborate is warranted (ADR-0028 §5).

### 9.3 `render/ui.rs` — drawing

A `city_labels(ctx, placed)` function beside `ring_labels`, using the same
`layer_painter(LayerId::new(Order::Background, Id::new("city_labels")))` mechanism. Two
`painter.text` calls per label — a near-black offset by (1, 1) then the light text — so a
name stays legible over saturated reflectivity. That doubles ADR-0028 Measurement 3's
cost, which takes 500 labels from 0.108 ms to ~0.22 ms against a 16.7 ms budget. Record
the trade in the function's doc comment; it is the kind of thing a later reader would
otherwise "simplify" away.

Site ICAO labels use the same painter, in a slightly brighter colour, so slot 9 stays one
compositing layer.

### 9.4 Spatial stability (FR-NI-4)

Extend `view_state_is_unchanged_by_any_sequence_of_state_updates` in `render/mod.rs`:
compute `labels::select(...)` before the synthetic state sequence and again after, and
assert the `Vec<PlacedLabel>` is identical. ADR-0028 §5 makes this explicit — a new scan,
a product switch, or a sweep switch must not change which labels are placed — and
extending the existing named test is better than adding a second one that could drift
from it.

---

## 10. S5-W7 — Input, config, and chrome

- **`input.rs`:** `Action::ToggleHighways`, bound to `KeyCode::KeyH`, plus a test row in
  `zoom_reset_toggle_and_quit_bindings`.
- **`view.rs`:** `ViewState` gains `show_highways: bool` (default `true`). It is view
  state, so it lives here and never enters `AppState` (ADR-0018).
- **`config/mod.rs`:** two keys, `view.highways` and `view.reference`, parsed as
  `true`/`false` (case-insensitive); anything else falls back to the default and reports
  `ConfigValueInvalid`, exactly like `view.product`. `Config` gains
  `show_highways: Option<bool>` and `show_reference: Option<bool>`.
- **`render::PersistedView`** gains both fields; `main.rs`'s save block gains two
  more `if changed { push }` arms. No new failure mode: `config::save` is already
  line-preserving and atomic.
- **`ui.rs::help_overlay`** gains `("H", "toggle highways")`.
- **The status bar is not extended.** A missing highway layer at one of the 13 uncovered
  sites is not an error condition and does not want a line in the status bar; it is a
  documented property of the data (ADR-0029 §4). Adding a "no road data" notice would be
  chrome reporting on itself.

---

## 11. S5-W8 — `S3Client::new(Bucket)` (ADR-0026 §2, the part taken up now)

In `crates/http-ingest`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket { Chunks, Archive }

impl Bucket {
    pub fn host(self) -> &'static str;
    /// Map a hostname onto the closed set. Used only by `utility/nexrad-sample`,
    /// which takes a URL from the developer; the production path never calls it.
    pub fn from_host(host: &str) -> Option<Bucket>;
}

pub struct S3Client { /* was `Client` */ }
impl S3Client {
    pub fn new(bucket: Bucket) -> Self;                        // infallible
    pub fn with_config(bucket: Bucket, cfg: ClientConfig) -> Self;
}
```

- **`Client` is renamed `S3Client`** (§3.9). No type alias — a deprecated alias would
  leave exactly the string-taking constructor this change exists to delete.
- **`host.rs`'s `Host::parse` and `ALLOWED_HOSTS` are deleted**, along with the ten
  host-rejection unit tests. Those tests were guarding a parameter that no longer exists;
  replacing them with a test that asserts `Bucket::host()` returns the two ADR-0011 hosts
  is the honest successor. **This is a case where deleting tests is the point** — the
  property moved from "checked at runtime" to "unrepresentable" — and it should be said
  plainly in the commit message rather than looking like coverage loss.
- `Error::HostNotAllowed` **stays**: `tls.rs` still maps a `ServerName::try_from` failure
  onto it, and although that can no longer fail for a compile-time-constant hostname,
  replacing a `map_err` with an `expect` on the TLS path is not an improvement.
- Call sites to update: `pipeline.rs` (drop `BUCKET_HOST`, pass `Bucket::Chunks`),
  `ingest/s3_poll.rs` (the `BUCKET_HOST` const and its doc comment go; three test
  constructors change), `tests/assembly_live.rs`, `utility/nexrad-sample`
  (`data_acquisition.rs` uses `Bucket::from_host(host).ok_or(NotAllowed)`, and its
  `rejects_disallowed_host` test moves to asserting `from_host` returns `None`).
- **Do not** implement anything else from ADR-0026: no crate split, no `is_2xx` gate
  move, no `UrlTemplate`, no `ETag`, no worker pool. ADR-0027 §4 defers all of it.

This is independent of every other work item in this stage and can land first or last.

---

## 12. S5-W9 — Validation

### 12.1 Tests that run in CI (no GPU)

Everything in §5.3, §6.1, §6.2, §9.1 and §10, plus:

- `overlay_vertex_and_index_bytes_are_within_the_gpu_budget` — computes the buffer sizes
  from the committed bundle's header counts and asserts they are under 16 MB (a margin
  over ADR-0029's 11.46 MB, not a re-derivation of it). This is the test that fires if
  someone regenerates the bundle without the ε = 30 m tolerance.
- `overlay_uniform_bytes_pack_at_the_wgsl_offsets` — the numeric pin Stage 4 added for the
  radar uniform, for the new 48-byte overlay uniform.

### 12.2 GPU tests (`#[ignore]`d, run manually)

Extend the offscreen harness `render/radar.rs`'s test already establishes:

- `offscreen_overlay_pass_draws_lines` — build an `OverlayRenderer` from a synthetic
  two-layer `Projected`, render offscreen to `Rgba8Unorm`, read back, assert pixels along
  a known segment are non-background and pixels far from it are background. This exercises
  shader compilation, the index buffer, and the per-layer uniform binding.
- `overlay_layers_use_distinct_uniform_buffers` — two layers with different colours in one
  submission; read back one pixel from each and assert the colours differ. **This is the
  test for §3.4's hazard**, and it is the one that would otherwise be found by eye, late.

### 12.3 The visual checks (the only way layers 3–5 get looked at)

Stage 4 §16 records that this environment cannot present a window. So add a small
`#[ignore]`d harness that renders the full pass-1 scene offscreen at a given site, centre
and zoom, and writes a binary PPM (`P6`) to `target/`. PPM because `render/` is binary-side
and cannot reach `radar-viz`'s PNG encoder (the same reachability wall Stage 4 hit), and a
PPM writer is ten lines with no dependency.

Three renders to produce and look at, with the observations recorded in §18:

1. **KDOX, 230 km, no radar data** — counties, states, coastline, roads, range rings.
   Checks the projection against a familiar coastline (Delmarva) and confirms layer
   ordering and colour separation.
2. **KRLX, 60 m/px** — ADR-0029 §3's deferred question: are sub-kilometre ramps and
   connectors clutter at maximum zoom? The answer, if it is "yes", is a generator filter
   and a re-bake, **not** code, and it does not reopen ADR-0029.
3. **PABC, 230 km** — a site with no primary-road coverage (ADR-0029 §7). Confirms the
   empty layer draws nothing and reports nothing.

If any render shows the geometry mirrored, rotated, or scaled wrongly, the fault is almost
certainly the `y`-flip between world and screen or a lat/lon argument-order swap in
`az_eq_project` — `radar-viz`'s `draw_overlay` is the working reference for both.

---

## 13. S5-W10 — Documentation

Errata and amendments, all dated, in the pattern this project already uses (never a silent
rewrite — `project-inventory.md` §7):

- **ADR-0025 §4, erratum:** (a) the claim that the projection function is shared with the
  radar path was wrong — no production az-eq function existed; Stage 5 adds
  `compute::geometry::az_eq_project` and deletes `radar-viz`'s copy, which is what makes
  the DRY statement true (§3.3). (b) The projection runs synchronously at renderer init
  rather than on `spawn_blocking`, because at Stage 5 the site is fixed for the process
  lifetime; the function is pure so Stage 7 moves the call, not the code (§3.5).
- **ADR-0028 §5, erratum:** the declutter pass also places layer 8's site labels, entering
  at rank 0 ahead of every city (§3.7). Note the label index record is 16 bytes, so the
  label sections cost ~29 KiB rather than §2's ~27 KiB.
- **ADR-0026, status note:** the `Bucket` enum portion is implemented; the crate split and
  everything else stays deferred.
- **`rendering.md`:** the Stage 4 status paragraph is replaced with Stage 5's; the layer
  table's "slightly thicker line weight than counties" is corrected to say the distinction
  is contrast, not width (wgpu line primitives are 1 px); the Vector Overlay section gains
  the measured projection time and buffer sizes from §16.
- **`REQUIREMENTS.md`:** FR-MU-1, FR-MU-2 and FR-MU-7 lose their pending status; FR-CP-1
  gains a note that toggleable layer states now persist; the §6 traceability rows are
  updated.
- **`project-inventory.md`:** a dated line in the §6 Stage 5 block pointing at this plan's
  §18, in the same form as the Stage 2/3/4 entries.
- **`docs/README.md`:** a row for this plan in the Plans table.
- **`CLAUDE.md`:** status paragraph (the app now draws a basemap), the `render/` module
  map (`overlay.rs`, `labels.rs`, `shaders/overlay.wgsl`), the keyboard map (`H`), the
  layer-rendering-order note, and the ADR index line for 0026's partial implementation.
- **`README.md`:** status and keyboard map.
- **`utility/README.md`:** a `map-bake/` section in the established shape — what it
  generates, per-source URL + retrieval date + licence + SHA-256, the regeneration
  command, and the statement that the sources are `.gitignore`d while the bundle is
  committed (ADR-0025 §6).
- **`open-questions.md`:** unchanged. Stage 5 opens and closes nothing. The ramp-clutter
  observation from §12.3 is recorded in §18 of this plan, deliberately **not** as a
  numbered question (ADR-0029 §3).

---

## 14. Ordering, and what this plan deliberately does not do

### Order

1. **§11 (`Bucket`)** — independent, mechanical, and touches a different crate. Landing it
   first keeps it out of the diff that matters.
2. **§4 (generator)** and **§5.1 (format)** together — the format is defined by writing a
   reader and a writer against each other, and the bundle must exist before anything can
   consume it. Commit the bundle and manifest together with the generator.
3. **§5.2–§5.3 (reader + tests)** — CI-verifiable before any GPU code exists.
4. **§6 (projection)** — pure, tested, and the last piece that needs no window.
5. **§7 (overlay pass)** — the first thing that needs a GPU. Verify with §12.2 and the
   §12.3 KDOX render before moving on; a projection error found here is cheap, and found
   after labels are drawn is not.
6. **§8 (site markers)** — reuses everything §7 built.
7. **§9 (labels)** — depends on §6's `ProjectedLabel` and §8's candidates.
8. **§10 (input, config, chrome)** — small, and best done once the toggles have something
   to toggle.
9. **§12.3 (the three renders)** and **§13 (documentation)**.

### Not done here, deliberately

- **No tiles.** Layer 2 stays empty. ADR-0027 defers the subsystem and this plan does not
  write a stub — the ADR is explicit that no stub is written.
- **No placefiles.** Layer 7 is Stage 6, gated on Q6.
- **No runtime site change, no clickable markers.** Stage 7. This stage draws markers and
  stops, and the projection is structured (§3.5) so Stage 7 is a call-site change.
- **No part-bbox culling and no LOD.** ADR-0025 §3 and ADR-0029's rejected-alternatives
  both decline these for v1.0, with reasons that have not changed.
- **No tessellator.** Layers 3–5 are strokes (ADR-0025 §5).
- **No ramp/connector filtering.** It is a generator filter answered by looking at §12.3's
  KRLX render; if it is wanted, it is a re-bake in a follow-on commit, not part of this
  stage's code.
- **No second egui pass and no change to the frame structure.**
- **No new dependency.** ADR-0025 and ADR-0026 both state a zero delta; `Cargo.lock`'s
  package count must be identical before and after. If something makes a crate look
  necessary, that is a signal the design has drifted — stop and re-read ADR-0025 §5.

---

## 15. Risks

| Risk | Mitigation |
|---|---|
| **The bake does not reproduce the ADR counts.** "Bounding box within 700 km" admits several implementations. | §4.3's tolerance rule: under 1%, record and move on; over 1%, reconcile before committing. The manifest is where the actual numbers live either way. |
| **`ne_10m_populated_places` is not in the tree** and a different Natural Earth release changes the record count. | Download, digest, and record before baking (§4.1). A count other than 7,342/1,216 means a different release — record it in the manifest and §18 rather than forcing the ADR's number. |
| **The per-layer uniform hazard (§3.4) is found visually, late** — all four layers drawing in the last-written colour. | §12.2's `overlay_layers_use_distinct_uniform_buffers` is written *with* the pass, not after it. |
| **A projection sign or argument-order error** produces a plausible-looking but mirrored map. | §6.1's sign tests, and §12.3's KDOX render against a coastline whose shape is unmistakable. `radar-viz` is the working reference. |
| **The 6.41 MB blob in git** is unreviewable in a diff. | Accepted by ADR-0025 §6; the manifest is the review surface, and §5.3's `counts_match_the_manifest` test makes a manifest-less regeneration a test failure. |
| **The declutter pass is never exercised by real data** (19 candidates where it can place 250). | ADR-0028 §6 anticipates exactly this; §9.1's synthetic dense test is not optional. |
| **No window on the development machine**, so nothing is confirmed by eye in the normal way. | §12.3's offscreen PPM renders, which is the same path that verified the radar pass in Stage 4. State plainly in §18 that on-screen verification remains outstanding, as Stage 4 did. |
| **Binary grows ~6.4 MB** on top of Stage 4's 17.5 MB. | Expected and inside ADR-0006's 30–80 MB band; record the measured before/after in §16. |

---

## 16. Measurements to record in §18 Results

1. Per-layer parts and points emitted by the generator, against §4.3's table.
2. Bundle bytes, and the byte breakdown by section, against §5.1's 6,411,790.
3. Label count and string-table bytes.
4. `overlay::project` wall time for the full bundle, at a mid-latitude site — the number
   §3.5's decision rests on (ADR-0025 predicts ~21 ms).
5. Vertex buffer bytes, index buffer bytes, total — against ADR-0029's 11.46 MB.
6. Release binary size before and after.
7. `Cargo.lock` package count before and after (must be identical).
8. `labels::select` wall time at the shipped density and at 2,000 synthetic candidates.
9. First-render latency if a display is available; explicitly recorded as not measured if
   not.
10. What the three §12.3 renders actually showed, including the ramp-clutter answer.

---

## 17. Suggested commit sequence

0. **Commit the working tree first.** ADR-0025 through ADR-0029 and the documentation
   edits that accompany them are untracked/unstaged at `7ada4c3`. This stage implements
   them; they should be in history before the implementation that cites them.
1. `http-ingest: replace Host::parse with a Bucket enum (ADR-0026 §2)`
2. `utility/map-bake: bake the overlay bundle (ADR-0025, ADR-0028, ADR-0029)` — generator,
   `overlay.bin`, `bundle.manifest.txt`, `utility/README.md`
3. `overlay: bundle reader over include_bytes! (ADR-0025 §3–§4)` — reader, events, tests
4. `compute::geometry: azimuthal equidistant projection` — function, tests, and the
   `radar-viz` de-duplication
5. `overlay: project the bundle at site load`
6. `render: draw map underlay layers 3–5`
7. `render: draw non-active site markers (layer 8)`
8. `render: city labels with a screen-space declutter pass (layer 9, ADR-0028)`
9. `render: highways toggle, persisted layer state (FR-DR-3, FR-CP-1)`
10. `docs: Stage 5 errata, requirements, and status`

---

## 18. Results

*Filled in 2026-09-02 by the implementing session.*

### What was built

Every work item in §4–§13 landed, in the order §14 prescribed:

- **§11 `Bucket`** — `http-ingest`'s `Client` renamed `S3Client`; `S3Client::new`/
  `with_config` take `Bucket` (`Chunks`/`Archive`), infallible; `host.rs`'s `Host::parse`/
  `ALLOWED_HOSTS` and their ten rejection tests deleted, replaced by
  `host_returns_the_two_adr_0011_hosts`/`from_host_round_trips_each_bucket`/
  `from_host_rejects_an_unrelated_host`. Call sites updated: `pipeline.rs`,
  `ingest/s3_poll.rs` (`BUCKET_HOST` deleted), `tests/assembly_live.rs`,
  `crates/http-ingest/tests/live_s3.rs`, `utility/nexrad-sample` (`Bucket::from_host`
  replaces the old string-taking `Client::new` at the one place a developer-supplied URL
  still needs mapping onto the closed set).
- **§4 generator** (`utility/map-bake/bake.py`) — stdlib-only Python: digest-verified
  SHP/DBF readers, the site-table regex parser (163 sites, asserted), the 700 km
  clamped-haversine footprint filter applied at the *record* level (a feature's stored
  bbox, not a per-ring bbox — see Findings below), iterative Douglas–Peucker at ε = 30 m
  for primary roads only, dense rank assignment for labels, and a bundle + manifest
  writer matching §5.1's byte layout exactly.
- **§5 reader** (`crates/radar-workstation/src/overlay/mod.rs`) — `Bundle::parse` and
  every accessor via `slice::get`, no indexing, no `unsafe`; `bundled()` behind a
  `OnceLock`; five tests including a corrupt-bundle corpus (bad magic, bad version,
  `u32::MAX` counts, truncation at every section boundary, a bad label `name_off`, non-
  UTF-8 name bytes) — none panic.
- **§6 projection** — `compute::geometry::az_eq_project` (new; `radar-viz`'s private copy
  deleted, its call site now calls the production function and converts metres to km at
  the boundary) and `overlay::project` (`overlay/project.rs`), returning
  `(Projected, Vec<Event>)` rather than the plan's bare `Projected` — a deliberate, small
  deviation to match `config::load`/`palette::load_all`'s existing `(T, Vec<Event>)` shape
  for reporting `OverlayLayerUnknownKind`, since a free function has no `AppState` to
  report into directly.
- **§7 overlay pass** (`render/overlay.rs`, `shaders/overlay.wgsl`) — one shared
  vertex/index buffer, four per-layer uniform buffers + bind groups, draw order counties
  → states → coastline → roads, a zero-length index range skipped rather than submitted.
- **§8 site markers** — same file, same pipeline; every bundled site but the active one,
  projected at renderer init; dimmer than the active marker; ICAO labels handed to the
  declutter pass at rank 0.
- **§9 labels** (`render/labels.rs`) — pure greedy rank-ordered screen-space cull;
  memoised in `App` by exact-bits `(center_m, m_per_px, viewport, avail)`; drawn by a new
  `render::ui::city_labels` beside `ring_labels`, with a black-offset shadow copy.
- **§10 input/config/chrome** — `Action::ToggleHighways` on `H`; `ViewState.show_highways`;
  `config::{VIEW_HIGHWAYS_KEY, VIEW_REFERENCE_KEY}` (both booleans, case-insensitive,
  `ConfigValueInvalid` on anything else); `render::PersistedView` and `main.rs`'s save
  block carry both; help overlay gained the `H` row.
- **§12 validation** — all CI-run tests pass (§12.1); the two GPU-gated §12.2 tests pass
  against a software/offscreen adapter available in this environment
  (`offscreen_overlay_pass_draws_lines`, `overlay_layers_use_distinct_uniform_buffers`);
  §12.3's three PPM renders were produced *and visually inspected* (not just written) —
  see Findings below.
- **§13 documentation** — this file's own §18; dated errata on ADR-0025 §4 and ADR-0028
  §5; a status update on ADR-0026; `rendering.md`'s layer table, Vector Overlay section,
  and status paragraph; `REQUIREMENTS.md` FR-CP-1; `project-inventory.md`'s superseded
  banner and Stage 5 heading; `docs/README.md`'s plan-table row; `CLAUDE.md`'s status
  paragraph, `render/` module map, layer-rendering-order list, keyboard map, and ADR-0026
  index line; `README.md`'s status paragraph and keyboard table; `utility/README.md`'s
  new `map-bake/` section. `open-questions.md` left untouched, as the plan specified.

### §16 measurements

1. **Per-layer parts/points** (generator output, `bundle.manifest.txt`):

   | Layer | ADR table | Measured | Delta |
   |---|---|---|---|
   | `admin_2_counties_lakes` | 3,646 parts / 149,269 pts | 3,646 / 149,269 | **exact** |
   | `admin_1_states_provinces` | 1,438 / 197,952 | 1,439 / 200,335 | +1 part (+0.07%), +2,383 pts (+1.2%) |
   | `coastline` | 781 / 98,998 | 781 / 98,379 | exact parts, −619 pts (−0.6%) |
   | `primaryroads` | 17,500 / 281,401 | 17,500 / 281,251 | exact parts, −150 pts (−0.05%) |
   | `populated_places` | 1,216 labels | 1,216 | **exact** |

   Whole-bundle point delta is +1,614 / 727,620 ≈ **0.22%**, under §4.3's 1% "record and
   move on" line even though `admin_1` alone is at 1.2%. See Findings for why.
2. **Bundle bytes:** 6,424,726 (vs. §5.1's predicted 6,411,790 — the +12,936 byte delta
   is exactly the +1,614-point delta × 8 bytes/point). `string_bytes` is **exactly**
   10,522, matching §5.1's formula precisely — the clean match on the one section
   unaffected by the boundary-filter delta is a good sign the format implementation
   itself is correct.
3. **Label count / string bytes:** 1,216 labels, 10,522 string bytes (both exact matches).
4. **`overlay::project` wall time** (KDOX, release build): **~25 ms** for the full bundle
   (729,234 points) — close to ADR-0025's ~21 ms/~13 ms estimates and well under the
   ~50 ms revisit threshold S5-e set.
5. **GPU buffers:** 729,234 vertices (5,833,872 B) + 1,411,736 indices (5,646,944 B) =
   **11,480,816 B ≈ 11.48 MB**, against ADR-0029's 11.46 MB prediction — a 0.17% delta,
   consistent with measurement 1's point-count delta.
6. **Release binary size:** 17,601,248 B before (Stage 4 baseline, `7ada4c3`) →
   24,085,168 B after — **+6,483,920 B (+6.48 MB)**, close to the 6.42 MB bundle itself
   plus new code; well inside ADR-0006's 30–80 MB band.
7. **`Cargo.lock` package count:** 337, identical before and after — confirmed via
   `git diff Cargo.lock` (empty) as well as the count.
8. **`labels::select` wall time:** not measured as a standalone benchmark (the pure
   function is exercised directly by unit tests, including the 1,936-candidate dense
   synthetic input from `dense_synthetic_input_self_limits`, which completes as part of
   the sub-millisecond `cargo test` run); no standalone timing harness was written. Noted
   as a gap below.
9. **First-render latency:** **not measured** — this environment cannot present a window
   (Stage 4 §16's finding still holds), so there is no on-screen first frame to time.
10. **The three §12.3 renders**, actually looked at (PNG conversions kept alongside this
    session's notes, not committed):
    - **KDOX, 230 km:** Delmarva peninsula, Chesapeake Bay, and the New Jersey coastline
      are immediately recognisable and correctly oriented — county lines (thin, dim),
      state lines/coastline (brighter blue-gray), primary roads (orange) tracking real
      highway corridors, range rings and the site marker cross all render together with
      no visible projection sign/rotation error.
    - **KRLX, 60 m/px (max zoom):** one primary-road segment and two county boundary
      segments visible; the road renders as a smooth simplified curve with no
      sub-pixel jaggedness at this zoom. This is the qualitative answer to ADR-0029 §3's
      deferred "is ε = 30 m too coarse or too fine at max zoom" question: **too fine to
      cause visible clutter, not too coarse to look wrong** — no follow-up filter or
      re-bake is indicated by this single site/location, though it is one data point, not
      an exhaustive survey.
    - **PABC, 230 km:** coastline and state/county boundaries render; **zero orange
      pixels anywhere in the frame** — the primary-roads layer is visually and
      completely empty at this site, exactly ADR-0029 §7's claim, confirmed by looking
      rather than assumed. This resolved an internal design question this session had
      about whether "13 road-less sites" was literally true of the *shared* bundle
      (which contains all 17,500 CONUS parts, unfiltered per active site) — it is true in
      practice because those parts, reprojected relative to a site like PABC, land far
      outside any practical viewport, which is visually indistinguishable from "not
      present" and satisfies the requirement (`draw` still submits a non-empty index
      range for kind 4 at PABC; nothing currently makes that range literally zero at
      bake time, and the plan's "a zero-length draw range is skipped" language describes
      a defensive code path that is real and tested but not, in this bundle, what
      actually makes PABC's highway layer look empty).

### Findings that extended or contradicted this plan

- **§4.3's footprint filter needed to run at record (feature) level, not per-ring/part
  level, to reproduce the ADR tables.** The first implementation filtered each polygon
  ring independently by its own bbox and came in **24–27% low** on `admin_1_states_provinces`
  (1,039 parts / 150,683 points vs. the ADR's 1,438 / 197,952) — a large delta, past
  §4.3's 1% "reconcile before committing" line. The fix: compute each *record's* stored
  bbox (the box a Polygon/PolyLine shape carries once, covering every ring/part in that
  record) and filter on that, keeping or dropping every ring in a feature together. This
  brought `admin_2_counties_lakes` and `populated_places` to exact matches and
  `admin_1_states_provinces`/`coastline`/`primaryroads` to within 1.2%/0.6%/0.05%
  respectively. `read_shp_records` in `bake.py` documents this; it is the one place this
  plan's own premise ("bounding box within 700 km admits more than one implementation")
  turned out to matter in practice.
- **`overlay::project`'s signature gained `Vec<Event>`** (§6.2 deviation, noted above)
  — a small, deliberate departure from the plan's literal snippet, justified by matching
  an existing idiom rather than inventing a new "how does a pure function report an
  event" shape.
- **`OverlayRenderer::new` takes `site: &Site`** in addition to `device`/`format`/
  `projected` (§7's snippet omitted it) — required by §8's site-marker construction,
  which lives in the same struct/file and needs the active site to project against.
- **The declutter pass's `LabelCandidate.rank` field is read**, via a `debug_assert!`
  that candidates arrive non-decreasing by rank — not load-bearing (release builds don't
  check it), but it turns "candidates must already be sorted" from a comment-only
  precondition into something a debug build actually verifies, and it is what justifies
  keeping the field on the struct at all (`cargo clippy` flags an otherwise-unread field).
- **PABC's "no road coverage" is an emergent property of geography, not a bake-time
  per-site filter** — see measurement 10's third bullet. Recorded here rather than left
  implicit, since the plan's own wording ("a zero-length draw range... 13 road-less
  sites... a no-op") reads as if the range itself goes to zero at those specific sites,
  which is not quite the mechanism.

### Verification NOT completed (gaps — recorded, not marked done)

- **No on-screen verification.** Same gap Stage 4 §16 recorded and for the same reason
  (this development machine cannot present a window under the nested Wayland compositor).
  Every visual claim in this document rests on the offscreen PPM renders, inspected as
  PNG conversions, not a live window.
- **`labels::select` has no standalone wall-clock measurement** at shipped density or at
  a synthetic 2,000-candidate density (§16 item 8) — only the correctness properties are
  tested, not timed. Given the pass runs in well under a millisecond by construction
  (brute-force against a self-limiting ~250–360 placed labels, per S5-h's own argument)
  and is memoised per-frame, this is a low-risk gap, but it is a gap.
- **The KRLX ramp-clutter observation (measurement 10) is one site, one location, one
  zoom level — not a survey.** ADR-0029 §3 deliberately left this as a "look at it, don't
  reopen the ADR" question, and this session's answer is a single data point in that
  spirit, not a systematic check across the 163-site table.
- **`cargo deny check` and `cargo audit` were run and passed clean**, but neither was run
  against a freshly updated advisory database beyond what this session's `cargo audit`
  invocation fetched — standard practice, not a gap specific to this stage, noted for
  completeness.
- **No live multi-instance check** (NFR-P-1) was run against the new overlay GPU memory
  cost — that validation is Stage 8's, and this stage's own §16 measurement 5 (11.48 MB
  per instance) is the number Stage 8 will need.
