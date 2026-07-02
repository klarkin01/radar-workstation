use crate::types::volume_constants::VolumeConstants;
use super::cursor::Cursor;

// All block parsers receive the full body slice and the pointer value from the
// block pointer table. A pointer of 0 means the block is absent. Soft failures
// (wrong block_id, truncated block) return None rather than propagating an error,
// since a radial with a missing optional block is still useful.

/// Parse the RVOL (Volume Constants) block. Returns None if the pointer is 0
/// or if the block cannot be read.
pub fn parse_rvol(body: &[u8], ptr: u32) -> Option<VolumeConstants> {
    if ptr == 0 {
        return None;
    }
    let offset = ptr as usize;
    let mut c = Cursor::at(body, offset).ok()?;

    // Preamble: block_id(4) + block_size(2) = 6 bytes
    let block_id = c.read_array4().ok()?;
    if &block_id != b"RVOL" {
        return None;
    }
    let _block_size = c.read_u16_be().ok()?;

    // Data fields (confirmed offsets — see docs/architecture/nexrad-binary-format.md §8)
    let _major = c.read_u8().ok()?;
    let _minor = c.read_u8().ok()?;
    let latitude = c.read_f32_be().ok()?;
    let longitude = c.read_f32_be().ok()?;
    let site_amsl_m = c.read_i16_be().ok()?;
    let feedhorn_agl_m = c.read_u16_be().ok()?;
    let calib_dbz = c.read_f32_be().ok()?;
    let txpower_h = c.read_f32_be().ok()?;
    let txpower_v = c.read_f32_be().ok()?;
    let sys_zdr = c.read_f32_be().ok()?;
    let phidp0 = c.read_f32_be().ok()?;
    let vcp_number = c.read_u16_be().ok()?;
    let processing_status = c.read_u16_be().ok()?;

    Some(VolumeConstants {
        latitude,
        longitude,
        site_amsl_m,
        feedhorn_agl_m,
        calib_dbz,
        txpower_h,
        txpower_v,
        sys_zdr,
        phidp0,
        vcp_number,
        processing_status,
    })
}

/// Radial constants extracted from the RRAD block.
pub struct RradConstants {
    pub unamb_range_km: f32,
    pub nyquist_vel_ms: f32,
}

/// Parse the RRAD (Radial Constants) block. Returns None if the pointer is 0
/// or if the block cannot be read.
pub fn parse_rrad(body: &[u8], ptr: u32) -> Option<RradConstants> {
    if ptr == 0 {
        return None;
    }
    let offset = ptr as usize;
    let mut c = Cursor::at(body, offset).ok()?;

    // Preamble: block_id(4) + block_size(2) = 6 bytes
    let block_id = c.read_array4().ok()?;
    if &block_id != b"RRAD" {
        return None;
    }
    let block_size = c.read_u16_be().ok()?;

    // Data fields common to v1 and v2
    let unamb_range_raw = c.read_u16_be().ok()?;
    let _horiz_noise = c.read_f32_be().ok()?;
    let _vert_noise = c.read_f32_be().ok()?;
    let nyquist_raw = c.read_u16_be().ok()?;

    // Unit conversion (ICD)
    let unamb_range_km = unamb_range_raw as f32 / 8.0;
    let nyquist_vel_ms = nyquist_raw as f32 / 100.0;

    // v2 detection: block_size >= 32 means radial_flags + spare2 are present
    // between spare and calib_const_h. We don't use calib constants in Phase 2,
    // so we don't need to parse further.
    let _ = block_size;

    Some(RradConstants { unamb_range_km, nyquist_vel_ms })
}
