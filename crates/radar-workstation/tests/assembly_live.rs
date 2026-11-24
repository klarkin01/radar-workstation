//! Live end-to-end test wiring the real S3 poller into the volume
//! assembler. `#[ignore]`d — dials out to a real bucket — mirroring the
//! live tests already in `src/ingest/s3_poll.rs`. Run with:
//!
//!   cargo test -p radar-workstation --test assembly_live -- --ignored --nocapture
//!
//! This is the first real measurement of FR-DA-3's "displayable within
//! 30-60 seconds" claim (S1-W1 in
//! docs/plans/stage-0-1-close-the-acquisition-path.md): wall-clock time
//! from poller start to the first SweepClosed event.
//!
//! `cut_identity_facts_hold_on_a_live_volume` is Stage 6a Part C Phase 0:
//! the data facts ADR-0031 D-2/D-3 rest on, measured on a real volume.
//! `RADAR_LIVE_SITE` picks the site (default KDOX) — run it once against a
//! site in VCP 212/215 too, for SAILS:
//!
//!   RADAR_LIVE_SITE=KTLX cargo test -p radar-workstation --test assembly_live \
//!     cut_identity -- --ignored --nocapture

use std::sync::Arc;
use std::time::{Duration, Instant};

use nexrad_decoder::{ProductKind, VcpDefinition, VolumeStatus};
use radar_workstation::assembly::{self, AssemblyConfig, AssemblyEvent, VolumeAssembler, CUT_ANGLE_TOLERANCE_DEG};
use radar_workstation::compute::cut::{cut_key, range_extent_m};
use radar_workstation::compute::{grid, DisplayProduct};
use radar_workstation::ingest::s3_poll::{IngestStatus, S3Poller, DEFAULT_POLL_INTERVAL};
use radar_workstation::ingest::ChunkEnvelope;
use radar_workstation::state::{AppState, RetentionPolicy};

const LIVE_TEST_SITE: &str = "KDOX";
const OVERALL_TIMEOUT: Duration = Duration::from_secs(20 * 60);

#[tokio::test]
#[ignore]
async fn polls_a_real_site_to_one_complete_volume() {
    let client = http_ingest::S3Client::new(http_ingest::Bucket::Chunks);
    let site = radar_workstation::sites::by_id(LIVE_TEST_SITE).expect("site in bundled table");
    let (status_tx, ingest_rx) = tokio::sync::watch::channel(IngestStatus::default());
    let poller = S3Poller::new(LIVE_TEST_SITE, client, status_tx);
    let app_state = Arc::new(AppState::new(site, ingest_rx, RetentionPolicy::default()));
    let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel::<ChunkEnvelope>(32);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<AssemblyEvent>(256);

    let poll_task =
        tokio::spawn(poller.run(chunk_tx, Arc::clone(&app_state), shutdown_rx.clone(), DEFAULT_POLL_INTERVAL));
    let assembly_task =
        tokio::spawn(assembly::run(chunk_rx, event_tx, AssemblyConfig::default(), shutdown_rx));

    let start = Instant::now();
    let mut time_to_first_sweep: Option<Duration> = None;
    let mut sweep_count = 0usize;

    let result = tokio::time::timeout(OVERALL_TIMEOUT, async {
        while let Some(event) = event_rx.recv().await {
            match event {
                AssemblyEvent::SweepClosed { sweep, .. } => {
                    sweep_count += 1;
                    if time_to_first_sweep.is_none() {
                        time_to_first_sweep = Some(start.elapsed());
                    }
                    println!(
                        "SweepClosed: el_num={} el_angle={:.2} radials={} complete={}",
                        sweep.elevation_number,
                        sweep.elevation_deg,
                        sweep.radials.len(),
                        sweep.complete
                    );
                }
                AssemblyEvent::VolumeClosed { volume } => {
                    println!(
                        "VolumeClosed: site={} vcp={} status={:?} sweeps={} total_time={:?}",
                        volume.site_id_str(),
                        volume.vcp_number,
                        volume.status,
                        volume.sweeps.len(),
                        start.elapsed(),
                    );
                    if volume.status == VolumeStatus::Complete {
                        return;
                    }
                }
                AssemblyEvent::LateRadialsDiscarded { elevation_number, count } => {
                    println!("LateRadialsDiscarded: el_num={elevation_number} count={count}");
                }
                AssemblyEvent::MissingStartChunk => {
                    println!("MissingStartChunk");
                }
            }
        }
    })
    .await;

    poll_task.abort();
    assembly_task.abort();

    result.expect("timed out waiting for a Complete VolumeClosed event");
    println!(
        "time_to_first_sweep={:?} sweep_count={sweep_count}",
        time_to_first_sweep.expect("at least one SweepClosed before VolumeClosed")
    );
}

/// One closed sweep, as Phase 0 tabulates it.
struct CutRow {
    elevation_number: u8,
    /// `Sweep::elevation_deg` — the first radial's angle.
    first_radial_deg: f32,
    /// The median radial angle `assembly::cut_angle` is checked against.
    measured_deg: f32,
    commanded_deg: Option<f32>,
    cut_deg: f32,
    ref_range_m: Option<u32>,
    has_vel: bool,
}

/// Stage 6a Part C Phase 0 (ADR-0031 D-2, D-3). Assembles one live volume
/// that began with a `-S` chunk (so Message 5 is known) and asserts:
///
/// 1. every sweep's `cut[elevation_number - 1]` exists and lies within
///    `CUT_ANGLE_TOLERANCE_DEG` of the measured angle;
/// 2. at every split-cut angle, the surveillance REF (no VEL) reaches at
///    least as far as the Doppler REF.
///
/// Drives `VolumeAssembler` directly rather than through `assembly::run`,
/// because the check needs the decoded VCP definition, which assembly does
/// not publish. A failure does not reopen D-2: the measured-angle fallback
/// covers it (the frame shows as lacking the tilt, never wrong).
#[tokio::test]
#[ignore]
async fn cut_identity_facts_hold_on_a_live_volume() {
    let site_id = std::env::var("RADAR_LIVE_SITE").unwrap_or_else(|_| LIVE_TEST_SITE.to_string());
    let client = http_ingest::S3Client::new(http_ingest::Bucket::Chunks);
    let site = radar_workstation::sites::by_id(&site_id).expect("site in bundled table");
    let (status_tx, ingest_rx) = tokio::sync::watch::channel(IngestStatus::default());
    let poller = S3Poller::new(site.id, client, status_tx);
    let app_state = Arc::new(AppState::new(site, ingest_rx, RetentionPolicy::default()));
    let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::channel::<ChunkEnvelope>(32);
    let poll_task = tokio::spawn(poller.run(chunk_tx, Arc::clone(&app_state), shutdown_rx, DEFAULT_POLL_INTERVAL));

    let mut assembler = VolumeAssembler::new(AssemblyConfig::default());
    let mut vcp: Option<VcpDefinition> = None;
    let mut rows: Vec<CutRow> = Vec::new();

    let result = tokio::time::timeout(OVERALL_TIMEOUT, async {
        while let Some(envelope) = chunk_rx.recv().await {
            let Ok(bytes) = radar_workstation::decompress_chunk(&envelope.raw_bytes, envelope.kind) else { continue };
            let events = if envelope.kind == radar_workstation::ChunkKind::Start {
                vcp = nexrad_decoder::parse_metadata_stream(&bytes).ok().and_then(|m| m.vcp);
                rows.clear();
                assembler.on_start_chunk(vcp.clone(), Instant::now())
            } else {
                let Ok(radials) = nexrad_decoder::parse_radial_stream(&bytes) else { continue };
                assembler.on_chunk(envelope.kind, radials, Instant::now())
            };
            for event in events {
                match event {
                    AssemblyEvent::SweepClosed { sweep, cut_deg, .. } => {
                        let commanded_deg = vcp.as_ref().and_then(|v| {
                            v.elevations.get((sweep.elevation_number as usize).wrapping_sub(1)).map(|c| c.elevation_deg)
                        });
                        let ref_range_m = grid::grid_sweep(&sweep, DisplayProduct::Reflectivity, cut_deg)
                            .map(|(g, _)| range_extent_m(&g));
                        let has_vel = sweep.radials.iter().any(|r| r.products.contains_key(&ProductKind::Vel));
                        let mut angles: Vec<f32> = sweep.radials.iter().map(|r| r.elevation_deg).collect();
                        angles.sort_by(f32::total_cmp);
                        let measured_deg = angles.get(angles.len() / 2).copied().unwrap_or(sweep.elevation_deg);
                        rows.push(CutRow {
                            elevation_number: sweep.elevation_number,
                            first_radial_deg: sweep.elevation_deg,
                            measured_deg,
                            commanded_deg,
                            cut_deg,
                            ref_range_m,
                            has_vel,
                        });
                    }
                    AssemblyEvent::VolumeClosed { volume } => {
                        println!("VolumeClosed: vcp={} status={:?} sweeps={} message5={}", volume.vcp_number, volume.status, volume.sweeps.len(), vcp.is_some());
                        if volume.status == VolumeStatus::Complete && vcp.is_some() {
                            return volume.vcp_number;
                        }
                        rows.clear();
                    }
                    _ => {}
                }
            }
        }
        0
    })
    .await;
    poll_task.abort();
    let vcp_number = result.expect("timed out waiting for a Complete volume that began with a -S chunk");

    println!("site={} vcp={vcp_number}", site.id);
    println!("el  first rad  median    commanded  cut     REF range   VEL");
    for r in &rows {
        println!(
            "{:>2}  {:>9.4}  {:>8.4}  {:>9}  {:>6.4}  {:>9}  {}",
            r.elevation_number,
            r.first_radial_deg,
            r.measured_deg,
            r.commanded_deg.map(|c| format!("{c:.4}")).unwrap_or_else(|| "—".into()),
            r.cut_deg,
            r.ref_range_m.map(|m| format!("{m} m")).unwrap_or_else(|| "—".into()),
            if r.has_vel { "yes" } else { "no" },
        );
    }

    for r in &rows {
        let commanded = r.commanded_deg.unwrap_or_else(|| panic!("el {}: no Message 5 cut at that index", r.elevation_number));
        assert!(
            (commanded - r.measured_deg).abs() <= CUT_ANGLE_TOLERANCE_DEG,
            "el {}: commanded {commanded} vs measured {} exceeds {CUT_ANGLE_TOLERANCE_DEG}°",
            r.elevation_number,
            r.measured_deg
        );
    }
    for surveillance in rows.iter().filter(|r| !r.has_vel) {
        let key = cut_key(surveillance.cut_deg);
        for doppler in rows.iter().filter(|r| r.has_vel && cut_key(r.cut_deg) == key) {
            if let (Some(s), Some(d)) = (surveillance.ref_range_m, doppler.ref_range_m) {
                assert!(
                    s >= d,
                    "split cut at {:.2}°: surveillance REF (el {}, {s} m) is shorter than Doppler REF (el {}, {d} m)",
                    surveillance.cut_deg,
                    surveillance.elevation_number,
                    doppler.elevation_number
                );
            }
        }
    }
}
