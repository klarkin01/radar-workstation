# ADR-0010: Cargo Workspace Structure

## Status
Accepted

## Context
The project requires a custom NEXRAD decoder (ADR-0008) that is architecturally distinct
from the application layer: it has no UI, GPU, or async runtime dependencies; it has
well-defined inputs (raw bytes from S3) and outputs (a VolumeScan struct); and it is
independently testable and potentially publishable to crates.io as a standalone library.

A decision was required on whether to structure the project as a single Cargo crate or
a workspace of multiple crates. The project also anticipates future decoder crates for
radar formats that succeed NEXRAD, making extensibility a structural consideration.

## Decision
The project uses a Cargo workspace with a virtual root manifest (the root Cargo.toml
declares only the workspace, not a package). Production crates live under `crates/`:

- `crates/radar-workstation` — the application binary
- `crates/nexrad-decoder` — the NEXRAD Level II decoder library
- `crates/http-ingest` — the workspace-local HTTP/1.1 client (ADR-0014)

Future decoder crates (e.g. for successor radar formats) are added as new library crates
under `crates/` and listed in the workspace members.

<!-- corrected 2026-07-30: the crates/ vs. utility/ split below was not recorded when
this ADR was originally written; it has since become a real structural decision. -->

**The `crates/` / `utility/` split.** The workspace also has development-only members
under `utility/` (`utility/nexrad-sample`, `utility/radar-viz`) — see
`utility/README.md`'s "not part of the product" boundary. They are Cargo workspace
members (so they build and test with `cargo build --workspace` / `cargo test
--workspace`) but are excluded from the default build via root `Cargo.toml`'s
`default-members`, which lists only the three `crates/` above. Nothing in `utility/` is
imported by anything in `crates/`. Separately, `crates/http-ingest/fuzz` is `exclude`d
from the workspace entirely — it needs a nightly toolchain and does not appear in
`Cargo.lock`.

## Consequences
- The NEXRAD decoder is independently testable without a GPU or window context.
- Adding support for a future radar format is a first-class operation: a new library
  crate under `crates/`, added to the workspace members list.
- The decoder can eventually be published to crates.io as a standalone library,
  benefiting the broader Rust and meteorological software communities.
- The application binary remains focused; the crate boundary enforces separation between
  data decoding and application logic more strongly than a module boundary would.
- A small amount of additional structure (multiple Cargo.toml files, path dependencies)
  is the accepted tradeoff.
