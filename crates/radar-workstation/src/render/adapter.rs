//! GPU adapter selection (ADR-0024). The render loop must run on the GPU that
//! *drives the display*, not the lowest-power one: on a hybrid-GPU Linux box
//! `PowerPreference::LowPower` picks the integrated adapter, the swapchain
//! images are allocated there with its tiling modifiers, and a compositor
//! that scans out only on the discrete GPU cannot import those dmabufs — the
//! Wayland connection is torn down several frames after a "successful" init.
//!
//! This module is split so the decision is pure and the I/O is thin:
//!
//! - [`rank`] and [`GpuOverride`] reason over injected [`AdapterFacts`] /
//!   [`DisplayDevice`] values and are unit-tested with no GPU.
//! - [`discover_displays`] is the only part that touches the filesystem, and
//!   **every** failure in it — a missing path, an unreadable file, malformed
//!   hex, no connectors — yields a shorter list, never an error, never a
//!   panic.
//!
//! Ranking is a *prediction* about compositor behaviour, so `render::gpu`
//! checks it by actually bringing a surface up on each candidate in order
//! (ADR-0024 §4.3). A compositor-side dmabuf-import rejection is asynchronous
//! and is not caught by that bring-up; correct selection is what avoids it,
//! and `render::mod`'s late-failure guard is the backstop.

use std::path::Path;

/// A display-capable GPU discovered from the kernel (sysfs), independent of
/// wgpu. Identifies which PCI device actually drives a connected output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayDevice {
    /// PCI address, `bus:device.function` form, e.g. `0000:01:00.0` — the
    /// same string wgpu reports in `AdapterInfo::device_pci_bus_id` on the
    /// Vulkan backend, so the match is exact rather than a heuristic.
    pub pci_address: String,
    pub vendor_id: u32,
    pub device_id: u32,
}

/// A projection of `wgpu::AdapterInfo` plus one surface fact, so [`rank`]
/// needs no GPU to test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterFacts {
    /// Index into the enumerated-adapter list; also the identity [`rank`]
    /// returns and the deterministic tie-break key.
    pub index: usize,
    pub name: String,
    /// `AdapterInfo::device_pci_bus_id`. Empty on the GL backend — which is
    /// why `(vendor, device)` is a second key in [`rank`].
    pub pci_address: String,
    pub vendor: u32,
    pub device: u32,
    pub device_type: wgpu::DeviceType,
    pub backend: wgpu::Backend,
    /// `!surface.get_capabilities(adapter).formats.is_empty()` — the adapter
    /// can produce at least one surface format for this window.
    pub presents_to_surface: bool,
}

/// Why [`rank`] (or an override) put the chosen adapter first — carried onto
/// `Gpu` so the startup diagnostic line can explain the choice in one line
/// (ADR-0024 §S6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionReason {
    /// Exact `pci_address` match against a connected display.
    MatchedDisplay,
    /// `(vendor, device)` match — used when the bus id was unavailable.
    MatchedDisplayVendorDevice,
    /// No display information was discoverable; this is the highest-ranked
    /// adapter that can present.
    NoDisplayInfo,
    /// `RADAR_GPU` forced this adapter (it still passed the bring-up check).
    ForcedByOverride,
    /// No hardware adapter could present; a software rasteriser was selected.
    SoftwareRasteriser,
}

impl std::fmt::Display for SelectionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::MatchedDisplay => "matched connected display",
            Self::MatchedDisplayVendorDevice => "matched display vendor/device",
            Self::NoDisplayInfo => "no display info — highest ranked presentable adapter",
            Self::ForcedByOverride => "forced by RADAR_GPU",
            Self::SoftwareRasteriser => "software rasteriser — no hardware adapter could present",
        };
        f.write_str(s)
    }
}

/// Score one adapter against the discovered displays. Higher is better; see
/// ADR-0024 §4.2 for the rule numbers.
fn score(a: &AdapterFacts, displays: &[DisplayDevice], have_display_info: bool) -> i32 {
    let mut s = 0;
    if !a.pci_address.is_empty()
        && displays.iter().any(|d| d.pci_address.eq_ignore_ascii_case(&a.pci_address))
    {
        s += 1000; // rule 3: exact PCI match
    } else if displays.iter().any(|d| d.vendor_id == a.vendor && d.device_id == a.device) {
        s += 500; // rule 4: vendor/device match (bus id unavailable)
    }
    // rule 5: no display info at all — lean toward the discrete adapter,
    // because "integrated adapter that cannot present" is the failure being
    // fixed. Deliberately *not* applied when a display was found: there, an
    // iGPU that owns the panel has already scored +1000 and should win.
    if !have_display_info && a.device_type == wgpu::DeviceType::DiscreteGpu {
        s += 100;
    }
    if a.backend == wgpu::Backend::Vulkan {
        s += 10; // rule 6: ADR-0003 primary over the GL fallback
    }
    s
}

/// Rank presentable adapters, best first. Pure over injected facts.
///
/// - Non-presentable adapters are absent from the output entirely (rule 1).
/// - `DeviceType::Cpu` is excluded unless nothing else can present (rule 2);
///   the caller reports a software rasteriser on the startup line.
/// - Ties break by `index`, so ordering is deterministic across runs (rule 7).
pub fn rank(adapters: &[AdapterFacts], displays: &[DisplayDevice]) -> Vec<usize> {
    let have_display_info = !displays.is_empty();

    let hardware: Vec<&AdapterFacts> = adapters
        .iter()
        .filter(|a| a.presents_to_surface && a.device_type != wgpu::DeviceType::Cpu)
        .collect();

    let candidates: Vec<&AdapterFacts> = if hardware.is_empty() {
        // rule 2: only now is a software rasteriser a candidate.
        adapters.iter().filter(|a| a.presents_to_surface).collect()
    } else {
        hardware
    };

    let mut scored: Vec<(i32, usize)> =
        candidates.iter().map(|a| (score(a, displays, have_display_info), a.index)).collect();
    // score descending, then index ascending — stable and deterministic.
    scored.sort_by(|l, r| r.0.cmp(&l.0).then(l.1.cmp(&r.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

/// Why the top-ranked candidate was chosen, for the startup diagnostic.
pub fn reason_for(chosen: &AdapterFacts, displays: &[DisplayDevice]) -> SelectionReason {
    if chosen.device_type == wgpu::DeviceType::Cpu {
        return SelectionReason::SoftwareRasteriser;
    }
    if !chosen.pci_address.is_empty()
        && displays.iter().any(|d| d.pci_address.eq_ignore_ascii_case(&chosen.pci_address))
    {
        return SelectionReason::MatchedDisplay;
    }
    if displays.iter().any(|d| d.vendor_id == chosen.vendor && d.device_id == chosen.device) {
        return SelectionReason::MatchedDisplayVendorDevice;
    }
    SelectionReason::NoDisplayInfo
}

/// The operator override, `RADAR_GPU` — the escape hatch for when the
/// heuristic is wrong (ADR-0024 §4.5). It bypasses [`rank`] but **not** the
/// bring-up check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuOverride {
    /// A PCI address, `0000:01:00.0` form.
    PciAddress(String),
    /// A case-insensitive substring of the adapter name, e.g. `nvidia`.
    NameSubstring(String),
    /// The first discrete GPU.
    Discrete,
    /// The first integrated GPU.
    Integrated,
}

/// `RADAR_GPU` was set to a value that matched no enumerated adapter.
#[derive(Debug, Clone)]
pub struct OverrideUnmatched {
    pub requested: String,
    pub available: Vec<String>,
}

impl GpuOverride {
    /// Parse the raw `RADAR_GPU` value. An empty or whitespace-only value is
    /// "unset" (`None`). Every other value parses to `Some(_)`: a bare word
    /// is a name substring at worst. Whether it actually *matches* an adapter
    /// is decided later by [`select`](Self::select), against the real list.
    pub fn parse(raw: &str) -> Option<GpuOverride> {
        let v = raw.trim();
        if v.is_empty() {
            return None;
        }
        Some(match v.to_ascii_lowercase().as_str() {
            "discrete" => GpuOverride::Discrete,
            "integrated" => GpuOverride::Integrated,
            lower if looks_like_pci(lower) => GpuOverride::PciAddress(lower.to_string()),
            lower => GpuOverride::NameSubstring(lower.to_string()),
        })
    }

    fn matches(&self, a: &AdapterFacts) -> bool {
        match self {
            GpuOverride::PciAddress(p) => a.pci_address.eq_ignore_ascii_case(p),
            GpuOverride::NameSubstring(s) => a.name.to_ascii_lowercase().contains(s),
            GpuOverride::Discrete => a.device_type == wgpu::DeviceType::DiscreteGpu,
            GpuOverride::Integrated => a.device_type == wgpu::DeviceType::IntegratedGpu,
        }
    }

    /// Indices of adapters this override selects, in `index` order. An empty
    /// match is an [`OverrideUnmatched`] error listing what is available —
    /// never a panic, never a silent fallback to ranking.
    pub fn select(&self, adapters: &[AdapterFacts]) -> Result<Vec<usize>, OverrideUnmatched> {
        let hits: Vec<usize> = adapters.iter().filter(|a| self.matches(a)).map(|a| a.index).collect();
        if hits.is_empty() {
            return Err(OverrideUnmatched {
                requested: self.describe(),
                available: adapters
                    .iter()
                    .map(|a| format!("{} ({:?}, {})", a.name, a.backend, pci_or_dash(&a.pci_address)))
                    .collect(),
            });
        }
        Ok(hits)
    }

    fn describe(&self) -> String {
        match self {
            GpuOverride::PciAddress(p) => format!("PCI address {p}"),
            GpuOverride::NameSubstring(s) => format!("name containing {s:?}"),
            GpuOverride::Discrete => "discrete".to_string(),
            GpuOverride::Integrated => "integrated".to_string(),
        }
    }
}

/// Render `pci_address`, or `-` when the backend did not report one.
pub fn pci_or_dash(pci: &str) -> &str {
    if pci.is_empty() {
        "-"
    } else {
        pci
    }
}

/// `0000:01:00.0` shape — four hex, colon, two hex, colon, two hex, dot, one
/// digit. Only used to disambiguate a `RADAR_GPU` value from a name substring.
fn looks_like_pci(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 12
        && bytes[..4].iter().all(u8::is_ascii_hexdigit)
        && bytes[4] == b':'
        && bytes[5..7].iter().all(u8::is_ascii_hexdigit)
        && bytes[7] == b':'
        && bytes[8..10].iter().all(u8::is_ascii_hexdigit)
        && bytes[10] == b'.'
        && bytes[11].is_ascii_digit()
}

/// Discover display-capable GPUs from a sysfs tree (`/sys` in production;
/// a fixture root in tests). Scans `{root}/class/drm/card*-*`, keeps
/// connectors whose `status` reads `connected`, resolves the parent card
/// (`card2-HDMI-A-2` → `card2`), and reads `card2/device`'s `vendor` /
/// `device` (hex) plus the PCI address from the last component of that
/// symlink's target. Deduplicates by PCI address. Any failure is a shorter
/// list, never an `Err`.
pub fn discover_displays(sysfs_root: &Path) -> Vec<DisplayDevice> {
    let drm = sysfs_root.join("class").join("drm");
    let Ok(entries) = std::fs::read_dir(&drm) else {
        return Vec::new();
    };

    let mut out: Vec<DisplayDevice> = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else { continue };
        // A connector is "card<N>-<CONNECTOR>"; the bare "card<N>" primary /
        // render node has no '-' and is skipped.
        let Some((card, _connector)) = name.split_once('-') else { continue };
        if !card.starts_with("card") {
            continue;
        }

        match std::fs::read_to_string(drm.join(name).join("status")) {
            Ok(status) if status.trim().eq_ignore_ascii_case("connected") => {}
            _ => continue,
        }

        let Some(device) = read_pci_device(&drm.join(card).join("device")) else { continue };
        if !out.iter().any(|d| d.pci_address == device.pci_address) {
            out.push(device);
        }
    }
    out
}

fn read_pci_device(device_link: &Path) -> Option<DisplayDevice> {
    // The PCI address is the last path component of the symlink target,
    // e.g. `../../../devices/pci0000:00/0000:01:00.0` → `0000:01:00.0`.
    let target = std::fs::read_link(device_link).ok()?;
    let pci_address = target.file_name()?.to_str()?.to_string();
    Some(DisplayDevice {
        pci_address,
        vendor_id: read_hex(&device_link.join("vendor"))?,
        device_id: read_hex(&device_link.join("device"))?,
    })
}

fn read_hex(path: &Path) -> Option<u32> {
    let raw = std::fs::read_to_string(path).ok()?;
    let raw = raw.trim();
    u32::from_str_radix(raw.strip_prefix("0x").unwrap_or(raw), 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compact `AdapterFacts` builder for the ranking tests. `ids` is
    /// `(vendor, device)`; everything a given test does not care about takes
    /// a presentable Vulkan default.
    fn a(
        index: usize,
        name: &str,
        pci: &str,
        ids: (u32, u32),
        kind: wgpu::DeviceType,
        backend: wgpu::Backend,
    ) -> AdapterFacts {
        AdapterFacts {
            index,
            name: name.to_string(),
            pci_address: pci.to_string(),
            vendor: ids.0,
            device: ids.1,
            device_type: kind,
            backend,
            presents_to_surface: true,
        }
    }

    use wgpu::Backend::{Gl, Vulkan};
    use wgpu::DeviceType::{Cpu, DiscreteGpu, IntegratedGpu};

    fn display(pci: &str, vendor: u32, device: u32) -> DisplayDevice {
        DisplayDevice { pci_address: pci.to_string(), vendor_id: vendor, device_id: device }
    }

    // The reproducing desktop: NVIDIA dGPU drives the only connected output;
    // an AMD iGPU is present but drives nothing. The dGPU must rank first.
    #[test]
    fn discrete_that_owns_the_display_ranks_first() {
        let adapters = [
            a(0, "AMD Radeon Graphics", "0000:0c:00.0", (0x1002, 0x164e), IntegratedGpu, Vulkan),
            a(1, "NVIDIA GeForce RTX 4070 SUPER", "0000:01:00.0", (0x10de, 0x2705), DiscreteGpu, Vulkan),
        ];
        let displays = [display("0000:01:00.0", 0x10de, 0x2705)];
        assert_eq!(rank(&adapters, &displays), vec![1, 0]);
        assert_eq!(reason_for(&adapters[1], &displays), SelectionReason::MatchedDisplay);
    }

    // A laptop: the iGPU owns the built-in panel, a dGPU is also present.
    // Principle 3's lightweight intent is preserved where it is achievable —
    // the iGPU wins because it matches the connected display.
    #[test]
    fn integrated_that_owns_the_panel_ranks_first() {
        let adapters = [
            a(0, "Intel Arc Graphics", "0000:00:02.0", (0x8086, 0x7d55), IntegratedGpu, Vulkan),
            a(1, "NVIDIA GeForce RTX 4060", "0000:01:00.0", (0x10de, 0x28e0), DiscreteGpu, Vulkan),
        ];
        let displays = [display("0000:00:02.0", 0x8086, 0x7d55)];
        assert_eq!(rank(&adapters, &displays)[0], 0);
    }

    #[test]
    fn single_adapter_ranks_itself() {
        let adapters = [a(0, "llvmpipe", "", (0x10005, 0), Cpu, Vulkan)];
        assert_eq!(rank(&adapters, &[]), vec![0]);
        assert_eq!(reason_for(&adapters[0], &[]), SelectionReason::SoftwareRasteriser);
    }

    #[test]
    fn no_display_info_prefers_presentable_discrete_then_integrated_then_cpu() {
        let adapters = [
            a(0, "llvmpipe", "", (0, 0), Cpu, Vulkan),
            a(1, "Integrated", "0000:00:02.0", (0x8086, 1), IntegratedGpu, Vulkan),
            a(2, "Discrete", "0000:01:00.0", (0x10de, 1), DiscreteGpu, Vulkan),
        ];
        assert_eq!(rank(&adapters, &[]), vec![2, 1]);
        assert_eq!(reason_for(&adapters[2], &[]), SelectionReason::NoDisplayInfo);
    }

    #[test]
    fn non_presentable_adapters_are_absent_from_the_ranking() {
        let mut absent = a(0, "Discrete no-present", "0000:01:00.0", (0x10de, 1), DiscreteGpu, Vulkan);
        absent.presents_to_surface = false;
        let adapters = [absent, a(1, "Integrated presents", "0000:00:02.0", (0x8086, 1), IntegratedGpu, Vulkan)];
        assert_eq!(rank(&adapters, &[]), vec![1]);
    }

    #[test]
    fn exact_pci_match_outranks_vendor_device_match() {
        let adapters = [
            // same vendor/device as the display, but no bus id → +500
            a(0, "GPU A", "", (0x10de, 0x2705), DiscreteGpu, Vulkan),
            // exact bus id → +1000
            a(1, "GPU B", "0000:01:00.0", (0x10de, 0x2705), DiscreteGpu, Vulkan),
        ];
        let displays = [display("0000:01:00.0", 0x10de, 0x2705)];
        assert_eq!(rank(&adapters, &displays), vec![1, 0]);
    }

    #[test]
    fn equal_scores_order_by_index_and_are_stable() {
        let adapters = [
            a(0, "A", "0000:03:00.0", (1, 1), DiscreteGpu, Vulkan),
            a(1, "B", "0000:04:00.0", (2, 2), DiscreteGpu, Vulkan),
            a(2, "C", "0000:05:00.0", (3, 3), DiscreteGpu, Vulkan),
        ];
        assert_eq!(rank(&adapters, &[]), vec![0, 1, 2]);
    }

    #[test]
    fn vulkan_outranks_gl_at_equal_display_match() {
        let adapters = [
            a(0, "GL adapter", "", (0x10de, 0x2705), DiscreteGpu, Gl),
            a(1, "Vulkan adapter", "", (0x10de, 0x2705), DiscreteGpu, Vulkan),
        ];
        assert_eq!(rank(&adapters, &[]), vec![1, 0]);
    }

    #[test]
    fn override_parses_every_form() {
        assert_eq!(GpuOverride::parse(""), None);
        assert_eq!(GpuOverride::parse("   "), None);
        assert_eq!(GpuOverride::parse("discrete"), Some(GpuOverride::Discrete));
        assert_eq!(GpuOverride::parse("INTEGRATED"), Some(GpuOverride::Integrated));
        assert_eq!(
            GpuOverride::parse("0000:01:00.0"),
            Some(GpuOverride::PciAddress("0000:01:00.0".to_string()))
        );
        assert_eq!(
            GpuOverride::parse("NVIDIA"),
            Some(GpuOverride::NameSubstring("nvidia".to_string()))
        );
    }

    #[test]
    fn override_selects_and_reports_unmatched() {
        let adapters = [
            a(0, "AMD Radeon Graphics", "0000:0c:00.0", (0x1002, 1), IntegratedGpu, Vulkan),
            a(1, "NVIDIA GeForce RTX 4070 SUPER", "0000:01:00.0", (0x10de, 1), DiscreteGpu, Vulkan),
        ];
        assert_eq!(GpuOverride::parse("nvidia").unwrap().select(&adapters).unwrap(), vec![1]);
        assert_eq!(GpuOverride::parse("0000:0c:00.0").unwrap().select(&adapters).unwrap(), vec![0]);
        assert_eq!(GpuOverride::parse("discrete").unwrap().select(&adapters).unwrap(), vec![1]);

        let err = GpuOverride::parse("nonsense").unwrap().select(&adapters).unwrap_err();
        assert_eq!(err.available.len(), 2);
        assert!(err.available.iter().any(|a| a.contains("NVIDIA")));
    }

    #[test]
    fn discover_displays_missing_class_drm_is_empty_not_an_error() {
        let dir = std::env::temp_dir().join("radar-workstation-no-drm-test");
        let _ = std::fs::create_dir_all(&dir);
        assert!(discover_displays(&dir).is_empty());
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sysfs").join(name)
    }

    #[test]
    fn discover_displays_desktop_finds_only_the_connected_gpu() {
        let displays = discover_displays(&fixture("desktop_nvidia"));
        assert_eq!(displays, vec![display("0000:01:00.0", 0x10de, 0x2705)]);
    }

    #[test]
    fn discover_displays_laptop_finds_the_panel_gpu() {
        let displays = discover_displays(&fixture("laptop_intel"));
        assert_eq!(displays, vec![display("0000:00:02.0", 0x8086, 0x7d55)]);
    }

    #[test]
    fn discover_displays_ignores_disconnected_and_unknown_status() {
        assert!(discover_displays(&fixture("all_disconnected")).is_empty());
    }

    #[test]
    fn discover_displays_skips_a_card_with_a_missing_vendor_file() {
        assert!(discover_displays(&fixture("missing_vendor")).is_empty());
    }

    #[test]
    fn discover_displays_skips_malformed_hex() {
        assert!(discover_displays(&fixture("malformed_hex")).is_empty());
    }
}
