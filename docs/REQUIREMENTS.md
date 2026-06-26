# Requirements

*This document defines what Radar Workstation must do, what it must not do, and what
the v1.0 scope boundary is. It sits between [PHILOSOPHY.md](PHILOSOPHY.md) (the principles
that govern decisions) and the architecture documents (the implementation that satisfies
these requirements). Requirements here are derived from and consistent with all accepted
ADRs and architecture documents. Where a requirement remains open pending a design
decision, it is marked **[OPEN — Qn]** with a reference to the relevant question in
[open-questions.md](open-questions.md).*

---

## 1. Scope

Radar Workstation is a single-site NEXRAD Level II radar analysis application for
Linux. Each running instance monitors one radar site, maintains its own independent
data pipeline, and renders to its own window. There is no server component, no
background daemon, no shared state between instances, and no network connection beyond
those the user explicitly configures.

The primary reference application is GR2Analyst (Windows-only, commercial). This is
the standard against which professional usability is measured.

---

## 2. Functional Requirements

### 2.1 Data Acquisition

**FR-DA-1.** The application must fetch NEXRAD Level II data from the NOAA AWS S3
public bucket (`s3://noaa-nexrad-level2`) via HTTPS. No authentication, API key, or
AWS SDK is required — access uses plain HTTP GET.

**FR-DA-2.** The application must poll for new volume scans on a configurable interval.
The default polling interval is 30 seconds. Polling must not block the render loop or
UI.

**FR-DA-3.** On launch, the application must fetch and display the most recent available
scan for the selected site before accepting the next polling cycle.

**FR-DA-4.** On site change, the application must immediately cancel the current polling
task, clear site-specific state, and begin fetching the most recent scan for the new
site. The user must see the new site's data within the time target defined in Section 5.1.

**FR-DA-5.** Network failures during polling must be handled gracefully. The last
successfully fetched scan must remain displayed. The status bar must indicate the error
condition and the age of the displayed data.

**FR-DA-6.** The application must fetch map imagery tiles via HTTPS from the configured
XYZ tile provider. Tile fetching must not block the render loop — missing tiles render
as transparent until delivered.

**FR-DA-7.** The application must fetch placefile data via HTTP or HTTPS from
user-configured URLs, on a per-placefile polling interval.

**FR-DA-8.** **[OPEN — Q3]** Fallback data source behavior: if AWS S3 is unavailable,
define whether the application falls back to an alternate source (e.g., Iowa State
Mesonet) or displays a degraded-mode error. Primary source (AWS S3) is confirmed.
Fallback behavior is unresolved.

---

### 2.2 NEXRAD Decoding

**FR-ND-1.** The decoder must parse NEXRAD Level II archive format files (WSR-88D),
bzip2 compressed, into an internal `VolumeScan` representation.

**FR-ND-2.** The decoder must support both per-radial (legacy) and Message 31 format
variants.

**FR-ND-3.** The decoder must support both standard-resolution and super-resolution
scan variants.

**FR-ND-4.** The decoder must extract all available moment data: reflectivity, velocity,
spectrum width, and dual-pol moments (ZDR, CC, KDP) where present in the scan.

**FR-ND-5.** All decoded moment values must be expressed in calibrated physical units:
dBZ for reflectivity, m/s for velocity, dB for ZDR and KDP, dimensionless for CC.

**FR-ND-6.** The decoder must return typed errors for all failure modes: truncated
files, corrupt headers, decompression failures, and unsupported format versions. It
must never panic on malformed input.

**FR-ND-7.** A decode failure must not crash or freeze the application. The most
recently successfully decoded scan must remain displayed, and the error must be surfaced
in the status bar.

**FR-ND-8.** The decoder is a separate library crate (`nexrad-decoder`) with its own
test suite. The test suite must exercise: known-good files from the NCEI archive
(multiple sites, scan modes, and eras), corrupt and truncated input, dual-pol and
non-dual-pol variants, and both resolution variants.

---

### 2.3 Radar Products

**FR-RP-1.** The application must display the following base products for all available
elevation tilts:
- Base reflectivity
- Base velocity
- Spectrum width

**FR-RP-2.** The application must derive and display the following products from the
decoded volume scan:
- Echo Tops (derived from multi-tilt reflectivity)
- VIL — Vertically Integrated Liquid (derived from reflectivity)

**FR-RP-3.** **[OPEN — Q8]** Dual-pol products (ZDR, CC, KDP): whether these are
included in the v1.0 product set is unresolved. They are decoded (FR-ND-4) in all
cases. The question is whether the compute and display pipeline for them is implemented
at v1.0. Conservative default: deferred to a post-v1.0 release.

**FR-RP-4.** **[OPEN — Q8]** Storm-relative velocity: requires a storm motion vector
input mechanism. Whether this is in v1.0 scope is unresolved.

**FR-RP-5.** **[OPEN — Q9]** Velocity dealiasing: raw Doppler velocity data contains
range-folded aliasing. Whether v1.0 ships with dealiasing, a known-limitation notice,
or a range-folding indicator only is unresolved.

**FR-RP-6.** All products must be pre-computed as color-mapped RGBA textures by the
compute layer before reaching the render loop. The render loop must not perform color
mapping or product derivation.

**FR-RP-7.** Switching between products or elevation tilts must not require re-fetching
or re-computing data. Product and tilt switches are GPU state changes only.

---

### 2.4 Display and Rendering

**FR-DR-1.** All geospatial content must be rendered in azimuthal equidistant projection
centered on the active radar site. This is the projection that preserves distance and
direction from the site — the correct reference frame for single-site radar interpretation.

**FR-DR-2.** The coordinate transform pipeline is:
- NEXRAD polar coordinates (range, azimuth) → Cartesian (km from site)
- Geographic coordinates (WGS84) → Azimuthal equidistant (km from site)
- Both → Screen pixels (zoom + pan transform)

**FR-DR-3.** Geospatial layers must be composited back-to-front in this order:
1. Background (solid dark color)
2. Terrain imagery tiles (optional, toggleable)
3. County boundaries (always visible)
4. State and country boundaries (always visible)
5. Major highways (toggleable)
6. Radar data (active product texture, alpha-composited)
7. Placefile overlays (in user-configured order)
8. Radar site markers
9. City labels (zoom-dependent)
10. UI chrome (egui, drawn last on top of all geospatial content)

**FR-DR-4.** Radar data below the minimum displayable threshold must render as fully
transparent, allowing map layers below to show through.

**FR-DR-5.** The steady-state render loop must target 60 fps. New scan arrivals must
not cause a perceptible frame rate drop.

**FR-DR-6.** The application must display a loading indicator when waiting for the
first scan after launch or site change.

**FR-DR-7.** The status bar must display: active site identifier, active product and
tilt, scan timestamp, polling status, and any active error conditions.

---

### 2.5 Map Underlays

**FR-MU-1.** County boundaries, state and country boundaries, and major highways must
be sourced from bundled Census TIGER/Line and Natural Earth shapefiles. These require
no network connection and must be available immediately at startup.

**FR-MU-2.** All vector overlay geometry must be pre-projected into azimuthal equidistant
coordinates at load time. The GPU receives already-projected geometry — no projection
happens per frame.

**FR-MU-3.** NEXRAD site locations must be sourced from a bundled site list derived
from the NOAA site registry. No network connection is required to populate the site
list.

**FR-MU-4.** The default map imagery tile provider must be the USGS National Map
(publicly accessible, no authentication). The tile provider URL must be user-configurable
to any XYZ-scheme HTTPS tile source.

**FR-MU-5.** Fetched map tiles must be cached to disk using an LRU eviction policy.
The cache must be stored in the XDG cache directory by default. **[OPEN — Q7]** Maximum
cache size and whether it is configurable are unresolved.

**FR-MU-6.** Each running instance maintains its own independent tile cache. **[OPEN —
Q5]** Whether instances may optionally share a single cache (to avoid redundant downloads
when monitoring geographically proximate sites) is unresolved.

---

### 2.6 Placefiles

**FR-PF-1.** The application must support the GRLevelX placefile format for user-supplied
overlays.

**FR-PF-2.** The application must support user-configuration of one or more placefile
URLs, each with an independent polling interval.

**FR-PF-3.** The minimum required placefile geometry types for v1.0 are: polygons
(warning outlines), polylines (storm tracks), icons (point markers), and text labels.

**FR-PF-4.** **[OPEN — Q6]** Full GRLevelX placefile feature scope for v1.0 is
unresolved. The minimum viable subset (FR-PF-3) covers the most widely used placefiles.
Additional features (HSV color blending, threshold conditions, etc.) may be deferred.

**FR-PF-5.** Placefile fetch failures must not crash or freeze the application. The
last successfully parsed placefile content must remain displayed. The error must be
surfaced in the status bar.

**FR-PF-6.** Placefiles must be rendered in user-configured order. The user must be
able to toggle individual placefiles on and off.

---

### 2.7 Site Selection

**FR-SS-1.** The application must support all operational NWS WSR-88D sites. The site
list is bundled with the binary; no network connection is required to enumerate sites.

**FR-SS-2.** The user must be able to change the active site at runtime without
restarting the application.

**FR-SS-3.** Radar site markers on the map must be clickable for site selection.

---

### 2.8 Navigation and Interaction

**FR-NI-1.** The user must be able to pan the map view using mouse drag and keyboard
input.

**FR-NI-2.** The user must be able to zoom using mouse wheel and keyboard input.

**FR-NI-3.** The user must be able to switch the active product and active elevation
tilt via keyboard input without requiring mouse interaction.

**FR-NI-4.** Pan and zoom state must be preserved across: new scan arrival, product
and tilt changes, placefile updates, and window resize. The view must never reset or
jump due to a data event. The user's spatial context is inviolable.

---

### 2.9 Color Tables

**FR-CT-1.** **[OPEN — Q11]** The color table format for user-supplied palettes is
unresolved. GRLevelX-compatible color table format is the strongly preferred choice,
as it gives immediate access to the existing community palette ecosystem. Confirm and
document the supported format.

**FR-CT-2.** Default color tables for all supported products must be bundled with the
application. The application must be usable with correct color mapping immediately
after installation, without requiring the user to supply palettes.

**FR-CT-3.** User-supplied color tables must be stored in the XDG config or data
directory. The application must load them at startup without requiring a restart after
installation.

---

### 2.10 Configuration Persistence

**FR-CP-1.** User configuration (active site, color table selections, placefile URLs
and polling intervals, tile provider URL, toggleable layer states) must persist across
application restarts.

**FR-CP-2.** Configuration must be stored in a plain-text, human-readable format in
the XDG config directory. The user must be able to edit configuration directly without
a separate configuration tool.

**FR-CP-3.** The application must start successfully with a missing or corrupt
configuration file, applying defaults. It must not crash on malformed configuration.

---

## 3. Behavioral Constraints

These are things the application must never do, under any circumstances. They are
derived from the security and instrument principles in [PHILOSOPHY.md](PHILOSOPHY.md)
and are non-negotiable.

**BC-1.** The application must never initiate a network connection that the user has
not explicitly configured. The only permitted connections are: NEXRAD data polling
(to the configured source), map tile fetching (to the configured tile provider), and
placefile fetching (to user-configured URLs). Nothing else.

**BC-2.** The application must never transmit telemetry, usage data, crash reports,
or any diagnostic information to any external server.

**BC-3.** The application must never require elevated privileges (root, sudo, setuid,
or capability bits) for installation, launch, or operation.

**BC-4.** Running instances must never communicate with each other. Each instance is
fully independent.

**BC-5.** The application must never write to any location outside its designated
directories: the XDG config directory (configuration), the XDG cache directory (tile
cache), and the XDG data directory (user palettes). It must not write to system
directories.

**BC-6.** The application must never crash on malformed, corrupt, truncated, or
unexpected NEXRAD data. All decode paths must return typed errors and be handled.

**BC-7.** The render loop must never initiate a network request, perform NEXRAD
decoding, or compute derived products. It reads shared state, uploads textures when
state changes, and draws. Nothing else.

**BC-8.** The application must never install or run a background service or daemon.
It runs as a normal user process and exits cleanly when the window is closed.

**BC-9.** The application must never use undocumented or unsafe memory operations
except where strictly necessary for FFI or hardware interaction, and any such use
must be isolated, documented, and justified.

---

## 4. Non-Functional Requirements

### 4.1 Performance

These are design targets. They must be validated during development, not assumed.

| Metric | Target |
|---|---|
| Frame rate (steady state) | 60 fps |
| Frame rate impact of new scan arrival | No perceptible drop |
| Time to first render after launch | < 2 seconds |
| Time to display after site change | < 5 seconds on a normal network connection |
| Memory per instance (steady state) | < 200 MB |
| GPU memory per instance | < 128 MB |
| Multiple simultaneous instances | No meaningful resource contention |

**NFR-P-1.** Multiple simultaneous instances — each monitoring a different radar site
— must be a supported and well-tested use case. Resource usage must scale approximately
linearly with instance count. An operator running four instances must not experience
degraded performance relative to running one.

---

### 4.2 Stability

**NFR-ST-1.** The application must not crash on any reachable error path: network
failures, corrupt or malformed data files, unexpected format variants, tile fetch
failures, placefile parse errors, or configuration errors.

**NFR-ST-2.** Panics are not acceptable on any code path that handles external data
(network responses, decoded files, configuration input). All `unwrap()` and `expect()`
calls on untrusted data are defects.

**NFR-ST-3.** All error conditions must surface to the user via the status bar. Silent
failures are not acceptable.

**NFR-ST-4.** The application must handle long-running operation gracefully: no memory
leaks over multi-hour sessions monitoring an active event.

---

### 4.3 Security

**NFR-SEC-1.** All network connections must use HTTPS. Plain HTTP is not permitted
for data fetching.

**NFR-SEC-2.** The dependency tree must be kept minimal. Each dependency is maintenance
surface and must be justified by necessity. Transitive dependencies are included in
this evaluation.

**NFR-SEC-3.** The dependency tree must pass `cargo audit` with no known vulnerabilities.
This must be enforced in CI.

**NFR-SEC-4.** Builds must be reproducible. The same source and dependency lock file
must produce a byte-identical binary, supporting independent verification.

**NFR-SEC-5.** No `unsafe` Rust may be introduced without a code comment explaining
why it is necessary and why it is sound. All unsafe blocks are subject to heightened
review.

**NFR-SEC-6.** The application must be approvable by security administrators in
government, corporate, and defense environments. Minimal privilege, no undisclosed
connections, no telemetry, and a fully auditable dependency tree are the basis for
that approval.

---

### 4.4 Usability

**NFR-UX-1.** The application must be operable entirely by keyboard for all primary
workflows: site selection, product switching, tilt switching, and navigation. Mouse
is supplementary, not required.

**NFR-UX-2.** The interface must be spatially stable. The user's view position must
never reset, jump, or reflow due to any data event. An operator who has spent time
positioning their view on an area of interest must not lose that context.

**NFR-UX-3.** The application must be usable at the skill level of an experienced
radar operator without documentation. Controls must follow established conventions
from reference applications in the domain.

**NFR-UX-4.** Startup time must not require the user to wait before the application
is responsive. The window must appear and the interface must be interactive before
the first scan has finished loading.

---

## 5. Platform

**PL-1.** The target platform is Linux. Windows and macOS are explicitly out of scope.

**PL-2.** The application must be a native compiled binary. No Electron wrapper, no
web runtime, no JVM or interpreter.

**PL-3.** **[OPEN — Q12]** The distribution mechanism is unresolved. Candidate options
are: AppImage (broadest compatibility), Flatpak (sandboxed desktop), native packages
(deb/rpm), or direct binary download. AppImage is the lowest-friction starting point.
Flatpak's security sandbox has implications for the government-approval use case that
must be evaluated.

**PL-4.** **[OPEN — Q13]** Minimum system requirements are unresolved. Must define:
minimum Linux kernel version, GPU requirements (Vulkan support required; OpenGL fallback
via wgpu's GL backend for older hardware?), minimum RAM, and minimum CPU. Headless
servers and software rendering are explicitly not supported — this must be documented
clearly.

---

## 6. v1.0 Scope Boundary

This section defines what is in and out of scope for the first public release. It is
the authoritative definition of "done" for v1.0.

### In Scope

- Single-site NEXRAD Level II analysis for all operational WSR-88D sites
- Base products: reflectivity, velocity, spectrum width (all tilts)
- Derived products: Echo Tops, VIL
- Full tilt set access and tilt switching
- Bundled vector map overlays: counties, states, country boundaries, major highways
- Pluggable XYZ map imagery tile provider (USGS default)
- On-disk tile cache
- GRLevelX placefile support (minimum viable subset — FR-PF-3)
- User-configurable placefile URLs with polling
- Keyboard-driven product, tilt, and navigation controls
- Bundled default color tables for all in-scope products
- User-supplied color table support **[OPEN — Q11]**
- Configuration persistence across restarts
- Multiple simultaneous independent instances

### Explicitly Deferred (Post-v1.0)

- Dual-pol products (ZDR, CC, KDP) in the display pipeline **[OPEN — Q8]**
- Storm-relative velocity **[OPEN — Q8]**
- Velocity dealiasing **[OPEN — Q9]**
- Full GRLevelX placefile feature set beyond the minimum viable subset **[OPEN — Q6]**
- Multi-site display (dual-panel, side-by-side comparison)
- NEXRAD Level III product support
- TDWR site support
- Regional or national radar composites
- Animation / loop playback of archived scans
- Windows or macOS support
- Mobile or web interface

### Explicitly Out of Scope (Not a Future Feature)

- Multi-site data fusion or mosaicking (contradicts the single-site instrument principle)
- Plugin or extension system (contradicts the restraint principle)
- Cloud sync or account system (contradicts the security and privacy principles)
- Telemetry or usage analytics (prohibited by BC-2)
- Any network connection not enumerated in BC-1

---

## 7. Open Requirements

The following requirements are explicitly incomplete pending resolution of open design
questions. Each is linked to the relevant question in
[open-questions.md](open-questions.md). These must be resolved before implementation
of the relevant subsystem begins.

| ID | Requirement | Blocked On |
|---|---|---|
| FR-DA-8 | Fallback NEXRAD data source behavior | Q3 |
| FR-MU-5 | Tile cache maximum size and configurability | Q7 |
| FR-MU-6 | Cross-instance tile cache sharing | Q5 |
| FR-RP-3 | Dual-pol products in v1.0 | Q8 |
| FR-RP-4 | Storm-relative velocity in v1.0 | Q8 |
| FR-RP-5 | Velocity dealiasing in v1.0 | Q9 |
| FR-PF-4 | Full placefile feature scope for v1.0 | Q6 |
| FR-CT-1 | Color table format | Q11 |
| PL-3 | Distribution mechanism | Q12 |
| PL-4 | Minimum system requirements | Q13 |

When an open question is resolved, update the corresponding requirement here, remove
the **[OPEN]** marker, and close the question in open-questions.md. If the resolution
is architecturally significant, record it as an ADR.
