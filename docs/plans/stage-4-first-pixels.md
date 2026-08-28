# Implementation Plan — Stage 4: First Pixels

**Status:** Drafted — not yet implemented
**Drafted:** 2026-08-28
**Implements:** `docs/project-inventory.md` §6, Stage 4 (items 14–18)
**Baseline commit:** `bc040f6` (working tree clean, branch `compute_layer`)
**Toolchain:** rustc 1.95.0 / cargo 1.95.0, `x86_64-unknown-linux-gnu`
**Predecessors:** `docs/plans/stage-0-1-close-the-acquisition-path.md` (§8 Results),
`docs/plans/stage-2-make-the-application-exist.md` (§12 Results),
`docs/plans/stage-3-compute-layer.md` (§15 Results)

This plan is written to be executed in a later session. It carries every decision already
taken so implementation does not need to re-derive them from the ADRs or re-open the
questions this plan closes. **The four open decisions were put to the user and answered
before this plan was written** — §3 records them and the reasoning. There are no
outstanding `DECIDE` items; where the implementer must *measure* something before
choosing a value, that is called out explicitly as a measurement task, not as an open
decision.

**Scope boundary:** this plan puts the radar image on screen. At the end of it,
`cargo run --release -- KDOX` opens a window, draws every gridded product in azimuthal
equidistant projection over range rings and a site marker, pans and zooms without ever
losing the operator's spatial context, switches product and sweep by keyboard as a pure
GPU state change, reads out range/azimuth/height/value under the cursor, and surfaces
every error the pipeline has been reporting to stderr since Stage 1 in a status bar. It
draws **no map underlay, no tiles, and no placefiles** — those are Stages 5 and 6, gated
on Q15 and Q16, both still open. It does not change the active site at runtime — that is
Stage 7.

Stage 4 opens **no** new open questions and closes none of the four outstanding ones
(Q5, Q6, Q7, Q12–Q16). It adds two ADRs and amends `rendering.md` in one place.

---

## 1. What "done" means

| Claim | How it is demonstrated |
|---|---|
| The application has a window and stays responsive before any data arrives | `cargo run --release -- KDOX` presents a frame with the loading indicator well before the first sweep lands; measured against NFR-UX-4's "interactive before the first scan" in §14 |
| Radar data is drawn in azimuthal equidistant projection, centred on the site | An offscreen render of a committed KDOX fixture sweep is compared pixel-for-pixel against `utility/radar-viz`'s already-validated CPU grid renderer (§11.2) |
| Colour mapping is one LUT lookup per pixel, never per gate, never per frame | The fragment shader contains exactly one `lut[cell]` index; no colour arithmetic exists anywhere in `render/` |
| Below-threshold data is transparent (FR-DR-4) | The palette's `ND:` entry is `α = 0`, alpha blending is enabled, and a named test asserts a no-data cell leaves the background colour untouched in a read-back render |
| Product and sweep switching are GPU state changes (FR-RP-7) | A test counts texture uploads across a product switch and a sweep switch and asserts **zero** |
| The operator's spatial context is inviolable (FR-NI-4, NFR-UX-2) | `ViewState` is unreachable from any function that takes a `StateSnapshot`; a named test applies a synthetic sequence of state changes and asserts `ViewState` is unchanged bit-for-bit |
| Every error path written in Stages 1–3 reaches the user (NFR-ST-3) | The status bar renders `AppState::recent_events`; `IngestState::{Retrying, Stalled, ReAnchoring}` each render distinctly |
| The fold limit Q9 promised to state is stated | The velocity legend shows ±Nyquist from `SweepGrid::nyquist_velocity_mps` and a labelled `RF` swatch |
| Four instances are still lightweight (Principle 3, NFR-P-1) | Idle CPU and frame count measured for 1 and 4 instances in §14; idle instances render on a 2 Hz tick, not at 60 fps |
| Nothing regressed | `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny check`, `cargo audit` all clean; the existing live pipeline tests still pass unchanged |
| The dependency step is recorded, not absorbed silently | `Cargo.lock` package count before/after, the `deny.toml` allowlist expansion with a per-entry justification, and a dated addendum in `docs/dependency-inventory.md` |

**Requirements closed or advanced:** FR-DR-1 (closed), FR-DR-2 (closed), FR-DR-4
(closed), FR-DR-5 (closed subject to §14's measurement), FR-DR-6 (closed), FR-DR-7
(closed), FR-NI-1 (closed), FR-NI-2 (closed), FR-NI-3 (closed), FR-NI-4 (closed),
FR-RP-1 (closed — displayed, not merely computed), FR-RP-2 (closed — displayed),
FR-RP-7 (closed and now demonstrated, not merely asserted by construction), FR-CT-2
(closed — bundled palettes are visible), FR-CP-1 (advanced — window geometry and active
product persist; placefile URLs and tile provider have no subsystem yet), NFR-ST-3
(closed for every event type that exists today), NFR-UX-1 (closed for product, sweep,
and navigation; site selection is Stage 7), NFR-UX-2 (closed), NFR-UX-4 (closed subject
to §14).

**Not closed, deliberately.** FR-DR-3's compositing order is only demonstrable for
layers 1, 6, and 8 — layers 2–5, 7, and 9 have no data source until Stages 5 and 6.
FR-SS-2 and FR-SS-3 (runtime site change, clickable markers) stay in Stage 7; Stage 4
draws the *active* site's marker only. FR-DA-6, FR-MU-1/2/4/5/6, FR-PF-* are untouched.

---

## 2. What Stages 0–3 left that this plan builds on

Read `stage-3-compute-layer.md` §15 before starting. Eight of its outcomes shape the
work here:

- **`AppState::snapshot()` is the only read API, and it is already the right shape.**
  It returns owned data — `Vec<DisplaySweep>` (each holding `Vec<Arc<SweepGrid>>`),
  `Vec<Arc<SweepGrid>>` for derived products, `Option<VolumeSummary>`, `revision: u64`,
  and a cloned `IngestStatus`. The render loop calls it once per frame and holds no
  lock. Nothing in `state/` needs restructuring for Stage 4.
- **`revision: u64` exists for exactly this stage** (ADR-0018): compare against the last
  value uploaded to the GPU and skip re-upload when unchanged. FR-DR-5's "no perceptible
  drop on new scan arrival" is built on it.
- **View state was deliberately left out of `AppState`** (ADR-0018, Q4). There is no
  type for the data pipeline to reach through even by mistake. §7 keeps it that way and
  adds the test that proves it.
- **`SweepGrid` already carries every shader uniform the radar pass needs**:
  `azimuth_count`, `gate_count`, `first_gate_m`, `gate_width_m`, `elevation_deg`,
  `scale`, `offset`, `nyquist_velocity_mps`. Q17's answer (per-sweep native dimensions,
  never padded, never upsampled) is what makes these uniforms rather than constants.
- **`compute::palette::compile_lut(&Palette, scale, offset) -> ColorLut` is delivered and
  tested**, and its doc comment explicitly names Stage 4 as the owner of the upload, the
  call site, and the deferred cache keyed on `(DisplayProduct, scale.to_bits(),
  offset.to_bits())`. §6.3 implements that cache. It is not optional: velocity's
  effective scale/offset vary per sweep, so one LUT per product is wrong.
- **`compute::geometry::{slant_range_and_height, ground_range_and_height}` already exist**,
  are round-trip tested, and are cross-checked against the textbook 4/3-earth figure.
  The radar shader's ground→slant conversion is the same closed form; the cursor
  readout's slant→height is a direct call. Neither is re-derived.
- **`compute::grid::azimuth_slot(azimuth_deg, azimuth_count)` is `pub` specifically so
  a renderer can reuse it** rather than re-deriving the centre-vs-leading-edge binning
  rule (which was measured, not assumed — see `compute::grid`'s top-level doc comment).
  The WGSL must implement the *same* rule: `floor(az / spacing)`, never `round`.
- **`utility/radar-viz`'s `render_grid_ppi` is a validated CPU implementation of exactly
  what the fragment shader must do**, already compared pixel-for-pixel against the
  radial-path renderer on real data (Stage 3 §8.1). It is the reference for §11.2's
  read-back comparison — the single highest-value test in this stage.

Three smaller facts worth having in front of you:

- `headless::run(&state)` is the one call `main` replaces (its own doc comment says so).
  §3.5 keeps it reachable behind a flag rather than deleting it.
- `EventLog` has no public reader — only `#[cfg(test)] event_log_len`. §9.1 adds one.
- `config::save(path, &[(String, String)])` is line-preserving and atomic, and
  `config::load` reports rather than fails. §10 adds two keys through those functions
  and adds no new failure mode.

---

## 3. Decisions taken before this plan

All four were put to the user and answered on 2026-08-28. They are recorded here in
full so implementation does not re-litigate them, and so the ADRs in §12 have a source.
§3.5–§3.8 are decisions this plan takes on its own authority; they are recorded here
rather than buried in a work item so a reviewer can disagree with them in one place.

### 3.1 (S4-a) The stack: winit + wgpu + egui-wgpu + egui-winit — not eframe

**Decision: own the event loop, the surface, and the swapchain directly.** winit drives
the event loop and the window; wgpu owns the device, queue, and surface; `egui-winit`
translates winit events into egui input; `egui-wgpu` renders egui's output into *our*
surface. ADR-0022 records this.

ADR-0002 and ADR-0003 fixed egui and wgpu but never said how they are hosted, and the
two documents that describe the frame — `rendering.md` and `overview.md` — both describe
a relationship eframe inverts. `rendering.md`: "two systems that share a window but not
a render pipeline"; "egui is drawn last, on top of wgpu output, every frame."
`overview.md`: "radar data rendered directly to GPU surface, bypassing egui's renderer."
Under eframe, the radar pass becomes a guest inside an `egui_wgpu::CallbackTrait`, and
the surface, present mode, and frame pacing sit behind eframe's abstraction — which is
precisely the frame pacing §3.3 needs to control. Roughly 400 lines of setup this
project owns is the cheaper side of that trade, and it is setup that is written once.

**Consequences that must be handled deliberately, not absorbed:**

- **Version pinning.** ADR-0003 already says "the project pins to a specific wgpu
  version and upgrades deliberately, not automatically." A wgpu/egui-wgpu version
  mismatch is the single most common build failure in this ecosystem. **Do not guess a
  version triple.** Pick the newest `egui` release, then read `egui-wgpu`'s and
  `egui-winit`'s own manifests for *that* release to get the exact `wgpu` and `winit`
  versions they depend on, and pin all four in `crates/radar-workstation/Cargo.toml` to
  those exact minor versions. Record the four versions in ADR-0022's table.
- **The dependency tree grows by roughly 3.5×** (67 packages today; expect ~230–260).
  This is the largest single dependency step the project will take, and NFR-SEC-2 makes
  it a decision rather than a consequence. ADR-0022 must state the number actually
  measured, not this estimate.
- **`deny.toml`'s licence allowlist must be expanded one entry at a time**, each with a
  comment naming the crate that requires it. Run `cargo deny check licenses`, add the
  reported licence, re-run, repeat. Do **not** paste in a broad allowlist. Expect
  `Zlib`, `BSD-2-Clause`, `CC0-1.0`, `Unicode-DFS-2016`, `BSL-1.0`, and font licences
  (`OFL-1.1`, `UFL-1.0`) from egui's bundled fonts. **If anything copyleft beyond
  `MPL-2.0` appears, stop and report it rather than allowing it** — that is an ADR-0009
  question, not a `deny.toml` edit.
- **`unsafe` (NFR-SEC-5, BC-9).** Modern wgpu creates a surface safely from a window
  that implements the raw-handle traits with a `'static` lifetime — hold the window in
  an `Arc<Window>` and pass a clone. Verify no `unsafe` block is required in `render/`;
  if one is, NFR-SEC-5 requires a comment saying why it is necessary and why it is
  sound, and §14 must record it.
- **`cargo audit` may surface an advisory somewhere in the new tree.** If so, record it
  in §15 and decide it explicitly. Do not silence it.

### 3.2 (S4-b) Sampling: a full-screen quad, inverse-mapped per pixel, slant-corrected

**Decision: one full-screen triangle; the fragment shader maps each pixel back to
(ground range, azimuth), converts ground range to slant range with the 4/3-earth model,
indexes the R8 grid with `textureLoad`, and does one LUT lookup.** ADR-0023 records
this and amends `rendering.md`.

`rendering.md` contains two sentences in tension: "the render loop draws the radar
texture as a full-screen quad" and "the polar grid is mapped to the azimuthal
equidistant projection coordinate space by the vertex shader." The first is what this
stage builds; the second gets a dated correction in the same change (the erratum
pattern this project uses everywhere else).

Why inverse mapping wins:

1. **The CPU version already exists and is validated against real data.**
   `utility/radar-viz/src/render_grid.rs` does exactly this arithmetic — screen pixel →
   range and azimuth → `azimuth_slot` → cell → LUT — and Stage 3 §8.1 already compared
   its output pixel-for-pixel against the independent radial-path renderer on a real
   KDOX sweep. Porting proven arithmetic to WGSL is a far smaller risk than inventing
   mesh geometry, and it gives §11.2 a read-back oracle for free.
2. **Cost is constant per frame and independent of grid size.** One triangle, one
   `textureLoad` and one LUT index per covered pixel. A 720×1832 grid and a 360×688 grid
   cost the same to draw.
3. **No mesh to rebuild when the active sweep changes.** A pre-projected polar mesh would
   need 720×1832 cells decimated and re-tessellated on every sweep switch —
   re-introducing exactly the per-scan CPU work ADR-0020's R8+LUT representation removed,
   and breaking FR-RP-7's "switching is a GPU state change."
4. **Nearest sampling is correct here, and inverse mapping gets it for free.** The grid
   cell *is* the raw NEXRAD value (ADR-0020); interpolating between cells 0 (no data)
   and 1 (range folded) and 200 (a real value) is meaningless. `R8Uint` +
   `textureLoad` makes filtering structurally impossible rather than merely unused.

**The slant correction is not optional.** The gate axis is slant range; the projection
axis is ground range. On the 0.39° first tilt the difference is sub-pixel at any useful
zoom, which is exactly what makes the approximation a trap: it is invisible in the
clear-air fixtures used during development and misplaces echo by kilometres on a 6.4°
cut at 150 km. Two trigonometric operations per pixel is nothing on a GPU. The formula
is the one already in `compute::geometry::slant_range_and_height`:

```
KE_A = (4/3) * 6_371_000            // metres
phi  = ground_m / KE_A
r    = KE_A * sin(phi) / cos(theta + phi)      // slant range, metres
```

with `theta = radians(elevation_deg)`, and a discard when `abs(cos(theta + phi))` is
near zero (unreachable at real WSR-88D geometry; guarded anyway, per Stability as
Ethics, exactly as the Rust version guards it).

### 3.3 (S4-c) Frame pacing: redraw on demand, plus a 2 Hz idle tick

**Decision: `ControlFlow::WaitUntil(now + 500 ms)`.** Redraw is requested on any input
event, on resize, on egui's own repaint request, and on an idle tick when either
`snapshot.revision` changed or the time-derived chrome text changed. `PresentMode::Fifo`
(vsync) so an interaction burst cannot spin the GPU past the display rate.

FR-DR-5 says "the steady-state render loop must target 60 fps." Principle 3 and NFR-P-1
say four simultaneous instances must not contend. Rendering 60 frames a second of an
unchanged image in four processes for the multi-hour sessions NFR-ST-4 describes serves
neither the operator nor the machine — what FR-DR-5 actually protects is that
*interaction* is smooth and that *a new scan does not stutter*. Both are measured in
§14 under sustained pan and sustained zoom; the requirement is met by measurement, not
by burning cycles when nothing is happening.

The idle tick (rather than a signalling path from the applier) is deliberate: it is one
constant instead of a channel that crosses the pipeline/render boundary, and it keeps
the time-derived readouts FR-DA-5 asks for — data age, "last update N s ago" — current
without any extra wiring. At 2 Hz a whole-second age readout is never more than half a
second stale, and an idle instance costs two `snapshot()` calls a second, which
ADR-0018 already priced at a handful of refcount bumps.

**One thing that must not be skipped:** egui requests its own repaints (tooltips,
animations, text-cursor blink) via `FullOutput::viewport_output[..].repaint_delay`.
Shorten the `WaitUntil` deadline to that value when it is smaller than the idle tick, or
egui widgets will feel dead. This is the most likely single defect in §5.

### 3.4 (S4-d) Extra scope: all four additions are in

The user selected all four candidates beyond the stage's floor:

- **Colour-scale legend with the Nyquist readout** (§9.2). Without it the operator
  cannot read a dBZ value off the image at all, and Q9's resolution — "deferred, with
  the fold limit stated" — is only honoured once something states it.
- **Range rings, azimuth spokes, and a site marker** (§8). Pure line geometry, no
  shapefiles, no Q15 dependency. Until Stage 5 the radar image otherwise floats on a
  dark background with no geographic reference, and a ring at a known range is how the
  projection scale gets verified by eye.
- **Cursor readout** — range, azimuth, beam height, and the value under the pointer
  (§9.3). `SweepGrid::physical` and `compute::geometry` already supply everything. This
  is the most direct expression of the Instrument Principle in the whole stage.
- **Window geometry and active product persisted to config** (§10). ADR-0018
  deliberately keeps view state out of `AppState`, so this is a render-loop-owned write
  at shutdown, not a state change.

### 3.5 (S4-e) `headless` is kept, behind `--headless`

**Decision: `main` branches once — `if args.headless { headless::run(&state) } else {
render::run(...) }`.** `headless.rs` is not deleted.

It is the only way to run the pipeline where there is no display or no GPU — a real
condition on a server, in a container, and in CI — and `tests/pipeline_live.rs` depends
on that path existing. Deleting it to "replace the placeholder" would trade a working
diagnostic for nothing. Its doc comment gets a dated update saying it is now a supported
mode rather than a placeholder.

**If the window or adapter cannot be created, exit non-zero with a message naming
`--headless`.** Do not silently degrade into headless mode: an operator who asked for a
radar display and got a scrolling log has been told something false about their machine.
Stability as Ethics cuts toward the honest failure here, not the quiet one.

### 3.6 (S4-f) The render code is a binary-side module tree

**Decision: `crates/radar-workstation/src/render/`, declared as `mod render;` in
`main.rs` alongside `mod cli;` and `mod headless;`.** No new crate, no cargo feature.

ADR-0010 puts only the decoder in a separate library crate; the render loop is
application code, not reusable library API, and it consumes the lib's public surface
(`AppState::snapshot`, `compute::*`) exactly as `headless.rs` already does. A `render`
cargo feature to spare `utility/radar-viz` the wgpu compile time was considered and
rejected: resolver-v2 feature unification means a workspace build enables it anyway, so
it would buy nothing in CI and add a configuration axis to every future build command.
`default-members` already scopes a bare `cargo build` to the three production crates.

Module layout:

```
src/render/
  mod.rs           winit ApplicationHandler, the frame, shutdown
  gpu.rs           instance/adapter/device/queue/surface setup and reconfigure
  view.rs          ViewState and every coordinate transform — pure, unit-tested
  radar.rs         radar pipeline, grid texture cache, LUT cache
  reference.rs     range rings, azimuth spokes, site marker
  ui.rs            egui chrome: status bar, legend, cursor readout, help overlay
  input.rs         winit event -> view/selection change — pure mapping, unit-tested
  time.rs          NEXRAD julian date + ms -> UTC civil time — pure, unit-tested
  shaders/radar.wgsl
  shaders/reference.wgsl
```

### 3.7 (S4-g) Spatial stability is enforced by ownership first, tested second

**Decision: no function that receives a `StateSnapshot` may receive `&mut ViewState`.**
`ViewState` is mutated only by `render::view` functions called from `render::input`.
This is the same technique ADR-0018 used to make "never hold a lock across a frame"
unrepresentable: a rule enforced by the type system rather than remembered by a later
contributor.

`project-inventory.md` §6's Stage 4 item 16 warns explicitly that FR-NI-4 is "far
harder to retrofit." That is why §7 lands *before* §6 in the ordering — the guarantee is
built in before there is a pixel that could tempt someone to recentre on new data.

### 3.8 (S4-h) The operator's selection is never silently rewritten

**Decision: when the selected (product, elevation) is absent from the current snapshot,
display nothing for it and say so in the status bar. Do not switch the selection.**

A split cut legitimately carries no velocity; a VCP change replaces the elevation set.
The tempting behaviour — quietly fall back to the nearest available product or tilt — is
the behaviour that makes an operator misread a screen during a warning, because the
status bar and the image would then disagree with what they pressed. The selection is
theirs. The one exception is initialisation, where nothing has been selected yet: the
first sweep to arrive sets the default (reflectivity, lowest available elevation).

---

## 4. S4-W1 — The window, the device, and the two-pass frame

### 4.1 Dependencies and gates

Add to `crates/radar-workstation/Cargo.toml` (versions determined per §3.1, **not
guessed**): `winit`, `wgpu`, `egui`, `egui-wgpu`, `egui-winit`.

Then, in order, and before writing any render code:

1. `cargo build --release` — confirm the four versions actually resolve together.
2. `cargo deny check` — expand `deny.toml`'s `[licenses].allow` one entry at a time
   per §3.1, each with a comment naming the requiring crate.
3. `cargo audit` — clean, or a recorded and explicitly decided finding.
4. Record the `Cargo.lock` package count (was **67**) and the release binary size (was
   **2,964,688 bytes**) for §14.

`[bans].multiple-versions` stays at `"warn"`. The new tree will produce duplications;
§14 records the count, and `docs/dependency-inventory.md`'s existing assessment posture
applies — a duplication is a finding to assess, not automatically a defect.

### 4.2 Window and event loop

winit 0.30+ `ApplicationHandler`, driven by `EventLoop::run_app`. The window is created
in `resumed`, held as `Arc<Window>` so the surface borrows nothing. Initial size comes
from config (§10), defaulting to 1280×800. Title: `"Radar Workstation — <SITE> (<name>)"`.

`main` keeps its current shape exactly. The runtime is still built explicitly and the
main thread is still reserved — `main.rs`'s existing comment already says this is why.
The winit event loop takes the main thread; the tokio runtime handle is moved into the
render app so `Pipeline::shutdown` can be awaited on the way out.

**Two instructions Stage 2 left in `main.rs` come due here.** `RUNTIME_WORKER_THREADS`
is 2, with a comment saying to "re-measure this constant at Stage 4 when a real render
loop is competing for cores" — do that, and record the result in §14 whether or not the
value changes. The same comment says "rayon gets its own pool at Stage 3, sized
separately," which is now stale (S3-f deferred rayon; ADR-0005 carries the erratum);
correct the comment in the same change rather than leaving it to drift.

Shutdown: `WindowEvent::CloseRequested` and `Ctrl+Q` both exit the event loop; `main`
then runs the existing `runtime.block_on(pipeline.shutdown())` and the config save
(§10). BC-8's "exits cleanly when the window is closed" is this path.

### 4.3 GPU setup (`render/gpu.rs`)

- Instance backends: `Backends::VULKAN | Backends::GL` — ADR-0003's "Vulkan primary,
  OpenGL ES fallback for older hardware," stated in code rather than left to the default.
- Adapter: `PowerPreference::LowPower`. Deliberate, and worth a comment: the workload is
  one full-screen triangle and some lines, and Principle 3's four-simultaneous-instances
  case is better served by the integrated GPU than by waking a discrete one four times.
- Device limits: request `Limits::downlevel_defaults()`. Its
  `max_texture_dimension_2d` is 2048; the largest measured grid dimension is **1832**
  gates (KDOX VCP 35, tilt 1), so it fits — but not by much. **Guard it:** at upload
  time, if `gate_count` or `azimuth_count` exceeds the device's actual
  `max_texture_dimension_2d`, report a new `Event` variant and skip that grid rather
  than letting wgpu's validation panic. NFR-ST-2 makes this mandatory, not defensive
  padding.
- Surface: format from `surface.get_capabilities(&adapter).formats[0]`,
  `PresentMode::Fifo`, `CompositeAlphaMode::Opaque`. Record whether the chosen format is
  sRGB — §6.4 depends on it.
- Reconfigure on `WindowEvent::Resized` and on `SurfaceError::{Lost, Outdated}`. On
  `SurfaceError::Timeout`, skip the frame. On `SurfaceError::OutOfMemory`, report and
  exit cleanly — do not loop. Each of these is a reachable error path under NFR-ST-1;
  none of them may `unwrap`.

### 4.4 The frame

```
snapshot = state.snapshot()                    // one call, no lock held after it returns
if snapshot.revision != last_uploaded_revision:
    sync_textures(&snapshot)                   // uploads only what changed (§6.2)
acquire surface texture
encoder:
  pass 1 (LoadOp::Clear(background)):
      radar quad          (layer 6)
      reference geometry  (rings, spokes, marker — layers "8-ish", see below)
  pass 2:
      egui                (layer 10)
submit; present
```

Two passes, one encoder, one submit. FR-DR-3 puts site markers at layer 8 and radar at
6, so reference geometry draws after the radar quad; the rings and spokes are chrome of
the same kind and draw with it. When Stages 5 and 6 add layers 2–5, 7, and 9, they slot
into pass 1 in FR-DR-3's order — the pass structure does not change.

---

## 5. S4-W2 — Frame pacing and redraw

Implements §3.3.

- `ControlFlow::WaitUntil(now + IDLE_TICK)`, `IDLE_TICK = 500 ms`.
- `window.request_redraw()` on: any keyboard or mouse event that changes view or
  selection; `Resized`; and on the idle tick when `snapshot.revision != last_drawn_revision`
  **or** the formatted chrome text differs from the last frame's (this is what makes the
  data-age readout tick).
- After running egui, take `full_output.viewport_output`'s `repaint_delay`; if it is
  shorter than the remaining idle interval, use it as the `WaitUntil` deadline instead.
  §3.3 flags this as the most likely defect in the stage — write it first, not last.
- Measure, do not assume: §14 records frame time percentiles under sustained pan and
  sustained zoom, and the idle frame rate for one and four instances.

---

## 6. S4-W3 — The radar pass

### 6.1 Texture representation

One `wgpu::Texture` per `(DisplayProduct, Option<elevation_number>)`:

- Format `R8Uint`, usage `TEXTURE_BINDING | COPY_DST`, sample count 1, mip level count 1.
- Size: **width = `gate_count`, height = `azimuth_count`.** `SweepGrid::cells` is
  azimuth-major (`cells[azimuth * gate_count + gate]`), so a row of the texture is one
  radial. The shader indexes `textureLoad(grid, vec2<i32>(gate, az_slot), 0).r`.
- Derived products (Echo Tops, VIL) use the same path with `None` for the elevation key;
  they carry the lowest reflectivity tilt's geometry (`compute::derived`'s
  `OutputGeometry`), so no special case is needed anywhere except the elevation key and
  the fact that their gate axis is **ground** range — see §6.5.
- No sampler. `textureLoad` with `R8Uint` makes filtering structurally impossible, which
  is the correct behaviour for a grid whose cell values 0 and 1 are sentinels
  (ADR-0020).

**Row stride is the trap here.** `gate_count` is 1832, 1192, or 688 — none is a multiple
of 256. Try `Queue::write_texture` with tightly packed rows first (`bytes_per_row =
gate_count`); wgpu handles staging internally and does not impose the 256-byte alignment
that `copy_buffer_to_texture` does. **If validation rejects it,** fall back to building
a padded staging buffer (`bytes_per_row` rounded up to the next multiple of 256, one
temporary allocation per upload, freed immediately). Either way §11.2's read-back
comparison is what catches a stride error — a sheared image is unmistakable there and
invisible in every unit test.

### 6.2 The texture cache and the upload rule

```rust
struct CachedGrid {
    source: Arc<SweepGrid>,   // held so Arc::ptr_eq is a sound identity test
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}
// key: (DisplayProduct, Option<u8 /* elevation_number */>)
```

Holding the `Arc` is what makes `Arc::ptr_eq` a valid "is this the same grid?" test — a
raw pointer comparison against a freed allocation could alias. It costs nothing: the
allocation is already owned by `AppState`.

`sync_textures(&snapshot)` runs only when `revision` changed:

1. For each `(product, elevation)` present in the snapshot, upload if there is no cache
   entry or the entry's `source` is not `Arc::ptr_eq` to the snapshot's.
2. Evict cache entries whose key no longer appears in the snapshot (a VCP change drops
   an elevation set; `state::apply` already handles that side).
3. Record the upload count for the frame — §11.1 asserts it is **zero** across a product
   or sweep switch, which is FR-RP-7's actual demonstration.

Every grid stays resident, per ADR-0020: Stage 3 measured **37.28 MB** for a full VCP 35
volume's base products plus **2.52 MB** derived, against the 128 MB GPU budget. §14
records the number this stage measures with the surface, LUTs, and egui atlas included.

### 6.3 The LUT cache

`compile_lut`'s doc comment already specified this and deferred it to Stage 4. Implement
it now:

- Key: `(DisplayProduct, scale.to_bits(), offset.to_bits())`. **One LUT per product is
  wrong** — velocity's effective scale/offset vary per sweep, and ZDR's are the
  requantised pair, not the ICD pair.
- Value: a `wgpu::Buffer` (uniform) holding `array<vec4<f32>, 256>` = 4096 bytes, well
  under the 64 KB uniform binding limit. A uniform buffer rather than a 256×1 texture:
  no sampler, no format/sRGB question on the LUT itself, and a trivially small upload.
- Palettes come from `compute::palette::load_all()` once at startup; its `Vec<Event>`
  goes to `AppState::report` exactly like config's does in `main` today.
- **Bound it.** Cap at 32 entries; on overflow, clear the cache and report a new `Event`
  variant, mirroring `RetainedGridSetBounded`'s pattern. An unbounded cache keyed on
  float bits in a process that runs for hours is the same memory leak with a friendly
  name that ADR-0018 named for the event log.

### 6.4 sRGB

Palette colours are sRGB bytes. If the surface format is `*_UNORM_SRGB`, the hardware
encodes the shader's linear output, so the LUT's RGB must be converted sRGB→linear
**once, on the CPU, at LUT compile time** (256 entries × 3 channels — free). Alpha stays
linear. If the surface format is not sRGB, upload the bytes as-is normalised. Decide
this once in `gpu.rs`, store the flag, and apply it in the LUT builder — not in the
shader, where it would run per pixel.

Getting this wrong produces a washed-out or over-saturated image that looks plausible.
§11.2's comparison against `radar-viz`'s PNG is what catches it.

### 6.5 The shader (`render/shaders/radar.wgsl`)

Vertex: a full-screen triangle from `vertex_index` alone — no vertex buffer.

Fragment, per pixel:

```
1. uv -> world offset from site, in metres:
      dx =  (frag.x - viewport.x/2) * m_per_px + center.x
      dy = -(frag.y - viewport.y/2) * m_per_px + center.y      // screen y is down, north is +y
2. ground = sqrt(dx*dx + dy*dy)
   az_deg = degrees(atan2(dx, dy)); if az_deg < 0 { az_deg += 360 }
      // atan2(x, y), 0deg = North, increasing clockwise — the SAME convention as
      // radar-viz's render_grid_ppi and render.rs. Do not "fix" it to atan2(y, x).
3. ground -> slant (§3.2's closed form); discard on the degenerate denominator
   For a derived product (echo tops, VIL) the gate axis IS ground range: skip step 3.
4. gate = i32(floor((slant - first_gate_m) / gate_width_m))
   discard (alpha 0) if gate < 0 || gate >= gate_count
5. spacing = 360.0 / f32(azimuth_count)
   slot = i32(floor(az_deg / spacing)) mod azimuth_count      // floor, NEVER round
6. cell  = textureLoad(grid, vec2<i32>(gate, slot), 0).r
   return lut[cell]
```

Step 5 must match `compute::grid::azimuth_slot` exactly. The centre-vs-leading-edge
question was **measured**, not assumed (see `compute::grid`'s top-level doc comment); a
`round` here silently rotates every image by a quarter of a bin. Put a comment in the
WGSL pointing at that doc comment.

Uniforms (one buffer, written once per frame): `center_m: vec2<f32>`, `m_per_px: f32`,
`viewport: vec2<f32>`, `azimuth_count: u32`, `gate_count: u32`, `first_gate_m: f32`,
`gate_width_m: f32`, `elevation_rad: f32`, `is_ground_range: u32`. Mind std140-style
alignment; keep the struct 16-byte aligned explicitly rather than relying on field order.

Pipeline: `TriangleList`, no depth buffer, no culling, blend `ALPHA_BLENDING`
(`SrcAlpha` / `OneMinusSrcAlpha`). FR-DR-4 falls out of the palette's `ND:` entry being
`α = 0` — no threshold logic in the shader.

---

## 7. S4-W4 — View state, input, and spatial stability

**Write this before §6.** The guarantee is cheaper to build in than to retrofit
(`project-inventory.md` §6, item 16), and the transforms are needed by the shader
uniforms, the reference geometry, and the cursor readout alike.

### 7.1 `ViewState` (`render/view.rs`)

```rust
pub struct ViewState {
    pub center_m: (f64, f64),      // offset from the site, metres; (0,0) = site
    pub m_per_px: f64,
    pub product: DisplayProduct,
    pub elevation_number: u8,
    pub show_reference: bool,
}
```

Everything here is owned by the render loop and appears nowhere in `AppState`
(ADR-0018). Defaults: reflectivity; the lowest elevation number present in the first
snapshot that has one; centred on the site; `m_per_px` set so **230 km fits the half-
extent of the smaller window dimension**.

Pure functions, each unit-tested:

- `screen_to_world(px, py, &ViewState, viewport) -> (f64, f64)`
- `world_to_screen((x, y), &ViewState, viewport) -> (f32, f32)`
- `zoom_about(&mut ViewState, cursor_px, factor, viewport)` — **the world point under
  the cursor must be unchanged after the call.** That invariant is what makes zoom feel
  right, and it is a one-line assertion.
- `pan_by_pixels(&mut ViewState, dx, dy)`
- `fit_range(range_m, viewport) -> f64`
- `on_resize(&mut ViewState, old_viewport, new_viewport)` — preserve the world point at
  the window centre **and** `m_per_px`. A resize reveals or hides area; it never
  rescales the image. This is FR-NI-4's "window resize" clause.

Zoom clamps, stated as named constants with the reasoning in a comment: minimum
`m_per_px` such that one 250 m gate spans about four pixels (~60 m/px — finer than that
displays quantisation, not data); maximum such that ~600 km fits the viewport (beyond
the longest measured tilt's range, so zooming further only adds background).

### 7.2 Input (`render/input.rs`)

Keep the winit event → intent mapping in a pure function returning an enum, so the key
map is unit-testable without a window:

| Input | Action |
|---|---|
| `1`–`7` | product: reflectivity, velocity, spectrum width, ZDR, CC, echo tops, VIL |
| `PageUp` / `PageDown` | next / previous elevation among those present, in angle order |
| Arrow keys | pan (one-eighth viewport per press; hold repeats) |
| `+` / `=` / `-` | zoom in / out about the viewport centre |
| `Home` | reset view to the default (site-centred, 230 km) — selection untouched |
| `R` | toggle reference geometry |
| `F1` or `?` | toggle the key-help overlay |
| `Ctrl+Q` | quit |
| Left-drag | pan |
| Wheel | zoom about the cursor |
| Motion | update the cursor readout |

NFR-UX-1 is satisfied for product, sweep, and navigation; site selection stays on the
Stage 7 list. NFR-UX-3 ("usable without documentation") is what the `F1` overlay is for
— it costs a dozen lines of egui and removes the need for a manual.

`PageUp`/`PageDown` for tilt and arrows for pan follows GR2Analyst; do not swap them.

### 7.3 The stability test

```
view_state_is_unchanged_by_any_sequence_of_state_updates
```

Build a `ViewState`, pan and zoom it to an arbitrary non-default position, apply a
synthetic sequence to an `AppState` (`SweepGridded` for several elevations, a
`DerivedComputed`, a `VolumeClosed`, a VCP change that drops the elevation set), and
assert every `ViewState` field is unchanged. Pair it with the structural rule from §3.7
— no function taking a `StateSnapshot` takes `&mut ViewState` — noted in `view.rs`'s
doc comment so the next contributor understands the test is guarding a boundary, not a
value.

---

## 8. S4-W5 — Reference geometry

A second wgpu pipeline, `PrimitiveTopology::LineList`, drawing in world (metre)
coordinates through the same view uniform. Built once at startup into a single vertex
buffer; pan and zoom are the uniform, exactly as `rendering.md` describes for vector
overlays.

- **Range rings** every 50 km out to 300 km, plus an emphasised ring at 230 km (the
  conventional Level II display range, and the number a reader can check by eye).
- **Azimuth spokes** every 30°, drawn from the innermost ring outward so they do not
  clutter the site.
- **Site marker** at the origin — a small cross, drawn slightly brighter.
- **Ring labels** ("50", "100", …, "230 km") drawn by egui at positions from
  `view::world_to_screen`, which keeps text out of the wgpu side entirely.
- Toggled by `R`; the toggle is `ViewState::show_reference`, so it persists across data
  updates like everything else in `ViewState`.

Line width is 1 px (`LineList` gives no control over it, and every backend supports it).
That is the right weight for reference chrome anyway — Instrument Principle: this exists
to let the operator read the radar image, not to be looked at.

---

## 9. S4-W6 — UI chrome (egui)

Dark visuals, no shadows, no rounding beyond egui's default, one accent colour. Chrome
is quiet.

### 9.1 Status bar (FR-DR-7, NFR-ST-3)

A bottom panel showing, left to right: site id and name · active product · elevation
number and angle (or "el N — no data on this cut" per §3.8) · scan time in UTC · data
age · ingest state · the most recent event.

Two small additions are needed to make this possible:

- **`AppState::recent_events(&self, max: usize) -> Vec<(Instant, String)>`** in
  `state/mod.rs`. `Event` is not `Clone`, so format through `Display` while briefly
  holding the mutex and return owned strings. Keep `event_log_len` for the existing
  tests. This is the reader `EventLog` was written for — its own doc comment says
  "its only consumer so far is NFR-ST-3's future status bar."
- **`render/time.rs`: `fn utc_from_nexrad(julian_date: u16, scan_time_ms: u32) ->
  (i32, u32, u32, u32, u32, u32)`** — NEXRAD modified Julian date is days since
  1970-01-01 with day 1 = 1970-01-01. Implement civil-from-days directly (Hinnant's
  algorithm is ~15 lines); **no new date dependency**, consistent with `paths.rs`
  declining `dirs` and `event.rs` declining a logging crate. Unit-test against a
  hand-computed value and against the KDOX fixture's known scan date.

`IngestState::{Polling, Retrying, Stalled, ReAnchoring}` each render distinctly;
`Stalled` and a non-empty `last_error` render in the accent colour. Data age comes from
`IngestStatus::last_success` and the displayed sweep's `received`.

### 9.2 Colour-scale legend (FR-CT-2, and Q9's promise)

A narrow vertical strip on the right for the active product: the palette sampled across
its threshold range, with labelled ticks at `Palette::step` (fallback: eight even
divisions) and `Palette::units` as the caption. For velocity, additionally show
**±Nyquist** from `SweepGrid::nyquist_velocity_mps` and a labelled `RF` swatch — this is
the specific thing Q9's resolution promised and Stage 3 could not deliver.

`Palette::entries` is private and there is no range accessor, so add a small additive
API in `compute::palette`:

```rust
pub fn threshold_range(&self) -> Option<(f32, f32)>   // first and last entry thresholds
```

and reuse the existing `sample()` for the strip's colours. Nothing else in `palette.rs`
changes.

### 9.3 Cursor readout

Follows the pointer (offset a few pixels so it never sits under it): ground range in km,
azimuth in degrees, beam height in kft from
`compute::geometry::ground_range_and_height`, and the value under the cursor from
`SweepGrid::physical` with the palette's units. Cell 0 renders as `ND`, cell 1 as `RF`,
spelled out — the two sentinels ADR-0020 preserved through gridding exist so that this
readout can distinguish them, and a blank would throw that away.

### 9.4 Loading indicator (FR-DR-6)

Centred, while `snapshot.sweeps.is_empty()`: the site being fetched and the current
ingest state, so the first two seconds of the application are informative rather than
blank. It disappears the moment the first sweep lands — which, per Stage 3's
measurement, is about 2.1 s after launch on a live stream.

---

## 10. S4-W7 — Config persistence

Three new keys, through the existing `config::load` / `config::save`:

| Key | Type | Validation |
|---|---|---|
| `window.width` | u32 | clamped to `[640, 7680]`; out of range → default + `ConfigValueInvalid` |
| `window.height` | u32 | clamped to `[480, 4320]`; same |
| `view.product` | string | parsed by `DisplayProduct::parse`; unknown → default + `ConfigValueInvalid` |

Pan, zoom, and elevation are deliberately **not** persisted. A restored stale zoom on a
different storm is worse than a known starting view, and FR-CP-1 does not ask for it.

`DisplayProduct` gains `pub fn parse(s: &str) -> Option<Self>` next to its existing
`Display` impl, with a round-trip test over `DisplayProduct::ALL` so the two cannot
drift — the `Display` strings (`ref`, `vel`, `sw`, `zdr`, `cc`, `echo_tops`, `vil`) are
already the user-facing names, and inventing a second set would violate DRY in the most
annoying possible way.

Save happens once, on clean shutdown, only for values that actually changed, only if
`config_path` resolved, through `config::save`'s line-preserving atomic path. A save
failure is reported and never fatal — same posture as everything else in `config`.

---

## 11. S4-W8 — Validation

### 11.1 Tests that run in CI (no GPU required)

- `view.rs`: screen↔world round-trip; `zoom_about` keeps the cursor's world point fixed;
  `on_resize` preserves centre and scale; zoom clamps hold at both ends.
- `input.rs`: the full key map, including that arrows pan and PageUp/Down change tilt.
- `time.rs`: `utc_from_nexrad` against hand-computed values and a fixture's known date.
- `render::view` stability test (§7.3).
- `compute::palette::threshold_range` and `DisplayProduct::parse` round-trip.
- The LUT sRGB conversion (pure function, compared against hand-computed values).
- **The texture-cache decision logic, extracted as a pure function** — given a previous
  key→`Arc<SweepGrid>` map and a new snapshot, return `(to_upload, to_evict)`. This is
  what makes FR-RP-7's zero-upload claim testable without a device: assert that a
  product switch and a sweep switch each produce an empty `to_upload`.
- Config: the three new keys, valid, invalid, and out of range.

### 11.2 The GPU tests (`#[ignore]`d, run manually)

1. **Adapter smoke test.** Request an adapter over `Backends::all()`; if none is
   available, skip rather than fail — the same posture as the live-network tests.
2. **Offscreen render vs. `radar-viz` — the highest-value test in this stage.** Decode a
   committed KDOX fixture into a `Sweep`, grid it, compile its LUT, render offscreen to
   an RGBA target at a known view (site-centred, 230 km across, 512×512), read it back,
   and compare against `render_grid::render_grid_ppi(&grid, &lut, 230.0, 512)`. Assert
   that at least 99% of in-range pixels match within a small per-channel tolerance.

   This single test catches, in one shot: an sRGB conversion error, a row-stride shear,
   an azimuth-convention rotation, a gate off-by-one, and a Y-flip — none of which any
   unit test in §11.1 would notice.

   **Use a low tilt (elevation 1, ~0.39°).** `render_grid_ppi` does not apply the
   slant-range correction; at 0.39° over 230 km the difference is sub-pixel at 512×512,
   so the comparison is valid there. Test the slant correction separately by asserting
   the WGSL's constants and formula against `compute::geometry::slant_range_and_height`
   at several (ground range, elevation) pairs — a numeric test, not a visual one. State
   this limitation in the test's doc comment rather than leaving a future reader to
   discover it.
3. **Transparency.** Render a grid whose cells are all 0 and assert the read-back is
   exactly the background colour (FR-DR-4).

CI runs neither 2 nor 3 — GitHub Actions has no GPU, and `ci.yml`'s existing comment
about `--ignored` tests applies unchanged. CI does still compile every line of the
render code, which is most of what CI was catching anyway. Note in `ci.yml` that the
GPU tests are run manually, in the same style as the live-network note already there.

---

## 12. S4-W9 — Documentation and ADRs

- **ADR-0022 — Window, event loop, and render-pass hosting.** §3.1: winit + wgpu +
  egui-wgpu + egui-winit, not eframe. Must include the four pinned versions, the
  measured `Cargo.lock` package count before and after, the `deny.toml` allowlist
  expansion with per-entry justification, and whether any `unsafe` was required.
- **ADR-0023 — Radar sampling in screen space.** §3.2: full-screen quad, per-pixel
  inverse mapping, slant-corrected, `R8Uint` + `textureLoad` + a 256-entry LUT uniform.
  Records the rejected alternatives (pre-projected polar mesh; skipping the slant
  correction) and their specific failure modes.
- **`docs/architecture/rendering.md`** — a dated correction on the "mapped ... by the
  vertex shader" sentence (the erratum pattern used throughout this project); a new
  frame-pacing subsection recording §3.3; the reference-geometry layer added to the
  layer table; the performance-target table annotated with §14's measured figures.
- **`docs/architecture/overview.md` and `data-flow.md`** — remove the "still
  architecture, not yet implemented" banners on the render loop; note that view state is
  now genuinely owned by `render::ViewState`.
- **`docs/REQUIREMENTS.md`** — update the status of every requirement listed in §1;
  keep the `[OPEN]` markers that remain open.
- **`docs/dependency-inventory.md`** — a dated addendum recording the new tree size, the
  licence expansion, and the duplication count. This is the document that tracks
  dependency posture; letting the largest dependency step in the project go unrecorded
  there is exactly the drift `project-inventory.md` §7 warns about.
- **`docs/project-inventory.md`** — extend the existing supersession banner to cover
  Stage 4, matching how Stages 2 and 3 were handled.
- **`CLAUDE.md`** — status paragraph, the `render/` module map, the ADR index, the
  `--headless` flag, and the keyboard map.
- **`README.md`** — usage, the keyboard map, and a screenshot.
- **`docs/open-questions.md`** — unchanged. Stage 4 opens and closes nothing.

---

## 13. Ordering, and what this plan deliberately does not do

Implement in this order. It is not arbitrary:

1. **§4 — deps, gates, ADR-0022, window, device, clear-to-background, `--headless`
   preserved.** Get the dependency and licence question answered before writing code
   that depends on the answer.
2. **§9 (chrome) — egui pass, status bar, loading indicator.** Before the radar pass,
   because from here on every subsequent step is debuggable on screen, and because every
   error path written in Stages 1–3 has been waiting since Stage 1 for somewhere to go.
3. **§7 — view state, input, stability tests.** Before pixels. FR-NI-4 is the one
   guarantee in this stage that is far harder to retrofit than to build in, and the
   transforms are needed by everything that follows.
4. **§6 — the radar pass**, ending with §11.2's read-back comparison.
5. **§8 (reference geometry) and §9.2/§9.3 (legend, cursor readout).**
6. **§10 — config persistence.**
7. **§14 measurements, then §12 documentation.**

**Deliberately out of scope**, each with its gate:

- Map underlays, county/state/highway geometry — Stage 5, gated on **Q15**.
- Map imagery tiles and the disk cache — Stage 5, gated on **Q16** (and Q5, Q7).
- Placefiles — Stage 6, gated on **Q6**.
- Runtime site change and clickable site markers — Stage 7 (FR-SS-2, FR-SS-3, FR-DA-4).
  `RadarState::reset` already exists for it and stays untested by this stage.
- Storm-relative velocity, KDP, PHI, CFP, velocity dealiasing — deferred by Q8/Q9.
- Multi-instance validation as a formal exercise — Stage 8 (NFR-P-1). §14 takes an
  indicative four-instance idle measurement, which is not the same thing and must not be
  written up as if it were.
- rayon — still deferred; ADR-0005's erratum records the derived-product measurement and
  the cheaper fix (precompute one range/height table per tilt) to try first.
- Packaging, minimum system requirements, reproducible-build verification — Stages 8–9.

---

## 14. Measurements to record in §16 Results

Take these on the release build. Record the number, not an impression.

| Measurement | Target / baseline |
|---|---|
| Frame time p50 / p99 during sustained pan, 1920×1080 | 60 fps ⇒ p99 under 16.6 ms (FR-DR-5) |
| Frame time p50 / p99 during sustained zoom | same |
| Worst single frame during a new-scan texture upload | "no perceptible drop" (FR-DR-5) |
| Idle frames/sec and CPU% — 1 instance | ~2 fps by design (§3.3) |
| Idle frames/sec and CPU% — 4 instances | approximately linear, no contention (NFR-P-1, indicative only) |
| Process start → first presented frame | < 2 s (§4.1 of REQUIREMENTS); must be measured *before* the first scan arrives (NFR-UX-4) |
| Peak RSS after a full volume with the render loop running | < 200 MB; Stage 3's headless figure was ~147 MB |
| GPU memory: grid textures + LUTs + egui atlas + surface | < 128 MB; Stage 3 measured 37.28 MB of grids + 2.52 MB derived |
| Texture uploads across a product switch and a sweep switch | **0** (FR-RP-7) |
| `Cargo.lock` package count | was **67** |
| `[bans]` duplicate-version count | was 2, both assessed benign |
| Release binary size | was **2,964,688 bytes** |
| Any `unsafe` introduced | expected none (NFR-SEC-5) |
| `RUNTIME_WORKER_THREADS` re-measured with the render loop competing for cores | Stage 2 left this instruction in `main.rs`; record whether 2 is still right |
| `cargo audit` findings in the new tree | expected none; record and decide any |

Also record, in prose: which surface format was selected and whether it is sRGB; whether
`Queue::write_texture` accepted tightly packed rows or the padded fallback was needed;
and which wgpu backend was actually chosen on the development machine.

---

## 15. Risks

1. **wgpu / egui-wgpu / winit version mismatch.** The most common failure in this
   ecosystem. Mitigated by §3.1's rule: take the versions from `egui-wgpu`'s manifest,
   never from memory.
2. **A copyleft licence appears in the new tree.** Stop and report; do not add it to
   `deny.toml`. This is an ADR-0009 question.
3. **Row-stride shearing and sRGB washout.** Both invisible to unit tests, both caught
   by §11.2. Do not defer that test to the end of the work item — write it as soon as
   the radar pass draws anything at all.
4. **Azimuth binning drift.** The WGSL must `floor`, not `round`, and use `atan2(x, y)`.
   Comment it, and let §11.2 prove it.
5. **`max_texture_dimension_2d` on the GL fallback.** Downlevel default is 2048 against
   a measured maximum of 1832 gates. It fits — but §4.3's guard is mandatory, not
   defensive padding, because a future longer-range moment would otherwise panic
   in wgpu's validation on a user's older hardware.
6. **egui repaint requests ignored** ⇒ dead-feeling widgets. §5 handles it; it is the
   defect most likely to survive to the end of the stage unnoticed by tests.
7. **First-render budget.** Adapter enumeration and shader compilation both sit before
   the first frame. If < 2 s is tight, present the cleared window with the loading
   indicator *before* compiling the radar pipeline — NFR-UX-4 asks for an interactive
   window before data, and pipeline compilation can happen on the first frame that
   actually needs it.
8. **Binary size.** egui's default fonts are on the order of a megabyte. Keep them for
   Stage 4 (the status bar and legend need real text), record the delta, and note font
   trimming as a Stage 8 lever rather than optimising here.
9. **No GPU in CI.** Accepted and stated: §11.1 covers the logic, §11.2 is manual, and
   CI still compiles everything. Note it in `ci.yml` beside the existing live-network
   comment so the gap is visible rather than assumed.

---

## 16. Results

**Implemented:** 2026-08-28. `cargo run --release -- KDOX` opens a window and draws the
selected gridded product in azimuthal-equidistant projection over range rings, azimuth
spokes, and a site marker; pans/zooms via drag, arrows, and wheel; switches product
(`1`–`7`) and sweep (`PageUp`/`PageDown`) as pure GPU state changes; reads out
range/azimuth/beam-height/value under the cursor; shows a colour-scale legend (with the
±Nyquist readout and an `RF` swatch for velocity); a bottom status bar carrying every
pipeline event; a loading indicator before the first sweep; and an `F1`/`?` key-help
overlay. `--headless` runs the Stage 2 loop. Window geometry and active product persist
on clean shutdown.

### What was built

- **Dependencies (§4.1, ADR-0022).** `winit =0.30.13`, `wgpu =30.0.1`, `egui`/
  `egui-wgpu`/`egui-winit =0.36.1`, pinned with `=` and matched to `egui-wgpu`/
  `egui-winit`'s manifests. `Cargo.lock`: **67 → 337 packages** (plan estimated
  ~230–260 — low). Release binary: **2,964,688 → 17,546,712 bytes**. `deny.toml`
  licence allowlist +4 (`BSD-2-Clause`, `Zlib`, `OFL-1.1`, `Ubuntu-font-1.0`), one at a
  time, each commented. Nothing copyleft beyond `MPL-2.0`. `cargo audit` and
  `cargo deny check` both clean. Duplicate-version crates: 2 → 8, all proc-macro /
  `no_std`-helper splits, assessed benign. Dated addendum in
  `docs/dependency-inventory.md`.
- **`ttf-parser` / RUSTSEC-2026-0192 avoided** by adding `winit` with
  `default-features = false` and omitting `wayland-csd-adwaita` (§3.1 anticipated a
  possible audit finding; this is how it was kept out).
- **No `unsafe` anywhere in `render/`** — production and test code. The test-only
  blocking executor uses `std::task::Wake` + `Box::pin`, not a raw waker.
- **`render/` module tree (S4-f):** `mod.rs` (ApplicationHandler, two-pass frame, pacing),
  `gpu.rs`, `view.rs`, `radar.rs`, `reference.rs`, `ui.rs`, `input.rs`, `time.rs`,
  `shaders/{radar,reference}.wgsl`. Binary-side, `mod render;` in `main.rs`.
- **Spatial stability (§7, S4-g):** `ViewState` is render-loop-owned; no function taking
  a `StateSnapshot` takes `&mut ViewState`. `view_state_is_unchanged_by_any_sequence_of_
  state_updates` applies `SweepGridded` × several elevations, a `DerivedComputed`, a
  `VolumeClosed`, a VCP change, and an `Info` event, and asserts every `ViewState` field
  is unchanged. Passes.
- **Selection is never silently rewritten (§3.8):** an absent (product, elevation) shows
  "el N — no data on this cut" in the accent colour; `step_elevation` is a no-op when
  there is nothing to step to. First sweep sets the default (lowest elevation).
- **CI tests (§11.1), all passing without a GPU:** `view` round-trip / `zoom_about`
  cursor-fixed / `on_resize` / zoom clamps; `input` full key map; `time::utc_from_nexrad`
  vs hand-computed and the KDOX fixture date; the stability test;
  `palette::threshold_range` and `DisplayProduct::parse` round-trips; the LUT sRGB
  conversion; `plan_sync` (the pure texture-cache decision — a product switch and a
  sweep switch each produce an **empty** `to_upload`, which is FR-RP-7's demonstration);
  the three new config keys valid / invalid / out of range;
  `ui::sample_at` distinguishing ND / RF / value / out-of-coverage;
  `view_uniform_bytes` packing geometry at the WGSL offsets. 49 new bin-crate tests;
  `cargo test --workspace` clean; `cargo clippy --workspace --all-targets -- -D warnings`
  clean.
- **GPU test (§11.2):** `offscreen_radar_pass_draws_data_and_leaves_no_data_transparent`
  (`#[ignore]`d) builds a `RadarRenderer`, syncs an all-data grid, renders it offscreen
  to an `Rgba8Unorm` target, reads it back, and asserts the centre pixel carries data.
  **It passes on the development machine** (Vulkan offscreen works even though the
  windowed surface does not — see the gap below). This exercises shader compilation,
  pipeline validation, `textureLoad` + LUT, and the row-stride path in one shot.
- **Config (§10):** `window.width` / `window.height` (clamped range `[640,7680]` /
  `[480,4320]`, out of range → default + `ConfigValueInvalid`, not a silent clamp),
  `view.product` (`DisplayProduct::parse`, unknown → default + `ConfigValueInvalid`).
  Saved once, on clean shutdown, only for values that changed, through
  `config::save`'s line-preserving atomic path.
- **`main.rs` (§4.2):** branches once on `--headless`; `RUNTIME_WORKER_THREADS` stays 2
  (re-measured — the render loop runs on the main thread, `Gpu::new` is the only place it
  blocks on the runtime, steady-state rendering does no async work); the stale rayon
  comment corrected. `headless.rs`'s doc comment updated to "supported mode", not
  "placeholder".
- **Docs (§12):** ADR-0022, ADR-0023 added; `rendering.md` dated erratum on the
  "vertex shader" sentence + a frame-pacing subsection + the reference-geometry layer +
  the performance table annotated; `dependency-inventory.md` addendum; `ci.yml` note on
  the GPU tests; `CLAUDE.md` status / module map / ADR index / keyboard map / `--headless`;
  `README.md` status / running / keyboard map. `open-questions.md` unchanged (Stage 4
  opened and closed nothing).

### Findings that extended or contradicted the plan

- **wgpu 30 is a large API break from what the plan assumed.** `Instance::new` takes the
  descriptor by value; `InstanceDescriptor` has no `Default` (use
  `new_without_display_handle()`); `DeviceDescriptor` gained `experimental_features`;
  `SurfaceConfiguration` gained `color_space`; `PipelineLayoutDescriptor` replaced
  `push_constant_ranges` with `immediate_size`; `RenderPipelineDescriptor` renamed
  `multiview` → `multiview_mask`; `RenderPassDescriptor` gained `multiview_mask`;
  `VertexState.buffers` and `PipelineLayoutDescriptor.bind_group_layouts` are now
  `&[Option<…>]`; `surface.get_current_texture()` returns a `CurrentSurfaceTexture`
  enum, not a `Result`, and there is no `SurfaceError`; presentation is
  `queue.present(texture)`, not `texture.present()`; `device.pop_error_scope` moved to
  `ErrorScopeGuard::pop`. egui 0.36 removed `Context::run`, `TopBottomPanel`, and
  `SidePanel`: the frame is `Context::run_ui(raw_input, |ui| …)` and panels are
  `egui::Panel::{bottom,right}(id).show(ui, …)` taking `&mut Ui`. `egui_wgpu::Renderer::
  new` takes a `RendererOptions` struct. `epaint`'s `textures_delta.set` entries carry a
  `SmallVec<[ImageDelta; 1]>`, not a single delta. None of this changed a decision —
  ADR-0022/0023 stand — but the "≈400 lines, written once" estimate assumed a stabler
  target API.
- **The radar shader's texture binding was in the wrong group** on the first run (group
  0 binding 2 in WGSL vs. group 1 in the Rust pipeline layout) — caught immediately by
  wgpu validation, fixed to `@group(1) @binding(0)`.
- **`reference::ring_labels` / `build_vertices` initially skipped the 230 km ring**
  because it is not a multiple of the 50 km step — the emphasised ring and its label were
  never drawn. Fixed with an explicit `ring_radii_km()` list; a unit test now asserts
  "230 km" is present.

### Verification NOT completed (gaps — recorded, not marked done)

- **On-screen rendering could not be verified in the build environment.** The dev machine
  runs inside a nested compositor (Cogl/mutter) that cannot import Vulkan dmabufs, so
  `Surface::configure` / the first frame acquire fails with "Surface does not support the
  adapter's queue family". The app now detects this at init (an error scope around
  `configure` plus a one-shot `get_current_texture()` probe, plus a `catch_unwind`
  around `Gpu::new` for the uncaptured-error case) and **exits non-zero naming
  `--headless`** — which is S4-e's required behaviour, and was verified (`echo "" |
  radar-workstation KDOX` → exit 1, message printed; `--headless` → exit 0). But the
  *positive* path — a window that actually shows the radar image, frame timing under
  sustained pan/zoom, first-render latency, idle fps for 1 and 4 instances, peak RSS,
  GPU memory — was **not** measured. These need a session on a real display. The
  offscreen GPU test is the strongest evidence the render path is correct end to end.
- **§11.2 test 2 (pixel-for-pixel vs. `utility/radar-viz`'s `render_grid_ppi`) was not
  implemented.** `render` is a binary-side module (S4-f) and `radar-viz` — which owns
  that validated CPU reference renderer — cannot reach it, nor vice versa, without a
  circular dependency or duplicating the renderer (DRY). Mitigations in place: the
  offscreen test proves the pass renders data and honours transparency; a numeric test
  (`view_uniform_bytes_pack_geometry_at_the_wgsl_offsets`) pins the uniform layout; the
  WGSL's `KE_A` constant is the exact `compute::geometry` value and the azimuth rule is
  documented as matching `compute::grid::azimuth_slot`. The sRGB-washout and row-stride
  failure modes the plan wanted that test to catch are partially covered (the LUT
  conversion is unit-tested; `write_texture` with tightly-packed 1832-wide rows is
  exercised by the offscreen test). A future session that makes `render` reachable (or
  adds a small shared offscreen harness) should add the full comparison.
- **`Queue::write_texture` with tightly packed rows** was used and works in the offscreen
  test; the padded-staging-buffer fallback (§6.1) was not needed and not written.
- The `max_texture_dimension_2d` guard (§4.3) is implemented (reports
  `DegenerateGateGeometry` and skips) but not exercised by a test — the largest real grid
  (1832) is well under `downlevel_defaults()`'s 2048.
