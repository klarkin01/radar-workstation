#!/usr/bin/env python3
"""Bake the map underlay bundle (`crates/radar-workstation/src/overlay/overlay.bin`
plus its `bundle.manifest.txt`) from five committed/downloaded shapefile
sources: three Natural Earth boundary/coastline layers, the TIGER primary
roads layer, and Natural Earth's populated places (city labels).

Implements docs/plans/stage-5-map-underlays.md §4 (ADR-0025 §2, ADR-0028,
ADR-0029). stdlib only — see ADR-0029 §5's "no third-party parser was
installed to answer a question about not shipping one," which applies here
too. Digest-verified against a fixed provenance table before anything is
emitted, so a substituted or re-downloaded source fails loudly rather than
silently baking different geometry (ADR-0025 §6).

Usage:
    python3 utility/map-bake/bake.py

Sources (four already on disk in the .gitignore'd utility/radar-viz/data/;
the fifth — ne_10m_populated_places — must be downloaded first):

    curl -o utility/radar-viz/data/ne_10m_populated_places.zip \\
        https://naciscdn.org/naturalearth/10m/cultural/ne_10m_populated_places.zip
    (cd utility/radar-viz/data && unzip -o ne_10m_populated_places.zip)

This is dev-only tooling (see utility/README.md) — not built, imported, or
run by anything in crates/. Its only output is the committed bundle and
manifest.
"""
from __future__ import annotations

import datetime
import hashlib
import math
import pathlib
import re
import struct
import sys

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent.parent
DATA_DIR = REPO_ROOT / "utility" / "radar-viz" / "data"
SITES_FILE = REPO_ROOT / "crates" / "radar-workstation" / "src" / "sites_generated.rs"
OUT_DIR = REPO_ROOT / "crates" / "radar-workstation" / "src" / "overlay"
BUNDLE_OUT = OUT_DIR / "overlay.bin"
MANIFEST_OUT = OUT_DIR / "bundle.manifest.txt"

MAGIC = b"RWMOVL01"
FORMAT_VERSION = 1

FOOTPRINT_KM = 700.0
SIMPLIFY_EPSILON_M = 30.0
EARTH_RADIUS_KM = 6371.0

# --- Provenance: every source this generator will read, and the digest it
# must match. A mismatch is a hard, loud failure (ADR-0025 §6) — this table
# is the review surface for "what geometry did this bundle actually come
# from," since the 6.41 MB blob itself is not reviewable in a diff.
SOURCES = {
    "tl_2025_us_primaryroads.zip": (
        "400453e97b9e6693dfecb7362ce7a6cf260d27050f7d84d2a024ba0710b94c07",
        "https://www2.census.gov/geo/tiger/TIGER2025/PRIMARYRD/tl_2025_us_primaryroads.zip",
        "2025-09-15",
        "U.S. Census Bureau TIGER/Line — public domain",
    ),
    "tl_2025_us_primaryroads.shp": (
        "0a71f09e16325e815961e5486b71e825c1da31e9d80fd58fe5b5da0c01ed313b",
        None, None, None,
    ),
    "tl_2025_us_primaryroads.dbf": (
        "4b9e2a05d259c73ced83eb6769db225b717945b52509444635869dfdb29dfce6",
        None, None, None,
    ),
    "ne_10m_admin_1_states_provinces.shp": (
        "c6f5c8b4b1320d9417033762419c6df1eb423989cd880fba78ea0b1e3522cbe4",
        "https://naciscdn.org/naturalearth/10m/cultural/ne_10m_admin_1_states_provinces.zip",
        "2022-05-09",
        "Natural Earth — public domain",
    ),
    "ne_10m_admin_1_states_provinces.dbf": (
        "445a8a9bea889634faf0af18081830df0b05b8471fc6af8dc42aecdd7a71bba1",
        None, None, None,
    ),
    "ne_10m_admin_2_counties_lakes.shp": (
        "3b2d28346a793500f855f130bbebe4562f17427a7b170f0aaec8bdefcb51114e",
        "https://naciscdn.org/naturalearth/10m/cultural/ne_10m_admin_2_counties_lakes.zip",
        "2022-05-09",
        "Natural Earth — public domain",
    ),
    "ne_10m_admin_2_counties_lakes.dbf": (
        "5f8a71a570b35a6164ce29dffce34bf69b05f1512f69626a6e8327bc22d25b3f",
        None, None, None,
    ),
    "ne_10m_coastline.shp": (
        "459a4a97c09db19aadf5244026612de9d43748be27f83a360242b99f7fabb3c1",
        "https://naciscdn.org/naturalearth/10m/physical/ne_10m_coastline.zip",
        "2021-11-14",
        "Natural Earth — public domain",
    ),
    "ne_10m_coastline.dbf": (
        "9ccc214342fe400bf8c7d91d7a5b276b0457b0ada03e8d4be16ac5ba13037f3b",
        None, None, None,
    ),
    "ne_10m_populated_places.shp": (
        "f4073365d248dfe35a44cca6715bf5b4812fa723294ed6a37043d7a3e09cc998",
        "https://naciscdn.org/naturalearth/10m/cultural/ne_10m_populated_places.zip",
        "2026-09-02",
        "Natural Earth — public domain",
    ),
    "ne_10m_populated_places.dbf": (
        "eff834a8727e06f28fc52b04393c462d074aecf9a7acd1b870e766240a995b6c",
        None, None, None,
    ),
}

# --- Layers this bundle carries, in bake (and bundle-table) order. `simplify`
# only ever applies to primaryroads (ADR-0029 §5) — the three Natural Earth
# layers stay at native density.
LAYERS = [
    # (kind, name, shp stem, simplify?)
    (1, "admin_2_counties_lakes", "ne_10m_admin_2_counties_lakes", False),
    (2, "admin_1_states_provinces", "ne_10m_admin_1_states_provinces", False),
    (3, "coastline", "ne_10m_coastline", False),
    (4, "primaryroads", "tl_2025_us_primaryroads", True),
]
LABEL_KIND = 5


def fail(msg: str) -> None:
    print(f"utility/map-bake/bake.py: {msg}", file=sys.stderr)
    sys.exit(1)


# --------------------------------------------------------------------------
# Provenance
# --------------------------------------------------------------------------


def verify_digests() -> None:
    for filename, (expected, *_rest) in SOURCES.items():
        path = DATA_DIR / filename
        if not path.exists():
            fail(f"missing source file: {path} (see this script's module docstring)")
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if digest != expected:
            fail(
                f"digest mismatch for {filename}: expected {expected}, got {digest} — "
                "a substituted or re-downloaded source must not be baked silently"
            )


# --------------------------------------------------------------------------
# Site table (for the 700 km footprint filter)
# --------------------------------------------------------------------------

SITE_LINE_RE = re.compile(
    r'Site\s*\{\s*id:\s*"([A-Z0-9]{4})".*?lat:\s*(-?[\d.]+),\s*lon:\s*(-?[\d.]+),'
)


def read_sites() -> list[tuple[str, float, float]]:
    text = SITES_FILE.read_text(encoding="utf-8")
    sites = [(m.group(1), float(m.group(2)), float(m.group(3))) for m in SITE_LINE_RE.finditer(text)]
    if len(sites) != 163:
        fail(
            f"expected 163 sites parsed from {SITES_FILE.name}, got {len(sites)} — "
            "a reformatted generated file must fail loudly, not silently filter against "
            "the wrong site count"
        )
    return sites


# --------------------------------------------------------------------------
# SHP reader (main file only — .shx is an index this generator doesn't need,
# it reads records sequentially)
# --------------------------------------------------------------------------

SHP_POLYLINE = 3
SHP_POLYGON = 5
SHP_POINT = 1


def read_shp_records(
    path: pathlib.Path,
) -> list[tuple[tuple[float, float, float, float], list[list[tuple[float, float]]]]]:
    """Every record in a PolyLine/Polygon shapefile, as (record_bbox, parts) —
    `record_bbox` is the shape's own stored box (covering every part/ring in
    the record), which is what the 700 km footprint filter is applied
    against (§4.3): a record's far-flung minor ring (a small offshore island
    in a states/provinces polygon, say) is kept or dropped with the rest of
    its feature, not filtered in isolation. Parts of fewer than 2 points are
    dropped. Polygon rings are treated as closed polylines — all rings,
    including holes, are kept when the record passes the filter."""
    data = path.read_bytes()
    file_len_words = struct.unpack(">i", data[24:28])[0]
    file_len_bytes = file_len_words * 2
    if file_len_bytes > len(data):
        fail(f"{path.name}: header file length exceeds actual file size")

    records: list[tuple[tuple[float, float, float, float], list[list[tuple[float, float]]]]] = []
    offset = 100
    while offset < file_len_bytes:
        if offset + 8 > len(data):
            fail(f"{path.name}: truncated record header at offset {offset}")
        content_words = struct.unpack(">i", data[offset + 4:offset + 8])[0]
        content_start = offset + 8
        content_end = content_start + content_words * 2
        if content_end > len(data):
            fail(f"{path.name}: truncated record content at offset {offset}")
        shape_type = struct.unpack("<i", data[content_start:content_start + 4])[0]
        if shape_type in (SHP_POLYLINE, SHP_POLYGON):
            body = data[content_start + 4:content_end]
            # box(32) num_parts(4) num_points(4) parts[num_parts](4 each) points[num_points](16 each)
            min_x, min_y, max_x, max_y = struct.unpack("<dddd", body[0:32])
            num_parts, num_points = struct.unpack("<ii", body[32:40])
            part_starts = struct.unpack(f"<{num_parts}i", body[40:40 + 4 * num_parts])
            points_offset = 40 + 4 * num_parts
            points = [
                struct.unpack("<dd", body[points_offset + 16 * i:points_offset + 16 * i + 16])
                for i in range(num_points)
            ]
            bounds = list(part_starts) + [num_points]
            parts = [points[bounds[i]:bounds[i + 1]] for i in range(num_parts)]
            parts = [p for p in parts if len(p) >= 2]
            if parts:
                records.append(((min_x, min_y, max_x, max_y), parts))
        elif shape_type != 0:
            fail(f"{path.name}: unexpected shape type {shape_type} in a PolyLine/Polygon file")
        offset = content_end
    return records


def read_shp_points(path: pathlib.Path) -> list[tuple[float, float]]:
    """Every (lon, lat) in a Point shapefile, in record order — the order
    `read_dbf_records` walks records in, so the two zip up positionally."""
    data = path.read_bytes()
    file_len_bytes = struct.unpack(">i", data[24:28])[0] * 2
    points: list[tuple[float, float]] = []
    offset = 100
    while offset < file_len_bytes:
        content_words = struct.unpack(">i", data[offset + 4:offset + 8])[0]
        content_start = offset + 8
        content_end = content_start + content_words * 2
        shape_type = struct.unpack("<i", data[content_start:content_start + 4])[0]
        if shape_type == SHP_POINT:
            x, y = struct.unpack("<dd", data[content_start + 4:content_start + 20])
            points.append((x, y))
        elif shape_type != 0:
            fail(f"{path.name}: unexpected shape type {shape_type} in a Point file")
        offset = content_end
    return points


# --------------------------------------------------------------------------
# DBF reader (only used for populated_places' NAME/SCALERANK/POP_MAX)
# --------------------------------------------------------------------------


def read_dbf_records(path: pathlib.Path, wanted: list[str]) -> list[dict[str, str]]:
    data = path.read_bytes()
    num_records = struct.unpack("<I", data[4:8])[0]
    header_len = struct.unpack("<H", data[8:10])[0]
    record_len = struct.unpack("<H", data[10:12])[0]

    fields: list[tuple[str, int, int]] = []  # (name, offset_in_record, length)
    field_offset = 1  # record starts with a 1-byte deletion flag
    pos = 32
    while data[pos:pos + 1] != b"\x0d":
        descriptor = data[pos:pos + 32]
        name = descriptor[0:11].split(b"\x00")[0].decode("ascii")
        length = descriptor[16]
        fields.append((name, field_offset, length))
        field_offset += length
        pos += 32
    if pos + 1 != header_len:
        fail(f"{path.name}: field descriptor table length disagrees with header_len")

    wanted_fields = [f for f in fields if f[0] in wanted]
    if len(wanted_fields) != len(wanted):
        found = {f[0] for f in wanted_fields}
        fail(f"{path.name}: missing expected field(s) {sorted(set(wanted) - found)}")

    records: list[dict[str, str]] = []
    for i in range(num_records):
        rec_start = header_len + i * record_len
        rec = data[rec_start:rec_start + record_len]
        if rec[0:1] == b"*":
            continue  # soft-deleted record
        try:
            values = {name: rec[off:off + length].decode("utf-8").strip() for name, off, length in wanted_fields}
        except UnicodeDecodeError as e:
            fail(f"{path.name}: record {i} field decode failed as UTF-8 ({e}) — hard error, not a mangled name")
            raise  # unreachable, fail() exits
        records.append(values)
    return records


# --------------------------------------------------------------------------
# 700 km footprint filter
# --------------------------------------------------------------------------


def haversine_km(lat1: float, lon1: float, lat2: float, lon2: float) -> float:
    p1, p2 = math.radians(lat1), math.radians(lat2)
    dphi = math.radians(lat2 - lat1)
    dlambda = math.radians(lon2 - lon1)
    a = math.sin(dphi / 2) ** 2 + math.cos(p1) * math.cos(p2) * math.sin(dlambda / 2) ** 2
    return 2 * EARTH_RADIUS_KM * math.asin(min(1.0, math.sqrt(a)))


def within_footprint_of_any_site(
    min_lon: float, min_lat: float, max_lon: float, max_lat: float, sites: list[tuple[str, float, float]]
) -> bool:
    for _id, slat, slon in sites:
        clamped_lat = min(max(slat, min_lat), max_lat)
        clamped_lon = min(max(slon, min_lon), max_lon)
        if haversine_km(slat, slon, clamped_lat, clamped_lon) <= FOOTPRINT_KM:
            return True
    return False


def part_bbox(part: list[tuple[float, float]]) -> tuple[float, float, float, float]:
    lons = [p[0] for p in part]
    lats = [p[1] for p in part]
    return min(lons), min(lats), max(lons), max(lats)


# --------------------------------------------------------------------------
# Douglas-Peucker, iterative, primary roads only (ADR-0029 §1, §4.4)
# --------------------------------------------------------------------------


def simplify_part(part: list[tuple[float, float]], epsilon_m: float) -> list[tuple[float, float]]:
    """Douglas-Peucker in local equirectangular metres about the part's own
    bbox-centre latitude. Iterative (explicit stack), not recursive — the
    longest TIGER primary-roads part is 2,826 points against Python's
    default 1,000-frame recursion limit."""
    if len(part) < 3:
        return part

    min_lon, min_lat, max_lon, max_lat = part_bbox(part)
    lat0 = math.radians((min_lat + max_lat) / 2.0)
    r = EARTH_RADIUS_KM * 1000.0

    def to_xy(lon: float, lat: float) -> tuple[float, float]:
        return (r * math.cos(lat0) * math.radians(lon), r * math.radians(lat))

    xy = [to_xy(lon, lat) for lon, lat in part]

    keep = [False] * len(part)
    keep[0] = True
    keep[-1] = True
    stack = [(0, len(part) - 1)]
    while stack:
        start, end = stack.pop()
        if end <= start + 1:
            continue
        (x0, y0), (x1, y1) = xy[start], xy[end]
        dx, dy = x1 - x0, y1 - y0
        seg_len_sq = dx * dx + dy * dy

        best_idx = -1
        best_dist = -1.0
        for i in range(start + 1, end):
            xi, yi = xy[i]
            if seg_len_sq == 0.0:
                dist = math.hypot(xi - x0, yi - y0)
            else:
                t = ((xi - x0) * dx + (yi - y0) * dy) / seg_len_sq
                t = min(1.0, max(0.0, t))
                px, py = x0 + t * dx, y0 + t * dy
                dist = math.hypot(xi - px, yi - py)
            if dist > best_dist:
                best_dist = dist
                best_idx = i

        if best_dist > epsilon_m:
            keep[best_idx] = True
            stack.append((start, best_idx))
            stack.append((best_idx, end))

    return [p for p, k in zip(part, keep) if k]


# --------------------------------------------------------------------------
# Bundle assembly
# --------------------------------------------------------------------------


def deg_to_fixed(deg: float) -> int:
    return round(deg * 1e7)


def build_bundle(
    layer_parts: dict[int, list[list[tuple[float, float]]]],
    labels: list[tuple[float, float, int, str]],  # lon, lat, rank, name
) -> tuple[bytes, dict]:
    layer_table = bytearray()
    part_index = bytearray()
    points = bytearray()
    label_index = bytearray()
    strings = bytearray()

    stats = {"layers": [], "label_count": len(labels), "string_bytes": 0}

    part_cursor = 0
    point_cursor = 0
    total_points = 0
    for kind, name, _stem, _simplify in LAYERS:
        parts = layer_parts[kind]
        first_part = part_cursor
        for part in parts:
            min_lon, min_lat, max_lon, max_lat = part_bbox(part)
            part_index += struct.pack(
                "<IIiiii",
                point_cursor,
                len(part),
                deg_to_fixed(min_lon),
                deg_to_fixed(min_lat),
                deg_to_fixed(max_lon),
                deg_to_fixed(max_lat),
            )
            for lon, lat in part:
                points += struct.pack("<ii", deg_to_fixed(lon), deg_to_fixed(lat))
            point_cursor += len(part)
            total_points += len(part)
            part_cursor += 1
        layer_table += struct.pack("<III", kind, first_part, len(parts))
        stats["layers"].append(
            {"kind": kind, "name": name, "parts": len(parts), "points": sum(len(p) for p in parts)}
        )

    # The label "layer" carries no parts of its own (first_part/part_count
    # are 0 — §5.1) but is still one row in the layer table so an unknown-
    # kind skip and a known-kind draw share the same iteration shape.
    layer_table += struct.pack("<III", LABEL_KIND, 0, 0)

    name_offsets: dict[int, tuple[int, int]] = {}
    for i, (_lon, _lat, _rank, name) in enumerate(labels):
        encoded = name.encode("utf-8")
        name_offsets[i] = (len(strings), len(encoded))
        strings += encoded

    for i, (lon, lat, rank, _name) in enumerate(labels):
        off, length = name_offsets[i]
        label_index += struct.pack(
            "<iiHIH", deg_to_fixed(lon), deg_to_fixed(lat), rank, off, length
        )

    stats["string_bytes"] = len(strings)
    stats["total_points"] = total_points
    stats["total_parts"] = part_cursor

    header = struct.pack(
        "<8sIIIIII",
        MAGIC,
        FORMAT_VERSION,
        len(LAYERS) + 1,  # + the label "layer" row
        part_cursor,
        total_points,
        len(labels),
        len(strings),
    )

    bundle = bytes(header) + bytes(layer_table) + bytes(part_index) + bytes(points) + bytes(label_index) + bytes(strings)
    return bundle, stats


# --------------------------------------------------------------------------
# Main
# --------------------------------------------------------------------------


def main() -> None:
    verify_digests()
    sites = read_sites()

    layer_parts: dict[int, list[list[tuple[float, float]]]] = {}
    for kind, name, stem, simplify in LAYERS:
        shp_path = DATA_DIR / f"{stem}.shp"
        records = read_shp_records(shp_path)
        raw_part_count = sum(len(parts) for _bbox, parts in records)
        kept: list[list[tuple[float, float]]] = []
        for bbox, parts in records:
            if within_footprint_of_any_site(*bbox, sites):
                kept.extend(parts)
        if simplify:
            kept = [simplify_part(p, SIMPLIFY_EPSILON_M) for p in kept]
            kept = [p for p in kept if len(p) >= 2]
        layer_parts[kind] = kept
        print(
            f"{name}: {raw_part_count} raw parts -> {len(kept)} kept parts, "
            f"{sum(len(p) for p in kept)} points"
        )

    # Labels: populated_places points + attributes, footprint-filtered
    # against the single point, sorted SCALERANK asc / POP_MAX desc, ranked
    # densely (ADR-0028 §2, §4.5).
    label_shp = DATA_DIR / "ne_10m_populated_places.shp"
    label_dbf = DATA_DIR / "ne_10m_populated_places.dbf"
    raw_points = read_shp_points(label_shp)
    raw_records = read_dbf_records(label_dbf, ["NAME", "SCALERANK", "POP_MAX"])
    if len(raw_points) != len(raw_records):
        fail(
            f"populated_places: {len(raw_points)} shape records but {len(raw_records)} dbf "
            "records — .shp and .dbf must be the same release"
        )

    kept_labels = []
    for (lon, lat), rec in zip(raw_points, raw_records):
        if not within_footprint_of_any_site(lon, lat, lon, lat, sites):
            continue
        name = rec["NAME"]
        if not name:
            continue
        scalerank = int(rec["SCALERANK"]) if rec["SCALERANK"] else 999
        pop_max = int(rec["POP_MAX"]) if rec["POP_MAX"] else 0
        kept_labels.append((lon, lat, scalerank, pop_max, name))

    kept_labels.sort(key=lambda r: (r[2], -r[3]))
    labels = [(lon, lat, rank, name) for rank, (lon, lat, _sr, _pm, name) in enumerate(kept_labels)]
    print(f"populated_places: {len(raw_points)} raw points -> {len(labels)} kept labels")

    bundle, stats = build_bundle(layer_parts, labels)

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    BUNDLE_OUT.write_bytes(bundle)

    sites_hash = hashlib.sha256(SITES_FILE.read_bytes()).hexdigest()
    manifest_lines = [
        f"format: {MAGIC.decode('ascii')} version {FORMAT_VERSION}",
        f"generator: utility/map-bake/bake.py",
        f"baked: {datetime.date.today().isoformat()}",
        "",
        "sources:",
    ]
    for filename, (digest, url, retrieved, license_) in SOURCES.items():
        if url is None:
            continue
        manifest_lines.append(f"  {filename}: url={url} retrieved={retrieved} license={license_} sha256={digest}")
    manifest_lines += [
        f"  sites_generated.rs: sha256={sites_hash} (700 km footprint filter run against this site table)",
        "",
        "layers:",
    ]
    for layer_stat in stats["layers"]:
        manifest_lines.append(
            f"  kind={layer_stat['kind']} {layer_stat['name']}: parts={layer_stat['parts']} points={layer_stat['points']}"
        )
    manifest_lines += [
        f"  kind={LABEL_KIND} populated_places (labels): count={stats['label_count']} string_bytes={stats['string_bytes']}",
        "",
        f"filter: 700 km from any bundled site (bbox, clamped-point haversine)",
        f"simplification: douglas-peucker, epsilon {SIMPLIFY_EPSILON_M:.0f} m, applied to primary_roads only",
        f"label rank sort key: SCALERANK ascending, POP_MAX descending (dense rank, tie-break)",
        "",
        f"total parts: {stats['total_parts']}",
        f"total points: {stats['total_points']}",
        f"total bundle bytes: {len(bundle)}",
    ]
    MANIFEST_OUT.write_text("\n".join(manifest_lines) + "\n", encoding="utf-8")

    print(f"\nwrote {BUNDLE_OUT} ({len(bundle)} bytes)")
    print(f"wrote {MANIFEST_OUT}")


if __name__ == "__main__":
    main()
