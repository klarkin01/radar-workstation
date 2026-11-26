# ADR-0033: Launch Backfill From the Chunk Bucket

## Status
Accepted (2026-11-25)

Part 2 of `design/usability-map-over-radar-and-backfill.md`, implementing D1 of
`docs/plans/stage-6a-time-handling.md` §3 and the v1.0 recommendation of
`docs/plans/stage-6a-checkpoint.md` CK-21. Adds FR-DA-11 and moves D1 into
`REQUIREMENTS.md` §6's in-scope list. D2–D4 (archive reader, historical mode,
enter-a-time) are not decided here and stay deferred.

## Context

The cold start takes only the newest volume (`cold_start_target`,
`ingest/s3_poll.rs`). That is right for first render, but the history ring
(ADR-0030) then fills at the radar's own pace, one volume every 4–10 minutes. The
loop the timeline plays (ADR-0031) is empty for the first 40–60 minutes after every
launch, which is exactly when an operator who has just started the app during an
event wants it.

The chunk bucket keeps about 48 h of volumes (ADR-0011 erratum), so the volumes
behind the live one are almost always still there. Fetching them needs no new
format, decoder or bucket: they are the same `-S`/`-I`/`-E` chunks the live poller
already reads.

## Decision

### 1. Live first, then backfill behind it

The cold-start path is unchanged. The newest volume still arrives first, so the
< 2 s first-render target holds. The live poller hands its cold-start listing and
anchor to backfill only after the poll that chose the anchor has delivered its
chunks (`S3Poller::pending_cold_start`, `ingest::backfill::AnchorSlot`). The bucket
is therefore listed once, not twice. The slot is one-shot and shared across
supervised restarts, so a panic in the ingest pipeline never starts a second
backfill.

`s3_poll::backfill_targets` walks back from the anchor through the retained arc
with `VolumeSeq::pred`. The walk is wrap-aware (`1 → 999` is one step) and stops at
the arc's oldest end. It returns targets newest first and skips sequence numbers
missing from the listing rather than fetching them.

### 2. N is the retention policy, not a second count

Backfill asks for `history.frames − 1` volumes
(`RetentionPolicy::backfill_volumes`). The ring's byte budget stops it earlier at
run time. One boolean, `history.backfill` (default `true`), turns it off for
operators on metered or locked-down links. With `history.budget_mb = 0`
(`RetentionPolicy::DISABLED`) backfill never starts.

### 3. A separate task, with its own connection and its own chain

Backfill is its own supervised unit (`pipeline::run_backfill`, `TaskKind::Backfill`).
It has its own `S3Client`, assembler, compute stage and applier, and shares no
channel with the live quartet. There are two reasons:

- `http-ingest` sends one request at a time on one connection (ADR-0014). Sharing
  the poller's client would hold live polling behind a whole volume of backfill
  fetches.
- `VolumeAssembler` tracks one volume at a time. Interleaving two volumes' chunks
  into one assembler would corrupt both. The compute stage's retained reflectivity
  set for Echo Tops/VIL is per volume for the same reason.

The chain is the same code live data runs through (`pipeline::run_chain`). Only the
applier's `state::Origin` differs, and only the ring insert reads it.

### 4. Disclosure: a second connection to the same bucket

While backfill runs, the instance holds **two** HTTPS connections to
`unidata-nexrad-level2-chunks.s3.amazonaws.com`: the live poller's and backfill's.
Both are user-initiated in the same sense the live connection is, because the user
started the instance for that site. Backfill's connection is dropped when backfill
stops. It fetches only the volume directories named by the cold start's listing.
`history.backfill = false` removes it. `QUICK-START.md` states the same.

### 5. The ring inserts behind the head and never displaces it

`FrameRing::behind_mut` places a backfilled frame at its `VolumeId` position. It
never creates a frame newer than the head of a non-empty ring. An empty ring admits
the volume, because backfill's first sweep can land before live's.

It refuses a new frame when `FrameRing::has_room_behind` is false, which is any of
the following:

- the ring is at `history.frames`;
- it is at `history.budget_mb`;
- `trim` has evicted anything since the last `clear`.

The last condition is what prevents churn. Without it, a backfill frame evicted
mid-fill would be re-created by its own next sweep, and evicted again, once per
sweep. A refused backfilled update is dropped silently. The backfill task reports
the stop instead (§6).

The window then moves forward with no new code. Live volumes push the head, and
`trim` evicts the oldest frame, as before.

The read side needs no change. `live_sweeps`/`live_derived` fold newest first, so
the head wins every elevation it has, and a frame inserted behind it only fills
what it lacks. This is the same carry-forward FR-DA-3 always did. The timeline's
`Paused(VolumeId)` focus is keyed by volume, not index, so an insert behind it does
not move the display. Both properties are tested.

### 6. Fetch one volume at a time, ask the ring before sending

The fetch loop (`ingest::backfill::fetch`) lists and fetches each target volume
whole, retrying up to three times, and only then feeds its chunks to the backfill
assembler in key order. A retry therefore never feeds a volume twice. Newest first
means a backfill that stops early has still filled the most recent part of the
loop.

Before **sending** a volume, the fetcher waits until the backfill applier has
closed the volume it sent before (`apply_loop`'s `volumes_closed` counter, capped
at 30 s), and then asks the ring for room. Without that wait the answer trails by
everything buffered in the chain. The first measurement (§8) showed that lag at
about two volumes: backfill fetched 11 volumes into a ring that kept 9. The next
volume's download still overlaps the previous one's gridding, so a full ring wastes
at most one download that is never sent. One volume can still be gridded and then
evicted, because a volume's size is not known until it has been gridded.

It stops when:

- all N volumes are fetched (`BackfillStop::Complete`);
- the listing runs out (`ListingExhausted`, the ~48 h retention edge);
- the ring has no room (`HistoryFull`);
- a volume fails all three attempts (`Failed`);
- `shutdown` fires (`Shutdown`).

Each stop is reported once, as `Event::BackfillFinished { fetched, requested, reason }`,
after the chain has drained. A supervised restart resumes at the volume that was in
flight (`FetchPlan::progress`), not from the start.

### 7. Progress is visible

`IngestStatus::backfill` is `Off`, `Backfilling { done, of }` or `Complete`. The
backfill task writes it with `send_modify` into the channel the poller already
publishes, and each writer touches only its own field. The status bar shows
"Backfill k/N" while it runs, so the operator can always see why the loop is
growing. The headless state line carries `backfill=…`. `--headless` runs the same
pipeline and backfills too (verified 2026-11-25, KDOX).

### 8. Measured

`ingest::s3_poll::tests::backfill_twelve_frames_latency_and_peak_rss`
(`#[ignore]`, live network) runs the production `Pipeline::spawn` with
`history.frames = 12` and the default 320 MB budget against KDOX. It measures the
process's peak RSS (`VmHWM`). There is no GPU in that process, so this is the
pipeline and ring alone. The windowed app adds driver mappings on top (ADR-0030,
checkpoint §2.4).

Run on 2026-11-25 (release build, KDOX):

| Run | First live sweep | Backfill done | Frames retained | History bytes | Peak RSS | Stop |
|---|---|---|---|---|---|---|
| Before the settle wait (§6) | 3.47 s | 44.7 s | 9 | 310 MiB | 460 MiB | `Complete`, 11 of 11 fetched |
| Shipped | 1.11 s | 42.6 s | 10 | 318 MiB | 482 MiB | `HistoryFull`, 10 of 11 |

Peak RSS is under the ADR-0030 target of < 200 MB + `history.budget_mb` (520 MB). The
first live sweep is within the < 2 s target in the shipped run. The 3.47 s in the
first run is most likely network variance on the cold connection. In both runs
backfill starts only after the live poll has returned, so it cannot delay the live
fetch. Its gridding does share the blocking pool with the live volume's, so this is
worth re-measuring on a slow machine. The byte budget, not `history.frames`, is what
bounds a KDOX VCP 35 loop at the defaults, as ADR-0030 §1 predicted.

## Consequences

- The loop is populated about 45 s after launch instead of 40–60 minutes.
- Launch costs about 11 volumes of chunk downloads (tens of MB) on the second
  connection. `history.backfill = false` is the remedy on a metered link.
- `Frame::first_applied` is arrival time, not scan time. For a backfilled frame they
  differ by up to an hour. Scan time is the frame's `VolumeId`.
- A backfilled frame whose budget-bound insert was refused is never retried. Once
  the ring is full, older history is not wanted until something is evicted, and by
  then those volumes are older still.
- `HistoryBudgetBound` keeps its definition (pushed ≥ `history.frames` while
  retaining fewer). A backfill that the budget stops short of `history.frames`
  reports `BackfillFinished { reason: HistoryFull }` instead, and the budget-bound
  event follows once live volumes push the count past `history.frames`.

## Alternatives considered

- **Backfill through the live poller.** This was rejected. One connection and one
  assembler would stall live polling for the length of a volume's fetch, and the
  poller's per-volume state machine would have to learn to walk backwards.
- **A separate `backfill.frames` count.** Rejected: it would be a second number
  that must agree with `history.frames` and can only be wrong in either direction.
- **Fetch all targets concurrently.** Rejected: `http-ingest` has no connection
  pool (ADR-0014), and N connections would be N undisclosed-by-default connections.
  The measured 42 s is acceptable.
