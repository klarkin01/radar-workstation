use crate::types::product::ProductMap;
use crate::types::site_parameters::SiteParameters;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadialStatus {
    StartOfElevation,
    Intermediate,
    EndOfElevation,
    StartOfVolume,
    EndOfVolume,
    /// Start of the volume's last elevation. Confirmed against real KTLH
    /// VCP 212 data (2026-07-31) and MetPy's `remap_status`
    /// (`START_ELEVATION | LAST_ELEVATION`): code 5 appeared on the single
    /// highest-elevation cut of a 16-elevation volume, not on a repeated
    /// low-level SAILS/MRLE cut. This corrects the repo's earlier
    /// assumption (both here and in `nexrad-binary-format.md` §6.1) that
    /// code 5 meant "SAILS supplemental low-level cut" — the same volume
    /// showed SAILS/MRLE-inserted low-level cuts (repeated elevation angle)
    /// carrying ordinary code 0 (`StartOfElevation`) with a new, incrementing
    /// `elevation_number`, never a repeated one. See S1-W4d in
    /// `docs/plans/stage-0-1-close-the-acquisition-path.md`.
    StartOfLastElevation,
    /// A radial status code this build does not recognize. Newer RDA builds
    /// have added status codes; an unrecognized code is not by itself a
    /// reason to discard 120 radials of real data (FR-ND-7). The radial's
    /// geometry and moment data are decoded and kept intact; only the
    /// closure-signal meaning of the code is unknown, and callers must
    /// treat it as `Intermediate` for sweep-closure purposes — an
    /// unrecognized code is by definition not a closure signal we can act
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
            5 => Self::StartOfLastElevation,
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
    /// Raw `az_spacing` code: 1 = 0.5° (super-resolution), 2 = 1.0°
    /// (standard resolution). See `docs/architecture/nexrad-binary-format.md` §6.1.
    pub azimuth_spacing_code: u8,
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
