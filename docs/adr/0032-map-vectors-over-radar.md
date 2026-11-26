# ADR-0032: Map Vectors Composite Over Radar

## Status
Accepted (2026-11-24)

Part 1 of `design/usability-map-over-radar-and-backfill.md`. Amends FR-DR-3's
compositing order and the Stage 5 colour rule (`docs/plans/stage-5-map-underlays.md`
§7, S5-W4) that the vector layers "sit *under* the radar image and must not compete
with it". ADR-0025, ADR-0027 and ADR-0029 cite layer numbers that this renumbers; their
text is corrected in place.

## Context

Stage 5 drew county, state/coastline and highway lines before the radar image, so
FR-DR-3 put them at layers 3–5 and radar at layer 6. The radar palette LUT gives every
cell that carries data an opaque colour (only no-data is transparent, FR-DR-4). Inside
an echo, then, the lines disappear completely. That is the area the operator is
looking at during severe weather, and it is where they most need to know which county a
storm is in and which road it is crossing.

The Stage 5 rule that the underlay "must not compete" with radar assumed the lines
were background reference for empty areas. In practice the operator uses them to place
an echo. GR2Analyst, the reference application, draws its map lines over the radar
data for the same reason.

## Decision

### 1. The new order

FR-DR-3 becomes:

| Order | Layer |
|---|---|
| 1 | Background |
| 2 | Terrain imagery tiles (empty in v1.0, ADR-0027) |
| 3 | Radar data |
| 4 | County boundaries |
| 5 | State / country boundaries and coastline |
| 6 | Major highways (toggleable) |
| 6a | Reference geometry: range rings, spokes, active-site marker |
| 7 | Placefile overlays |
| 8 | Radar site markers |
| 9 | City labels |
| 10 | UI chrome |

Layers 7–10 keep their numbers. Reference geometry is still drawn immediately after
the highest-numbered map layer, so it stays "6a" and stays above the boundaries. Range
rings and the active-site marker are never covered by a county line.

Terrain tiles stay under the radar when they return post-v1.0. Imagery is a fill and
would hide the echo, which lines do not.

### 2. One place records the order

Pass 1's scene is drawn by one function, `render::Scene::draw`. Both the frame and
the `#[ignore]`d offscreen read-back tests (`render::offscreen`) call it, so the test
checks the order a frame actually uses and does not keep a copy of it.

### 3. The lines are slightly transparent and recoloured for contrast over echoes

The pipeline already uses `ALPHA_BLENDING`, so only the constants in
`render::overlay::DRAW_ORDER` change:

| Layer | Stage 5 RGBA | ADR-0032 RGBA |
|---|---|---|
| Counties | `[0.36, 0.38, 0.43, 0.55]` | `[0.62, 0.64, 0.70, 0.80]` |
| States / provinces, coastline (one colour) | `[0.58, 0.62, 0.70, 0.75]` | `[0.88, 0.90, 0.95, 0.90]` |
| Primary roads | `[0.72, 0.55, 0.30, 0.60]` | `[0.92, 0.58, 0.72, 0.85]` |

Alpha is 0.80–0.90, so a line tints the echo under it and does not erase it.

**How they were chosen.** Each candidate was alpha-blended over every `SolidColor`
entry of the seven bundled palettes and over `BACKGROUND`, and scored by CIELAB ΔE
(CIE76) between the blended pixel and the pixel without the line. ΔE was used because
luminance contrast alone cannot separate a light line from a saturated red echo of
similar luminance. Measured minimum ΔE:

| Layer | Background | Reflectivity ≥ 45 dBZ | Velocity (all) | Weakest entry |
|---|---|---|---|---|
| Counties, Stage 5 | 21.8 | 32.8 | 4.4 | velocity −1 m/s |
| Counties, ADR-0032 | 52.6 | 51.4 | 21.1 | spectrum width 5 m/s (7.2) |
| States, Stage 5 | 48.4 | 46.3 | 19.3 | spectrum width 5 m/s |
| States, ADR-0032 | 80.2 | 69.1 | 36.7 | spectrum width 8 m/s (6.4), ZDR 0 dB (6.8) |
| Roads, Stage 5 | 45.3 | 21.3 | 17.3 | reflectivity 45 dBZ |
| Roads, ADR-0032 | 67.1 | 43.1 | 40.6 | spectrum width 5 m/s (32.2) |

- **Counties** stay the least prominent line on the bare background (ΔE 52.6 against
  80.2 and 67.1). They are now light enough to read over 50–60 dBZ red, where the
  Stage 5 grey disappeared.
- **States and coastline** are near-white, the strongest line on every saturated
  palette entry.
- **Roads** move from orange to rose. Orange matched 45–50 dBZ and the outbound
  velocity colours, so roads disappeared in the echoes that matter most. Rose is not a
  hue in any bundled palette, and its lowest ΔE is 32.2 across all seven.

**Stated limit.** No neutral line can contrast with a neutral palette entry. The near-white
end of spectrum width and ZDR ≈ 0 dB (220 grey) leave the counties and states lines
faint. This is stated, not engineered around: those products are read for their
values, not for geography, and the roads line stays clear over them.

**Visual check.** `render::offscreen::map_lines_read_over_strong_echoes_and_velocity`
renders KDOX at 230 km over a synthetic sweep with 45–75 dBZ sectors, and over a
−60 to +60 m/s velocity sweep, with and without the vectors. It writes both
composites to `target/composite_kdox_{ref_strong,vel}.ppm`. Inside the radar disc, the
vectors change 13,636 pixels with a median per-channel delta of 171 (reflectivity) and
183 (velocity). Under the Stage 5 order the same test changes 33 pixels (the site
markers only) and fails.

**sRGB surfaces.** The overlay colours are passed to the shader unconverted, but
`render::radar` linearises its LUT when the surface is `*_UNORM_SRGB`. On such a
surface the lines render somewhat lighter than the table says, and blend in linear
light. The ordering and prominence above still hold there, since lighter lines only
raise contrast over saturated echoes. This was already true in Stage 5 and is not
changed here.

## Consequences

- The geography is readable inside an echo, where the operator needs it.
- The lines now cover a small part of the echo: the pixels on a 1 px line are tinted
  10–20 % toward the echo colour, not replaced. The cursor readout samples the grid,
  not the screen, so a line never changes a read value.
- The highway toggle (`H`) matters more now that roads are drawn over the echo. It
  continues to persist (`view.highways`).
- No pipeline, shader, buffer, or dependency change; frame cost is unchanged (the same
  draws, in a different order).

## Amended documents

`REQUIREMENTS.md` FR-DR-3 (and its layer-number references), `docs/architecture/rendering.md`
layer table, `CLAUDE.md` "Layer Rendering Order", ADR-0025 and ADR-0027 (layer numbers
only), and the layer-number doc comments in `render/`.
