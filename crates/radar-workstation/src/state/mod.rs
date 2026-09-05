//! Shared application state (S2-W1, ADR-0018, Q4). `AppState` is the single
//! coordination point between the data pipeline and, from Stage 4 on, the
//! render loop.
//!
//! Q4's answer, in one paragraph: this is *not* `Arc<RwLock<AppState>>` as
//! `overview.md`/`data-flow.md` originally said — see the ADR-0018 erratum
//! there. It is `Arc<AppState>`, where `AppState` holds an interior
//! `RwLock<RadarState>` scoped to radar data only. View state (pan, zoom,
//! active product/sweep, window geometry) is owned outright by the render
//! loop and never enters this type at all. Ingest health is read through the
//! `watch::Receiver<IngestStatus>` `S3Poller::status()` already publishes —
//! no copy, no second source of truth.
//!
//! **Stage 3 (S3-g):** `RadarState` holds grids, not raw radials. Once a
//! sweep is gridded, its `Vec<Radial>` is released — the last `Arc<Sweep>`
//! is dropped when `compute::compute_loop`'s `spawn_blocking` closure
//! returns. `last_complete` is metadata only ([`VolumeSummary`]), not the
//! `VolumeScan` itself: nothing downstream needs the sweeps of a volume
//! already gridded, and the gridded cells recover the exact physical value
//! via each grid's own effective scale/offset.
//!
//! [`AppState::snapshot`] is the only read API. It takes the read lock,
//! clones `Arc`s and `Copy` fields, and drops the lock before returning —
//! there is deliberately no `fn read(&self) -> RwLockReadGuard<'_, _>`, so
//! "never hold a lock across a frame" is a property of the type system
//! rather than a rule a later contributor has to remember.

mod apply;
pub mod history;

use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Instant;

use nexrad_decoder::{VolumeScan, VolumeStatus};
use tokio::sync::watch;

use crate::assembly::VolumeId;
use crate::compute::SweepGrid;
use crate::event::{Event, EventLog};
use crate::ingest::s3_poll::IngestStatus;
use crate::sites::Site;

pub use apply::apply;
pub use history::{Frame, RetentionPolicy};

/// One elevation's most recently closed sweep, held for display as grids —
/// not the sweep itself (S3-g). Cheap to clone: `Arc` for each grid,
/// `Copy`/small-`Vec` for everything else — this is what keeps
/// [`AppState::snapshot`]'s per-frame cost a handful of refcount bumps.
#[derive(Debug, Clone)]
pub struct DisplaySweep {
    pub elevation_number: u8,
    pub elevation_deg: f32,
    pub volume: VolumeId,
    pub vcp_number: u16,
    /// When this sweep was applied. Feeds FR-DA-5's data-age display.
    pub received: Instant,
    /// One entry per base product present on this sweep, sorted by
    /// `DisplayProduct` (see `compute::DisplayProduct::BASE`'s doc comment).
    pub grids: Vec<Arc<SweepGrid>>,
}

/// Metadata for the last successfully completed volume (FR-DA-5). Replaces
/// the Stage 2 `Option<Arc<VolumeScan>>` (S3-g): the sweeps a `VolumeScan`
/// held are already gridded and released by the time a volume closes, so
/// nothing downstream needs anything but this volume's identity and the
/// site parameters it carried. Small and `Copy` — no `Arc` needed.
#[derive(Debug, Clone, Copy)]
pub struct VolumeSummary {
    pub volume: VolumeId,
    pub vcp_number: u16,
    pub status: VolumeStatus,
    pub latitude: f32,
    pub longitude: f32,
    pub site_amsl_m: i16,
}

impl VolumeSummary {
    pub fn from_scan(scan: &VolumeScan) -> Self {
        Self {
            volume: VolumeId { julian_date: scan.julian_date, scan_time_ms: scan.scan_time_ms },
            vcp_number: scan.vcp_number,
            status: scan.status,
            latitude: scan.latitude,
            longitude: scan.longitude,
            site_amsl_m: scan.site_amsl_m,
        }
    }
}

/// Radar data only — see this module's top-level doc comment for why view
/// state and ingest status live elsewhere. Not constructed directly outside
/// this module; go through [`AppState::new`].
pub struct RadarState {
    pub site: &'static Site,
    /// The retained volumes (ADR-0030). Everything Stage 3's `RadarState`
    /// held directly — the merged sweep map, the derived-product set, which
    /// volume they came from, `last_complete` — is now derivable from this
    /// ring: `current_vcp` is the newest frame's `vcp_number`, the live
    /// sweep/derived views are [`history::live_sweeps`]/[`history::live_derived`]
    /// folds over it, and `last_complete` is the newest frame (searching
    /// backwards) whose `complete` is `Some`. Private: mutated only through
    /// [`apply`] and [`Self::reset`].
    history: history::FrameRing,
    /// Increments on every applied change. The render loop (Stage 4) will
    /// compare this against the value it last uploaded to the GPU and skip
    /// texture re-upload when unchanged (FR-DR-5).
    pub revision: u64,
}

impl RadarState {
    fn new(site: &'static Site, policy: history::RetentionPolicy) -> Self {
        Self { site, history: history::FrameRing::new(policy), revision: 0 }
    }

    /// Empties everything and switches to `site`. FR-DA-4 (site change)
    /// consumes this from Stage 7 on; nothing before Stage 7 calls it
    /// outside tests. Always bumps `revision` — a reset is itself a change
    /// the render loop must notice, and `revision` stays a single
    /// monotonically increasing counter across a site switch rather than
    /// resetting to a value the render loop may have already seen.
    ///
    /// The history ring is cleared, not carried over (ADR-0030 §3.4): a
    /// frame's grids are polar grids around a specific site, and there is
    /// nothing in the old ring that could be correctly displayed under the
    /// new one.
    pub fn reset(&mut self, site: &'static Site) {
        self.site = site;
        self.history.clear();
        self.revision += 1;
    }
}

/// Owned snapshot of [`RadarState`], returned by [`AppState::snapshot`].
/// Nothing in here borrows from the lock. Cost: N `Arc<Frame>` clones for
/// `frames` (one refcount bump per retained frame, whatever each frame
/// holds — ADR-0030 §3.2), plus the live fold behind `sweeps`/`derived`
/// (bounded by retained frames × elevations, at most a few dozen map
/// probes at the proposed default retention) and the same handful of
/// `DisplaySweep` clones Stage 3 always did.
pub struct StateSnapshot {
    pub site: &'static Site,
    /// The live merged view — newest sweep per elevation number, folded
    /// over `frames` ([`history::live_sweeps`]). Byte-for-byte what Stage 5
    /// displayed; every consumer of this field is unchanged by ADR-0030.
    pub sweeps: Vec<DisplaySweep>,
    /// The live derived-product view ([`history::live_derived`]).
    pub derived: Vec<Arc<SweepGrid>>,
    pub last_complete: Option<VolumeSummary>,
    /// Every retained frame, oldest → newest (ADR-0030). `Part C` (the
    /// timeline) is the first consumer that reads this directly; nothing
    /// before it does.
    pub frames: Vec<Arc<history::Frame>>,
    pub revision: u64,
    pub ingest: IngestStatus,
}

pub struct AppState {
    radar: RwLock<RadarState>,
    ingest: watch::Receiver<IngestStatus>,
    events: Mutex<EventLog>,
}

impl AppState {
    pub fn new(site: &'static Site, ingest: watch::Receiver<IngestStatus>, policy: history::RetentionPolicy) -> Self {
        Self { radar: RwLock::new(RadarState::new(site, policy)), ingest, events: Mutex::new(EventLog::new()) }
    }

    /// A poisoned lock (a panic while holding it) must never become a
    /// second, permanent failure mode on top of whatever panicked —
    /// Stage 2's supervision (S2-W2) restarts the task that panicked, but
    /// only if the state it touched is still usable afterwards. Recovering
    /// the guard rather than propagating the poison is what keeps a single
    /// task panic from taking every future `snapshot()` down with it,
    /// including the render loop's from Stage 4 on.
    fn read_radar(&self) -> RwLockReadGuard<'_, RadarState> {
        self.radar.read().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_radar(&self) -> RwLockWriteGuard<'_, RadarState> {
        self.radar.write().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn snapshot(&self) -> StateSnapshot {
        let radar = self.read_radar();
        let deque = radar.history.as_deque();
        StateSnapshot {
            site: radar.site,
            sweeps: history::live_sweeps(deque),
            derived: history::live_derived(deque),
            last_complete: deque.iter().rev().find_map(|f| f.complete),
            frames: radar.history.snapshot_frames(),
            revision: radar.revision,
            ingest: self.ingest.borrow().clone(),
        }
    }

    /// Apply one compute-layer update. Returns whether anything changed
    /// (i.e. whether `revision` was bumped) — the pipeline (S2-W2) uses
    /// this only for tests; production code does not need to branch on it.
    ///
    /// `StateUpdate::Info`'s `LateRadialsDiscarded`/`MissingStartChunk` are
    /// observability, not radar data (ADR-0012 rule table) — reported here,
    /// at the `AppState` level where the event log lives, rather than by
    /// the pure `state::apply`, which only ever touches `RadarState`. The
    /// same split now covers `apply`'s own events (a late volume discarded,
    /// the byte budget starting to bind, ADR-0030) — `apply` returns them
    /// rather than reporting them itself, for the same reason.
    pub fn apply_event(&self, event: crate::compute::StateUpdate, now: Instant) -> bool {
        use crate::compute::StateUpdate;
        if let StateUpdate::Info(assembly_event) = &event {
            match assembly_event {
                crate::assembly::AssemblyEvent::LateRadialsDiscarded { elevation_number, count } => {
                    self.report(Event::LateRadialsDiscarded { elevation_number: *elevation_number, count: *count });
                }
                crate::assembly::AssemblyEvent::MissingStartChunk => self.report(Event::MissingStartChunk),
                crate::assembly::AssemblyEvent::SweepClosed { .. }
                | crate::assembly::AssemblyEvent::VolumeClosed { .. } => {}
            }
        }
        let (changed, events) = apply(&mut self.write_radar(), event, now);
        for event in events {
            self.report(event);
        }
        changed
    }

    pub fn reset(&self, site: &'static Site) {
        self.write_radar().reset(site);
    }

    /// The one place a task with an `Arc<AppState>` should report an
    /// [`Event`] — forwards to the stderr sink and pushes into the bounded
    /// in-memory log in one call, so the two sinks can never drift apart.
    pub fn report(&self, event: Event) {
        crate::event::log_to_stderr(&event);
        self.events.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(event);
    }

    /// The most recent `max` reported events as formatted strings, newest
    /// last (S4-W6 §9.1). This is the reader [`EventLog`] was written for —
    /// NFR-ST-3's status bar. Briefly takes the event mutex; `max` is
    /// expected to be a handful.
    pub fn recent_events(&self, max: usize) -> Vec<(Instant, String)> {
        self.events.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).recent(max)
    }

    #[cfg(test)]
    pub(crate) fn event_log_len(&self) -> usize {
        self.events.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::StateUpdate;

    fn test_site() -> &'static Site {
        crate::sites::by_id("KDOX").expect("KDOX is in the bundled table")
    }

    fn app_state() -> AppState {
        let (_tx, rx) = watch::channel(IngestStatus::default());
        AppState::new(test_site(), rx, history::RetentionPolicy::default())
    }

    #[test]
    fn snapshot_of_fresh_state_is_empty() {
        let state = app_state();
        let snap = state.snapshot();
        assert_eq!(snap.site.id, "KDOX");
        assert!(snap.sweeps.is_empty());
        assert!(snap.derived.is_empty());
        assert!(snap.last_complete.is_none());
        assert_eq!(snap.revision, 0);
    }

    #[test]
    fn reset_bumps_revision_and_clears_state() {
        let state = app_state();
        let ktlh = crate::sites::by_id("KTLH").expect("KTLH is in the bundled table");
        state.reset(ktlh);
        let snap = state.snapshot();
        assert_eq!(snap.site.id, "KTLH");
        assert!(snap.sweeps.is_empty());
        assert_eq!(snap.revision, 1);
    }

    #[test]
    fn apply_event_reports_informational_assembly_events() {
        let state = app_state();
        assert_eq!(state.event_log_len(), 0);
        let changed = state.apply_event(
            StateUpdate::Info(crate::assembly::AssemblyEvent::LateRadialsDiscarded { elevation_number: 3, count: 5 }),
            Instant::now(),
        );
        assert!(!changed);
        assert_eq!(state.event_log_len(), 1, "LateRadialsDiscarded must reach the event log");

        state.apply_event(
            StateUpdate::Info(crate::assembly::AssemblyEvent::MissingStartChunk),
            Instant::now(),
        );
        assert_eq!(state.event_log_len(), 2, "MissingStartChunk must reach the event log");
    }

    #[test]
    fn report_forwards_to_the_event_log() {
        let state = app_state();
        assert_eq!(state.event_log_len(), 0);
        state.report(Event::UnrecognizedKeySuffix { key: "KDOX/166/x".to_string() });
        assert_eq!(state.event_log_len(), 1);
    }

    #[test]
    fn recent_events_returns_the_newest_formatted_last_and_bounded_by_max() {
        let state = app_state();
        for n in 0..5 {
            state.report(Event::ConfigLineUnparseable { line: n });
        }
        let recent = state.recent_events(3);
        assert_eq!(recent.len(), 3, "capped at max");
        assert!(recent.last().unwrap().1.contains("line 4"), "newest event is last: {:?}", recent.last());
        assert!(recent.first().unwrap().1.contains("line 2"), "oldest of the window is first");
    }
}
