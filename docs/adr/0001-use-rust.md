# ADR-0001: Use Rust as the Implementation Language

## Status
Accepted

## Context
This application requires native compiled performance, a multithreaded architecture,
GPU access, and a memory safety posture acceptable for strict enterprise environments.
The developer has a strong preference against C++. Julia was evaluated and
rejected due to GC pauses, weak UI ecosystem, and runtime overhead incompatible with the
lightweight and instrument-feel requirements. A language that compiles to native code with
no runtime was required.

## Decision
Rust is the implementation language for the entire application.

## Consequences
- Memory safety is guaranteed by construction, satisfying security posture requirements
  without runtime overhead.
- No garbage collector eliminates GC pause risk in the render and data pipeline loops.
- Cargo provides a best-in-class build system and dependency management toolchain.
- `cargo audit` and the RustSec advisory database provide machine-readable dependency
  vulnerability posture for security reviewers.
- NSA and CISA guidance explicitly recommends memory-safe languages for new development;
  Rust satisfies this by construction and can be cited in security documentation.
- The Rust GUI ecosystem is less mature than C++/Qt. This is accepted and mitigated by
  the choice of egui. See ADR-0002.
