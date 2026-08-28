# ADR-0022: Window, Event Loop, and Render-Pass Hosting (S4-a)

## Status
Accepted

> **Erratum (2026-08-28).** S4-a's adapter choice — `PowerPreference::LowPower`, on the
> reasoning that four integrated-GPU instances beat four discrete ones — is **superseded
> by [ADR-0024](0024-gpu-adapter-selection.md)**. On a hybrid-GPU Linux system the
> low-power adapter is the integrated one, which frequently does *not* drive the display;
> its swapchain dmabufs are then rejected asynchronously by the compositor and the Wayland
> connection is torn down several frames after a "successful" init. `render/gpu.rs` now
> enumerates every adapter, ranks by which one owns a connected display, and verifies the
> choice by bringing a surface up on it. Nothing else in this ADR changes.

## Context
ADR-0002 and ADR-0003 fixed egui and wgpu but never said how they are *hosted* — who
owns the OS window, the event loop, the GPU surface, and the swapchain. The two documents
that describe the frame both describe a relationship `eframe` inverts:

- `rendering.md`: "two systems that share a window but not a render pipeline"; "egui is
  drawn last, on top of wgpu output, every frame."
- `overview.md`: "radar data rendered directly to GPU surface, bypassing egui's renderer."

Under `eframe` the radar pass becomes a guest inside an `egui_wgpu::CallbackTrait`, and
the surface, present mode, and frame pacing sit behind eframe's abstraction — which is
precisely the frame pacing S4-c needs to control (redraw-on-demand plus a 2 Hz idle tick,
not 60 fps of an unchanged image in four processes).

## Decision

**Own the event loop, the surface, and the swapchain directly.**

| Concern | Crate | Pinned version |
|---|---|---|
| Event loop + window | `winit` | `=0.30.13` |
| Device, queue, surface | `wgpu` | `=30.0.1` |
| egui ⇄ winit input translation | `egui-winit` | `=0.36.1` |
| egui → our surface rendering | `egui-wgpu` | `=0.36.1` |
| Immediate-mode UI | `egui` | `=0.36.1` |

The four non-egui-core versions are **not guessed**: `egui`/`egui-wgpu`/`egui-winit` are
released together at one version (0.36.1, the newest at implementation time); `wgpu 30.0.1`
is the exact version `egui-wgpu 0.36.1` depends on; `winit 0.30.13` is the minimum
`egui-winit 0.36.1` accepts (`^0.30.13`). All four are pinned with `=` in
`crates/radar-workstation/Cargo.toml` and are upgraded deliberately, per ADR-0003.

Roughly 400 lines of setup this project owns (`render/gpu.rs`, `render/mod.rs`'s
`ApplicationHandler`) is the cheaper side of that trade, and it is written once.

`winit` and `wgpu` are added with `default-features = false`:

- `winit` — `["x11", "wayland", "wayland-dlopen", "rwh_06"]`. The default set also pulls
  `wayland-csd-adwaita`, whose client-side-decoration fallback drags in the **unmaintained**
  `ttf-parser` (RUSTSEC-2026-0192) via `sctk-adwaita`. The compositor draws our
  decorations; we do not need egui-drawn ones. Dropping that one feature keeps the
  advisory out of the tree entirely.
- `wgpu` — `["wgsl", "vulkan", "gles"]`. WGSL is the shader language; Vulkan is primary and
  OpenGL ES is the fallback for older hardware (ADR-0003), stated in code
  (`Backends::VULKAN | GL`) rather than left to the default.

### `render/` is a binary-side module tree (S4-f)

`crates/radar-workstation/src/render/`, declared `mod render;` in `main.rs` alongside
`mod cli;` and `mod headless;`. ADR-0010 puts only the decoder in a separate library
crate; the render loop is application code, not reusable library API, and it consumes the
lib's public surface (`AppState::snapshot`, `compute::*`) exactly as `headless.rs`
already does. A `render` cargo feature to spare `utility/radar-viz` the wgpu compile time
was rejected: resolver-v2 feature unification enables it in a workspace build anyway.

## Consequences

- **Dependency count: 67 → 337 packages** (`Cargo.lock`). This is the largest single
  dependency step the project will take; NFR-SEC-2 makes it a decision, recorded here,
  not a silent consequence. The estimate in the plan (~230–260) was low.
- **`deny.toml` licence allowlist expanded four entries**, each with a comment naming the
  requiring crate: `BSD-2-Clause` (arrayref, via wgpu-hal), `Zlib` (foldhash, via
  egui/ahash), `OFL-1.1` and `Ubuntu-font-1.0` (`epaint_default_fonts` — egui's bundled
  UI fonts). All permissive. **Nothing copyleft beyond `MPL-2.0` appeared** — that would
  have been an ADR-0009 question, not a `deny.toml` edit.
- **`[bans].multiple-versions` stays `"warn"`.** The new tree carries a second version of
  8 crates (`getrandom`, `hashbrown`, `linux-raw-sys`, `rustc-hash`, `rustix`, `syn`,
  `thiserror`, `thiserror-impl`) — up from 2 duplications before. Assessed, not
  automatically a defect: they are proc-macro/`no_std`-helper splits deep in the build
  graph, not runtime-code divergence. `docs/dependency-inventory.md` carries the dated
  addendum.
- **`cargo audit` is clean** in the new tree (after the `winit` feature trim above).
- **No `unsafe` anywhere in `render/`** (NFR-SEC-5, BC-9). Modern wgpu creates a surface
  safely from an `Arc<Window>` that carries the raw-handle traits with a `'static`
  lifetime.
- **Release binary size: 2,964,688 → 17,546,712 bytes** (+14.6 MB). egui's bundled fonts
  and naga/wgpu account for most of it. The status bar and legend need real text, so the
  fonts stay for Stage 4; font trimming is noted as a Stage 8 lever (plan risk #8), not
  optimised here.
- **`--headless` is retained** (S4-e, ADR supersedes nothing): `main` branches once, and
  `headless.rs` stays the supported mode for a server, a container, or CI. A window or
  adapter that cannot be created exits non-zero naming `--headless` — never a silent
  degrade into a scrolling log.

## Rejected alternatives

- **`eframe`** — inverts the frame relationship both architecture documents describe, and
  hides the present mode and frame pacing S4-c must control.
- **Trimming `winit` further (no x11 or no wayland)** — a single-binary Linux build must
  run on both session types without a rebuild.
- **A `render` cargo feature** — bought nothing under resolver-v2 feature unification, and
  added a configuration axis to every build command.
