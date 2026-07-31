# Decoder Test Coverage Status

This is a documentary status note, not a test plan. It records how far the current test
suite is from FR-ND-8 (`docs/REQUIREMENTS.md`), and is the kind of thing that goes stale
fast — re-derive it from `ls tests/fixtures/` and the test names in `tests/*.rs` rather
than trusting this file's numbers if it's been a while since either changed.

## What the suite covers today

**KDOX VCP 35** (super-resolution, dual-pol, 2026-06-29): 25 tests in
`tests/decode_radial.rs` against five fixtures, one per chunk kind (`-S` start-of-volume,
start-of-elevation, intermediate, end-of-elevation, end-of-volume).

**KTLH VCP 212** (2026-07-31): 4 tests in `tests/decode_ktlh_vcp212.rs`, three fixtures.
A second site, a precipitation VCP with SAILS/MRLE supplemental cuts, and the first
standard-resolution fixture. See S1-W4d in
`docs/plans/stage-0-1-close-the-acquisition-path.md`.

**KTLH VCP 121, archive bucket, 2010-06-01** (non-dual-pol era): 4 tests in
`tests/decode_ktlh_vcp121_legacy.rs` against five fixtures extracted directly from a
pre-BZ2-wrapped archive volume file (see that file's module doc comment for the envelope
difference from the chunk stream).

**Hostile-input hardening:** `tests/decoder_hardening.rs`, a corpus of golden and
hostile fixtures (structural truncation, hostile pointers, hostile counts, hostile
framing) run through `corpus_never_panics` and a seeded-mutator
`mutated_inputs_never_panic`, mirroring the pattern `http-ingest` established. Gated on
plain `cargo test`, not a manual fuzz session. See S1-W4a.

The tests themselves are good — the gap below is fixture breadth, not test quality:

- Physical-value conversion (`(raw − offset) / scale`) for every moment, including the
  16-bit products (ZDR, PHI) that use a different word size than the 8-bit ones.
- Per-status geometry: azimuth, elevation, radial status, and site/volume metadata
  (RVOL, RRAD) decode correctly on every chunk kind.
- Per-moment gate geometry: gate count, first gate, gate width for each of the seven
  product kinds.
- Reserved-code handling: values reserved by the ICD (e.g. "below threshold," "range
  folded") return `None` rather than a bogus physical value.
- Unrecognized radial status codes decode to `RadialStatus::Unknown` with geometry and
  moment data intact, rather than discarding the radial (S1-W4c).
- Truncation and legacy-record-skip cases, now backed by a full hostile-input corpus
  rather than one hand-written case each.

## Distance from FR-ND-8

FR-ND-8 requires the suite to exercise known-good files across multiple sites, scan
modes, and eras; corrupt and truncated input; dual-pol and non-dual-pol variants; and
both resolution variants.

| FR-ND-8 requires | Current coverage |
|---|---|
| Multiple sites | KDOX and KTLH |
| Multiple scan modes | VCP 35 (clear air), VCP 212 (precipitation, SAILS/MRLE), VCP 121 (legacy) |
| Multiple eras | 2026-06-29/2026-07-31 (current) and 2010-06-01 (pre-dual-pol archive) |
| Corrupt input | Committed hostile corpus + seeded mutator (`decoder_hardening.rs`) |
| Truncated input | Six truncation cases at every structural boundary, in the corpus |
| Dual-pol and non-dual-pol variants | Both — KDOX/KTLH VCP 212 (dual-pol) vs. KTLH VCP 121 (non-dual-pol) |
| Super-res and standard-res variants | Both — measured from real KTLH VCP 212 and KDOX VCP 35 data |

Remaining gaps, not yet closed:

- Only two sites. FR-ND-8's "multiple sites" is satisfied in kind but not in breadth —
  regional hardware/firmware variation across the ~160-site network is not sampled.
- No fixture for a VCP other than 35/212/121 (e.g. 12, which was seen live during S1-W4d
  scanning at KGRK but not captured as a fixture).
- No fixture demonstrating a missing/absent moment mid-volume (a real dropped block, as
  opposed to the synthetic corpus case).

## The asymmetry that used to be worth naming

FR-ND-6 / BC-6 / NFR-ST-2 require the decoder to never panic on malformed input. Before
S1-W4a, that property rested on exactly one truncation test, while `http-ingest` — a
comparable untrusted-input path — had a 31-file fuzz corpus gated on stable `cargo test`.
That gap is now closed: `decoder_hardening.rs` mirrors the same pattern (committed
corpus + seeded xorshift64 mutator, shared via `crates/fuzz-support`) directly against
this crate's public `parse_radial_stream` entry point.

## `-S` chunk metadata is not decoded

`parse_radial_stream` (`src/parse/mod.rs`) silently skips any record whose message type
is not 31 — by design, per its own doc comment, since the primary workload is radial
data. One consequence: a `-S` chunk's metadata messages (2 RDA Status, 3
Performance/Maintenance, 5 VCP, 15 Clutter Filter Map, 18 RDA Adaptation Data) are never
decoded by this crate. `docs/adr/0012-volume-assembly-state-machine.md`'s
`VolumeContext` initialization and `docs/architecture/nexrad-data-types.md`'s stated
"Role in this application" for `-S` chunks ("initialize a `VolumeContext`") both describe
behavior that depends on decoding those messages. This is Stage 1's S1-W2 work item.
