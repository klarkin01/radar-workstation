use std::sync::Arc;

use crate::types::sweep::Sweep;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeStatus {
    InProgress,
    Complete,
    /// A new start chunk arrived before the end chunk for this volume.
    Superseded,
    /// No chunk activity within the watchdog window.
    TimedOut,
}

#[derive(Debug, Clone)]
pub struct VolumeScan {
    pub site_id: [u8; 4],
    pub scan_time_ms: u32,
    pub julian_date: u16,
    pub vcp_number: u16,
    pub latitude: f32,
    pub longitude: f32,
    pub site_amsl_m: i16,
    /// `Arc` because each sweep is handed to the compute layer at closure
    /// (ADR-0012) *and* remains in this closed `VolumeScan` — sweep closure
    /// is permanent and the data is immutable after hand-off, so a shared
    /// reference avoids copying a multi-megabyte sweep on every hand-off.
    pub sweeps: Vec<Arc<Sweep>>,
    pub status: VolumeStatus,
}

impl VolumeScan {
    pub fn site_id_str(&self) -> &str {
        std::str::from_utf8(&self.site_id).unwrap_or("????")
    }

    pub fn sweep(&self, elevation_number: u8) -> Option<&Arc<Sweep>> {
        self.sweeps.iter().find(|s| s.elevation_number == elevation_number)
    }
}
