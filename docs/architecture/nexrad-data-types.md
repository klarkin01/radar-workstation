# NEXRAD Level II Data Types — Executive Summary

*A high-level overview of the four data types encountered in the NEXRAD Level II
ecosystem, their differences, and their implications for Radar Workstation,
Meteorological. For byte-level format details, see `CLAUDE.md` and the inspection
utilities in `utility/nexrad-inspect/`.*

---

## The Four Types at a Glance

| Type | Source | Contains | Latency | Retention |
|---|---|---|---|---|
| Volume file | `unidata-nexrad-level2` | Complete volume scan | ~5 min after scan completes | Permanent archive |
| Start chunk (`-S`) | `unidata-nexrad-level2-chunks` | Volume metadata | Seconds after scan begins | 24 hours |
| Intermediate chunk (`-I`) | `unidata-nexrad-level2-chunks` | Radial data (~100°) | Seconds after antenna passes | 24 hours |
| End chunk (`-E`) | `unidata-nexrad-level2-chunks` | Final radial data | Seconds after scan completes | 24 hours |

---

## Volume Files

Volume files are assembled, complete representations of a full volume scan — every
tilt, every radial, from start of scan to end. They are produced by assembling the
real-time chunk stream and stored permanently in the `unidata-nexrad-level2` archive.

**What they contain:** All Message 31 radials for the full volume, plus embedded
metadata and calibration blocks. The format is more complex internally than the chunk
stream — it includes a second layer of BZ2-compressed sub-blocks that must be
unwrapped during decoding.

**Latency:** A volume file does not exist until the scan is complete. For a 5-minute
VCP, the file appears roughly 5 minutes after the antenna begins rotating. Downloading
and decoding adds further delay. Total latency from scan start to displayed data is
typically 6-8 minutes.

**Retention:** Permanent. The full archive extends back to 1991.

**Role in this application:** Secondary source. Used for historical event analysis,
regression testing the decoder against known-good data, and as a fallback when the
chunk stream is unavailable. Not used for real-time operational display.

---

## Start Chunks (`-S`)

The first chunk in a volume scan sequence. Arrives within seconds of the scan beginning,
before any radial data has been collected. Small (typically a few kilobytes compressed).

**What they contain:** Volume-level metadata required to correctly interpret the
incoming radial data:
- **VCP definition** (Message 5) — which elevation angles will be scanned, pulse
  repetition frequencies, and waveform types for this scan
- **RDA Status** (Message 2) — current operational state of the radar hardware
- **Performance/Maintenance data** (Message 3) — transmitter power, receiver noise,
  and other hardware health indicators at scan time
- **RDA Adaptation data** (Message 18) — site-specific hardware calibration constants;
  relatively static, updated infrequently
- **Clutter Filter Map** (Message 15) — which range bins are masked for ground clutter
  suppression; relatively static, updated infrequently

**Latency:** Near-zero. Available within seconds of scan initiation.

**Role in this application:** Volume context establishment. The decoder reads the `-S`
chunk first to initialize a `VolumeContext` — the VCP, calibration constants, and site
parameters that apply to all subsequent radial data in this volume. Messages 2, 3, and
5 are operationally current; Messages 15 and 18 are effectively static reference data.

---

## Intermediate Chunks (`-I`)

The bulk of the chunk stream. Each intermediate chunk contains approximately 120
radials — roughly 100° of azimuthal coverage at one elevation. Multiple intermediate
chunks arrive throughout the scan as the antenna rotates.

**What they contain:** Exclusively Message 31 radial data. Each radial carries:
- Azimuth and elevation angles
- All available dual-polarization moments (REF, VEL, ZDR, PHI, RHO, CFP) with gate
  geometry and scaling parameters
- Tilt number and radial status (start of elevation, intermediate, end of elevation)

A full 360° sweep at one tilt is covered by approximately 3 intermediate chunks (360°
÷ 100° per chunk). A complete VCP with 14 tilts produces roughly 42 intermediate chunks,
plus variation for SAILS cuts and adaptive scanning.

**Latency:** Each chunk appears seconds after the antenna completes that 100° arc.
The first intermediate chunk of the lowest tilt arrives roughly 30-60 seconds after
scan start — long before the volume is complete.

**Role in this application:** Primary operational data. The data pipeline accumulates
intermediate chunks into the in-progress `VolumeScan` and signals the compute layer
as each tilt completes. This is what enables partial-scan rendering and sub-minute
update latency.

---

## End Chunks (`-E`)

The final chunk in a volume scan sequence. Structurally identical to intermediate
chunks in content, but distinguished by a signed negative length prefix — a deliberate
protocol-level signal that this is the last chunk of the volume.

**What they contain:** The final 120 radials of the volume scan — the last ~100° arc
of the highest tilt, or whichever tilt the scan ends on. Same Message 31 format as
intermediate chunks.

**Latency:** Arrives within seconds of the antenna completing its final tilt. At this
point the full volume scan is available in the chunk stream.

**Role in this application:** Volume completion signal. When the data pipeline receives
an end chunk, it decodes the final radials, marks the `VolumeScan` complete, and
triggers final compute and rendering. The negative length prefix is detected during
format identification and handled transparently — the decoder sees the same Message 31
stream as with intermediate chunks.

---

## Implications for the Data Pipeline

The chunk stream imposes a natural state machine on the data pipeline:

```
IDLE
  │  ← -S chunk arrives
  ▼
AWAITING_DATA (VolumeContext initialized)
  │  ← -I chunks arrive continuously
  ▼
ACCUMULATING (radials added to VolumeScan per chunk)
  │  ← tilt completes (radial status = End of Elevation)
  ├─► signal compute layer: render this tilt
  │  ← -E chunk arrives
  ▼
COMPLETE (VolumeScan finalized, full render triggered)
  │
  ▼
IDLE (await next -S chunk)
```

The pipeline must handle missing chunks gracefully — a dropped `-I` chunk should result
in a gap in azimuthal coverage for that tilt, not a stalled or crashed pipeline. The
`VolumeScan` struct should represent absent data explicitly rather than blocking on it.
