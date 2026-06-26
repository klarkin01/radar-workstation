# Open Questions

*Unresolved design questions that need answers before or during implementation of the
relevant subsystem. Remove a question when it is resolved, and record the decision in
an ADR if it is architecturally significant.*

---

## Critical — Must Resolve Before Implementation



---

## Architecture — Resolve Before the Relevant Subsystem

**Q4: Exact shared state structure?**
`Arc<RwLock<AppState>>` is the chosen pattern. The structure of `AppState` needs to be
defined: what it holds, how it is partitioned, and whether a single lock or multiple
finer-grained locks better serves the read/write patterns of the render loop vs. the
data pipeline.

**Q5: How are multiple instances coordinated, if at all?**
Each instance is designed to be fully independent. Is there any case where instances
should share resources — for example, a shared on-disk tile cache to avoid redundant
downloads when multiple instances are monitoring sites in the same geographic area?
If yes, this requires a cache access coordination mechanism. If no, document explicitly.

**Q6: What is the placefile format support scope?**
GRLevelX placefile format support is planned. How complete does this implementation need
to be at v1.0? The format has a broad feature set. Define a minimum viable subset that
covers the most widely used placefiles (warnings, storm reports, METARs, lightning) and
defer less common features.

**Q7: How is the disk tile cache managed?**
Tile caching requires a cache eviction policy, a maximum size, and a directory location.
Define: default cache location (XDG cache dir convention on Linux), maximum cache size
(configurable?), eviction policy (LRU by last access time is standard), and whether
cache is shared across instances or per-instance.

---

## Data and Products — Resolve During Decoder Implementation

**Q8: Which derived products are in scope for v1.0?**
GR2Analyst derives Echo Tops, VIL, VILD, POSH, and MEHS from Level II reflectivity.
Define the v1.0 product set. A conservative starting point: base reflectivity, base
velocity, storm-relative velocity, spectrum width (all tilts), plus Echo Tops and VIL
as derived products. Dual-pol products (ZDR, CC, KDP) are high value but add decoder
and rendering complexity.

**Q9: Velocity dealiasing — implement or defer?**
Velocity aliasing is a known limitation of raw Doppler data that significantly affects
usability of the velocity product. GR2Analyst implements dealiasing. This is
algorithmically non-trivial. Decide whether v1.0 ships with dealiasing, ships with
a known limitation notice, or ships with a simple range-folding indicator only.

**Q13: Backup data source?**
We will default to NOAA S3 for our primary data source. However, we may want to implement
a backup/secondary source, or the ability to configure multiple sources. The ability to 
configure sources would be most flexible, but likely more involved. It is also likely that 
the NOAA S3 source would be most authoritative and reliable.  

---

## Rendering — Resolve During Rendering Subsystem Design

**Q10: What projection is used for the display?**
NEXRAD data is in polar coordinates centered on the radar site. The map underlay uses
geographic coordinates (WGS84). A projection is needed to render both in a common space.
Azimuthal equidistant projection centered on the radar site is the conventional choice
for single-site radar display and matches GR2Analyst's behavior. Confirm this is correct
and document the coordinate transform pipeline.

**Q11: How is color table / palette support handled?**
GR2Analyst supports user-supplied color tables in a documented format, and a large
community ecosystem of custom palettes exists. Supporting GRLevelX-compatible color
table format would give immediate access to this ecosystem. Define: which palette format
to support, where user palettes are stored, and how defaults are shipped with the
application.

---

## Distribution — Resolve Before First Public Release

**Q12: What is the Linux distribution strategy?**
Options: AppImage (broadest compatibility, self-contained), Flatpak (sandboxed, good
for desktop Linux users), native packages (deb/rpm — higher maintenance burden),
or direct binary download. AppImage is the lowest-friction starting point. Flatpak
has security sandbox implications worth evaluating given the government use case.

**Q13: What are the minimum system requirements?**
Define minimum: Linux kernel version, GPU requirements (OpenGL version for the wgpu
GL backend fallback), RAM, and CPU. Users on older hardware or headless servers with
software rendering are explicitly out of scope — document this clearly.
