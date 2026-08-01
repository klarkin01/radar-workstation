//! The applier: a pure function, in the `VolumeAssembler` pattern (S2-W1
//! §3.3). No `async`, no I/O, no clock access beyond the `now` parameter —
//! the same reasoning as `VolumeAssembler` applies for the same reason, and
//! it is what makes retention behavior across a volume boundary an ordinary
//! unit test rather than something that needs real time to pass.

use std::time::Instant;

use nexrad_decoder::VolumeStatus;

use crate::assembly::AssemblyEvent;

use super::{DisplaySweep, RadarState};

/// Apply one assembly event to `state`. Returns whether anything changed
/// (i.e. whether `revision` was bumped).
pub fn apply(state: &mut RadarState, event: AssemblyEvent, now: Instant) -> bool {
    match event {
        AssemblyEvent::SweepClosed { sweep, volume, vcp_number } => {
            // A VCP change invalidates the old pattern's elevation set
            // entirely: a new pattern has a different set of tilts, so
            // carrying forward an elevation number from the old one would
            // display a sweep that no longer corresponds to any tilt the
            // antenna is actually scanning. An incomplete volume *within*
            // the same VCP does not hit this — that's the merged-display
            // retention this module exists for.
            if state.current_vcp.is_some_and(|current| current != vcp_number) {
                state.sweeps.clear();
            }
            state.current_vcp = Some(vcp_number);

            let elevation_number = sweep.elevation_number;
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
            state.sweeps.insert(elevation_number, DisplaySweep { sweep, volume, vcp_number, received: now });
            state.revision += 1;
            true
        }
        AssemblyEvent::VolumeClosed { volume } => {
            // TimedOut / Superseded volumes clear nothing (ADR-0012): a
            // visible gap beats silently modifying data already displayed.
            // Only a Complete volume becomes "the last successfully fetched
            // scan" FR-DA-5 asks for.
            if volume.status != VolumeStatus::Complete {
                return false;
            }
            state.last_complete = Some(std::sync::Arc::new(volume));
            state.revision += 1;
            true
        }
        // Observability, not data — informational events do not touch
        // sweeps or bump revision. AppState::report (not this function)
        // is where they reach the event log.
        AssemblyEvent::LateRadialsDiscarded { .. } | AssemblyEvent::MissingStartChunk => false,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nexrad_decoder::{Sweep, VolumeScan, VolumeStatus};

    use crate::assembly::VolumeId;

    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    fn volume_id(scan_time_ms: u32) -> VolumeId {
        VolumeId { julian_date: 20000, scan_time_ms }
    }

    fn sweep(elevation_number: u8) -> Arc<Sweep> {
        Arc::new(Sweep {
            elevation_number,
            elevation_deg: elevation_number as f32 * 0.5,
            nyquist_velocity_mps: Some(8.0),
            unambiguous_range_km: Some(300.0),
            radials: Vec::new(),
            complete: true,
        })
    }

    fn sweep_closed(elevation_number: u8, scan_time_ms: u32, vcp_number: u16) -> AssemblyEvent {
        AssemblyEvent::SweepClosed { sweep: sweep(elevation_number), volume: volume_id(scan_time_ms), vcp_number }
    }

    fn volume_scan(status: VolumeStatus) -> VolumeScan {
        VolumeScan {
            site_id: *b"KDOX",
            scan_time_ms: 0,
            julian_date: 20000,
            vcp_number: 35,
            latitude: 38.8258,
            longitude: -75.4401,
            site_amsl_m: 15,
            sweeps: Vec::new(),
            status,
        }
    }

    fn new_state() -> RadarState {
        RadarState::new(crate::sites::by_id("KDOX").expect("KDOX in bundled table"))
    }

    #[test]
    fn sweep_closed_becomes_visible_immediately() {
        let mut state = new_state();
        let changed = apply(&mut state, sweep_closed(1, 100, 35), t0());
        assert!(changed);
        assert_eq!(state.revision, 1);
        assert_eq!(state.sweeps.len(), 1);
        assert!(state.sweeps.contains_key(&1));
    }

    #[test]
    fn stale_sweep_does_not_overwrite_newer() {
        let mut state = new_state();
        apply(&mut state, sweep_closed(1, 200, 35), t0());
        let revision_after_first = state.revision;

        let changed = apply(&mut state, sweep_closed(1, 100, 35), t0());
        assert!(!changed, "an older volume's sweep must not replace a newer one");
        assert_eq!(state.revision, revision_after_first);
        assert_eq!(state.sweeps[&1].volume, volume_id(200));
    }

    #[test]
    fn timed_out_volume_does_not_become_last_complete() {
        let mut state = new_state();
        let changed = apply(&mut state, AssemblyEvent::VolumeClosed { volume: volume_scan(VolumeStatus::TimedOut) }, t0());
        assert!(!changed);
        assert!(state.last_complete.is_none());
    }

    #[test]
    fn complete_volume_becomes_last_complete() {
        let mut state = new_state();
        let changed =
            apply(&mut state, AssemblyEvent::VolumeClosed { volume: volume_scan(VolumeStatus::Complete) }, t0());
        assert!(changed);
        assert_eq!(state.last_complete.as_ref().unwrap().status, VolumeStatus::Complete);
    }

    #[test]
    fn superseded_volume_leaves_visible_sweeps_intact() {
        let mut state = new_state();
        apply(&mut state, sweep_closed(1, 100, 35), t0());
        apply(&mut state, sweep_closed(2, 100, 35), t0());
        let revision_before = state.revision;

        let changed =
            apply(&mut state, AssemblyEvent::VolumeClosed { volume: volume_scan(VolumeStatus::Superseded) }, t0());
        assert!(!changed);
        assert_eq!(state.revision, revision_before);
        assert_eq!(state.sweeps.len(), 2, "a superseded volume must not clear already-visible sweeps");
    }

    #[test]
    fn vcp_change_drops_elevations_from_the_old_pattern() {
        let mut state = new_state();
        apply(&mut state, sweep_closed(1, 100, 35), t0());
        apply(&mut state, sweep_closed(2, 100, 35), t0());
        assert_eq!(state.sweeps.len(), 2);

        // A new VCP arrives with an elevation 1 but not (yet) elevation 2 —
        // elevation 2's stale VCP-35 sweep must not remain displayed as if
        // it were part of the new pattern.
        apply(&mut state, sweep_closed(1, 200, 212), t0());
        assert_eq!(state.sweeps.len(), 1, "old VCP's elevations must be dropped on a VCP change");
        assert!(state.sweeps.contains_key(&1));
    }

    #[test]
    fn same_vcp_incomplete_volume_does_not_drop_other_elevations() {
        let mut state = new_state();
        apply(&mut state, sweep_closed(1, 100, 35), t0());
        apply(&mut state, sweep_closed(2, 100, 35), t0());
        // Next volume, same VCP, only elevation 1 has closed so far.
        apply(&mut state, sweep_closed(1, 200, 35), t0());
        assert_eq!(state.sweeps.len(), 2, "elevation 2 from the prior volume must remain visible");
        assert_eq!(state.sweeps[&2].volume, volume_id(100));
    }

    #[test]
    fn informational_events_do_not_bump_revision() {
        let mut state = new_state();
        let before = state.revision;
        let changed = apply(&mut state, AssemblyEvent::LateRadialsDiscarded { elevation_number: 1, count: 3 }, t0());
        assert!(!changed);
        assert_eq!(state.revision, before);

        let changed = apply(&mut state, AssemblyEvent::MissingStartChunk, t0());
        assert!(!changed);
        assert_eq!(state.revision, before);
    }

    #[test]
    fn reset_clears_all_radar_state() {
        let mut state = new_state();
        apply(&mut state, sweep_closed(1, 100, 35), t0());
        apply(&mut state, AssemblyEvent::VolumeClosed { volume: volume_scan(VolumeStatus::Complete) }, t0());
        assert!(!state.sweeps.is_empty());
        assert!(state.last_complete.is_some());

        let ktlh = crate::sites::by_id("KTLH").expect("KTLH in bundled table");
        state.reset(ktlh);
        assert!(state.sweeps.is_empty());
        assert!(state.last_complete.is_none());
        assert_eq!(state.site.id, "KTLH");
    }
}
