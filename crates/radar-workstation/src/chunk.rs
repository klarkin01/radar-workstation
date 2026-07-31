use std::io::Read;

use bzip2::read::BzDecoder;

const VOLUME_HEADER_LEN: usize = 24;
const BLOCK_LEN_PREFIX: usize = 4;
const START_SENTINEL: u32 = 0xFFFF_FFFF;
/// Rough BZ2 expansion factor for NEXRAD moment data, used only as a
/// `with_capacity` hint to avoid repeated reallocation during decompression.
/// Overestimating wastes a bit of memory; underestimating costs a realloc —
/// this favors the cheaper mistake.
const DECOMPRESSION_RATIO_HINT: usize = 4;
/// Upper bound on the total decompressed size of one chunk, summed across
/// every BZ2 block it contains. bzip2's worst-case expansion ratio is
/// several thousand to one, and the input is attacker-influenceable network
/// data (S3 chunk bytes) on a path whose governing principle is that the
/// application must not fall over during an event — an unbounded
/// `read_to_end` would let one hostile or corrupted chunk exhaust memory
/// instead of failing cleanly. Sized from observed data: an `-I` chunk
/// decompresses to roughly 120 radials of a few kilobytes each, so 32 MiB
/// leaves generous headroom. A single bound per chunk is sufficient since
/// chunks are decompressed one at a time.
const MAX_DECOMPRESSED_CHUNK_BYTES: u64 = 32 * 1024 * 1024;

/// Read the 4 big-endian bytes at `pos`. Callers are responsible for ensuring
/// `pos + 4 <= data.len()`.
fn read_be4(data: &[u8], pos: usize) -> [u8; 4] {
    [data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]
}

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
    /// The byte-sniffed kind disagrees with the kind the caller expected
    /// (e.g. derived from the S3 key suffix). Byte-sniffing is authoritative
    /// for parsing, but a mismatch signals a corrupt or mislabeled chunk.
    KindMismatch { expected: ChunkKind, detected: ChunkKind },
    Decompression(std::io::Error),
    /// Total decompressed size across the chunk's BZ2 block(s) exceeded
    /// `MAX_DECOMPRESSED_CHUNK_BYTES`. Guards against a decompression bomb —
    /// see that constant's doc comment.
    DecompressedTooLarge { limit: u64 },
}

impl std::fmt::Display for ChunkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => write!(f, "chunk data too short to identify"),
            Self::UnrecognizedFormat => write!(f, "unrecognized chunk format"),
            Self::KindMismatch { expected, detected } => write!(
                f,
                "chunk kind mismatch: expected {expected:?}, byte contents indicate {detected:?}"
            ),
            Self::Decompression(e) => write!(f, "BZ2 decompression error: {e}"),
            Self::DecompressedTooLarge { limit } => {
                write!(f, "decompressed chunk exceeded {limit} byte limit")
            }
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
        let prefix = i32::from_be_bytes(read_be4(data, 0));
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
/// `expected` is the kind the caller believes this chunk to be (e.g. derived
/// from the S3 key suffix). It is cross-checked against the byte-sniffed kind
/// from [`detect_chunk_kind`], which remains authoritative for how the chunk
/// is actually parsed; a disagreement returns [`ChunkError::KindMismatch`]
/// rather than silently trusting either source.
///
/// The returned bytes begin at the first CTM header and are ready for
/// [`nexrad_decoder::parse_radial_stream`]. The 24-byte volume header in
/// start chunks is stripped; the caller receives only the message stream.
pub fn decompress_chunk(data: &[u8], expected: ChunkKind) -> Result<Vec<u8>, ChunkError> {
    let detected = detect_chunk_kind(data)?;
    if detected != expected {
        return Err(ChunkError::KindMismatch { expected, detected });
    }
    match detected {
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
            let prefix = i32::from_be_bytes(read_be4(data, 0));
            let block_len = prefix.unsigned_abs() as usize;
            let compressed = data
                .get(BLOCK_LEN_PREFIX..BLOCK_LEN_PREFIX + block_len)
                .ok_or(ChunkError::TooShort)?;
            let mut out = Vec::with_capacity(compressed.len() * DECOMPRESSION_RATIO_HINT);
            let mut budget = MAX_DECOMPRESSED_CHUNK_BYTES;
            decompress_block_bounded(compressed, &mut out, &mut budget)?;
            Ok(out)
        }
    }
}

/// Decompress one BZ2 block into `out`, decrementing `budget` by the number
/// of bytes produced. Errs with [`ChunkError::DecompressedTooLarge`] rather
/// than reading past `budget`, so a hostile block cannot exhaust memory —
/// see [`MAX_DECOMPRESSED_CHUNK_BYTES`]'s doc comment. `budget` is shared and
/// threaded through by the caller so it bounds the *total* across every
/// block in a multi-block chunk, not each block individually.
fn decompress_block_bounded(compressed: &[u8], out: &mut Vec<u8>, budget: &mut u64) -> Result<(), ChunkError> {
    // Read one byte past the budget so an exact-fit stream doesn't falsely
    // trip the limit while a genuinely oversized one still does.
    let mut limited = BzDecoder::new(compressed).take(*budget + 1);
    let before = out.len();
    limited.read_to_end(out).map_err(ChunkError::Decompression)?;
    let produced = (out.len() - before) as u64;
    if produced > *budget {
        return Err(ChunkError::DecompressedTooLarge { limit: MAX_DECOMPRESSED_CHUNK_BYTES });
    }
    *budget -= produced;
    Ok(())
}

/// Walk `[4-byte length][BZ2 data]` block pairs and concatenate decompressed output.
///
/// `has_sentinel`: start chunks terminate with a `0xFFFFFFFF` length word;
/// intermediate chunks read until data is exhausted.
fn decompress_blocks(data: &[u8], has_sentinel: bool) -> Result<Vec<u8>, ChunkError> {
    let mut out = Vec::with_capacity(data.len() * DECOMPRESSION_RATIO_HINT);
    let mut offset = 0;
    let mut budget = MAX_DECOMPRESSED_CHUNK_BYTES;

    while offset + BLOCK_LEN_PREFIX <= data.len() {
        let len_word = u32::from_be_bytes(read_be4(data, offset));
        offset += BLOCK_LEN_PREFIX;

        if has_sentinel && len_word == START_SENTINEL {
            break;
        }

        let block_len = len_word as usize;
        let compressed = data
            .get(offset..offset + block_len)
            .ok_or(ChunkError::TooShort)?;
        offset += block_len;

        decompress_block_bounded(compressed, &mut out, &mut budget)?;
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

    /// A BZ2 block that decompresses to 64 MiB of zeros, generated by
    /// Python's `bz2` module — well past `MAX_DECOMPRESSED_CHUNK_BYTES`
    /// while the compressed bytes are trivially small. Proves the bound is
    /// actually enforced, not just present in the type.
    const BOMB: &[u8] = include_bytes!("../tests/fixtures/bomb_64mib_zeros.bz2");

    #[test]
    fn decompression_bomb_in_end_chunk_is_rejected() {
        let mut data = Vec::new();
        data.extend_from_slice(&(-(BOMB.len() as i32)).to_be_bytes());
        data.extend_from_slice(BOMB);

        let result = decompress_chunk(&data, ChunkKind::End);
        assert!(
            matches!(result, Err(ChunkError::DecompressedTooLarge { limit }) if limit == MAX_DECOMPRESSED_CHUNK_BYTES),
            "expected DecompressedTooLarge, got {result:?}"
        );
    }

    #[test]
    fn decompression_bomb_in_intermediate_chunk_is_rejected() {
        let mut data = Vec::new();
        data.extend_from_slice(&(BOMB.len() as u32).to_be_bytes());
        data.extend_from_slice(BOMB);

        let result = decompress_chunk(&data, ChunkKind::Intermediate);
        assert!(
            matches!(result, Err(ChunkError::DecompressedTooLarge { limit }) if limit == MAX_DECOMPRESSED_CHUNK_BYTES),
            "expected DecompressedTooLarge, got {result:?}"
        );
    }

    #[test]
    fn decompression_bomb_split_across_many_small_blocks_is_still_bounded() {
        // A -S chunk with many small blocks must not slip past the bound by
        // spreading the bomb across blocks that are each individually small
        // — the budget must be shared across the whole chunk, not per block.
        let mut data = Vec::new();
        data.extend_from_slice(&[0u8; VOLUME_HEADER_LEN]);
        for _ in 0..3 {
            data.extend_from_slice(&(BOMB.len() as u32).to_be_bytes());
            data.extend_from_slice(BOMB);
        }
        data.extend_from_slice(&START_SENTINEL.to_be_bytes());

        let result = decompress_chunk(&data, ChunkKind::Start);
        assert!(
            matches!(result, Err(ChunkError::DecompressedTooLarge { limit }) if limit == MAX_DECOMPRESSED_CHUNK_BYTES),
            "expected DecompressedTooLarge, got {result:?}"
        );
    }

    #[test]
    fn detect_unrecognized() {
        assert!(matches!(
            detect_chunk_kind(&[0u8; 8]),
            Err(ChunkError::UnrecognizedFormat)
        ));
    }
}
