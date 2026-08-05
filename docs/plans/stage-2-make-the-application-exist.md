# Implementation Plan — Stage 2: Make the Application Exist

**Status:** Implemented — see §12 Results
**Drafted:** 2026-07-31
**Implemented:** 2026-07-31
**Implements:** `docs/project-inventory.md` §6, Stage 2 (items 6–9)
**Baseline commit:** `7df19d6` (working tree clean)
**Toolchain:** rustc 1.95.0 / cargo 1.95.0, `x86_64-unknown-linux-gnu`
**Predecessor:** `docs/plans/stage-0-1-close-the-acquisition-path.md` (implemented; see its §8 Results)

This plan is written to be executed in a later session. It carries every decision already
taken so implementation does not need to re-derive them from the ADRs. Where a decision is
still open it is marked **DECIDE** and states a recommendation and the reasoning behind it.

**Scope boundary:** this plan turns the library into a *program*. At the end of it,
`cargo run --release -- KDOX` starts a process that polls, decodes, assembles, and holds
the current radar state in memory, observably, with configuration and a site list behind
it. It draws nothing. No window, no wgpu, no egui, no compute layer, no textures — those
are Stages 3 and 4, and every one of them is gated on a question this plan does not
answer.

Stage 2 answers exactly one open question: **Q4** (shared state structure and lock
granularity). It is unblocked today and blocks both the compute layer and the render
loop, which is why it is here and not later.

---

## 1. What "done" means

| Claim | How it is demonstrated |
|---|---|
| Q4 is answered and recorded | `docs/adr/0018-shared-application-state.md`; Q4 moved to the Resolved section of `open-questions.md` |
| The binary is a real program | `cargo run --release -- KDOX` polls live S3 and holds state; `main.rs` no longer contains a `TODO` |
| Radar state is readable without blocking a writer | `AppState::snapshot()` is the only read API and returns owned data; the type system makes holding a lock guard across a frame impossible |
| Sweeps become visible as they close, not at volume end | An integration test drives the applier from a recorded `AssemblyEvent` sequence and asserts state visibility after each `SweepClosed` |
| The last good scan survives a failure | Applier tests: a `TimedOut` / `Superseded` volume never clears already-visible sweeps (ADR-0012, FR-DA-5) |
| The pipeline starts and stops cleanly | `Pipeline::spawn` / `shutdown().await` test: all tasks joined, no orphan tasks, no panic on double shutdown |
| A panicking task does not kill the application | Supervision test: an injected task panic is caught, surfaced as a typed event, and the pipeline restarts with backoff |
| Sites are enumerable with no network | `sites::all()` / `sites::by_id()` unit tests; one `#[ignore]`d live test cross-checking the bundled list against the chunk bucket's actual site prefixes |
| Configuration persists, and never prevents startup | Round-trip test; missing-file test; corrupt-file test; a mutator-driven fuzz test over the config parser using the existing `crates/fuzz-support` |
| Memory is measured, not assumed | Peak RSS after one volume, after four volumes, and across a volume boundary, recorded in §8 Results against the < 200 MB target |

**Requirements closed or advanced:** FR-MU-3 and FR-SS-1 (closed), FR-CP-1 / FR-CP-2 /
FR-CP-3 (closed for the configuration surface that exists at Stage 2), FR-DA-2's
"configurable interval" clause (closed), FR-DA-3 (advanced — sweeps are held for display
as they close; nothing displays them yet), FR-DA-5 (advanced — the last good scan is now
actually *retained* somewhere, not merely not-overwritten), NFR-ST-1 (advanced — startup
and configuration are now on the must-not-crash list and tested as such).

**Not closed, deliberately:** everything with a UI surface. FR-DR-6, FR-DR-7 and NFR-ST-3
still have no status bar to surface into. The seam exists; Stage 4 attaches to it.

---

## 2. What Stage 1 left that this plan builds on

Read `stage-0-1-close-the-acquisition-path.md` §8 before starting. Four of its outcomes
directly shape the work here:

- **`Arc<Sweep>` is already in place** (`VolumeScan.sweeps: Vec<Arc<Sweep>>`). The state
  store can hold sweeps *and* the closed volume they came from at the cost of a refcount
  bump, not a copy. Most of §3's retention design depends on this already being true.
- **`S3Poller::status()` already publishes a `watch::Receiver<IngestStatus>`** with typed
  errors. `AppState` must **hold that receiver**, not copy its contents into a second
  structure that then needs syncing. This is the single most important DRY constraint in
  this plan.
- **`event::Event` already exists** as a typed enum with one stderr sink and an explicit
  instruction not to reach for `eprintln!` elsewhere. Stage 2 adds the second sink, not a
  second mechanism.
- **First `SweepClosed` arrives ~1.5 s after poller start** (measured live, KDOX VCP 35).
  The wiring in this plan is therefore not on the critical path for the < 2 s
  first-render target; the render loop's own startup will be.

---

## 3. S2-W1 — Answer Q4 and build shared application state

**Requirement:** Q4. **Blocks:** the compute layer (Stage 3) and the render loop (Stage 4).

New module `crates/radar-workstation/src/state/`. This is the spine of Stage 2; W2, W3 and
W4 attach to it.

### 3.1 The answer to Q4, in one paragraph

`Arc<RwLock<AppState>>` as written in `overview.md` and `data-flow.md` is one lock over
everything, including data the render loop owns exclusively and data that is already
behind its own channel. The answer this plan proposes is narrower on all three counts:

1. **One lock, over radar data only.** `Arc<AppState>`, where `AppState` holds
   `RwLock<RadarState>` internally. Radar data is the only thing with a genuine
   many-reads/occasional-write pattern.
2. **View state is not shared at all.** Pan, zoom, active product, active sweep, and
   window geometry are owned outright by the render loop. Nothing in the data pipeline
   may write them — `data-flow.md` already states "the data pipeline does not modify
   user settings", and FR-NI-4's inviolability guarantee is far easier to hold when
   exactly one thread can move the viewport.
3. **Status is read through the channel that already publishes it.** `AppState` holds the
   `watch::Receiver<IngestStatus>` from `S3Poller::status()`. No copy, no sync step, no
   second source of truth.

**DECIDE (S2-a): interior locks on a shared `AppState`, rather than one outer
`RwLock<AppState>`.** *Recommendation: yes, as above.* This is a deviation from the
literal phrasing in three documents and therefore gets an ADR (§3.6) rather than a silent
implementation. The reasoning: an outer lock forces the render loop to take a write-capable
lock's read side over data it will never read (config, event log) and over data that is
already synchronised elsewhere (ingest status), and it makes the memory-retention question
in §3.3 harder to reason about because everything's lifetime becomes the lock's lifetime.

### 3.2 Types

```rust
// crates/radar-workstation/src/state/mod.rs

/// Identifies which volume a sweep came from. Ordered by (julian_date,
/// scan_time_ms) — scan_time_ms is milliseconds since midnight UTC and
/// wraps daily, so it is never compared on its own.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct VolumeId { julian_date: u16, scan_time_ms: u32 }

pub struct DisplaySweep {
    pub sweep: Arc<Sweep>,
    pub volume: VolumeId,
    pub vcp_number: u16,
    /// When this sweep was applied. Feeds FR-DA-5's data-age display.
    pub received: Instant,
}

pub struct RadarState {
    pub site: &'static Site,               // from S2-W3; fixed for this pipeline's life
    /// Newest closed sweep per elevation number, carried across volume
    /// boundaries so a closing volume never blanks the display.
    sweeps: BTreeMap<u8, DisplaySweep>,
    pub last_complete: Option<Arc<VolumeScan>>,
    pub revision: u64,
}

pub struct AppState {
    radar: RwLock<RadarState>,
    ingest: watch::Receiver<IngestStatus>,
    events: Mutex<EventLog>,
}
```

**`snapshot()` is the only read API.**

```rust
impl AppState {
    pub fn snapshot(&self) -> StateSnapshot;   // takes the read lock, clones Arcs, drops it
}

pub struct StateSnapshot {
    pub site: &'static Site,
    pub sweeps: Vec<DisplaySweep>,   // DisplaySweep is Arc + Copy fields; cheap
    pub last_complete: Option<Arc<VolumeScan>>,
    pub revision: u64,
    pub ingest: IngestStatus,
}
```

There is deliberately no `fn read(&self) -> RwLockReadGuard<'_, RadarState>`. Returning
owned data makes "the render loop never holds a lock across a frame" a property of the
type system rather than a rule in a document that a later contributor has to remember.
The cost is one `Vec` allocation and N refcount bumps per frame, where N is the sweep
count (14 for KDOX VCP 35) — negligible against a 16.6 ms frame budget, and measurable in
Stage 4 if it ever stops being negligible.

`revision` increments on every applied change. The render loop compares it against the
value it last uploaded to the GPU and skips texture re-upload when unchanged (FR-DR-5:
new scan arrivals must not cause a perceptible frame-rate drop).

### 3.3 The applier: a pure function, in the Stage 1 pattern

Mirror `VolumeAssembler` exactly — a pure synchronous core with time injected, wrapped by
a thin async shell. The same reasoning applies for the same reason, and consistency
between the two is itself worth something:

```rust
/// Apply one assembly event. Returns whether anything changed (i.e.
/// whether `revision` was bumped).
pub fn apply(state: &mut RadarState, event: AssemblyEvent, now: Instant) -> bool;
```

Rules, each with a named test:

| Rule | Rationale | Test |
|---|---|---|
| `SweepClosed` replaces the entry for its elevation number | Partial-scan rendering (ADR-0012, FR-DA-3) | `sweep_closed_becomes_visible_immediately` |
| A sweep from an older `VolumeId` never replaces a newer one | Re-anchoring and late chunks can deliver out of order | `stale_sweep_does_not_overwrite_newer` |
| `VolumeClosed` sets `last_complete` **only** for `VolumeStatus::Complete` | FR-DA-5's "last successfully fetched scan" | `timed_out_volume_does_not_become_last_complete` |
| `VolumeClosed` with `TimedOut` / `Superseded` clears nothing | ADR-0012: a visible gap beats silently modifying rendered data | `superseded_volume_leaves_visible_sweeps_intact` |
| Stale elevations are dropped only on a **VCP change** | A new VCP has a different elevation set; an incomplete volume in the same VCP does not | `vcp_change_drops_elevations_from_the_old_pattern` |
| `LateRadialsDiscarded` / `MissingStartChunk` push to the event log, do not touch sweeps | They are observability, not data | `informational_events_do_not_bump_revision` |
| `reset(site)` empties everything | FR-DA-4 consumes this in Stage 7 | `reset_clears_all_radar_state` |

**DECIDE (S2-b): retention policy — one merged sweep per elevation, plus the last complete
volume.** *Recommendation: yes.* Because `VolumeScan.sweeps` is already `Vec<Arc<Sweep>>`,
holding both the merged per-elevation map and `last_complete` costs only refcounts wherever
they overlap, which is most of the time. Distinct memory is held only during a volume
transition, when some elevations have advanced and others have not. Rough arithmetic for
the worst case: a super-resolution sweep is ~720 radials × ~1832 gates × up to 7 moments
≈ 9 MB; 14 sweeps ≈ 130 MB; a full transition therefore approaches two volumes' worth
before the older Arcs drop. **That is close enough to the 200 MB target that it must be
measured, not assumed** — see §8's measurement list, and §7's risk entry. If it proves
tight, the mitigation is Stage 3's, not Stage 2's: once the compute layer produces
textures, raw radials for products nobody is displaying can be dropped after computation.
Do not pre-optimise that here.

### 3.4 The event log

A bounded `VecDeque<(Instant, Event)>`, capacity 64, behind its own `Mutex`. This is the
second sink `event.rs` anticipated: `log_to_stderr` stays as the process-wide sink, and
the log gives NFR-ST-3's status bar something to read at Stage 4. Bounded, because an
unbounded diagnostic buffer in a process that runs for hours during an event is a memory
leak with a friendly name.

Push through one function so both sinks stay in step:

```rust
impl AppState { pub fn report(&self, event: Event); }   // -> log_to_stderr + ring buffer
```

Every current `event::log_to_stderr` call site inside a task that has an `Arc<AppState>`
moves to `report`. Call sites that do not (unit tests, the poller's own internals before
wiring) keep the direct sink.

### 3.5 Channels and backpressure

All `mpsc` channels are **bounded**, for the same reason the event log is:

| Channel | Capacity | Reasoning |
|---|---|---|
| poller → assembly (`ChunkEnvelope`) | 32 | A volume is ~79 chunks; 32 absorbs a burst without letting a stalled consumer buffer a whole volume of raw bytes |
| assembly → applier (`AssemblyEvent`) | 64 | One volume produces ~14 `SweepClosed` + 1 `VolumeClosed`; 64 is four volumes of headroom |

Backpressure propagates naturally: a slow applier blocks assembly's send, which blocks the
poller's send, which delays the next poll. That is the correct behaviour — it is bounded,
visible in `IngestStatus`, and never grows memory. Note it in a comment so a later reader
does not "fix" it by unbounding a channel.

### 3.6 Documentation and ADR

- **New `docs/adr/0018-shared-application-state.md`** — Accepted. Records the Q4 answer:
  interior locks, radar-only lock scope, render-owned view state, status read through the
  existing `watch`, `snapshot()` as the only read API, and the retention policy. State the
  alternatives (one outer `RwLock<AppState>`; lock-free double-buffer / `ArcSwap`) and why
  they were not chosen — an `ArcSwap`-style swap is a reasonable future refinement if
  measurement in Stage 4 shows read-lock contention, and saying so now costs nothing.
- **`docs/open-questions.md`** — move Q4 to Resolved with the decision and where it landed.
- **`docs/architecture/overview.md`, `docs/architecture/data-flow.md`, `CLAUDE.md`** — all
  three say `Arc<RwLock<AppState>>`. Correct them to match, in the same pass. This is
  precisely the "design drift" failure mode `project-inventory.md` §7 names; the fix is to
  amend the documents at the moment the code diverges, not later.

---

## 4. S2-W2 — Runtime skeleton

**Requirement:** the "Data Pipeline (tokio)" of `data-flow.md`, made real.
**Blocks:** every subsequent stage.

New module `crates/radar-workstation/src/pipeline.rs`, plus a rewritten `main.rs`.

### 4.1 Do not use `#[tokio::main]`

**DECIDE (S2-c): construct the tokio runtime explicitly; leave the main thread free.**
*Recommendation: yes, and this is the one decision here that is expensive to reverse.*
At Stage 4 the winit/egui event loop takes the main thread and will not give it back —
on some platforms it never returns. If Stage 2 writes `#[tokio::main] async fn main()`,
Stage 4 has to unpick it. Write it correctly the first time:

```rust
fn main() -> ExitCode {
    let args = cli::parse(std::env::args_os());   // §6.4
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_io().enable_time()
        .worker_threads(2)          // §4.4
        .thread_name("rw-io")
        .build()?;
    let state = Arc::new(AppState::new(site, ingest_status_rx));
    let pipeline = Pipeline::spawn(&runtime, site, Arc::clone(&state), &config);

    // Stage 2: a headless loop that prints state transitions and exits on
    // EOF. Stage 4 replaces exactly this call with the event loop.
    headless::run(&state);

    runtime.block_on(pipeline.shutdown());
    ExitCode::SUCCESS
}
```

The comment marking the replacement point matters — it is what makes Stage 4's first step
a two-line change rather than an archaeology exercise.

### 4.2 `Pipeline`: spawn, own, shut down

```rust
pub struct Pipeline { tasks: Vec<JoinHandle<()>>, shutdown: watch::Sender<bool> }

impl Pipeline {
    pub fn spawn(rt: &Runtime, site: &'static Site, state: Arc<AppState>, cfg: &Config) -> Self;
    pub async fn shutdown(self);   // signal, then join every task
}
```

Three tasks: the poller, `assembly::run`, and the applier. Shutdown is a
`watch::Sender<bool>` — **not** `tokio_util::sync::CancellationToken`, which would add a
dependency for something `tokio::sync` already does in ten lines. Each task's loop gains a
`tokio::select!` arm on the shutdown receiver.

`assembly::run` and `S3Poller::run` currently terminate when their channel closes. That is
correct but *lazy*: the poller only notices at its next send, up to a poll interval plus an
in-flight fetch later. Add the shutdown arm to both so shutdown is prompt and deterministic,
and so the same mechanism serves FR-DA-4's site-change cancellation in Stage 7 without a
second design. Building `spawn`/`shutdown` as a pair now is what makes Stage 7's site change
"drop this pipeline, spawn another" rather than a refactor.

**Test:** `spawn` with an injected chunk source (§4.5), assert all handles join within a
short timeout, assert `shutdown` is safe if the pipeline already exited on its own.

### 4.3 Supervision

`panic = "unwind"` is deliberate (root `Cargo.toml`): a panic in one task must not take
down the workstation mid-warning. That guarantee is only real if something notices the
task died.

**DECIDE (S2-d): restart panicked pipeline tasks with capped exponential backoff,
indefinitely.** *Recommendation: yes.* A supervisor task awaits each `JoinHandle`; on
`Err(JoinError::panic)` it reports a typed event, updates status, and respawns after a
backoff of 1 s doubling to a 30 s cap, resetting after 5 minutes of clean running. Not a
bounded retry count: during an event, an application that has given up permanently is
worse than one that keeps trying and says so. Restarting the assembler from `Idle` costs
at most one volume of continuity, and ADR-0012's `TimedOut`/missing-chunk paths already
define that degraded behaviour.

Factor the backoff into a pure function — `fn backoff(consecutive_restarts: u32) ->
Duration` — and unit-test the sequence and the cap. Same shape as `next_target` and
`cold_start_baseline`: the decision is pure, the I/O is not.

New `event::Event` variants: `TaskPanicked { task: TaskKind }`, `TaskRestarted { task,
after: Duration }`.

### 4.4 Runtime sizing

`worker_threads(2)`. The entire Stage 2 workload is one poll every five seconds, one
BZ2 decompression per chunk, and one decode — this is not a throughput problem, and the
default (one worker per core) would allocate eight-plus threads per instance against
NFR-P-1's four-simultaneous-instances requirement and "Lightweight by Design". rayon gets
its own pool at Stage 3, sized separately. Record the number as a named constant with this
reasoning, and re-measure at Stage 4 when there is a real render loop competing for cores.

### 4.5 The test seam

`S3Poller` needs a real `http_ingest::Client`, whose host allowlist is compile-time. Rather
than weaken that (ADR-0014 exists precisely to prevent this kind of erosion), split the
spawn:

```rust
impl Pipeline {
    pub fn spawn(rt, site, state, cfg) -> Self;                      // production
    fn spawn_from_chunks(rt, rx: mpsc::Receiver<ChunkEnvelope>, state) -> Self;  // tests
}
```

`spawn` builds the poller and delegates. Everything below the poller — assembly, applier,
supervision, shutdown — is exercised offline by feeding `rx` from a test. One `#[ignore]`d
live test runs the real thing end to end against a live site, asserting that state becomes
non-empty within 60 s and printing the wall-clock time to first visible sweep (a direct
comparison against Stage 1's measured 1.5 s, now through the full wiring).

---

## 5. S2-W3 — Bundled NEXRAD site list

**Requirements:** FR-MU-3, FR-SS-1. Blocked on nothing.

New module `crates/radar-workstation/src/sites.rs` plus a generated table.

### 5.1 Representation

**DECIDE (S2-e): a generated `const` Rust table, not a bundled JSON file parsed at
startup.** *Recommendation: the Rust table.* `overview.md` and ADR-0006 both say "a
bundled JSON file". Honouring that literally means either adding `serde_json` for data
that never changes at runtime, or hand-rolling a JSON parser and putting it on the startup
path — and a startup path that can fail to parse data we ship ourselves is a failure mode
invented for no benefit (Stability as Ethics). A generated table:

```rust
pub struct Site {
    pub id: &'static str,       // "KDOX"
    pub name: &'static str,     // "Dover"
    pub state: &'static str,    // "DE"
    pub lat: f64, pub lon: f64,
    pub elevation_m: i32,
}

pub static SITES: &[Site] = &[ /* generated, sorted by id */ ];
```

costs zero dependencies, zero startup work, zero runtime failure modes, and is validated
by the compiler. ~160–200 entries is a few kilobytes of `.rodata`.

This supersedes a clause of ADR-0006. Add a **dated erratum** to ADR-0006 (following the
pattern ADR-0014 established) and correct `overview.md`'s "bundled JSON file" line — do not
silently diverge.

### 5.2 Generation and provenance

The source data is a US government public-domain registry (NOAA/NWS station list). Fetching
and converting it is development tooling, not production code, so it belongs in `utility/`:

- Add `utility/nexrad-sites/` (or a script alongside `utility/nexrad-inspect/`, whichever
  fits the existing shape better) that reads the registry export and emits
  `crates/radar-workstation/src/sites_generated.rs`.
- Commit both the source export and the generated file. Record in `utility/README.md`:
  where the data came from, the date retrieved, its public-domain status, and the exact
  command to regenerate.
- Filter to **operational WSR-88D sites only**. TDWR and other radar types are out of
  scope (Restraint is a Feature), and including them would put site IDs in the picker that
  the chunk bucket has no data for.

### 5.3 API and validation

```rust
pub fn all() -> &'static [Site];
pub fn by_id(id: &str) -> Option<&'static Site>;   // binary search; case-insensitive
```

Nothing more. `nearest(lat, lon)` waits for FR-SS-3's clickable markers in Stage 4/7 —
adding it now would be an untested function with no caller.

Tests: the table is sorted and has no duplicate IDs (a debug assertion *and* a unit test —
`by_id`'s binary search depends on it); every entry has a plausible lat/lon and a 4-character
ID; known-good spot checks (KDOX and KTLH against the values in `CLAUDE.md`, which came from
real decoded RVOL blocks — a genuine cross-source check, not a tautology).

**One `#[ignore]`d live test worth writing:** list the chunk bucket's top-level
`CommonPrefixes` (the existing `Client::list_prefix` with `delimiter=/` does this already)
and diff against the bundled list. Report sites present in the bucket but missing from the
table, and vice versa. This turns "is our site list current?" from an opinion into a
command, and it re-uses machinery that already exists rather than adding any.

---

## 6. S2-W4 — Configuration, paths, and the command line

**Requirements:** FR-CP-1, FR-CP-2, FR-CP-3; FR-DA-2's configurable-interval clause.

New modules `crates/radar-workstation/src/config/` and `src/paths.rs`.

### 6.1 XDG paths — one module, no crate

**DECIDE (S2-g): compute XDG paths with `std::env`, not the `directories` / `dirs`
crate.** *Recommendation: no crate.* The application is Linux-only (`REQUIREMENTS.md`), so
this is three environment variables with documented fallbacks — roughly thirty lines,
against a crate whose value is cross-platform behaviour we do not need.

```rust
pub fn config_dir() -> Option<PathBuf>;   // $XDG_CONFIG_HOME/radar-workstation, else $HOME/.config/...
pub fn cache_dir()  -> Option<PathBuf>;   // Stage 5's tile cache (FR-MU-5) uses this
pub fn data_dir()   -> Option<PathBuf>;   // Stage 3's user colour tables (FR-CT-3) use this
```

All three now, in one place, even though two have no caller until later stages — they are
the same six lines each and splitting them across three stages guarantees three subtly
different implementations. `Option`, not `PathBuf`: `HOME` unset is a real condition on
service accounts and must degrade to "run with defaults, persist nothing", never panic.
Relative paths in `XDG_*_HOME` must be rejected per the XDG spec, not joined blindly.

### 6.2 Format and parser

**DECIDE (S2-f): a workspace-local `key = value` parser, or `toml` + `serde`?**
**This one needs the user** — option 2 adds dependencies, and CLAUDE.md is explicit.

1. **`toml` + `serde` + `serde_derive`.** Familiar, well-specified, derive-driven. Costs
   several packages including proc-macro machinery, on a graph that ADR-0014 worked hard to
   get down to 78, for a config file that will have single-digit keys at Stage 2.
2. **A workspace-local line-oriented parser.** `# comment`, `key = value`, dotted keys for
   grouping (`ingest.poll_interval_seconds = 5`), values as string/int/bool/float. ~150
   lines plus tests. Fits the `http-ingest` precedent exactly: this is an untrusted-input
   parser on a must-not-crash path, and this project's answer to that has been to own it
   and fuzz it.

*Recommendation: option 2*, on the strength of FR-CP-3 (must start with a corrupt config),
NFR-SEC-2, and the fact that `crates/fuzz-support` already exists to test exactly this
shape of parser. If option 1 is chosen instead, the fuzz work in §6.5 still applies —
`serde` deserialisation of hostile input is not automatically safe from panics in *our*
`TryFrom` glue — and ADR-0019 records the dependency instead of the parser.

Parsing is **lenient by construction**: an unparseable line is skipped and reported as a
typed event, never fatal. A key with a bad value falls back to that key's default and is
reported. There is no error return from "load config" at all — only `(Config, Vec<Event>)`.
FR-CP-3 is then a property of the signature, not of a code path someone has to remember to
test.

### 6.3 Round-tripping without destroying the user's file

Two problems share one solution:

- FR-CP-2 lets the user hand-edit the file, including comments. Rewriting it wholesale
  from a struct erases them.
- NFR-P-1 expects four instances running at once. Wholesale rewriting means the last one to
  save clobbers every setting the others changed.

**Save is read-modify-write at the line level:** re-read the file, replace the value on
lines whose keys this instance changed, append genuinely new keys, and leave every other
line — comments, blank lines, unknown keys, other instances' settings — byte-identical.
Then write to a temporary file in the same directory and `rename` over the target, so a
crash or a full disk mid-write can never leave a truncated config (Stability as Ethics).

Test the three properties directly: comments survive a save; an unknown key survives a
save; a concurrent-instance simulation (write A, write B, re-read) preserves both changes.

### 6.4 The configuration surface, and the command line

Deliberately tiny at Stage 2 — every key here is a key that must be supported forever:

| Key | Default | Notes |
|---|---|---|
| `site` | none | ICAO ID. Validated against `sites::by_id`; unknown ID falls back to default and reports |
| `ingest.poll_interval_seconds` | 5 | FR-DA-2. Clamped to a documented `[2, 60]`; a clamp is reported, not silent — an unclamped small value would hammer a public bucket |

Stage 1's watchdog timeout and recovery thresholds stay named constants, per that plan's
explicit decision. They become config keys only if operational experience says so.

**The command line takes one positional site argument**, which overrides `site` for this
instance:

```
radar-workstation [SITE] [--config PATH] [--help] [--version]
```

Hand-rolled over `std::env::args_os` — a positional and three flags do not justify `clap`.
This is the multi-instance ergonomic (NFR-P-1: four instances, four sites, one config
file), and it is what makes §6.3's read-modify-write coherent: the CLI selects the site,
the config remembers the default, and only an explicit in-UI site change (Stage 7) writes
`site` back.

Site resolution order, tested: CLI argument → config `site` → **DECIDE (S2-h): what
happens with neither?** *Recommendation: exit with a usage message listing a few example
site IDs, exit code 2.* The alternative — defaulting to some arbitrary site — starts a
network connection to a site the user never asked for, which sits badly against BC-1.
Stage 4 can replace this with a site-picker on an empty display; at Stage 2 there is no
display to pick on.

### 6.5 Tests

Round-trip; missing file → defaults, no error; unreadable file (permissions) → defaults,
reported; corrupt file (binary garbage, truncated mid-line, `=` with no key, duplicate
keys, a 10 MB single line) → defaults or partial values, never a panic; unknown key
preserved across save; comment preserved across save; atomic-write leaves no partial file;
a `mutated_config_never_panics` test using `crates/fuzz-support`'s seeded mutator over a
small committed corpus, exactly as `nexrad-decoder` and `http-ingest` do it. Three parsers
in this workspace now share one mutator — which is the return on S1-d's decision to give
it a home.

---

## 7. Ordering, and what this plan deliberately does not do

**Recommended order: S2-W1 → S2-W2 → S2-W3 → S2-W4.**

State first because everything else writes into it. The runtime skeleton second, with the
site as a required CLI argument and no config — that produces a running, observable program
at the earliest possible point, which is the same instinct that put "get one pixel on
screen early" in the inventory's ordering rationale. The site list third, which turns the
raw string argument into a validated `&'static Site`. Configuration last, which turns the
required argument into an optional one. Each step leaves the binary working.

Not done here, recorded so a later session does not read these as oversights:

- **No compute layer and no stub for one.** A stub would encode an answer to Q8, Q11 or
  Q17 before those are asked. `state/apply.rs` documents the single line where Stage 3's
  compute dispatch attaches; that is the whole of Stage 2's contribution to it.
- **No window, no wgpu, no egui.** Stage 4. §4.1's runtime shape is the only accommodation
  made for it, because it is the only one that is expensive to retrofit.
- **No product or texture fields in `AppState`.** Same reason. `revision` and the
  applier's shape are designed so adding them is additive.
- **No site switching.** `RadarState::reset(site)` and `Pipeline::shutdown()` exist and are
  tested; wiring them to a UI action is Stage 7 (FR-DA-4, FR-SS-2).
- **No signal handling.** `tokio`'s `signal` feature pulls `signal-hook-registry` and
  friends into a 78-package graph, to catch a Ctrl-C whose default disposition is already
  correct for a headless skeleton that persists nothing on exit. Stage 4's window-close
  event is the real shutdown trigger; revisit there. (**S2-i**, no user decision needed
  unless the recommendation is rejected.)
- **No logging crate.** S1-f's decision stands; §3.4 extends the typed-event mechanism
  rather than adding a second one.
- **No tile, placefile, or archive-failover code.** Blocked on Q16, Q6, Q14 respectively.

---

## 8. Risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Two volumes of retained sweeps approach the < 200 MB target | **Medium** | High — a headline NFR | Measure peak RSS across a volume boundary (§9) *before* Stage 3 designs texture retention. If tight, Stage 3 drops raw radials for undisplayed products after compute; do not pre-optimise in Stage 2 |
| `#[tokio::main]` written by reflex, then fought at Stage 4 | Medium | Medium | §4.1 is explicit; the replacement-point comment in `main.rs` is the reminder |
| The Q4 answer proves wrong under a real 60 fps read load | Low | Medium | `snapshot()` is the only read API, so swapping the interior to a double-buffer/`ArcSwap` scheme touches one function. ADR-0018 records this as the anticipated refinement path |
| Config save clobbers a concurrent instance's settings | Medium | Low | §6.3's line-level read-modify-write; tested with a two-instance simulation |
| The bundled site list goes stale | High (over time) | Low | The `#[ignore]`d bucket-diff test in §5.3 makes staleness a one-command check; the regeneration command is in `utility/README.md` |
| Design drift: `overview.md`/`data-flow.md`/`CLAUDE.md` keep saying `Arc<RwLock<AppState>>` | Medium | Medium | The doc corrections are listed as work in §3.6 and appear in the commit sequence, not as follow-up |
| ADR-0006's "bundled JSON" clause is superseded silently | Medium | Low | Dated erratum required by §5.1, following ADR-0014's pattern |
| Supervision masks a real bug by restarting forever | Low | Medium | Every restart emits a typed event and updates `IngestStatus`; the backoff cap makes a crash-loop visible in the log rather than silent |

---

## 9. Measurements to record in §10 Results

Numbers, not impressions — the convention `dependency-inventory-remediation.md` §9 and
`stage-0-1` §8 established.

- Wall-clock from process start to the first sweep visible in `AppState` (live, one site),
  against Stage 1's 1.5 s poller-only figure.
- Peak RSS after: startup, one complete volume, four complete volumes, and specifically
  *across* a volume boundary. Against the < 200 MB target.
- Thread count of the running process (against "Lightweight by Design" and NFR-P-1).
- Steady-state CPU over a full volume cycle, one instance and four instances.
- `Cargo.lock` package count before and after (78 at baseline — expected unchanged unless
  S2-f chooses option 1).
- Release binary size before and after the site table.
- Time to load and parse configuration at startup (should be sub-millisecond; it is on the
  < 2 s first-render path from Stage 4 onward).
- Whether the bucket-diff test (§5.3) reports any site mismatch, and which.

---

## 10. Suggested commit sequence

Each line is one reviewable commit; each keeps `cargo test --release --workspace` and
`clippy -D warnings` green.

1. `state` module: `RadarState`, `AppState`, `snapshot()`, `apply()` + unit tests *(S2-W1)*
2. Event log + `AppState::report`; existing `log_to_stderr` call sites migrated *(S2-W1)*
3. ADR-0018; Q4 moved to Resolved; `overview.md` / `data-flow.md` / `CLAUDE.md` corrected *(S2-W1)*
4. Shutdown arms on `S3Poller::run` and `assembly::run` *(S2-W2)*
5. `Pipeline::spawn` / `shutdown` + `spawn_from_chunks` test seam *(S2-W2)*
6. Supervision + `backoff()` + panic-restart test + new `Event` variants *(S2-W2)*
7. `main.rs`: explicit runtime, headless loop, replacement-point comment *(S2-W2)*
8. Live end-to-end `#[ignore]`d test through the full wiring *(S2-W2)*
9. `utility/` site-list generator + source export + `utility/README.md` provenance *(S2-W3)*
10. `sites.rs` + generated table + tests; ADR-0006 erratum; `overview.md` corrected *(S2-W3)*
11. `paths.rs` (config/cache/data) + tests *(S2-W4)*
12. Config parser + corpus + mutator test; ADR-0019 per the S2-f decision *(S2-W4)*
13. Config save (line-preserving, atomic) + tests *(S2-W4)*
14. CLI parsing, site resolution order, config wired into `Pipeline::spawn` *(S2-W4)*
15. `README.md` / `docs/README.md` status and plans-index updates; `project-inventory.md`
    Stage 2 marked done

---

## 11. Open decisions summary

| # | Decision | Recommendation | Needs the user? |
|---|---|---|---|
| S2-a | Interior locks on a shared `AppState` vs. one outer `RwLock<AppState>` | Interior locks, radar-only lock scope | No — but it changes three documents and gets ADR-0018 |
| S2-b | Retention: merged per-elevation sweeps + last complete volume | Yes; measure RSS before Stage 3 | No |
| S2-c | Explicit tokio runtime, main thread reserved | Yes — expensive to reverse at Stage 4 | No |
| S2-d | Panic supervision and restart policy | Capped exponential backoff, indefinite retries | No |
| S2-e | Site list as a generated `const` table vs. bundled JSON | Generated table; ADR-0006 erratum | Worth confirming — it supersedes an ADR clause |
| S2-f | Config format: workspace-local parser vs. `toml` + `serde` | Workspace-local parser | **Yes** — option 2 adds dependencies |
| S2-g | XDG paths via `std::env` vs. the `directories` crate | `std::env`, one module | No if the recommendation stands; **yes** if the crate is wanted |
| S2-h | No site on the CLI and none in config | Usage message, exit 2 | No |
| S2-i | Ctrl-C / `tokio` `signal` feature | Defer to Stage 4; no feature added now | Worth confirming — it is a visible behaviour choice |

---

## 12. Results

All of S2-W1 through S2-W4 were implemented and are green on
`cargo build --release --workspace`, `cargo test --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny check`, and
`cargo audit`. `cargo run --release -- KDOX` is a real, runnable program. Measured
numbers below, not impressions — same convention as `stage-0-1-close-the-acquisition-
path.md` §8.

**Open decisions.** All recommendations in §11 were accepted, including the three that
needed the user (S2-e, S2-f, S2-i) — asked and confirmed before implementation began.

**Tests.** 244 tests passing, 9 `#[ignore]`d live-network tests, 0 failing, across the
whole workspace (up from 123 passing / 12 ignored at the Stage 0/1 baseline).
`radar-workstation` alone: 106 unit tests (`--lib`), 13 in the CLI binary's own test
target, plus the `assembly_live`, `pipeline_live`, and `config_hardening` integration
test files.

**Wall-clock time to first visible sweep, through the full `Pipeline`/`AppState`
wiring** (live KDOX, VCP 35, `tests/pipeline_live.rs`): **3.565 s** from
`Pipeline::spawn` to the first non-empty `AppState::snapshot()`. Against Stage 1's
1.5 s poller-and-assembler-only figure, the difference is the added S3 listing/cold-
start overhead now measured end-to-end rather than in isolation — still far under
FR-DA-3's 30–60 s target and BC-1's < 2 s first-render budget has ~1.5 s of headroom
left for Stage 4's own startup cost.

**Memory.** Peak RSS measured live (KDOX): **~29 MB** at 2 s (3 sweeps closed),
**~44 MB** at 90 s (6 of 14 sweeps closed, one volume still in progress) —
comfortably under the < 200 MB target with more than half the sweeps yet to close.
**Not measured this session:** RSS specifically *across* a volume boundary (the "two
volumes' worth" transition risk ADR-0018 §12 flags) — a full KDOX VCP 35 volume takes
~5–6 minutes to close, longer than fit in this session. Given ~44 MB at under half a
volume, the risk that a full transition approaches 200 MB now looks unlikely, but
should still be measured directly (`VmHWM` across a `VolumeClosed` event) before
Stage 3 designs texture retention on top of it, per the risk table's own mitigation.

**Threads.** 4–5 total (main thread, 2 tokio `rw-io` workers, tokio's internal timer
driver, and the headless stdin-reader thread while it's alive) — well inside
"Lightweight by Design" and NFR-P-1's four-simultaneous-instances headroom.
Steady-state CPU across one/four instances was not measured this session (would need
several 5–6 minute volume cycles per instance); flagged as follow-up alongside the
volume-boundary RSS measurement above.

**Dependency graph.** `Cargo.lock`: **67 packages, unchanged** before and after this
plan (`fuzz-support`, added as a `radar-workstation` dev-dependency, was already a
workspace member counted in that total — zero new external crates). Confirms S2-f's
"no new dependency" recommendation held in practice, not just in the decision.

**Binary size.** Release build of `radar-workstation`: **2,857,072 bytes** (~2.8 MB),
including the 163-entry bundled site table.

**Configuration load time:** **60.1 µs** for a realistic 3-line file
(`config::tests::load_of_a_realistic_file_is_fast`, run with `--nocapture`) — several
orders of magnitude under the "should be sub-millisecond" target, kept as a permanent
regression-guard test (asserts < 50 ms) rather than a one-off measurement.

**Bucket-diff live test** (`bucket_site_prefixes_match_bundled_site_list`, run live
2026-07-31): bundled table has 163 sites; the chunk bucket's top-level prefixes have
203. The 46 present in the bucket but absent from the table are every one a `T`-prefixed
TDWR code (plus one `FOP1`) — exactly the station type the bundled table deliberately
excludes (Restraint is a Feature), confirming the filter in `utility/nexrad-sites/
generate.py` is working as intended, not a gap. The 6 present in the table but absent
from the bucket — `KCRI`, `KLIX`, `KOUN`, `LPLA`, `RKJK`, `RODN` — are informational,
not necessarily wrong: `KCRI`/`KLIX`/`KOUN` may simply not have produced a volume in the
bucket's 24h retention window at the moment this test ran, but `LPLA` (Azores), `RKJK`
(Kunsan), and `RODN` (Kadena) are the overseas DoD-operated WSR-88D sites
`utility/README.md` already flagged as uncertain — this is now direct evidence they do
not currently publish to the public real-time chunk bucket, though they remain
legitimate operational WSR-88D installations per the NOAA HOMR source data and are kept
in the table on that basis.

**Findings that contradicted or extended the plan:**

- **`AssemblyEvent::SweepClosed` had to grow two fields.** The plan's `DisplaySweep`
  (§3.2) needs a `VolumeId` and `vcp_number` per sweep, but nothing in the existing
  `AssemblyEvent` carried that — `Sweep` itself has no volume-identifying fields. Rather
  than have `state::apply` re-derive or guess this, `VolumeId` was added to
  `crate::assembly` (not `state`, where the plan sketched it — see ADR-0018) and
  `SweepClosed` now carries `volume`/`vcp_number`, read from `VolumeContext` at the exact
  point a sweep closes (which is always after that context is populated). This is the
  single largest deviation from the plan's literal type sketch, though not from its
  intent.
- **Literal per-task supervision (§4.3's "await each `JoinHandle`") is not
  implementable for this pipeline** — a panic in one task drops that task's owned
  channel endpoint, which permanently closes the channel for its peer regardless of
  whether the peer itself panicked, so an independently-restarted task can never be
  handed a replacement for an endpoint it never owned. The poller/assembly/applier trio
  is supervised as one unit instead (`pipeline::supervise`, `run_ingest_trio`) — a panic
  anywhere in it tears down and rebuilds all three together with fresh channels. Every
  externally-visible property S2-d asked for still holds (typed `TaskPanicked`/
  `TaskRestarted` events, capped exponential backoff, indefinite retries, ~one volume of
  lost continuity per restart); see `pipeline.rs`'s top-level doc comment for the full
  reasoning. `TaskKind` correspondingly has one variant, `IngestPipeline`, not
  per-subsystem ones.
- **`S3Poller` needed a signature change beyond what §4 anticipated:** `status_tx`
  moved from being created inside `S3Poller::new` to being passed in externally
  (`watch::Sender<IngestStatus>`), and `poll_once`/`apply_recovery`/`list_volume_folders`
  now take an `&AppState` to report through. Neither is mentioned in the plan, but both
  are direct consequences of decisions the plan *does* make: `AppState` holds a
  `watch::Receiver<IngestStatus>` that must outlive any individual `S3Poller` a restart
  creates (S2-a), and "every current `log_to_stderr` call site inside a task that has an
  `Arc<AppState>` moves to `report`" (§3.4) applies transitively to a task's private
  helper methods, not just its top-level `run` function.
- **`Pipeline::spawn` takes `&tokio::runtime::Handle`, not `&tokio::runtime::Runtime`**
  as literally sketched in §4.2/§4.5. A `Handle` is what `Runtime::handle()` gives
  (production call sites in `main.rs` are unaffected) and also what
  `Handle::current()` gives from inside an already-running async context — which is
  what makes `spawn_from_chunks` callable from a plain `#[tokio::test]` async fn at all,
  without standing up a second nested `Runtime` just to get a `&Runtime` to pass in.
- **`spawn_from_chunks` does not itself exercise panic-restart.** An `mpsc::Receiver`
  is a single-use resource that cannot be reconstructed after a panic drops it (the
  same constraint behind the trio-supervision finding above), so the test-provided `rx`
  can only ever be consumed once. Panic-restart is instead tested generically, directly
  against `pipeline::supervise`, with a synthetic repeatable task factory
  (`supervise_restarts_after_panics_and_reports_typed_events`) — `spawn_from_chunks`'s
  own tests cover exactly what §4.2 asked for (clean join on shutdown, shutdown safety
  after the pipeline already exited).
- **The NOAA site list source turned out to be directly fetchable** (`https://
  www.ncei.noaa.gov/access/homr/file/nexrad-stations.txt`, NCEI's HOMR export) rather
  than needing to be assembled by hand — 163 operational WSR-88D sites (filtered from
  210 total rows, the rest TDWR) with ICAO id, name, state, lat/lon, and elevation, all
  cross-checked against `CLAUDE.md`'s KDOX ground truth (lat/lon match to 4 decimal
  places). One genuine ambiguity surfaced: HOMR's `ELEV` field (164 ft for KDOX) does
  not match the RVOL-decoded `site_amsl_m` (15 m) — the two almost certainly measure
  different reference points (total antenna/feedhorn elevation vs. bare site elevation;
  15 m site + a plausible ~34 m tower ≈ 164 ft), so the bundled table's `elevation_m` is
  documented as HOMR's figure and the cross-source spot-check test compares only
  lat/lon, not elevation, against the decoder's ground truth.
