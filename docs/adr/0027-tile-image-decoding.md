# ADR-0027: Tile Image Decoding — Deferred to Post-v1.0, With the Answer Recorded

## Status
Accepted (2026-08-28)

Resolves [Q18](../open-questions.md), and closes [Q5](../open-questions.md) and
[Q7](../open-questions.md) by deferral — both were to be answered *with* the tile cache,
and there is now no tile cache in v1.0.

Defers the **implementation** of [ADR-0007](0007-tile-providers.md) (pluggable XYZ tile
providers) and [ADR-0026](0026-tile-http-boundary.md) (the tile HTTP boundary) to
post-v1.0. Neither is superseded and neither is reopened: both remain Accepted, their
designs stand as written, and this ADR records the measurements a future implementer
would otherwise have to re-derive. One part of ADR-0026 is taken up now regardless of the
deferral (§4).

Amends `REQUIREMENTS.md` §6: map imagery tiles and the on-disk tile cache move from
In Scope to Explicitly Deferred (Post-v1.0).

## Context

Q18 asked what decodes tile image bodies. It framed the choice as own-versus-take and
named a JPEG decoder as *"the largest single dependency and stability question in Stage
5."* It is that. But it bundles four decisions that separate cleanly:

1. Which formats v1.0 accepts — and therefore which providers are usable
2. Who writes the decoder
3. How a hostile tile is contained (BC-6, NFR-ST-2, the decompression-bomb bound)
4. Whether the tile subsystem is in v1.0 at all

Q15 and Q16 were both settled by measuring premises that turned out to be wrong. Q18's
premises were measured the same way, against the live providers on 2026-08-28, before
any option was ranked. The measurements are recorded here in full because their value
outlives this decision.

### Measurement 1 — image format is a per-tile property, not a per-provider one

This is the finding that reorders everything else. Four of the five USGS National Map
services serve **both** JPEG and PNG, interleaved by tile within a single service and a
single URL template:

| Service | JPEG | PNG | |
|---|---|---|---|
| `USGSTopo` | 20 | 0 | single-format *as sampled* |
| `USGSImageryOnly` | 15 | 5 | **mixed** |
| `USGSImageryTopo` | 3 | 3 | **mixed** |
| `USGSShadedReliefOnly` | 14 | 3 | **mixed** |
| `USGSHydroCached` | 3 | 3 | **mixed** |

This is ArcGIS's mixed-format cache behaviour: PNG32 where a tile carries transparency or
sparse content, JPEG where it is dense. The switch is not confined to blank or trivial
tiles — `USGSImageryOnly` over KDOX returns a 169 KB PNG at z9 and a 148 KB PNG at z10,
both full-content, with JPEG on either side of them in the zoom stack.

Q18's first candidate answer — *"own a minimal PNG decoder and restrict v1.0 to PNG
providers"* — therefore does not exist as an option. It is not that it rules out the USGS
default *as configured*; it is that no configuration of the USGS default yields PNG only.
Both decoders are mandatory for the ADR-0007 default provider, and format cannot be
pinned by provider, by template, or by configuration. It must be determined per response.

`USGSTopo` is recorded as JPEG-only across 20 samples, not as a JPEG-only *service*. A
cache rebuild could introduce PNG tiles into it without notice. Nothing may depend on
that row.

### Measurement 2 — the JPEG profile is narrow, and that narrowness is not a contract

All 56 JPEGs in the corpus, across four services and zoom levels 3–16, are byte-profile
identical:

```
SOF0 baseline · 8-bit precision · 3 components · 4:4:4 (no chroma subsampling)
no restart markers · single scan · 4 DHT · 2 DQT · 256×256 · APP0/JFIF only, no ICC
```

Zero progressive JPEGs. Q18's sizing — *"baseline DCT plus Huffman plus progressive
mode"* — overstates what is actually on the wire by a wide margin. A decoder for exactly
this profile is roughly 600 lines.

**This is the trap, and it is why the narrowness argues against owning rather than for
it.** These are the settings of a tile cache built years ago by an agency that has never
promised them to anyone. ADR-0008 owns the NEXRAD decoder because ICD 2620002 *is* a
contract: the format is specified, published, and stable, and a conformant decoder stays
conformant. A tile provider's encoder settings are a private implementation detail. A
cache rebuild that enables 4:2:0 subsampling or progressive encoding would silently break
the basemap — and the failure would land on an operator during weather, not in CI. The
measured profile is evidence about *today*, and a decoder scoped to it inherits a
dependency on someone else's unstated configuration.

That asymmetry — specified-and-stable versus observed-and-unpromised — is the reason
ADR-0008's own-the-boundary reasoning does not extend to tile codecs, and it should be
the first thing re-read if this decision is ever revisited.

### Measurement 3 — the PNG profile, and what it costs

17 PNGs: 15 are 8-bit RGBA non-interlaced with chunks `IHDR/pHYs/IDAT/IEND` (USGS); 2 are
8-bit palette with `PLTE` (OpenStreetMap, Carto). No interlacing, no 16-bit, no `iCCP`.

Either shape requires DEFLATE. [ADR-0015](0015-bzip2.md) already decided this class of
question in the other direction: BZ2 was delegated to a third party *specifically*
because a general-purpose compression algorithm is not what this project should
hand-roll, even though it sits on the attacker-influenced chunk path. DEFLATE is the same
kind of thing. Owning PNG end-to-end would contradict ADR-0015 on its own reasoning.

### Measurement 4 — neither cost nor containment is the hard part

Decoding is free. Using `png` 0.18 + `jpeg-decoder` 0.3 over the 73-tile corpus:

```
mean 0.32 ms · median 0.31 ms · max 0.70 ms   (per 256×256 tile, to RGBA8)
```

A 40-tile viewport is ~13 ms of CPU, off-thread. Performance plays no part in this
decision, in either direction.

The decompression-bomb bound is trivial, because **a tile has exactly one legal size.**
A hand-built 545-byte PNG declaring 30000×30000 RGBA — 3.4 GB decoded — is rejected in
microseconds by gating on the declared dimensions at the header, before any allocation:

```
REJECT bomb.png    BadDims(30000, 30000)
```

Both crates expose header-first reads (`read_info()`) ahead of frame allocation, so this
is roughly five lines and it is a far stronger bound than any expansion-ratio heuristic.

Two smaller findings, recorded so they are not rediscovered: out-of-extent tiles return a
clean `HTTP/1.1 404` with a 572-byte HTML body, so the missing-tile path is a status-code
decision and needs no content sniffing; and `png` returns palette images as `Indexed`
unless `Transformations::EXPAND` is set, without which OSM and Carto tiles fail to decode
at all.

### Measurement 5 — the dependency cost, resolved and vendored

| Candidate | Packages compiled | LOC | `unsafe` blocks |
|---|---|---|---|
| `png` 0.18 (default features) | 9 | ~37k | 51 — `png` itself **0**; all of it in `crc32fast` (15), `flate2` (32), `simd-adler32` (36) |
| `jpeg-decoder` 0.3 (`default-features = false`) | 1 | 5,484 | 16, confined to SIMD IDCT / colour-convert |
| `zune-jpeg` 0.5 | 2 | 9,737 | 79 |
| **`png` + `jpeg-decoder`** | **11** | ~43k | ~67 |

`png`'s `zlib-rs` backend — 430 `unsafe` blocks — is an **opt-in feature, not a default**;
the 11-package figure is with it off. `png` pulls `miniz_oxide` 0.8 directly while
`flate2` pulls 0.9, a duplicate version; `deny.toml` sets `multiple-versions = "warn"`, so
this warns rather than fails.

### What the tile layer is actually worth

[ADR-0007](0007-tile-providers.md) already states the answer: imagery is off by default,
and *"the application is fully functional without imagery."*
[ADR-0025](0025-bundled-overlay-geometry.md) has since made the vector basemap
substantially better than it was when that sentence was written — counties, states and
provinces, coastline, and TIGER primary roads, baked and projected per site load.

So the question is what compositing layer 2 adds *on top of* a complete vector reference
map. It is not "where am I," "what county is this," or "what road is that" — those are
layers 3, 4, 5, and 8, all of which ship in v1.0. It is terrain: ridges, valleys, and
shadowing behind a storm, and aerial imagery at close zoom. That is genuine analytical
content for an operator reasoning about upslope enhancement or a line crossing the
Appalachians. It is also the *only* thing on the list that requires putting a new
untrusted-input parser on a network path, for a layer that is off until the user turns it
on.

## Decision

### 1. The tile subsystem is deferred to post-v1.0

v1.0 ships a **vector-only basemap**. Compositing layer 2 (terrain imagery) is
unpopulated; layers 1 and 3–10 are unaffected. `REQUIREMENTS.md` §6 moves map imagery
tiles and the on-disk tile cache to Explicitly Deferred, and FR-DA-6, FR-MU-4, FR-MU-5,
and FR-MU-6 are marked deferred rather than open.

This is a scope decision under Restraint is a Feature, taken because the measurements
above establish that the tile layer is the one v1.0 item whose cost is a new
untrusted-input parser on a network path and whose benefit is an optional, off-by-default
decoration over an already-complete reference map. It is not a judgement that the layer
lacks value — see §2, which exists precisely because it has value.

### 2. When the subsystem is built, this is the answer

Recorded now, while the measurements are fresh, so that returning to this costs a
re-validation rather than a redesign.

**Take `png` 0.18 and `jpeg-decoder` 0.3** (`default-features = false`, which drops
`rayon` and six crates). Own the policy around them; do not own the codecs.

The reasoning is the deliberate inverse of ADR-0008's, and is stated here so the
inconsistency is understood as intentional: **this project owns a boundary when the
format is fixed by a contract it can read and the dependency cost is large.** Neither
holds for tile codecs. The JPEG profile is a provider's private choice (Measurement 2),
and the cost is 11 small pure-Rust packages rather than the 144 that motivated ADR-0014.
`png` and `jpeg-decoder` are both `image-rs` crates, both continuously fuzzed by
OSS-Fuzz, and `png` itself contains no `unsafe` at all — a better-audited position than a
first-party decoder would reach in any realistic amount of effort.

The containment is the load-bearing half, and is where the project's own work goes:

| Sub-decision | Choice |
|---|---|
| **Accepted formats** | PNG and JPEG, dispatched per response on **magic bytes**, never on `Content-Type` (Measurement 1). Anything else is a typed error and a missing tile. |
| **Dimension gate** | Reject at the header, before allocation, unless the declared dimensions equal the provider's configured tile size (256×256 default). This *is* the decompression-bomb bound. |
| **Body cap** | ADR-0026's 4 MB stands unchanged; the largest tile measured is 169 KB. |
| **Colour handling** | PNG: `Transformations::EXPAND` for palette and `tRNS` → RGBA8. JPEG: `RGB24` and `L8` → RGBA8. CMYK and YCCK rejected. |
| **Panic containment** | Decode on `spawn_blocking` inside `catch_unwind`. `panic = "unwind"` is already deliberate for this in `Cargo.toml`, and `render/mod.rs` already establishes the in-tree pattern for GPU bring-up. A panicking tile becomes a missing tile and a status-bar line. |
| **Blast radius** | A decoded tile reaches the GPU texture atlas and the disk cache and nothing else. It never enters `AppState`, the radar path, or the compute layer. |
| **`zlib-rs`** | Not enabled. Named here so it is not turned on later for throughput that Measurement 4 shows is irrelevant. |
| **Fuzzing** | The 73-tile corpus plus a seeded mutator, on stable `cargo test`, mirroring the pattern `http-ingest` established and `nexrad-decoder` still lacks. |

Q5 (cache sharing) and Q7 (cache size and eviction) return with the subsystem, unchanged
and unanswered.

### 3. No code stub is written

There will be **no** `crates/tile-fetch`, no `TileClient` returning a not-implemented
error, and no tile-related configuration key that parses but does nothing.

A stub would be a net liability. It is dead code on the government and defense approval
surface (NFR-SEC-6), where a reviewer finding a tile-fetching crate must then establish
that it cannot fetch anything. It weakens ADR-0026's strongest claim — that BC-1 is
auditable in one sitting — by adding a destination that exists in the source but not in
fact; with tiles deferred, the complete set of destinations is *two S3 buckets named in
an enum, and Stage 6's placefile URLs from config*, which is a shorter and stronger
statement. And a configuration key that accepts a value and silently does nothing is
precisely the failure NFR-ST-3 prohibits.

What preserves the design is ADR-0026 and this ADR, both of which are specific down to
the API sketch, the sub-decision table, and the measurements. Documentation is the
correct artifact for a deferred design; code is not.

### 4. ADR-0026's `Bucket` enum is taken up now; its crate split is not

ADR-0026 specified three crates: the `http-ingest` engine, `s3-fetch`, and `tile-fetch`.
The split's entire motivation was hosting a second policy crate. With `tile-fetch`
deferred, splitting now would produce a two-crate structure with one consumer —
speculative generality, and a refactor of working, tested code for no present benefit.
ADR-0026's own central finding, that *the seam it splits on already existed inside the
crate*, is what makes deferring the split safe rather than expensive: it will be no
harder later than it is today.

**Taken up now, inside the existing crate:** replacing `Host::parse(&str)` with
`S3Client::new(bucket: Bucket)` over a two-variant enum, so that no string reaches host
selection and no method accepts a hostname. This is a compiler-checked guarantee that the
radar path cannot be pointed at another host, it is worth having on its own merits
independently of tiles, and it is a few hours of work rather than a three-crate refactor.

**Deferred with the subsystem:** the engine/policy split, `crates/tile-fetch`, the
`UrlTemplate` parser, `ETag` / `If-None-Match` support, the N-worker concurrency model,
and the tile `ClientConfig` budget. All remain as specified in ADR-0026.

## Consequences

- **Zero dependency delta for v1.0.** The production graph stays at 78 lockfile packages.
  Combined with ADR-0025 (five crates removed, zero added) and ADR-0026 (zero), Stage 5's
  total dependency delta is −5.
- **No new untrusted-input parser ships in v1.0.** The complete set of parsers on a
  network path remains the NEXRAD decoder, the HTTP/1.1 response framer, and the S3
  ListObjectsV2 XML reader — all three of which already exist and are tested.
- **BC-1's audit gets shorter.** Two S3 buckets and Stage 6's configured placefile URLs.
- **Three open questions close at once** — Q18 by decision, Q5 and Q7 by deferral —
  leaving Stage 5 as overlay work only, with no blocking questions in front of it.
- **The radar path's host guarantee still strengthens** (§4), which was the part of
  ADR-0026 with value independent of tiles.
- **The v1.0 basemap is vector-only, and that is a real reduction.** An operator gets
  counties, states, coastline, primary roads, site markers, and range rings, but no
  terrain and no imagery. For terrain-influenced storm interpretation this is a genuine
  loss, accepted deliberately and recorded as the principal cost of this ADR.
- **ADR-0007 and ADR-0026 are Accepted but unimplemented**, which is a state no other ADR
  in this project is in. Both need a status note pointing here, or a future reader will
  reasonably assume the code exists.
- **The measurements have a shelf life.** Provider formats, the JPEG profile, and crate
  versions were true on 2026-08-28 and are evidence, not guarantees. §2 must be
  re-validated — not merely re-read — before it is implemented. The corpus is kept in the
  repository so re-validation is a diff rather than a re-derivation.
- **Deferral is not free either.** Returning to this means re-entering a design context
  that will be months cold. §2 and ADR-0026 are the mitigation, and they are unusually
  complete for exactly that reason.

## Rejected alternatives

- **Own a minimal PNG decoder and restrict v1.0 to PNG providers** (Q18's first candidate).
  Not available: four of five USGS services serve both formats from one template, and no
  configuration yields PNG only (Measurement 1). The option is unavailable rather than
  merely costly.
- **Own both decoders.** Requires either owning DEFLATE — contradicting ADR-0015 on its
  own reasoning — or taking `miniz_oxide` anyway, and puts the project's most
  failure-prone code on an untrusted network path in service of an off-by-default
  decorative layer. Q18 anticipated this conclusion and it survives measurement.
- **Own the JPEG decoder, take `png`.** Intellectually the most consistent option: own
  the format-specific parsing (Huffman + DCT), delegate the general-purpose compression
  algorithm (DEFLATE), which is exactly the ADR-0008 / ADR-0015 split applied to images.
  At the measured profile it is only ~600 lines. Rejected on Measurement 2: the profile is
  observed, not specified, and a decoder scoped to it takes a silent dependency on a
  provider's unstated encoder configuration. This is the closest call in this ADR.
- **Own the PNG container only, over `miniz_oxide`, and take `jpeg-decoder`.** Four
  packages instead of eleven for roughly 250 owned lines (chunk walk, five unfilter types,
  `PLTE`/`tRNS` expansion, reject interlace). Genuinely in this project's character and
  the strongest of the partial-ownership options. Rejected because it accepts owned-parser
  risk on an untrusted path to delete seven small, pure-Rust, well-fuzzed crates — a poor
  trade at this project's stated priorities, though a defensible one. Worth reconsidering
  only if the package count is later judged unacceptable.
- **`png` + `zune-jpeg` instead of `jpeg-decoder`.** Nearly twice the LOC, five times the
  `unsafe`, and a single maintainer — the exact profile ADR-0025 and
  `dependency-inventory.md` E-07 rejected for `shapefile` and `dbase`. `jpeg-decoder`
  dominates it on every axis this project weighs; `zune-jpeg` wins only on throughput,
  which Measurement 4 shows is irrelevant at 0.32 ms per tile.
- **Ship the tile subsystem in v1.0 with the §2 answer.** Fully viable — §2 is a complete,
  measured, defensible design, and this was the recommendation before scope was
  reconsidered. Rejected because it spends the +11 packages, a new untrusted-input parser,
  and the Q5/Q7 cache design on the only v1.0 item that is optional, off by default, and
  layered beneath a reference map that is already complete without it. Deferring costs a
  re-validation later; shipping costs all of that now.
- **Decode on the GPU / defer to `wgpu` texture upload** (Q18's fourth candidate). Not
  viable for either format; `wgpu` ingests neither JPEG nor PNG. Recorded as considered
  and dismissed on the facts.

## Open questions this ADR does not answer

<!-- erratum 2026-08-30: Q19 is resolved by [ADR-0028](0028-city-labels.md). Two of the
premises stated below did not survive measurement and are corrected there rather than
edited out here. (1) "a format extension **and a version bump**" — ADR-0025 was accepted
but never implemented, so labels are designed into format version 1 and there is nothing
to migrate. (2) The implied layer-9/layer-10 ordering conflict does not exist:
compositing layers 1–8 are all wgpu and layer 10 is egui, so egui's lowest order *is*
slot 9. The source this ADR called obvious (Natural Earth `populated_places`) is the one
the density measurement contradicts — it is adopted anyway, explicitly as provisional,
because a working end-to-end path is worth more than the source at this stage. Q20 remains
open. [Superseded 2026-09-02: Q20 is resolved by ADR-0029 — see the erratum on the Q20
bullet below.] -->

Both belong to Stage 5's overlay work and are unrelated to tiles. They surfaced while
scoping the vector-only basemap and are now tracked as **[Q19](../open-questions.md)** and
**[Q20](../open-questions.md)**, raised by this ADR.

- **[Q19](../open-questions.md) — city labels have no geometry source, and the bundle
  cannot represent one.** Compositing layer 9 (zoom-dependent city labels) is specified in
  `rendering.md`, and ADR-0025's Context notes that place names need DBF attributes — but
  ADR-0025's source table has no populated-places row, and its §3 bundle format stores
  only `points × { lon i32, lat i32 }` with no string storage anywhere. So this needs a
  format extension and a version bump, not just a source. With imagery deferred, layer 9
  is a larger share of what the basemap conveys.
- **[Q20](../open-questions.md) — TIGER Primary Roads is unmeasured.** ADR-0025 records it
  as not yet in the tree, expected to be the largest single layer, and the one most likely
  to want a bake-time simplification tolerance. A measurement before it is a decision.
  <!-- erratum 2026-09-02: resolved by [ADR-0029](0029-primary-roads-simplification.md).
  Both expectations here held — it is the largest layer by 8× and it does want a tolerance
  (Douglas–Peucker, ε = 30 m). What this ADR did not anticipate is *which* budget binds:
  not binary size, and not the projection pass, but GPU buffers — 57.29 MB unsimplified
  against a 128 MB per-instance target. -->
