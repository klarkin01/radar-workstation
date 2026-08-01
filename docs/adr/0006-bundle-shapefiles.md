# ADR-0006: Bundle Shapefiles for Basemap Vector Data

## Status
Accepted

## Context
The application requires vector overlay data for counties, states, country boundaries,
coastlines, and major highways. This data must be available immediately at startup, with
no network dependency, and must render correctly at all zoom levels. Options considered
were: runtime tile server (MapLibre, MapTiler, Mapbox), embedded vector tiles, and
bundled shapefiles loaded at startup.

## Decision
Census TIGER/Line shapefiles (counties, states, highways) and Natural Earth data (country
boundaries, coastlines) are bundled with the application binary. NEXRAD site locations
are bundled as a JSON file derived from the NOAA site list. All vector data is loaded once
at startup and tessellated into GPU geometry held in memory for the lifetime of the process.

## Consequences
- Zero network dependency for basemap vector data. The application works fully offline
  for all overlay layers.
- No API key required for any vector data layer.
- Consistent with the security requirement of no undisclosed network connections.
- Startup includes a one-time tessellation cost. This is acceptable and should complete
  in under one second on modern hardware.
- Bundled data increases binary/package size by approximately 30-80MB depending on
  geographic scope and geometry simplification level. This is acceptable.
- Basemap data does not change frequently. Updates (e.g. new NEXRAD sites, county
  boundary changes) are handled via application releases, not runtime fetching.
- The `geo` and `shapefile` crates handle file parsing. `lyon` handles tessellation of
  geographic polygons into GPU-ready triangle meshes.

## Erratum (added during Stage 2, 2026-07-31)

1. **NEXRAD site locations are a generated `const` Rust table, not a bundled JSON file
   parsed at startup.** This ADR's Decision and Consequences sections say "NEXRAD site
   locations are bundled as a JSON file derived from the NOAA site list." Implementing
   that literally means adding `serde_json` (or a hand-rolled JSON parser) to the
   application's startup path for data that never changes at runtime — a startup path
   that can fail to parse data the project ships itself, for no benefit (Stability as
   Ethics). Instead, `utility/nexrad-sites/generate.py` converts a committed NOAA HOMR
   station export (see `utility/README.md` for provenance) into
   `crates/radar-workstation/src/sites_generated.rs`, a `pub static SITES: &[Site]`
   compiled directly into the binary. Zero dependencies, zero runtime parse step, zero
   startup failure mode, and the table's shape is checked by the compiler rather than by
   a parser. See `docs/open-questions.md` Q4 (Resolved) and
   `docs/adr/0018-shared-application-state.md`, which records this alongside the Q4
   decision it shipped with. This erratum applies only to the *NEXRAD site list*; the
   Decision's shapefile/vector-overlay bundling (counties, states, highways, coastlines,
   loaded via `geo`/`shapefile`/`lyon`) is unaffected and remains open per Q15.
