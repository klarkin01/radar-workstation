# ADR-0011: Target the Real-Time Chunk Stream as Primary Data Source

## Status
Accepted

## Context
NEXRAD Level II data is available from two distinct sources on the Unidata AWS S3
infrastructure:

**Assembled volume files** (`unidata-nexrad-level2`)
Complete volume scans assembled from chunk data and stored as a single file per scan.
These are the files GR2Analyst and most third-party radar applications consume. A new
file appears roughly every 4-6 minutes (one VCP cycle). The file must be fully available
before it can be downloaded and decoded.

**Real-time chunk stream** (`unidata-nexrad-level2-chunks`)
Individual data chunks delivered to S3 as the radar antenna rotates. Each chunk
represents approximately 120 radials (~100° of azimuthal coverage at one tilt). Chunks
are available within seconds of the antenna completing that portion of the scan. Chunks
persist for a maximum of 24 hours before being purged.

Three chunk types exist per volume scan:
- `-S` (start): Volume metadata — VCP definition, site calibration, clutter map, RDA
  status. Contains no radial data. One per volume.
- `-I` (intermediate): Pure Message 31 radial data, BZ2-compressed. 120 radials per
  chunk. Multiple per volume (count varies by VCP, typically 80-100).
- `-E` (end): Final radial data chunk, identical in content to `-I` chunks but with a
  signed negative length prefix as an end-of-volume signal. One per volume.

The chunk stream format was empirically validated using the inspection utilities in
`utility/nexrad-inspect/`. Key findings:
- `-I` and `-E` chunks decompress to pure Message 31 streams with no additional
  complexity.
- `-S` chunks contain the context needed to correctly interpret the radial data.
- A complete volume = one `-S` + N × `-I` + one `-E`, consumed in sequence.

## Decision
The real-time chunk stream (`unidata-nexrad-level2-chunks`) is the primary data source
for the application. The data pipeline consumes chunks as they arrive and begins
rendering as soon as the first complete tilt is available.

Assembled volume files (`unidata-nexrad-level2`) are retained as a secondary source
for non-real-time use cases: loading historical events, testing, and fallback if the
chunk stream is unavailable.

## Rationale

**Latency.** The lowest elevation tilt (0.5°) completes approximately 30-60 seconds
into a volume scan. Chunk-based ingestion allows this tilt to be displayed while the
antenna is still scanning higher elevations. Volume-based ingestion requires waiting
for the entire scan to complete — up to 5 minutes later. During rapidly evolving
severe weather events this latency difference is operationally significant.

**Alignment with operational practice.** NWS meteorologists working on AWIPS workstations
consume the LDM stream directly and see each tilt as it completes. GR2Analyst's
volume-based approach is an implementation simplification that trades latency for
development convenience. This application targets the same operational context as AWIPS
and should match its data currency.

**Simpler decoded format.** Assembled volume files contain type 32 messages that wrap
internally BZ2-compressed sub-blocks, requiring a second decompression pass. Intermediate
chunks are pure Message 31 streams — after a single BZ2 decompression they require no
further unwrapping. The chunk format is the cleaner decoding target.

**Natural pipeline fit.** The chunk stream maps cleanly onto the application's
streaming data pipeline architecture. Each chunk can be decoded independently as it
arrives and its radials accumulated into the in-progress `VolumeScan`. The volume-based
approach requires downloading a multi-megabyte file in full before any decoding begins,
which is incompatible with progressive rendering.

Iowa State Mesonet was considered as a fallback source but provides no capability distinct from the two NOAA sources — it is an archive without a real-time chunk stream. It is not supported.

## Consequences

**Decoder must handle all three chunk types.** The `-S`, `-I`, and `-E` chunks have
distinct binary structures:
- `-S`: 24-byte volume header + unsigned 4-byte length prefix + BZ2 data
- `-I`: unsigned 4-byte length prefix + BZ2 data
- `-E`: signed negative 4-byte length prefix + BZ2 data (abs value = compressed size)

Format is detected from the first 4 bytes: `AR2V` → assembled volume; negative signed
int with `BZh9` at offset 4 → `-E` chunk; unsigned int with `BZh9` at offset 4 → `-I`
chunk; `BZh9` at offset 28 → `-S` chunk.

**Volume state machine required.** The data pipeline must track volume assembly state:
awaiting `-S`, accumulating `-I` chunks, and detecting `-E` to signal completion. Chunk
sequence numbers (embedded in the S3 object key) provide ordering. The pipeline must
handle missing chunks gracefully without stalling.

**Partial scan rendering.** The render loop must be capable of displaying an in-progress
volume — a `VolumeScan` that has some tilts fully populated and others absent. This is
a feature, not a limitation: users see the lowest tilt within ~60 seconds of scan start
rather than waiting for the full volume.

**24-hour chunk retention.** Chunks are purged after 24 hours. Historical analysis
(loading past events) must use the assembled volume archive. The application should
switch data sources transparently based on whether the requested time is within the
chunk retention window.

**No impact on the decoder library.** The decoder crate (`radar-decoder`) operates on
decompressed Message 31 byte streams regardless of how those bytes arrived. The chunk
format complexity is entirely contained in the data acquisition layer. The decoder sees
the same input whether the source was a chunk or an assembled volume file.
