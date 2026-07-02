use crate::types::moment::{MomentData, MomentKind};
use super::cursor::Cursor;

/// Parse a single moment data block. Returns None if the block cannot be read
/// or if the block_id is not a known moment type.
pub fn parse_moment(body: &[u8], ptr: u32) -> Option<(MomentKind, MomentData)> {
    if ptr == 0 {
        return None;
    }
    let offset = ptr as usize;
    let mut c = Cursor::at(body, offset).ok()?;

    // Preamble: block_id(4) + block_size(2) + version(2) = 8 bytes
    let block_id = c.read_array4().ok()?;
    let kind = MomentKind::from_block_id(&block_id)?;
    let _block_size = c.read_u16_be().ok()?;
    let _version = c.read_u16_be().ok()?;

    // Moment data header (20 bytes, at block offset 8)
    let gate_count = c.read_u16_be().ok()?;
    let first_gate_m = c.read_u16_be().ok()?;
    let gate_width_m = c.read_u16_be().ok()?;
    let _tover = c.read_u16_be().ok()?;
    let _snr_threshold = c.read_u16_be().ok()?;
    let _spare = c.read_u8().ok()?;
    let word_size = c.read_u8().ok()?;
    let scale = c.read_f32_be().ok()?;
    let offset_val = c.read_f32_be().ok()?;

    if word_size != 8 && word_size != 16 {
        return None;
    }

    let byte_count = gate_count as usize * (word_size as usize / 8);
    let raw = c.read_bytes(byte_count).ok()?;

    Some((
        kind,
        MomentData {
            gate_count,
            first_gate_m,
            gate_width_m,
            word_size,
            scale,
            offset: offset_val,
            data: raw.to_vec(),
        },
    ))
}
