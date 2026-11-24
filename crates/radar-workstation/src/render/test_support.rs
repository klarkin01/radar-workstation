//! Shared builders for the render-side unit tests (`timeline`, `radar`,
//! `render`'s guard test): retained frames and snapshots built through the
//! library's own `Frame`/`DisplaySweep`/`StateSnapshot` types, so every test
//! module shares one definition of "a frame holding these sweeps".

use std::sync::Arc;
use std::time::Instant;

use radar_workstation::assembly::VolumeId;
use radar_workstation::compute::{DisplayProduct, SweepGrid};
use radar_workstation::state::history::{self, Frame};
use radar_workstation::state::{DisplaySweep, StateSnapshot};

pub fn volume(scan_time_ms: u32) -> VolumeId {
    VolumeId { julian_date: 20_000, scan_time_ms }
}

/// A 4×4 grid at `cut_deg` (measured angle offset slightly, as real data is).
pub fn grid(product: DisplayProduct, elevation_number: u8, cut_deg: f32) -> Arc<SweepGrid> {
    Arc::new(SweepGrid {
        product,
        azimuth_count: 4,
        gate_count: 4,
        first_gate_m: 0,
        gate_width_m: 250,
        elevation_number,
        elevation_deg: cut_deg + 0.02,
        cut_deg,
        nyquist_velocity_mps: Some(8.0),
        scale: 2.0,
        offset: 66.0,
        cells: vec![0u8; 16],
        filled_azimuths: 0,
    })
}

/// One sweep for [`frame`]: `(elevation_number, cut_deg, products)`.
pub type SweepSpec<'a> = (u8, f32, &'a [DisplayProduct]);

/// A retained frame holding exactly the sweeps in `sweeps`.
pub fn frame(scan_time_ms: u32, vcp_number: u16, sweeps: &[SweepSpec]) -> Arc<Frame> {
    let v = volume(scan_time_ms);
    let mut frame = Frame::new(v, vcp_number, Instant::now());
    for &(elevation_number, cut_deg, products) in sweeps {
        frame.insert_sweep(DisplaySweep {
            elevation_number,
            elevation_deg: cut_deg + 0.02,
            cut_deg,
            volume: v,
            vcp_number,
            received: Instant::now(),
            grids: products.iter().map(|&p| grid(p, elevation_number, cut_deg)).collect(),
        });
    }
    Arc::new(frame)
}

/// `frame` plus a derived-product set.
pub fn frame_with_derived(scan_time_ms: u32, vcp_number: u16, derived: &[DisplayProduct]) -> Arc<Frame> {
    let mut f = (*frame(scan_time_ms, vcp_number, &[])).clone();
    f.set_derived(derived.iter().map(|&p| grid(p, 0, 0.0)).collect());
    Arc::new(f)
}

/// A snapshot over `frames` (oldest → newest), with the live views folded
/// exactly as `AppState::snapshot` folds them.
pub fn snapshot(frames: Vec<Arc<Frame>>) -> StateSnapshot {
    StateSnapshot {
        site: radar_workstation::sites::by_id("KDOX").unwrap(),
        sweeps: history::live_sweeps(&frames),
        derived: history::live_derived(&frames),
        last_complete: None,
        frames,
        revision: 1,
        ingest: radar_workstation::ingest::s3_poll::IngestStatus::default(),
    }
}
