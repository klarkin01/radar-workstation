# ADR-0023: Radar Sampling in Screen Space (S4-b)

## Status
Accepted

## Context
`rendering.md` contained two sentences in tension:

- "the render loop draws the radar texture as a full-screen quad"
- "the polar grid is mapped to the azimuthal equidistant projection coordinate space by
  the **vertex shader**"

Stage 4 builds the first. The second gets a dated correction in `rendering.md` (the
erratum pattern this project uses throughout).

## Decision

**One full-screen triangle. The fragment shader maps each covered pixel back to (ground
range, azimuth), converts ground range to slant range with the 4/3-earth model, indexes
the R8 grid with `textureLoad`, and does exactly one LUT lookup.**

- Format `R8Uint`, one texture per `(DisplayProduct, Option<elevation_number>)`, native
  grid dimensions (Q17 — never padded, never upsampled). `width = gate_count`,
  `height = azimuth_count`; a texture row is one radial.
- The 256-entry palette LUT (ADR-0021, `compute::palette::compile_lut`) is uploaded as a
  `uniform` buffer holding `array<vec4<f32>, 256>` (4096 bytes), keyed on
  `(DisplayProduct, scale.to_bits(), offset.to_bits())` — **not** one per product, because
  velocity's effective scale/offset vary per sweep and ZDR's are the requantised pair.
  The cache is bounded (32 entries; clear + report on overflow), mirroring
  `RetainedGridSetBounded`.
- The shader contains **exactly one `lut[cell]` index and no colour arithmetic**. sRGB→
  linear conversion, when the surface format needs it, happens once on the CPU at LUT
  compile time (256 × 3 channels), never per pixel.
- **The slant-range correction is not optional.** The gate axis is slant range; the
  projection axis is ground range. On the 0.39° first tilt the difference is sub-pixel at
  any useful zoom — which is exactly what makes skipping it a trap: invisible in the
  clear-air fixtures used during development, and it misplaces echo by kilometres on a
  6.4° cut at 150 km. The closed form is the one already in
  `compute::geometry::slant_range_and_height`:

  ```
  KE_A = (4/3) · 6_371_000
  phi  = ground_m / KE_A
  slant = KE_A · sin(phi) / cos(theta + phi)      // theta = radians(elevation_deg)
  ```

  with a `discard` when `abs(cos(theta + phi))` is near zero (unreachable at real WSR-88D
  geometry; guarded anyway, per Stability as Ethics). For a **derived** product (Echo
  Tops, VIL) the gate axis *is* ground range (`compute::derived`) — step skipped, keyed by
  an `is_ground_range` uniform.
- Azimuth binning is `floor(az / spacing)`, **never `round`**, matching
  `compute::grid::azimuth_slot` exactly. The centre-vs-leading-edge rule was *measured*,
  not assumed (see `compute::grid`'s top-level doc comment); a `round` here silently
  rotates every image by a quarter of a bin. Azimuth is `atan2(x, y)` (0° = North,
  increasing clockwise) — the same convention as `radar-viz`'s `render_grid_ppi`.
- Blend `ALPHA_BLENDING`. FR-DR-4 (below-threshold data transparent) falls out of the
  palette's `ND:` entry being `α = 0` — no threshold logic in the shader. `R8Uint` +
  `textureLoad` makes filtering structurally impossible, which is correct for a grid whose
  cell values 0 and 1 are sentinels (ADR-0020).

## Consequences

- The CPU version already exists and is validated against real data:
  `utility/radar-viz/src/render_grid.rs` does exactly this arithmetic, and Stage 3 §8.1
  compared it pixel-for-pixel against the independent radial-path renderer on a real KDOX
  sweep. Porting proven arithmetic to WGSL is a far smaller risk than inventing mesh
  geometry.
- Cost is constant per frame and independent of grid size: one triangle, one `textureLoad`
  and one LUT index per covered pixel. A 720×1832 grid and a 360×688 grid cost the same.
- No mesh to rebuild on a sweep switch — product and sweep switching stay pure GPU state
  changes (FR-RP-7), demonstrated by `plan_sync` producing an empty upload list across
  both.

## Rejected alternatives

- **A pre-projected polar mesh transformed in the vertex shader** (what `rendering.md`
  originally described) — would need 720×1832 cells decimated and re-tessellated on every
  sweep switch, re-introducing exactly the per-scan CPU work ADR-0020's R8+LUT
  representation removed, and breaking FR-RP-7.
- **Skipping the slant correction** — see above: invisible in development fixtures,
  kilometres of echo misplacement on high tilts at long range.
