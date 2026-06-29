#!/usr/bin/env python3
"""
inspect_geometry.py — NEXRAD Level II scan geometry inspector
Radar Workstation, Meteorological — development utility

Reconstructs the scan geometry from one or more -I (intermediate) chunk
files: tilt summary, radial coverage, moment availability, gate geometry,
waveform type, PRF, super-resolution flag, and Nyquist/unambiguous range.

No MetPy dependency — parses ICD binary structure directly.

Usage:
    python inspect_geometry.py <file> [<file> ...]
    python inspect_geometry.py 20260626-102243-002-I
    python inspect_geometry.py 20260626-102243-00*-I
"""

import argparse
import bz2
import struct
import sys
from pathlib import Path

from nexrad_msg31 import (
    CTM_HEADER_SIZE, MSG_HEADER_SIZE, MSG31_HDR_OFFSET,
    MSG31_HDR_FMT, MSG31_HDR_FIELDS, MSG31_HDR_LEN,
    MOMENT_HDR_FMT, MOMENT_HDR_FIELDS, MOMENT_HDR_LEN,
    MOMENT_BLOCKS, RADIAL_STATUS, WAVEFORM_TYPES,
    RVOL_FMT, RVOL_FIELDS, RELV_FMT, RELV_FIELDS,
    RRAD_V1_FMT, RRAD_V1_FIELDS, RRAD_V2_FMT, RRAD_V2_FIELDS,
    PTR_FMT, PTR_SIZE, decode_msg31,
)


VOLUME_HEADER_SIZE = 24
MSG_HEADER_FMT_OUTER = '>HBBHHLHH'
MSG_HEADER_LEN_OUTER = struct.calcsize(MSG_HEADER_FMT_OUTER)


# ---------------------------------------------------------------------------
# Decompression (mirrors inspect_messages.py)
# ---------------------------------------------------------------------------

def decompress_inter_chunk(data: bytes) -> bytes:
    """Decompress an intermediate (-I) chunk into a flat message stream."""
    import io
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
        try:
            out.write(bz2.decompress(data[offset:offset + block_len]))
        except Exception:
            break
        offset += block_len
    return out.getvalue()


# ---------------------------------------------------------------------------
# Message walker — yields raw_bytes for each Message 31
# ---------------------------------------------------------------------------

def iter_msg31(data: bytes):
    """Yield raw bytes for each Message 31 in a flat decompressed stream."""
    offset = 0
    while offset < len(data):
        msg_start = offset + CTM_HEADER_SIZE
        if msg_start + MSG_HEADER_LEN_OUTER > len(data):
            break
        raw = data[msg_start:msg_start + MSG_HEADER_LEN_OUTER]
        try:
            (size_hw, _, msg_type, _, _, _, _, _) = struct.unpack(MSG_HEADER_FMT_OUTER, raw)
        except struct.error:
            break
        if size_hw == 0:
            offset += 2432
            continue
        msg_size_bytes = size_hw * 2
        if msg_type == 31:
            yield data[offset:offset + CTM_HEADER_SIZE + msg_size_bytes]
        advance = CTM_HEADER_SIZE + msg_size_bytes
        advance = (advance + 3) & ~3
        offset += advance


# ---------------------------------------------------------------------------
# Geometry accumulator
# ---------------------------------------------------------------------------

class TiltGeometry:
    """Accumulates geometry data across all radials of a tilt."""

    def __init__(self, el_num: int):
        self.el_num        = el_num
        self.el_angles     = []
        self.az_angles     = []
        self.radial_count  = 0
        self.statuses      = set()
        self.moments       = {}   # name -> list of moment header dicts
        self.nyquist_vals  = []
        self.unamb_vals    = []
        self.vcp           = None
        self.waveform_type = None

    def add_radial(self, r: dict) -> None:
        h  = r['hdr']
        rc = r['rad_consts'] or {}
        vc = r['vol_consts'] or {}

        self.radial_count += 1
        self.el_angles.append(h['el_angle'])
        self.az_angles.append(h['az_angle'])
        self.statuses.add(h['radial_status'])

        if rc.get('nyquist_vel_ms'):
            self.nyquist_vals.append(rc['nyquist_vel_ms'])
        if rc.get('unamb_range_km'):
            self.unamb_vals.append(rc['unamb_range_km'])
        if vc.get('vcp') and self.vcp is None:
            self.vcp = vc['vcp']

        for name, mh in r['moments'].items():
            if name not in self.moments:
                self.moments[name] = []
            self.moments[name].append(mh)

    def el_angle_mean(self) -> float:
        return sum(self.el_angles) / len(self.el_angles) if self.el_angles else 0.0

    def az_spacing(self) -> float:
        if len(self.az_angles) < 2:
            return 0.0
        angles = sorted(self.az_angles)
        diffs = [angles[i+1] - angles[i] for i in range(len(angles) - 1)]
        return sum(diffs) / len(diffs) if diffs else 0.0

    def az_range(self) -> tuple:
        if not self.az_angles:
            return (0.0, 0.0)
        return (min(self.az_angles), max(self.az_angles))

    def is_super_res(self) -> bool:
        """Super-resolution tilts have ~0.5° azimuthal spacing."""
        return self.az_spacing() < 0.75

    def nyquist(self) -> str:
        if not self.nyquist_vals:
            return 'N/A'
        return f"{self.nyquist_vals[0]:.2f} m/s"

    def unamb_range(self) -> str:
        if not self.unamb_vals:
            return 'N/A'
        return f"{self.unamb_vals[0]:.1f} km"

    def moment_summary(self) -> dict:
        """Return per-moment gate geometry summary (from first radial)."""
        summary = {}
        for name, mh_list in sorted(self.moments.items()):
            if mh_list:
                mh = mh_list[0]
                summary[name] = {
                    'gate_count':  mh['gate_count'],
                    'first_gate':  mh['first_gate'] / 1000.0,
                    'gate_width':  mh['gate_width'] / 1000.0,
                    'word_size':   mh['word_size'],
                    'scale':       mh['scale'],
                    'offset':      mh['offset'],
                }
        return summary


# ---------------------------------------------------------------------------
# Output
# ---------------------------------------------------------------------------

SEP = "=" * 90
SUB = "-" * 90


def print_geometry(tilts: dict, source_files: list[str], vcp: int | None) -> None:
    print(f"\n{SEP}")
    print(f"  SCAN GEOMETRY SUMMARY")
    if source_files:
        print(f"  Source: {', '.join(source_files)}")
    if vcp:
        print(f"  VCP: {vcp}")
    print(f"  Tilts found: {len(tilts)}")
    print(SEP)

    for el_num in sorted(tilts.keys()):
        t = tilts[el_num]
        az_start, az_end = t.az_range()
        super_res = "Yes (0.5°)" if t.is_super_res() else "No  (1.0°)"
        moments_present = ' '.join(sorted(t.moments.keys()))

        print(f"\n  Tilt {el_num:>2}  —  El {t.el_angle_mean():>7.4f}°  |  "
              f"{t.radial_count:>4} radials  |  "
              f"Az {az_start:.2f}° → {az_end:.2f}°  |  "
              f"Spacing ~{t.az_spacing():.3f}°  |  "
              f"Super-res: {super_res}")
        print(f"          Nyquist: {t.nyquist():<14}  "
              f"Unamb range: {t.unamb_range():<12}  "
              f"Moments: {moments_present}")

        # Per-moment gate geometry
        print(f"\n          {'Moment':<8} {'Gates':>7} {'First (km)':>11} "
              f"{'Width (km)':>11} {'Word':>5} {'Scale':>10} {'Offset':>10}")
        print("          " + "-" * 68)
        for name, mg in t.moment_summary().items():
            print(
                f"          {name:<8} {mg['gate_count']:>7} {mg['first_gate']:>11.3f} "
                f"{mg['gate_width']:>11.4f} {mg['word_size']:>5} "
                f"{mg['scale']:>10.4f} {mg['offset']:>10.4f}"
            )

    print(f"\n{SEP}\n")


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(
        description="Inspect scan geometry of NEXRAD Level II intermediate chunk files.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument("files", nargs="+",
                        help="One or more -I chunk files to inspect")

    args = parser.parse_args()

    tilts: dict[int, TiltGeometry] = {}
    vcp = None
    source_files = []

    for filepath in args.files:
        path = Path(filepath)
        if not path.exists():
            print(f"Warning: file not found: {path}", file=sys.stderr)
            continue

        source_files.append(path.name)
        raw = path.read_bytes()

        # Detect and decompress
        if raw[4:8] == b'BZh9':
            data = decompress_inter_chunk(raw)
        elif raw[:4] == b'AR2V' or raw[:8] == b'ARCHIVE2':
            data = raw[VOLUME_HEADER_SIZE:]
        else:
            print(f"Warning: unrecognised format for {path.name}, skipping.", file=sys.stderr)
            continue

        for raw_msg in iter_msg31(data):
            r = decode_msg31(raw_msg)
            if r is None:
                continue

            el_num = r['hdr']['el_num']
            if el_num not in tilts:
                tilts[el_num] = TiltGeometry(el_num)
            tilts[el_num].add_radial(r)

            if vcp is None and r['vol_consts']:
                vcp = r['vol_consts'].get('vcp')

    if not tilts:
        print("No Message 31 radials found.")
        sys.exit(1)

    print_geometry(tilts, source_files, vcp)


if __name__ == "__main__":
    main()
