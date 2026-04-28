# ADR-0004: Use tokio for Asynchronous I/O

## Status
Accepted

## Context
The application must perform continuous non-blocking network I/O — polling for new NEXRAD
volume scans, fetching map tiles, and retrieving placefiles — without stalling the UI or
render loop. A synchronous threading approach (one thread per network operation) was
considered but rejected as wasteful and difficult to coordinate cleanly at scale. An async
runtime is the correct abstraction.

## Decision
tokio is the async runtime for all network I/O in the application.

## Consequences
- All network operations are non-blocking. The render loop never waits on I/O.
- tokio is the dominant, most mature async runtime in the Rust ecosystem. It is
  production-proven at scale.
- Radar polling, tile fetching, and placefile retrieval each run as independent async
  tasks, cleanly isolated from one another.
- tokio introduces a non-trivial conceptual model (async/await, tasks, channels). This
  complexity is accepted because the alternative — blocking I/O on dedicated threads —
  creates its own coordination complexity and is less idiomatic in Rust.
- Communication between the tokio async layer and the synchronous render loop is handled
  via channels, keeping the boundary explicit and clean.
