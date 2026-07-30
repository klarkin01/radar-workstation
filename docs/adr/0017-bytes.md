# ADR-0017: Use bytes for Zero-Copy Buffer Handoff

## Status
Accepted

## Context
NEXRAD chunks arrive over the network, get handed from the acquisition layer
(`http_ingest::Client::get_object`) to the framing layer (`ChunkEnvelope`, `chunk.rs`)
to the decoder (`nexrad-decoder`), each a separate crate boundary. A chunk body can be
on the order of 100–200 KB; copying it at each handoff is pure waste on a path that
already runs once per ~5-second poll per active radial batch, with no correctness benefit.

`bytes::Bytes` was already part of the workspace (`ChunkEnvelope::raw_bytes`) before this
ADR existed, flagged by the dependency audit behind ADR-0014 as an undocumented
dependency decision. This ADR closes that gap.

## Decision
The `bytes` crate (`bytes = "1"`) is used for buffer handoff across crate boundaries in
the acquisition and framing layers. `http_ingest::Client::get_object` and `list_prefix`
both return `Bytes`; `ChunkEnvelope::raw_bytes` is `Bytes`.

`Bytes` is a reference-counted, cheaply-clonable, immutable view over a shared buffer —
cloning it is an atomic refcount bump, not a copy, and sub-slicing (as `http-ingest`'s
connection layer does when splitting a keepalive read buffer between one response's body
and the next response's leftover bytes) is also zero-copy.

## Consequences
- `bytes` and its transitive dependencies were already present in the workspace via
  `tokio` and (formerly) `reqwest`; this ADR adds no new transitive dependency weight.
  It remains part of the graph after ADR-0014's `reqwest` removal because `http-ingest`
  depends on it directly (see ADR-0014 §3.1).
- Buffers are immutable once received from the network. Any component that needs to
  mutate chunk data (none currently do) would need to copy out of the `Bytes` explicitly
  — this is the standard, intentional tradeoff of the type.
- `nexrad-decoder` (ADR-0008) does not depend on `bytes` — it operates on `&[u8]`
  slices borrowed from the caller's `Bytes`, keeping the decoder's own dependency list
  at zero. Zero-copy handoff stops at the decoder boundary by design.
