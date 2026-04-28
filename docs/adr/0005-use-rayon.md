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
