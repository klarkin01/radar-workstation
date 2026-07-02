#!/usr/bin/env python3
"""
gen_fixtures.py — Generate binary test fixtures for nexrad-decoder unit tests

Scans NEXRAD Level II chunk files and extracts specific Message 31 radial
records as raw binary files. Fixtures are committed to the repo so that
nexrad-decoder tests do not depend on the chunk decompression pipeline.

Each fixture is the complete raw bytes of one Message 31 record as it appears
in the decompressed stream: 12-byte CTM header + 16-byte message header + body.
This is exactly the byte slice that the decoder will receive per message.

Usage:
    python gen_fixtures.py <chunks_dir> <output_dir>

Example:
    python gen_fixtures.py ../../downloads/KDOX_20260629_1801 \
        ../../crates/nexrad-decoder/tests/fixtures
"""

import argparse
import bz2
import io
import struct
import sys
from pathlib import Path

# ---------------------------------------------------------------------------
# ICD constants (mirrors inspect_messages.py)
# ---------------------------------------------------------------------------

VOLUME_HEADER_SIZE = 24
CTM_HEADER_SIZE    = 12
MSG_HEADER_FMT     = '>HBBHHLHH'
MSG_HEADER_LEN     = struct.calcsize(MSG_HEADER_FMT)
LEGACY_MSG_SIZE    = 2432

# Radial status codes (ICD Table XI)
STATUS_NAMES = {
    0: 'start_of_elevation',
    1: 'intermediate',
    2: 'end_of_elevation',
    3: 'start_of_volume',
    4: 'end_of_volume',
    5: 'start_of_elevation_sails',
}

# Message 31 body layout (ICD 2620002 Table V):
#   radar_id(4) + ms_since_midnight(4) + julian_date(2) + az_num(2) + az_angle(4)
#   + compression(1) + spare(1) + radial_len(2) + az_spacing(1) + radial_status(1) ...
# radial_status is at body offset 21, i.e. record offset 28 + 21 = 49.
MSG31_HDR_OFFSET    = CTM_HEADER_SIZE + 16   # = 28
RADIAL_STATUS_OFFSET = MSG31_HDR_OFFSET + 21  # byte within the full record


# ---------------------------------------------------------------------------
# Decompression (mirrors inspect_messages.py)
# ---------------------------------------------------------------------------

def decompress_inter(data: bytes) -> bytes:
    """Decompress an intermediate (-I) chunk. Returns flat decompressed stream."""
    out = io.BytesIO()
    offset = 0
    while offset < len(data):
        if offset + 4 > len(data):
            break
        block_len = struct.unpack('>I', data[offset:offset + 4])[0]
        offset += 4
        if block_len == 0xFFFFFFFF:
            break
        if offset + block_len > len(data):
            break
        out.write(bz2.decompress(data[offset:offset + block_len]))
        offset += block_len
    return out.getvalue()


def decompress_start(data: bytes) -> bytes:
    """Decompress a start (-S) chunk. Preserves the 24-byte volume header."""
    out = io.BytesIO()
    out.write(data[:VOLUME_HEADER_SIZE])
    offset = VOLUME_HEADER_SIZE
    while offset < len(data):
        if offset + 4 > len(data):
            break
        block_len = struct.unpack('>I', data[offset:offset + 4])[0]
        offset += 4
        if block_len == 0xFFFFFFFF:
            break
        if offset + block_len > len(data):
            break
        out.write(bz2.decompress(data[offset:offset + block_len]))
        offset += block_len
    return out.getvalue()


def decompress_end(data: bytes) -> bytes:
    """Decompress an end (-E) chunk. Length prefix is signed negative."""
    length = abs(struct.unpack('>i', data[:4])[0])
    return bz2.decompress(data[4:4 + length])


def decompress_chunk(path: Path) -> bytes | None:
    """Auto-detect chunk type and return decompressed byte stream."""
    raw = path.read_bytes()
    if len(raw) < 8:
        return None

    if raw[:4] == b'AR2V' or raw[:8] == b'ARCHIVE2':
        stream = decompress_start(raw)
    elif struct.unpack('>i', raw[:4])[0] < 0 and raw[4:8] == b'BZh9':
        stream = decompress_end(raw)
        stream = bytes(VOLUME_HEADER_SIZE) + stream
    elif raw[4:8] == b'BZh9':
        stream = decompress_inter(raw)
        stream = bytes(VOLUME_HEADER_SIZE) + stream
    elif raw[28:32] == b'BZh9':
        stream = decompress_start(raw)
    else:
        return None
    return stream


# ---------------------------------------------------------------------------
# Message walker (mirrors inspect_messages.py)
# ---------------------------------------------------------------------------

def iter_msg31(stream: bytes):
    """
    Yield (raw_bytes, radial_status) for every Message 31 in the decompressed stream.
    raw_bytes is the complete record: CTM header + message header + body.
    """
    offset = VOLUME_HEADER_SIZE
    while offset < len(stream):
        msg_start = offset + CTM_HEADER_SIZE
        if msg_start + MSG_HEADER_LEN > len(stream):
            break

        raw_hdr = stream[msg_start:msg_start + MSG_HEADER_LEN]
        try:
            (size_hw, _, msg_type, _, _, _, _, _) = struct.unpack(MSG_HEADER_FMT, raw_hdr)
        except struct.error:
            break

        if size_hw == 0:
            offset += LEGACY_MSG_SIZE
            continue

        msg_size_bytes = size_hw * 2
        record = stream[offset:offset + CTM_HEADER_SIZE + msg_size_bytes]

        if msg_type == 31 and len(record) >= RADIAL_STATUS_OFFSET + 1:
            radial_status = record[RADIAL_STATUS_OFFSET]
            yield record, radial_status

        if msg_type == 31:
            advance = CTM_HEADER_SIZE + msg_size_bytes
            advance = (advance + 3) & ~3
        else:
            advance = LEGACY_MSG_SIZE

        offset += advance


# ---------------------------------------------------------------------------
# Fixture selection
# ---------------------------------------------------------------------------

# Target statuses and the descriptive suffix for the output filename.
# We want exactly one file per status code present in the data.
TARGET_STATUSES = {
    3: 'start_of_volume',
    0: 'start_of_elevation',
    1: 'intermediate',
    2: 'end_of_elevation',
    4: 'end_of_volume',
    5: 'start_of_elevation_sails',
}


def collect_fixtures(chunks_dir: Path) -> dict[int, bytes]:
    """
    Scan chunk files in order, collecting the first Message 31 of each
    target radial status. Returns {status_code: raw_bytes}.
    """
    collected: dict[int, bytes] = {}
    remaining = set(TARGET_STATUSES.keys())

    chunk_files = sorted(chunks_dir.iterdir())
    print(f"Scanning {len(chunk_files)} chunk files in {chunks_dir} ...")

    for path in chunk_files:
        if not remaining:
            break
        stream = decompress_chunk(path)
        if stream is None:
            print(f"  skip (unknown format): {path.name}")
            continue

        for record, status in iter_msg31(stream):
            if status in remaining:
                collected[status] = record
                remaining.discard(status)
                name = TARGET_STATUSES.get(status, str(status))
                print(f"  captured status={status} ({name})  {len(record):,} bytes  from {path.name}")
            if not remaining:
                break

    if remaining:
        missing = [TARGET_STATUSES.get(s, str(s)) for s in sorted(remaining)]
        print(f"  Note: no radials found for: {', '.join(missing)}")

    return collected


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('chunks_dir', help='Directory containing NEXRAD chunk files')
    parser.add_argument('output_dir', help='Output directory for fixture .bin files')
    parser.add_argument('--prefix', default='kdox_vcp35',
                        help='Filename prefix (default: kdox_vcp35)')
    parser.add_argument('--dry-run', action='store_true',
                        help='Print what would be written without writing')
    args = parser.parse_args()

    chunks_dir = Path(args.chunks_dir)
    output_dir = Path(args.output_dir)

    if not chunks_dir.is_dir():
        print(f"Error: chunks directory not found: {chunks_dir}")
        sys.exit(1)

    output_dir.mkdir(parents=True, exist_ok=True)

    fixtures = collect_fixtures(chunks_dir)

    print(f"\nWriting {len(fixtures)} fixture files to {output_dir} ...")
    for status, record in sorted(fixtures.items()):
        name = TARGET_STATUSES.get(status, f'status_{status}')
        filename = f'{args.prefix}_{name}.bin'
        out_path = output_dir / filename
        if args.dry_run:
            print(f"  [dry-run] would write {len(record):,} bytes -> {filename}")
        else:
            out_path.write_bytes(record)
            print(f"  wrote {len(record):,} bytes -> {filename}")

    print("\nDone.")


if __name__ == '__main__':
    main()
