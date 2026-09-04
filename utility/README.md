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
| `nexrad-sites/generate.py` | Generate `crates/radar-workstation/src/sites_generated.rs` (the bundled WSR-88D site table) from `nexrad-sites/data/nexrad-stations.txt` |
| `map-bake/bake.py` | Generate `crates/radar-workstation/src/overlay/overlay.bin` and `bundle.manifest.txt` (the map underlay bundle: counties, states/provinces, coastline, primary roads, city labels) from five shapefile sources |

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

## nexrad-sites/

Generates the bundled WSR-88D site table (`crates/radar-workstation/src/sites_generated.rs`,
S2-W3, FR-MU-3/FR-SS-1) from a committed NOAA station export. A generated `const` Rust
table, not a bundled JSON file parsed at startup — see the dated erratum in
[`docs/adr/0006-bundle-shapefiles.md`](../docs/adr/0006-bundle-shapefiles.md) and
[`docs/adr/0018-shared-application-state.md`](../docs/adr/0018-shared-application-state.md).

**Source data:** `data/nexrad-stations.txt`, retrieved 2026-07-31 from
`https://www.ncei.noaa.gov/access/homr/file/nexrad-stations.txt` — NOAA NCEI's Historical
Observing Metadata Repository (HOMR) NEXRAD station export. U.S. federal government data,
public domain. Filtered to `STNTYPE == NEXRAD` (163 sites at retrieval time), which excludes
the co-listed TDWR sites (Restraint is a Feature — TDWR is out of scope). Includes a handful
of overseas DoD-operated WSR-88D sites (Kadena, Kunsan, Osan/Humphreys, Lajes) with a blank
`state` field in the source data; whether these publish to the real-time chunk bucket is
exactly what the `#[ignore]`d `bucket_site_prefixes_match_bundled_site_list` live test (in
`crates/radar-workstation/src/ingest/s3_poll.rs`) checks.

**Regenerate:**

```bash
curl -o utility/nexrad-sites/data/nexrad-stations.txt \
  https://www.ncei.noaa.gov/access/homr/file/nexrad-stations.txt
python3 utility/nexrad-sites/generate.py
```

The generator asserts the source file's fixed-width column layout matches what it expects
before parsing, so a reflowed source format fails loudly at generation time rather than
silently misparsing a field.

---

## map-bake/

Generates the map underlay bundle (`crates/radar-workstation/src/overlay/overlay.bin` and
`bundle.manifest.txt`, S5-W1/W2, ADR-0025, ADR-0028, ADR-0029) from five shapefile sources.
stdlib-only Python — no `shapefile`/`dbase`/`geo`/`lyon` dependency, following the same
"own the boundary rather than install a parser to answer a question about not shipping
one" reasoning `nexrad-sites/generate.py` and the fixture generators already follow.
Digest-verified before it emits anything: a substituted or re-downloaded source fails
loudly rather than being baked silently.

**Sources** — four already on disk (`.gitignore`d) in `utility/radar-viz/data/`; the
fifth must be downloaded before baking:

| File | Source | Retrieved | License |
|---|---|---|---|
| `tl_2025_us_primaryroads.{shp,dbf}` | [Census TIGER/Line](https://www2.census.gov/geo/tiger/TIGER2025/PRIMARYRD/) | 2025-09-15 | Public domain |
| `ne_10m_admin_1_states_provinces.{shp,dbf}` | [Natural Earth](https://naciscdn.org/naturalearth/10m/cultural/) | 2022-05-09 | Public domain |
| `ne_10m_admin_2_counties_lakes.{shp,dbf}` | [Natural Earth](https://naciscdn.org/naturalearth/10m/cultural/) | 2022-05-09 | Public domain |
| `ne_10m_coastline.{shp,dbf}` | [Natural Earth](https://naciscdn.org/naturalearth/10m/physical/) | 2021-11-14 | Public domain |
| `ne_10m_populated_places.{shp,dbf}` | [Natural Earth](https://naciscdn.org/naturalearth/10m/cultural/) | 2026-09-02 | Public domain |

Exact SHA-256 digests for every source are recorded in `map-bake/bake.py`'s `SOURCES`
table and in the committed `bundle.manifest.txt` — the digest check is what makes a
regeneration from substituted inputs fail loudly rather than baking silently.

**Regenerate:**

```bash
curl -o utility/radar-viz/data/ne_10m_populated_places.zip \
  https://naciscdn.org/naturalearth/10m/cultural/ne_10m_populated_places.zip
(cd utility/radar-viz/data && unzip -o ne_10m_populated_places.zip)
python3 utility/map-bake/bake.py
```

The generator prints per-layer part/point counts, filters every layer to within 700 km of
any bundled site's bounding box, simplifies the primary-roads layer only (Douglas–Peucker,
ε = 30 m, iterative — ADR-0029 §1), and writes both the bundle and its manifest. It parses
`crates/radar-workstation/src/sites_generated.rs` for the footprint filter's site table
(the same site table `nexrad-sites/generate.py` produces), asserting the parsed count is
163 so a reformatted generated file fails loudly rather than silently filtering against
the wrong sites.

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
