//! Launch backfill (ADR-0033, Stage 6a Part D1): fills the history ring
//! with the volumes behind the live one, so the loop is populated at launch
//! instead of after 40–60 minutes of watching.
//!
//! This module is the fetch side: the anchor hand-off from the live poller,
//! the per-volume fetch loop and its stop conditions, and the status it
//! publishes. Assembly, compute and the applier are the same code live data
//! runs through (`pipeline::run_chain`); only the ring insert knows a volume
//! came from here (`state::Origin::Backfill`). `pipeline` owns the task and
//! its supervision.
//!
//! **A second connection.** Backfill runs on its own `S3Client`, to the same
//! bucket as the live poller (`unidata-nexrad-level2-chunks`). `http-ingest`
//! sends one request at a time on one connection (ADR-0014), so sharing the
//! poller's client would hold live polling behind a whole volume of backfill
//! fetches. The connection exists only while backfill runs, and is disclosed
//! in ADR-0033 and `QUICK-START.md`; `history.backfill = false` removes it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch};

use crate::state::AppState;

use super::s3_poll::{self, IngestErrorKind, IngestStatus, PollError};
use super::volume_seq::VolumeSeq;
use super::ChunkEnvelope;

/// Attempts per volume before backfill gives up on the network. Each attempt
/// is a whole volume (list + every chunk), fetched into memory before any of
/// it is fed to assembly, so a retry never feeds a volume twice. `http-ingest`
/// already retries once at the connection layer (ADR-0014).
const FETCH_ATTEMPTS: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(2);
/// How long the fetcher waits for the volume it last sent to close in the
/// ring before asking the ring for room anyway. A volume grids in a few
/// seconds; one that never closes (its `-E` chunk expired or was never
/// written) stays open until the next volume's chunks supersede it, so this
/// bounds the wait rather than deadlocking on it.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(30);

/// What the live poller's cold start hands to backfill: the volume-folder
/// listing it already made (so the bucket is listed once, not twice) and
/// the volume it anchored on.
#[derive(Debug, Clone)]
pub struct ColdStart {
    pub folders: Vec<VolumeSeq>,
    pub anchor: VolumeSeq,
}

/// A one-shot hand-off of the [`ColdStart`], shared by every `S3Poller` a
/// supervised restart builds. The first poller to finish a cold start
/// publishes; any later one (after a restart) finds the slot empty and does
/// nothing, so a panic in the ingest pipeline never starts a second backfill.
pub struct AnchorSlot(Mutex<Option<oneshot::Sender<ColdStart>>>);

impl AnchorSlot {
    pub fn new() -> (Arc<Self>, oneshot::Receiver<ColdStart>) {
        let (tx, rx) = oneshot::channel();
        (Arc::new(Self(Mutex::new(Some(tx)))), rx)
    }

    pub fn publish(&self, cold_start: ColdStart) {
        let sender = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
        if let Some(tx) = sender {
            // The receiver is gone only if backfill already stopped
            // (shutdown); nothing to hand off to.
            let _ = tx.send(cold_start);
        }
    }
}

/// Backfill's progress, as `IngestStatus::backfill` (ADR-0033). The status
/// bar shows "Backfill k/N" while `Backfilling`, so the operator can always
/// see why the loop is growing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackfillStatus {
    /// `history.backfill = false`, retention disabled, or not started yet.
    #[default]
    Off,
    /// `done` of `of` volumes fetched and handed to assembly.
    Backfilling { done: usize, of: usize },
    /// Stopped, for whichever reason `Event::BackfillFinished` gave.
    Complete,
}

/// Why backfill stopped — reported once, as `Event::BackfillFinished`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackfillStop {
    /// Every requested volume was fetched.
    Complete,
    /// The listing held fewer volumes behind the anchor than requested
    /// (the bucket's ~48 h retention, or a fresh site); all of them were
    /// fetched.
    ListingExhausted,
    /// The history ring had no room behind its head before the next volume
    /// (`history.frames` or `history.budget_mb` reached).
    HistoryFull,
    /// A volume failed [`FETCH_ATTEMPTS`] times.
    Failed(IngestErrorKind),
    Shutdown,
}

impl std::fmt::Display for BackfillStop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Complete => write!(f, "complete"),
            Self::ListingExhausted => write!(f, "no older volumes in the bucket"),
            Self::HistoryFull => write!(f, "history is full (history.frames / history.budget_mb)"),
            Self::Failed(kind) => write!(f, "fetch failed ({kind:?})"),
            Self::Shutdown => write!(f, "shut down"),
        }
    }
}

/// What one backfill run needs, borrowed for the length of [`fetch`].
pub struct FetchPlan<'a> {
    pub site_id: &'a str,
    /// Newest first, from `s3_poll::backfill_targets`.
    pub targets: &'a [VolumeSeq],
    /// How many volumes were asked for (`history.frames − 1`), to tell
    /// `Complete` from `ListingExhausted`.
    pub requested: usize,
    /// Targets already fetched. Shared across supervised restarts, so a
    /// restart resumes at the volume that was in flight instead of starting
    /// over.
    pub progress: &'a AtomicUsize,
}

/// Fetches `plan.targets` one volume at a time, newest first, and sends each
/// volume's chunks to `tx` in key order. Newest first means a backfill that
/// stops early has still filled the most recent part of the loop.
///
/// The ring is asked for room before each volume is *sent*, and only once
/// it reflects the volume sent before it: `volumes_closed` counts the
/// `VolumeClosed` updates the backfill applier has applied, and the fetcher
/// waits for it to catch up (up to [`SETTLE_TIMEOUT`]). Without that wait
/// the answer trails by everything still buffered in the chain — measured
/// at about two volumes — and backfill grids volumes the next `trim`
/// evicts. The next volume's download overlaps the previous one's gridding,
/// so the most a full ring wastes is one download that is never sent.
///
/// Returns why it stopped. Drops `tx` on return, which drains and ends the
/// backfill chain behind it.
pub async fn fetch(
    client: &mut http_ingest::S3Client,
    plan: FetchPlan<'_>,
    tx: mpsc::Sender<ChunkEnvelope>,
    mut volumes_closed: watch::Receiver<usize>,
    app_state: &AppState,
    status_tx: &watch::Sender<IngestStatus>,
    shutdown: &mut watch::Receiver<bool>,
) -> BackfillStop {
    let of = plan.targets.len();
    let mut sent = 0usize;
    loop {
        let done = plan.progress.load(Ordering::SeqCst);
        let Some(&target) = plan.targets.get(done) else { break };
        if *shutdown.borrow() {
            return BackfillStop::Shutdown;
        }
        if !app_state.backfill_has_room() {
            return BackfillStop::HistoryFull;
        }

        let prefix = format!("{}/{target}/", plan.site_id);
        let envelopes = match fetch_volume_with_retry(client, app_state, &prefix, shutdown).await {
            Some(Ok(envelopes)) => envelopes,
            Some(Err(e)) => return BackfillStop::Failed(IngestErrorKind::from(&e)),
            None => return BackfillStop::Shutdown,
        };

        let settled = tokio::select! {
            _ = tokio::time::timeout(SETTLE_TIMEOUT, volumes_closed.wait_for(|n| *n >= sent)) => true,
            _ = shutdown.wait_for(|s| *s) => false,
        };
        if !settled {
            return BackfillStop::Shutdown;
        }
        if !app_state.backfill_has_room() {
            return BackfillStop::HistoryFull;
        }

        if !envelopes.is_empty() {
            sent += 1;
        }
        for envelope in envelopes {
            if tx.send(envelope).await.is_err() {
                return BackfillStop::Shutdown;
            }
        }

        plan.progress.store(done + 1, Ordering::SeqCst);
        status_tx.send_modify(|s| s.backfill = BackfillStatus::Backfilling { done: done + 1, of });
    }
    if of >= plan.requested {
        BackfillStop::Complete
    } else {
        BackfillStop::ListingExhausted
    }
}

/// One volume directory, listed and fetched whole, retried up to
/// [`FETCH_ATTEMPTS`] times. `None` if `shutdown` fired first.
async fn fetch_volume_with_retry(
    client: &mut http_ingest::S3Client,
    app_state: &AppState,
    prefix: &str,
    shutdown: &mut watch::Receiver<bool>,
) -> Option<Result<Vec<ChunkEnvelope>, PollError>> {
    let mut attempt = 1;
    loop {
        let result = tokio::select! {
            result = fetch_volume(client, app_state, prefix) => result,
            _ = shutdown.wait_for(|s| *s) => return None,
        };
        match result {
            Ok(envelopes) => return Some(Ok(envelopes)),
            Err(e) if attempt >= FETCH_ATTEMPTS => return Some(Err(e)),
            Err(_) => attempt += 1,
        }
        tokio::select! {
            _ = tokio::time::sleep(RETRY_DELAY) => {}
            _ = shutdown.wait_for(|s| *s) => return None,
        }
    }
}

/// Every chunk in one volume directory, in key (= chronological) order —
/// the live poller's own listing and fetch functions, on backfill's client.
async fn fetch_volume(
    client: &mut http_ingest::S3Client,
    app_state: &AppState,
    prefix: &str,
) -> Result<Vec<ChunkEnvelope>, PollError> {
    let (keys, _pages) = s3_poll::list_all_keys_after(client, prefix, None).await?;
    let (envelopes, _saw_end) = s3_poll::fetch_keys(client, app_state, &keys).await?;
    Ok(envelopes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(n: u64) -> VolumeSeq {
        VolumeSeq::new(n).unwrap()
    }

    #[test]
    fn the_anchor_slot_publishes_once() {
        let (slot, mut rx) = AnchorSlot::new();
        slot.publish(ColdStart { folders: vec![seq(5)], anchor: seq(5) });
        slot.publish(ColdStart { folders: vec![seq(6)], anchor: seq(6) });
        let first = rx.try_recv().expect("the first cold start is handed off");
        assert_eq!(first.anchor, seq(5), "a restarted poller's cold start must not replace it");
    }

    #[test]
    fn publishing_after_backfill_stopped_is_harmless() {
        let (slot, rx) = AnchorSlot::new();
        drop(rx);
        slot.publish(ColdStart { folders: Vec::new(), anchor: VolumeSeq::MIN });
    }
}
