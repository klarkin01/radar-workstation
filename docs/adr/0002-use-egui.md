# ADR-0002: Use egui as the UI Framework

## Status
Accepted

## Context
A native UI framework is required for the application shell — menus, toolbars, product
selector, color scale, status bar. The framework must be pure Rust, GPU-accelerated,
actively maintained, and compatible with embedding a custom wgpu render surface. OS widget
conformance is not required; the application does not need to look like the
surrounding desktop environment. Qt via cxx-qt bindings was considered but rejected due to
the complexity of a Rust/C++ bridge and the introduction of a large C++ dependency.

## Decision
egui is the UI framework for the application.

## Consequences
- Pure Rust — no C++ dependency, no FFI boundary in the UI layer.
- Immediate mode rendering means the UI is always consistent with application state with
  no reconciliation layer.
- GPU-accelerated via wgpu, compatible with embedding a custom wgpu render pass for the
  radar display surface.
- Does not produce native OS widgets. This is acceptable and consistent with the
  instrument philosophy — the application defines its own visual language.
- egui is actively maintained and used in production tools. It is the most mature
  pure-Rust immediate mode UI option available.
- Complex retained-mode widget patterns (e.g. large scrollable lists) require more
  deliberate implementation in immediate mode. Acceptable given the application's
  relatively simple UI structure.
