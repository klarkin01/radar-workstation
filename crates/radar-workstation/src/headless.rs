//! The headless run mode (`--headless`). Blocks the main thread, printing a
//! line whenever shared state changes, until stdin reaches EOF.
//!
//! **Stage 4 (S4-e, 2026-08-28):** this is no longer a placeholder. `main`
//! branches once — `if args.headless { headless::run(&state, policy) } else {
//! render::run(...) }` — and this path is the *supported* way to run the
//! pipeline where there is no display or GPU: a server, a container, CI.
//! `tests/pipeline_live.rs` depends on it. The Stage 2 note about deferring
//! signal handling to a window-close event still holds for the render path;
//! here, stdin EOF (Ctrl-D, or an immediate close when stdin isn't a live
//! terminal) remains the clean shutdown trigger.

use std::fmt::Write as _;
use std::io::BufRead;
use std::sync::mpsc;
use std::time::Duration;

use radar_workstation::state::{AppState, RetentionPolicy, StateSnapshot};

const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// This stage's whole user-visible surface (S3-W2 §5.5): per revision
/// change, one line naming every sweep held, the products present on each
/// with their grid dimensions and fill fraction, and the derived products
/// once a volume has completed — diagnosable before there is a renderer.
/// Stage 6a Part B (ADR-0030) adds `frames=<retained>/<requested>` and
/// `hist=<n>MiB` — the ring's depth against what was asked for, and its
/// total byte footprint — so the ring is diagnosable before Part C draws a
/// timeline over it.
fn format_state_line(snapshot: &StateSnapshot, policy: RetentionPolicy) -> String {
    let hist_bytes: usize = snapshot.frames.iter().map(|f| f.bytes()).sum();
    let mut line = format!(
        "[{}] rev={} frames={}/{} hist={}MiB sweeps={} derived={} last_complete_vcp={:?} ingest={:?}",
        snapshot.site.id,
        snapshot.revision,
        snapshot.frames.len(),
        policy.frames,
        hist_bytes / (1024 * 1024),
        snapshot.sweeps.len(),
        snapshot.derived.len(),
        snapshot.last_complete.map(|v| v.vcp_number),
        snapshot.ingest.state,
    );
    for sweep in &snapshot.sweeps {
        let _ = write!(line, " el={} ({:.2}°)", sweep.elevation_number, sweep.elevation_deg);
        for grid in &sweep.grids {
            let fill_pct = if grid.azimuth_count > 0 {
                grid.filled_azimuths as f32 / grid.azimuth_count as f32 * 100.0
            } else {
                0.0
            };
            let _ = write!(line, " {} {}x{} {:.0}%", grid.product, grid.azimuth_count, grid.gate_count, fill_pct);
        }
    }
    if !snapshot.derived.is_empty() {
        line.push_str(" derived:");
        for grid in &snapshot.derived {
            let _ = write!(line, " {} {}x{}", grid.product, grid.azimuth_count, grid.gate_count);
        }
    }
    line
}

/// Runs until stdin closes (Ctrl-D, or immediately if stdin isn't a live
/// terminal — e.g. piped from `/dev/null` in an automated run).
pub fn run(state: &AppState, policy: RetentionPolicy) {
    let (eof_tx, eof_rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.lock().read_line(&mut line) {
                Ok(0) => break,  // EOF
                Err(_) => break, // no usable stdin (e.g. not a terminal, already closed)
                Ok(_) => {}
            }
        }
        // The receiver may already be gone if `run` returned via some other
        // path; a failed send here just means nobody's listening anymore.
        let _ = eof_tx.send(());
    });

    println!("radar-workstation: running headless — press Ctrl-D to exit cleanly");
    let mut last_printed_revision = None;
    loop {
        match eof_rx.recv_timeout(POLL_INTERVAL) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                println!("radar-workstation: stdin closed, shutting down");
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }

        let snapshot = state.snapshot();
        if last_printed_revision != Some(snapshot.revision) {
            last_printed_revision = Some(snapshot.revision);
            println!("{}", format_state_line(&snapshot, policy));
        }
    }
}
