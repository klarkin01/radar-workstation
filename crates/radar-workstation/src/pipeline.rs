//! Runtime skeleton (S2-W2): spawns, supervises, and cleanly shuts down the
//! data pipeline. `Pipeline::spawn` is the production entry point;
//! `spawn_from_chunks` is the test seam (§4.5) that skips the real
//! `S3Poller` so assembly/applier/shutdown are exercised offline.
//!
//! **Supervision granularity.** The plan this module implements
//! (`docs/plans/stage-2-make-the-application-exist.md` §4.3) describes a
//! supervisor awaiting "each" task's `JoinHandle` independently. That is not
//! implementable as written for this pipeline: the poller, assembly,
//! compute, and applier are connected by point-to-point `mpsc` channels,
//! each end owned by value. If one task's future panics, tokio drops
//! everything it owned during unwind — including its channel endpoint —
//! which permanently closes that channel for the peer on the other end,
//! whether or not that peer itself panicked. An independently-restarted
//! task cannot be handed a replacement for a receiver or sender it never
//! owned in the first place. So the poller/assembly/compute/applier quartet
//! (Stage 3 adds the compute stage between assembly and the applier) is
//! supervised as **one unit**: a panic anywhere in it tears down and
//! rebuilds all four together, with fresh channels and a fresh
//! `S3Poller`/`VolumeAssembler`. This still satisfies every property S2-d
//! asks for — a panic doesn't kill the process, it's reported as a typed
//! event, backoff is capped and exponential, retries are indefinite — and
//! "restarting from Idle costs at most one volume of continuity"
//! (ADR-0012) holds exactly as before, since the whole quartet re-enters
//! `Idle` together.
//!
//! **Launch backfill (ADR-0033) is a second, separately supervised unit.**
//! It shares no channel with the quartet: it has its own `S3Client` (a
//! second connection to the same bucket — `http-ingest` serializes
//! requests on one connection, so sharing the poller's would stall live
//! polling) and its own assembler, compute stage and applier, because
//! `VolumeAssembler` tracks one volume at a time and interleaving two
//! volumes' chunks would corrupt both. Its chain is the same code as live
//! data's ([`run_chain`]); only the applier's `Origin` differs. The one
//! thing that crosses between the two units is the cold start's listing and
//! anchor, handed over once through an [`AnchorSlot`].
//!
//! **`Handle`, not `Runtime`.** The plan sketches `Pipeline::spawn(rt:
//! &Runtime, ...)`. This takes `&tokio::runtime::Handle` instead —
//! `Runtime::handle()` gives one, so production call sites are unaffected,
//! but a `Handle` is also what `Handle::current()` gives from inside an
//! already-running runtime, which is what makes `spawn_from_chunks`
//! testable from a plain `#[tokio::test]` async fn without standing up a
//! second nested `Runtime`.

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::assembly::{self, AssemblyConfig, AssemblyEvent};
use crate::compute::{self, StateUpdate};
use crate::event::{Event, TaskKind};
use crate::ingest::backfill::{self, AnchorSlot, BackfillStatus, ColdStart, FetchPlan};
use crate::ingest::s3_poll::{self, IngestStatus, S3Poller};
use crate::ingest::volume_seq::VolumeSeq;
use crate::ingest::ChunkEnvelope;
use crate::sites::Site;
use crate::state::{AppState, Origin};

/// poller → assembly channel capacity. A volume is ~79 chunks; 32 absorbs a
/// burst without letting a stalled consumer buffer a whole volume of raw
/// bytes. Deliberately bounded (ADR-0018 §3.5) — backpressure here is
/// correct behavior (it delays the next poll rather than growing memory);
/// do not "fix" it by unbounding this channel.
const CHUNK_CHANNEL_CAPACITY: usize = 32;
/// assembly → compute channel capacity. One volume produces ~14
/// `SweepClosed` + 1 `VolumeClosed`; 64 is four volumes of headroom.
const EVENT_CHANNEL_CAPACITY: usize = 64;
/// compute → applier channel capacity. Same reasoning as
/// `EVENT_CHANNEL_CAPACITY` — four volumes of headroom for `StateUpdate`,
/// which is produced roughly one-for-one with the `AssemblyEvent`s that
/// drive it.
const COMPUTE_CHANNEL_CAPACITY: usize = 64;

const BACKOFF_INITIAL_SECS: u64 = 1;
const BACKOFF_CAP: Duration = Duration::from_secs(30);
/// How long a supervised unit must run cleanly before a subsequent panic is
/// treated as a fresh failure rather than a continuation of a crash loop.
const BACKOFF_RESET_AFTER_CLEAN: Duration = Duration::from_secs(5 * 60);

/// Capped exponential backoff for supervised task restarts (S2-d): 1s
/// doubling to a 30s cap. `consecutive_restarts` is clamped before
/// shifting, so this never risks overflow no matter how long a process has
/// been crash-looping.
fn backoff(consecutive_restarts: u32) -> Duration {
    let doublings = consecutive_restarts.min(5); // 1 << 5 = 32s already exceeds the cap
    Duration::from_secs(BACKOFF_INITIAL_SECS << doublings).min(BACKOFF_CAP)
}

/// Restart bookkeeping for one supervised unit: how long to wait before
/// respawning, and when to forget past failures. Pure aside from the
/// `Instant` it's given — same shape as `VolumeAssembler`/`state::apply`,
/// so the reset-after-clean-running behavior is an ordinary unit test
/// rather than something that needs five real minutes to pass.
struct RestartPolicy {
    consecutive_restarts: u32,
    last_restart_at: Option<Instant>,
}

impl RestartPolicy {
    fn new() -> Self {
        Self { consecutive_restarts: 0, last_restart_at: None }
    }

    /// Called when the supervised unit panics. Returns how long to wait
    /// before respawning.
    fn on_panic(&mut self, now: Instant) -> Duration {
        if let Some(last) = self.last_restart_at {
            if now.duration_since(last) >= BACKOFF_RESET_AFTER_CLEAN {
                self.consecutive_restarts = 0;
            }
        }
        let delay = backoff(self.consecutive_restarts);
        self.consecutive_restarts += 1;
        self.last_restart_at = Some(now);
        delay
    }
}

/// Runs `make_task()` under supervision. `make_task` is called fresh for
/// every (re)spawn — that's what makes "restart from scratch" possible,
/// since each attempt gets brand new task-local state (and, for the
/// production trio, brand new channels).
///
/// A panic (`JoinError::is_panic`) is reported as `TaskPanicked`, waits out
/// a capped exponential backoff (interruptible by `shutdown`), is reported
/// as `TaskRestarted`, and respawns. A *normal* return — the task decided
/// to stop, whether because `shutdown` fired or its channel closed — ends
/// supervision without restarting; only a panic does. `handle.await` is
/// always driven to completion (never raced against `shutdown` directly),
/// so a caller that awaits `supervise` itself can rely on every task it
/// spawned having actually finished — no detached background task left
/// running after this returns.
async fn supervise<F, Fut>(kind: TaskKind, state: &AppState, mut shutdown: watch::Receiver<bool>, mut make_task: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let mut policy = RestartPolicy::new();
    loop {
        let handle = tokio::spawn(make_task());
        match handle.await {
            Ok(()) => return,
            Err(join_err) if join_err.is_panic() => {
                state.report(Event::TaskPanicked { task: kind });
                if *shutdown.borrow() {
                    return; // shutting down anyway; do not restart
                }
                let delay = policy.on_panic(Instant::now());
                state.report(Event::TaskRestarted { task: kind, after: delay });
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = shutdown.wait_for(|s| *s) => return,
                }
            }
            // Aborted directly (`JoinHandle::abort`) rather than panicked —
            // nothing in this codebase does that to a supervised task, but
            // handle it as terminal rather than looping forever on it.
            Err(_aborted) => return,
        }
    }
}

/// One applier task: applies each compute-layer update from `origin` to
/// `state` as it arrives. Exits when `rx` closes or `shutdown` fires.
/// `volumes_closed`, when given, counts every `VolumeClosed` applied — the
/// backfill fetcher waits on it so it asks the ring for room only once the
/// ring reflects the volume it last sent (ADR-0033).
async fn apply_loop(
    mut rx: mpsc::Receiver<StateUpdate>,
    state: Arc<AppState>,
    mut shutdown: watch::Receiver<bool>,
    origin: Origin,
    volumes_closed: Option<watch::Sender<usize>>,
) {
    loop {
        tokio::select! {
            event = rx.recv() => {
                let Some(event) = event else { return };
                let closed = matches!(event, StateUpdate::VolumeClosed { .. });
                state.apply_event_from(event, Instant::now(), origin);
                if let (true, Some(counter)) = (closed, &volumes_closed) {
                    counter.send_modify(|n| *n += 1);
                }
            }
            _ = shutdown.wait_for(|s| *s) => return,
        }
    }
}

/// Assembly → compute → applier over chunks from `rx`, with fresh channels
/// between them, joined. The one chain every chunk source runs through: the
/// live poller, launch backfill (with its own instance — see this module's
/// doc comment), and the `spawn_from_chunks` test seam. Ends once `rx`
/// closes and everything behind it has drained, or on `shutdown`.
/// `volumes_closed` is passed to [`apply_loop`].
async fn run_chain(
    rx: mpsc::Receiver<ChunkEnvelope>,
    state: Arc<AppState>,
    shutdown: watch::Receiver<bool>,
    origin: Origin,
    volumes_closed: Option<watch::Sender<usize>>,
) {
    let (event_tx, event_rx) = mpsc::channel::<AssemblyEvent>(EVENT_CHANNEL_CAPACITY);
    let (update_tx, update_rx) = mpsc::channel::<StateUpdate>(COMPUTE_CHANNEL_CAPACITY);
    tokio::join!(
        assembly::run(rx, event_tx, AssemblyConfig::default(), shutdown.clone()),
        compute::compute_loop(event_rx, update_tx, Arc::clone(&state), shutdown.clone()),
        apply_loop(update_rx, state, shutdown, origin, volumes_closed),
    );
}

/// Builds a fresh `S3Poller` and fresh channels, then runs the
/// poller/assembly/compute/applier quartet to completion. Called once per
/// attempt by `Pipeline::spawn`'s `supervise` factory — see this module's
/// top-level doc comment for why the quartet restarts together rather than
/// independently. `status_tx` is a clone of the one sender created in
/// `Pipeline::spawn` — see `S3Poller::new`'s doc comment for why every
/// restart publishes into the same long-lived channel rather than a fresh
/// one. `anchor_slot` is `Some` when launch backfill is on.
async fn run_ingest_pipeline(
    site_id: String,
    state: Arc<AppState>,
    shutdown: watch::Receiver<bool>,
    status_tx: watch::Sender<IngestStatus>,
    poll_interval: Duration,
    anchor_slot: Option<Arc<AnchorSlot>>,
) {
    let client = http_ingest::S3Client::new(http_ingest::Bucket::Chunks);
    let mut poller = S3Poller::new(site_id, client, status_tx);
    if let Some(slot) = anchor_slot {
        poller = poller.with_anchor_slot(slot);
    }
    let (chunk_tx, chunk_rx) = mpsc::channel::<ChunkEnvelope>(CHUNK_CHANNEL_CAPACITY);

    tokio::join!(
        poller.run(chunk_tx, Arc::clone(&state), shutdown.clone(), poll_interval),
        run_chain(chunk_rx, state, shutdown, Origin::Live, None),
    );
}

/// The launch backfill unit (ADR-0033). Waits for the live poller's cold
/// start, picks the volumes behind its anchor, then fetches them under
/// supervision. Returns once backfill has stopped, or at once on `shutdown`
/// before the anchor arrives. `requested` is `history.frames − 1`.
async fn run_backfill(
    site_id: String,
    state: Arc<AppState>,
    mut shutdown: watch::Receiver<bool>,
    status_tx: watch::Sender<IngestStatus>,
    anchor_rx: oneshot::Receiver<ColdStart>,
    requested: usize,
) {
    // Live first: the poller publishes only after its first successful poll
    // has delivered the live volume's chunks.
    let cold_start = tokio::select! {
        cold_start = anchor_rx => cold_start.ok(),
        _ = shutdown.wait_for(|s| *s) => None,
    };
    let Some(cold_start) = cold_start else { return };

    let targets: Arc<[VolumeSeq]> = s3_poll::backfill_targets(&cold_start.folders, cold_start.anchor, requested).into();
    status_tx.send_modify(|s| s.backfill = BackfillStatus::Backfilling { done: 0, of: targets.len() });
    let progress = Arc::new(AtomicUsize::new(0));

    let attempt_state = Arc::clone(&state);
    let attempt_shutdown = shutdown.clone();
    supervise(TaskKind::Backfill, &state, shutdown, move || {
        run_backfill_attempt(
            site_id.clone(),
            Arc::clone(&attempt_state),
            attempt_shutdown.clone(),
            status_tx.clone(),
            Arc::clone(&targets),
            requested,
            Arc::clone(&progress),
        )
    })
    .await;
}

/// One supervised backfill attempt: a fresh `S3Client` (the second
/// connection) and a fresh chain, fetching from `progress` onwards.
/// Reports the stop once, after the chain has drained, so the event and the
/// `Complete` status follow the last volume into the ring.
async fn run_backfill_attempt(
    site_id: String,
    state: Arc<AppState>,
    shutdown: watch::Receiver<bool>,
    status_tx: watch::Sender<IngestStatus>,
    targets: Arc<[VolumeSeq]>,
    requested: usize,
    progress: Arc<AtomicUsize>,
) {
    let mut client = http_ingest::S3Client::new(http_ingest::Bucket::Chunks);
    let (chunk_tx, chunk_rx) = mpsc::channel::<ChunkEnvelope>(CHUNK_CHANNEL_CAPACITY);
    let (closed_tx, closed_rx) = watch::channel(0usize);
    let mut fetch_shutdown = shutdown.clone();
    let plan = FetchPlan { site_id: &site_id, targets: &targets, requested, progress: &progress };

    let (reason, ()) = tokio::join!(
        backfill::fetch(&mut client, plan, chunk_tx, closed_rx, &state, &status_tx, &mut fetch_shutdown),
        run_chain(chunk_rx, Arc::clone(&state), shutdown, Origin::Backfill, Some(closed_tx)),
    );

    state.report(Event::BackfillFinished { fetched: progress.load(Ordering::SeqCst), requested, reason });
    status_tx.send_modify(|s| s.backfill = BackfillStatus::Complete);
}

/// Owns the running pipeline's task(s) and the means to stop them: the
/// supervised ingest quartet, plus the launch backfill unit when it is on
/// (ADR-0033). Later stages (placefile polling) add further independent
/// tasks the same way.
pub struct Pipeline {
    tasks: Vec<JoinHandle<()>>,
    shutdown: watch::Sender<bool>,
}

impl Pipeline {
    /// Production entry point: polls `site`'s real-time chunk stream,
    /// assembles volumes, and applies them into `state`, indefinitely,
    /// restarting the whole ingest trio with backoff if any part of it
    /// panics. `status_tx` is the sender half of the `watch` channel whose
    /// receiver half the caller already gave to `state`'s `AppState::new`
    /// (FR-DA-5) — passed in rather than created here so that channel
    /// outlives every individual `S3Poller` a restart creates. `poll_interval`
    /// is `Config::poll_interval` (S2-W4, FR-DA-2) — already clamped to
    /// `[config::POLL_INTERVAL_MIN, config::POLL_INTERVAL_MAX]` by the
    /// caller; this function does not re-validate it.
    ///
    /// `backfill_volumes` is how many volumes behind the live one launch
    /// backfill fetches (ADR-0033) — `history.frames − 1`, or 0 when
    /// `history.backfill = false`. 0 spawns no backfill task and opens no
    /// second connection.
    pub fn spawn(
        rt: &Handle,
        site: &'static Site,
        state: Arc<AppState>,
        status_tx: watch::Sender<IngestStatus>,
        poll_interval: Duration,
        backfill_volumes: usize,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let site_id = site.id.to_string();
        let mut tasks = Vec::new();

        let anchor_slot = (backfill_volumes > 0).then(|| {
            let (slot, anchor_rx) = AnchorSlot::new();
            tasks.push(rt.spawn(run_backfill(
                site_id.clone(),
                Arc::clone(&state),
                shutdown_rx.clone(),
                status_tx.clone(),
                anchor_rx,
                backfill_volumes,
            )));
            slot
        });

        let supervised_state = Arc::clone(&state);
        let supervised_shutdown = shutdown_rx.clone();
        tasks.push(rt.spawn(async move {
            let factory_state = Arc::clone(&supervised_state);
            let factory_shutdown = supervised_shutdown.clone();
            supervise(TaskKind::IngestPipeline, &supervised_state, supervised_shutdown, move || {
                run_ingest_pipeline(
                    site_id.clone(),
                    Arc::clone(&factory_state),
                    factory_shutdown.clone(),
                    status_tx.clone(),
                    poll_interval,
                    anchor_slot.clone(),
                )
            })
            .await;
        }));

        Self { tasks, shutdown: shutdown_tx }
    }

    /// Test seam (§4.5): skips `S3Poller` entirely, feeding chunks from
    /// `rx` (which the caller retains the paired `Sender` for) directly
    /// into a live [`run_chain`], and — when `backfill_rx` is `Some` —
    /// chunks from it into a second, backfill-origin chain (ADR-0033),
    /// skipping the backfill fetcher the same way. Not wrapped in
    /// `supervise`, because each `Receiver` is a single-use resource (it
    /// cannot be recreated after a panic drops it, the same reason the
    /// production quartet must restart as a unit rather than
    /// independently). Panic-survival is exercised generically instead,
    /// against `supervise` directly with a synthetic repeatable task
    /// factory — see `tests::` below.
    #[cfg(test)]
    fn spawn_from_chunks(
        rt: &Handle,
        rx: mpsc::Receiver<ChunkEnvelope>,
        backfill_rx: Option<mpsc::Receiver<ChunkEnvelope>>,
        state: Arc<AppState>,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let handle = rt.spawn(async move {
            let backfill_state = Arc::clone(&state);
            let backfill_shutdown = shutdown_rx.clone();
            let backfill = async move {
                if let Some(backfill_rx) = backfill_rx {
                    run_chain(backfill_rx, backfill_state, backfill_shutdown, Origin::Backfill, None).await;
                }
            };
            tokio::join!(run_chain(rx, state, shutdown_rx, Origin::Live, None), backfill);
        });

        Self { tasks: vec![handle], shutdown: shutdown_tx }
    }

    /// Signals shutdown, then joins every task. Safe to call even if the
    /// pipeline already exited on its own — `watch::Sender::send` on a
    /// channel with no live receivers is a no-op we deliberately ignore,
    /// and joining an already-finished `JoinHandle` returns immediately.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        for task in self.tasks {
            let _ = task.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use tokio::sync::watch;

    use crate::ingest::s3_poll::IngestStatus;

    use super::*;

    fn test_app_state() -> Arc<AppState> {
        let (_tx, rx) = watch::channel(IngestStatus::default());
        Arc::new(AppState::new(
            crate::sites::by_id("KDOX").expect("KDOX in bundled table"),
            rx,
            crate::state::RetentionPolicy::default(),
        ))
    }

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        assert_eq!(backoff(0), Duration::from_secs(1));
        assert_eq!(backoff(1), Duration::from_secs(2));
        assert_eq!(backoff(2), Duration::from_secs(4));
        assert_eq!(backoff(3), Duration::from_secs(8));
        assert_eq!(backoff(4), Duration::from_secs(16));
        assert_eq!(backoff(5), Duration::from_secs(30));
        assert_eq!(backoff(6), Duration::from_secs(30));
        assert_eq!(backoff(1_000_000), Duration::from_secs(30), "must never overflow, however large the streak");
    }

    #[test]
    fn restart_policy_resets_after_a_clean_running_period() {
        let mut policy = RestartPolicy::new();
        let t0 = Instant::now();

        assert_eq!(policy.on_panic(t0), Duration::from_secs(1));
        assert_eq!(policy.on_panic(t0 + Duration::from_secs(1)), Duration::from_secs(2));
        assert_eq!(policy.on_panic(t0 + Duration::from_secs(2)), Duration::from_secs(4));

        let long_after = t0 + Duration::from_secs(2) + BACKOFF_RESET_AFTER_CLEAN;
        assert_eq!(policy.on_panic(long_after), Duration::from_secs(1), "streak must reset after a clean period");
    }

    /// S2-d: an injected panic is caught, surfaced as typed
    /// `TaskPanicked`/`TaskRestarted` events, and the supervised unit
    /// restarts with backoff rather than taking the process down. Exercises
    /// `supervise` directly with a synthetic repeatable task factory —
    /// `spawn_from_chunks`'s real channel wiring can't survive a panic
    /// (see its doc comment), so this is the seam that actually tests
    /// restart-through-panic. Real backoff delays (1s, 2s) elapse for real
    /// — deliberately, since this is exactly the timing behavior under
    /// test.
    #[tokio::test]
    async fn supervise_restarts_after_panics_and_reports_typed_events() {
        let state = test_app_state();
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_for_factory = Arc::clone(&attempts);

        supervise(TaskKind::IngestPipeline, &state, shutdown_rx, move || {
            let attempts = Arc::clone(&attempts_for_factory);
            async move {
                let n = attempts.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    panic!("synthetic panic on attempt {n}");
                }
                // Third attempt returns normally; supervise stops here.
            }
        })
        .await;

        assert_eq!(attempts.load(Ordering::SeqCst), 3, "must retry indefinitely, not give up after one failure");
        assert_eq!(state.event_log_len(), 4, "2 panics x (TaskPanicked + TaskRestarted) = 4 events");
    }

    #[tokio::test]
    async fn supervise_does_not_restart_after_a_clean_exit() {
        let state = test_app_state();
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_for_factory = Arc::clone(&attempts);

        supervise(TaskKind::IngestPipeline, &state, shutdown_rx, move || {
            let attempts = Arc::clone(&attempts_for_factory);
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
            }
        })
        .await;

        assert_eq!(attempts.load(Ordering::SeqCst), 1, "a normal return must not trigger a restart");
        assert_eq!(state.event_log_len(), 0);
    }

    /// §4.2: spawn with an injected chunk source, assert the handle joins
    /// within a short timeout, assert shutdown is safe even after the
    /// pipeline already exited on its own.
    #[tokio::test]
    async fn spawn_from_chunks_joins_promptly_on_shutdown() {
        let (chunk_tx, chunk_rx) = mpsc::channel(CHUNK_CHANNEL_CAPACITY);
        let pipeline = Pipeline::spawn_from_chunks(&Handle::current(), chunk_rx, None, test_app_state());
        drop(chunk_tx); // nothing to feed; the pipeline should still shut down cleanly

        let result = tokio::time::timeout(Duration::from_secs(5), pipeline.shutdown()).await;
        assert!(result.is_ok(), "shutdown must complete promptly, not hang");
    }

    #[tokio::test]
    async fn spawn_from_chunks_shutdown_is_safe_after_the_pipeline_already_exited() {
        let (chunk_tx, chunk_rx) = mpsc::channel(CHUNK_CHANNEL_CAPACITY);
        let pipeline = Pipeline::spawn_from_chunks(&Handle::current(), chunk_rx, None, test_app_state());
        // Closing the sender makes assembly::run's rx.recv() return None,
        // ending the pipeline on its own before shutdown() is ever called.
        drop(chunk_tx);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let result = tokio::time::timeout(Duration::from_secs(5), pipeline.shutdown()).await;
        assert!(result.is_ok(), "shutdown on an already-exited pipeline must not hang or panic");
    }

    // --- launch backfill through the test seam (ADR-0033) ---

    const START_OF_VOLUME: &[u8] = include_bytes!("../../nexrad-decoder/tests/fixtures/kdox_vcp35_start_of_volume.bin");
    const END_OF_VOLUME: &[u8] = include_bytes!("../../nexrad-decoder/tests/fixtures/kdox_vcp35_end_of_volume.bin");
    const LIVE_SCAN_MS: u32 = 64_800_000;
    const VOLUME_MS: u32 = 300_000;

    /// A fixture Message 31 restamped with `scan_time_ms` — the collection
    /// time sits after the 12-byte CTM header, the 16-byte message header
    /// and the 4-byte radar id (`docs/architecture/nexrad-binary-format.md`).
    /// The first radial's time is what makes a `VolumeId`, so restamping
    /// turns one fixture into as many distinct volumes as a test needs.
    fn stamped(message: &[u8], scan_time_ms: u32) -> Vec<u8> {
        let mut m = message.to_vec();
        m[32..36].copy_from_slice(&scan_time_ms.to_be_bytes());
        m
    }

    /// One whole (tiny) volume as a single real `-E` chunk: a negative
    /// length prefix, then a BZh9 block of the start-of-volume radial
    /// (elevation 1) and the end-of-volume radial (elevation 16). Decodes
    /// through the production `chunk`/decoder/assembly path into two
    /// gridded sweeps and a closed volume.
    fn whole_volume_chunk(scan_time_ms: u32) -> ChunkEnvelope {
        use std::io::Write as _;
        let mut stream = stamped(START_OF_VOLUME, scan_time_ms);
        stream.extend(stamped(END_OF_VOLUME, scan_time_ms));
        let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::best());
        encoder.write_all(&stream).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut raw = (-(compressed.len() as i32)).to_be_bytes().to_vec();
        raw.extend(compressed);
        ChunkEnvelope { kind: crate::chunk::ChunkKind::End, raw_bytes: bytes::Bytes::from(raw) }
    }

    async fn wait_for_frames(state: &AppState, n: usize) -> crate::state::StateSnapshot {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshot = state.snapshot();
                if snapshot.frames.len() >= n {
                    return snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the ring never reached {n} frame(s)"))
    }

    fn scan_times(snapshot: &crate::state::StateSnapshot) -> Vec<u32> {
        snapshot.frames.iter().map(|f| f.volume.scan_time_ms).collect()
    }

    #[tokio::test]
    async fn backfilled_volumes_land_behind_the_live_head_in_volume_order() {
        let state = test_app_state();
        let (live_tx, live_rx) = mpsc::channel(CHUNK_CHANNEL_CAPACITY);
        let (backfill_tx, backfill_rx) = mpsc::channel(CHUNK_CHANNEL_CAPACITY);
        let pipeline = Pipeline::spawn_from_chunks(&Handle::current(), live_rx, Some(backfill_rx), Arc::clone(&state));

        live_tx.send(whole_volume_chunk(LIVE_SCAN_MS)).await.unwrap();
        wait_for_frames(&state, 1).await;
        // Newest first, as `backfill::fetch` sends them.
        backfill_tx.send(whole_volume_chunk(LIVE_SCAN_MS - VOLUME_MS)).await.unwrap();
        backfill_tx.send(whole_volume_chunk(LIVE_SCAN_MS - 2 * VOLUME_MS)).await.unwrap();
        let snapshot = wait_for_frames(&state, 3).await;
        pipeline.shutdown().await;

        assert_eq!(
            scan_times(&snapshot),
            vec![LIVE_SCAN_MS - 2 * VOLUME_MS, LIVE_SCAN_MS - VOLUME_MS, LIVE_SCAN_MS],
            "three frames, oldest first"
        );
        assert!(
            snapshot.sweeps.iter().all(|s| s.volume.scan_time_ms == LIVE_SCAN_MS),
            "the live view still comes from the live head"
        );
    }

    #[tokio::test]
    async fn shutting_down_mid_backfill_leaves_a_consistent_ring() {
        let state = test_app_state();
        let (live_tx, live_rx) = mpsc::channel(CHUNK_CHANNEL_CAPACITY);
        let (backfill_tx, backfill_rx) = mpsc::channel(CHUNK_CHANNEL_CAPACITY);
        let pipeline = Pipeline::spawn_from_chunks(&Handle::current(), live_rx, Some(backfill_rx), Arc::clone(&state));

        live_tx.send(whole_volume_chunk(LIVE_SCAN_MS)).await.unwrap();
        wait_for_frames(&state, 1).await;
        for k in 1..=4 {
            backfill_tx.send(whole_volume_chunk(LIVE_SCAN_MS - k * VOLUME_MS)).await.unwrap();
        }
        let result = tokio::time::timeout(Duration::from_secs(5), pipeline.shutdown()).await;
        assert!(result.is_ok(), "shutdown must not wait for backfill to finish");

        let times = scan_times(&state.snapshot());
        assert_eq!(times.last(), Some(&LIVE_SCAN_MS), "the live head is still the head");
        assert!(times.windows(2).all(|w| w[0] < w[1]), "strictly oldest → newest, whatever landed: {times:?}");
        assert!(times.len() <= 5);
    }
}
