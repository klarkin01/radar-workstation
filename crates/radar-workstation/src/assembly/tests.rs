use std::time::{Duration, Instant};

use nexrad_decoder::{
    ProductMap, Radial, RadialStatus, SiteParameters, VcpDefinition, VolumeStatus,
};

use super::*;

fn radial(elevation_number: u8, status: RadialStatus) -> Radial {
    Radial {
        site_id: *b"TEST",
        scan_time_ms: 0,
        julian_date: 0,
        azimuth_deg: 0.0,
        elevation_deg: elevation_number as f32 * 0.5,
        azimuth_number: 1,
        azimuth_spacing_code: 2,
        radial_status: status,
        elevation_number,
        unambiguous_range_km: Some(300.0),
        nyquist_velocity_mps: Some(8.0),
        site_parameters: None,
        products: ProductMap::new(),
    }
}

fn radial_with_vcp(elevation_number: u8, status: RadialStatus, vcp_number: u16) -> Radial {
    let mut r = radial(elevation_number, status);
    r.site_parameters = Some(SiteParameters {
        latitude: 38.0,
        longitude: -75.0,
        site_amsl_m: 15,
        feedhorn_agl_m: 34,
        calib_dbz: 0.0,
        txpower_h: 0.0,
        txpower_v: 0.0,
        sys_zdr: 0.0,
        phidp0: 0.0,
        vcp_number,
        processing_status: 0,
    });
    r
}

fn vcp_definition(vcp_number: u16) -> VcpDefinition {
    VcpDefinition { vcp_number, pattern_type: 2, elevations: Vec::new() }
}

fn t0() -> Instant {
    Instant::now()
}

fn sweep_closed_events(events: &[AssemblyEvent]) -> Vec<(u8, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            AssemblyEvent::SweepClosed { sweep, .. } => Some((sweep.elevation_number, sweep.complete)),
            _ => None,
        })
        .collect()
}

fn volume_closed(events: &[AssemblyEvent]) -> Option<&VolumeScan> {
    events.iter().find_map(|e| match e {
        AssemblyEvent::VolumeClosed { volume } => Some(volume),
        _ => None,
    })
}

#[test]
fn start_chunk_initializes_context() {
    let mut a = VolumeAssembler::new(AssemblyConfig::default());
    let events = a.on_chunk(ChunkKind::Start, vec![], t0());

    assert_eq!(a.state(), AssemblyState::AwaitingData);
    assert!(events.is_empty(), "a bare -S with no accumulation in progress should emit nothing");
}

#[test]
fn first_intermediate_begins_accumulating() {
    let mut a = VolumeAssembler::new(AssemblyConfig::default());
    a.on_chunk(ChunkKind::Start, vec![], t0());

    let events = a.on_chunk(
        ChunkKind::Intermediate,
        vec![radial_with_vcp(1, RadialStatus::StartOfVolume, 212)],
        t0(),
    );

    assert_eq!(a.state(), AssemblyState::Accumulating);
    assert!(events.is_empty(), "starting a sweep should not itself close anything");
}

#[test]
fn end_of_elevation_closes_sweep() {
    let mut a = VolumeAssembler::new(AssemblyConfig::default());
    a.on_chunk(ChunkKind::Start, vec![], t0());
    a.on_chunk(ChunkKind::Intermediate, vec![radial(1, RadialStatus::StartOfVolume)], t0());
    let events =
        a.on_chunk(ChunkKind::Intermediate, vec![radial(1, RadialStatus::EndOfElevation)], t0());

    assert_eq!(sweep_closed_events(&events), vec![(1, true)]);
}

#[test]
fn elevation_change_closes_previous_sweep_incomplete() {
    let mut a = VolumeAssembler::new(AssemblyConfig::default());
    a.on_chunk(ChunkKind::Start, vec![], t0());
    a.on_chunk(ChunkKind::Intermediate, vec![radial(1, RadialStatus::StartOfVolume)], t0());

    // Elevation 2 arrives without elevation 1 ever seeing EndOfElevation.
    let events =
        a.on_chunk(ChunkKind::Intermediate, vec![radial(2, RadialStatus::StartOfElevation)], t0());

    assert_eq!(sweep_closed_events(&events), vec![(1, false)]);
    assert_eq!(a.state(), AssemblyState::Accumulating);
}

#[test]
fn late_radials_for_closed_sweep_are_discarded() {
    let mut a = VolumeAssembler::new(AssemblyConfig::default());
    a.on_chunk(ChunkKind::Start, vec![], t0());
    a.on_chunk(ChunkKind::Intermediate, vec![radial(1, RadialStatus::StartOfVolume)], t0());
    a.on_chunk(ChunkKind::Intermediate, vec![radial(1, RadialStatus::EndOfElevation)], t0());

    // Elevation 1's sweep is now closed; a late radial for it must not
    // reopen or mutate it.
    let events = a.on_chunk(
        ChunkKind::Intermediate,
        vec![radial(1, RadialStatus::Intermediate), radial(1, RadialStatus::Intermediate)],
        t0(),
    );

    assert!(matches!(
        events.as_slice(),
        [AssemblyEvent::LateRadialsDiscarded { elevation_number: 1, count: 2 }]
    ));
    assert_eq!(a.closed_sweeps.len(), 1, "the closed sweep must not be reopened or duplicated");
    assert_eq!(a.closed_sweeps[0].radials.len(), 2, "the original sweep's radials must be unmutated");
}

#[test]
fn decoded_vcp_from_message5_takes_priority_over_rvol_fallback() {
    let mut a = VolumeAssembler::new(AssemblyConfig::default());
    a.on_start_chunk(Some(vcp_definition(35)), t0());
    // The radial's RVOL block disagrees (212) — Message 5, once decoded,
    // must win.
    a.on_chunk(ChunkKind::Intermediate, vec![radial_with_vcp(1, RadialStatus::StartOfVolume, 212)], t0());
    let events = a.on_chunk(ChunkKind::End, vec![radial_with_vcp(1, RadialStatus::EndOfVolume, 212)], t0());

    let volume = volume_closed(&events).expect("VolumeClosed event");
    assert_eq!(volume.vcp_number, 35);
}

#[test]
fn missing_vcp_falls_back_to_rvol_number() {
    let mut a = VolumeAssembler::new(AssemblyConfig::default());
    // No Message 5 decoded for this -S (e.g. it was absent or unreadable).
    a.on_start_chunk(None, t0());
    a.on_chunk(ChunkKind::Intermediate, vec![radial_with_vcp(1, RadialStatus::StartOfVolume, 212)], t0());
    let events = a.on_chunk(ChunkKind::End, vec![radial_with_vcp(1, RadialStatus::EndOfVolume, 212)], t0());

    let volume = volume_closed(&events).expect("VolumeClosed event");
    assert_eq!(volume.vcp_number, 212);
}

#[test]
fn end_chunk_completes_volume() {
    let mut a = VolumeAssembler::new(AssemblyConfig::default());
    a.on_chunk(ChunkKind::Start, vec![], t0());
    a.on_chunk(
        ChunkKind::Intermediate,
        vec![radial_with_vcp(1, RadialStatus::StartOfVolume, 35)],
        t0(),
    );
    let events = a.on_chunk(ChunkKind::End, vec![radial(1, RadialStatus::EndOfVolume)], t0());

    // SweepClosed must precede VolumeClosed within the same call.
    assert_eq!(sweep_closed_events(&events), vec![(1, true)]);
    let sweep_idx = events.iter().position(|e| matches!(e, AssemblyEvent::SweepClosed { .. })).unwrap();
    let volume_idx = events.iter().position(|e| matches!(e, AssemblyEvent::VolumeClosed { .. })).unwrap();
    assert!(sweep_idx < volume_idx);

    let volume = volume_closed(&events).expect("VolumeClosed event");
    assert_eq!(volume.status, VolumeStatus::Complete);
    assert_eq!(volume.sweeps.len(), 1);
    assert_eq!(volume.vcp_number, 35);
    assert_eq!(a.state(), AssemblyState::Idle);
}

#[test]
fn early_start_chunk_supersedes_volume() {
    let mut a = VolumeAssembler::new(AssemblyConfig::default());
    a.on_chunk(ChunkKind::Start, vec![], t0());
    a.on_chunk(ChunkKind::Intermediate, vec![radial(1, RadialStatus::StartOfVolume)], t0());

    // A new -S arrives before -E.
    let events = a.on_chunk(ChunkKind::Start, vec![], t0());

    assert_eq!(sweep_closed_events(&events), vec![(1, false)]);
    let volume = volume_closed(&events).expect("VolumeClosed event");
    assert_eq!(volume.status, VolumeStatus::Superseded);

    // The new volume begins immediately in the same call.
    assert_eq!(a.state(), AssemblyState::AwaitingData);
}

#[test]
fn watchdog_times_out_stalled_volume() {
    let config = AssemblyConfig { watchdog_timeout: Duration::from_secs(600) };
    let mut a = VolumeAssembler::new(config);
    let start = t0();
    a.on_chunk(ChunkKind::Start, vec![], start);
    a.on_chunk(ChunkKind::Intermediate, vec![radial(1, RadialStatus::StartOfVolume)], start);

    // Well before the threshold: nothing happens.
    let events = a.on_tick(start + Duration::from_secs(300));
    assert!(events.is_empty());
    assert_eq!(a.state(), AssemblyState::Accumulating);

    // Past the threshold: the stalled volume closes.
    let events = a.on_tick(start + Duration::from_secs(601));
    assert_eq!(sweep_closed_events(&events), vec![(1, false)]);
    let volume = volume_closed(&events).expect("VolumeClosed event");
    assert_eq!(volume.status, VolumeStatus::TimedOut);
    assert_eq!(a.state(), AssemblyState::Idle);
}

#[test]
fn watchdog_is_a_no_op_when_idle() {
    let mut a = VolumeAssembler::new(AssemblyConfig::default());
    let events = a.on_tick(t0() + Duration::from_secs(10_000));
    assert!(events.is_empty());
}

#[test]
fn intermediate_without_start_chunk_still_accumulates() {
    let mut a = VolumeAssembler::new(AssemblyConfig::default());
    let events = a.on_chunk(ChunkKind::Intermediate, vec![radial(1, RadialStatus::Intermediate)], t0());

    assert!(matches!(events.first(), Some(AssemblyEvent::MissingStartChunk)));
    assert_eq!(a.state(), AssemblyState::Accumulating);
}

#[test]
fn missing_intermediate_leaves_azimuth_gap() {
    // No interpolation or gap-filling: a sweep with fewer radials than a
    // full 360° sweep simply has fewer radials. Nothing actively "detects"
    // a gap — the absence is the whole story.
    let mut a = VolumeAssembler::new(AssemblyConfig::default());
    a.on_chunk(ChunkKind::Start, vec![], t0());
    a.on_chunk(ChunkKind::Intermediate, vec![radial(1, RadialStatus::StartOfVolume)], t0());
    // Simulate a dropped -I chunk: azimuths 2..119 never arrive, straight to
    // the closing radial.
    let events =
        a.on_chunk(ChunkKind::Intermediate, vec![radial(1, RadialStatus::EndOfElevation)], t0());

    let AssemblyEvent::SweepClosed { sweep, .. } = &events[0] else { panic!("expected SweepClosed") };
    assert_eq!(sweep.radials.len(), 2, "only the radials actually received are present");
    assert!(sweep.complete, "EndOfElevation was received, so the sweep is still marked complete");
}

#[test]
fn reset_returns_to_idle_and_drops_in_flight_state() {
    let mut a = VolumeAssembler::new(AssemblyConfig::default());
    a.on_chunk(ChunkKind::Start, vec![], t0());
    a.on_chunk(ChunkKind::Intermediate, vec![radial(1, RadialStatus::StartOfVolume)], t0());
    assert_eq!(a.state(), AssemblyState::Accumulating);

    a.reset();

    assert_eq!(a.state(), AssemblyState::Idle);
    // A tick after reset must not resurrect a stale timedout volume.
    assert!(a.on_tick(t0() + Duration::from_secs(10_000)).is_empty());
}
