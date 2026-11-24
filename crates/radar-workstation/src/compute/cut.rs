//! Tilt identity and the one grid-selection rule (ADR-0031 D-2, D-3).
//!
//! **Identity.** A tilt is its *commanded* cut angle (`SweepGrid::cut_deg`,
//! from `assembly::cut_angle`), not its `elevation_number` — which names
//! different angles in different VCPs and splits one split cut into two
//! numbers — and not its measured angle, which drifts up to ~0.08° and sits
//! 0.1° from a neighbouring pattern's cut (VCP 31's 2.5° vs VCP 35's 2.4°).
//! [`CutKey`] buckets the commanded angle to 0.01°.
//!
//! **Selection (D-3).** Among the grids at one `CutKey` that carry a
//! product, take the greatest `(range_extent_m, volume, elevation_number)`:
//! the full-range surveillance REF over a split cut's range-limited Doppler
//! REF, the newest SAILS/MRLE re-closure among equals, and the only VEL.
//! Defined once, here — `compute::derived`'s tilt dedupe, the live view,
//! every retained frame and GPU residency all call [`best_at_cut`].

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::assembly::VolumeId;
use crate::compute::{DisplayProduct, SweepGrid};

/// A commanded cut angle in hundredths of a degree (ADR-0031 D-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CutKey(pub i32);

impl CutKey {
    pub fn degrees(self) -> f32 {
        self.0 as f32 / 100.0
    }
}

/// The 0.01° bucket an angle falls in.
pub fn cut_key(deg: f32) -> CutKey {
    CutKey((deg * 100.0).round() as i32)
}

/// The slant range a grid's last gate ends at, in metres.
pub fn range_extent_m(grid: &SweepGrid) -> u32 {
    grid.first_gate_m as u32 + grid.gate_count as u32 * grid.gate_width_m as u32
}

/// D-3: the grid for `product` at `cut` among `candidates`, each tagged with
/// the volume it came from, or `None` if no candidate carries `product` at
/// that cut. Nothing is ever substituted from another cut.
pub fn best_at_cut<'a>(
    candidates: impl IntoIterator<Item = (VolumeId, &'a Arc<SweepGrid>)>,
    product: DisplayProduct,
    cut: CutKey,
) -> Option<(VolumeId, &'a Arc<SweepGrid>)> {
    candidates
        .into_iter()
        .filter(|(_, grid)| grid.product == product && cut_key(grid.cut_deg) == cut)
        .max_by_key(|(volume, grid)| (range_extent_m(grid), *volume, grid.elevation_number))
}

/// The distinct cuts among `grids`, ascending.
pub fn distinct_cuts<'a>(grids: impl IntoIterator<Item = &'a Arc<SweepGrid>>) -> Vec<CutKey> {
    grids.into_iter().map(|g| cut_key(g.cut_deg)).collect::<BTreeSet<_>>().into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(scan_time_ms: u32) -> VolumeId {
        VolumeId { julian_date: 20_000, scan_time_ms }
    }

    fn grid(product: DisplayProduct, elevation_number: u8, cut_deg: f32, gate_count: u16) -> Arc<SweepGrid> {
        Arc::new(SweepGrid {
            product,
            azimuth_count: 1,
            gate_count,
            first_gate_m: 2125,
            gate_width_m: 250,
            elevation_number,
            elevation_deg: cut_deg + 0.03,
            cut_deg,
            nyquist_velocity_mps: None,
            scale: 2.0,
            offset: 66.0,
            cells: vec![0u8; gate_count as usize],
            filled_azimuths: 1,
        })
    }

    #[test]
    fn a_split_cut_selects_the_surveillance_ref_and_the_doppler_vel() {
        let v = volume(100);
        let surveillance_ref = grid(DisplayProduct::Reflectivity, 1, 0.48, 1832);
        let doppler_ref = grid(DisplayProduct::Reflectivity, 2, 0.48, 1192);
        let doppler_vel = grid(DisplayProduct::Velocity, 2, 0.48, 1192);
        let all = [&surveillance_ref, &doppler_ref, &doppler_vel];
        let cut = cut_key(0.48);

        let (_, r) = best_at_cut(all.iter().map(|g| (v, *g)), DisplayProduct::Reflectivity, cut).unwrap();
        assert!(Arc::ptr_eq(r, &surveillance_ref), "the full-range surveillance REF must win");
        let (_, vel) = best_at_cut(all.iter().map(|g| (v, *g)), DisplayProduct::Velocity, cut).unwrap();
        assert!(Arc::ptr_eq(vel, &doppler_vel), "the only VEL at the cut");
    }

    #[test]
    fn sails_re_closures_select_the_newest() {
        let v = volume(100);
        let first = grid(DisplayProduct::Reflectivity, 1, 0.48, 1832);
        let sails = grid(DisplayProduct::Reflectivity, 9, 0.48, 1832);
        let (_, g) = best_at_cut([(v, &first), (v, &sails)], DisplayProduct::Reflectivity, cut_key(0.48)).unwrap();
        assert!(Arc::ptr_eq(g, &sails), "equal range: the later closure (higher elevation_number) wins");
    }

    #[test]
    fn a_newer_volume_wins_among_equal_range() {
        let older = grid(DisplayProduct::Reflectivity, 1, 0.48, 1832);
        let newer = grid(DisplayProduct::Reflectivity, 1, 0.48, 1832);
        let (v, _) =
            best_at_cut([(volume(200), &newer), (volume(100), &older)], DisplayProduct::Reflectivity, cut_key(0.48))
                .unwrap();
        assert_eq!(v, volume(200));
    }

    #[test]
    fn vcp_31s_2_5_never_matches_vcp_35s_2_4() {
        let v = volume(100);
        let vcp31 = grid(DisplayProduct::Reflectivity, 3, 2.5, 1000);
        assert!(best_at_cut([(v, &vcp31)], DisplayProduct::Reflectivity, cut_key(2.4)).is_none());
    }

    #[test]
    fn a_fallback_measured_angle_does_not_match_a_commanded_cut() {
        // Commanded 0.4834 buckets to 48; a fallback measured 0.5273 buckets
        // to 53 — the frame lacks the tilt rather than showing another.
        let v = volume(100);
        let fallback = grid(DisplayProduct::Reflectivity, 1, 0.5273, 1832);
        assert!(best_at_cut([(v, &fallback)], DisplayProduct::Reflectivity, cut_key(0.4834)).is_none());
    }

    #[test]
    fn distinct_cuts_are_ascending_and_deduplicated() {
        let grids = [
            grid(DisplayProduct::Reflectivity, 3, 1.3, 10),
            grid(DisplayProduct::Reflectivity, 1, 0.48, 10),
            grid(DisplayProduct::Velocity, 2, 0.48, 10),
        ];
        assert_eq!(distinct_cuts(grids.iter()), vec![cut_key(0.48), cut_key(1.3)]);
    }

    #[test]
    fn range_extent_is_first_gate_plus_gates_times_width() {
        assert_eq!(range_extent_m(&grid(DisplayProduct::Reflectivity, 1, 0.5, 1832)), 2125 + 1832 * 250);
    }
}
