use crate::types::radial::Radial;

#[derive(Debug, Clone)]
pub struct Tilt {
    pub elevation_number: u8,
    pub radials: Vec<Radial>,
    /// True when an EndOfElevation (or EndOfVolume) radial status was received.
    pub complete: bool,
}
