/// Volume Coverage Pattern definition, decoded from Message 5. Only the
/// fields the compute layer will plausibly need are retained — the
/// remaining ICD-defined per-elevation fields (per-sector PRF/pulse-count
/// breakdowns, moment thresholds) are read past during parsing but not
/// kept. See `docs/architecture/nexrad-binary-format.md` §15.
#[derive(Debug, Clone)]
pub struct VcpDefinition {
    pub vcp_number: u16,
    pub pattern_type: u16,
    pub elevations: Vec<VcpElevationCut>,
}

#[derive(Debug, Clone, Copy)]
pub struct VcpElevationCut {
    /// Commanded elevation angle in degrees. This is **not** the same value
    /// as a Message 31 radial's measured `elevation_deg` — confirmed to
    /// differ by up to ~0.08° at the lowest cuts in real KDOX VCP 35 data.
    /// See `docs/architecture/nexrad-binary-format.md` §15.2; do not treat
    /// the two as interchangeable.
    pub elevation_deg: f32,
    /// 0 = Constant Phase, 1 = Random Phase, 2 = SZ2 Phase.
    pub channel_config: u8,
    /// 1 = Contiguous Surveillance, 2 = Contiguous Doppler (with ambiguity
    /// resolution), 3 = Contiguous Doppler (without), 4 = Batch, 5 =
    /// Staggered Pulse Pair.
    pub waveform: u8,
    /// Bitmask: bit0 = 0.5° azimuth / 0.25km range resolution, bit1 =
    /// Doppler to 300km, bit2 = dual-polarization control, bit3 =
    /// dual-polarization to 300km.
    pub super_res: u8,
}
