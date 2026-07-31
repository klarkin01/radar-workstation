#!/usr/bin/env python3
"""
extract_legacy_archive_fixtures.py — Extract Message 31 fixtures from a
pre-dual-pol archive volume file (S1-W4d, non-dual-pol era).

Unlike the real-time chunk stream, archive volume files of this era
(`V03` extension, ~2010 and earlier at most sites) are NOT internally
BZ2-block-wrapped: the message stream begins directly after the 24-byte
volume header. `gen_fixtures.py`'s `decompress_chunk` assumes BZ2 blocks
(the chunk-stream and newer archive-file format) and does not apply here,
so this is a separate, minimal script rather than a flag on that one —
the envelope is different enough that forcing them into one code path
would obscure both.

Source file (2026-07-31): gunzip
    https://unidata-nexrad-level2.s3.amazonaws.com/2010/06/01/KTLH/KTLH20100601_000029_V03.gz
VCP 121, no dual-pol moment blocks on any elevation — see
docs/plans/stage-0-1-close-the-acquisition-path.md S1-W4d.

Usage:
    python extract_legacy_archive_fixtures.py <ar2v_file> <output_dir> --prefix ktlh_vcp121
"""

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from gen_fixtures import iter_msg31  # noqa: E402 (path insert must come first)

TARGET_STATUSES = {
    3: "start_of_volume",
    0: "start_of_elevation",
    1: "intermediate",
    2: "end_of_elevation",
    4: "end_of_volume",
}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__,
                                      formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("archive_file", help="Gunzipped AR2V archive volume file")
    parser.add_argument("output_dir", help="Output directory for fixture .bin files")
    parser.add_argument("--prefix", required=True, help="Filename prefix, e.g. ktlh_vcp121")
    args = parser.parse_args()

    data = Path(args.archive_file).read_bytes()
    if data[:4] != b"AR2V":
        print(f"Error: {args.archive_file} does not start with the AR2V volume header magic")
        sys.exit(1)

    out_dir = Path(args.output_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    remaining = set(TARGET_STATUSES)
    for record, status in iter_msg31(data):
        if status in remaining:
            name = TARGET_STATUSES[status]
            out_path = out_dir / f"{args.prefix}_{name}.bin"
            out_path.write_bytes(record)
            print(f"  wrote {len(record):,} bytes -> {out_path.name} (status={status})")
            remaining.discard(status)
        if not remaining:
            break

    if remaining:
        missing = [TARGET_STATUSES[s] for s in sorted(remaining)]
        print(f"  Note: no radials found for: {', '.join(missing)}")


if __name__ == "__main__":
    main()
