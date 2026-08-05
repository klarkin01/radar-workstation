//! The applier: a pure function, in the `VolumeAssembler` pattern (S2-W1
//! §3.3). No `async`, no I/O, no clock access beyond the `now` parameter —
//! the same reasoning as `VolumeAssembler` applies for the same reason, and
//! it is what makes retention behavior across a volume boundary an ordinary
//! unit test rather than something that needs real time to pass.
//!
//! Stage 3 changes the input type from `AssemblyEvent` to
//! `compute::StateUpdate` — everything downstream of assembly now deals in
//! grids, not radials — but every rule below is the Stage 2 rule, unchanged
//! in meaning; only how the input is constructed changed.

use std::time::Instant;

use nexrad_decoder::VolumeStatus;

use crate::compute::StateUpdate;

use super::{DisplaySweep, RadarState};

/// Apply one compute-layer update to `state`. Returns whether anything
/// changed (i.e. whether `revision` was bumped).
pub fn apply(state: &mut RadarState, event: StateUpdate, now: Instant) -> bool {
    match event {
        StateUpdate::SweepGridded { elevation_number, elevation_deg, volume, vcp_number, grids } => {
            // A VCP change invalidates the old pattern's elevation set
            // entirely: a new pattern has a different set of tilts, so
            // carrying forward an elevation number from the old one would
            // display a sweep that no longer corresponds to any tilt the
            // antenna is actually scanning. An incomplete volume *within*
            // the same VCP does not hit this — that's the merged-display
            // retention this module exists for. The old VCP's derived
            // products (Echo Tops/VIL) are equally meaningless under a new
            // tilt set, so they are cleared alongside the sweeps.
            if state.current_vcp.is_some_and(|current| current != vcp_number) {
                state.sweeps.clear();
                state.derived.clear();
                state.derived_volume = None;
            }
            state.current_vcp = Some(vcp_number);

            let replace = match state.sweeps.get(&elevation_number) {
                // Re-anchoring and late chunks can in principle deliver
                // events out of order; a sweep from an older volume must
                // never clobber one already displayed from a newer volume.
                Some(existing) => volume >= existing.volume,
                None => true,
            };
            if !replace {
                return false;
            }
            state.sweeps.insert(
                elevation_number,
                DisplaySweep { elevation_number, elevation_deg, volume, vcp_number, received: now, grids },
            );
            state.revision += 1;
            true
        }
        StateUpdate::DerivedComputed { volume, vcp_number: _, grids } => {
            // Same out-of-order guard as sweeps: a `DerivedComputed` from
            // an older volume must never overwrite a newer volume's Echo
            // Tops/VIL.
            if state.derived_volume.is_some_and(|existing| volume < existing) {
                return false;
            }
            // Replaced wholesale, not merged: Echo Tops and VIL are
            // properties of one whole volume, so a partial merge would mix
            // one volume's low-level tilts with another's.
            state.derived.clear();
            for grid in grids {
                state.derived.insert(grid.product, grid);
            }
            state.derived_volume = Some(volume);
            state.revision += 1;
            true
        }
        StateUpdate::VolumeClosed { summary } => {
            // TimedOut / Superseded volumes clear nothing (ADR-0012): a
            // visible gap beats silently modifying data already displayed
            // — and that includes leaving the previous `derived` set in
            // place (FR-DA-5: last good data stays displayed).
            if summary.status != VolumeStatus::Complete {
                return false;
            }
            state.last_complete = Some(summary);
            state.revision += 1;
            true
        }
        // Observability, not data — informational events do not touch
        // sweeps, derived products, or revision. AppState::apply_event (not
        // this function) is where they reach the event log.
        StateUpdate::Info(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nexrad_decoder::VolumeStatus;

    use crate::assembly::VolumeId;
    use crate::compute::{DisplayProduct, SweepGrid};

    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    fn volume_id(scan_time_ms: u32) -> VolumeId {
        VolumeId { julian_date: 20000, scan_time_ms }
    }

    fn grid(product: DisplayProduct, elevation_number: u8) -> Arc<SweepGrid> {
        Arc::new(SweepGrid {
            product,
            azimuth_count: 4,
            gate_count: 4,
            first_gate_m: 0,
            gate_width_m: 250,
            elevation_number,
            elevation_deg: elevation_number as f32 * 0.5,
            nyquist_velocity_mps: Some(8.0),
            scale: 2.0,
            offset: 66.0,
            cells: vec![0u8; 16],
            filled_azimuths: 0,
        })
    }

    fn sweep_gridded(elevation_number: u8, scan_time_ms: u32, vcp_number: u16) -> StateUpdate {
        StateUpdate::SweepGridded {
            elevation_number,
            elevation_deg: elevation_number as f32 * 0.5,
            volume: volume_id(scan_time_ms),
            vcp_number,
            grids: vec![grid(DisplayProduct::Reflectivity, elevation_number)],
        }
    }

    fn volume_summary(status: VolumeStatus) -> super::super::VolumeSummary {
        super::super::VolumeSummary {
            volume: volume_id(0),
            vcp_number: 35,
            status,
            latitude: 38.8258,
            longitude: -75.4401,
            site_amsl_m: 15,
        }
    }

    fn new_state() -> RadarState {
        RadarState::new(crate::sites::by_id("KDOX").expect("KDOX in bundled table"))
    }

    #[test]
    fn sweep_closed_becomes_visible_immediately() {
        let mut state = new_state();
        let changed = apply(&mut state, sweep_gridded(1, 100, 35), t0());
        assert!(changed);
        assert_eq!(state.revision, 1);
        assert_eq!(state.sweeps.len(), 1);
        assert!(state.sweeps.contains_key(&1));
    }

    #[test]
    fn stale_sweep_does_not_overwrite_newer() {
        let mut state = new_state();
        apply(&mut state, sweep_gridded(1, 200, 35), t0());
        let revision_after_first = state.revision;

        let changed = apply(&mut state, sweep_gridded(1, 100, 35), t0());
        assert!(!changed, "an older volume's sweep must not replace a newer one");
        assert_eq!(state.revision, revision_after_first);
        assert_eq!(state.sweeps[&1].volume, volume_id(200));
    }

    #[test]
    fn timed_out_volume_does_not_become_last_complete() {
        let mut state = new_state();
        let changed = apply(&mut state, StateUpdate::VolumeClosed { summary: volume_summary(VolumeStatus::TimedOut) }, t0());
        assert!(!changed);
        assert!(state.last_complete.is_none());
    }

    #[test]
    fn complete_volume_becomes_last_complete() {
        let mut state = new_state();
        let changed =
            apply(&mut state, StateUpdate::VolumeClosed { summary: volume_summary(VolumeStatus::Complete) }, t0());
        assert!(changed);
        assert_eq!(state.last_complete.unwrap().status, VolumeStatus::Complete);
    }

    #[test]
    fn superseded_volume_leaves_visible_sweeps_intact() {
        let mut state = new_state();
        apply(&mut state, sweep_gridded(1, 100, 35), t0());
        apply(&mut state, sweep_gridded(2, 100, 35), t0());
        let revision_before = state.revision;

        let changed =
            apply(&mut state, StateUpdate::VolumeClosed { summary: volume_summary(VolumeStatus::Superseded) }, t0());
        assert!(!changed);
        assert_eq!(state.revision, revision_before);
        assert_eq!(state.sweeps.len(), 2, "a superseded volume must not clear already-visible sweeps");
    }

    #[test]
    fn vcp_change_drops_elevations_from_the_old_pattern() {
        let mut state = new_state();
        apply(&mut state, sweep_gridded(1, 100, 35), t0());
        apply(&mut state, sweep_gridded(2, 100, 35), t0());
        assert_eq!(state.sweeps.len(), 2);

        // A new VCP arrives with an elevation 1 but not (yet) elevation 2 —
        // elevation 2's stale VCP-35 sweep must not remain displayed as if
        // it were part of the new pattern.
        apply(&mut state, sweep_gridded(1, 200, 212), t0());
        assert_eq!(state.sweeps.len(), 1, "old VCP's elevations must be dropped on a VCP change");
        assert!(state.sweeps.contains_key(&1));
    }

    #[test]
    fn same_vcp_incomplete_volume_does_not_drop_other_elevations() {
        let mut state = new_state();
        apply(&mut state, sweep_gridded(1, 100, 35), t0());
        apply(&mut state, sweep_gridded(2, 100, 35), t0());
        // Next volume, same VCP, only elevation 1 has closed so far.
        apply(&mut state, sweep_gridded(1, 200, 35), t0());
        assert_eq!(state.sweeps.len(), 2, "elevation 2 from the prior volume must remain visible");
        assert_eq!(state.sweeps[&2].volume, volume_id(100));
    }

    #[test]
    fn informational_events_do_not_bump_revision() {
        let mut state = new_state();
        let before = state.revision;
        let changed = apply(
            &mut state,
            StateUpdate::Info(crate::assembly::AssemblyEvent::LateRadialsDiscarded { elevation_number: 1, count: 3 }),
            t0(),
        );
        assert!(!changed);
        assert_eq!(state.revision, before);

        let changed =
            apply(&mut state, StateUpdate::Info(crate::assembly::AssemblyEvent::MissingStartChunk), t0());
        assert!(!changed);
        assert_eq!(state.revision, before);
    }

    #[test]
    fn reset_clears_all_radar_state() {
        let mut state = new_state();
        apply(&mut state, sweep_gridded(1, 100, 35), t0());
        apply(&mut state, StateUpdate::VolumeClosed { summary: volume_summary(VolumeStatus::Complete) }, t0());
        assert!(!state.sweeps.is_empty());
        assert!(state.last_complete.is_some());

        let ktlh = crate::sites::by_id("KTLH").expect("KTLH in bundled table");
        state.reset(ktlh);
        assert!(state.sweeps.is_empty());
        assert!(state.last_complete.is_none());
        assert_eq!(state.site.id, "KTLH");
    }

    #[test]
    fn derived_products_are_replaced_not_merged() {
        let mut state = new_state();
        let first = vec![grid(DisplayProduct::EchoTops, 0)];
        apply(&mut state, StateUpdate::DerivedComputed { volume: volume_id(100), vcp_number: 35, grids: first }, t0());
        assert_eq!(state.derived.len(), 1);

        let second = vec![grid(DisplayProduct::Vil, 0)];
        apply(&mut state, StateUpdate::DerivedComputed { volume: volume_id(200), vcp_number: 35, grids: second }, t0());
        assert_eq!(state.derived.len(), 1, "a newer volume's derived set must replace, not merge with, the old one");
        assert!(state.derived.contains_key(&DisplayProduct::Vil));
        assert!(!state.derived.contains_key(&DisplayProduct::EchoTops));
    }

    #[test]
    fn stale_derived_products_do_not_overwrite_newer() {
        let mut state = new_state();
        let newer = vec![grid(DisplayProduct::Vil, 0)];
        apply(&mut state, StateUpdate::DerivedComputed { volume: volume_id(200), vcp_number: 35, grids: newer }, t0());

        let older = vec![grid(DisplayProduct::EchoTops, 0)];
        let changed =
            apply(&mut state, StateUpdate::DerivedComputed { volume: volume_id(100), vcp_number: 35, grids: older }, t0());
        assert!(!changed, "an older volume's derived products must not replace a newer volume's");
        assert!(state.derived.contains_key(&DisplayProduct::Vil));
    }

    #[test]
    fn timed_out_volume_leaves_derived_products_intact() {
        let mut state = new_state();
        let grids = vec![grid(DisplayProduct::Vil, 0)];
        apply(&mut state, StateUpdate::DerivedComputed { volume: volume_id(100), vcp_number: 35, grids }, t0());

        apply(&mut state, StateUpdate::VolumeClosed { summary: volume_summary(VolumeStatus::TimedOut) }, t0());
        assert!(state.derived.contains_key(&DisplayProduct::Vil), "FR-DA-5: last good data stays displayed");
    }
}
