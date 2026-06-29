#!/usr/bin/env python3
"""
inspect_messages.py — Raw NEXRAD Level II message inspector
Radar Workstation, Meteorological — development utility

Walks the raw binary structure of a Level II file and reports every
message found, including message type, sequence number, date, time,
size, and segment info. Operates independently of MetPy — parses the
ICD binary structure directly.

Handles both file formats:
  - Uncompressed Message-31 volume files (e.g. KABR20260626_000248_V06)
  - Internally BZ2-compressed chunk files (e.g. 20260626-102243-002-I)

Format is auto-detected from magic bytes.

Usage:
    python inspect_messages.py <file>
    python inspect_messages.py <file> --limit 50
    python inspect_messages.py <file> --type 31
    python inspect_messages.py <file> --summary
"""

import argparse
import bz2
import io
import struct
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

# Local utility — Message 31 decoder
try:
    from nexrad_msg31 import decode_msg31, RADIAL_STATUS, WAVEFORM_TYPES
    HAS_MSG31 = True
except ImportError:
    HAS_MSG31 = False


# ---------------------------------------------------------------------------
# NEXRAD ICD constants
# ---------------------------------------------------------------------------

VOLUME_HEADER_SIZE = 24     # Archive II volume header (first record)
CTM_HEADER_SIZE    = 12     # Legacy CTM wrapper prepended to each message
MSG_HEADER_SIZE    = 28     # NEXRAD message header (follows CTM header)
LEGACY_MSG_SIZE    = 2432   # Fixed record size for pre-Message-31 format

# Message header format: big-endian
# size_halfwords (2), rda_channel (1), msg_type (1), seq_num (2),
# date (2), time_ms (4), num_segments (2), segment_num (2)
MSG_HEADER_FMT = '>HBBHHLHH'
MSG_HEADER_LEN = struct.calcsize(MSG_HEADER_FMT)  # should be 16 bytes

# Known message type descriptions
MSG_TYPES = {
    1:  "Digital Radar Data (legacy)",
    2:  "RDA Status Data",
    3:  "Performance/Maintenance Data",
    5:  "Volume Coverage Pattern",
    6:  "RDA Control Commands",
    7:  "Volume Coverage Pattern (RPG)",
    8:  "Clutter Sensor Zones",
    9:  "Request for Data",
    10: "Console Message",
    11: "Loop Back Test (RDA → RPG)",
    12: "Loop Back Test (RPG → RDA)",
    13: "Clutter Filter Bypass Map",
    15: "Clutter Filter Map",
    17: "RDA Adaptation Data",
    18: "RDA Adaptation Data (new)",
    20: "Spare",
    29: "User Selectable Parameters",
    31: "Digital Radar Data Generic Format (Message 31)",
    32: "Spare / Unknown",
}


# ---------------------------------------------------------------------------
# Date/time helpers
# ---------------------------------------------------------------------------

def nexrad_datetime(date_code: int, time_ms: int) -> str:
    """Convert NEXRAD Julian date code and milliseconds-since-midnight to ISO string."""
    try:
        base = datetime(1970, 1, 1, tzinfo=timezone.utc)
        dt = base + timedelta(days=date_code - 1, milliseconds=time_ms)
        return dt.strftime("%Y-%m-%d %H:%M:%S.%f")[:-3] + " UTC"
    except Exception:
        return f"date={date_code} time_ms={time_ms}"


# ---------------------------------------------------------------------------
# Format detection
# ---------------------------------------------------------------------------

def detect_format(data: bytes) -> str:
    """
    Detect file format from magic bytes.

    Returns:
        'uncompressed' — standard Archive II volume file
        'chunk_start'  — LDM chunk, start-of-volume (-S suffix)
        'chunk_inter'  — LDM chunk, intermediate (-I suffix), unsigned length prefix
        'chunk_end'    — LDM chunk, end-of-volume (-E suffix), signed negative length prefix
    """
    if len(data) < 28:
        return 'unknown'

    # Archive II volume header always begins with AR2V (current) or ARCHIVE2 (legacy)
    # May still be internally BZ2-chunked despite the AR2V header — check both
    if data[:4] == b'AR2V' or data[:8] == b'ARCHIVE2':
        if data[28:32] == b'BZh9':
            return 'chunk_start'
        return 'uncompressed'

    # End chunk: signed negative 4-byte length prefix — high bit set, BZh9 follows at offset 4
    # Interpret as signed int; negative value signals end-of-volume
    length_signed = struct.unpack('>i', data[:4])[0]
    if length_signed < 0 and data[4:8] == b'BZh9':
        return 'chunk_end'

    # Intermediate chunk: unsigned 4-byte length prefix followed by BZ2 magic
    if data[4:8] == b'BZh9':
        return 'chunk_inter'

    # Start chunk: 24-byte volume header, then 4-byte length prefix, then BZ2 magic
    if data[28:32] == b'BZh9':
        return 'chunk_start'

    return 'unknown'


# ---------------------------------------------------------------------------
# Decompression
# ---------------------------------------------------------------------------

def decompress_chunks(data: bytes, skip_header: bool) -> bytes:
    """
    Decompress a sequence of LDM BZ2-compressed records into a flat byte stream.

    Each record: [4-byte big-endian length][BZ2 compressed data]
    If skip_header is True, the first 24 bytes (volume header) are preserved
    as-is and prepended to the output before decompression begins.
    """
    out = io.BytesIO()
    offset = 0

    if skip_header:
        out.write(data[:VOLUME_HEADER_SIZE])
        offset = VOLUME_HEADER_SIZE

    while offset < len(data):
        if offset + 4 > len(data):
            break

        block_len = struct.unpack('>I', data[offset:offset + 4])[0]
        offset += 4

        # Sentinel: 0xFFFFFFFF marks end of compressed records
        if block_len == 0xFFFFFFFF:
            break

        if offset + block_len > len(data):
            print(f"  Warning: truncated block at offset {offset} "
                  f"(expected {block_len} bytes, got {len(data) - offset})")
            break

        compressed = data[offset:offset + block_len]
        offset += block_len

        try:
            out.write(bz2.decompress(compressed))
        except Exception as e:
            print(f"  Warning: decompression failed at offset {offset}: {e}")
            break

    return out.getvalue()


# ---------------------------------------------------------------------------
# Message walking
# ---------------------------------------------------------------------------

def walk_messages(data: bytes, filter_type: int | None, exclude_types: list[int] | None = None) -> list[dict]:
    """
    Walk raw (decompressed) NEXRAD data and extract all message headers.
    Returns a list of dicts, one per message.
    """
    messages = []
    offset = VOLUME_HEADER_SIZE  # skip volume header

    while offset < len(data):
        # Skip the 12-byte CTM header that precedes each message in the stream
        msg_start = offset + CTM_HEADER_SIZE

        if msg_start + MSG_HEADER_LEN > len(data):
            break

        raw = data[msg_start:msg_start + MSG_HEADER_LEN]
        try:
            (size_hw, rda_channel, msg_type, seq_num,
             date_code, time_ms, num_segments, segment_num) = struct.unpack(MSG_HEADER_FMT, raw)
        except struct.error:
            break

        # size_hw is message size in halfwords (2-byte units), including the header
        # A value of 0 or implausibly large usually means we've lost sync
        if size_hw == 0:
            offset += LEGACY_MSG_SIZE
            continue

        msg_size_bytes = size_hw * 2

        if filter_type is None or msg_type == filter_type:
            if not exclude_types or msg_type not in exclude_types:
                # Capture raw bytes for the full message (CTM header + message body)
                raw_bytes = data[offset:offset + CTM_HEADER_SIZE + msg_size_bytes]
                messages.append({
                    'offset':       offset,
                    'msg_type':     msg_type,
                    'seq_num':      seq_num,
                    'date_code':    date_code,
                    'time_ms':      time_ms,
                    'num_segments': num_segments,
                    'segment_num':  segment_num,
                    'size_bytes':   msg_size_bytes,
                    'rda_channel':  rda_channel,
                    'raw_bytes':    raw_bytes,
                })

        # Advance: CTM header + message size, padded to LEGACY_MSG_SIZE boundary
        # Message 31 is variable length; others use fixed 2432-byte records
        if msg_type == 31:
            # Variable length: advance by CTM + actual message size, aligned to 4 bytes
            advance = CTM_HEADER_SIZE + msg_size_bytes
            advance = (advance + 3) & ~3  # 4-byte align
        else:
            advance = LEGACY_MSG_SIZE

        offset += advance

        if offset >= len(data):
            break

    return messages


# ---------------------------------------------------------------------------
# Output
# ---------------------------------------------------------------------------

def hex_dump(data: bytes, bytes_per_row: int = 16, indent: int = 6) -> None:
    """Print a classic hex + ASCII dump of raw bytes."""
    pad = " " * indent
    for i in range(0, len(data), bytes_per_row):
        chunk = data[i:i + bytes_per_row]
        hex_part = " ".join(f"{b:02x}" for b in chunk)
        ascii_part = "".join(chr(b) if 32 <= b < 127 else "." for b in chunk)
        print(f"{pad}{i:06x}  {hex_part:<{bytes_per_row * 3}}  {ascii_part}")


def print_msg31_radial(m: dict, index: int) -> None:
    """Print a decoded Message 31 radial row replacing the standard message line."""
    r = decode_msg31(m['raw_bytes'])
    if r is None:
        print(f"  {index:<6} (Message 31 decode failed — offset {m['offset']})")
        return

    h = r['hdr']
    rc = r['rad_consts'] or {}
    ec = r['el_consts'] or {}
    vc = r['vol_consts'] or {}

    status     = RADIAL_STATUS.get(h['radial_status'], f"Unknown ({h['radial_status']})")
    nyquist    = rc.get('nyquist_vel_ms', 'N/A')
    unamb      = rc.get('unamb_range_km', 'N/A')
    atmos      = ec.get('atmos_atten', 'N/A')
    vcp        = vc.get('vcp', 'N/A')
    moments    = ' '.join(sorted(r['moments'].keys())) or '(none)'

    print(
        f"  {index:<5} "
        f"Az {h['az_angle']:>7.3f}°  "
        f"El {h['el_angle']:>6.3f}°  "
        f"Tilt {h['el_num']:>2}  "
        f"Rad {h['az_num']:>4}  "
        f"Status: {status:<26}  "
        f"VCP {vcp:<5}  "
        f"Nyq {nyquist:<6}  "
        f"Unamb {unamb:<6}  "
        f"Moments: {moments}"
    )

    # Per-moment gate geometry
    for name, mh in sorted(r['moments'].items()):
        first_gate_km = mh['first_gate'] / 1000.0
        gate_width_km = mh['gate_width'] / 1000.0
        print(
            f"        {name:<5}  "
            f"gates={mh['gate_count']:<5}  "
            f"first={first_gate_km:.3f} km  "
            f"width={gate_width_km:.4f} km  "
            f"word_size={mh['word_size']}b  "
            f"scale={mh['scale']:.4f}  "
            f"offset={mh['offset']:.4f}"
        )


def print_messages(messages: list[dict], show_summary: bool, dump: bool = False, decode31: bool = False) -> None:
    if show_summary:
        print_summary(messages)
        return

    if decode31 and not HAS_MSG31:
        print("  Warning: nexrad_msg31.py not found — --decode31 unavailable.")
        decode31 = False

    # Only print standard header if we're not in decode31-only mode
    all_msg31 = decode31 and all(m['msg_type'] == 31 for m in messages)
    if not all_msg31:
        print(f"\n  {'#':<6} {'Offset':>10} {'Type':>6} {'Description':<42} "
              f"{'Seq':>6} {'Seg':>5} {'Size (B)':>10} {'Timestamp'}")
        print("  " + "-" * 120)

    for i, m in enumerate(messages):
        if decode31 and m['msg_type'] == 31:
            print_msg31_radial(m, i)
        else:
            desc = MSG_TYPES.get(m['msg_type'], f"Unknown ({m['msg_type']})")
            ts = nexrad_datetime(m['date_code'], m['time_ms'])
            print(
                f"  {i:<6} {m['offset']:>10} {m['msg_type']:>6}  {desc:<42} "
                f"{m['seq_num']:>6} {m['segment_num']:>3}/{m['num_segments']:<3} "
                f"{m['size_bytes']:>8}  {ts}"
            )
            if dump:
                print()
                hex_dump(m['raw_bytes'])
                print()


def print_summary(messages: list[dict]) -> None:
    from collections import Counter
    counts = Counter(m['msg_type'] for m in messages)

    print(f"\n  {'Type':>6}  {'Count':>8}  Description")
    print("  " + "-" * 60)
    for msg_type, count in sorted(counts.items()):
        desc = MSG_TYPES.get(msg_type, f"Unknown ({msg_type})")
        print(f"  {msg_type:>6}  {count:>8}  {desc}")
    print(f"\n  Total messages: {len(messages)}")


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(
        description="Inspect raw NEXRAD Level II messages.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument("file", help="Path to Level II file (uncompressed or BZ2 chunk)")
    parser.add_argument("--limit", type=int, default=None,
                        help="Limit output to first N messages")
    parser.add_argument("--type", type=int, default=None, dest="msg_type",
                        help="Filter to a specific message type (e.g. --type 31)")
    parser.add_argument("--exclude", type=int, nargs="+", default=None, dest="exclude_types",
                        help="Exclude one or more message types (e.g. --exclude 31 or --exclude 31 2 5)")
    parser.add_argument("--summary", action="store_true",
                        help="Print a summary table of message type counts instead of full listing")
    parser.add_argument("--dump", action="store_true",
                        help="Print a hex dump of raw bytes for each matched message")
    parser.add_argument("--decode31", action="store_true",
                        help="Decode Message 31 radials into per-radial rows with az/el/moment detail")
    parser.add_argument("--raw", action="store_true",
                        help="Skip format detection and treat file as a flat message stream at offset 0")

    args = parser.parse_args()
    path = Path(args.file)

    if not path.exists():
        print(f"Error: file not found: {path}")
        sys.exit(1)

    raw = path.read_bytes()

    print(f"\nFile:   {path}")
    print(f"Size:   {len(raw):,} bytes")

    if args.raw:
        print(f"Format: raw (forced, skipping detection)")
        # Prepend a synthetic volume header so walk_messages skips to offset 24 as usual,
        # but the actual message stream starts immediately after it
        data = bytes(VOLUME_HEADER_SIZE) + raw
    else:
        fmt = detect_format(raw)
        print(f"Format: {fmt}")

        if fmt == 'uncompressed':
            data = raw
        elif fmt == 'chunk_inter':
            print("Decompressing BZ2 blocks...")
            data = decompress_chunks(raw, skip_header=False)
            data = bytes(VOLUME_HEADER_SIZE) + data
            print(f"Decompressed size: {len(data):,} bytes")
        elif fmt == 'chunk_start':
            print("Decompressing BZ2 blocks (start chunk, preserving volume header)...")
            data = decompress_chunks(raw, skip_header=True)
            print(f"Decompressed size: {len(data):,} bytes")
        elif fmt == 'chunk_end':
            print("Decompressing BZ2 block (end chunk, signed negative length prefix)...")
            length = abs(struct.unpack('>i', raw[:4])[0])
            decompressed = bz2.decompress(raw[4:4 + length])
            data = bytes(VOLUME_HEADER_SIZE) + decompressed
            print(f"Decompressed size: {len(data):,} bytes")
        else:
            print("Error: could not detect file format.")
            sys.exit(1)

    messages = walk_messages(data, filter_type=args.msg_type, exclude_types=args.exclude_types)

    if args.limit:
        messages = messages[:args.limit]

    print(f"\n{'=' * 122}")
    print_messages(messages, show_summary=args.summary, dump=args.dump, decode31=args.decode31)
    print(f"\n{'=' * 122}\n")


if __name__ == "__main__":
    main()
