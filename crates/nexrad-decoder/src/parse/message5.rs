use crate::types::vcp::{VcpDefinition, VcpElevationCut};
use crate::DecodeError;
use super::cursor::Cursor;

// Record layout: same CTM(12) + message header(16) = 28-byte offset as
// Message 31 (see docs/architecture/nexrad-binary-format.md §15).
const BODY_OFFSET: usize = 28;
const VCP_HEADER_SIZE: usize = 22;
const EL_CUT_SIZE: usize = 46;
/// Bytes of the 46-byte elevation cut record actually decoded (elevation
/// angle + channel config + waveform + super-res flags); the remainder is
/// read past but not retained.
const EL_CUT_DECODED_SIZE: usize = 5;

/// Parse a Message 5 (Volume Coverage Pattern) record.
pub fn parse_vcp(record: &[u8]) -> Result<VcpDefinition, DecodeError> {
    if record.len() < BODY_OFFSET + VCP_HEADER_SIZE {
        return Err(DecodeError::Truncated { context: "msg5 header" });
    }
    let body = &record[BODY_OFFSET..];
    let mut c = Cursor::new(body);

    let _vcp_size_hw = c.read_u16_be()?;
    let pattern_type = c.read_u16_be()?;
    let vcp_number = c.read_u16_be()?;
    let num_el_cuts = c.read_u16_be()?;
    let _vcp_version = c.read_u8()?;
    let _clutter_map_group = c.read_u8()?;
    let _dop_res = c.read_u8()?;
    let _pulse_width = c.read_u8()?;
    let _spare1 = c.read_bytes(4)?;
    let _vcp_sequencing = c.read_u16_be()?;
    let _vcp_supplemental_info = c.read_u16_be()?;
    let _spare2 = c.read_bytes(2)?;

    let mut elevations = Vec::with_capacity(num_el_cuts as usize);
    for _ in 0..num_el_cuts {
        let el_angle_raw = c.read_u16_be()?;
        let channel_config = c.read_u8()?;
        let waveform = c.read_u8()?;
        let super_res = c.read_u8()?;
        let _rest = c.read_bytes(EL_CUT_SIZE - EL_CUT_DECODED_SIZE)?;

        elevations.push(VcpElevationCut {
            elevation_deg: el_angle_raw as f32 * 360.0 / 65536.0,
            channel_config,
            waveform,
            super_res,
        });
    }

    Ok(VcpDefinition { vcp_number, pattern_type, elevations })
}
