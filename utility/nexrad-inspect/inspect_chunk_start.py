#!/usr/bin/env python3
"""
inspect_chunk_start.py — Decompress and inspect a NEXRAD LDM start chunk (-S)
Radar Workstation, Meteorological — development utility

Decompresses the BZ2 payload of a start chunk and prints the
decompressed size, first 64 bytes as raw text, and first 64 bytes
as hex. Writes the full decompressed content to /tmp/s_chunk_decompressed.bin.

Usage:
    python inspect_chunk_start.py <file>
"""

import bz2
import struct
import sys
from pathlib import Path

if len(sys.argv) != 2:
    print("Usage: python inspect_chunk_start.py <file>")
    sys.exit(1)

path = Path(sys.argv[1])

if not path.exists():
    print(f"Error: file not found: {path}")
    sys.exit(1)

data = path.read_bytes()
VOLUME_HEADER_SIZE = 24
length = struct.unpack(">I", data[VOLUME_HEADER_SIZE:VOLUME_HEADER_SIZE + 4])[0]
decompressed = bz2.decompress(data[VOLUME_HEADER_SIZE + 4:VOLUME_HEADER_SIZE + 4 + length])

open("/tmp/s_chunk_decompressed.bin", "wb").write(decompressed)

print(f"Decompressed size: {len(decompressed)} bytes")
print(f"First 64 bytes:     {decompressed[:64]}")
print(f"First 64 bytes (hex): {decompressed[:64].hex()}")
