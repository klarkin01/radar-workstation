#!/usr/bin/env python3
"""
inspect_compression.py — Check internal compression format of a NEXRAD Level II file.

Usage:
    python inspect_compression.py <file>
"""

import sys
from pathlib import Path

if len(sys.argv) != 2:
    print("Usage: python inspect_compression.py <file>")
    sys.exit(1)

path = Path(sys.argv[1])

if not path.exists():
    print(f"Error: file not found: {path}")
    sys.exit(1)

with open(path, "rb") as f:
    f.seek(4)  # Alter as needed depending on the file structure to reach the compression header
    chunk_header = f.read(4)
    print(chunk_header)
