#!/usr/bin/env python3
"""
inspect_metadata.py — NEXRAD Level II metadata field inspector
Radar Workstation, Meteorological — development utility

Inspects the metadata fields present in a Level II archive file.
Defaults to volume-level fields only. Pass --radials to see radial
header fields (sampled from the first radial of the first sweep).
Pass --values to show field values alongside field names.

Usage:
    python inspect_metadata.py <file>
    python inspect_metadata.py <file> --radials
    python inspect_metadata.py <file> --volume --radials
    python inspect_metadata.py <file> --values
    python inspect_metadata.py <file> --radials --values
"""

import argparse
import sys
from pathlib import Path

try:
    from metpy.io import Level2File
except ImportError as e:
    print(f"Error: missing dependency — {e}")
    print("Install with: pip install metpy numpy")
    sys.exit(1)


def fields_from(obj, show_values: bool) -> list:
    """Return a list of (field_name, value_or_None) from a namedtuple or object."""
    if hasattr(obj, '_fields'):
        if show_values:
            return [(f, getattr(obj, f, None)) for f in obj._fields]
        else:
            return [(f, None) for f in obj._fields]
    elif hasattr(obj, '__dict__'):
        if show_values:
            return list(vars(obj).items())
        else:
            return [(k, None) for k in vars(obj)]
    return []


def print_fields(title: str, items: list) -> None:
    print(f"\n  {title}")
    print("  " + "-" * 50)
    for name, value in items:
        if value is None:
            print(f"    {name}")
        else:
            print(f"    {name}: {value}")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Inspect metadata fields in a NEXRAD Level II file.",
    )
    parser.add_argument("file", help="Path to Level II file")
    parser.add_argument("--volume", action="store_true",
                        help="Show volume-level fields (default if neither flag is passed)")
    parser.add_argument("--radials", action="store_true",
                        help="Show radial header fields (sampled from first radial of first sweep)")
    parser.add_argument("--values", action="store_true",
                        help="Show field values alongside field names")

    args = parser.parse_args()

    # Default to --volume if neither scope flag is passed
    show_volume = args.volume or not args.radials
    show_radials = args.radials

    path = Path(args.file)
    if not path.exists():
        print(f"Error: file not found: {path}")
        sys.exit(1)

    print(f"\nOpening: {path}")
    try:
        f = Level2File(str(path))
    except Exception as e:
        print(f"Error reading file: {e}")
        sys.exit(1)

    print(f"File parsed successfully.\n")
    print("=" * 54)

    if show_volume:
        print_fields("Volume header fields (f.vol_hdr)", fields_from(f.vol_hdr, args.values))

        # Top-level file attributes
        top_level = [("stid", f.stid if args.values else None),
                     ("dt",   f.dt   if args.values else None)]
        print_fields("Top-level file attributes", top_level)

    if show_radials and f.sweeps and f.sweeps[0]:
        radial = f.sweeps[0][0]

        # radial is a tuple: (Msg31DataHdr, VolConsts, ElConsts, ?, data_dict)
        labels = ["Msg31DataHdr", "VolConsts", "ElConsts"]
        for i, label in enumerate(labels):
            if i < len(radial) and radial[i] is not None:
                print_fields(f"Radial block [{i}]: {label}", fields_from(radial[i], args.values))

        # Data block moment keys
        if len(radial) > 4 and isinstance(radial[4], dict):
            data_dict = radial[4]
            print_fields("Radial block [4]: moment keys (data dict)",
                         [(k.decode() if isinstance(k, bytes) else str(k),
                           type(v).__name__ if args.values else None)
                          for k, v in data_dict.items()])

            # Fields within each moment header
            for key, val in data_dict.items():
                name = key.decode() if isinstance(key, bytes) else str(key)
                moment_hdr = val[0]
                print_fields(f"  Moment '{name}' header fields",
                             fields_from(moment_hdr, args.values))

    elif show_radials:
        print("\n  No sweep data available for radial inspection.")

    print("\n" + "=" * 54 + "\n")


if __name__ == "__main__":
    main()
