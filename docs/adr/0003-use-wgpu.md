# ADR-0003: Use wgpu for Radar Data Rendering

## Status
Accepted

## Context
Radar data must be rendered directly to GPU — not through egui's rendering path — to
achieve the performance and visual quality required by the instrument principle. A
cross-platform GPU abstraction is needed that runs natively on Linux (Vulkan primary,
OpenGL fallback), is pure Rust, and can be embedded within an egui application frame.
Raw Vulkan was considered but rejected as unnecessarily complex for this use case.
OpenGL directly was considered but rejected as dated and lacking a clean Rust-native API.

## Decision
wgpu is the GPU rendering layer for all geospatial content: map underlay, radar data,
vector overlays, and placefile content.

## Consequences
- Runs natively on Vulkan on Linux, with OpenGL ES fallback for older hardware.
- Pure Rust API with no unsafe code required for normal usage.
- Serves as the WebGPU implementation in Firefox, Servo, and Deno — it is production
  grade and not experimental.
- The wgpu API has historically moved quickly with breaking changes between versions.
  The project pins to a specific wgpu version and upgrades deliberately, not
  automatically.
- Radar data rendering bypasses egui entirely, giving full control over the render
  pipeline for the display surface. egui and wgpu share the window and swap chain but
  not the render pipeline.
- Shader code is written in WGSL, wgpu's native shader language, which is portable
  across all wgpu backends.
