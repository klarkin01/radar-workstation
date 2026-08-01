# Documentation Index

## Start here

Read in this order:

1. [`PHILOSOPHY.md`](PHILOSOPHY.md) — the design principles that govern every other
   decision in this repository. Predates and supersedes the architecture and the ADRs.
2. [`REQUIREMENTS.md`](REQUIREMENTS.md) — what the application must do, must not do,
   and what is still open.
3. [`architecture/overview.md`](architecture/overview.md) — technology stack, project
   structure, subsystem breakdown.
4. [`adr/`](adr/) — the ADR index below, one record per significant technical decision.

## Top-level documents

| Document | Description |
|---|---|
| [`PHILOSOPHY.md`](PHILOSOPHY.md) | Design principles: Instrument Principle, Stability as Ethics, Lightweight by Design, Security as First-Class, Restraint is a Feature. |
| [`REQUIREMENTS.md`](REQUIREMENTS.md) | Functional and non-functional requirements, including which ones are blocked on an open design question. |
| [`open-questions.md`](open-questions.md) | Unresolved design questions, what they block, and (as of this index) a resolution log for questions already closed. |
| [`dependency-inventory.md`](dependency-inventory.md) | Point-in-time audit of the dependency graph against `PHILOSOPHY.md`, as of `74a1065` (2026-07-29). Superseded for current state by `plans/dependency-inventory-remediation.md` §9 — see the banner at the top of the document. |
| [`documentation-inventory.md`](documentation-inventory.md) | Point-in-time audit of this documentation set against the code, as of `d46042c` (2026-07-30). The evidence base for `plans/documentation-remediation.md`. |
| [`project-inventory.md`](project-inventory.md) | Point-in-time audit of the project against the v1.0 scope boundary, as of `668b1ca` (2026-07-30) — what exists, what doesn't, and the order the missing work should be taken up in. Superseded for current state by each stage plan's own Results section — see the banner at the top of the document. |

## Architecture (`architecture/`)

| Document | Description |
|---|---|
| [`overview.md`](architecture/overview.md) | Entry point: technology stack, project structure tree, subsystem overview, implementation status. |
| [`data-flow.md`](architecture/data-flow.md) | How data moves from external sources (NEXRAD chunk stream, tile providers, placefiles) through the pipeline to the display. |
| [`rendering.md`](architecture/rendering.md) | How the application draws to the screen: the egui/wgpu split, projection, layer order, texture generation. |
| [`nexrad-binary-format.md`](architecture/nexrad-binary-format.md) | Complete byte-level layout of every NEXRAD structure the decoder parses — chunk envelopes, message headers, block layouts. Ground truth for the decoder, confirmed against fixture files. |
| [`nexrad-data-types.md`](architecture/nexrad-data-types.md) | High-level summary of the four NEXRAD Level II data types (`-S`/`-I`/`-E` chunks, archive volumes) and their role in the application. |

## Architectural Decision Records (`adr/`)

All decisions below are `Accepted` unless noted otherwise. See each ADR for the full
context, alternatives considered, and consequences.

| ADR | Title |
|---|---|
| [0001](adr/0001-use-rust.md) | Use Rust as the implementation language |
| [0002](adr/0002-use-egui.md) | Use egui as the UI framework |
| [0003](adr/0003-use-wgpu.md) | Use wgpu for radar data rendering |
| [0004](adr/0004-use-tokio.md) | Use tokio for asynchronous I/O |
| [0005](adr/0005-use-rayon.md) | Use rayon for compute parallelism |
| [0006](adr/0006-bundle-shapefiles.md) | Bundle shapefiles for basemap vector data |
| [0007](adr/0007-tile-providers.md) | Pluggable XYZ tile providers for background imagery |
| [0008](adr/0008-custom-decoder.md) | Implement a custom NEXRAD Level II decoder |
| [0009](adr/0009-open-source.md) | Release under an open source license (Apache-2.0) |
| [0010](adr/0010-workspace-structure.md) | Cargo workspace structure |
| [0011](adr/0011-chunk-stream-data-source.md) | Target the real-time chunk stream as primary data source |
| [0012](adr/0012-volume-assembly-state-machine.md) | Volume assembly state machine and missing chunk handling |
| [0013](adr/0013-http-client.md) | HTTP client dependency for S3 data acquisition — **Superseded by ADR-0014** |
| [0014](adr/0014-http-ingest-own-the-boundary.md) | Own the HTTP boundary — replace reqwest with a workspace-local client (`crates/http-ingest`) |
| [0015](adr/0015-bzip2.md) | Use bzip2 for NEXRAD chunk decompression |
| [0016](adr/0016-quick-xml.md) | Use quick-xml for S3 `ListObjectsV2` response parsing |
| [0017](adr/0017-bytes.md) | Use bytes for zero-copy buffer handoff |
| [0018](adr/0018-shared-application-state.md) | Shared application state structure (Q4) |
| [0019](adr/0019-config-format.md) | Configuration file format — workspace-local parser, not toml/serde |

## Plans (`plans/`)

A plan document is an executable work plan: scope, ordering, and acceptance criteria
written before the work starts. Plans are retained after execution — the `§9 Results`
(or equivalent) section becomes the record of what was actually done and measured, not
just what was intended.

| Document | Status | Description |
|---|---|---|
| [`0014-http-ingest-implementation.md`](plans/0014-http-ingest-implementation.md) | Draft | Implementation plan for `crates/http-ingest` (ADR-0014). |
| [`dependency-inventory-remediation.md`](plans/dependency-inventory-remediation.md) | Implemented | Closed most of `dependency-inventory.md`'s findings (E-02 through E-06); also where E-11 and E-12 were found and fixed. Its §9 Results is the authoritative account of the current dependency posture. |
| [`documentation-remediation.md`](plans/documentation-remediation.md) | This document's own remediation plan | Addresses `documentation-inventory.md`'s eleven findings, including writing this index. |
| [`stage-0-1-close-the-acquisition-path.md`](plans/stage-0-1-close-the-acquisition-path.md) | Implemented | Volume assembly state machine (ADR-0012), `-S` metadata decoding, poller skipped-volume recovery, decoder hardening. Its §8 Results is the authoritative account of what was measured. |
| [`stage-2-make-the-application-exist.md`](plans/stage-2-make-the-application-exist.md) | Implemented | Answers Q4 (ADR-0018); runtime/supervision skeleton; bundled site list; configuration persistence (ADR-0019). `main.rs` is a real, runnable program as of this plan. Its §12 Results is the authoritative account of what was measured. |
