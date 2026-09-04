# Tile corpus — captured 2026-08-28

**Nothing in this workspace reads these files yet, and that is deliberate.**

The tile subsystem is deferred to post-v1.0 by
[ADR-0027](../../../../../docs/adr/0027-tile-image-decoding.md). This corpus is the
evidence behind that decision and the re-validation baseline for the day it is picked up:
ADR-0027 §2 records *what* to build, and these files are what its measurements were taken
against. Returning to the subsystem should be a diff against this corpus, not a fresh
round of provider probing.

Delete this directory only together with ADR-0027 §2.

## Contents

60 unique tiles, 1.2 MB, captured from four providers over zoom levels 3–16:

| Provider | Tiles | Note |
|---|---|---|
| USGS National Map (`basemap.nationalmap.gov`) | 56 | The ADR-0007 default. Five services: `USGSTopo`, `USGSImageryOnly`, `USGSImageryTopo`, `USGSShadedReliefOnly`, `USGSHydroCached` |
| ArcGIS Online (`server.arcgisonline.com`) | 1 | `World_Imagery` |
| OpenStreetMap (`tile.openstreetmap.org`) | 1 | Palette PNG — the case `png` fails on without `Transformations::EXPAND` |
| Carto (`basemaps.cartocdn.com`) | 1 | Palette PNG |

49 JPEG, 11 PNG. `manifest.json` carries per-tile SHA-256, byte size, format, source URL,
and — where the same bytes were served from more than one URL — every URL that produced
them.

`synthetic-bomb-30000x30000.png` is **not** from a provider. It is a hand-built 545-byte
PNG whose `IHDR` declares 30000×30000 RGBA, which is 3.4 GB decoded. It exercises the
dimension gate in ADR-0027 §2, which must reject it at the header before allocating.

`scan.py` reports the JPEG marker profile (SOF type, precision, components, sampling
factors, restart interval, scan count) and PNG chunk profile (bit depth, colour type,
interlace, chunk list) for any file given to it. It produced the Measurement 2 and
Measurement 3 tables in ADR-0027:

```
python3 scan.py *.jpg *.png
```

## What the corpus establishes

- **Image format is a per-tile property, not a per-provider one.** Four of the five USGS
  services serve both JPEG and PNG from one URL template, interleaved by zoom, and not
  only for blank tiles — `usgs-usgsimageryonly-z9-y195-x148.png` is 169 KB of real
  content with JPEG tiles on either side of it in the zoom stack.
- **Every JPEG here is the same profile**: SOF0 baseline, 8-bit, 3 components, 4:4:4, no
  restart markers, single scan, JFIF only. No progressive JPEG appears anywhere.
- **PNGs are 8-bit RGBA non-interlaced (USGS) or 8-bit palette (OSM, Carto).**

These are observations of one day's cache, not guarantees. ADR-0027's Consequences say so
explicitly, and the re-validation is the point of keeping the manifest digests.
