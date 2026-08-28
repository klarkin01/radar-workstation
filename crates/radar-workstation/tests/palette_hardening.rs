//! Palette-parser hardening (S3-W3, ADR-0021): a committed corpus plus
//! seeded-mutator fuzzing gated on plain `cargo test` — the same pattern
//! `nexrad-decoder`'s `decoder_hardening.rs`, `http-ingest`'s
//! `fuzz/corpus/parse_response/`, and `config_hardening.rs` already
//! established. Four parsers in this workspace now share one mutator
//! (`crates/fuzz-support`).

use radar_workstation::compute::palette;
use radar_workstation::compute::DisplayProduct;

fn corpus_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/palette_corpus")
}

fn read_corpus() -> Vec<Vec<u8>> {
    let dir = corpus_dir();
    std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("could not read corpus dir {dir:?}: {e}"))
        .map(|entry| std::fs::read(entry.expect("readable dir entry").path()).expect("readable corpus file"))
        .collect()
}

#[test]
fn corpus_never_panics() {
    let corpus = read_corpus();
    assert!(!corpus.is_empty(), "corpus directory should not be empty");
    for data in &corpus {
        let text = String::from_utf8_lossy(data);
        let _ = palette::parse(&text, DisplayProduct::Reflectivity); // must not panic
    }
}

#[test]
fn mutated_palette_never_panics() {
    let seeds = read_corpus();
    assert!(!seeds.is_empty());

    let mut rng = fuzz_support::XorShift64::new(0x5EED_BA1E_1234_5678);
    for _ in 0..5000 {
        let mutated = fuzz_support::mutate_one(&mut rng, &seeds);
        let text = String::from_utf8_lossy(&mutated);
        let _ = palette::parse(&text, DisplayProduct::Reflectivity); // must not panic
    }
}
