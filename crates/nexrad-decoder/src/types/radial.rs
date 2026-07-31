use crate::types::product::ProductMap;
use crate::types::site_parameters::SiteParameters;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadialStatus {
    StartOfElevation,
    Intermediate,
    EndOfElevation,
    StartOfVolume,
    EndOfVolume,
    /// SAILS supplemental low-level cut.
    SailsCut,
    /// A radial status code this build does not recognize. Newer RDA builds
    /// have added status codes (e.g. for MRLE variants); an unrecognized
    /// code is not by itself a reason to discard 120 radials of real data
    /// (FR-ND-7). The radial's geometry and moment data are decoded and kept
    /// intact; only the closure-signal meaning of the code is unknown, and
    /// callers must treat it as `Intermediate` for sweep-closure purposes —
    /// an unrecognized code is by definition not a closure signal we can act
    /// on.
    Unknown(u8),
}

impl RadialStatus {
    pub fn from_code(code: u8) -> Self {
        match code {
            0 => Self::StartOfElevation,
            1 => Self::Intermediate,
            2 => Self::EndOfElevation,
            3 => Self::StartOfVolume,
            4 => Self::EndOfVolume,
            5 => Self::SailsCut,
            other => Self::Unknown(other),
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
    /// Unambiguous range in km. `None` if the RRAD block was absent or unreadable.
    pub unambiguous_range_km: Option<f32>,
    /// Nyquist velocity in m/s. `None` if the RRAD block was absent or unreadable.
    pub nyquist_velocity_mps: Option<f32>,
    /// Site and volume metadata from the RVOL block. Present when the RVOL
    /// block pointer is non-zero. In observed KDOX data the RVOL block is
    /// populated on every radial, not only on `StartOfVolume`.
    pub site_parameters: Option<SiteParameters>,
    pub products: ProductMap,
}
