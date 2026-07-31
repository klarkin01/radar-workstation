# Fuzzing `http-ingest`

This directory is `exclude`d from the workspace (root `Cargo.toml`) and does not appear
in `Cargo.lock`. It costs nothing in a normal `cargo build`/`cargo test`.

Running the `parse_response` target requires, none of which are installed or needed for
any normal build:

- Nightly Rust (`libfuzzer-sys` requires nightly's `-Z sanitizer` support)
- `cargo install cargo-fuzz`
- LLVM's libFuzzer (a C++ library, linked in by `cargo-fuzz`)

```sh
cargo +nightly fuzz run parse_response
```

**This is an amplifier, not the primary defense.** The safety property — no panic on any
input, including the fuzz corpus and mutations of it — is enforced on **stable** Rust by
two tests in `src/response.rs`: `fuzz_corpus_never_panics` and `mutated_inputs_never_panic`
(a seeded xorshift mutator over the corpus). Those run in every ordinary `cargo test`, so a
regression fails a normal test run rather than depending on someone remembering to launch
the nightly fuzzer. The corpus in `corpus/` is what `cargo fuzz run` would explore further
and what those two stable tests already replay.
