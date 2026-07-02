use std::collections::HashMap;

use crate::types::moment::{MomentData, MomentKind};
use crate::types::volume_constants::VolumeConstants;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadialStatus {
    StartOfElevation,
    Intermediate,
    EndOfElevation,
    StartOfVolume,
    EndOfVolume,
    /// SAILS supplemental low-level cut.
    StartOfElevationSails,
}

impl RadialStatus {
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::StartOfElevation),
            1 => Some(Self::Intermediate),
            2 => Some(Self::EndOfElevation),
            3 => Some(Self::StartOfVolume),
            4 => Some(Self::EndOfVolume),
            5 => Some(Self::StartOfElevationSails),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Radial {
    /// ICAO site identifier from the Message 31 header (e.g. `b"KDOX"`).
    pub site_id: [u8; 4],
    /// Milliseconds since midnight UTC, from the Message 31 header.
    pub scan_time_ms: u32,
    /// NEXRAD Julian date (days since 1970-01-01, where day 1 = 1970-01-01).
    pub julian_date: u16,
    pub azimuth_deg: f32,
    pub elevation_deg: f32,
    pub azimuth_number: u16,
    pub radial_status: RadialStatus,
    pub elevation_number: u8,
    /// Unambiguous range in km. 0.0 if the RRAD block was absent or unreadable.
    pub unamb_range_km: f32,
    /// Nyquist velocity in m/s. 0.0 if the RRAD block was absent or unreadable.
    pub nyquist_vel_ms: f32,
    /// Site and volume metadata from the RVOL block. Present when the RVOL
    /// block pointer is non-zero. In observed KDOX data the RVOL block is
    /// populated on every radial, not only on `StartOfVolume`.
    pub volume_constants: Option<VolumeConstants>,
    pub moments: HashMap<MomentKind, MomentData>,
}
