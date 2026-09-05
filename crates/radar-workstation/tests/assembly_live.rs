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

use std::sync::Arc;
use std::time::{Duration, Instant};

use nexrad_decoder::VolumeStatus;
use radar_workstation::assembly::{self, AssemblyConfig, AssemblyEvent};
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
