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

| Script | Purpose |
|---|---|
| `nexrad-inspect/inspect_header.py` | Dump and summarize Level II file headers |

---

## nexrad-inspect/

Python utilities for inspecting NEXRAD Level II archive files using
[MetPy](https://unidata.github.io/MetPy/latest/index.html) as a well-tested
independent decoder. The primary use case is cross-validating the Rust decoder
in `crates/radar-decoder` against MetPy's output, field by field.

### Dependencies

```
pip install metpy numpy
```

No virtual environment is required for simple use, though one is recommended
if you're working across multiple projects.

### inspect_header.py

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
python inspect_header.py /path/to/KXXX20240501_120000_V06

# Detailed radial headers for sweep 0 (first 10 radials)
python inspect_header.py /path/to/file --sweep 0

# More radials
python inspect_header.py /path/to/file --sweep 0 --radials 50

# Moment statistics for sweep 2
python inspect_header.py /path/to/file --sweep 2 --moments

# Raw namedtuple dump (useful for finding field names during decoder development)
python inspect_header.py /path/to/file --raw

# All of the above at once
python inspect_header.py /path/to/file --sweep 0 --radials 20 --moments --raw
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

- **NOAA NEXRAD S3 archive (free, no auth):**
  `s3://noaa-nexrad-level2/<YYYY>/<MM>/<DD>/<SITE>/`
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
