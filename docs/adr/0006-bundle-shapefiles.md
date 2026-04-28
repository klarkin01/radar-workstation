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
