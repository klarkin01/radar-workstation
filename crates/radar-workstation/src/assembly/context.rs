use nexrad_decoder::{Radial, VcpDefinition};

/// Volume-level metadata: the decoded VCP definition (Message 5, S1-W2) and
/// site parameters. Lives here, not in `nexrad-decoder` — the decoder
/// parses formats, the assembly layer holds session state.
///
/// Populated from whichever source provides it: primarily a `-S` chunk's
/// Message 5, falling back to the RVOL block on whichever radial happens to
/// carry one (ADR-0012's missing-`-S` fallback — this must work whether or
/// not a `-S` chunk was ever seen, so it does not assume any arbitrary
/// radial carries an RVOL block; it just takes the value if present).
/// Carried-forward statics from Messages 15/18 (clutter filter map, RDA
/// adaptation data) are out of Stage 1 scope — see `TESTING.md`.
#[derive(Default)]
pub(crate) struct VolumeContext {
    pub site_id: [u8; 4],
    pub scan_time_ms: u32,
    pub julian_date: u16,
    pub vcp: Option<VcpDefinition>,
    /// VCP number from a radial's RVOL block. Used only when `vcp` (from
    /// Message 5) is unavailable — see `vcp_number`.
    rvol_vcp_number: Option<u16>,
    pub latitude: f32,
    pub longitude: f32,
    pub site_amsl_m: i16,
}

impl VolumeContext {
    /// The VCP number to report on the closed `VolumeScan`: from the
    /// decoded Message 5 if one arrived, else the RVOL-block fallback, else
    /// 0 (neither source has been seen yet).
    pub fn vcp_number(&self) -> u16 {
        self.vcp.as_ref().map(|v| v.vcp_number).or(self.rvol_vcp_number).unwrap_or(0)
    }

    pub fn apply_vcp(&mut self, vcp: VcpDefinition) {
        self.vcp = Some(vcp);
    }

    /// `is_first` captures the per-radial timestamp/site-id fields from the
    /// very first radial of the volume only; RVOL-derived fields are taken
    /// from whichever radial carries them, since that isn't guaranteed to
    /// be the first one.
    pub fn apply_radial(&mut self, radial: &Radial, is_first: bool) {
        if is_first {
            self.site_id = radial.site_id;
            self.scan_time_ms = radial.scan_time_ms;
            self.julian_date = radial.julian_date;
        }
        if let Some(sp) = &radial.site_parameters {
            self.rvol_vcp_number = Some(sp.vcp_number);
            self.latitude = sp.latitude;
            self.longitude = sp.longitude;
            self.site_amsl_m = sp.site_amsl_m;
        }
    }
}
