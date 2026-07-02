#[derive(Debug, Clone)]
pub struct VolumeConstants {
    pub latitude: f32,
    pub longitude: f32,
    /// Site elevation above mean sea level, meters.
    pub site_amsl_m: i16,
    /// Feedhorn height above ground level, meters.
    pub feedhorn_agl_m: u16,
    pub calib_dbz: f32,
    pub txpower_h: f32,
    pub txpower_v: f32,
    pub sys_zdr: f32,
    pub phidp0: f32,
    pub vcp_number: u16,
    pub processing_status: u16,
}
