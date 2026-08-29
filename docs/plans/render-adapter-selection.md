# Implementation Plan — Durable GPU Adapter Selection and Render Error Reporting

**Status:** Implemented and verified on the reproducing machine 2026-08-28 (§10 Results,
§11 Verification matrix).
**Drafted:** 2026-08-28
**Fixes:** startup crash on hybrid-GPU Linux systems (see §1); three error-reporting defects (§2)
**Baseline:** branch `first_pixels`, after `docs/plans/stage-4-first-pixels.md`
**Touches:** `crates/radar-workstation/src/render/{gpu.rs,mod.rs}`, new `render/adapter.rs`,
`crates/radar-workstation/src/main.rs`, `docs/adr/`, `CLAUDE.md`
**New dependencies:** none. Display discovery uses `std::fs` against sysfs.

---

## 1. The defect

On a hybrid-GPU Linux system, `cargo run --release -- KDOX` dies during the first frames:

```
[destroyed object]: error 7: failed to import supplied dmabufs: Could not bind the given EGLImage to a CoglTexture2D
[radar-workstation] wgpu error: Validation Error / In Surface::configure / Surface does not support the adapter's queue family
Protocol error 7 on object @0
radar-workstation: could not create a render surface: Exit Failure: 1
```

**Root cause.** `gpu.rs` requests `PowerPreference::LowPower`, which selects the *integrated*
adapter. On the reproducing machine (NVIDIA RTX 4070 SUPER at `0000:01:00.0` + AMD Granite Ridge
iGPU at `0000:0c:00.0`), GNOME Shell holds open only `/dev/dri/card2`, `/dev/dri/renderD129` and
`/dev/nvidia*` — the compositor renders and scans out exclusively on NVIDIA, and the AMD iGPU has
no connected connector. The app therefore allocates swapchain images on AMD, with AMD tiling
modifiers, and hands those dmabufs to an NVIDIA-only compositor, which cannot import them.

**Why the existing guards miss it.** The AMD adapter satisfies
`vkGetPhysicalDeviceWaylandPresentationSupportKHR`, so `compatible_surface` does not filter it out;
`Surface::configure` succeeds; the init probe in `Gpu::new` succeeds. The rejection happens
*asynchronously, in the compositor*, after a real present. The compositor destroys the buffer
object, the surface goes `Lost`/`Outdated`, `reconfigure()` runs against a dead `wl_surface`, and
the Wayland connection is torn down.

**Confirmed by bisection** (existing binary, no code changes):

| Vulkan ICD restricted to | Result |
|---|---|
| `nvidia_icd.x86_64.json` | runs clean |
| `radeon_icd.x86_64.json` | byte-for-byte reproduction, exit 1 |
| both (default) | reproduction |

**Fix strategy.** Stop expressing a *power* preference and start selecting the adapter that owns
the display, verified by actually bringing up a surface on it, with an honest failure report and a
working operator override when the heuristic is wrong.

---

## 2. The three error-reporting defects to clean up

| # | Defect | Location |
|---|---|---|
| E1 | Every `run_app` error is laundered into `GpuInitError::Surface`, producing `could not create a render surface: Exit Failure: 1` for a failure that happened many frames *after* successful init, and misdirecting the operator to `--headless` when their display is fine | [mod.rs:103](../../crates/radar-workstation/src/render/mod.rs#L103) |
| E2 | The comment claims `with_env()` honours `WGPU_POWER_PREF`. It does not — `InstanceDescriptor::with_env` (wgpu-types 30.0.1) reads only `WGPU_BACKEND`, `WGPU_VALIDATION`/`WGPU_DEBUG` and backend options. `PowerPreference::from_env()` is a separate function that is never called, so the documented escape hatch for exactly this hardware is silently inert | [gpu.rs:56-58](../../crates/radar-workstation/src/render/gpu.rs#L56-L58) |
| E3 | `main` suggests `--headless` for *any* `Err` from `render::run`, including mid-session failures | [main.rs:137](../../crates/radar-workstation/src/main.rs#L137) |

---

## 3. What "done" means

| Claim | How it is demonstrated |
|---|---|
| The app selects the GPU that drives the display, not the lowest-power one | `cargo run --release -- KDOX` on the reproducing machine runs clean with both ICDs visible; the startup line names the NVIDIA adapter |
| Selection is correct on a laptop too, without re-introducing a discrete-GPU bias | Unit test: iGPU owns the connected panel, dGPU present → iGPU ranked first |
| A wrong guess degrades to a clear message, never a protocol-error crash | Forcing the non-display adapter (`RADAR_GPU=<pci>` or a restricted ICD) exits non-zero naming every adapter tried and the override variable |
| Selection logic is testable without a GPU | `rank()` is pure over injected facts; the whole suite runs on CI, which has no GPU |
| `--headless` is suggested only when it is actually the remedy | E1/E3 fixed; a mid-session event-loop failure reports its own cause |
| No comment in `render/` claims an environment variable that does not work | E2 fixed; `RADAR_GPU` is implemented and documented |
| Nothing regressed | `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny check`, `cargo audit` all clean |

---

## 4. Design

Two halves, split so the decision is pure and the I/O is thin.

### 4.1 Facts in

```rust
// A display-capable GPU discovered from the kernel, independent of wgpu.
pub struct DisplayDevice { pub pci_address: String, pub vendor_id: u32, pub device_id: u32 }

// A projection of wgpu::AdapterInfo, so ranking needs no GPU to test.
pub struct AdapterFacts {
    pub index: usize,
    pub name: String,
    pub pci_address: String,      // AdapterInfo::device_pci_bus_id; empty on some backends
    pub vendor: u32,
    pub device: u32,
    pub device_type: wgpu::DeviceType,
    pub backend: wgpu::Backend,
    pub presents_to_surface: bool, // surface.get_capabilities(adapter) yields >=1 format
}
```

`AdapterInfo::device_pci_bus_id` is `"0000:01:00.0"` on Vulkan — the same string as the sysfs PCI
path component, so the match is exact, not a heuristic. It may be empty on the GL backend, which is
why vendor/device IDs are a second key.

### 4.2 The decision

```rust
pub fn rank(adapters: &[AdapterFacts], displays: &[DisplayDevice]) -> Vec<usize>
```

Pure. Returns candidate indices, best first. Rules:

1. **Exclude** any adapter with `presents_to_surface == false`. Never a candidate.
2. **Exclude** `DeviceType::Cpu` unless no other candidate remains; if a software rasteriser is
   selected, say so on the startup line.
3. `+1000` — `pci_address` matches a `DisplayDevice` exactly.
4. `+500` — `(vendor, device)` matches a `DisplayDevice` (used when the bus id is unavailable).
5. `+100` — no `DisplayDevice` was discoverable at all *and* `device_type == DiscreteGpu`. This is
   the unknown-environment tiebreak; it deliberately leans away from the integrated adapter,
   because "integrated adapter that cannot present" is the failure being fixed.
6. `+10` — `backend == Vulkan` (ADR-0003 primary over the GL fallback).
7. Ties break by `index`, so ordering is deterministic across runs.

Note what rule 3 buys on a laptop: the iGPU owns the panel, matches a `DisplayDevice`, and wins —
Principle 3's lightweight intent is preserved wherever it is actually achievable, and abandoned
only where it is impossible.

### 4.3 Verification by bring-up

Ranking is a prediction about compositor behaviour, so it is checked rather than trusted. Walk the
ranked candidates; for each, request the device/queue, configure the surface inside a validation
error scope, and acquire one frame. On any error, drop everything and try the next candidate.
Exhausting the list is `RenderError::NoPresentableAdapter`.

**State this limitation in the code comment, honestly:** a compositor-side dmabuf import rejection
is asynchronous and does *not* surface during this bring-up. The bring-up loop catches
queue-family and configure failures; correct *selection* is what prevents the dmabuf case. §4.4 is
the backstop for what neither catches.

### 4.4 Late-failure guard

Today a post-init surface failure ends as `Exit Failure: 1`. Add a counter in `App`: if
`reconfigure()` is reached more than 3 times within the first 2 seconds, or a reconfigure raises a
validation error, stop and set `RenderError::PresentationLost` with the adapter's name and PCI
address plus the `RADAR_GPU` hint. Do **not** attempt mid-session adapter re-selection — by the
time this fires the Wayland connection is already unusable. The goal is an accurate message.

### 4.5 Operator override

`RADAR_GPU` — the escape hatch E2 falsely promised. Accepts a PCI address (`0000:01:00.0`), a
case-insensitive substring of the adapter name (`nvidia`), or `discrete` / `integrated`. It bypasses
ranking but **not** §4.3's bring-up check. An unmatched value is a clean error listing available
adapters — never a panic, never a silent fallback.

---

## 5. Steps

**S1 — `render/adapter.rs` (new).**
Define `DisplayDevice`, `AdapterFacts`, `rank()`, and `GpuOverride` + its parser. Pure code only,
no wgpu calls beyond the `DeviceType`/`Backend` enums. Unit tests live here.

**S2 — sysfs display discovery, in `render/adapter.rs`.**
`fn discover_displays(sysfs_root: &Path) -> Vec<DisplayDevice>`:
scan `{root}/class/drm/card*-*`, keep entries whose `status` reads `connected`, resolve the parent
card (`card2-HDMI-A-2` → `card2`), read `card2/device/vendor` and `card2/device/device` (hex, `0x`
prefixed), and take the PCI address from the last component of the resolved `card2/device` symlink
target. Deduplicate by PCI address. **Every** failure — missing path, unreadable file, malformed
hex, no connectors — returns an empty or partial list; never an error, never a panic. Take the root
as a parameter so tests point it at a fixture tree.

**S3 — rewrite `Gpu::new` selection in `render/gpu.rs`.**
Replace `request_adapter(PowerPreference::LowPower, …)` with:
`instance.enumerate_adapters(backends)` → build `AdapterFacts` for each (setting
`presents_to_surface` from `surface.get_capabilities(&adapter).formats.is_empty()`) →
`discover_displays("/sys")` → apply `RADAR_GPU` if set, else `rank()` → §4.3 bring-up loop.
Keep the existing `Gpu` fields, `resize`, `reconfigure`, `on_uncaptured_error` handler, and the
`downlevel_defaults` limits unchanged. Record the chosen `AdapterFacts` on `Gpu` for §S6.
Delete the false `WGPU_POWER_PREF` sentence (E2); keep `with_env()` and document accurately that it
carries `WGPU_BACKEND` only, and that `RADAR_GPU` is the adapter override.

**S4 — `RenderError` replaces `GpuInitError`, in `render/gpu.rs`.**
Variants: `Surface(String)`, `NoAdapter(String)`, `Device(String)`,
`NoPresentableAdapter { tried: Vec<String> }`, `PresentationLost { adapter: String }`,
`EventLoop(String)`. `Display` for `NoPresentableAdapter` lists each adapter as
`name (backend, pci)` and names `RADAR_GPU`. Rename references across `render/` and `main.rs`.

**S5 — fix the error paths (E1, E3).**
[mod.rs:103](../../crates/radar-workstation/src/render/mod.rs#L103): map `run_app`'s error to
`RenderError::EventLoop`, not `Surface`. Same for `EventLoop::new()` at
[mod.rs:71](../../crates/radar-workstation/src/render/mod.rs#L71).
In `run_render` ([main.rs:133-139](../../crates/radar-workstation/src/main.rs#L133-L139)), print the
`--headless` line only for `Surface | NoAdapter | Device | NoPresentableAdapter`. For
`PresentationLost` print the adapter and the `RADAR_GPU` hint; for `EventLoop` print the cause
alone. Exit code stays `ExitCode::FAILURE` in all cases.

**S6 — the startup diagnostic line ([mod.rs:411-414](../../crates/radar-workstation/src/render/mod.rs#L411-L414)).**
Extend it to name the adapter and *why* it was chosen, so the next report of this class is
one line long:

```
[radar-workstation] GPU: NVIDIA GeForce RTX 4070 SUPER (Vulkan, 0000:01:00.0) — matched connected display; surface Rgba8UnormSrgb (sRGB: true)
```

Reasons: `matched connected display`, `matched display vendor/device`, `no display info — highest
ranked presentable adapter`, `forced by RADAR_GPU`, `software rasteriser — no hardware adapter could present`.

**S7 — late-failure guard (§4.4)** in `render/mod.rs` around
[the reconfigure at mod.rs:245](../../crates/radar-workstation/src/render/mod.rs#L245).

---

## 6. Tests

All CPU-only — CI has no GPU and must stay that way.

`rank()`: reproducing desktop (dGPU owns display, iGPU does not) → dGPU first; laptop (iGPU owns
panel) → iGPU first; single adapter → itself; no display info → presentable discrete before
integrated, Cpu last; non-presentable adapters absent from the output entirely; exact PCI match
outranks vendor/device match; equal scores order by index, asserted stable.

`discover_displays()`: fixture sysfs trees under
`crates/radar-workstation/tests/fixtures/sysfs/` covering connected / disconnected / `unknown`
status, a missing `vendor` file, malformed hex, and an absent `class/drm` — each returns a
sensible list and never panics.

`GpuOverride` parsing: PCI address, name substring, `discrete`, `integrated`, unmatched value →
`Err`, empty string → treated as unset.

`RenderError`: `Display` for `NoPresentableAdapter` contains every tried adapter and the string
`RADAR_GPU`.

Existing `render/` tests — including
`view_state_is_unchanged_by_any_sequence_of_state_updates` — must pass unchanged.

---

## 7. Documentation

- **ADR-0024** — *GPU adapter selection: match the display device, verify by bring-up.* Context is
  §1; decision is §4; consequences include the deliberate departure from `LowPower` and what
  Principle 3 still buys on a laptop.
- **ADR-0022** — dated erratum: the `PowerPreference::LowPower` choice recorded in S4-a is
  superseded by ADR-0024.
- **`CLAUDE.md`** — add `0024` to the ADR index; add `adapter.rs` to the `render/` module map with
  a one-line description; note `RADAR_GPU` beside the CLI line.
- **`docs/architecture/rendering.md`** — one paragraph on adapter selection and the override.

---

## 8. Verification matrix (manual, on the reproducing machine)

| Command | Expected |
|---|---|
| `cargo run --release -- KDOX` | selects NVIDIA, startup line says `matched connected display`, window renders, no protocol error |
| `VK_DRIVER_FILES=/usr/share/vulkan/icd.d/radeon_icd.x86_64.json cargo run --release -- KDOX` | either renders, or exits non-zero with `NoPresentableAdapter`/`PresentationLost` naming the AMD adapter and `RADAR_GPU` — **never** `Exit Failure: 1` |
| `RADAR_GPU=0000:0c:00.0 cargo run --release -- KDOX` | forces AMD; clean diagnostic failure, no crash |
| `RADAR_GPU=nonsense cargo run --release -- KDOX` | exits listing available adapters |
| `WGPU_BACKEND=gl cargo run --release -- KDOX` | exercises the empty-`pci_address` path; renders or fails cleanly |
| `cargo run --release -- KDOX --headless` | unchanged behaviour |

Record results in a §9 Results section on completion, per house convention.

---

## 9. Non-goals

Mid-session adapter re-selection; explicit PRIME / cross-GPU buffer sharing; a GPU picker in the
UI; any change to the render passes, `ViewState`, shaders, or `AppState`; new dependencies.

---

## 10. Results (2026-08-28)

Implemented on branch `first_pixels`, no commit.

**Code.**

- **`crates/radar-workstation/src/render/adapter.rs` (new, ~490 lines with tests).**
  `DisplayDevice`, `AdapterFacts`, `SelectionReason`, `rank()`, `reason_for()`,
  `GpuOverride` + `parse` / `select`, `discover_displays()` (sysfs, parameterised root,
  every failure → shorter list). 16 unit tests, all CPU-only.
- **`render/gpu.rs` rewritten.** `GpuInitError` → `RenderError` with the six §S4 variants
  and the `Display` impls from §S4/§S5. `Gpu::new` now: `enumerate_adapters` → build
  `AdapterFacts` (`presents_to_surface` from `get_capabilities(..).formats`) →
  `discover_displays("/sys")` → `RADAR_GPU` override or `rank()` → §4.3 bring-up loop
  (`bring_up`, per-candidate device + configure-in-error-scope + one frame acquired and
  dropped). `Gpu` carries the chosen `AdapterFacts`, its `SelectionReason`, and an
  uncaptured-error sink for §4.4. The false `WGPU_POWER_PREF` sentence is gone (E2); the
  `with_env()` comment now says `WGPU_BACKEND` only.
- **`render/mod.rs`.** `pub use gpu::RenderError`. `EventLoop::new` / `run_app` errors →
  `RenderError::EventLoop` (E1). Late-failure guard (§4.4) on the `Lost`/`Outdated`
  reconfigure path: `reconfigure_count` + `first_frame_at`; > 3 reconfigures inside 2 s,
  or a validation error in the sink, → `RenderError::PresentationLost` + `event_loop.exit()`.
  Startup line rewritten to §S6's form.
- **`main.rs`.** `run_render` prints the `--headless` line only for
  `Surface | NoAdapter | Device | NoPresentableAdapter`; `PresentationLost` and
  `EventLoop` print their own cause (E3). Exit code unchanged (`FAILURE`).

**Tests.** `crates/radar-workstation/tests/fixtures/sysfs/` — six fixture trees
(`desktop_nvidia`, `laptop_intel`, `all_disconnected`, `missing_vendor`, `malformed_hex`,
plus a temp-dir case for absent `class/drm`). `cargo test --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny check`, `cargo audit`
all clean. `Cargo.lock` unchanged at 337 packages — no new dependency.

**Docs.** ADR-0024 written; ADR-0022 carries a dated erratum; `CLAUDE.md` ADR index,
`render/` module map, and env vars updated; `docs/architecture/rendering.md` gains a
"GPU Adapter Selection" section.

One deviation from §S4/§S5: `render::run` also maps a `run_app` `Err` that lands within
10 s of the first frame to `PresentationLost` rather than `EventLoop`. The compositor's
dmabuf rejection is asynchronous enough that on this hardware it tears the Wayland
connection down (and `run_app` returns `Err`) *before* the reconfigure-path guard's
`event_loop.exit()` takes effect — so the guard's `fatal_error` is never observed and the
grace-window fallback is what produces the accurate message. Both paths are kept.

---

## 11. Verification matrix (2026-08-28, on the reproducing machine)

Hardware exactly as §1: NVIDIA GeForce RTX 4070 SUPER at `0000:01:00.0`, AMD Granite
Ridge iGPU (RADV RAPHAEL_MENDOCINO) at `0000:0c:00.0`, GNOME Shell on Wayland, both Vulkan
ICDs visible.

| Command | Result |
|---|---|
| `cargo run --release -- KDOX` | ✅ selects NVIDIA; startup line `GPU: NVIDIA GeForce RTX 4070 SUPER (Vulkan, 0000:01:00.0) — matched connected display`; window renders, ran to the test timeout with no protocol error |
| `RADAR_GPU=0000:0c:00.0 … KDOX` (force AMD, the §1 repro) | ✅ startup line `— forced by RADAR_GPU`; the dmabuf import fails as before, but the app now exits 1 with `the GPU stopped being able to present partway through the session (adapter: AMD … 0000:0c:00.0) … set RADAR_GPU to …` — **not** `Exit Failure: 1` |
| `RADAR_GPU=nonsense … KDOX` | ✅ exits 1 listing all four enumerated adapters and the `RADAR_GPU` usage line |
| `WGPU_BACKEND=gl … KDOX` | ✅ the GL adapter reports no surface format on this compositor → `NoPresentableAdapter` naming it, exits 1, suggests `--headless` — clean, no crash |
| `… KDOX --headless` | ✅ unchanged — Stage 2 state loop, exits 0 on stdin close |

Not re-measured: the frame-timing / RSS / GPU-memory figures in stage-4 §14 — this change
does not touch the render passes. `VK_DRIVER_FILES=…/radeon_icd.json` (matrix §8 row 2)
was not run separately; `RADAR_GPU=0000:0c:00.0` exercises the same AMD-only path.
