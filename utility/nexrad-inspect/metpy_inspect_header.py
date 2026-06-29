#!/usr/bin/env python3
"""
inspect_header.py — NEXRAD Level II header inspector
Radar Workstation, Meteorological — development utility

Reads a Level II archive file and prints a structured summary of its
header content: volume header, VCP, sweep geometry, per-radial metadata,
and available moments. Intended as a cross-validation reference during
development of the Rust decoder.

Usage:
    python inspect_header.py <file>
    python inspect_header.py <file> --sweep 0
    python inspect_header.py <file> --sweep 0 --radials 5
    python inspect_header.py <file> --moments
    python inspect_header.py <file> --raw

Dependencies:
    pip install metpy numpy
"""

import argparse
import sys
from datetime import timezone
from pathlib import Path

try:
    import numpy as np
    from metpy.io import Level2File
except ImportError as e:
    print(f"Error: missing dependency — {e}")
    print("Install with: pip install metpy numpy")
    sys.exit(1)


# ---------------------------------------------------------------------------
# Formatting helpers
# ---------------------------------------------------------------------------

SEP_MAJOR = "=" * 72
SEP_MINOR = "-" * 72
SEP_SECTION = "  " + "-" * 60


def section(title: str) -> None:
    print(f"\n{SEP_MAJOR}")
    print(f"  {title}")
    print(SEP_MAJOR)


def subsection(title: str) -> None:
    print(f"\n  {title}")
    print(SEP_SECTION)


def field(label: str, value, indent: int = 4) -> None:
    pad = " " * indent
    print(f"{pad}{label:<36} {value}")


def decode_rad_status(code: int) -> str:
    """Translate radial status code to human-readable string."""
    mapping = {
        0: "Start of New Elevation",
        1: "Intermediate Radial",
        2: "End of Elevation",
        3: "Beginning of Volume Scan",
        4: "End of Volume Scan",
        5: "Start of New Elevation (cut)",
    }
    return mapping.get(code, f"Unknown ({code})")


def decode_vcp(vcp: int) -> str:
    """Translate VCP number to common operational description."""
    mapping = {
        11: "VCP 11 — Severe Weather (14 tilts, ~5 min)",
        12: "VCP 12 — Severe Weather (14 tilts, SAILS capable)",
        21: "VCP 21 — Precipitation/Clear Air (~6 min)",
        31: "VCP 31 — Clear Air (long PRI, ~10 min)",
        32: "VCP 32 — Clear Air (short PRI)",
        35: "VCP 35 — SAILS (6 tilts, ~3.5 min)",
        80: "VCP 80 — Maintenance/Test",
        112: "VCP 112 — AVSET Severe Weather",
        212: "VCP 212 — AVSET Precipitation",
        215: "VCP 215 — AVSET Clear Air",
    }
    return mapping.get(vcp, f"VCP {vcp}")


def safe_attr(obj, attr, default="N/A"):
    """Return attribute value or default if absent/None."""
    val = getattr(obj, attr, default)
    return default if val is None else val


# ---------------------------------------------------------------------------
# Output sections
# ---------------------------------------------------------------------------

def print_volume_header(f: Level2File) -> None:
    section("VOLUME HEADER")
    vh = f.vol_hdr
    field("Station ID (from vol hdr):", safe_attr(vh, "icao", "N/A"))
    field("Volume scan date (raw):", safe_attr(vh, "date", "N/A"))
    field("Volume scan time (raw ms):", safe_attr(vh, "time_ms", "N/A"))
    # Resolved timestamp from MetPy
    field("Resolved timestamp (UTC):", str(f.dt))
    field("Station ID (resolved):", f.stid.decode() if isinstance(f.stid, bytes) else f.stid)


def print_site_constants(f: Level2File) -> None:
    section("SITE CONSTANTS  (from first radial VolConsts block)")
    if not f.sweeps or not f.sweeps[0]:
        print("  No sweep data available.")
        return

    radial = f.sweeps[0][0]
    vc = radial[1]  # VolConsts namedtuple

    field("Latitude (deg):", f"{safe_attr(vc, 'lat'):.6f}")
    field("Longitude (deg):", f"{safe_attr(vc, 'lon'):.6f}")
    field("Site AMSL (ft):", safe_attr(vc, "site_amsl"))
    field("Feedhorn AGL (ft):", safe_attr(vc, "feedhorn_agl"))
    field("Calib dBZ offset:", safe_attr(vc, "calib_dbz"))
    field("TX Power H (dBm):", safe_attr(vc, "txpower_h"))
    field("TX Power V (dBm):", safe_attr(vc, "txpower_v"))
    field("System ZDR (dB):", safe_attr(vc, "sys_zdr"))
    field("System PhiDP0 (deg):", safe_attr(vc, "phidp0"))
    field("VCP:", decode_vcp(safe_attr(vc, "vcp", 0)))
    field("Build / Processing Status:", safe_attr(vc, "processing_status"))


def print_sweep_table(f: Level2File) -> None:
    section("SWEEP SUMMARY")
    print(f"\n  {'#':<5} {'Elev Angle':>12} {'Radials':>10} {'Moments Available'}")
    print("  " + "-" * 65)

    for i, sweep in enumerate(f.sweeps):
        if not sweep:
            print(f"  {i:<5} (empty sweep)")
            continue

        first_radial = sweep[0]
        hdr = first_radial[0]
        el_angle = safe_attr(hdr, "el_angle", float("nan"))

        # Collect available moments from first radial's data dict
        data_dict = first_radial[4] if len(first_radial) > 4 else {}
        moments = ", ".join(
            k.decode() if isinstance(k, bytes) else str(k)
            for k in sorted(data_dict.keys())
        )

        print(f"  {i:<5} {el_angle:>11.4f}°  {len(sweep):>9}  {moments or '(none)'}")

    print(f"\n  Total sweeps: {len(f.sweeps)}")


def print_sweep_detail(f: Level2File, sweep_idx: int, num_radials: int) -> None:
    section(f"SWEEP {sweep_idx} DETAIL")

    if sweep_idx >= len(f.sweeps):
        print(f"  Error: sweep index {sweep_idx} out of range (file has {len(f.sweeps)} sweeps).")
        return

    sweep = f.sweeps[sweep_idx]
    if not sweep:
        print("  Sweep is empty.")
        return

    # Elevation constants block from first radial
    first_radial = sweep[0]
    ec = first_radial[2] if len(first_radial) > 2 else None
    if ec is not None:
        subsection("Elevation Constants")
        field("Atmospheric attenuation:", safe_attr(ec, "atmos_atten"))

    # Moment geometry — print per-moment gate parameters from first radial
    data_dict = first_radial[4] if len(first_radial) > 4 else {}
    if data_dict:
        subsection("Moment Gate Geometry (from first radial)")
        print(f"    {'Moment':<8} {'First Gate (km)':>16} {'Gate Width (km)':>16} {'Num Gates':>10}")
        print("    " + "-" * 54)
        for key in sorted(data_dict.keys()):
            mhdr = data_dict[key][0]
            name = key.decode() if isinstance(key, bytes) else str(key)
            field_val = (
                f"{safe_attr(mhdr, 'first_gate'):>16.3f}  "
                f"{safe_attr(mhdr, 'gate_width'):>14.4f}  "
                f"{safe_attr(mhdr, 'num_gates'):>9}"
            )
            print(f"    {name:<8} {field_val}")

    # Per-radial table
    count = min(num_radials, len(sweep))
    subsection(f"Radial Headers (first {count} of {len(sweep)})")
    print(
        f"    {'#':<5} {'Az (deg)':>10} {'El (deg)':>10} "
        f"{'Az Num':>8} {'Status'}"
    )
    print("    " + "-" * 65)

    for idx in range(count):
        hdr = sweep[idx][0]
        az = safe_attr(hdr, "az_angle", float("nan"))
        el = safe_attr(hdr, "el_angle", float("nan"))
        az_num = safe_attr(hdr, "az_num", "?")
        rad_status = safe_attr(hdr, "rad_status", -1)
        status_str = decode_rad_status(rad_status) if isinstance(rad_status, int) else str(rad_status)
        print(f"    {idx:<5} {az:>10.4f}  {el:>10.4f}  {az_num:>8}  {status_str}")

    if num_radials < len(sweep):
        print(f"    ... ({len(sweep) - num_radials} more radials not shown)")


def print_moment_stats(f: Level2File, sweep_idx: int = 0) -> None:
    section(f"MOMENT DATA STATISTICS  (sweep {sweep_idx})")

    if sweep_idx >= len(f.sweeps):
        print(f"  Sweep index {sweep_idx} out of range.")
        return

    sweep = f.sweeps[sweep_idx]
    if not sweep:
        print("  Sweep is empty.")
        return

    first_radial = sweep[0]
    data_dict = first_radial[4] if len(first_radial) > 4 else {}

    for key in sorted(data_dict.keys()):
        name = key.decode() if isinstance(key, bytes) else str(key)
        subsection(f"Moment: {name}")

        try:
            arr = np.array([ray[4][key][1] for ray in sweep], dtype=float)
            valid = arr[~np.isnan(arr)]
            field("Array shape (radials × gates):", str(arr.shape))
            field("Valid gate count:", f"{len(valid):,} of {arr.size:,} ({100*len(valid)/arr.size:.1f}%)")
            if len(valid) > 0:
                field("Min:", f"{valid.min():.4f}")
                field("Max:", f"{valid.max():.4f}")
                field("Mean:", f"{valid.mean():.4f}")
                field("Std dev:", f"{valid.std():.4f}")
            else:
                field("All values:", "masked / NaN")
        except Exception as e:
            field("Error reading moment:", str(e))


def print_raw(f: Level2File, sweep_idx: int = 0) -> None:
    section(f"RAW FIRST RADIAL  (sweep {sweep_idx})")
    if sweep_idx >= len(f.sweeps) or not f.sweeps[sweep_idx]:
        print("  No data.")
        return
    radial = f.sweeps[sweep_idx][0]
    for i, item in enumerate(radial):
        if i == 4:
            print(f"\n  [4] Data blocks (dict):")
            for k, v in item.items():
                name = k.decode() if isinstance(k, bytes) else str(k)
                print(f"      {name}: header={v[0]}, data shape={np.array(v[1]).shape}")
        else:
            print(f"\n  [{i}] {item}")


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(
        description="Inspect headers of a NEXRAD Level II archive file.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument("file", help="Path to Level II file (.ar2v, .gz, .bz2)")
    parser.add_argument(
        "--sweep", type=int, default=None,
        help="Print detailed radial headers for this sweep index (0-based)"
    )
    parser.add_argument(
        "--radials", type=int, default=10,
        help="Number of radials to show in sweep detail (default: 10)"
    )
    parser.add_argument(
        "--moments", action="store_true",
        help="Print per-moment data statistics for --sweep (default: sweep 0)"
    )
    parser.add_argument(
        "--raw", action="store_true",
        help="Dump the raw namedtuple representation of the first radial"
    )

    args = parser.parse_args()
    path = Path(args.file)

    if not path.exists():
        print(f"Error: file not found: {path}")
        sys.exit(1)

    print(f"\nOpening: {path}")
    print("(MetPy will decompress and decode — may take a moment for large files...)\n")

    try:
        f = Level2File(str(path))
    except Exception as e:
        print(f"Error reading file: {e}")
        sys.exit(1)

    print_volume_header(f)
    print_site_constants(f)
    print_sweep_table(f)

    sweep_target = args.sweep if args.sweep is not None else 0

    if args.sweep is not None:
        print_sweep_detail(f, sweep_target, args.radials)

    if args.moments:
        print_moment_stats(f, sweep_target)

    if args.raw:
        print_raw(f, sweep_target)

    print(f"\n{SEP_MAJOR}")
    print("  Done.")
    print(SEP_MAJOR + "\n")


if __name__ == "__main__":
    main()
