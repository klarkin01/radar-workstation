# ADR-0007: Pluggable XYZ Tile Providers for Background Imagery

## Status
Accepted — **implementation deferred to post-v1.0** (2026-08-28).

The decision below stands as written and is not superseded or reopened. What changed is
timing: [ADR-0027](0027-tile-image-decoding.md) defers the tile subsystem out of v1.0,
because decoding tile bodies requires a new untrusted-input parser on a network path in
service of a layer this ADR itself describes as optional and off by default. v1.0 ships a
vector-only basemap ([ADR-0025](0025-bundled-overlay-geometry.md)). No tile-fetching
code exists, and none is stubbed — see ADR-0027 §3.

## Context
GR2Analyst provides satellite/terrain background imagery that activates at closer zoom
levels, sourced from NASA JPL and USDA WMS servers. Those specific endpoints have
reliability issues and may no longer be fully functional. A more robust and flexible
approach is needed. The imagery layer must be optional and toggleable, consistent with
the philosophy that a dark background is a legitimate and preferred choice for many users.

## Decision
Background imagery is provided via a pluggable XYZ tile provider model. The tile fetching
layer accepts a URL template (e.g. `https://server/{z}/{x}/{y}.png`), fetches tiles on
demand, and caches them to disk. USGS National Map terrain tiles are the default provider.
Users may configure alternative providers. The imagery layer is off by default and toggleable.

## Consequences
- No single tile server dependency. If a provider changes URLs or goes offline, switching
  providers requires only a configuration change.
- Disk tile cache means previously viewed areas load instantly without re-fetching.
  Critical for operational use in degraded-network environments.
- USGS National Map is a US government source — free, no API key, stable, and
  appropriate for a government-facing application.
- Tile fetching is handled by the tokio async layer and never blocks the render loop.
- Users in air-gapped or classified environments can disable imagery entirely and rely
  on the bundled vector overlays. The application is fully functional without imagery.
- Future providers (higher resolution imagery, alternative terrain styles) can be added
  without architectural changes.
