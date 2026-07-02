use std::collections::HashMap;

use crate::{DecodeError, Radial, RadialStatus};
use super::blocks::{parse_rvol, parse_rrad};
use super::cursor::Cursor;
use super::moment::parse_moment;

// Record layout (offsets from byte 0 of the raw record passed to this function):
//   [0..12]   CTM header (opaque, skipped)
//   [12..28]  Message header (16 bytes)
//   [28..]    Message 31 body
// All block pointers within the body are body-relative offsets.
const BODY_OFFSET: usize = 28;

pub fn parse_message31(record: &[u8]) -> Result<Radial, DecodeError> {
    if record.len() < BODY_OFFSET + 32 {
        return Err(DecodeError::Truncated { context: "msg31 record" });
    }
    let body = &record[BODY_OFFSET..];
    let mut c = Cursor::new(body);

    // Fixed header (32 bytes, body offset 0–31)
    let site_id = c.read_array4()?;
    let scan_time_ms = c.read_u32_be()?;
    let julian_date = c.read_u16_be()?;
    let az_num = c.read_u16_be()?;
    let az_angle = c.read_f32_be()?;
    let _compression = c.read_u8()?;
    let _spare = c.read_u8()?;
    let _radial_len = c.read_u16_be()?;
    let _az_spacing = c.read_u8()?;
    let radial_status_code = c.read_u8()?;
    let el_num = c.read_u8()?;
    let _sector_cut_num = c.read_u8()?;
    let el_angle = c.read_f32_be()?;
    let _radial_spot_blanking = c.read_u8()?;
    let _az_index_mode = c.read_u8()?;
    let num_data_blks = c.read_u16_be()?;

    let radial_status = RadialStatus::from_code(radial_status_code)
        .ok_or(DecodeError::UnsupportedMessageType { got: radial_status_code })?;

    // Block pointer table (body offset 32): vol_ptr, el_ptr, rad_ptr, then moment ptrs
    let vol_ptr = c.read_u32_be()?;
    let _el_ptr = c.read_u32_be()?;
    let rad_ptr = c.read_u32_be()?;

    let num_moment_ptrs = (num_data_blks as usize).saturating_sub(3);
    let mut moment_ptrs = Vec::with_capacity(num_moment_ptrs);
    for _ in 0..num_moment_ptrs {
        moment_ptrs.push(c.read_u32_be()?);
    }

    // Block parsing — soft failures return None and the field defaults or is absent
    let volume_constants = parse_rvol(body, vol_ptr);

    let (unamb_range_km, nyquist_vel_ms) = match parse_rrad(body, rad_ptr) {
        Some(r) => (r.unamb_range_km, r.nyquist_vel_ms),
        None => (0.0, 0.0),
    };

    let mut moments = HashMap::with_capacity(num_moment_ptrs);
    for ptr in moment_ptrs {
        if let Some((kind, data)) = parse_moment(body, ptr) {
            moments.insert(kind, data);
        }
    }

    Ok(Radial {
        site_id,
        scan_time_ms,
        julian_date,
        azimuth_deg: az_angle,
        elevation_deg: el_angle,
        azimuth_number: az_num,
        radial_status,
        elevation_number: el_num,
        unamb_range_km,
        nyquist_vel_ms,
        volume_constants,
        moments,
    })
}
