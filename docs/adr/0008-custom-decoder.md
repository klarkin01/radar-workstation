# ADR-0008: Implement a Custom NEXRAD Level II Decoder

## Status
Accepted

## Context
The application's entire analytical capability depends on correct, complete parsing of
NEXRAD Level II archive format files. No mature, well-maintained Rust crate for NEXRAD
Level II decoding exists. Options considered were: wrapping an existing C library (e.g.
RSL), or implementing a decoder from scratch against the NCEI format specification.

## Decision
A custom NEXRAD Level II decoder is implemented in Rust against the NCEI archive format
specification. It is structured as an internal library with its own test suite, cleanly
separated from the rest of the application.

## Consequences
- Full ownership of the decoder means no dependency on an external project's maintenance
  schedule, API stability, or security posture.
- The decoder can be extended to support new scan modes, products, or format revisions
  without waiting on an upstream maintainer.
- Positions the project well for adaptation to the next-generation radar network, where
  a new decoder will be required and existing implementations will not exist.
- Implementing a correct decoder requires careful study of the NCEI specification and
  thorough testing against real archive files. This is the highest-risk piece of the
  initial implementation and should be prototyped and validated first.
- A comprehensive test suite against known-good Level II files is mandatory. Decoder
  correctness is the foundation of all product derivation and display.
