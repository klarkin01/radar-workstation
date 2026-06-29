# ADR-0012: Volume Assembly State Machine and Missing Chunk Handling

## Status
Accepted

## Context
The real-time chunk stream (established as the primary data source in ADR-0011) delivers
volume scan data as a sequence of three chunk types: one `-S` (start), N × `-I`
(intermediate), and one `-E` (end). The data pipeline must assemble these into a
`VolumeScan` struct incrementally as chunks arrive.

Unlike assembled volume files, the chunk stream offers no atomic "here is a complete
volume" guarantee. Chunks may arrive out of order, be delayed, or be absent entirely
due to transient S3 delivery failures, network issues between the RPG and the LDM feed,
or radar hardware faults mid-scan. The pipeline must remain functional under all of
these conditions.

The stability-as-ethics principle (PHILOSOPHY.md) applies directly: a crash or hang
during a tornado warning because a single chunk was dropped is an unacceptable failure
mode.

## Decision
The data pipeline implements an explicit volume assembly state machine with defined
behavior for each missing chunk type. Volume completion is detected from multiple
signals, not solely from the `-E` chunk. The `VolumeScan` struct carries a completion
status field distinguishing complete from incomplete volumes.

## State Machine

```
IDLE
  │  ← -S chunk arrives
  ▼
AWAITING_DATA
  │  (VolumeContext initialized from -S metadata)
  │  ← -I chunk arrives
  ▼
ACCUMULATING
  │  ← -I chunks continue arriving; radials accumulated per tilt
  │
  │  Tilt closure signals (any one is sufficient):
  ├─ (a) radial with status End of Elevation arrives
  │       → close current tilt, signal compute layer
  ├─ (b) radial with new elevation number arrives (no End of Elevation seen)
  │       → close previous tilt as incomplete, signal compute layer, begin new tilt
  │
  │  Volume exit conditions:
  │
  ├─ (1) -E chunk arrives
  │       decode final radials, mark VolumeScan Complete
  │       → IDLE
  │
  ├─ (2) new -S chunk arrives before -E
  │       close current tilt and VolumeScan as ClosedByNextVolume
  │       → AWAITING_DATA (begin new volume immediately)
  │
  └─ (3) watchdog timeout (no chunk for ~10-15 minutes)
          close current tilt and VolumeScan as ClosedByTimeout
          → IDLE
```

**Late data discard rule:** any radial whose elevation number matches an already-closed
tilt is discarded. Tilt closure is permanent. Once a tilt has been handed to the compute
layer it is immutable — retroactively modifying a rendered tilt would require re-running
the compute pass, introduce texture synchronization hazards, and silently change data
already displayed to the user. A visible gap is preferable to invisible data modification.

## VolumeScan Completion Status

The `VolumeScan` struct carries an explicit completion status:

- **`Complete`** — `-E` chunk received; all data present or accounted for
- **`ClosedByNextVolume`** — a new `-S` arrived before `-E`; final chunk(s) absent
- **`ClosedByTimeout`** — no chunk activity within the watchdog period; final state unknown
- **`InProgress`** — currently accumulating; not yet closed

The render loop and compute layer may use this field to decide whether to display a
visual indicator that a volume is incomplete. No data is withheld or suppressed — a
`ClosedByNextVolume` volume is rendered as-is, with gaps where data is absent.

## Missing Chunk Behavior by Type

**Missing `-I` chunk**

Impact: a gap in azimuthal coverage for that tilt — typically ~100° of missing radials.

Handling: the `VolumeScan` represents absent azimuths explicitly as missing data, not
as zeroes or interpolated values. The pipeline continues accumulating subsequent chunks
without stalling. The render loop draws nothing for absent gates, or optionally renders
a subtle coverage indicator. No interpolation or gap-filling is performed — this would
misrepresent the data.

**Missing `-S` chunk**

Impact: no pre-scan volume context. The VCP definition, calibration constants, and
current RDA status are unavailable before radial data begins arriving.

Handling: context initialization is deferred to the first `-I` chunk. The VCP number
and site calibration constants are present in every Message 31 radial's RVOL block —
the `-S` provides them earlier, but is not their only source. Static reference data
(clutter filter map, adaptation data from Messages 15 and 18) is carried forward from
the previous volume if available, or omitted if this is the first volume of a session.
A warning is logged. Decoding proceeds.

**Missing `-E` chunk**

Impact: the final ~100° of the last tilt is absent, and the explicit end-of-volume
signal is never received.

Handling: volume completion is detected by two additional signals that do not depend
on `-E` arrival:

1. **Next `-S` arrival.** The start of a new volume implicitly closes the previous one.
   This is the primary fallback. The current `VolumeScan` is marked `ClosedByNextVolume`
   and handed to the compute layer. The new `-S` immediately initializes the next volume.

2. **Watchdog timeout.** If no chunk arrives within approximately 10-15 minutes (well
   beyond the longest VCP cycle), the current `VolumeScan` is marked `ClosedByTimeout`
   and the pipeline returns to IDLE. This handles session-end and prolonged outages.

Note: the end-of-volume radial status flag (code 4) is carried in the final radial of
the `-E` chunk's Message 31 stream. In the missing `-E` case this flag is never
received, which is why `-S` arrival and timeout are the fallback signals rather than
attempting to infer completion from radial status alone.

## Considered Alternatives

**Waiting window for end-of-elevation signal**

A waiting window was considered: when a new elevation number is detected without a
prior End of Elevation radial, hold the previous tilt open for a short period (e.g.
2-3 seconds) before closing it, in case the end-of-elevation chunk arrives late.

This was rejected for three reasons:

1. **Out-of-order chunk delivery is extremely rare in practice.** The Unidata LDM
   infrastructure assembles and writes chunks to S3 sequentially. The scenario this
   window defends against — a chunk arriving out of sequence — is not a realistic
   operational failure mode. The more likely cause of a missing end-of-elevation
   signal is a dropped chunk, meaning the data genuinely never arrived. No waiting
   window recovers from that.

2. **It degrades latency unconditionally.** Every tilt would render slightly later,
   even when nothing is missing, in order to defend against a failure mode that almost
   never occurs. This undermines the primary advantage of the chunk stream over
   assembled volume files.

3. **It adds implementation complexity for no observed benefit.** A waiting window
   requires a per-tilt timer, a tentatively-closed tilt state, and a late-data
   reconciliation path. This complexity must be maintained and tested. If operational
   experience reveals that out-of-order delivery is causing visible gaps in practice,
   a waiting window can be added then. It is not warranted upfront.

## Consequences

- The `VolumeScan` struct must represent absent data explicitly. Missing radials are a
  first-class state, not an error condition. Panic or unwrap on absent data is
  prohibited in the decoder and compute layer.

- The data pipeline requires a watchdog timer, independent of the chunk receive path,
  to handle the timeout closure case.

- The compute layer and render loop receive `VolumeScan` structs that may be incomplete.
  Both must be written to handle partial volumes without assuming full coverage.

- Partial-scan rendering — displaying completed tilts before the volume is closed — is
  an explicit design goal, not a side effect. Each tilt closure triggers an incremental
  render signal regardless of overall volume completion status.

- Tilt closure is permanent. The compute layer and render loop must never assume a
  closed tilt will be modified after it has been handed off.

- Late-arriving radials for closed tilts are discarded and logged. This is the correct
  behavior: a visible gap is more honest than silent retroactive data modification.

- The pipeline never blocks waiting for a missing chunk or a missing end-of-elevation
  signal. Forward progress is always possible given any subset of the chunk stream.
