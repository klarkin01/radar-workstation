# ADR-0018: Shared Application State Structure (Q4)

## Status
Accepted

## Context
`overview.md` and `data-flow.md` both specify `Arc<RwLock<AppState>>` as the
coordination point between the data pipeline, the (future) compute layer, and the
render loop, with `AppState` described as holding everything: the current volume scan,
derived product textures, active site configuration, loaded placefile data, tile cache
index, user settings (active product, color table, zoom, pan position), and application
status.

Q4 asked for the exact structure and lock granularity. Stage 2 (`docs/plans/
stage-2-make-the-application-exist.md`, S2-W1) is the first point at which that
structure has a real consumer — the volume assembly state machine (ADR-0012) now
produces `AssemblyEvent`s that need somewhere to land — and the first point at which
getting the lock scope wrong has a cost: the render loop does not exist yet, but its
one hard constraint already does (`BC-7`: it may never block on I/O, decoding, or
computation).

Three problems with the literal `Arc<RwLock<AppState>>` reading, found while designing
the type this ADR introduces:

1. **One lock over everything** forces every reader — including a 60fps render loop
   reading data it will never write — to take a lock's read side over fields it does
   not touch (config, the event log) and over data that is *already* synchronized
   elsewhere (`S3Poller::status()`'s `watch::Receiver<IngestStatus>`, S1-W3b). A second
   sync mechanism layered on top of the first is redundant, not extra safety.
2. **View state under the same lock as radar data** blurs a boundary `data-flow.md`
   already states in words ("the data pipeline does not modify user settings") but does
   not enforce in types. FR-NI-4's pan/zoom inviolability guarantee is far easier to
   hold when exactly one thread (the render loop) can ever move the viewport, and that
   is only true if the type system says so.
3. **Retention reasoning** (below) becomes harder against one large lock: everything's
   lifetime becomes the lock's lifetime, instead of being scoped to the one map that
   actually needs it.

## Decision

**One lock, over radar data only.** `Arc<AppState>`, where `AppState` holds
`RwLock<RadarState>` internally — not an outer `RwLock<AppState>`. `RadarState` is the
only data in the system with a genuine many-reads/occasional-write pattern at this
stage.

**View state is not shared at all.** Pan, zoom, active product, active sweep, and
window geometry (Stage 4+) are owned outright by the render loop. Nothing in the data
pipeline may write them, and nothing in `AppState` represents them — there is no type
for the data pipeline to reach through even by mistake.

**Ingest status is read through the channel that already publishes it.** `AppState`
holds the `watch::Receiver<IngestStatus>` from `S3Poller::status()` (S1-W3b) directly.
No copy, no sync step, no second source of truth.

**`snapshot()` is the only read API.** `AppState::snapshot(&self) -> StateSnapshot`
takes the read lock, clones `Arc`s and `Copy` fields, and drops the lock before
returning. There is deliberately no `fn read(&self) -> RwLockReadGuard<'_, RadarState>`
— "never hold a lock across a frame" becomes a property of the type system, not a rule
documented here for a later contributor to remember. The cost is one `Vec` allocation
and N `Arc` refcount bumps per call, where N is the sweep count (14 for KDOX VCP 35) —
negligible against a 16.6ms frame budget once there is a render loop to measure it in
(Stage 4).

**Retention: one merged sweep per elevation, plus the last complete volume.**
`RadarState` holds a `BTreeMap<u8, DisplaySweep>` (newest closed sweep per elevation
number, carried across volume boundaries so a closing volume never blanks the display)
and `last_complete: Option<Arc<VolumeScan>>`. Because `VolumeScan.sweeps` is already
`Vec<Arc<Sweep>>` (Stage 1), holding both costs only refcounts wherever they overlap,
which is most of the time — distinct memory is held only during a volume transition,
when some elevations have advanced and others have not. A super-resolution sweep is
~9MB; 14 sweeps ~130MB; a full transition therefore briefly approaches two volumes'
worth before the older `Arc`s drop. This is close enough to the 200MB target
(`REQUIREMENTS.md` §4.1) that it must be *measured*, not assumed — see this plan's §9/
§12 for the recorded numbers. If it proves tight, the mitigation belongs to Stage 3 (the
compute layer can drop raw radials for products nobody is displaying, once computed),
not to this ADR.

**Revision counter.** `RadarState::revision: u64` increments on every applied change.
The render loop (Stage 4) will compare it against the value it last uploaded to the GPU
and skip texture re-upload when unchanged (FR-DR-5: new scan arrivals must not cause a
perceptible frame-rate drop).

**The applier is a pure function**, mirroring `VolumeAssembler` (ADR-0012): `fn
apply(state: &mut RadarState, event: AssemblyEvent, now: Instant) -> bool`, with `now`
injected rather than read internally. Rules: `SweepClosed` replaces the entry for its
elevation number immediately (partial-scan rendering); a sweep from an older
`VolumeId` never replaces a newer one already displayed; `VolumeClosed` sets
`last_complete` only for `VolumeStatus::Complete` — `TimedOut`/`Superseded` clear
nothing (ADR-0012: a visible gap beats silently modifying rendered data); a VCP change
drops the old pattern's elevations (a new VCP has a different elevation set — an
incomplete volume in the *same* VCP does not); `LateRadialsDiscarded`/
`MissingStartChunk` are observability only, handled by `AppState::report`, and never
reach the applier's revision bump.

**`VolumeId`** (ordered by `(julian_date, scan_time_ms)`) lives on `AssemblyEvent`, in
`crate::assembly`, not in the `state` module — it is populated from `VolumeContext`
at the moment a sweep closes, which is data the assembler already holds. `state::apply`
consumes it; it does not derive it. This is a refinement of this plan's original
sketch, which placed the type in `state` without specifying where its value would come
from — keeping the computation next to the data it is computed from avoids a
duplicate, and keeps `AssemblyEvent` self-describing for any future consumer.

**Event log**: a bounded `VecDeque<(Instant, Event)>`, capacity 64, behind its own
`Mutex` in `AppState`, fed through `AppState::report` (which also calls
`event::log_to_stderr`) so the two sinks cannot drift apart. Bounded for the same reason
every channel in this design is bounded (below) — an unbounded diagnostic buffer in a
process that may run for hours during an active weather event is a memory leak with a
friendly name.

**Lock poisoning is recovered, not propagated.** `AppState`'s internal read/write
helpers use `.unwrap_or_else(|poisoned| poisoned.into_inner())` rather than
`.unwrap()`. A panic while holding the write lock (which Stage 2's supervision, S2-W2,
is specifically designed to survive by restarting the task) must not turn into a second,
permanent failure mode where every subsequent `snapshot()` — including the render
loop's, from Stage 4 on — panics too.

**Channels are bounded.** Poller → assembly (`ChunkEnvelope`, capacity 32) and assembly
→ applier (`AssemblyEvent`, capacity 64). Backpressure propagates naturally: a slow
applier blocks assembly's send, which blocks the poller's send, which delays the next
poll. This is bounded, visible in `IngestStatus`, and never grows memory — it is the
same design principle as the event log's bound.

## Alternatives Considered

**One outer `RwLock<AppState>`**, as `overview.md`/`data-flow.md` literally say. Rejected
per the three problems in Context: it makes the render loop take a lock over data it
never touches and data already synchronized elsewhere, and it makes the retention
question above harder to reason about because everything's lifetime becomes the lock's.

**Lock-free double-buffer / `ArcSwap`-style swap.** Not chosen for Stage 2 — an `RwLock`
with a short-held write side and a read side that only clones is simple, well
understood, and has not been shown to be a bottleneck (there is no render loop yet to
contend with it). Recorded here as the anticipated refinement path: because
`snapshot()` is the *only* read API, swapping `RadarState`'s interior for a
double-buffer/`ArcSwap` scheme touches one function if Stage 4 measurement shows
read-lock contention at 60fps. This is deliberate — the type boundary was chosen so
this alternative stays cheap to adopt later rather than needing to be decided now.

## Consequences
- The data pipeline holds `Arc<AppState>` and calls `apply_event`/`report`; nothing
  else touches `RadarState` directly.
- The render loop (Stage 4) will call `snapshot()` once per frame and own everything
  else about what it displays (pan, zoom, active product/sweep) independently.
- Q4 is resolved. Recorded in `docs/open-questions.md`'s Resolved section.
- `overview.md`, `data-flow.md`, and `CLAUDE.md`'s `Arc<RwLock<AppState>>` phrasing is
  corrected in the same change that adds this ADR — the "design drift" failure mode
  `project-inventory.md` §7 names is avoided by fixing the documents at the moment the
  code diverges, not later.
- `docs/adr/0006-bundle-shapefiles.md` gains a dated erratum for the NEXRAD site list
  representation (S2-W3, a separate but co-shipped decision) rather than a new ADR,
  since it supersedes one clause of an existing accepted decision rather than
  introducing a new architectural concern.
