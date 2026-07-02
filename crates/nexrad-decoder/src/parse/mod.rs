mod blocks;
mod cursor;
mod moment;
mod radial;

use crate::{DecodeError, Radial};
use cursor::Cursor;
use radial::parse_message31;

const CTM_HEADER_SIZE: usize = 12;
const MSG_HEADER_SIZE: usize = 16;
const LEGACY_MSG_SIZE: usize = 2432;
const SIZE_HW_OFFSET: usize = CTM_HEADER_SIZE;

/// Parse all Message 31 radials from a decompressed NEXRAD chunk byte stream.
///
/// The caller must strip the 24-byte volume header before passing `data`, so
/// that `data[0]` is the first CTM header byte. Intermediate and end chunks
/// have no volume header after decompression. Start chunks do — strip those 24
/// bytes first.
///
/// Non-Message-31 records are silently skipped. Returns an error only if the
/// stream framing is irrecoverably corrupt.
pub fn parse_radial_stream(data: &[u8]) -> Result<Vec<Radial>, DecodeError> {
    let mut radials = Vec::new();
    let mut offset = 0;

    while offset + CTM_HEADER_SIZE + MSG_HEADER_SIZE <= data.len() {
        // Read size_hw and msg_type from the message header (after CTM header)
        let mut hdr = Cursor::at(data, offset + SIZE_HW_OFFSET)?;
        let size_hw = hdr.read_u16_be()?;
        let _rda_channel = hdr.read_u8()?;
        let msg_type = hdr.read_u8()?;

        if size_hw == 0 {
            offset += LEGACY_MSG_SIZE;
            continue;
        }

        let msg_size_bytes = size_hw as usize * 2;

        if msg_type == 31 {
            let record_end = offset + CTM_HEADER_SIZE + msg_size_bytes;
            let record = data
                .get(offset..record_end)
                .ok_or(DecodeError::Truncated { context: "msg31 record slice" })?;

            radials.push(parse_message31(record)?);

            // Advance to next record, aligned to 4 bytes
            let advance = (CTM_HEADER_SIZE + msg_size_bytes + 3) & !3;
            offset += advance;
        } else {
            offset += LEGACY_MSG_SIZE;
        }
    }

    Ok(radials)
}
