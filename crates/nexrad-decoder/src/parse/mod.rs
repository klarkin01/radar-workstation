mod blocks;
mod cursor;
mod message5;
mod product;
mod radial;

use crate::types::vcp::VcpDefinition;
use crate::{DecodeError, Radial};
use cursor::Cursor;
use message5::parse_vcp;
use radial::parse_message31;

const CTM_HEADER_SIZE: usize = 12;
const MSG_HEADER_SIZE: usize = 16;
const LEGACY_MSG_SIZE: usize = 2432;
const SIZE_HW_OFFSET: usize = CTM_HEADER_SIZE;
/// Chunks carry 120 Message 31 radials in observed data; used only as a
/// `with_capacity` hint to avoid reallocation growth, not a hard limit.
const TYPICAL_RADIALS_PER_CHUNK: usize = 120;

/// The fields of the 16-byte message header (§5 of the format doc) that
/// every caller of [`for_each_message`] needs to decide what to do with a
/// record. Framing fields (`seq_num`, `julian_date`, `time_ms`,
/// `num_segments`, `segment_num`) are not exposed because no caller needs
/// them yet — `for_each_message` itself is where segment handling would go
/// if a message in scope ever needed it (see §15 of the format doc: not
/// needed for Message 5 at any defined VCP's elevation count).
pub(crate) struct MessageHeader {
    pub msg_type: u8,
}

/// Walks the flat NEXRAD message stream shared by every chunk kind's
/// decompressed output, calling `f` with each record's message header and
/// the full record slice (CTM header + message header + body) — the exact
/// byte slice `parse_message31` and `parse_vcp` each expect.
///
/// This is the framing logic that must not exist in more than once place
/// (the DRY instruction) since it is also where the hostile-input risk
/// lives: legacy-size skipping, the size-halfwords convention, and Message
/// 31's variable-length 4-byte-aligned advance. `f` returning `Err` stops
/// the walk immediately and propagates the error, matching
/// `parse_radial_stream`'s existing contract of failing on a truncated
/// Message 31 record rather than skipping past it.
fn for_each_message<'a>(
    data: &'a [u8],
    mut f: impl FnMut(MessageHeader, &'a [u8]) -> Result<(), DecodeError>,
) -> Result<(), DecodeError> {
    let mut offset = 0;

    while offset + CTM_HEADER_SIZE + MSG_HEADER_SIZE <= data.len() {
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

            f(MessageHeader { msg_type }, record)?;

            // Advance to next record, aligned to 4 bytes
            let advance = (CTM_HEADER_SIZE + msg_size_bytes + 3) & !3;
            offset += advance;
        } else {
            let record_end = (offset + LEGACY_MSG_SIZE).min(data.len());
            let record = &data[offset..record_end];

            f(MessageHeader { msg_type }, record)?;

            offset += LEGACY_MSG_SIZE;
        }
    }

    Ok(())
}

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
    let mut radials = Vec::with_capacity(TYPICAL_RADIALS_PER_CHUNK);

    for_each_message(data, |hdr, record| {
        if hdr.msg_type == 31 {
            radials.push(parse_message31(record)?);
        }
        Ok(())
    })?;

    Ok(radials)
}

/// Volume-level metadata decoded from a `-S` chunk's message stream (same
/// framing walk as `parse_radial_stream`, disjoint content: `-S` chunks
/// carry no Message 31 data in this codebase's decoder, so the two entry
/// points are never both non-empty for the same chunk).
#[derive(Debug, Default)]
pub struct VolumeMetadata {
    pub vcp: Option<VcpDefinition>,
}

/// Parse volume-level metadata (currently: Message 5, the VCP definition)
/// from a decompressed `-S` chunk byte stream. Like `parse_radial_stream`,
/// the caller must strip the 24-byte volume header first.
pub fn parse_metadata_stream(data: &[u8]) -> Result<VolumeMetadata, DecodeError> {
    let mut metadata = VolumeMetadata::default();

    for_each_message(data, |hdr, record| {
        if hdr.msg_type == 5 {
            metadata.vcp = Some(parse_vcp(record)?);
        }
        Ok(())
    })?;

    Ok(metadata)
}
