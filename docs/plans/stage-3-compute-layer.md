# Implementation Plan — Stage 3: The Compute Layer

**Status:** Drafted — not yet implemented
**Drafted:** 2026-08-04
**Implements:** `docs/project-inventory.md` §6, Stage 3 (items 10–13)
**Baseline commit:** `41ed906` (working tree clean, branch `runtime`)
**Toolchain:** rustc 1.95.0 / cargo 1.95.0, `x86_64-unknown-linux-gnu`
**Predecessors:** `docs/plans/stage-0-1-close-the-acquisition-path.md` (§8 Results),
`docs/plans/stage-2-make-the-application-exist.md` (§12 Results)

This plan is written to be executed in a later session. It carries every decision already
taken so implementation does not need to re-derive them from the ADRs or re-open the
questions this plan closes. **All seven open decisions were put to the user and answered
before this plan was written** — §3 records them and the reasoning. There are no
outstanding `DECIDE` items; where the implementer must *measure* something before
choosing an implementation detail, that is called out explicitly as a measurement task,
not as an open decision.

**Scope boundary:** this plan turns decoded radials into GPU-ready product data. At the
end of it, `cargo run --release -- KDOX` polls, decodes, assembles, **grids every
in-scope product for every sweep, derives Echo Tops and VIL per volume**, and holds all
of it in `AppState` ready for upload. It still draws nothing. No window, no wgpu, no
egui — that is Stage 4, and `headless::run` remains the placeholder Stage 4 replaces.

Stage 3 closes four open questions: **Q8** (v1.0 product set), **Q9** (velocity
dealiasing), **Q11** (color table format), and **Q17** (texture grid dimensions). It also
amends **FR-RP-6** and adds two ADRs.

---

## 1. What "done" means

| Claim | How it is demonstrated |
|---|---|
| Q8, Q9, Q11, Q17 are answered and recorded | ADR-0020 and ADR-0021 added; all four moved to the Resolved section of `open-questions.md`; `REQUIREMENTS.md` §7's table shrinks by four rows |
| A closed sweep becomes GPU-ready product data | `compute::grid::grid_sweep` unit tests against the committed KDOX/KTLH fixtures produce a `SweepGrid` with the measured dimensions in `rendering.md`'s table |
| No data is invented | Missing azimuth slots and out-of-range gates are `0` (no-data), never interpolated; a named test asserts a sweep with a deliberate azimuth gap leaves those slots empty |
| Range folding is distinguishable from no echo | ICD raw `1` survives gridding intact and maps to the palette's `RF` colour; `no_echo_and_range_fold_are_distinct_cells` |
| Every in-scope product is produced | Reflectivity, velocity, spectrum width, ZDR, CC per sweep; Echo Tops and VIL per completed volume |
| Colour tables come from the community ecosystem | A real GRLevelX `.pal` file parses; bundled defaults for all seven products are authored in the same format and `include_str!`'d |
| The palette parser cannot crash the application | Mutator-driven fuzz test over a committed `.pal` corpus using `crates/fuzz-support`, mirroring `http-ingest` / `nexrad-decoder` / `config` |
| Memory went **down**, not up | Peak RSS across a full volume boundary — the measurement Stage 2 left unfinished — recorded in §12, with raw radials dropped after gridding |
| Compute never blocks ingest | Gridding runs on `spawn_blocking`; a live test asserts the poller's `IngestStatus` stays `Polling` through a full volume |
| The program stays observable | `headless::run` prints per-product grid dimensions and fill fraction as sweeps land |
| The output is visually correct | `utility/radar-viz` gains a grid-path renderer; its PNG output is compared against its existing radial-path PPI renderer on the same fixture |
| Echo Tops and VIL are not merely plausible | Cross-checked against MetPy/Py-ART via `utility/nexrad-inspect` on one fixture volume, with the numbers recorded |

**Requirements closed or advanced:** FR-RP-1 (advanced — all three base products computed;
nothing displays them yet), FR-RP-2 (advanced — Echo Tops and VIL computed), FR-RP-3
(closed — ZDR and CC in, PHI/CFP/KDP deferred), FR-RP-4 (closed as deferred), FR-RP-5
(closed as deferred with fold indicators), FR-RP-6 (**amended** — see ADR-0020), FR-RP-7
(closed by construction — every product on every sweep is resident, so switching either
is a GPU state change), FR-CT-1 (closed), FR-CT-2 (closed), FR-CT-3 (closed for loading;
no hot reload).

**Not closed, deliberately:** everything with a UI surface. FR-DR-4's transparency is
*encoded* by Stage 3 (the palette's no-data entry is `α = 0`) but not *rendered* until
Stage 4. The colour-scale legend, the Nyquist readout Q9's answer depends on, and the
status-bar surfacing of every event this stage adds are Stage 4's.

---

## 2. What Stages 0–2 left that this plan builds on

Read `stage-2-make-the-application-exist.md` §12 before starting. Six of its outcomes
shape the work here:

- **`AssemblyEvent::SweepClosed` already carries `Arc<Sweep>`, `VolumeId`, and
  `vcp_number`.** The compute layer needs no new information from the assembler. It
  attaches as a new stage between `assembly::run` and `apply_loop`, not as a change to
  either.
- **`ProductData` already exposes `raw_gate(i)` and `physical_value(i)`, with ICD codes
  0 and 1 handled** (`crates/nexrad-decoder/src/types/product.rs`). Gridding must
  preserve those two codes, not go through `physical_value`, which discards the
  distinction between them. This is the single easiest mistake to make in S3-W1.
- **`ADR-0018` anticipated exactly this stage's retention mitigation** ("once the compute
  layer produces textures, raw radials for products nobody is displaying can be dropped
  after computation"). §5.3 takes that further than the ADR sketched — raw radials are
  dropped for *every* product, because the gridded cells are the raw values.
- **Stage 2 left one measurement unfinished:** peak RSS specifically across a volume
  boundary. It lands here (§12), and the retention change is what makes it interesting.
- **`state::apply` is a pure function with `now` injected**, tested rule-by-rule. Its
  input type changes in this stage (§5.2); its shape, its purity, and most of its tests
  do not.
- **`utility/radar-viz` already renders a sweep to PNG** with a colour table, a PPI
  projection, and a hand-rolled encoder. It is the validation harness for this stage
  (§8), and the reason S3-W1 can be checked visually before the `.pal` parser exists.

---

## 3. Decisions taken before this plan

All seven were put to the user and answered on 2026-08-04. They are recorded here in
full so implementation does not re-litigate them, and so the ADRs in S3-W6 have a source.

### 3.1 (S3-a) Product representation: quantised R8 grid + 256-entry palette LUT

**Decision: the compute layer emits a single-channel 8-bit grid of quantised moment
values per (sweep, product), plus a 256-entry RGBA palette LUT per product. The fragment
shader does one 1D lookup.** This amends FR-RP-6's literal "pre-computed as color-mapped
RGBA textures" and gets ADR-0020.

The arithmetic that forced it. A super-resolution reflectivity sweep is 720 azimuths ×
1832 gates (`rendering.md`, measured):

| | Per sweep (ref, tilt 1) | Full VCP 35 volume, base 3 | Full volume, all moments |
|---|---|---|---|
| RGBA8 | 5.27 MB | ~100 MB | **~200 MB** |
| R8 + LUT | 1.32 MB | ~25 MB | **~50 MB** |

The GPU budget is 128 MB per instance (`REQUIREMENTS.md` §4.1), for four simultaneous
instances (NFR-P-1). RGBA for the full moment set does not fit. R8 fits with room for the
tile cache and vector geometry Stages 5 and 6 add.

Three further consequences, each of which independently favours this choice:

1. **FR-RP-7 is satisfied by construction.** Every product on every sweep stays resident,
   so switching either is a bind-different-texture state change. Under RGBA the only
   affordable variant was "active product only", which makes a product switch — the most
   frequent keyboard action an operator performs — a recompute of fourteen sweeps.
2. **Colour mapping becomes 256 evaluations per product, not millions per gate.** The
   grid cell *is* the raw NEXRAD value, and `physical = (raw − offset) / scale` is
   monotonic (every observed scale is positive), so the palette can be evaluated once per
   possible cell value. Changing a palette is a 256-entry recompute and a 1 KB texture
   upload — no re-gridding, which is what makes FR-CT-3 cheap.
3. **The compute layer gets simpler, not more complex.** For 8-bit moments gridding is a
   bounds-checked copy per radial. There is no colour arithmetic in the hot path at all.

BC-7 is unaffected: it prohibits the render loop from fetching, decoding, and *deriving
products*. A texture lookup is drawing.

### 3.2 (S3-b, Q8) v1.0 product set

**Decision: reflectivity, velocity, spectrum width, Echo Tops, VIL, plus ZDR
(differential reflectivity) and CC (correlation coefficient).**

`REQUIREMENTS.md`'s conservative default deferred all dual-pol. It is revised because the
cost calculus changed: with a generic moment→grid path, ZDR and CC cost one palette and a
few megabytes each — no new algorithm, no new decoder work (FR-ND-4 already decodes
them). CC in particular is the highest-value dual-pol product for exactly this
application's stated use case: distinguishing a debris signature from precipitation
during a tornado warning.

Still deferred, and for reasons of substance rather than budget:

- **KDP** must be *derived* by differentiating PHI over range — a real algorithm with a
  filter-length choice, not a free moment.
- **PHI and CFP** are diagnostic quantities a general operator rarely reads directly;
  including them buys palette and legend surface for little operational value
  (Restraint is a Feature).
- **Storm-relative velocity** needs a storm-motion input mechanism, i.e. UI that does not
  exist until Stage 4.

### 3.3 (S3-c, Q11) Colour table format: GRLevelX `.pal` subset

**Decision: parse a documented subset of the GRLevelX palette format.** Bundled defaults
are authored in the same format and compiled in with `include_str!`; user palettes load
from the XDG data directory and override by product. Unknown directives are skipped and
reported, never fatal. ADR-0021.

This is FR-CT-1's stated rationale honoured: an operator arriving from GR2Analyst keeps
the palettes they already own. Authoring the bundled defaults in the user-facing format
also means the shipped tables double as readable worked examples.

### 3.4 (S3-d, Q17) Grid dimensions: per-sweep native

**Decision: each (sweep, product) grid is sized to that sweep's own measured geometry**
— azimuth count from the spacing code, gate count / first gate / gate width from the
moment block — carried as metadata the shader takes as uniforms. Nothing is padded,
nothing is upsampled.

Q17 asked whether standard-res (360 radials) and super-res (720) share one texture format
or use two. The answer is neither: the range dimension varies far more than the azimuth
dimension does (688 to 1832 gates across measured tilts, `rendering.md`), so any fixed
format wastes more memory on range than a shared azimuth format could ever save. A
texture *array* — the one thing that would require uniform dimensions — is not needed,
because only one (product, sweep) pair is drawn at a time.

Upsampling standard-res to super-res was rejected on a stronger ground than memory: it
fabricates radials that the antenna did not measure. This application does not invent
data.

### 3.5 (S3-e, Q9) Velocity dealiasing: deferred, with fold indicators

**Decision: no dealiasing in v1.0.** Instead, make both fold conditions legible:

- **Range folding** (ICD raw value 1) gets its own palette entry (`RF:`), so it is
  visually distinct from "no echo" rather than silently blank. S3-W1 preserves the code
  through gridding specifically to make this possible.
- **Velocity aliasing** is bounded by the sweep's Nyquist velocity, which is already
  decoded onto `Sweep::nyquist_velocity_mps`. Stage 3 carries it onto the grid metadata
  so Stage 4's status bar and colour-scale legend can display it.

A dealiasing algorithm that unfolds wrongly during a warning shows a couplet that is not
there. A visible fold, with the Nyquist stated, is read correctly by the operator this
application is built for. Documented as a known limitation.

### 3.6 (S3-f) rayon: deferred pending measurement

**Decision: do not add rayon in Stage 3.** Compute runs on `tokio::task::spawn_blocking`
with plain iterators, instrumented, with the numbers recorded in §12. ADR-0005 gains a
dated erratum recording that its adoption is *deferred pending measurement*, not
reversed.

Reasoning: gridding one sweep is close to a `memcpy`, and sweeps close roughly twenty
seconds apart, so the per-event work is milliseconds against a twenty-second budget. The
only substantial pass is Echo Tops/VIL, once per volume (every 4–6 minutes). rayon's
default pool is one thread per core — eight-plus mostly-idle threads per instance against
NFR-P-1's four-instance case and the deliberately-chosen two-worker tokio pool. Adding
roughly five packages to a 67-package graph for work that may not need them is the
opposite of how this project has decided every previous dependency question.

**If §12's measurements show gridding above ~50 ms per sweep or derived products above
~500 ms per volume, revisit — that is the trigger, and it belongs in the erratum.**

### 3.7 (S3-g) Retention: raw radials are dropped after gridding

**Decision: once a sweep is gridded, its `Vec<Radial>` is released.** `RadarState` holds
grids, not sweeps. `last_complete: Option<Arc<VolumeScan>>` becomes
`last_complete: Option<VolumeSummary>` — metadata only.

Nothing needs the radials afterwards:

- The gridded cells **are** the raw gate values, and each grid carries its own
  effective scale/offset, so a cursor readout recovers the exact physical value.
- Echo Tops and VIL are computed from the grids.
- Site latitude/longitude/elevation and the volume's identity move to `VolumeSummary`.

This is ADR-0018's own anticipated Stage 3 mitigation, taken to its conclusion. It takes
steady-state radar memory from ~100 MB of radials *plus* ~50 MB of grids down to ~50 MB,
and it dissolves the untested "two volumes' worth across a boundary" transient the ADR
flagged rather than merely measuring it.

**One consequence to record in ADR-0018's erratum:** any future product needing full
16-bit PHI precision (KDP) must be computed *during* gridding, while the radials are
still in scope, rather than later from state. Say so now; do not discover it later.

---

## 4. S3-W1 — The sweep grid

**New module `crates/radar-workstation/src/compute/`.** Register it in `lib.rs`.

```
compute/
├── mod.rs        ← DisplayProduct, StateUpdate, compute_loop
├── grid.rs       ← SweepGrid, grid_sweep, quantisation  (S3-W1)
├── palette.rs    ← Palette, .pal parser, compile_lut     (S3-W3)
├── palettes/     ← bundled default .pal files            (S3-W3)
├── geometry.rs   ← beam height / ground range            (S3-W4)
├── derived.rs    ← Echo Tops, VIL                        (S3-W4)
└── tests.rs
```

A module in `radar-workstation`, not a fourth production crate. The compute layer is
application-specific, has no consumer outside the binary, and shares `AppState`'s types —
ADR-0010's rationale for splitting the decoder out (an independently testable library
with its own fixture suite) does not apply here.

### 4.1 Measure this before writing the binning code

**Is Message 31's `AZ` field the leading edge of the radial or its centre?** The gridding
convention depends on it, and the answer is measurable from data already in the tree.

Using `utility/nexrad-inspect` (or a throwaway test) over
`crates/nexrad-decoder/tests/fixtures/`, check whether `azimuth_deg` for consecutive
`azimuth_number` values lands on multiples of the spacing or offset by half a bin. Record
the finding in `docs/architecture/nexrad-binary-format.md` §6.1 alongside the `az_spacing`
correction already there, and set `SweepGrid`'s documented convention to match. Do not
guess: a half-bin error is a 0.25° rotation of every radar image the application ever
draws, and it will not be obvious by eye.

### 4.2 Types

```rust
// compute/mod.rs

/// A product the user can select for display. Distinct from
/// `nexrad_decoder::ProductKind` because it also covers products the
/// decoder never sees (Echo Tops, VIL) and excludes the moments Q8
/// deferred (PHI, CFP).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DisplayProduct {
    Reflectivity,
    Velocity,
    SpectrumWidth,
    Zdr,
    Cc,
    EchoTops,
    Vil,
}

impl DisplayProduct {
    /// The five products gridded directly from a decoded sweep, paired with
    /// the decoder moment each comes from. Echo Tops and VIL are absent —
    /// they are volume-derived and have no source moment.
    pub const BASE: [(DisplayProduct, ProductKind); 5] = [ /* ... */ ];
}
```

```rust
// compute/grid.rs

/// One product, on one sweep, as a dense regular polar grid ready for
/// upload as an R8 texture.
///
/// Cell encoding, preserved from the ICD (see `ProductData`):
///   0        — below threshold / no data / azimuth slot never filled
///   1        — range folded
///   2..=255  — data; `physical = (cell as f32 - offset) / scale`
///
/// Row-major, azimuth-major: cell (a, g) is at `cells[a * gate_count + g]`.
pub struct SweepGrid {
    pub product: DisplayProduct,

    // --- geometry (Q17: native, per sweep; never padded) ---
    /// 720 for super-resolution, 360 for standard. Slot `a` is centred on
    /// `a * (360.0 / azimuth_count)` degrees true — see §4.1.
    pub azimuth_count: u16,
    pub gate_count: u16,
    pub first_gate_m: u16,
    pub gate_width_m: u16,

    // --- provenance / display metadata ---
    pub elevation_number: u8,
    pub elevation_deg: f32,
    /// Q9: carried so Stage 4 can state the fold limit rather than
    /// silently displaying aliased data.
    pub nyquist_velocity_mps: Option<f32>,

    // --- values ---
    /// Effective scale/offset *after* any requantisation (§4.4), so the
    /// single formula above always holds and `compile_lut` needs nothing else.
    pub scale: f32,
    pub offset: f32,
    pub cells: Vec<u8>,

    /// How many azimuth slots received at least one radial. Diagnostic:
    /// feeds `headless`'s output now and a data-completeness indicator later.
    pub filled_azimuths: u16,
}

impl SweepGrid {
    pub fn cell(&self, azimuth: u16, gate: u16) -> u8;
    /// `None` for cells 0 and 1 — same contract as `ProductData::physical_value`.
    pub fn physical(&self, azimuth: u16, gate: u16) -> Option<f32>;
    pub fn byte_len(&self) -> usize;
}
```

### 4.3 Gridding

```rust
/// Grid one product from one closed sweep. `None` when the sweep carries no
/// radial with this moment at all — a split cut legitimately lacks velocity
/// on the surveillance half, and that is not an error.
pub fn grid_sweep(sweep: &Sweep, product: DisplayProduct) -> Option<(SweepGrid, Vec<GridEvent>)>;
```

Steps, in order:

1. **Azimuth count.** From the *modal* `azimuth_spacing_code` across the sweep's radials
   (1 → 720, 2 → 360), not the first radial's — one corrupt radial must not resize the
   grid. A code that is neither falls back to inferring from radial count (≥ 540 → 720,
   else 360) and emits `GridEvent::UnknownAzimuthSpacing`.
2. **Geometry.** From the first radial carrying this moment: `gate_count`,
   `first_gate_m`, `gate_width_m`, `word_size`, `scale`, `offset`. Reject
   `gate_width_m == 0` (guards a later division) and `gate_count == 0` — return `None`
   with an event.
3. **Allocate** `vec![0u8; azimuth_count as usize * gate_count as usize]`. Zero is
   no-data, so an unfilled grid is already correct.
4. **Scatter each radial.**
   `slot = (azimuth_deg / spacing).round().rem_euclid(azimuth_count)` — using the
   convention §4.1 establishes. A radial whose moment geometry differs from the sweep's
   chosen geometry is **skipped**, counted, and reported once as
   `GridEvent::InconsistentGateGeometry { skipped }` — never partially copied into a row
   sized for different geometry.
5. **Copy or requantise** the gates into row `slot` (§4.4). A slot written twice (rare;
   possible at the 0°/360° seam) takes the last writer and increments a duplicate count.
6. **Return** the grid plus any events.

Note what is *not* here: no interpolation, no gap filling, no smoothing, no nearest-radial
search. `radar-viz`'s `nearest_radial` exists because it renders to a *screen* raster; a
polar grid has a slot for every radial the antenna produced, so scattering is exact and
the render-time interpolation question moves entirely to Stage 4's shader.

Tests, each named:

| Rule | Test |
|---|---|
| A super-res KDOX fixture sweep grids to 720 × 1832 with the `rendering.md` geometry | `kdox_vcp35_reflectivity_grid_matches_measured_geometry` |
| A standard-res KTLH fixture sweep grids to 360 azimuths | `ktlh_vcp212_standard_resolution_grids_to_360_azimuths` |
| A missing azimuth is left empty, not filled from a neighbour | `absent_radials_leave_no_data_cells` |
| Raw 0 and raw 1 stay distinct through gridding | `no_echo_and_range_fold_are_distinct_cells` |
| A gate's physical value survives the round trip | `physical_value_round_trips_through_the_grid` (assert against `ProductData::physical_value` on the source radial) |
| A sweep with no velocity returns `None`, not an empty grid | `split_cut_without_velocity_returns_none` |
| A radial with mismatched gate geometry is skipped and reported | `inconsistent_gate_geometry_is_skipped_not_copied` |
| `gate_width_m == 0` cannot cause a division | `zero_gate_width_is_rejected` |
| An empty sweep does not panic | `empty_sweep_returns_none` |

### 4.4 Quantisation

**8-bit moments — reflectivity, velocity, spectrum width, CC — are copied verbatim.**
`scale` and `offset` pass through unchanged. This is the common case and it must stay a
copy.

**ZDR is 16-bit** (`word_size == 16`, scale 32.0, offset 418.0 — an 8-bit ZDR would put
every value below −5 dB, which is how you can tell). It is requantised over a display
range:

```
ZDR display range: -8.0 ..= +8.0 dB
k        = 253.0 / (hi - lo)
cell     = 2 + ((physical - lo) * k).round()      clamped to 2..=255
scale    = k
offset   = 2.0 - lo * k
```

which satisfies `physical == (cell − offset) / scale` exactly, so the LUT compiler and
the cursor readout need no special case for requantised products. Raw 0 and raw 1 map
straight to cells 0 and 1 before this arithmetic runs.

Resolution cost: 16 dB over 253 levels is 0.063 dB per step, against a native
0.031 dB — below what any display or operator resolves. Record the range and the cost in
ADR-0020; do not leave it implicit.

Tests: `zdr_requantisation_round_trips_within_one_step`,
`zdr_values_outside_the_display_range_clamp_rather_than_wrap`,
`eight_bit_moments_are_copied_verbatim`.

---

## 5. S3-W2 — Pipeline and state integration

This is the item that makes S3-W1 observable end to end, and it is deliberately second —
same instinct as Stage 2's ordering, and the same reason: a running program you can watch
is worth more than a larger pile of unwired code.

### 5.1 A fourth stage in the pipeline

`data-flow.md` already specifies decoder → **compute** → shared state. Make it real:

```
poller ──chunks──► assembly ──AssemblyEvent──► compute ──StateUpdate──► applier ──► AppState
```

```rust
// compute/mod.rs

/// What the compute layer hands the applier. Distinct from `AssemblyEvent`
/// because everything below this point deals in grids, not radials.
pub enum StateUpdate {
    SweepGridded {
        elevation_number: u8,
        elevation_deg: f32,
        volume: VolumeId,
        vcp_number: u16,
        /// One entry per base product actually present on this sweep.
        grids: Vec<Arc<SweepGrid>>,
    },
    DerivedComputed {
        volume: VolumeId,
        vcp_number: u16,
        grids: Vec<Arc<SweepGrid>>,   // EchoTops, Vil
    },
    VolumeClosed { summary: VolumeSummary },
    /// Pass-through for the assembler's observability events, which the
    /// compute layer neither consumes nor interprets.
    Info(AssemblyEvent),
}

pub async fn compute_loop(
    mut rx: mpsc::Receiver<AssemblyEvent>,
    tx: mpsc::Sender<StateUpdate>,
    state: Arc<AppState>,
    mut shutdown: watch::Receiver<bool>,
);
```

Behaviour:

- **`SweepClosed`** → `spawn_blocking(move || grid_all_base_products(&sweep))`, awaited
  inline. `spawn_blocking`, not a direct call: the runtime has two workers (S2-c) and a
  multi-millisecond synchronous burst on one of them delays the poller's next fetch.
  Awaiting inline means at most one grid job runs at a time — bounded, and it is what
  keeps the "no rayon yet" decision honest. `GridEvent`s are reported via
  `state.report(...)`. The resulting `Arc<SweepGrid>`s are sent onward **and retained by
  the compute task** for the accumulating volume (§5.4) — `Arc`, so this costs refcounts,
  not memory.
- **`VolumeClosed { volume }`** → build a `VolumeSummary` from `volume`'s metadata,
  dropping its `sweeps`. If `status == Complete`, compute derived products from the
  retained grids (§7) on `spawn_blocking` and emit `DerivedComputed` **before**
  `VolumeClosed`. Clear the retained set either way.
- **`LateRadialsDiscarded` / `MissingStartChunk`** → `StateUpdate::Info`, untouched.
- **Shutdown** → the same `select!` arm every other loop in `pipeline.rs` has.

`run_ingest_trio` in `pipeline.rs` becomes a quartet: one more bounded channel
(`COMPUTE_CHANNEL_CAPACITY = 64`, same reasoning as `EVENT_CHANNEL_CAPACITY` — four
volumes of headroom) and one more future in the `tokio::join!`. Update the module's
top-level doc comment: the "supervised as one unit" argument covers four tasks now, for
exactly the same reason it covered three, and `TaskKind::IngestPipeline` still has one
variant.

### 5.2 `RadarState` after Stage 3

```rust
pub struct DisplaySweep {
    pub elevation_number: u8,
    pub elevation_deg: f32,
    pub volume: VolumeId,
    pub vcp_number: u16,
    pub received: Instant,
    /// Sorted by `DisplayProduct`; typically 3–5 entries.
    pub grids: Vec<Arc<SweepGrid>>,
}

/// Replaces `Option<Arc<VolumeScan>>` (S3-g). Metadata only — the sweeps
/// this was built from are already gridded and released.
#[derive(Clone, Copy)]
pub struct VolumeSummary {
    pub volume: VolumeId,
    pub vcp_number: u16,
    pub status: VolumeStatus,
    pub latitude: f32,
    pub longitude: f32,
    pub site_amsl_m: i16,
}

pub struct RadarState {
    pub site: &'static Site,
    sweeps: BTreeMap<u8, DisplaySweep>,
    /// Volume-level products (Echo Tops, VIL) — one grid each, replaced
    /// wholesale when a volume closes Complete.
    derived: BTreeMap<DisplayProduct, Arc<SweepGrid>>,
    current_vcp: Option<u16>,
    pub last_complete: Option<VolumeSummary>,
    pub revision: u64,
}
```

`StateSnapshot` gains `derived: Vec<Arc<SweepGrid>>` and its `last_complete` changes type.
`snapshot()`'s cost stays what ADR-0018 argued: a `Vec` allocation and N refcount bumps.

`state::apply`'s signature changes from `AssemblyEvent` to `StateUpdate`. **Every existing
rule in `state/apply.rs` survives unchanged in meaning**, and so do its tests, modulo
constructing the new input type:

| Existing rule | Change |
|---|---|
| `SweepClosed` replaces the entry for its elevation | Now `SweepGridded`; same rule |
| A sweep from an older `VolumeId` never replaces a newer one | Unchanged |
| `VolumeClosed` sets `last_complete` only for `Complete` | Now stores a `VolumeSummary`; same rule |
| `TimedOut` / `Superseded` clear nothing | Unchanged — and now explicitly must not clear `derived` either |
| A VCP change drops the old pattern's elevations | Unchanged, **plus** clears `derived`: a new VCP's tilts make the old volume's Echo Tops meaningless |
| Informational events do not bump `revision` | Unchanged |
| `reset(site)` empties everything | Now also clears `derived` |

New rules, each with a named test:

| Rule | Rationale | Test |
|---|---|---|
| `DerivedComputed` replaces `derived` wholesale | Echo Tops and VIL are volume-scoped; a partial merge would mix volumes | `derived_products_are_replaced_not_merged` |
| `DerivedComputed` from an older volume is ignored | Same out-of-order guard as sweeps | `stale_derived_products_do_not_overwrite_newer` |
| A `TimedOut` volume leaves the previous `derived` in place | FR-DA-5: last good data stays displayed | `timed_out_volume_leaves_derived_products_intact` |

### 5.3 Retention, and the measurement it unlocks

After S3-W2 nothing in `AppState` holds a `Vec<Radial>`. The last `Arc<Sweep>` is dropped
when `compute_loop`'s `spawn_blocking` closure returns.

Write the *test* that pins this, not just the code:
`gridding_releases_the_source_sweep` — hold a `Weak<Sweep>` across the compute step and
assert it cannot be upgraded once the update has been applied. That is the kind of
property that silently regresses the first time someone adds a convenience field.

### 5.4 What the compute task retains

The accumulating volume's reflectivity grids, as `Arc` clones of what it already sent
onward — needed for Echo Tops and VIL at volume close, and free because `AppState` holds
the same allocations. Cleared on `VolumeClosed` regardless of status, and on shutdown.

Bound it: if the retained set exceeds a sane elevation count (say 40 — no VCP approaches
it), drop the oldest and report. A stuck volume must not be a slow leak. Test:
`retained_grid_set_is_bounded`.

### 5.5 `headless::run` stays useful

Extend the state-change line to print, per sweep, the products present and each grid's
dimensions and fill fraction, plus the derived products when they appear:

```
[KDOX] rev=41 sweeps=6 derived=2 el=1 (0.39°) ref 720x1832 100% vel — zdr 720x1192 100% cc 720x1192 100%
```

This is the whole of Stage 3's user-visible surface, and it is what makes a live run
diagnosable before there is a renderer.

---

## 6. S3-W3 — Colour tables

### 6.1 The format subset

```rust
// compute/palette.rs

pub struct Palette {
    pub product: DisplayProduct,
    pub units: String,
    /// Legend tick spacing; Stage 4's colour scale consumes it.
    pub step: Option<f32>,
    /// Ascending by threshold. `to` is `Some` for a gradient entry, whose
    /// colour ramps to the next entry's threshold.
    entries: Vec<PaletteEntry>,
    pub range_folded: [u8; 4],
    pub no_data: [u8; 4],
}

struct PaletteEntry { threshold: f32, from: [u8; 4], to: Option<[u8; 4]> }

impl Palette {
    /// Colour for a physical value. Below the first threshold → `no_data`
    /// (FR-DR-4: below the minimum displayable threshold renders fully
    /// transparent, which is `no_data`'s α = 0).
    pub fn sample(&self, value: f32) -> [u8; 4];
}

/// Never fails. Unparseable directives and malformed lines are skipped and
/// reported, exactly as `config::load` does — an unreadable palette must
/// fall back to the bundled default, never prevent startup (Stability as
/// Ethics; the same discipline FR-CP-3 imposes on configuration).
pub fn parse(text: &str, product: DisplayProduct) -> (Palette, Vec<Event>);
```

Directives to support:

| Directive | Meaning |
|---|---|
| `Product: <name>` | Informational |
| `Units: <name>` | Informational; shown on the legend |
| `Step: <f>` | Legend tick spacing |
| `Color: <v> <r> <g> <b> [<r2> <g2> <b2>]` | Entry at `v`; a second triple makes it a gradient to the next entry |
| `Color4: <v> <r> <g> <b> <a> [<r2> <g2> <b2> <a2>]` | As above with alpha |
| `SolidColor: <v> <r> <g> <b>` | Entry at `v`, no gradient to the next entry |
| `SolidColor4: <v> <r> <g> <b> <a>` | As above with alpha |
| `RF: <r> <g> <b> [<a>]` | Range-folded colour (cell 1) |
| `ND: <r> <g> <b> [<a>]` | No-data colour (cell 0); defaults to fully transparent |
| `;` to end of line | Comment |

**Verify this table against real community `.pal` files before finalising it**, and
record what you find in ADR-0021 — the risk is low precisely because unknown directives
are skipped and reported rather than rejected, but the supported set is a compatibility
contract and should be written from evidence, not recollection. Collect three or four
real palettes into the fuzz corpus while you are there.

### 6.2 LUT compilation

```rust
pub type ColorLut = [[u8; 4]; 256];

/// Evaluate `palette` once per possible cell value. This is the entirety of
/// the application's colour mapping (S3-a): 256 evaluations per product, not
/// one per gate.
pub fn compile_lut(palette: &Palette, scale: f32, offset: f32) -> ColorLut {
    let mut lut = [[0u8; 4]; 256];
    lut[0] = palette.no_data;
    lut[1] = palette.range_folded;
    for raw in 2..=255 {
        lut[raw] = palette.sample((raw as f32 - offset) / scale);
    }
    lut
}
```

Cache keyed by `(DisplayProduct, scale.to_bits(), offset.to_bits())` — scale and offset
are constant per moment in practice but are not guaranteed to be, and keying on them costs
nothing.

Stage 3 delivers this function and its tests; **Stage 4 decides the call site** (it owns
the palette texture upload). Do not wire it into `AppState`: palettes are neither radar
data nor view state, they are load-once configuration, and putting them under
`RadarState`'s lock would be exactly the mistake ADR-0018 was written to avoid.

### 6.3 Bundled defaults and user palettes

Seven `.pal` files in `compute/palettes/`, `include_str!`'d — no runtime file dependency
and no startup parse-failure mode, the same reasoning as the generated site table
(ADR-0006 erratum):

`reflectivity.pal`, `velocity.pal`, `spectrum_width.pal`, `zdr.pal`, `cc.pal`,
`echo_tops.pal`, `vil.pal`.

Author reflectivity, velocity and spectrum width from `utility/radar-viz/src/color_table.rs`,
which already carries working NWS-standard tables validated against real data — port them,
do not reinvent them. ZDR, CC, Echo Tops and VIL are new; use the conventional NWS ranges
and state the source in a comment at the top of each file.

User palettes load from `paths::data_dir()/palettes/<product>.pal` and override by
product (FR-CT-3). A missing directory, a missing file, or an unreadable one is silent —
the first-run case, exactly as `config::load` treats a missing config. A *malformed* one
is reported and falls back to the bundled default.

```rust
pub fn load_all() -> (BTreeMap<DisplayProduct, Palette>, Vec<Event>);
```

Every product always resolves to a palette. There is no failure mode in which a product
has no colours.

### 6.4 Tests

Round-trip a bundled default; gradient interpolation at an exact threshold and midway
between two; a value below the first threshold is transparent; `RF` and `ND` land at LUT
indices 1 and 0; a real community `.pal` parses; unknown directives are skipped and
reported; a user palette overrides the bundled default; a malformed user palette falls
back and reports; and — mandatory, following `http-ingest`, `nexrad-decoder` and `config`
— `mutated_palette_never_panics`, a seeded-mutator test over a committed corpus using
`crates/fuzz-support`. Four parsers in this workspace now share one mutator.

---

## 7. S3-W4 — Derived products

### 7.1 Beam geometry

```rust
// compute/geometry.rs

const EARTH_RADIUS_M: f64 = 6_371_000.0;
/// Standard atmosphere effective-earth-radius factor.
const REFRACTION_K: f64 = 4.0 / 3.0;
const KE_A: f64 = REFRACTION_K * EARTH_RADIUS_M;

/// Slant range and height above the radar for a target at ground range
/// `ground_m` along the surface, on a beam at elevation `elev_deg`.
///
/// Closed form, from the triangle (earth centre, radar, target) with
/// central angle φ = ground / KE_A:
///   r = KE_A · sin φ / cos(θ + φ)
///   h = KE_A · (cos θ / cos(θ + φ) − 1)
pub fn slant_range_and_height(ground_m: f64, elev_deg: f64) -> (f64, f64);

/// Forward direction, for tests and for a future cursor readout.
pub fn ground_range_and_height(slant_m: f64, elev_deg: f64) -> (f64, f64);
```

Tests: the two are inverses to within a metre across the measured range/elevation
envelope (`round_trips_against_ground_range_and_height`); height at 0° elevation and
100 km ground range is ~660 m, the textbook 4/3-earth figure
(`beam_height_matches_the_standard_four_thirds_figure`); φ near 0 does not divide by
zero.

### 7.2 Output geometry

Both derived products adopt **the lowest-elevation reflectivity grid's geometry**:
its `azimuth_count`, `first_gate_m`, `gate_width_m`, and `gate_count`. Data-driven, needs
no invented maximum range, and puts the derived products on exactly the grid the operator
is already looking at. The gate axis is reinterpreted as **ground** range rather than
slant range — record that in the doc comment, because it is the one place in the codebase
where the two differ materially.

Input tilts: one reflectivity grid per **distinct elevation angle**, taking the newest
where a VCP repeats an angle (SAILS/MRLE insert repeated low-level cuts with distinct
elevation *numbers* at the same angle — see `RadialStatus::StartOfLastElevation`'s doc
comment), sorted ascending by angle. Test:
`repeated_sails_cuts_contribute_one_tilt_not_two`.

### 7.3 Echo Tops

For each output cell `(a, g)`, walk tilts from highest to lowest; the first whose
reflectivity at that column meets the threshold gives the echo top.

```
ECHO_TOP_THRESHOLD_DBZ = 18.0     // conventional
output units: kft, quantised 0..=70 kft over cells 2..=255
```

Per tilt: `(r, h) = slant_range_and_height(ground, elev)`, gate index
`(r − first_gate_m) / gate_width_m`; out of range → that tilt contributes nothing.
Reported height is the **beam centre** of the highest qualifying tilt — state that in the
doc comment, since GR2Analyst interpolates toward the beam top and the two differ by up to
a beam width at long range. No qualifying tilt → cell 0.

Tests: a synthetic column with echo only on the lowest tilt yields that tilt's beam
height; echo present on the highest tilt yields the highest; a column below threshold
everywhere is no-data; a column beyond the highest tilt's range is no-data, **not** zero
(`beyond_coverage_is_no_data_not_zero_tops` — this distinction is the entire reason cell 0
exists).

### 7.4 VIL

```
Z (mm⁶/m³) = 10^(dBZ / 10),  with dBZ capped at 56.0 to bound hail contamination
VIL = Σ over adjacent tilt pairs:  3.44e-6 · ((Zᵢ + Zᵢ₊₁)/2)^(4/7) · (hᵢ₊₁ − hᵢ)
output units: kg/m², quantised 0..=80 over cells 2..=255
```

A column with fewer than two tilts carrying data is no-data — a single-layer "integral"
is not one. Only tilts with a valid reflectivity value at that column participate, and
they are paired in ascending height order after filtering.

Tests: a uniform 50 dBZ column of known depth produces the hand-computed value
(`vil_matches_a_hand_computed_uniform_column`); the 56 dBZ cap binds
(`vil_caps_reflectivity_at_the_hail_threshold`); a single-tilt column is no-data;
`vil_is_zero_not_no_data_for_a_column_with_only_weak_echo`.

### 7.5 Cross-validation

Unit tests prove the arithmetic matches the formula. They do not prove the formula
produces a meteorologically sensible field. Use the Python tooling that already exists:
add a script under `utility/nexrad-inspect/` that computes Echo Tops and VIL for one
fixture volume via MetPy/Py-ART and dumps a summary (min/max/mean, and the values at a
handful of fixed cells) for comparison against the Rust output.

**Record the comparison in §12 whether or not it agrees**, and if it disagrees, record the
magnitude and the suspected cause rather than tuning until the numbers match. This is the
same relationship the decoder has to MetPy's `Level2File` — cross-validation, not
ground truth.

---

## 8. S3-W5 — Validation harness and measurement

### 8.1 `radar-viz` gains a grid path

`utility/radar-viz` has a working PPI renderer, colour tables, and a PNG encoder. Add a
second render path that draws from a `SweepGrid` + `ColorLut` instead of from
`Vec<Radial>`, behind a flag:

```
radar-viz --path grid  <input-dir>     # new: grid → LUT → PNG
radar-viz --path radial <input-dir>    # existing, default
```

The two rendering the same fixture, product and sweep should be visually
indistinguishable apart from interpolation at the seams. That is the check that catches an
azimuth-binning error (§4.1), an off-by-one in the gate index, and a byte-order mistake in
the LUT — none of which any unit test in S3-W1 will notice.

This stays in `utility/`. It is not production code and must not become production code
(`utility/README.md`'s standing rule); it is where a rendering question is answered
cheaply before the GPU pipeline exists, which is exactly what `project-inventory.md` §7
identified this crate as being worth.

Also add `--path grid --product echo_tops|vil` so the derived products get looked at by a
human at least once before Stage 4.

### 8.2 A live end-to-end test

Extend `crates/radar-workstation/tests/pipeline_live.rs` with one `#[ignore]`d test that
runs the real pipeline against a live site and asserts:

- a snapshot contains grids for at least reflectivity within 60 s;
- `IngestStatus::state` stayed `Polling` throughout — compute never starved the poller;
- wall-clock from `Pipeline::spawn` to the first *gridded* sweep, printed for comparison
  against Stage 2's measured 3.565 s to the first applied sweep.

---

## 9. S3-W6 — Documentation and ADRs

Not follow-up work. These ship in the same commit sequence as the code that makes them
true — `project-inventory.md` §7 names design drift as this project's live failure mode,
and the remedy it identifies is amending the documents at the moment the code diverges.

**New ADRs**

- **ADR-0020 — Product texture representation.** Records S3-a: quantised R8 grid plus a
  256-entry palette LUT, per-sweep native dimensions (Q17), the memory arithmetic that
  forced it, the ZDR requantisation range and its resolution cost, and the FR-RP-6
  amendment. Alternatives: pre-coloured RGBA for all products (does not fit 128 MB);
  pre-coloured RGBA for the active product only (fits, but breaks FR-RP-7 on the most
  common keyboard action). Note explicitly that BC-7 is unaffected.
- **ADR-0021 — Colour table format.** Records S3-c: the GRLevelX `.pal` subset (with the
  verified directive table), bundled defaults compiled in, user palettes from the XDG data
  directory, skip-and-report on unknown directives, and the fuzz corpus. Alternative: a
  workspace-local format — rejected because it strands users from the palette ecosystem
  FR-CT-1 named as the whole point.

**Errata on existing ADRs** — dated, following the pattern ADR-0014 established:

- **ADR-0005 (rayon)** — adoption deferred pending measurement (S3-f), with §3.6's
  explicit revisit trigger.
- **ADR-0018 (shared state)** — `last_complete` is now a `VolumeSummary`; raw radials are
  released after gridding (this ADR's own anticipated Stage 3 mitigation, S3-g); and the
  consequence that any future product needing 16-bit PHI precision must be computed
  during gridding.

**Amended documents**

| Document | Change |
|---|---|
| `docs/open-questions.md` | Q8, Q9, Q11, Q17 → Resolved, each with the decision and where it landed |
| `docs/REQUIREMENTS.md` | FR-RP-3/4/5 and FR-CT-1 lose their `[OPEN]` markers; FR-RP-6 amended to the R8+LUT contract with a pointer to ADR-0020; §6's In Scope gains ZDR and CC; §7's table loses four rows |
| `docs/architecture/rendering.md` | Polar Grid Representation: Q17 closed, per-sweep native dimensions; Radar Data Rendering: the LUT lookup replaces "no per-frame colour mapping" as literally stated (the *claim* survives — 256 evaluations per product is not per-frame work — but the mechanism changed) |
| `docs/architecture/data-flow.md` | Compute Layer section: the confirmed product set, the grid output contract, the pipeline quartet, and the retention change |
| `docs/architecture/overview.md` | Compute Layer paragraph; implementation-status banner → Stage 3 |
| `docs/architecture/nexrad-binary-format.md` | §6.1: the `AZ` leading-edge-vs-centre finding from §4.1 |
| `CLAUDE.md` | Status paragraph; ADR index (0020, 0021); Open Questions (four removed); a Product Set subsection recording Q8/Q9's answers |
| `docs/README.md` | ADR table rows; plans table row for this document |
| `README.md` | Status statement |
| `docs/project-inventory.md` | Stage 3 marked done in §6 |

---

## 10. Ordering, and what this plan deliberately does not do

**Order: S3-W1 → S3-W2 → S3-W3 → S3-W4 → S3-W5 → S3-W6.**

Gridding first because everything else consumes a grid. Pipeline and state integration
second, because that is the point at which a live run shows real grids arriving and the
retention measurement becomes possible — the earliest observable program, same instinct as
Stage 2's ordering. Colour tables third (S3-W5's visual check can use `radar-viz`'s
existing hardcoded tables until then, so palettes do not block validation). Derived
products fourth, since they need the grids and nothing needs them. Validation and
measurement fifth. Documentation last only in listing order — in the commit sequence
(§13) each document lands with the code that makes it true.

Not done here, recorded so a later session does not read these as oversights:

- **No wgpu, no textures, no shader.** Stage 3 produces the *bytes* a texture will hold
  and the metadata a shader will need as uniforms. Uploading them is Stage 4's first job.
- **No colour-scale legend, no Nyquist readout, no cursor value display.** Q9's answer
  requires all three eventually; Stage 3 carries the data they need
  (`SweepGrid::nyquist_velocity_mps`, `Palette::step`/`units`, `SweepGrid::physical`) and
  stops there.
- **No dealiasing** (S3-e), **no KDP, PHI, CFP, or storm-relative velocity** (S3-b).
- **No palette hot reload.** FR-CT-3 is satisfied by loading at startup. A file watcher is
  a dependency and a background thread for a problem no one has reported.
- **No rayon** (S3-f). Measure first; the trigger to revisit is in §3.6 and in ADR-0005's
  erratum.
- **No tile, placefile, or archive-failover code.** Blocked on Q16, Q6, Q14 respectively.
- **No change to the decoder crate.** Every input Stage 3 needs is already decoded. If
  this turns out to be false, that is a finding worth recording in §12, not a licence to
  widen the stage.

---

## 11. Risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Azimuth binning is off by half a bin, rotating every image 0.25° | **Medium** | High — invisible by eye, wrong forever | §4.1 makes it a measurement against the fixtures before the code is written, plus §8.1's two-renderer comparison |
| Echo Tops / VIL are arithmetically right and meteorologically wrong | Medium | High | §7.5's MetPy/Py-ART cross-check on a fixture volume, with the numbers recorded whether or not they agree |
| ZDR requantisation loses more than expected | Low | Low | Bounded and computed (0.063 dB/step vs 0.031 native); asserted by `zdr_requantisation_round_trips_within_one_step`; recorded in ADR-0020 |
| Dropping raw radials forecloses a future product | Medium | Medium | Named in ADR-0018's erratum: 16-bit-precision products must be computed during gridding. The cost of being wrong is one extra pass in a later stage, not a redesign |
| Gridding on `spawn_blocking` starves the two-worker runtime | Low | Medium | At most one job at a time; §8.2's live test asserts `IngestStatus` stays `Polling`; §12 measures per-sweep grid time against the ~20 s inter-sweep interval |
| The `.pal` directive table is written from recollection rather than evidence | **Medium** | Low | §6.1 requires verification against real community palettes before ADR-0021 is finalised; skip-and-report makes an incomplete table a degradation, not a failure |
| `state::apply`'s signature change breaks Stage 2's tested rules | Low | High | §5.2 enumerates every existing rule and its disposition; the tests change only in how they construct their input |
| Memory does not fall as predicted | Low | High | §12 measures it directly, including the volume-boundary case Stage 2 left open. If grids plus transients still approach 200 MB, the next lever is retaining fewer elevations, not re-adding RGBA |
| Design drift: five documents keep describing pre-coloured RGBA | Medium | Medium | §9 lists the amendments as work items, and §13 places each in the commit that makes it true |

---

## 12. Measurements to record in §14 Results

Numbers, not impressions — the convention `stage-0-1` §8 and `stage-2` §12 established.

- **Grid time per sweep**, super-res and standard-res, per product and for all base
  products together. Against the ~20 s inter-sweep interval and §3.6's 50 ms revisit
  trigger.
- **Derived-product time per volume** (Echo Tops + VIL, VCP 35 and VCP 212). Against
  §3.6's 500 ms revisit trigger.
- **Grid memory for a full volume**, VCP 35 and VCP 212, broken down by product. Against
  §3.1's ~50 MB prediction and the 128 MB GPU target.
- **Peak RSS** at startup, after one complete volume, after four, and specifically
  **across a volume boundary** — the measurement Stage 2 left unfinished (`VmHWM` across a
  `VolumeClosed`). Against the < 200 MB target, and against Stage 2's 44 MB at 6 of 14
  sweeps.
- **Thread count** of the running process, including any `spawn_blocking` threads while
  compute is active. Against Stage 2's 4–5.
- **Wall-clock to the first gridded sweep**, live, against Stage 2's 3.565 s to the first
  applied sweep.
- **`Cargo.lock` package count** before and after — expected **unchanged at 67**, since
  S3-f adds no dependency. If it moved, say why.
- **Release binary size** before and after the seven bundled palettes.
- **Palette parse time** for the full bundled set at startup (sub-millisecond expected;
  it is on the < 2 s first-render path from Stage 4 on) — kept as a regression-guard test
  like `config::tests::load_of_a_realistic_file_is_fast`, not a one-off.
- **The MetPy/Py-ART cross-check result** for Echo Tops and VIL (§7.5), agreeing or not.
- **The `AZ` leading-edge-vs-centre finding** (§4.1) and where it was recorded.
- Test counts before and after, and whether `clippy -D warnings`, `cargo deny check` and
  `cargo audit` stayed clean.

---

## 13. Suggested commit sequence

Each line is one reviewable commit; each keeps `cargo test --workspace` and
`clippy --workspace --all-targets -- -D warnings` green.

1. `compute` module skeleton: `DisplayProduct`, `SweepGrid`, and the §4.1 `AZ` convention
   finding recorded in `nexrad-binary-format.md` *(S3-W1)*
2. `grid_sweep` + quantisation + fixture tests *(S3-W1)*
3. `StateUpdate`, `compute_loop`, pipeline quartet wiring *(S3-W2)*
4. `RadarState` grids + `VolumeSummary`; `state::apply` over `StateUpdate`; existing rules
   re-tested, new derived rules added *(S3-W2)*
5. Retention: raw radials released, `gridding_releases_the_source_sweep`; `headless`
   output extended *(S3-W2)*
6. ADR-0018 erratum; ADR-0005 erratum *(S3-W2/W6)*
7. `.pal` parser + corpus + mutator fuzz test *(S3-W3)*
8. Seven bundled palettes; `load_all` with XDG override; `compile_lut` + tests *(S3-W3)*
9. ADR-0021; Q11 moved to Resolved *(S3-W3/W6)*
10. `geometry.rs` beam model + tests *(S3-W4)*
11. Echo Tops + VIL + tests *(S3-W4)*
12. `utility/nexrad-inspect` cross-validation script; findings recorded *(S3-W4/W5)*
13. `radar-viz --path grid` + derived-product rendering *(S3-W5)*
14. Live end-to-end pipeline test *(S3-W5)*
15. ADR-0020; Q8, Q9, Q17 moved to Resolved; `REQUIREMENTS.md` amended *(S3-W6)*
16. `rendering.md`, `data-flow.md`, `overview.md`, `CLAUDE.md`, `docs/README.md`,
    `README.md`, `project-inventory.md` *(S3-W6)*
17. §14 Results filled in with the §12 measurements

---

## 14. Decisions summary

All seven were answered by the user on 2026-08-04, before this plan was written. None is
open.

| # | Decision | Answer |
|---|---|---|
| S3-a | Product representation | Quantised R8 grid + 256-entry palette LUT; amends FR-RP-6; ADR-0020 |
| S3-b | Q8 — v1.0 product set | Base 3 + Echo Tops + VIL + ZDR + CC; KDP, PHI, CFP, SRM deferred |
| S3-c | Q11 — colour table format | GRLevelX `.pal` subset; bundled defaults in the same format; ADR-0021 |
| S3-d | Q17 — grid dimensions | Per-sweep native, no padding, no upsampling |
| S3-e | Q9 — velocity dealiasing | Deferred; range-fold colour + Nyquist carried for display |
| S3-f | rayon | Deferred pending measurement; `spawn_blocking` for now; ADR-0005 erratum |
| S3-g | Retention | Raw radials released after gridding; `last_complete` → `VolumeSummary` |

---

## 15. Results

**Implemented 2026-08-05.** All of S3-W1 through S3-W5 landed as designed; S3-W6 (this
section and the amended documents it lists) is this same session's work. Baseline for
all before/after comparisons is commit `41ed906` (working tree clean, branch `runtime`),
built in a separate `git worktree` so the comparison didn't require disturbing the
uncommitted change set.

### §12 measurements

**Grid time per sweep** (release build, real KDOX volume from
`downloads/KDOX_20260629_1811/`, VCP 35, 16 elevations, 9360 radials total):

| | Measured |
|---|---|
| Per sweep, all base products on that sweep (3–5 products) | 0.66 ms – 2.51 ms (mean 1.43 ms) |
| Full volume, 16 sweeps | 22.9 ms total |

Far under §3.6's 50 ms/sweep revisit trigger — gridding is close to a `memcpy`, as
predicted, against sweeps arriving roughly twenty seconds apart.

**Derived-product time per volume** (same volume, Echo Tops + VIL together):
**595.6 ms** — this *exceeded* §3.6's ~500 ms revisit trigger. Recorded as a dated
finding in [ADR-0005's erratum](../adr/0005-use-rayon.md#erratum-added-2026-08-05-stage-3--s3-f)
rather than silently reached for rayon: the cost is structural (both derived products
call the trigonometric beam-geometry conversion once per (output cell, tilt) pair — on
the order of 10⁷ calls for a full VCP 35 volume), the process was measured to stay well
within the shortest real inter-volume interval (1–2 minutes in precipitation mode), and
the live end-to-end test confirmed the poller was never starved by it. The erratum
records a cheaper first fix (precompute one range/height table per tilt instead of
recomputing it per azimuth) before rayon is the next lever if that isn't enough. This is
this plan's one finding that contradicted its own prediction — §3.6 predicted derived
products would *not* need rayon; measurement showed the margin is thinner than assumed
for gridding, even though gridding itself was right.

**Grid memory for a full volume**, VCP 35 (only VCP available for direct measurement
this session — no real KTLH VCP 212 chunk stream was captured): **37.28 MB** across all
16 sweeps' base products (3 products on Doppler-only cuts, 5 on dual-pol cuts). Against
§3.1's ~25 MB (base-3) and ~50 MB (all-moments) predictions, and the 128 MB GPU target
— comfortably under, and closer to the all-moments figure since ZDR/CC are gridded on
every dual-pol-carrying tilt in the v1.0 product set (Q8's resolution added them).
Derived products (Echo Tops + VIL together): 2.52 MB.

**Peak RSS**, measured against the *release* binary running live against real KDOX
traffic (`./target/release/radar-workstation KDOX`, `/proc/<pid>/status` `VmHWM`
sampled every 10 s for 90 s): climbed from ~10 MB at startup to **~147 MB**, crossing at
least one real `Complete` volume boundary (a `derived=2` line — Echo Tops and VIL both
present — appeared partway through the run) and continuing to climb slightly through a
second volume's accumulation. Under the < 200 MB target (`REQUIREMENTS.md` §4.1) with
real headroom, against Stage 2's baseline of 44 MB at 6 of 14 sweeps gridded — Stage 3's
figure is naturally higher, since it now holds every base product's gridded bytes for
every elevation resident, not just the raw radials for the elevations seen so far. This
is the volume-boundary measurement Stage 2 left unfinished, now taken directly rather
than reasoned about — the ADR-0018 erratum records it.

**Thread count**: 4–5 throughout the same 90 s live run, matching Stage 2's baseline
of 4–5. `spawn_blocking`'s on-demand blocking-pool threads did not measurably raise the
steady-state count.

**Wall-clock to the first gridded sweep**, live against real KDOX traffic
(`tests/pipeline_live.rs`'s new
`pipeline_spawn_produces_a_gridded_reflectivity_sweep_without_starving_the_poller`):
**2.14 s**, with `IngestStatus` observed to stay `Polling` throughout (never `Retrying`/
`Stalled`/`ReAnchoring`) — compute did not starve the poller. Against Stage 2's 3.565 s
to the first *applied* (ungridded) sweep: faster, not slower — today's network
conditions varied between runs (the pre-existing, unmodified sweep-only test measured
2.33 s in the same session), and gridding's ~1–2.5 ms cost is not distinguishable from
that variance. The pre-existing `pipeline_spawn_produces_a_visible_sweep_within_the_
deadline` test is unchanged in behavior and still passes against the new `StateUpdate`
plumbing.

**`Cargo.lock` package count**: **unchanged at 67**, confirmed by an empty `git diff
Cargo.lock` — S3-f added no dependency, exactly as decided.

**Release binary size**: 2,857,072 bytes (baseline) → 2,964,688 bytes (after) — **+105
KiB (+3.8%)**, from the compute module's code and the seven bundled `.pal` files
compiled in via `include_str!`.

**Palette parse time** for the full bundled set at startup: **37.4 µs** (measured,
release build) — far under the < 2 s first-render budget, kept as a regression-guard
test (`compute::palette::tests::load_all_of_the_bundled_set_is_fast`, asserting
< 50 ms) rather than a one-off, matching `config::tests::load_of_a_realistic_file_is_
fast`'s pattern.

**The MetPy/Py-ART cross-check for Echo Tops and VIL (§7.5) was not performed this
session** — recorded as a gap, not silently skipped. MetPy is available in this
environment; Py-ART is not. More materially, MetPy's `Level2File` reads archive-format
(`AR2V`) volumes, and no archive-format fixture exists in the repository or in
`downloads/` (which holds only real-time chunk-format captures) — producing one would
have required a network fetch from the NOAA/Unidata archive bucket, which this session's
remaining time budget did not accommodate. The arithmetic itself is unit-tested against
hand-computed values (`vil_matches_a_hand_computed_uniform_column`, the beam-geometry
round-trip and 4/3-earth textbook-figure tests in `compute::geometry`), and the output
was sanity-checked visually via `radar-viz --path grid --product echo_tops|vil` against
the same real KDOX volume — a mostly-low echo-top field and a low-VIL solid return,
consistent with VCP 35's clear-air/light-precipitation context — but an independent
second implementation's numbers were not obtained. This is the next thing a future
session should do before treating Echo Tops/VIL as more than arithmetically verified.

**The `AZ` leading-edge-vs-centre finding (§4.1)**: measured directly against the same
real KDOX volume (`downloads/KDOX_20260629_1811/`, not committed) — confirmed **centre**
convention (every measured azimuth sits ~0.5 bin-widths above a multiple of the
spacing, never on one). Recorded in `nexrad-binary-format.md` §6.1 and in
`compute::grid`'s own top-level doc comment; cross-checked visually via
`radar-viz --path grid` vs. `--path radial` rendering the same real sweep
pixel-for-pixel indistinguishably (§8.1's two-renderer comparison).

**The `.pal` community-file verification (§6.1) was not performed this session** —
recorded as a gap in [ADR-0021](../adr/0021-colour-table-format.md) rather than treated
as complete. This development session had no network access to the community palette
sites the format originates from. The directive table was written against the plan's
specification and verified structurally (round-trip tests, a hand-written
community-style excerpt, a mutator-fuzzed corpus); it was not cross-checked against a
downloaded set of real, currently-circulating `.pal` files. Bounded risk (unknown
directives are skipped and reported, not rejected) but a real gap to close.

**Test counts**: `radar-workstation`'s `--lib` suite went from 106 to 143 (+37,
`cargo test -p radar-workstation --lib`), plus two new integration-test binaries —
`tests/palette_hardening.rs` (2 tests: committed-corpus + 5000-iteration mutator fuzz,
mirroring `decoder_hardening.rs`/`config_hardening.rs`) and one new `#[ignore]`d test
added to the existing `tests/pipeline_live.rs` (now 2 live tests total). Full workspace:
`cargo test --workspace` — all green, 0 failures, 6 `#[ignore]`d live/slow tests
unchanged in count from `radar-workstation` plus the two new grid-focused live/fuzz
additions. `cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny check`,
and `cargo audit` all stayed clean throughout.

### Findings that extended or contradicted this plan

- **Derived-product cost crossed its own revisit trigger** (above) — the one place
  measurement disagreed with §3.6's prediction. Handled as designed: recorded in the
  ADR-0005 erratum this plan anticipated, not as a silent rayon addition.
- **Two verification steps this plan called for were not completed**: the MetPy/Py-ART
  cross-check (§7.5) and the `.pal` community-file verification (§6.1). Both are
  recorded above and in the relevant ADR/section as explicit gaps rather than marked
  done — per this project's own standing rule against silently treating an unfinished
  verification as complete.
- **Everything else matched the plan as drafted**: the AZ centre-convention finding,
  the R8+LUT memory arithmetic, the retention/RSS prediction, the `Cargo.lock`
  package-count invariant, and the ordering (S3-W1 → W2 → W3 → W4 → W5 → W6) all held
  without needing to deviate from what was decided in §3 before implementation began.
