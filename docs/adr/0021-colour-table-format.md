# ADR-0021: Colour Table Format (Q11, FR-CT-1)

## Status
Accepted

## Context
FR-CT-1 left the palette format for user-supplied colour tables unresolved. GR2Analyst
supports user-supplied palettes in the GRLevelX `.pal` format, and a large community
ecosystem of custom palettes already exists in that format. FR-CT-1's own stated
rationale for wanting GRLevelX compatibility at all was "immediate access to the existing
community palette ecosystem" — an operator arriving from GR2Analyst keeps the palettes
they already own.

This project's established posture toward an untrusted-input parser on a must-not-crash
path is to own it and fuzz it rather than pull in a third-party parser — `http-ingest`
(ADR-0014), `nexrad-decoder` (ADR-0008), and `config` (ADR-0019) all took this path for
the same reason: none of those formats' existing crates were judged to justify their
dependency cost against a project with a stated minimal-dependency posture, and a
hand-rolled parser gated on plain `cargo test` fuzzing catches what a dependency's own
test suite might not for this project's specific input shapes.

## Decision

**A documented subset of the GRLevelX `.pal` format, parsed by a workspace-local
parser** (`compute::palette::parse`). Bundled defaults for all seven v1.0 products are
authored in the same format and compiled in with `include_str!` — no runtime file
dependency, no startup parse-failure mode (the same reasoning as the generated site
table, ADR-0006's erratum). User palettes load from `paths::data_dir()/palettes/
<product>.pal` and override the bundled default by product (FR-CT-3).

**Loading never fails.** `parse(text, product) -> (Palette, Vec<Event>)` — an
unparseable line or an unrecognized directive is skipped and reported, never fatal
(Stability as Ethics, the same discipline `config::load` and FR-CP-3 impose on
configuration). `compute::palette::load_all` extends the guarantee to the whole product
set: every `DisplayProduct` always resolves to a palette. A missing user-palette
directory or file is silent (the expected first-run case, matching `config::load`'s
treatment of a missing config file). A user palette that exists but produces zero usable
colour entries — every line malformed — falls back to the bundled default and is
reported; there is no failure mode in which a product has no colours at all.

### Supported directive subset

| Directive | Meaning |
|---|---|
| `Product: <name>` | Informational; ignored (the caller already knows the product from which bundled/override file it loaded) |
| `Units: <name>` | Informational; shown on Stage 4's legend |
| `Step: <f>` | Legend tick spacing |
| `Color: <v> <r> <g> <b> [<r2> <g2> <b2>]` | Entry at threshold `v`; a second colour triple makes it a gradient to the next entry's threshold |
| `Color4: <v> <r> <g> <b> <a> [<r2> <g2> <b2> <a2>]` | As `Color:`, with alpha |
| `SolidColor: <v> <r> <g> <b>` | Entry at `v`, a flat step — never a gradient to the next entry |
| `SolidColor4: <v> <r> <g> <b> <a>` | As `SolidColor:`, with alpha |
| `RF: <r> <g> <b> [<a>]` | Range-folded colour (cell 1); defaults to opaque grey if never set |
| `ND: <r> <g> <b> [<a>]` | No-data colour (cell 0); defaults to fully transparent if never set (FR-DR-4) |
| `;` to end of line | Comment |

**This table was written against the plan's specification and verified structurally
(round-trip and gradient-interpolation tests, a hand-written community-style excerpt in
the parser's own test suite, and a mutator-fuzzed corpus) but was *not* cross-checked
against a downloaded set of real, currently-circulating community `.pal` files** — this
development session had no network access to the community palette sites the format
originates from. This is recorded as a gap rather than silently treated as complete: the
risk is bounded (an unrecognized directive is skipped and reported, not rejected, so an
incomplete table degrades gracefully rather than failing), but the supported set should
be verified against real files, and this note updated, before FR-CT-1 is treated as
fully closed in practice rather than closed in format-contract terms.

### Colour mapping

`Palette::sample(value) -> [u8; 4]` returns the colour of the entry at or below
`value`'s threshold, interpolating for a gradient entry; below the first threshold (or
an empty palette) returns `no_data`. `compute::palette::compile_lut` evaluates `sample`
once per possible 8-bit cell value (`(raw − offset) / scale` for `raw` in `2..=255`,
plus `no_data` at index 0 and `range_folded` at index 1) — this is the entirety of the
application's colour mapping (ADR-0020's S3-a): 256 evaluations per product, not one
per gate.

### Fuzz corpus

`crates/radar-workstation/tests/palette_hardening.rs`, following the pattern
`nexrad-decoder`'s `decoder_hardening.rs`, `http-ingest`'s `fuzz/corpus/parse_response/`,
and `config_hardening.rs` already established: a committed corpus
(`tests/fixtures/palette_corpus/`, seeded with the bundled defaults plus hand-written
malformed/hostile samples) run through `crates/fuzz-support`'s seeded mutator 5000 times
on plain `cargo test`. Four parsers in this workspace now share one mutator.

## Alternatives Considered

**A workspace-local format**, matching `config`'s `key = value` shape. Rejected: it
strands users from exactly the palette ecosystem FR-CT-1 named as the whole point of
wanting a documented external format in the first place.

**A third-party `.pal` parser crate**, if one existed with a track record comparable to
this project's other dependency decisions. None was found meeting that bar; the format
is narrow enough (documented directive table above) that owning it is a few hundred
lines, in line with `config`'s own parser.

## Consequences
- Seven bundled `.pal` files (`compute/palettes/*.pal`) ship compiled into the binary.
  Reflectivity, velocity, and spectrum width are ported verbatim from
  `utility/radar-viz/src/color_table.rs`'s already-validated NWS-standard tables; ZDR,
  CC, Echo Tops, and VIL are newly authored for this project over conventional NWS-style
  display ranges (each file's header comment records this and states the range).
- FR-CT-1 loses its `[OPEN]` marker; Q11 moves to `open-questions.md`'s Resolved
  section, with the verification gap above carried forward rather than hidden.
- `paths::data_dir()` (already implemented, S2-W4) gains its first real caller.
- No hot reload: a palette change takes effect on next startup (FR-CT-3's "load at
  startup" clause). A file watcher is a dependency and a background thread for a problem
  no one has reported yet.
