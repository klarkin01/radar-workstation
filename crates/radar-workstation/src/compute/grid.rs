//! Gridding: turns one closed `Sweep` into a dense polar `SweepGrid` per
//! product (S3-W1). No interpolation, no gap filling, no nearest-radial
//! search — a polar grid has a slot for every radial the antenna produced,
//! so scattering each radial's gates into its own row is exact. The
//! render-time interpolation question (what a shader does between rows) is
//! Stage 4's, not this module's.
//!
//! **AZ convention (§4.1, measured 2026-08-05):** `Radial::azimuth_deg` is
//! the radial's bin **centre**, not its leading edge. Measured directly
//! against a full real KDOX VCP 35 volume
//! (`downloads/KDOX_20260629_1811/`, not committed — see
//! `docs/architecture/nexrad-binary-format.md` §6.1 for the recorded
//! finding): for every radial on elevation 1 (0.5° spacing),
//! `(azimuth_deg / 0.5) mod 1 ≈ 0.5`, i.e. every measured angle sits almost
//! exactly half a bin above a multiple of the spacing (229.24896°,
//! 229.75159°, 230.24323°, ... — never 229.0, 229.5, 230.0). A leading-edge
//! convention would put every measured angle at a whole multiple of the
//! spacing instead. [`azimuth_slot`] bins accordingly: `floor(az /
//! spacing)`, not `round(az / spacing)` — the latter would silently rotate
//! every image drawn from this grid by a quarter of a bin.

use std::collections::HashMap;
use std::sync::Arc;

use nexrad_decoder::{ProductKind, Sweep};

use crate::compute::DisplayProduct;
use crate::event::Event;

/// One product, on one sweep, as a dense regular polar grid ready for
/// upload as an R8 texture (ADR-0020).
///
/// Cell encoding, preserved from the ICD (see `nexrad_decoder::ProductData`):
///   - `0`       — below threshold / no data / azimuth slot never filled
///   - `1`       — range folded
///   - `2..=255` — data; `physical = (cell as f32 - offset) / scale`
///
/// Row-major, azimuth-major: cell `(azimuth, gate)` is at
/// `cells[azimuth * gate_count + gate]`.
#[derive(Debug, Clone)]
pub struct SweepGrid {
    pub product: DisplayProduct,

    // --- geometry (Q17: native, per sweep; never padded, never upsampled) ---
    /// 720 for super-resolution, 360 for standard. Slot `a` covers azimuths
    /// `[a * spacing, (a + 1) * spacing)`, where `spacing = 360.0 /
    /// azimuth_count` — see this module's top-level doc comment for why the
    /// slot's *centre*, not its start, is what `azimuth_deg` reports.
    pub azimuth_count: u16,
    pub gate_count: u16,
    pub first_gate_m: u16,
    pub gate_width_m: u16,

    // --- provenance / display metadata ---
    pub elevation_number: u8,
    pub elevation_deg: f32,
    /// Q9: carried so Stage 4 can state the fold limit rather than
    /// silently displaying aliased data.
    pub nyquist_velocity_mps: Option<f32>,

    // --- values ---
    /// Effective scale/offset *after* any requantisation (§4.4) — the
    /// physical-value formula above always holds against these, whatever
    /// the source moment's own ICD scale/offset was.
    pub scale: f32,
    pub offset: f32,
    pub cells: Vec<u8>,

    /// How many azimuth slots received at least one radial. Diagnostic:
    /// feeds `headless`'s output now and a data-completeness indicator
    /// later.
    pub filled_azimuths: u16,
}

impl SweepGrid {
    pub fn cell(&self, azimuth: u16, gate: u16) -> u8 {
        self.cells[azimuth as usize * self.gate_count as usize + gate as usize]
    }

    /// `None` for cells 0 (no-data) and 1 (range-folded) — same contract as
    /// `ProductData::physical_value`.
    pub fn physical(&self, azimuth: u16, gate: u16) -> Option<f32> {
        let raw = self.cell(azimuth, gate);
        if raw < 2 {
            return None;
        }
        Some((raw as f32 - self.offset) / self.scale)
    }

    pub fn byte_len(&self) -> usize {
        self.cells.len()
    }
}

/// ZDR's display range (§4.4): 16 dB over 253 usable levels (cells 2..=255)
/// is 0.063 dB/step, against 0.031 dB native resolution — below what any
/// display or operator resolves. An 8-bit ZDR grid at the native ICD
/// scale/offset (32.0 / 418.0) would put every value below -5 dB, which is
/// how the 16-bit word size is detectable in the first place.
const ZDR_DISPLAY_MIN_DB: f32 = -8.0;
const ZDR_DISPLAY_MAX_DB: f32 = 8.0;

fn zdr_effective_scale_offset() -> (f32, f32) {
    let scale = 253.0 / (ZDR_DISPLAY_MAX_DB - ZDR_DISPLAY_MIN_DB);
    let offset = 2.0 - ZDR_DISPLAY_MIN_DB * scale;
    (scale, offset)
}

/// `physical == (cell - offset) / scale` exactly (up to one quantisation
/// step) for the `(scale, offset)` pair `zdr_effective_scale_offset`
/// returns — so the LUT compiler and a future cursor readout need no
/// special case for a requantised product.
fn quantise_zdr(physical: f32, scale: f32) -> u8 {
    let cell = (2.0 + (physical - ZDR_DISPLAY_MIN_DB) * scale).round();
    cell.clamp(2.0, 255.0) as u8
}

/// The azimuth count (720 super-res / 360 standard-res) from the *modal*
/// `azimuth_spacing_code` across the sweep's radials, not the first
/// radial's — one corrupt radial must not resize the grid. A modal code
/// that is neither 1 nor 2 falls back to inferring from radial count.
fn modal_azimuth_count(sweep: &Sweep) -> (u16, Option<Event>) {
    let mut counts: HashMap<u8, usize> = HashMap::new();
    for radial in &sweep.radials {
        *counts.entry(radial.azimuth_spacing_code).or_insert(0) += 1;
    }
    let modal_code = counts.into_iter().max_by_key(|&(_, count)| count).map(|(code, _)| code);
    match modal_code {
        Some(1) => (720, None),
        Some(2) => (360, None),
        _ => {
            let inferred = if sweep.radials.len() >= 540 { 720 } else { 360 };
            let event =
                Event::UnknownAzimuthSpacingCode { elevation_number: sweep.elevation_number, inferred_azimuth_count: inferred };
            (inferred, Some(event))
        }
    }
}

/// Which azimuth row `azimuth_deg` scatters into, given this module's
/// centre convention (top-level doc comment). `pub`, not `pub(crate)`:
/// `compute::derived` reuses it to reindex an azimuth slot between two
/// grids of different `azimuth_count` (a lower super-res tilt and a higher
/// standard-res one, say), and `utility/radar-viz`'s grid render path
/// reuses it to map a screen pixel's azimuth to a grid slot — the same
/// binning rule applies in both cases, and a validation tool re-deriving it
/// independently would be exactly the kind of drift it exists to catch.
pub fn azimuth_slot(azimuth_deg: f32, azimuth_count: u16) -> u16 {
    let spacing_deg = 360.0 / azimuth_count as f32;
    let idx = (azimuth_deg / spacing_deg).floor() as i64;
    idx.rem_euclid(azimuth_count as i64) as u16
}

/// Gate geometry shared by every radial carrying `kind`, taken from the
/// first radial that has it. `None` if no radial in the sweep carries this
/// moment at all — a split cut legitimately lacks velocity on the
/// surveillance half, and that is not an error.
struct SourceGeometry {
    gate_count: u16,
    first_gate_m: u16,
    gate_width_m: u16,
    word_size: u8,
    scale: f32,
    offset: f32,
}

fn source_geometry(sweep: &Sweep, kind: ProductKind) -> Option<SourceGeometry> {
    let moment = sweep.radials.iter().find_map(|r| r.products.get(&kind))?;
    Some(SourceGeometry {
        gate_count: moment.gate_count,
        first_gate_m: moment.first_gate_m,
        gate_width_m: moment.gate_width_m,
        word_size: moment.word_size,
        scale: moment.scale,
        offset: moment.offset,
    })
}

/// Grid one product from one closed sweep. `None` when the sweep carries no
/// radial with this moment at all, or when the sweep's chosen geometry is
/// degenerate (an event is reported for the latter, not the former — a
/// missing moment on a legitimate split cut is not a fault).
pub fn grid_sweep(sweep: &Sweep, product: DisplayProduct) -> Option<(SweepGrid, Vec<Event>)> {
    let kind = product.source_moment()?;
    let mut events = Vec::new();

    let (azimuth_count, spacing_event) = modal_azimuth_count(sweep);
    events.extend(spacing_event);
    if azimuth_count == 0 {
        return None;
    }

    let geometry = source_geometry(sweep, kind)?;
    if geometry.gate_width_m == 0 || geometry.gate_count == 0 {
        events.push(Event::DegenerateGateGeometry { product, elevation_number: sweep.elevation_number });
        return None;
    }

    let gate_count = geometry.gate_count;
    let is_zdr16 = geometry.word_size == 16;
    let (eff_scale, eff_offset) =
        if is_zdr16 { zdr_effective_scale_offset() } else { (geometry.scale, geometry.offset) };

    let mut cells = vec![0u8; azimuth_count as usize * gate_count as usize];
    let mut filled = vec![false; azimuth_count as usize];
    let mut inconsistent_skipped = 0usize;
    let mut duplicate_slots = 0usize;

    for radial in &sweep.radials {
        let Some(moment) = radial.products.get(&kind) else { continue };
        if moment.gate_count != geometry.gate_count
            || moment.first_gate_m != geometry.first_gate_m
            || moment.gate_width_m != geometry.gate_width_m
            || moment.word_size != geometry.word_size
        {
            inconsistent_skipped += 1;
            continue;
        }

        let slot = azimuth_slot(radial.azimuth_deg, azimuth_count);
        if filled[slot as usize] {
            duplicate_slots += 1;
        }
        filled[slot as usize] = true;

        let row = slot as usize * gate_count as usize;
        for gate in 0..gate_count as usize {
            let Some(raw) = moment.raw_gate(gate) else { continue };
            cells[row + gate] = match (is_zdr16, raw) {
                (false, raw) => raw as u8,
                (true, raw) if raw < 2 => raw as u8,
                (true, raw) => {
                    let physical = (raw as f32 - geometry.offset) / geometry.scale;
                    quantise_zdr(physical, eff_scale)
                }
            };
        }
    }

    if inconsistent_skipped > 0 {
        events.push(Event::InconsistentGateGeometry {
            product,
            elevation_number: sweep.elevation_number,
            skipped: inconsistent_skipped,
        });
    }
    if duplicate_slots > 0 {
        events.push(Event::DuplicateAzimuthSlot {
            product,
            elevation_number: sweep.elevation_number,
            count: duplicate_slots,
        });
    }

    let filled_azimuths = filled.iter().filter(|&&f| f).count() as u16;

    Some((
        SweepGrid {
            product,
            azimuth_count,
            gate_count,
            first_gate_m: geometry.first_gate_m,
            gate_width_m: geometry.gate_width_m,
            elevation_number: sweep.elevation_number,
            elevation_deg: sweep.elevation_deg,
            nyquist_velocity_mps: sweep.nyquist_velocity_mps,
            scale: eff_scale,
            offset: eff_offset,
            cells,
            filled_azimuths,
        },
        events,
    ))
}

/// Grid every `DisplayProduct::BASE` product present on `sweep`. Run on
/// `spawn_blocking` by `compute::compute_loop` — this is the whole of one
/// blocking-thread job.
pub fn grid_all_base_products(sweep: &Sweep) -> (Vec<Arc<SweepGrid>>, Vec<Event>) {
    let mut grids = Vec::new();
    let mut events = Vec::new();
    for (product, _) in DisplayProduct::BASE {
        if let Some((grid, grid_events)) = grid_sweep(sweep, product) {
            grids.push(Arc::new(grid));
            events.extend(grid_events);
        }
    }
    (grids, events)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::test_support::{radial, radial16, sweep};

    #[test]
    fn kdox_vcp35_reflectivity_grid_matches_measured_geometry() {
        // Geometry from CLAUDE.md's confirmed KDOX VCP 35 fixture table:
        // 1832 gates, 2.125 km first gate, 0.25 km gate width, super-res.
        let mut radials = Vec::new();
        for n in 0..720u16 {
            let az = (n as f32 + 0.5) * 0.5;
            radials.push(radial(1, az, 1, &[(ProductKind::Ref, 1832, 2125, 250, 8, 2.0, 66.0)]));
        }
        let s = sweep(1, 0.39, radials);

        let (grid, events) = grid_sweep(&s, DisplayProduct::Reflectivity).expect("must grid");
        assert!(events.is_empty(), "a clean full sweep should report nothing: {events:?}");
        assert_eq!(grid.azimuth_count, 720);
        assert_eq!(grid.gate_count, 1832);
        assert_eq!(grid.first_gate_m, 2125);
        assert_eq!(grid.gate_width_m, 250);
        assert_eq!(grid.filled_azimuths, 720);
    }

    #[test]
    fn ktlh_vcp212_standard_resolution_grids_to_360_azimuths() {
        let mut radials = Vec::new();
        for n in 0..360u16 {
            let az = (n as f32 + 0.5) * 1.0;
            radials.push(radial(1, az, 2, &[(ProductKind::Ref, 1192, 2125, 250, 8, 2.0, 66.0)]));
        }
        let s = sweep(1, 0.5, radials);

        let (grid, _events) = grid_sweep(&s, DisplayProduct::Reflectivity).expect("must grid");
        assert_eq!(grid.azimuth_count, 360);
        assert_eq!(grid.filled_azimuths, 360);
    }

    #[test]
    fn absent_radials_leave_no_data_cells() {
        // Only two of 720 azimuth slots are ever written.
        let radials = vec![
            radial(1, 0.25, 1, &[(ProductKind::Ref, 4, 0, 250, 8, 2.0, 66.0)]),
            radial(1, 180.25, 1, &[(ProductKind::Ref, 4, 0, 250, 8, 2.0, 66.0)]),
        ];
        let s = sweep(1, 0.5, radials);

        let (grid, _events) = grid_sweep(&s, DisplayProduct::Reflectivity).expect("must grid");
        assert_eq!(grid.filled_azimuths, 2);
        // A slot nowhere near either written radial must be all zero.
        for gate in 0..grid.gate_count {
            assert_eq!(grid.cell(90, gate), 0, "unfilled slot must be no-data, not interpolated");
        }
    }

    #[test]
    fn no_echo_and_range_fold_are_distinct_cells() {
        // synthetic_raw always puts a no-data code at gate 0 and a
        // range-fold code at gate 1 — see test_support's doc comment.
        let radials = vec![radial(1, 0.25, 1, &[(ProductKind::Ref, 4, 0, 250, 8, 2.0, 66.0)])];
        let s = sweep(1, 0.5, radials);

        let (grid, _events) = grid_sweep(&s, DisplayProduct::Reflectivity).expect("must grid");
        assert_eq!(grid.cell(0, 0), 0);
        assert_eq!(grid.cell(0, 1), 1);
        assert!(grid.physical(0, 0).is_none());
        assert!(grid.physical(0, 1).is_none());
    }

    #[test]
    fn physical_value_round_trips_through_the_grid() {
        // synthetic_raw(133) == 133 -> (133 - 66) / 2.0 = 33.5 dBZ.
        let radials = vec![radial(1, 0.25, 1, &[(ProductKind::Ref, 200, 0, 250, 8, 2.0, 66.0)])];
        let s = sweep(1, 0.5, radials);

        let (grid, _events) = grid_sweep(&s, DisplayProduct::Reflectivity).expect("must grid");
        let v = grid.physical(0, 133).expect("raw=133 must be Some");
        assert!((v - 33.5).abs() < 1e-5, "expected 33.5 dBZ, got {v}");
    }

    #[test]
    fn split_cut_without_velocity_returns_none() {
        let radials = vec![radial(1, 0.25, 1, &[(ProductKind::Ref, 4, 0, 250, 8, 2.0, 66.0)])];
        let s = sweep(1, 0.5, radials);
        assert!(grid_sweep(&s, DisplayProduct::Velocity).is_none());
    }

    #[test]
    fn inconsistent_gate_geometry_is_skipped_not_copied() {
        let mut radials = vec![radial(1, 0.25, 1, &[(ProductKind::Ref, 4, 0, 250, 8, 2.0, 66.0)])];
        // Second radial claims a different gate_count for the same moment.
        radials.push(radial(1, 90.25, 1, &[(ProductKind::Ref, 8, 0, 250, 8, 2.0, 66.0)]));
        let s = sweep(1, 0.5, radials);

        let (grid, events) = grid_sweep(&s, DisplayProduct::Reflectivity).expect("must grid");
        assert_eq!(grid.gate_count, 4, "geometry is fixed by the first radial carrying the moment");
        assert_eq!(grid.filled_azimuths, 1, "the mismatched radial must not be scattered in");
        assert!(events.iter().any(|e| matches!(e, Event::InconsistentGateGeometry { skipped: 1, .. })));
    }

    #[test]
    fn zero_gate_width_is_rejected() {
        let radials = vec![radial(1, 0.25, 1, &[(ProductKind::Ref, 4, 0, 0, 8, 2.0, 66.0)])];
        let s = sweep(1, 0.5, radials);
        assert!(grid_sweep(&s, DisplayProduct::Reflectivity).is_none());
    }

    #[test]
    fn empty_sweep_returns_none() {
        let s = sweep(1, 0.5, Vec::new());
        assert!(grid_sweep(&s, DisplayProduct::Reflectivity).is_none());
    }

    #[test]
    fn zdr_requantisation_round_trips_within_one_step() {
        // Native ZDR calibration: scale=32.0, offset=418.0 (CLAUDE.md).
        // raw=450 -> (450-418)/32 = 1.0 dB.
        let radials = vec![radial16(1, 0.25, 1, ProductKind::Zdr, 4, 0, 250, 32.0, 418.0, &[0, 1, 450, 2])];
        let s = sweep(1, 0.5, radials);

        let (grid, _events) = grid_sweep(&s, DisplayProduct::Zdr).expect("must grid");
        let physical = grid.physical(0, 2).expect("raw 450 must be Some");
        assert!((physical - 1.0).abs() < 0.063, "expected ~1.0 dB within one step, got {physical}");
    }

    #[test]
    fn zdr_values_outside_the_display_range_clamp_rather_than_wrap() {
        assert_eq!(quantise_zdr(100.0, zdr_effective_scale_offset().0), 255);
        assert_eq!(quantise_zdr(-100.0, zdr_effective_scale_offset().0), 2);
    }

    #[test]
    fn eight_bit_moments_are_copied_verbatim() {
        let radials = vec![radial(1, 0.25, 1, &[(ProductKind::Vel, 4, 0, 250, 8, 2.0, 129.0)])];
        let s = sweep(1, 0.5, radials);
        let (grid, _events) = grid_sweep(&s, DisplayProduct::Velocity).expect("must grid");
        assert!((grid.scale - 2.0).abs() < 1e-6);
        assert!((grid.offset - 129.0).abs() < 1e-6);
    }
}
