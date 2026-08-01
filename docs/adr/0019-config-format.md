# ADR-0019: Configuration File Format (S2-f)

## Status
Accepted

## Context
FR-CP-1/2/3 require persisted, human-editable, plain-text configuration that must never
prevent startup, even when missing or corrupt. `docs/plans/stage-2-make-the-application-exist.md`
§6.2 posed this as an explicit user decision (S2-f) because option 1 adds dependencies, and
`CLAUDE.md` requires asking before doing that.

Two options were considered:

1. **`toml` + `serde` + `serde_derive`.** Familiar, well-specified, derive-driven. Costs
   several packages including proc-macro machinery, on a dependency graph ADR-0014 worked
   to bring down to 78 packages, for a config file with single-digit keys at this stage.
2. **A workspace-local `key = value` parser.** `# comment` lines, `key = value` lines,
   dotted keys for grouping (`ingest.poll_interval_seconds`), values as free-form text
   whose specific type each known key decides for itself. ~150 lines plus tests.

The user chose option 2.

## Decision
Configuration is parsed by a workspace-local parser (`crates/radar-workstation/src/
config/parse.rs`), not `toml`/`serde`. This follows the precedent `http-ingest` (ADR-0014)
and `nexrad-decoder` (ADR-0008) already established: an untrusted-input parser on a
must-not-crash path is owned by this project and fuzzed with the same seeded-mutator
harness (`crates/fuzz-support`) those two crates use, rather than pulled in as a
dependency. `crates/radar-workstation/tests/config_hardening.rs` runs a committed corpus
(`tests/fixtures/config_corpus/`) plus 5000 mutated variants of it through
`config::load` on every `cargo test` — not a nightly fuzz session someone has to
remember to run.

**Loading never returns an error.** `config::load(path) -> (Config, Vec<Event>)` — a
missing file is the expected first-run case (silent defaults, not reported); any other
read failure, an unparseable line, an invalid value, an unknown site, or an
out-of-range interval each fall back to that field's default and are reported as their
own typed `Event`. FR-CP-3 is therefore a property of this function's signature, not a
code path someone has to remember to test.

**Saving is line-preserving and atomic** (`config::save`, `crates/radar-workstation/src/
config/save.rs`): re-read the file, replace the line for each changed key in place,
append genuinely new keys, and leave every other line — comments, blank lines, unknown
keys, another instance's settings — byte-identical. Write goes to a temp file in the
same directory (named with the process ID, so concurrent instances per NFR-P-1 never
collide) and is `rename`d into place, so a crash or full disk mid-write can never leave
a truncated config. This solves two problems at once: FR-CP-2 lets the user hand-edit
the file including comments, and a wholesale rewrite from a struct would both erase
those comments and let the last of several simultaneously-running instances (NFR-P-1)
clobber every setting the others changed.

## Consequences
- Zero new external dependencies. `Cargo.lock`'s package count is unaffected by this
  decision (see the plan's §9/§12 measurements).
- The configuration surface is deliberately tiny at Stage 2 (`site`, `ingest.
  poll_interval_seconds`) — every key added later goes through the same
  parse/load/save path, which is designed to extend by adding a match arm in
  `config::apply_key`, not by redesigning the format.
- If a future stage's configuration surface grows complex enough (nested structures,
  nontrivial validation, many dozens of keys) that a hand-rolled parser becomes the
  wrong tradeoff, revisit this ADR rather than silently drifting — the same posture
  ADR-0014 and the ADR-0006 erratum take toward their own decisions.
- Unlike `toml`, this format has no native representation for lists, tables, or nested
  structures beyond one level of dotted-key grouping. Placefile URLs (FR-CP-1, plural,
  each with its own polling interval) will need either repeated numbered keys
  (`placefile.1.url`, `placefile.2.url`) or a single delimited value; that design
  question is deferred to Stage 6 rather than answered speculatively here.
