# ADR-0028: City Labels — A Provisional Source, a Bundle String Table, and a Screen-Space Declutter Pass

## Status
Accepted (2026-08-30)

Resolves [Q19](../open-questions.md). Amends [ADR-0025](0025-bundled-overlay-geometry.md)
§1 (source table), §2 (footprint filter), §3 (bundle format), and §4 (runtime path).
Adds FR-MU-7. Supersedes nothing.

## Context

`rendering.md` and FR-DR-3 both place **city labels at compositing layer 9**, sourced
from "bundled label data." ADR-0025's source table has no populated-places row, so layer
9 has had no data source at all. [ADR-0027](0027-tile-image-decoding.md) raised this as a
formal question when it deferred the tile subsystem out of v1.0 and, in doing so, made
the vector basemap the whole basemap.

Q19 asked four things together: the source and its licence, the bundle format extension,
the zoom-threshold policy, and which renderer draws the text. **Three of the four
collapsed under measurement, and the fourth — the source — turned out to be the one whose
presumed answer the numbers contradict.**

### The measurements

All distances are great-circle against the committed 163-site table in
`crates/radar-workstation/src/sites_generated.rs`, so they cannot drift from the site
list, on the same principle as ADR-0025 §2.

**Measurement 1 — source density.** Q19 named Natural Earth 10m `populated_places` as
the obvious candidate, "consistent with ADR-0025's other three Natural Earth layers."
It was downloaded and measured rather than assumed:

| Site | NE 10m | Census Gazetteer | GeoNames cities1000 |
|---|---|---|---|
| KDOX ≤230 km / ≤100 km / ≤50 km | **19 / 2 / 1** | 1,991 / 312 / 83 | 1,742 / 148 / 42 |
| KTLX | 16 / 4 / 3 | 787 / 170 / 58 | 258 / 77 / 34 |
| KLOT | 28 / 8 / 3 | 1,586 / 491 / 209 | 956 / 418 / 227 |
| KGLD | 7 / 1 / 1 | 267 / 44 / 8 | 74 / 11 / 3 |
| KBIS | 7 / 1 / 1 | 292 / 50 / 11 | 65 / 14 / 3 |

Natural Earth is a small-scale **world** cartography set. Nineteen labels in a full
230 km PPI and two inside 100 km is far below what the operational question — *what town
is that hook echo about to hit* — needs. The consistency argument is exactly what makes
this look obvious, and it is what fails.

**Measurement 2 — the alternatives' costs.**

| | NE 10m populated_places | Census Gazetteer places | GeoNames cities1000 |
|---|---|---|---|
| Records | 7,342 | 32,329 | 170,860 |
| Kept within 700 km of a site | 1,216 | 32,319 | 25,016 |
| Ranking field | `SCALERANK`, `LABELRANK`, `NATSCALE`, `POP_MAX`, `MIN_ZOOM` | **none** | population |
| Coverage | global | US + PR only | global |
| Licence | **public domain** | **public domain** (17 USC §105) | **CC BY 4.0 — attribution required** |
| Bundle add | **~27 KiB** | ~723 KiB | ~641 KiB |

Census leaves **five sites with zero labels** — LPLA (Lajes), PGUA (Guam), RKJK, RKSG,
RODN — the exact overseas DoD sites ADR-0025 §2 covers "for free" precisely *because*
Natural Earth is global. Border sites lose a large share: KATX 56% of in-range places
non-US, KCBW 56%, KBUF 54%, KCXX 45%, KBRO 31%, KEPZ 28%, KTYX 28%.

Census also has no ranking field, and the obvious repair does not work: joining the
gazetteer to Census `sub-est2024` population estimates on state+place FIPS covers
19,471 of 32,329 rows (60.2%) and **3 of 12,523 CDPs (0.0%)** — the unincorporated rural
communities a storm operator most needs named. Ranking would fall back to a proxy chosen
by eye, which is the failure mode Q20 exists to avoid.

**Measurement 3 — egui text cost.** Against the pinned `egui 0.36.1`, `run_ui` +
`tessellate`, 1920×1080, `FontId::proportional(12.0)`, 60 iterations after warm-up:

```
 labels    static ms   panning ms       tris
     50        0.011        0.011       1158
    200        0.061        0.061       4958
    500        0.108        0.101      12758
   1000        0.203        0.213      25706
   2000        0.410        0.419      53622
```

Against a 16.7 ms frame budget this is noise, and **panning costs the same as static** —
egui's galley cache keys on text and font, not position. Text rendering is not a
constraint at any plausible label count.

**Measurement 4 — declutter yield.** A greedy rank-ordered screen-space collision cull
at KDOX, 1920×1080, run against the *dense* source (GeoNames) because Natural Earth is
too sparse to exercise it:

| View radius | candidates | actually placed |
|---|---|---|
| 460 km | 6,519 | 360 |
| 230 km | 2,997 | 254 |
| 100 km | 1,054 | 173 |
| 50 km | 84 | 52 |

(Label box approximated at 6.6 px/char, 14 px line, 3 px pad — sufficient to establish
the magnitude, not a final layout constant.) The pass self-limits output to ~250–360
labels regardless of source density, and a naive O(n²) implementation costs 6.3 ms at
230 km / 19.1 ms at 460 km in Python — sub-millisecond in Rust with a uniform grid.

**Measurement 5 — name lengths.** Maximum UTF-8 name bytes: **NE 25, Census 57,
GeoNames 97.** Across NE's 1,216 kept records: 10,522 name bytes total, 49 non-ASCII
(accented Mexican and Québec placenames).

### What the measurements settle

- **The renderer sub-question is not architecturally weighty.** Q19 worried that layer 9
  sits below layer 10 while text is egui's job, so egui would either violate the ordering
  or need two passes. Checked against `render/mod.rs`: compositing layers **1–8 are all
  wgpu** (pass 1) and layer 10 is egui (pass 2). There is no wgpu layer above 9, so
  egui's *lowest* order is exactly slot 9. `render/ui.rs::ring_labels` already does this
  for range-ring labels — world position → `view::world_to_screen` → `layer_painter(
  LayerId::new(Order::Background, …))`, inside the single egui pass. City labels are that
  function with a different point set.
- **The format extension is not a version bump.** ADR-0025 is accepted but
  **unimplemented** — there is no `utility/map-bake/`, no `crates/radar-workstation/src/
  overlay/`, and no bundle blob in the tree. Labels are designed into version 1 of the
  format, not migrated into version 2.
- **The zoom-threshold policy is subsumed.** A rank-ordered greedy cull *is* the zoom
  policy: labels appear as screen space opens up. A hard threshold is a cruder version of
  a pass that is needed regardless.
- **The source is genuinely open**, and is the only sub-question measurement does not
  answer on its own — because it trades density against licence and coverage, which is a
  judgement, not a number.

## Decision

### 1. Source: Natural Earth 10m `populated_places` — chosen for v1.0, explicitly provisional

Public domain and global, matching ADR-0025's other three layers, at ~27 KiB of bundle.
Its density is known to be low and is accepted as a v1.0 limitation (§6 below).

The reasoning is that **the plumbing is worth more than the source right now.** Layer 9
has never existed in any form; a working end-to-end path — bake, bundle, project, select,
draw — is what turns a denser source into a regeneration rather than a project. The two
denser candidates each carry a cost that is not worth paying to reach a first
implementation: GeoNames adds the workspace's only non-public-domain data dependency
inside a government/defense approval surface (Principle 4), and Census gives up global
coverage that every other overlay layer has, blanking five sites outright.

### 2. Rank normalisation at bake time — the mechanism that keeps the source swappable

The runtime sees exactly four things per label: `{ lon, lat, rank, name }`. No
`SCALERANK`, no `POP_MAX`, no country code, no feature class.

`rank` is a dense `u16`, ascending, `0` = draw first, assigned at bake time by sorting on
whichever importance signal the source offers. The runtime never interprets it beyond
ordering. Natural Earth's `SCALERANK`/`POP_MAX`, GeoNames' population, and a Census
land-area proxy all collapse into the same field, so **swapping the source is a generator
change and a bundle regeneration — no format change, no runtime change, no reversal of
this ADR.** That property is the point of this decision, not a side effect of it.

### 3. Bundle format extension (ADR-0025 §3)

`magic` stays `RWMOVL01` and `version` stays **1** — nothing has been baked, so this is
the first version of the format, not a second one. A layer `kind` discriminant for
labelled points is added, along with two new sections:

```
label index   label_count × { lon i32, lat i32, rank u16,
                              name_off u32, name_len u16 }
strings       one contiguous UTF-8 blob
```

Labels get their **own section** rather than being modelled as one-point parts in the
existing part index: a part carries a 24-byte bounding box to describe a point that is
already its own bounding box.

**This amends ADR-0025 §3's stated invariant, deliberately and on the record.** §3 says
"all cross-references are element counts, not byte offsets." A packed string table needs
byte offsets; there is no way around it. The invariant's *purpose* is unchanged, because
`slice::get(off .. off + len)` is still a checked range that yields `None` rather than
panicking — which is exactly what §4 already promises. The letter changes; the property
that matters does not. It is recorded here rather than left to be discovered, per
`project-inventory.md` §7 on design drift.

The alternative that preserves §3 literally — a fixed-width name field — is rejected
because it is the one choice that would lock the format to the provisional source:
Measurement 5 shows a 32-byte field fits Natural Earth and truncates both denser
candidates.

Names are stored as UTF-8 `NAME`, not `NAMEASCII`: 49 of 1,216 kept records are
non-ASCII, and mangling a Québec or Mexican placename to save four bytes serves nobody.

### 4. Bake-time filter (ADR-0025 §2)

Labels use the same 700 km site-footprint filter as the geometry layers, read from the
same committed site table, keeping 1,216 of 7,342 records. No population or `SCALERANK`
floor is applied at bake time: at this density the whole set is worth keeping, and the
declutter pass is what decides what is drawn. A bake-time floor becomes worth revisiting
only with a denser source.

### 5. Screen-space declutter — the one genuinely new piece of machinery

A greedy, rank-ordered, screen-space collision cull selects which labels are drawn.

- **Render-loop owned, next to `ViewState`, never in `AppState`** (ADR-0018). It is a
  function of the view, and the view is not shared state.
- **Pure**: `(labels, &ViewState, viewport) -> Vec<PlacedLabel>`, unit-tested without a
  window — the pattern `view.rs`, `input.rs`, `adapter.rs`, and `time.rs` already
  establish in `render/`.
- **FR-NI-4 applies.** The named spatial-stability test that asserts a synthetic sequence
  of state updates leaves `ViewState` unchanged extends to label selection: a new scan,
  a product switch, or a sweep switch must not change which labels are placed.
- Recomputed when the view or viewport changes, memoised otherwise. At the measured cost
  nothing more elaborate is warranted.
- **Candidates are culled against `ctx.available_rect()`** before placement, so labels
  never paint over the status bar or legend. This is the correct fix for the real problem
  (labels hidden under chrome) and it incidentally sidesteps the intra-egui z-ordering
  question, since egui panels share `Order::Background` and ordering within one order is
  insertion-dependent.

Module placement follows the existing precedent exactly: `render/labels.rs` owns bundle
access and selection, `render/ui.rs` draws — the same split by which `reference.rs`
computes `ring_labels()` and `ui.rs` paints them.

### 6. The measured sparsity is recorded as a known limitation

**19 labels in a KDOX 230 km PPI; 2 within 100 km.** Written down here so that a future
reader who sees two labels on screen does not go looking for a bug in the declutter pass
that is not there. The v1.0 basemap names major population centres, not every settlement.

Two consequences follow honestly from this and are accepted:

- The declutter pass will essentially never reject a candidate at Natural Earth density —
  19 candidates into a view that comfortably places ~250. It is therefore built ahead of
  the data that exercises it, and **must be unit-tested with synthetic dense input**,
  which is cheap because the function is pure. Without that test the pass compiles but is
  never exercised.
- The path back is deliberately short: choose a source, re-run the generator, review the
  manifest diff. Measurement 2 is the table to re-read, and Measurement 1 is why.

## Consequences

- **No new dependencies.** No parser, no font work, no crate. The bundle grows ~27 KiB;
  egui already renders text and already renders world-projected text in `ring_labels`.
- **Layer 9 gains a data source, and FR-MU-7 gains a requirement.** Before this ADR, city
  labels appeared only in FR-DR-3's compositing list and had no functional requirement at
  all — the requirement set and the compositing order disagreed.
- **ADR-0025 §3's element-counts-only invariant is narrowed** to the geometry sections,
  with the string table explicitly exempted and the checked-access property preserved.
- **The source is the one part of this decision expected to change.** Everything else —
  format, filter, rank field, declutter, renderer — is designed to survive that change
  untouched. If it does not, this ADR was wrong about the seam, and that is the thing to
  re-examine first.
- **A future denser source will make the declutter pass load-bearing rather than
  decorative,** and its constants (font metrics, padding, the ~250-label practical yield)
  will want re-tuning against a real display at that point. They are not tuned against
  one now; Measurement 4's box approximation is explicitly a magnitude, not a layout.

## Rejected alternatives

- **GeoNames `cities1000` now.** The best data of the three — 170,860 records, global,
  with a real population field, covering the overseas DoD sites (RKJK 255 labels in
  230 km, RODN 40, PGUA 32, LPLA 48) and the border sites Census abandons. Rejected for
  v1.0 on licence: CC BY 4.0 attribution is easy to satisfy but makes this the only
  non-public-domain data in the binary, inside the approval surface Principle 4 exists to
  protect. Revisit deliberately, not by default.
- **Census Gazetteer places now.** Public domain, denser than Natural Earth, and
  lineage-consistent with FR-MU-1's TIGER wording. Rejected because it blanks five sites
  entirely, strips 28–56% of labels from a dozen border sites, and has no honest ranking
  field — the `sub-est` join reaches 0.0% of CDPs.
- **A hard zoom threshold instead of a declutter pass** (the policy Q19 asked to decide).
  You need the cull regardless once any denser source lands, and a threshold is a worse
  version of what the cull already does. Building the threshold first means building the
  cull twice.
- **Fixed-width name field** (`[u8; 32]`), preserving ADR-0025 §3 literally. Fits Natural
  Earth's 25-byte maximum and truncates Census (57) and GeoNames (97) — it would encode
  the provisional source into the format, which is the one outcome this ADR is shaped to
  avoid.
- **Drawing labels through wgpu** to keep every overlay layer a `LineList`. Requires a
  glyph atlas, shaping, and a text pipeline the project does not have and does not need:
  egui already has all three, and Measurement 3 says the cost is 0.1 ms at 500 labels.
  `rendering.md` already routes placefile text and ring labels through egui for the same
  reason.
- **Skipping layer 9 for v1.0** the way ADR-0027 skipped tiles. Defensible, and the
  precedent is fresh. Rejected because the two situations are not alike: deferring tiles
  removed a new untrusted-input parser on a network path, whereas layer 9 adds ~27 KiB of
  data the project generates itself, no parser, no network, and no dependency — and,
  with tiles gone, the vector basemap is now the only thing orienting the operator.

## Erratum (added 2026-09-02, Stage 5 / S5-g)

§5's declutter pass also places layer 8's radar site labels (ICAO identifiers), entering
the pass first, at rank 0, ahead of every city candidate. This was not in §5's original
scope — that section describes only city labels — but two independent passes for layer 8
and layer 9 would let a city name land on a radar site's identifier, the one label
collision that matters most, since the site markers are what the operator navigates by.
The pass's signature and purity are unchanged (`select(candidates, view, viewport, avail)
-> Vec<PlacedLabel>`); `rank` was already defined as "a dense ascending ordering the
runtime never interprets beyond ordering," which is exactly what letting a second
candidate source enter at a reserved low rank uses. `render::labels` (not
`render::overlay`) owns the pass; `render::overlay::OverlayRenderer` builds the
site-label candidates (world position + ICAO text) at renderer init, alongside the site
marker geometry, and hands them to the same `select` call the city candidates go through.

Also worth recording precisely, since it changes a number in §2 and §4: the label index
record specified there is 16 bytes (`lon i32, lat i32, rank u16, name_off u32,
name_len u16` — a variable-length name via the shared string table, not the fixed
`[u8; 32]` this ADR's Decision section also considered and rejected in the same
paragraph). At 1,216 labels that is ~19 KiB, not the ~16.6 KiB a 14-byte record would
cost — Measurement 4's number was computed against the wrong record size. The label
sections (index + string table) cost ~29 KiB total against §2's ~27 KiB estimate. Neither
delta changes any conclusion in this ADR.
