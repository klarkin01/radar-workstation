//! Live end-to-end test through the full wiring (S2-W2 §4.5): `main.rs`'s
//! actual production path — `Pipeline::spawn` — against a real site,
//! asserting `AppState` becomes non-empty within a generous deadline and
//! printing the wall-clock time to first visible sweep. `#[ignore]`d —
//! dials out to a real bucket. Run with:
//!
//!   cargo test -p radar-workstation --test pipeline_live -- --ignored --nocapture
//!
//! `tests/assembly_live.rs` already measures the poller+assembler pair in
//! isolation (1.5s, per stage-0-1's Results). This measures the same thing
//! through the full `Pipeline`/`AppState` wiring this plan adds — a direct
//! comparison against that earlier, narrower figure.

use std::sync::Arc;
use std::time::{Duration, Instant};

use radar_workstation::ingest::s3_poll::IngestStatus;
use radar_workstation::pipeline::Pipeline;
use radar_workstation::sites;
use radar_workstation::state::AppState;

const LIVE_TEST_SITE: &str = "KDOX";
const DEADLINE: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[tokio::test]
#[ignore]
async fn pipeline_spawn_produces_a_visible_sweep_within_the_deadline() {
    let site = sites::by_id(LIVE_TEST_SITE).expect("site in bundled table");
    let (status_tx, status_rx) = tokio::sync::watch::channel(IngestStatus::default());
    let state = Arc::new(AppState::new(site, status_rx));

    let start = Instant::now();
    let pipeline = Pipeline::spawn(
        &tokio::runtime::Handle::current(),
        site,
        Arc::clone(&state),
        status_tx,
        radar_workstation::ingest::s3_poll::DEFAULT_POLL_INTERVAL,
    );

    let result = tokio::time::timeout(DEADLINE, async {
        loop {
            let snapshot = state.snapshot();
            if !snapshot.sweeps.is_empty() {
                return snapshot;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await;

    let elapsed = start.elapsed();
    pipeline.shutdown().await;

    let snapshot = result.expect("no sweep became visible in AppState within the deadline");
    println!(
        "pipeline_spawn_produces_a_visible_sweep_within_the_deadline: time_to_first_sweep={elapsed:?} \
         sweeps={} ingest_state={:?}",
        snapshot.sweeps.len(),
        snapshot.ingest.state,
    );
}
