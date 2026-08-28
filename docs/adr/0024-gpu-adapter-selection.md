# ADR-0024: GPU Adapter Selection — Match the Display Device, Verify by Bring-Up

## Status
Accepted (2026-08-28)

Supersedes the `PowerPreference::LowPower` adapter choice recorded in
[ADR-0022](0022-render-loop-hosting.md) / plan S4-a.

## Context

On a hybrid-GPU Linux system, `cargo run --release -- KDOX` died during the first frames:

```
[destroyed object]: error 7: failed to import supplied dmabufs: Could not bind the given EGLImage to a CoglTexture2D
[radar-workstation] wgpu error: Validation Error / In Surface::configure / Surface does not support the adapter's queue family
radar-workstation: could not create a render surface: Exit Failure: 1
```

**Root cause.** `render/gpu.rs` requested `PowerPreference::LowPower`, which selects the
*integrated* adapter. On the reproducing machine (NVIDIA RTX 4070 SUPER at `0000:01:00.0`
plus an AMD Granite Ridge iGPU at `0000:0c:00.0`) GNOME Shell renders and scans out
exclusively on NVIDIA; the AMD iGPU has no connected connector. The app allocated
swapchain images on AMD, with AMD tiling modifiers, and handed those dmabufs to an
NVIDIA-only compositor, which cannot import them.

**Why the Stage 4 guards missed it.** The AMD adapter satisfies
`vkGetPhysicalDeviceWaylandPresentationSupportKHR`, so `compatible_surface` does not
filter it out; `Surface::configure` succeeds; the init probe succeeds. The rejection
happens *asynchronously, in the compositor*, after a real present — the surface then goes
`Lost`/`Outdated`, `reconfigure()` runs against a dead `wl_surface`, and the Wayland
connection is torn down.

**Confirmed by bisection** (Stage 4 binary, no code changes): restricting the Vulkan ICD
to `nvidia_icd` runs clean; restricting it to `radeon_icd` is a byte-for-byte
reproduction, exit 1; the default (both) reproduces.

Alongside the crash, three error-reporting defects made the failure illegible:

- **E1** — every `run_app` error was laundered into `Surface`, so a failure many frames
  *after* successful init printed `could not create a render surface` and misdirected the
  operator to `--headless`.
- **E2** — a comment claimed `InstanceDescriptor::with_env()` honours `WGPU_POWER_PREF`.
  It does not (wgpu-types 30.0.1 reads only `WGPU_BACKEND` and validation flags there);
  `PowerPreference::from_env()` is a separate function that was never called. The
  documented escape hatch for exactly this hardware was inert.
- **E3** — `main` suggested `--headless` for *any* `Err` from `render::run`, including a
  mid-session event-loop failure.

## Decision

Stop expressing a *power* preference. Select the adapter that **owns the display**,
verify it by actually bringing a surface up on it, and fail with an honest report and a
working operator override when the heuristic is wrong.

### 1. Pure decision over injected facts (`render/adapter.rs`)

```rust
struct DisplayDevice  { pci_address: String, vendor_id: u32, device_id: u32 }
struct AdapterFacts   { index, name, pci_address, vendor, device, device_type, backend, presents_to_surface }
fn rank(adapters: &[AdapterFacts], displays: &[DisplayDevice]) -> Vec<usize>   // best first, pure
```

`AdapterInfo::device_pci_bus_id` is `"0000:01:00.0"` on the Vulkan backend — the same
string as the sysfs PCI path component, so the match is exact, not a heuristic. It is
empty on the GL backend, which is why `(vendor, device)` is a second key.

`rank()` rules (see the module for the code):

1. Exclude any adapter that cannot present (`presents_to_surface == false`).
2. Exclude `DeviceType::Cpu` unless nothing else can present; if a software rasteriser is
   selected, the startup line says so.
3. `+1000` — `pci_address` matches a connected `DisplayDevice` exactly.
4. `+500` — `(vendor, device)` matches (used when the bus id is unavailable).
5. `+100` — **no** `DisplayDevice` was discoverable *and* `device_type == DiscreteGpu`.
   The unknown-environment tiebreak; it deliberately leans away from the integrated
   adapter, because "integrated adapter that cannot present" is the failure being fixed.
6. `+10` — `backend == Vulkan` (ADR-0003 primary over the GL fallback).
7. Ties break by `index`, so ordering is deterministic across runs.

On a laptop, rule 3 gives the right answer for free: the iGPU owns the panel, matches a
`DisplayDevice`, and wins — Principle 3's lightweight intent is preserved wherever it is
actually achievable, and abandoned only where it is impossible.

### 2. Sysfs display discovery (`discover_displays(sysfs_root: &Path)`)

Scan `{root}/class/drm/card*-*`, keep connectors whose `status` reads `connected`,
resolve the parent card (`card2-HDMI-A-2` → `card2`), read `card2/device`'s `vendor` /
`device` (hex) and take the PCI address from the last component of that symlink's target.
Deduplicate by PCI address. **Every** failure — missing path, unreadable file, malformed
hex, no connectors — returns a shorter list; never an `Err`, never a panic. The root is a
parameter so tests point it at a fixture tree.

### 3. Verification by bring-up (`render/gpu.rs`)

Ranking is a *prediction* about compositor behaviour, so it is checked. Walk the ranked
candidates; for each, request the device/queue, configure the surface inside a validation
error scope, and acquire one frame. On any error, drop everything and try the next.
Exhausting the list is `RenderError::NoPresentableAdapter { tried }`, whose `Display`
lists each adapter as `name (backend, pci)` and names `RADAR_GPU`.

**Honest limitation, stated in the code:** a compositor-side dmabuf-import rejection is
asynchronous and does *not* surface during bring-up. This loop catches device-request and
configure/acquire failures; correct *selection* is what prevents the dmabuf case, and §4
is the backstop.

### 4. Late-failure guard (`render/mod.rs`)

Two layers, both producing `RenderError::PresentationLost { adapter }` naming the adapter
and the `RADAR_GPU` hint:

- **On the reconfigure path** — if the surface goes `Lost`/`Outdated` and `reconfigure()`
  is reached more than 3 times within the first 2 seconds, or the uncaptured-error sink
  shows a validation error after a reconfigure, stop.
- **Grace-window fallback** — the compositor's dmabuf rejection is asynchronous enough
  that it can tear the Wayland connection down (so `event_loop.run_app` returns `Err`)
  *before* the reconfigure guard's `exit()` is observed. So a `run_app` error that lands
  within 10 s of the first frame is reported as `PresentationLost`, not `EventLoop`.

**No** mid-session adapter re-selection — by the time either fires the Wayland connection
is already unusable. The goal is an accurate message, not recovery.

### 5. Operator override — `RADAR_GPU`

The escape hatch E2 falsely promised. Accepts a PCI address (`0000:01:00.0`), a
case-insensitive substring of the adapter name (`nvidia`), or `discrete` / `integrated`.
It bypasses ranking but **not** §3's bring-up check. An unmatched value is a clean
`NoPresentableAdapter` error listing the available adapters — never a panic, never a
silent fallback.

### 6. `RenderError` replaces `GpuInitError`

Variants `Surface`, `NoAdapter`, `Device`, `NoPresentableAdapter { tried }`,
`PresentationLost { adapter }`, `EventLoop`. `main` prints the `--headless` suggestion
only for `Surface | NoAdapter | Device | NoPresentableAdapter`; `PresentationLost` and
`EventLoop` report their own cause (fixes E1, E3). Exit code stays `FAILURE` in all
cases.

### 7. Startup diagnostic line

```
[radar-workstation] GPU: NVIDIA GeForce RTX 4070 SUPER (Vulkan, 0000:01:00.0) — matched connected display; surface Rgba8UnormSrgb (sRGB: true)
```

Reasons: `matched connected display`, `matched display vendor/device`, `no display info —
highest ranked presentable adapter`, `forced by RADAR_GPU`, `software rasteriser — no
hardware adapter could present`.

## Consequences

- **No new dependencies.** Display discovery is `std::fs` against sysfs.
- **The deliberate departure from `LowPower`.** On a true hybrid desktop the discrete GPU
  is now selected — it is the one that can present. On a laptop the iGPU still wins
  (rule 3), so the four-simultaneous-instances case Principle 3 protects is unchanged
  wherever it was ever achievable. Where it is not achievable (dGPU-only compositor),
  a working display beats a lightweight one.
- **Selection logic is testable without a GPU.** `rank()`, `GpuOverride`, and
  `discover_displays()` (against fixture sysfs trees in
  `crates/radar-workstation/tests/fixtures/sysfs/`) are unit-tested on CI, which has no
  GPU.
- **A wrong guess degrades to a message, not a protocol-error crash.** Forcing the
  non-display adapter (`RADAR_GPU=<pci>` or a restricted ICD) exits non-zero naming every
  adapter tried and the override variable.
- `ADR-0022`'s pinned crate versions, the two-pass frame, `ViewState` ownership, the
  shaders, and `AppState` are all untouched.

## Rejected alternatives

- **`PowerPreference::HighPerformance`** — re-introduces a discrete-GPU bias that is wrong
  on a laptop, and still a *power* proxy for a *display-ownership* question.
- **Mid-session adapter re-selection** — the Wayland connection is already torn down by
  the time the late-failure guard fires.
- **Explicit PRIME / cross-GPU buffer sharing** — a much larger surface than the problem
  needs; selecting the right adapter avoids the cross-GPU path entirely.
- **A GPU picker in the UI** — chrome for a once-per-machine decision; `RADAR_GPU` in the
  environment is the right weight.
