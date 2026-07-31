# utility/

Development utilities for Radar Workstation, Meteorological.

These scripts are **not part of the product**. They exist to support development
activities: cross-validating the Rust decoder, exploring Level II file structure,
generating test fixtures, and performing spot-checks against known-good data.

They carry no stability guarantee, no versioning contract, and no production
support obligation. They may be incomplete, opinionated, or written for a single
session's purpose.

---

## Contents

| Path | Purpose |
|---|---|
| `nexrad-inspect/metpy_inspect_header.py` | Dump and summarize Level II file headers (site constants, sweep summary, radial detail) |
| `nexrad-inspect/metpy_inspect_metadata.py` | Inspect NEXRAD Level II metadata fields via MetPy |
| `nexrad-inspect/metpy_inspect_compression.py` | Check the internal compression format of a Level II file |
| `nexrad-inspect/inspect_chunk_start.py` | Decompress and inspect a real-time LDM start (`-S`) chunk |
| `nexrad-inspect/inspect_geometry.py` | Inspect NEXRAD scan geometry (gate spacing, sweep structure) |
| `nexrad-inspect/inspect_messages.py` | Dump raw NEXRAD Level II messages |
| `nexrad-inspect/nexrad_msg31.py` | Reference Python decoder for Message 31 radials (ICD 2620002), used as a cross-check oracle |
| `nexrad-inspect/gen_fixtures.py` | Generate binary test fixtures for `nexrad-decoder`'s unit tests |
| `nexrad-sample/` (Rust) | Fetch (`fetch-sample`) and decode (`decode-sample`) NEXRAD chunk files from S3 for manual inspection |
| `radar-viz/` (Rust) | Render a decoded volume scan to a PNG PPI image for visual verification — see its own README |

---

## nexrad-inspect/

Python utilities for inspecting NEXRAD Level II archive files using
[MetPy](https://unidata.github.io/MetPy/latest/index.html) as a well-tested
independent decoder. The primary use case is cross-validating the Rust decoder
in `crates/nexrad-decoder` against MetPy's output, field by field.

### Dependencies

```
pip install metpy numpy
```

No virtual environment is required for simple use, though one is recommended
if you're working across multiple projects.

### metpy_inspect_header.py

Reads a Level II archive and prints a structured summary of:

- Volume header (station ID, timestamp)
- Site constants (lat/lon, AMSL, feedhorn AGL, calibration values, TX power, VCP)
- Sweep summary table (elevation angle, radial count, available moments per sweep)
- Per-sweep radial header detail (azimuth, elevation, radial status)
- Per-moment gate geometry (first gate, gate width, gate count)
- Per-moment data statistics (valid gate count, min/max/mean/std)
- Raw namedtuple dump of the first radial for deep inspection

**Basic usage:**

```bash
# Print volume header, site constants, and sweep summary
python metpy_inspect_header.py /path/to/KXXX20240501_120000_V06

# Detailed radial headers for sweep 0 (first 10 radials)
python metpy_inspect_header.py /path/to/file --sweep 0

# More radials
python metpy_inspect_header.py /path/to/file --sweep 0 --radials 50

# Moment statistics for sweep 2
python metpy_inspect_header.py /path/to/file --sweep 2 --moments

# Raw namedtuple dump (useful for finding field names during decoder development)
python metpy_inspect_header.py /path/to/file --raw

# All of the above at once
python metpy_inspect_header.py /path/to/file --sweep 0 --radials 20 --moments --raw
```

Accepts `.ar2v`, `.gz`, and `.bz2` files. MetPy handles decompression
transparently, including the internal BZ2 chunked format used in real-time
network distribution.

---

## Data files

Level II archive files (`.ar2v`, `.gz`) are **not tracked in this repository**.
They are large, they are freely available from NOAA's public S3 archive, and
committing them would pollute the project history.

See `.gitignore` for the exclusion patterns.

Sample files can be obtained from:

- **Unidata NEXRAD S3 archive (free, no auth, current):**
  `s3://unidata-nexrad-level2/<YYYY>/<MM>/<DD>/<SITE>/` for assembled volume files, or
  `s3://unidata-nexrad-level2-chunks/<SITE>/<volume-sequence>/<YYYYMMDD-HHMMSS>-<n>-<kind>`
  for real-time chunks, e.g. `KDOX/166/20260728-095259-001-S` (chunks persist 24 hours).
  `<volume-sequence>` is an **unpadded**, monotonically increasing per-site integer
  identifying the volume scan — it is not derivable from wall-clock time, and its
  lexical order does not match numeric order across digit widths (`"78"` sorts after
  `"709"`), so listing the bucket flatly and sorting by key does not give chronological
  order. Within one volume's directory the fixed-width `<timestamp>-<n>-<kind>`
  filenames do sort chronologically. See `docs/architecture/nexrad-binary-format.md`
  for the file-level format and ADR-0014 erratum item 8 for the provenance of this
  correction. The legacy `noaa-nexrad-level2` bucket stopped receiving updates
  September 1, 2025 but retains historical data through that date.
  Browse at [https://registry.opendata.aws/noaa-nexrad/](https://registry.opendata.aws/noaa-nexrad/)

- **Iowa State IEM archive:**
  [https://mesonet.agron.iastate.edu/archive/](https://mesonet.agron.iastate.edu/archive/)

A small number of well-chosen sample files (specific events, known edge cases)
may be stored locally for consistent regression testing, but they live outside
the repository on the developer's machine.

---

## Adding new utilities

Create a subdirectory named for the utility's domain (e.g., `nexrad-inspect/`,
`data-gen/`, `vcp-analysis/`). Add a brief entry to the table above. If the
utility has its own dependencies beyond the base Python scientific stack, note
them in a `requirements.txt` inside the subdirectory.

Utilities may be written in Python, shell, or any language convenient for the
task. There is no requirement for consistency across utilities.

---

## Relationship to the product

Nothing in `utility/` is linked to, imported by, or depended on by any crate
in `crates/`. If a utility produces logic that belongs in the product (a
parsing heuristic, a calibration formula), that logic should be re-implemented
in Rust within the appropriate crate, with the utility script serving only as
the reference or test oracle.
