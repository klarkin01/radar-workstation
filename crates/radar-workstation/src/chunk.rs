use std::io::Read;

use bzip2::read::BzDecoder;

const VOLUME_HEADER_LEN: usize = 24;
const BLOCK_LEN_PREFIX: usize = 4;
const START_SENTINEL: u32 = 0xFFFF_FFFF;

/// Which type of NEXRAD real-time chunk this is.
///
/// Detection is based on the first 8 raw bytes before decompression
/// (see `docs/architecture/nexrad-binary-format.md` §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkKind {
    /// `-S` suffix. 24-byte volume header + compressed blocks.
    /// Contains metadata messages only (VCP, RDA status, etc.); no radials.
    Start,
    /// `-I` suffix. One or more compressed blocks; no volume header.
    /// Contains 120 Message 31 radials.
    Intermediate,
    /// `-E` suffix. Single compressed block with a negative length prefix.
    /// Contains the final 120 Message 31 radials of the volume.
    End,
}

#[derive(Debug)]
pub enum ChunkError {
    TooShort,
    UnrecognizedFormat,
    Decompression(std::io::Error),
}

impl std::fmt::Display for ChunkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => write!(f, "chunk data too short to identify"),
            Self::UnrecognizedFormat => write!(f, "unrecognized chunk format"),
            Self::Decompression(e) => write!(f, "BZ2 decompression error: {e}"),
        }
    }
}

impl std::error::Error for ChunkError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if let Self::Decompression(e) = self {
            Some(e)
        } else {
            None
        }
    }
}

/// Identify the chunk type from raw (pre-decompression) bytes.
///
/// End chunks must be tested before Intermediate: both carry `BZh9` at offset 4,
/// but End chunks have a negative signed `i32` at offset 0.
pub fn detect_chunk_kind(data: &[u8]) -> Result<ChunkKind, ChunkError> {
    if data.len() < 8 {
        return Err(ChunkError::TooShort);
    }
    // Primary start detection: volume header magic "AR2V"
    if &data[0..4] == b"AR2V" {
        return Ok(ChunkKind::Start);
    }
    // End before Intermediate: both have BZh9 at offset 4
    if &data[4..8] == b"BZh9" {
        let prefix = i32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        return Ok(if prefix < 0 { ChunkKind::End } else { ChunkKind::Intermediate });
    }
    // Alternate start detection: BZh9 at volume_header(24) + length_prefix(4) = offset 28
    if data.len() >= 32 && &data[28..32] == b"BZh9" {
        return Ok(ChunkKind::Start);
    }
    Err(ChunkError::UnrecognizedFormat)
}

/// Decompress a raw chunk into a flat NEXRAD message stream.
///
/// The returned bytes begin at the first CTM header and are ready for
/// [`nexrad_decoder::parse_radial_stream`]. The 24-byte volume header in
/// start chunks is stripped; the caller receives only the message stream.
pub fn decompress_chunk(data: &[u8]) -> Result<Vec<u8>, ChunkError> {
    match detect_chunk_kind(data)? {
        ChunkKind::Start => {
            if data.len() < VOLUME_HEADER_LEN + BLOCK_LEN_PREFIX {
                return Err(ChunkError::TooShort);
            }
            decompress_blocks(&data[VOLUME_HEADER_LEN..], true)
        }
        ChunkKind::Intermediate => decompress_blocks(data, false),
        ChunkKind::End => {
            if data.len() < BLOCK_LEN_PREFIX {
                return Err(ChunkError::TooShort);
            }
            let prefix = i32::from_be_bytes([data[0], data[1], data[2], data[3]]);
            let block_len = prefix.unsigned_abs() as usize;
            let compressed = data
                .get(BLOCK_LEN_PREFIX..BLOCK_LEN_PREFIX + block_len)
                .ok_or(ChunkError::TooShort)?;
            let mut out = Vec::new();
            BzDecoder::new(compressed)
                .read_to_end(&mut out)
                .map_err(ChunkError::Decompression)?;
            Ok(out)
        }
    }
}

/// Walk `[4-byte length][BZ2 data]` block pairs and concatenate decompressed output.
///
/// `has_sentinel`: start chunks terminate with a `0xFFFFFFFF` length word;
/// intermediate chunks read until data is exhausted.
fn decompress_blocks(data: &[u8], has_sentinel: bool) -> Result<Vec<u8>, ChunkError> {
    let mut out = Vec::new();
    let mut offset = 0;

    while offset + BLOCK_LEN_PREFIX <= data.len() {
        let len_word = u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);
        offset += BLOCK_LEN_PREFIX;

        if has_sentinel && len_word == START_SENTINEL {
            break;
        }

        let block_len = len_word as usize;
        let compressed = data
            .get(offset..offset + block_len)
            .ok_or(ChunkError::TooShort)?;
        offset += block_len;

        BzDecoder::new(compressed)
            .read_to_end(&mut out)
            .map_err(ChunkError::Decompression)?;
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bytes(prefix: [u8; 4], magic_at_4: Option<&[u8; 4]>) -> Vec<u8> {
        let mut v = vec![0u8; 32];
        v[0..4].copy_from_slice(&prefix);
        if let Some(m) = magic_at_4 {
            v[4..8].copy_from_slice(m);
        }
        v
    }

    #[test]
    fn detect_start_by_ar2v() {
        let data = make_bytes(*b"AR2V", None);
        assert_eq!(detect_chunk_kind(&data).unwrap(), ChunkKind::Start);
    }

    #[test]
    fn detect_end_by_negative_prefix() {
        let data = make_bytes((-1000i32).to_be_bytes(), Some(b"BZh9"));
        assert_eq!(detect_chunk_kind(&data).unwrap(), ChunkKind::End);
    }

    #[test]
    fn detect_intermediate_by_positive_prefix() {
        let data = make_bytes(1000u32.to_be_bytes(), Some(b"BZh9"));
        assert_eq!(detect_chunk_kind(&data).unwrap(), ChunkKind::Intermediate);
    }

    #[test]
    fn detect_too_short() {
        assert!(matches!(detect_chunk_kind(&[0u8; 4]), Err(ChunkError::TooShort)));
    }

    #[test]
    fn detect_unrecognized() {
        assert!(matches!(
            detect_chunk_kind(&[0u8; 8]),
            Err(ChunkError::UnrecognizedFormat)
        ));
    }
}
