use crate::types::tilt::Tilt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeStatus {
    InProgress,
    Complete,
    /// A new start chunk arrived before the end chunk for this volume.
    ClosedByNextVolume,
    /// No chunk activity within the watchdog window.
    ClosedByTimeout,
}

#[derive(Debug, Clone)]
pub struct VolumeScan {
    pub site_id: [u8; 4],
    pub scan_time_ms: u32,
    pub julian_date: u16,
    pub vcp_number: u16,
    pub latitude: f32,
    pub longitude: f32,
    pub site_height_m: i16,
    pub tilts: Vec<Tilt>,
    pub status: VolumeStatus,
}

impl VolumeScan {
    pub fn site_id_str(&self) -> &str {
        std::str::from_utf8(&self.site_id).unwrap_or("????")
    }

    pub fn tilt(&self, elevation_number: u8) -> Option<&Tilt> {
        self.tilts.iter().find(|t| t.elevation_number == elevation_number)
    }
}
