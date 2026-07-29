use crate::types::product::{ProductData, ProductKind};
use super::cursor::Cursor;

/// Parse a single product data block. Returns None if the block cannot be read
/// or if the block_id is not a known product type.
pub fn parse_product(body: &[u8], ptr: u32) -> Option<(ProductKind, ProductData)> {
    let (mut c, block_id) = Cursor::for_block(body, ptr)?;
    let kind = ProductKind::from_block_id(&block_id)?;
    // Preamble: block_id(4) + block_size(2) + version(2) = 8 bytes
    let _block_size = c.read_u16_be().ok()?;
    let _version = c.read_u16_be().ok()?;

    // Product data header (20 bytes, at block offset 8)
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
        ProductData {
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
