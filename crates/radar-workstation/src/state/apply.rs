//! The applier: a pure function, in the `VolumeAssembler` pattern (S2-W1
//! §3.3). No `async`, no I/O, no clock access beyond the `now` parameter —
//! the same reasoning as `VolumeAssembler` applies for the same reason, and
//! it is what makes retention behavior across a volume boundary an ordinary
//! unit test rather than something that needs real time to pass.
//!
//! Stage 3 changed the input type from `AssemblyEvent` to
//! `compute::StateUpdate` — everything downstream of assembly deals in
//! grids, not radials. Stage 6a Part B (ADR-0030) changes what happens to a
//! `StateUpdate` once it arrives: every rule below now routes into
//! `RadarState`'s [`history::FrameRing`](super::history::FrameRing) rather
//! than a single merged map. Every rule is the same rule — a VCP change
//! still hides the old pattern's elevations from the live view, a stale
//! sweep still cannot clobber a newer one, `TimedOut`/`Superseded` still
//! clear nothing — but several of them are now structural properties of the
//! ring (a sweep can only ever land in its own volume's frame) rather than
//! guards this function enforces by comparison.
//!
//! Two things a single `bool` could say before but no longer can: whether
//! the byte budget started binding (`history::FrameRing::trim`), and
//! whether a sweep or derived set arrived for a volume old enough that no
//! retained frame would take it (`history::LateVolume`). Both are
//! observability, not data — reported by `AppState::apply_event`, which
//! owns the event log, exactly as `StateUpdate::Info` already was — so
//! `apply` returns `(bool, Vec<Event>)` rather than a bare `bool`.

use std::time::Instant;

use nexrad_decoder::VolumeStatus;

use crate::event::Event;

use super::{DisplaySweep, RadarState};

/// Apply one compute-layer update to `state`. Returns whether anything
/// changed (i.e. whether `revision` was bumped) and any events the caller
/// (`AppState::apply_event`) should report.
pub fn apply(state: &mut RadarState, event: crate::compute::StateUpdate, now: Instant) -> (bool, Vec<Event>) {
    use crate::compute::StateUpdate;

    match event {
        StateUpdate::SweepGridded { elevation_number, elevation_deg, volume, vcp_number, grids } => {
            let landed = state.history.head_mut(volume, vcp_number, now).map(|frame| {
                frame.insert_sweep(DisplaySweep { elevation_number, elevation_deg, volume, vcp_number, received: now, grids });
            });
            if landed.is_err() {
                return (false, vec![Event::LateVolumeDiscarded { volume }]);
            }
            let mut events = Vec::new();
            if let Some(event) = state.history.trim() {
                events.push(event);
            }
            state.revision += 1;
            (true, events)
        }
        StateUpdate::DerivedComputed { volume, vcp_number, grids } => {
            let landed = state.history.head_mut(volume, vcp_number, now).map(|frame| frame.set_derived(grids));
            if landed.is_err() {
                return (false, vec![Event::LateVolumeDiscarded { volume }]);
            }
            let mut events = Vec::new();
            if let Some(event) = state.history.trim() {
                events.push(event);
            }
            state.revision += 1;
            (true, events)
        }
        StateUpdate::VolumeClosed { summary } => {
            // TimedOut / Superseded volumes clear nothing (ADR-0012): a
            // visible gap beats silently modifying data already displayed
            // — and that includes leaving whatever frame this volume has
            // untouched.
            if summary.status != VolumeStatus::Complete {
                return (false, Vec::new());
            }
            // A volume that closed without a single gridded sweep has no
            // frame to mark — `frame_mut` must not conjure one (unlike
            // `head_mut`, which the sweep/derived paths use).
            match state.history.frame_mut(summary.volume) {
                Some(frame) => {
                    frame.complete = Some(summary);
                    state.revision += 1;
                    (true, Vec::new())
                }
                None => (false, Vec::new()),
            }
        }
        // Observability, not data — informational events do not touch the
        // ring or `revision`. `AppState::apply_event` (not this function)
        // is where they reach the event log.
        StateUpdate::Info(_) => (false, Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nexrad_decoder::VolumeStatus;

    use crate::assembly::VolumeId;
    use crate::compute::{DisplayProduct, StateUpdate, SweepGrid};
    use crate::state::history::{self, RetentionPolicy};

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
        RadarState::new(crate::sites::by_id("KDOX").expect("KDOX in bundled table"), RetentionPolicy::default())
    }

    /// The live view, read through the same fold `AppState::snapshot` uses —
    /// every test below reads through here rather than `state.sweeps`,
    /// which no longer exists (ADR-0030).
    fn live_sweeps(state: &RadarState) -> Vec<DisplaySweep> {
        history::live_sweeps(state.history.as_deque())
    }

    #[test]
    fn sweep_closed_becomes_visible_immediately() {
        let mut state = new_state();
        let (changed, events) = apply(&mut state, sweep_gridded(1, 100, 35), t0());
        assert!(changed);
        assert!(events.is_empty());
        assert_eq!(state.revision, 1);
        let sweeps = live_sweeps(&state);
        assert_eq!(sweeps.len(), 1);
        assert!(sweeps.iter().any(|s| s.elevation_number == 1));
    }

    #[test]
    fn stale_sweep_does_not_overwrite_newer() {
        let mut state = new_state();
        apply(&mut state, sweep_gridded(1, 200, 35), t0());
        let revision_after_first = state.revision;

        let (changed, _events) = apply(&mut state, sweep_gridded(1, 100, 35), t0());
        assert!(!changed, "an older volume's sweep must not replace a newer one");
        assert_eq!(state.revision, revision_after_first);
        let sweeps = live_sweeps(&state);
        assert_eq!(sweeps.iter().find(|s| s.elevation_number == 1).unwrap().volume, volume_id(200));
    }

    #[test]
    fn a_stale_sweep_lands_in_its_own_frame_and_does_not_disturb_the_live_view() {
        // Unlike Stage 3, a stale sweep for a volume no longer retained is
        // simply discarded (there is no frame behind the head to land in at
        // all, with only one volume so far retained) — the live view must
        // still show only the newer sweep.
        let mut state = new_state();
        apply(&mut state, sweep_gridded(1, 200, 35), t0());
        let (changed, events) = apply(&mut state, sweep_gridded(1, 100, 35), t0());
        assert!(!changed);
        assert!(matches!(events.as_slice(), [Event::LateVolumeDiscarded { .. }]));
        let sweeps = live_sweeps(&state);
        assert_eq!(sweeps.len(), 1);
        assert_eq!(sweeps[0].volume, volume_id(200));
    }

    #[test]
    fn timed_out_volume_does_not_become_last_complete() {
        let mut state = new_state();
        let (changed, _events) =
            apply(&mut state, StateUpdate::VolumeClosed { summary: volume_summary(VolumeStatus::TimedOut) }, t0());
        assert!(!changed);
        assert!(state.history.newest().is_none());
    }

    #[test]
    fn a_completed_volume_marks_its_own_frame_complete() {
        let mut state = new_state();
        apply(&mut state, sweep_gridded(1, 0, 35), t0());
        let (changed, _events) =
            apply(&mut state, StateUpdate::VolumeClosed { summary: volume_summary(VolumeStatus::Complete) }, t0());
        assert!(changed);
        assert_eq!(state.history.newest().unwrap().complete.unwrap().status, VolumeStatus::Complete);
    }

    #[test]
    fn a_volume_closing_without_any_gridded_sweep_is_a_no_op() {
        let mut state = new_state();
        let (changed, _events) =
            apply(&mut state, StateUpdate::VolumeClosed { summary: volume_summary(VolumeStatus::Complete) }, t0());
        assert!(!changed, "no frame exists yet for this volume; VolumeClosed must not conjure one");
    }

    #[test]
    fn superseded_volume_leaves_visible_sweeps_intact() {
        let mut state = new_state();
        apply(&mut state, sweep_gridded(1, 100, 35), t0());
        apply(&mut state, sweep_gridded(2, 100, 35), t0());
        let revision_before = state.revision;

        let (changed, _events) =
            apply(&mut state, StateUpdate::VolumeClosed { summary: volume_summary(VolumeStatus::Superseded) }, t0());
        assert!(!changed);
        assert_eq!(state.revision, revision_before);
        assert_eq!(live_sweeps(&state).len(), 2, "a superseded volume must not clear already-visible sweeps");
    }

    #[test]
    fn vcp_change_drops_elevations_from_the_old_pattern() {
        let mut state = new_state();
        apply(&mut state, sweep_gridded(1, 100, 35), t0());
        apply(&mut state, sweep_gridded(2, 100, 35), t0());
        assert_eq!(live_sweeps(&state).len(), 2);

        // A new VCP arrives with an elevation 1 but not (yet) elevation 2 —
        // elevation 2's stale VCP-35 sweep must not remain displayed as if
        // it were part of the new pattern. The old frame is still in the
        // ring (ADR-0030 §3.4) — only the live *view* drops it.
        apply(&mut state, sweep_gridded(1, 200, 212), t0());
        let sweeps = live_sweeps(&state);
        assert_eq!(sweeps.len(), 1, "old VCP's elevations must be dropped from the live view on a VCP change");
        assert!(sweeps.iter().any(|s| s.elevation_number == 1));
        assert_eq!(state.history.len(), 2, "the old VCP's frame must still be retained, not deleted");
    }

    #[test]
    fn same_vcp_incomplete_volume_does_not_drop_other_elevations() {
        let mut state = new_state();
        apply(&mut state, sweep_gridded(1, 100, 35), t0());
        apply(&mut state, sweep_gridded(2, 100, 35), t0());
        // Next volume, same VCP, only elevation 1 has closed so far.
        apply(&mut state, sweep_gridded(1, 200, 35), t0());
        let sweeps = live_sweeps(&state);
        assert_eq!(sweeps.len(), 2, "elevation 2 from the prior volume must remain visible");
        assert_eq!(sweeps.iter().find(|s| s.elevation_number == 2).unwrap().volume, volume_id(100));
    }

    #[test]
    fn informational_events_do_not_bump_revision() {
        let mut state = new_state();
        let before = state.revision;
        let (changed, events) = apply(
            &mut state,
            StateUpdate::Info(crate::assembly::AssemblyEvent::LateRadialsDiscarded { elevation_number: 1, count: 3 }),
            t0(),
        );
        assert!(!changed);
        assert!(events.is_empty());
        assert_eq!(state.revision, before);

        let (changed, events) =
            apply(&mut state, StateUpdate::Info(crate::assembly::AssemblyEvent::MissingStartChunk), t0());
        assert!(!changed);
        assert!(events.is_empty());
        assert_eq!(state.revision, before);
    }

    #[test]
    fn reset_clears_the_history_ring() {
        let mut state = new_state();
        apply(&mut state, sweep_gridded(1, 0, 35), t0());
        apply(&mut state, StateUpdate::VolumeClosed { summary: volume_summary(VolumeStatus::Complete) }, t0());
        assert!(!live_sweeps(&state).is_empty());
        assert!(state.history.newest().unwrap().complete.is_some());

        let ktlh = crate::sites::by_id("KTLH").expect("KTLH in bundled table");
        state.reset(ktlh);
        assert!(live_sweeps(&state).is_empty());
        assert!(state.history.newest().is_none());
        assert_eq!(state.site.id, "KTLH");
    }

    #[test]
    fn a_second_volume_starts_a_second_frame() {
        let mut state = new_state();
        apply(&mut state, sweep_gridded(1, 100, 35), t0());
        apply(&mut state, sweep_gridded(1, 200, 35), t0());
        assert_eq!(state.history.len(), 2);
    }

    #[test]
    fn sweeps_from_one_volume_all_land_in_one_frame() {
        let mut state = new_state();
        apply(&mut state, sweep_gridded(1, 100, 35), t0());
        apply(&mut state, sweep_gridded(2, 100, 35), t0());
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.history.newest().unwrap().sweeps.len(), 2);
    }

    #[test]
    fn history_depth_never_exceeds_the_policy() {
        let mut state = RadarState::new(
            crate::sites::by_id("KDOX").expect("KDOX in bundled table"),
            RetentionPolicy { frames: 3, budget_bytes: usize::MAX },
        );
        for t in [100, 200, 300, 400, 500] {
            apply(&mut state, sweep_gridded(1, t, 35), t0());
        }
        assert!(state.history.len() <= 3, "history must never exceed the configured frame count");
    }

    #[test]
    fn derived_products_are_replaced_not_merged() {
        let mut state = new_state();
        let first = StateUpdate::DerivedComputed {
            volume: volume_id(100),
            vcp_number: 35,
            grids: vec![grid(DisplayProduct::EchoTops, 0)],
        };
        apply(&mut state, first, t0());
        assert_eq!(state.history.newest().unwrap().derived.len(), 1);

        let second = StateUpdate::DerivedComputed {
            volume: volume_id(100),
            vcp_number: 35,
            grids: vec![grid(DisplayProduct::Vil, 0)],
        };
        apply(&mut state, second, t0());
        let derived = &state.history.newest().unwrap().derived;
        assert_eq!(derived.len(), 1, "a newer derived set must replace, not merge with, the old one within a frame");
        assert!(derived.contains_key(&DisplayProduct::Vil));
        assert!(!derived.contains_key(&DisplayProduct::EchoTops));
    }

    #[test]
    fn stale_derived_products_do_not_overwrite_newer() {
        let mut state = new_state();
        let newer = StateUpdate::DerivedComputed {
            volume: volume_id(200),
            vcp_number: 35,
            grids: vec![grid(DisplayProduct::Vil, 0)],
        };
        apply(&mut state, newer, t0());

        let older = StateUpdate::DerivedComputed {
            volume: volume_id(100),
            vcp_number: 35,
            grids: vec![grid(DisplayProduct::EchoTops, 0)],
        };
        let (changed, events) = apply(&mut state, older, t0());
        assert!(!changed, "an older volume's derived products must not land ahead of a newer one");
        assert!(matches!(events.as_slice(), [Event::LateVolumeDiscarded { .. }]));
        assert!(state.history.newest().unwrap().derived.contains_key(&DisplayProduct::Vil));
    }

    #[test]
    fn timed_out_volume_leaves_derived_products_intact() {
        let mut state = new_state();
        let grids = StateUpdate::DerivedComputed { volume: volume_id(100), vcp_number: 35, grids: vec![grid(DisplayProduct::Vil, 0)] };
        apply(&mut state, grids, t0());

        apply(&mut state, StateUpdate::VolumeClosed { summary: volume_summary(VolumeStatus::TimedOut) }, t0());
        assert!(
            state.history.newest().unwrap().derived.contains_key(&DisplayProduct::Vil),
            "FR-DA-5: last good data stays displayed"
        );
    }
}
