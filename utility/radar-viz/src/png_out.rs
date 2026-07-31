//! Minimal, dependency-free PNG encoder for `radar-viz`'s PPI output.
//!
//! Replaces the `image` crate (which pulled `moxcms`, `pxfm`, `png`, `flate2`,
//! and half a dozen more transitive crates for one call site: an RGBA
//! `ImageBuffer` and `.save()`). `radar-viz` is a dev tool, so the value here
//! isn't the tool — it's that the workspace doesn't end up owning a raster
//! encoder via a path (`image`) that was never meant to reach `crates/`.
//!
//! Encoding uses stored (uncompressed) deflate blocks, so files are larger
//! than a real PNG encoder would produce — fine for a developer utility that
//! writes one file per invocation. See
//! `docs/plans/dependency-inventory-remediation.md` W6 for the design.

use std::io;
use std::path::Path;

/// RGBA8 raster image, row-major, top row first.
pub struct Raster {
    width: u32,
    height: u32,
    px: Vec<u8>,
}

impl Raster {
    pub fn filled(width: u32, height: u32, color: [u8; 4]) -> Self {
        let mut px = Vec::with_capacity(width as usize * height as usize * 4);
        for _ in 0..(width as usize * height as usize) {
            px.extend_from_slice(&color);
        }
        Self { width, height, px }
    }

    pub fn put_pixel(&mut self, x: u32, y: u32, color: [u8; 4]) {
        let idx = (y as usize * self.width as usize + x as usize) * 4;
        self.px[idx..idx + 4].copy_from_slice(&color);
    }
}

pub fn write_png(raster: &Raster, path: &Path) -> io::Result<()> {
    std::fs::write(path, encode_png(raster))
}

fn encode_png(raster: &Raster) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&raster.width.to_be_bytes());
    ihdr.extend_from_slice(&raster.height.to_be_bytes());
    // bit depth 8, color type 6 (RGBA), compression 0, filter 0, interlace 0
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    write_chunk(&mut out, b"IHDR", &ihdr);

    write_chunk(&mut out, b"IDAT", &zlib_stored(&scanlines(raster)));
    write_chunk(&mut out, b"IEND", &[]);
    out
}

/// Per scanline: a filter-type byte of 0 (None), followed by `width * 4`
/// raw RGBA bytes. No filtering is applied — this is what "None" means.
fn scanlines(raster: &Raster) -> Vec<u8> {
    let row_bytes = raster.width as usize * 4;
    let mut data = Vec::with_capacity((row_bytes + 1) * raster.height as usize);
    for row in 0..raster.height as usize {
        data.push(0);
        let start = row * row_bytes;
        data.extend_from_slice(&raster.px[start..start + row_bytes]);
    }
    data
}

/// Wraps `data` in a minimal zlib stream (RFC 1950) using uncompressed
/// ("stored", RFC 1951 §3.2.4) deflate blocks — no Huffman coding, so this
/// is not a real compressor, just a valid container. Blocks are capped at
/// 65535 bytes, the format's maximum for a stored block's LEN field.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    const MAX_BLOCK: usize = 65535;

    let mut out = Vec::with_capacity(data.len() + (data.len() / MAX_BLOCK + 1) * 5 + 6);
    out.extend_from_slice(&[0x78, 0x01]); // CMF/FLG: deflate, 32K window, fastest

    let mut offset = 0;
    loop {
        let end = (offset + MAX_BLOCK).min(data.len());
        let is_final = end == data.len();
        let len = (end - offset) as u16;
        out.push(if is_final { 0x01 } else { 0x00 }); // BFINAL | BTYPE(00)=stored
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes()); // NLEN = one's complement of LEN
        out.extend_from_slice(&data[offset..end]);
        offset = end;
        if is_final {
            break;
        }
    }

    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn write_chunk(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(chunk_type);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&out[start..]).to_be_bytes());
}

fn adler32(data: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % MOD_ADLER;
        b = (b + a) % MOD_ADLER;
    }
    (b << 16) | a
}

/// CRC-32 (polynomial 0xEDB88320, the PNG/zlib/gzip variant), computed
/// bit-by-bit — no lookup table, per the design in W6.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independently computed (Python + `zlib`, cross-checked against
    /// `zlib.decompress` for round-trip validity) for a 2×2 raster of known
    /// colors: (0,0)=red, (1,0)=green, (0,1)=blue, (1,1)=semi-transparent
    /// white. Reviewed by hand against the PNG chunk structure below.
    #[test]
    fn golden_bytes_2x2() {
        let mut raster = Raster::filled(2, 2, [255, 0, 0, 255]);
        raster.put_pixel(1, 0, [0, 255, 0, 255]);
        raster.put_pixel(0, 1, [0, 0, 255, 255]);
        raster.put_pixel(1, 1, [255, 255, 255, 128]);

        let bytes = encode_png(&raster);
        let expected = hex_literal(
            "89504e470d0a1a0a0000000d494844520000000200000002080600000072b60d24\
             0000001d494441547801011200edff00ff0000ff00ff00ff000000ffffffffff80\
             494909784bd9ce030000000049454e44ae426082",
        );
        assert_eq!(bytes, expected);
    }

    fn hex_literal(s: &str) -> Vec<u8> {
        let clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        (0..clean.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn scanline_length_matches_invariant() {
        let raster = Raster::filled(5, 3, [1, 2, 3, 4]);
        assert_eq!(scanlines(&raster).len(), 3 * (1 + 5 * 4));
    }

    #[test]
    fn stored_block_count_matches_invariant() {
        // 3 bytes header + LEN + NLEN per block, then the 4-byte Adler-32
        // trailer; verify against a payload spanning multiple 65535-byte
        // stored blocks.
        let data = vec![0u8; 65535 * 2 + 10];
        let expected_blocks = (data.len() as f64 / 65535.0).ceil() as usize;
        let zlib = zlib_stored(&data);
        // 2-byte zlib header + expected_blocks * 5-byte block headers +
        // payload + 4-byte Adler-32 trailer.
        assert_eq!(zlib.len(), 2 + expected_blocks * 5 + data.len() + 4);
    }

    #[test]
    fn crc32_matches_published_test_vector() {
        // The canonical CRC-32 test vector: CRC32(b"123456789") = 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn write_png_round_trips_through_std_fs() {
        let dir = std::env::temp_dir().join(format!("radar-viz-png-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.png");

        let raster = Raster::filled(1, 1, [9, 8, 7, 6]);
        write_png(&raster, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[0..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

        std::fs::remove_dir_all(&dir).ok();
    }
}
