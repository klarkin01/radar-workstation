# ADR-0005: Use rayon for Compute Parallelism

## Status
Accepted

## Context
Deriving products from a NEXRAD volume scan — Echo Tops, VIL, VILD, dual-pol products,
and others — is CPU-bound work that is embarrassingly parallel across the radial grid.
This computation must not block the UI or render loop. A manual threading approach was
considered but rejected in favor of a data parallelism abstraction that handles thread
pool management automatically.

## Decision
rayon is used for all CPU-bound product computation.

## Consequences
- Parallel iterators allow product derivation code to be written in a straightforward,
  readable style while automatically distributing work across available CPU cores.
- rayon manages its own thread pool, separate from tokio's async executor. The two do
  not interfere.
- Product computation is fire-and-forget from the perspective of the data pipeline:
  a new volume scan arrives, computation is dispatched to rayon, results are written
  to shared application state when complete.
- rayon is a mature, widely used crate with an excellent safety record. It fits the
  conservative dependency philosophy.
- CPU utilization during product computation will spike across cores. This is expected
  and correct behavior. The lightweight-per-instance requirement applies to idle and
  steady-state operation, not to the brief computation burst following a new scan.

## Erratum (added 2026-08-05, Stage 3 / S3-f)

**Adoption is deferred pending measurement, not reversed.** Stage 3
(`docs/plans/stage-3-compute-layer.md` §3.6) implemented gridding and Echo Tops/VIL on
`tokio::task::spawn_blocking` with plain iterators instead of adding rayon, and measured
the actual cost rather than assuming it:

| Pass | Measured (release, real KDOX VCP 35 volume, 16 elevations, 9360 radials) | §3.6's revisit trigger |
|---|---|---|
| Gridding, per sweep | 0.66 ms – 2.5 ms (mean 1.43 ms) | ~50 ms |
| Gridding, full volume (16 sweeps, 3–5 products each) | 22.9 ms total | — |
| Echo Tops + VIL, per volume | **595.6 ms** | ~500 ms |

Gridding is nowhere near its trigger — it is close to a `memcpy`, exactly as predicted,
against sweeps arriving roughly twenty seconds apart. **Echo Tops/VIL crossed the
trigger**: 595.6 ms against the ~500 ms threshold this ADR's Stage 3 plan set in advance.
The cause is structural, not an implementation accident: both derived products walk
every output cell (up to 720 × 1832) against every retained tilt (up to sixteen),
calling the trigonometric beam-geometry conversion (`compute::geometry::
slant_range_and_height`) per (cell, tilt) pair — on the order of 10⁷ geometry
evaluations for a full VCP 35 volume, each several `sin`/`cos`/`asin` calls deep.

**This is the recorded trigger to revisit rayon — scoped to the derived-products pass
specifically, not to gridding.** A full volume closes only every 4–6 minutes (clear air)
or 1–2 minutes (precipitation mode), so 595.6 ms is not yet a correctness problem (it is
comfortably under even the tightest inter-volume interval), and the live end-to-end test
(`tests/pipeline_live.rs`) confirmed `IngestStatus` stayed `Polling` throughout a real
run — the poller was not starved. But it is close enough to the next VCP's shortest
cycle that it is the first place to look before the margin narrows further (a busier
VCP, a slower host, or a fourth simultaneous instance per NFR-P-1 sharing the same
cores). The lowest-cost fix likely does not need rayon at all: `slant_range_and_height`
is called with the *same* `(ground_m, elevation_deg)` pair for every azimuth at a given
gate index and tilt, so precomputing one range/height table per tilt (indexed by gate)
instead of recomputing it per `(azimuth, gate)` cell removes the redundant trigonometry
before parallelism is even considered. If that restructuring alone does not bring the
volume-derived pass back under the trigger, rayon's parallel iterators over the output
grid's rows are the next lever — Echo Tops and VIL are exactly the "embarrassingly
parallel across the radial grid" workload this ADR's original Context anticipated.
