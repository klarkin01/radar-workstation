# Contributing

## Build and test

See the root [`README.md`](README.md#build-and-test) for the build, test, lint, and
audit commands.

## Before adding a dependency

**No new dependency without an ADR.** This is the project's most distinctive
constraint. Every direct dependency in `crates/` is deliberate and recorded in
`docs/adr/` — see the index in [`docs/README.md`](docs/README.md#architectural-decision-records-adr).
If your change needs a new dependency, propose the ADR first; adding the dependency
without one means the work gets redone.

This applies to `crates/`. `utility/` is looser — see below — but still worth a note in
the PR description.

## The `utility/` boundary

Nothing in `utility/` is imported by any crate in `crates/`. It exists for development
tasks: cross-validating the decoder, exploring file structure, generating test
fixtures. Logic that belongs in the product gets reimplemented in Rust in `crates/`,
not promoted out of `utility/`. See [`utility/README.md`](utility/README.md) for what's
there and what it's for.

## DRY

Per `PHILOSOPHY.md`'s Principle 4 (Clean, Uncomplex Code): code adheres to the DRY
principle. Before adding a new abstraction or duplicating logic, check whether an
existing module already owns that responsibility.

## Before proposing an architectural change

Read the relevant ADR in `docs/adr/`, then [`docs/PHILOSOPHY.md`](docs/PHILOSOPHY.md) —
it predates and supersedes every architectural decision in this repository, and design
choices are evaluated against it first.

## Formatting

There is no `rustfmt.toml`, and adopting one is a deliberately deferred decision. Some
existing code intentionally departs from default `rustfmt` output. Do not run
`cargo fmt` across the tree — it will produce a large, unrelated diff.

## CI

The workflow in `.github/workflows/` has not yet run on a real GitHub Actions runner.
If your PR is the first to trigger it, failures may reflect the workflow itself rather
than your change — flag it rather than assuming your code is at fault.
