//! Hostile-input hardening for `config::load` (S2-W4 §6.5). Mirrors the
//! pattern `http-ingest`'s `fuzz/corpus/parse_response/` and
//! `nexrad-decoder`'s `tests/decoder_hardening.rs` established: a committed
//! corpus run through a plain assertion, then the same corpus mutated by
//! `fuzz-support`'s seeded mutator, both gated on stable `cargo test` — not
//! a nightly fuzz session someone has to remember to run.
//!
//! Fuzzing goes through `config::load(path)` end to end — write the
//! (possibly mutated, possibly non-UTF-8) bytes to a temp file and load it
//! — rather than the internal line parser directly. `load` is the actual
//! trust boundary (FR-CP-3: "must start successfully with a missing or
//! corrupt configuration file"), and going through it also exercises
//! `read_to_string`'s UTF-8 validation, site lookup, and interval parsing,
//! not just tokenization.

use radar_workstation::config;

fn corpus_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config_corpus")
}

fn read_corpus() -> Vec<Vec<u8>> {
    let dir = corpus_dir();
    std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("could not read corpus dir {dir:?}: {e}"))
        .map(|entry| std::fs::read(entry.expect("readable dir entry").path()).expect("readable corpus file"))
        .collect()
}

fn temp_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("radar-workstation-config-hardening-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir.join(name)
}

/// Writes `data` to a temp file and loads it. The point under test is only
/// "does this panic" — `load`'s own unit tests already cover what specific
/// events/defaults are correct for known inputs.
fn load_bytes(data: &[u8]) {
    let path = temp_path("candidate.cfg");
    std::fs::write(&path, data).expect("write candidate config");
    let _ = config::load(&path); // must not panic
}

#[test]
fn corpus_never_panics() {
    let corpus = read_corpus();
    assert!(!corpus.is_empty(), "corpus directory should not be empty");
    for data in &corpus {
        load_bytes(data);
    }
}

#[test]
fn mutated_inputs_never_panic() {
    let seeds = read_corpus();
    assert!(!seeds.is_empty());

    let mut rng = fuzz_support::XorShift64::new(0xC0FF_EE15_5AFE_5EED);
    for _ in 0..5000 {
        let mutated = fuzz_support::mutate_one(&mut rng, &seeds);
        load_bytes(&mutated);
    }
}
