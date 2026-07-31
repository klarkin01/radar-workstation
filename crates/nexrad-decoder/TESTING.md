# Decoder Test Coverage Status

This is a documentary status note, not a test plan. It records how far the current test
suite is from FR-ND-8 (`docs/REQUIREMENTS.md`), and is the kind of thing that goes stale
fast — re-derive it from `ls tests/fixtures/` and the test names in `tests/decode_radial.rs`
rather than trusting this file's numbers if it's been a while since either changed.

## What the suite covers today

24 tests in `tests/decode_radial.rs`, all passing, against five fixtures in
`tests/fixtures/` — all extracted from real KDOX (Dover, DE) chunks, VCP 35 (clear-air
surveillance), 2026-06-29, one per chunk kind (`-S` start-of-volume, start-of-elevation,
intermediate, end-of-elevation, end-of-volume).

The tests themselves are good — the gap below is fixture breadth, not test quality:

- Physical-value conversion (`(raw − offset) / scale`) for every moment, including the
  16-bit products (ZDR, PHI) that use a different word size than the 8-bit ones.
- Per-status geometry: azimuth, elevation, radial status, and site/volume metadata
  (RVOL, RRAD) decode correctly on every chunk kind.
- Per-moment gate geometry: gate count, first gate, gate width for each of the seven
  product kinds.
- Reserved-code handling: values reserved by the ICD (e.g. "below threshold," "range
  folded") return `None` rather than a bogus physical value.
- One truncation case (`truncated_msg31_record_returns_error`) and one legacy-record
  skip case (`legacy_size_records_are_skipped`).

## Distance from FR-ND-8

FR-ND-8 requires the suite to exercise known-good files across multiple sites, scan
modes, and eras; corrupt and truncated input; dual-pol and non-dual-pol variants; and
both resolution variants.

| FR-ND-8 requires | Current coverage |
|---|---|
| Multiple sites | One — KDOX only |
| Multiple scan modes | One — VCP 35 (clear air) only |
| Multiple eras | One — 2026-06-29 only |
| Corrupt input | None |
| Truncated input | Partial — one case |
| Dual-pol and non-dual-pol variants | Dual-pol only; no non-dual-pol fixture |
| Super-res and standard-res variants | Super-res only; no standard-res fixture |

None of this is a defect in what exists — FR-ND-8 is a v1.0 requirement and the decoder
is early. **The gap is that nothing states this plainly elsewhere**, which is what this
file is for. `data-flow.md`'s Testing section links here rather than asserting present-tense
coverage it doesn't have.

## The asymmetry worth naming

FR-ND-6 / BC-6 / NFR-ST-2 require the decoder to never panic on malformed input. That
property currently rests on exactly one truncation test. One crate over,
`crates/http-ingest` — a comparable untrusted-input path (HTTP response framing instead
of NEXRAD message framing) — has a 31-file fuzz corpus gated on stable `cargo test`
(`response.rs`'s `fuzz_corpus_never_panics` and `mutated_inputs_never_panic`, a seeded
xorshift mutator over the corpus). A regression there fails an ordinary test run, not a
nightly fuzz session someone has to remember to launch. The precedent for how to test a
parser against hostile input already exists in this workspace; the decoder doesn't use
it yet.

## `-S` chunk metadata is not decoded

`parse_radial_stream` (`src/parse/mod.rs`) silently skips any record whose message type
is not 31 — by design, per its own doc comment, since the primary workload is radial
data. One consequence: a `-S` chunk's metadata messages (2 RDA Status, 3
Performance/Maintenance, 5 VCP, 15 Clutter Filter Map, 18 RDA Adaptation Data) are never
decoded by this crate. `docs/adr/0012-volume-assembly-state-machine.md`'s
`VolumeContext` initialization and `docs/architecture/nexrad-data-types.md`'s stated
"Role in this application" for `-S` chunks ("initialize a `VolumeContext`") both describe
behavior that depends on decoding those messages — which is correctly scoped as
not-yet-built at this stage (the volume assembly layer is design-only), but was not
previously stated as a decoder-level gap anywhere.
