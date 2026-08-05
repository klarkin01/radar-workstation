# ADR-0020: Product Texture Representation (Q17, amends FR-RP-6)

## Status
Accepted

## Context
`REQUIREMENTS.md` FR-RP-6 says, literally, that "all products must be pre-computed as
color-mapped RGBA textures by the compute layer." Q17 asked whether standard-resolution
(360 radials/sweep) and super-resolution (720 radials/sweep) share one texture format or
use two.

The arithmetic that forced a different answer than "RGBA, one format." A super-resolution
reflectivity sweep is 720 azimuths × 1832 gates (measured, `rendering.md`):

| | Per sweep (ref, tilt 1) | Full VCP 35 volume, base 3 | Full volume, all seven moments |
|---|---|---|---|
| RGBA8 | 5.27 MB | ~100 MB | ~200 MB |
| R8 + 256-entry LUT | 1.32 MB | ~25 MB | ~50 MB |

The GPU budget is 128 MB per instance (`REQUIREMENTS.md` §4.1), for four simultaneous
instances (NFR-P-1). RGBA for the full seven-moment set does not fit that budget at all.
R8 fits with room to spare for the tile cache and vector geometry Stages 5–6 add.

Three further consequences, each independently favoring R8 + LUT over RGBA:

1. **FR-RP-7 is satisfied by construction.** Every product on every sweep stays
   resident, so switching either is a bind-different-texture GPU state change. Under
   RGBA the only affordable variant was "active product only," which makes a product
   switch — the most frequent keyboard action an operator performs — a recompute of up
   to fourteen sweeps.
2. **Colour mapping becomes 256 evaluations per product, not millions per gate.** A
   grid cell *is* the raw NEXRAD value (`0` = no data, `1` = range fold, `2..=255` =
   `(raw − offset) / scale`), and every observed scale is positive, so `physical` is
   monotonic in `raw`. The palette can therefore be evaluated once per possible cell
   value (`compute::palette::compile_lut`) rather than once per gate. Changing a
   palette is a 256-entry recompute and a 1 KB texture upload, not a re-grid — the
   mechanism that makes FR-CT-3 (load a new palette without restarting) cheap.
3. **The compute layer is simpler, not more complex.** For 8-bit moments, gridding is a
   bounds-checked copy per radial (`compute::grid::grid_sweep`). There is no colour
   arithmetic anywhere in the hot path.

`BC-7` (the render loop may never fetch, decode, or *derive* products) is unaffected by
this decision: a 256-entry texture lookup per pixel is drawing, the same category of
operation as sampling any other texture.

## Decision

**The compute layer emits a single-channel 8-bit grid of quantised moment values per
(sweep, product), plus a 256-entry RGBA palette LUT per product** (`compute::grid`,
`compute::palette`). The fragment shader does one 1D lookup (Stage 4's job — this stage
delivers the LUT-compilation function and stops there, per `compile_lut`'s own doc
comment).

**Cell encoding**, preserved from the ICD exactly:
```
0        — below threshold / no data / azimuth slot never filled
1        — range folded
2..=255  — data; physical = (raw − offset) / scale
```

**Grid dimensions are per-sweep native (Q17), never padded, never upsampled.** Each
`SweepGrid` carries its own `azimuth_count` (720 or 360, from the sweep's *modal*
`azimuth_spacing_code`, not the first radial's), `gate_count`, `first_gate_m`, and
`gate_width_m` as metadata the shader will take as uniforms. Q17's premise — that a
shared format would require padding or upsampling one resolution to match the other —
does not apply once the representation is R8 + LUT: the range dimension varies far more
across measured tilts (688 to 1832 gates) than the azimuth dimension does (360 vs 720),
so no fixed format is efficient regardless of the resolution question. A texture
*array*, the one representation that would require uniform dimensions across grids, is
not needed: only one (product, sweep) pair is ever drawn at a time. Upsampling
standard-resolution to super-resolution was rejected on a stronger ground than memory —
it would fabricate radials the antenna never measured, which this application does not
do (Stability as Ethics).

**16-bit moments are requantised over a bounded display range at gridding time**, so
they still fit an 8-bit cell. ZDR (`word_size == 16`, native scale 32.0, offset 418.0)
is the only in-scope 16-bit moment. Its ICD scale/offset would put every physically
plausible value below cell 0 in an 8-bit encoding — that is the tell that a native 8-bit
copy is wrong for this moment. It is requantised over **−8.0..=+8.0 dB**:

```
k        = 253.0 / (hi - lo)                          // hi=8.0, lo=-8.0
cell     = 2 + ((physical - lo) * k).round()           clamped to 2..=255
scale    = k
offset   = 2.0 - lo * k
```

which satisfies `physical == (cell − offset) / scale` exactly (to one quantisation
step) for the grid's own *effective* `scale`/`offset` — so `compile_lut` and a future
cursor readout need no special case for a requantised product; every `SweepGrid`
carries the formula's inputs directly. Resolution cost: 16 dB over 253 usable levels is
0.063 dB/step, against 0.031 dB native — below what any display or human operator
resolves. Raw codes 0 and 1 map straight to cells 0 and 1 before this arithmetic runs,
so range-fold and no-data survive requantisation exactly as they do for 8-bit moments.

## Alternatives Considered

**Pre-coloured RGBA for all products.** Does not fit the 128 MB GPU budget for the
seven-moment v1.0 set (~200 MB), per the table above.

**Pre-coloured RGBA for the active product only.** Fits the budget, but makes FR-RP-7
false for the most common keyboard action an operator performs: switching products
would require recomputing every resident sweep's texture, not swapping a bound texture.

## Consequences
- `compute::grid::SweepGrid` is the shipped representation; `compute::palette::ColorLut`
  (`[[u8; 4]; 256]`) is the shipped colour-mapping function's output type.
  `compute::palette::compile_lut` is delivered and tested in Stage 3; Stage 4 owns the
  palette texture upload and the call site (`compile_lut`'s doc comment records this
  explicitly, including the deferred caching keyed on `(DisplayProduct, scale.to_bits(),
  offset.to_bits())`).
- FR-RP-6 is amended: "pre-computed as color-mapped RGBA textures" becomes "pre-computed
  as a quantised R8 grid plus a 256-entry palette LUT, colour-mapped by the shader via a
  single lookup" — the property FR-RP-6 protects (no per-frame colour arithmetic) is
  unchanged; the mechanism is not RGBA.
- Q17 is resolved and moved to `open-questions.md`'s Resolved section.
- Any future 16-bit-precision product (PHI/KDP, deferred per Q8) needs its own
  requantisation range decided at the point it is added — this ADR does not generalize
  ZDR's `-8..=+8` range to other moments.
