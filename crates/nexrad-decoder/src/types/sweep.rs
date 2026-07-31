use crate::types::radial::Radial;

#[derive(Debug, Clone)]
pub struct Sweep {
    pub elevation_number: u8,
    /// Elevation angle in degrees. Hoisted from the first radial of the
    /// sweep, not independently decoded — elevation angle is a per-sweep
    /// constant in practice.
    pub elevation_deg: f32,
    /// Nyquist velocity in m/s, hoisted from the first radial's RRAD block.
    /// `None` if that radial's RRAD block was absent or unreadable.
    pub nyquist_velocity_mps: Option<f32>,
    /// Unambiguous range in km, hoisted from the first radial's RRAD block.
    /// `None` if that radial's RRAD block was absent or unreadable.
    pub unambiguous_range_km: Option<f32>,
    pub radials: Vec<Radial>,
    /// True when an EndOfElevation (or EndOfVolume) radial status was received.
    pub complete: bool,
}
