//! Volume-derived products (S3-W4 §7.3/§7.4): Echo Tops and VIL, computed
//! once a volume closes `Complete` from the reflectivity grids the compute
//! task retained across it. Both adopt **the lowest-elevation reflectivity
//! grid's geometry** (§7.2) — data-driven, no invented maximum range, and
//! it puts the derived products on exactly the grid the operator is
//! already looking at. The gate axis is reinterpreted as **ground** range
//! rather than slant range — the one place in this codebase where the two
//! differ materially.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::compute::{geometry, grid::SweepGrid, DisplayProduct};
use crate::event::Event;

pub const ECHO_TOP_THRESHOLD_DBZ: f32 = 18.0;
const ECHO_TOP_MAX_KFT: f32 = 70.0;
const METERS_PER_FOOT: f32 = 0.3048;

pub const VIL_HAIL_CAP_DBZ: f32 = 56.0;
const VIL_MAX_KG_M2: f32 = 80.0;

struct OutputGeometry {
    azimuth_count: u16,
    gate_count: u16,
    first_gate_m: u16,
    gate_width_m: u16,
}

impl OutputGeometry {
    fn from_grid(grid: &SweepGrid) -> Self {
        Self {
            azimuth_count: grid.azimuth_count,
            gate_count: grid.gate_count,
            first_gate_m: grid.first_gate_m,
            gate_width_m: grid.gate_width_m,
        }
    }
}

/// One reflectivity grid per distinct elevation angle, ascending, newest
/// wins where a VCP repeats an angle (SAILS/MRLE insert repeated low-level
/// cuts with distinct elevation *numbers* at the same angle — see
/// `RadialStatus::StartOfLastElevation`'s doc comment). Angles are bucketed
/// to the nearest 0.01° rather than compared for exact float equality —
/// real repeated cuts measure stable well under that tolerance.
fn distinct_tilts_ascending(ref_grids: &[Arc<SweepGrid>]) -> Vec<Arc<SweepGrid>> {
    let mut by_angle: BTreeMap<i32, Arc<SweepGrid>> = BTreeMap::new();
    for grid in ref_grids {
        let bucket = (grid.elevation_deg * 100.0).round() as i32;
        // `ref_grids` is in sweep-closure order; a later insert for the
        // same bucket is the newer closure and is kept.
        by_angle.insert(bucket, Arc::clone(grid));
    }
    by_angle.into_values().collect()
}

fn azimuth_deg_center(a: u16, azimuth_count: u16) -> f32 {
    (a as f32 + 0.5) * (360.0 / azimuth_count as f32)
}

/// Re-expresses azimuth slot `a`, indexed against a grid with `from_count`
/// azimuths, as the corresponding slot in a grid with `to_count` azimuths
/// — needed because a VCP can mix super- and standard-resolution tilts, so
/// the output grid's azimuth count does not always match every input
/// tilt's. Identity when the counts already match (the common case).
fn reindex_azimuth(a: u16, from_count: u16, to_count: u16) -> u16 {
    if from_count == to_count {
        return a;
    }
    super::grid::azimuth_slot(azimuth_deg_center(a, from_count), to_count)
}

fn dbz_to_z(dbz: f32) -> f64 {
    10f64.powf(dbz as f64 / 10.0)
}

/// One layer's contribution to the VIL sum: `3.44e-6 · ((z0+z1)/2)^(4/7) · dh`.
fn vil_layer_contribution(z0: f64, z1: f64, dh_m: f64) -> f64 {
    3.44e-6 * ((z0 + z1) / 2.0).powf(4.0 / 7.0) * dh_m
}

/// Quantises a physical value over `0..=max` into cells `2..=255` — the
/// same shape as ZDR's requantisation (`grid::quantise_zdr`), specialised
/// to a `[0, max]` display range instead of a signed one.
fn quantise(physical: f32, max: f32) -> u8 {
    let k = 253.0 / max;
    (2.0 + physical.clamp(0.0, max) * k).round().clamp(2.0, 255.0) as u8
}

/// The gate index within `tilt`'s own grid for a target at slant range
/// `slant_m`, or `None` if it falls outside the tilt's measured range —
/// "beyond coverage" (nothing was ever measured there), which must stay
/// distinct from "measured and found below threshold".
fn gate_index(slant_m: f64, tilt: &SweepGrid) -> Option<u16> {
    if tilt.gate_width_m == 0 {
        return None;
    }
    let idx = (slant_m - tilt.first_gate_m as f64) / tilt.gate_width_m as f64;
    if !idx.is_finite() || idx < 0.0 || idx >= tilt.gate_count as f64 {
        return None;
    }
    Some(idx as u16)
}

/// Compute Echo Tops and VIL from the accumulating volume's retained
/// reflectivity grids. `ref_grids` need not be pre-sorted or deduplicated
/// by angle — this function does both via `distinct_tilts_ascending`.
/// Returns no grids (not an error, and no events) if there are no
/// reflectivity tilts at all.
pub fn compute_derived(ref_grids: &[Arc<SweepGrid>]) -> (Vec<Arc<SweepGrid>>, Vec<Event>) {
    let events = Vec::new();
    let tilts = distinct_tilts_ascending(ref_grids);
    let Some(lowest) = tilts.first() else {
        return (Vec::new(), events);
    };
    let out = OutputGeometry::from_grid(lowest);

    let mut grids = Vec::new();
    if let Some(g) = echo_tops(&tilts, &out) {
        grids.push(Arc::new(g));
    }
    if let Some(g) = vil(&tilts, &out) {
        grids.push(Arc::new(g));
    }
    (grids, events)
}

/// For each output cell, walk tilts from highest to lowest; the first whose
/// reflectivity at that column meets [`ECHO_TOP_THRESHOLD_DBZ`] gives the
/// echo top, reported as the **beam centre** of the highest qualifying
/// tilt — not interpolated toward the beam top (GR2Analyst does that; the
/// two differ by up to a beam width at long range). No qualifying tilt (or
/// no tilt covers that column at all) leaves the cell at no-data.
fn echo_tops(tilts: &[Arc<SweepGrid>], out: &OutputGeometry) -> Option<SweepGrid> {
    if tilts.is_empty() {
        return None;
    }
    let mut cells = vec![0u8; out.azimuth_count as usize * out.gate_count as usize];
    let mut filled = vec![false; out.azimuth_count as usize];

    for a in 0..out.azimuth_count {
        for g in 0..out.gate_count {
            let ground_m = out.first_gate_m as f64 + g as f64 * out.gate_width_m as f64;
            let mut best_height_m: Option<f64> = None;

            for tilt in tilts.iter().rev() {
                let (slant_m, height_m) = geometry::slant_range_and_height(ground_m, tilt.elevation_deg as f64);
                let Some(gate) = gate_index(slant_m, tilt) else { continue };
                let az = reindex_azimuth(a, out.azimuth_count, tilt.azimuth_count);
                if tilt.physical(az, gate).is_some_and(|dbz| dbz >= ECHO_TOP_THRESHOLD_DBZ) {
                    best_height_m = Some(height_m);
                    break;
                }
            }

            if let Some(height_m) = best_height_m {
                let kft = (height_m as f32 / METERS_PER_FOOT) / 1000.0;
                cells[a as usize * out.gate_count as usize + g as usize] = quantise(kft, ECHO_TOP_MAX_KFT);
                filled[a as usize] = true;
            }
        }
    }

    Some(SweepGrid {
        product: DisplayProduct::EchoTops,
        azimuth_count: out.azimuth_count,
        gate_count: out.gate_count,
        first_gate_m: out.first_gate_m,
        gate_width_m: out.gate_width_m,
        elevation_number: 0,
        elevation_deg: 0.0,
        nyquist_velocity_mps: None,
        scale: 253.0 / ECHO_TOP_MAX_KFT,
        offset: 2.0,
        cells,
        filled_azimuths: filled.iter().filter(|&&f| f).count() as u16,
    })
}

/// `VIL = Σ over adjacent tilt pairs: 3.44e-6 · ((Zᵢ + Zᵢ₊₁)/2)^(4/7) ·
/// (hᵢ₊₁ − hᵢ)`, reflectivity capped at [`VIL_HAIL_CAP_DBZ`] to bound hail
/// contamination. A column with fewer than two tilts carrying data at that
/// column is no-data (cell 0) — a single-layer "integral" is not one; a
/// column with two or more tilts but weak-or-absent echo integrates to a
/// real zero (cell 2), which must stay distinct from no-data.
fn vil(tilts: &[Arc<SweepGrid>], out: &OutputGeometry) -> Option<SweepGrid> {
    if tilts.is_empty() {
        return None;
    }
    let mut cells = vec![0u8; out.azimuth_count as usize * out.gate_count as usize];
    let mut filled = vec![false; out.azimuth_count as usize];

    for a in 0..out.azimuth_count {
        for g in 0..out.gate_count {
            let ground_m = out.first_gate_m as f64 + g as f64 * out.gate_width_m as f64;

            // Ascending in height because `tilts` is ascending in angle and
            // height increases with elevation angle at a fixed ground range.
            let mut points: Vec<(f64, f32)> = Vec::new();
            for tilt in tilts {
                let (slant_m, height_m) = geometry::slant_range_and_height(ground_m, tilt.elevation_deg as f64);
                let Some(gate) = gate_index(slant_m, tilt) else { continue };
                let az = reindex_azimuth(a, out.azimuth_count, tilt.azimuth_count);
                if let Some(dbz) = tilt.physical(az, gate) {
                    points.push((height_m, dbz.min(VIL_HAIL_CAP_DBZ)));
                }
            }
            if points.len() < 2 {
                continue;
            }

            let kg_m2: f64 = points
                .windows(2)
                .map(|w| vil_layer_contribution(dbz_to_z(w[0].1), dbz_to_z(w[1].1), w[1].0 - w[0].0))
                .sum();

            cells[a as usize * out.gate_count as usize + g as usize] = quantise(kg_m2 as f32, VIL_MAX_KG_M2);
            filled[a as usize] = true;
        }
    }

    Some(SweepGrid {
        product: DisplayProduct::Vil,
        azimuth_count: out.azimuth_count,
        gate_count: out.gate_count,
        first_gate_m: out.first_gate_m,
        gate_width_m: out.gate_width_m,
        elevation_number: 0,
        elevation_deg: 0.0,
        nyquist_velocity_mps: None,
        scale: 253.0 / VIL_MAX_KG_M2,
        offset: 2.0,
        cells,
        filled_azimuths: filled.iter().filter(|&&f| f).count() as u16,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A single-azimuth, generous-range reflectivity tilt with one echo
    /// placed at ground range `ground_m` (or no echo at all, if `dbz` is
    /// `None`). `scale=1.0, offset=0.0` — the raw byte *is* the dBZ value —
    /// keeps the test's expected numbers simple.
    fn tilt(elevation_deg: f32, ground_m: f64, dbz: Option<f32>) -> Arc<SweepGrid> {
        let (slant_m, _height_m) = geometry::slant_range_and_height(ground_m, elevation_deg as f64);
        let gate_width_m = 100u16;
        let idx = (slant_m / gate_width_m as f64).floor() as usize;
        let gate_count = (idx + 10) as u16;
        let mut cells = vec![0u8; gate_count as usize];
        if let Some(dbz) = dbz {
            cells[idx] = dbz as u8;
        }
        Arc::new(SweepGrid {
            product: DisplayProduct::Reflectivity,
            azimuth_count: 1,
            gate_count,
            first_gate_m: 0,
            gate_width_m,
            elevation_number: 0,
            elevation_deg,
            nyquist_velocity_mps: None,
            scale: 1.0,
            offset: 0.0,
            cells,
            filled_azimuths: 1,
        })
    }

    fn out_geometry(ground_m: u16) -> OutputGeometry {
        OutputGeometry { azimuth_count: 1, gate_count: 1, first_gate_m: ground_m, gate_width_m: 1 }
    }

    #[test]
    fn echo_top_uses_lowest_tilt_when_only_it_has_qualifying_echo() {
        let ground_m = 2000.0;
        let low = tilt(0.5, ground_m, Some(30.0));
        let high = tilt(5.0, ground_m, None);
        let out = out_geometry(2000);

        let grid = echo_tops(&[low, high], &out).expect("must produce a grid");
        let (_, h) = geometry::slant_range_and_height(ground_m, 0.5);
        let expected_kft = (h as f32 / METERS_PER_FOOT) / 1000.0;
        let got = grid.physical(0, 0).expect("must be filled");
        assert!((got - expected_kft).abs() < 0.2, "expected {expected_kft} kft, got {got}");
    }

    #[test]
    fn echo_top_uses_the_highest_qualifying_tilt_when_multiple_have_echo() {
        let ground_m = 2000.0;
        let low = tilt(0.5, ground_m, Some(30.0));
        let high = tilt(5.0, ground_m, Some(25.0));
        let out = out_geometry(2000);

        let grid = echo_tops(&[low, high], &out).unwrap();
        let (_, h) = geometry::slant_range_and_height(ground_m, 5.0);
        let expected_kft = (h as f32 / METERS_PER_FOOT) / 1000.0;
        let got = grid.physical(0, 0).unwrap();
        assert!((got - expected_kft).abs() < 0.2, "expected {expected_kft} kft, got {got}");
    }

    #[test]
    fn column_below_threshold_everywhere_is_no_data() {
        let ground_m = 2000.0;
        let low = tilt(0.5, ground_m, Some(5.0)); // below the 18 dBZ threshold
        let out = out_geometry(2000);

        let grid = echo_tops(&[low], &out).unwrap();
        assert!(grid.physical(0, 0).is_none());
        assert_eq!(grid.cell(0, 0), 0);
    }

    #[test]
    fn beyond_coverage_is_no_data_not_zero_tops() {
        let low = tilt(0.5, 2000.0, Some(30.0)); // covers only a few km
        let out = out_geometry(60_000); // 60 km — far beyond that tilt's coverage
        let grid = echo_tops(&[low], &out).unwrap();
        assert!(grid.physical(0, 0).is_none(), "beyond every tilt's coverage must be no-data, not zero");
    }

    #[test]
    fn vil_matches_a_hand_computed_uniform_column() {
        // A uniform 50 dBZ layer, 1000 m deep: Z = 10^(50/10) = 100000;
        // contribution = 3.44e-6 * 100000^(4/7) * 1000.
        let z = dbz_to_z(50.0);
        let got = vil_layer_contribution(z, z, 1000.0);
        let expected = 3.44e-6 * 100_000f64.powf(4.0 / 7.0) * 1000.0;
        assert!((got - expected).abs() < 1e-9, "got {got}, expected {expected}");
    }

    #[test]
    fn vil_caps_reflectivity_at_the_hail_threshold() {
        let ground_m = 2000.0;
        let t1 = tilt(0.5, ground_m, Some(70.0)); // above the cap
        let t2 = tilt(5.0, ground_m, Some(70.0));
        let out = out_geometry(2000);

        let grid = vil(&[t1, t2], &out).unwrap();
        let got = grid.physical(0, 0).unwrap();

        let (_, h1) = geometry::slant_range_and_height(ground_m, 0.5);
        let (_, h2) = geometry::slant_range_and_height(ground_m, 5.0);
        let capped_z = dbz_to_z(VIL_HAIL_CAP_DBZ);
        let expected = vil_layer_contribution(capped_z, capped_z, h2 - h1) as f32;
        assert!((got - expected).abs() < 1.0, "expected ~{expected}, got {got} — cap must have applied");
    }

    #[test]
    fn single_tilt_column_is_no_data() {
        let ground_m = 2000.0;
        let t1 = tilt(0.5, ground_m, Some(30.0));
        let out = out_geometry(2000);

        let grid = vil(&[t1], &out).unwrap();
        assert!(grid.physical(0, 0).is_none());
        assert_eq!(grid.cell(0, 0), 0);
    }

    #[test]
    fn vil_is_zero_not_no_data_for_a_column_with_only_weak_echo() {
        let ground_m = 2000.0;
        let t1 = tilt(0.5, ground_m, Some(2.0));
        let t2 = tilt(5.0, ground_m, Some(2.0));
        let out = out_geometry(2000);

        let grid = vil(&[t1, t2], &out).unwrap();
        assert!(grid.physical(0, 0).is_some(), "two tilts with weak-but-present echo must be a real value");
    }

    #[test]
    fn repeated_sails_cuts_contribute_one_tilt_not_two() {
        let first = tilt(0.5, 2000.0, Some(20.0));
        let mut first_closure = (*first).clone();
        first_closure.elevation_number = 1;
        let mut sails_repeat = (*first).clone();
        sails_repeat.elevation_number = 9; // same angle, new elevation_number, closed later

        let grids = vec![Arc::new(first_closure), Arc::new(sails_repeat)];
        let distinct = distinct_tilts_ascending(&grids);
        assert_eq!(distinct.len(), 1, "two grids at the same angle must collapse to one tilt");
        assert_eq!(distinct[0].elevation_number, 9, "the later closure must win");
    }
}
