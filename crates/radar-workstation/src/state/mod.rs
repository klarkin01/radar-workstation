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
//! [`AppState::snapshot`] is the only read API. It takes the read lock,
//! clones `Arc`s and `Copy` fields, and drops the lock before returning —
//! there is deliberately no `fn read(&self) -> RwLockReadGuard<'_, _>`, so
//! "never hold a lock across a frame" is a property of the type system
//! rather than a rule a later contributor has to remember.

mod apply;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Instant;

use nexrad_decoder::{Sweep, VolumeScan};
use tokio::sync::watch;

use crate::assembly::{AssemblyEvent, VolumeId};
use crate::event::{Event, EventLog};
use crate::ingest::s3_poll::IngestStatus;
use crate::sites::Site;

pub use apply::apply;

/// One elevation's most recently closed sweep, held for display. Cheap to
/// clone: `Arc` for the sweep data itself, `Copy` fields for everything
/// else — this is exactly what makes [`AppState::snapshot`]'s per-frame cost
/// a handful of refcount bumps rather than a copy of the radial data.
#[derive(Clone)]
pub struct DisplaySweep {
    pub sweep: Arc<Sweep>,
    pub volume: VolumeId,
    pub vcp_number: u16,
    /// When this sweep was applied. Feeds FR-DA-5's data-age display.
    pub received: Instant,
}

/// Radar data only — see this module's top-level doc comment for why view
/// state and ingest status live elsewhere. Not constructed directly outside
/// this module; go through [`AppState::new`].
pub struct RadarState {
    pub site: &'static Site,
    /// Newest closed sweep per elevation number, carried across volume
    /// boundaries so a closing volume never blanks the display (ADR-0012,
    /// FR-DA-3). Private: mutated only through [`apply`] and [`Self::reset`],
    /// which are the only places the retention/staleness rules need to be
    /// enforced.
    sweeps: BTreeMap<u8, DisplaySweep>,
    /// VCP number of the sweeps currently held, if any. Used by [`apply`] to
    /// detect a VCP change, which invalidates the old pattern's elevation
    /// set entirely (S2-W1 §3.3) — an incomplete volume in the *same* VCP
    /// does not trigger this.
    current_vcp: Option<u16>,
    pub last_complete: Option<Arc<VolumeScan>>,
    /// Increments on every applied change. The render loop (Stage 4) will
    /// compare this against the value it last uploaded to the GPU and skip
    /// texture re-upload when unchanged (FR-DR-5).
    pub revision: u64,
}

impl RadarState {
    fn new(site: &'static Site) -> Self {
        Self { site, sweeps: BTreeMap::new(), current_vcp: None, last_complete: None, revision: 0 }
    }

    /// Empties everything and switches to `site`. FR-DA-4 (site change)
    /// consumes this from Stage 7 on; nothing in Stage 2 calls it outside
    /// tests. Always bumps `revision` — a reset is itself a change the
    /// render loop must notice, and `revision` stays a single monotonically
    /// increasing counter across a site switch rather than resetting to a
    /// value the render loop may have already seen.
    pub fn reset(&mut self, site: &'static Site) {
        self.site = site;
        self.sweeps.clear();
        self.current_vcp = None;
        self.last_complete = None;
        self.revision += 1;
    }
}

/// Owned snapshot of [`RadarState`], returned by [`AppState::snapshot`].
/// Nothing in here borrows from the lock.
pub struct StateSnapshot {
    pub site: &'static Site,
    pub sweeps: Vec<DisplaySweep>,
    pub last_complete: Option<Arc<VolumeScan>>,
    pub revision: u64,
    pub ingest: IngestStatus,
}

pub struct AppState {
    radar: RwLock<RadarState>,
    ingest: watch::Receiver<IngestStatus>,
    events: Mutex<EventLog>,
}

impl AppState {
    pub fn new(site: &'static Site, ingest: watch::Receiver<IngestStatus>) -> Self {
        Self { radar: RwLock::new(RadarState::new(site)), ingest, events: Mutex::new(EventLog::new()) }
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
        StateSnapshot {
            site: radar.site,
            sweeps: radar.sweeps.values().cloned().collect(),
            last_complete: radar.last_complete.clone(),
            revision: radar.revision,
            ingest: self.ingest.borrow().clone(),
        }
    }

    /// Apply one assembly event. Returns whether anything changed (i.e.
    /// whether `revision` was bumped) — the pipeline (S2-W2) uses this only
    /// for tests; production code does not need to branch on it.
    ///
    /// `LateRadialsDiscarded`/`MissingStartChunk` are observability, not
    /// radar data (ADR-0012 rule table) — reported here, at the `AppState`
    /// level where the event log lives, rather than by the pure
    /// `state::apply`, which only ever touches `RadarState`.
    pub fn apply_event(&self, event: AssemblyEvent, now: Instant) -> bool {
        match &event {
            AssemblyEvent::LateRadialsDiscarded { elevation_number, count } => {
                self.report(Event::LateRadialsDiscarded { elevation_number: *elevation_number, count: *count });
            }
            AssemblyEvent::MissingStartChunk => self.report(Event::MissingStartChunk),
            AssemblyEvent::SweepClosed { .. } | AssemblyEvent::VolumeClosed { .. } => {}
        }
        apply(&mut self.write_radar(), event, now)
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

    #[cfg(test)]
    pub(crate) fn event_log_len(&self) -> usize {
        self.events.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_site() -> &'static Site {
        crate::sites::by_id("KDOX").expect("KDOX is in the bundled table")
    }

    fn app_state() -> AppState {
        let (_tx, rx) = watch::channel(IngestStatus::default());
        AppState::new(test_site(), rx)
    }

    #[test]
    fn snapshot_of_fresh_state_is_empty() {
        let state = app_state();
        let snap = state.snapshot();
        assert_eq!(snap.site.id, "KDOX");
        assert!(snap.sweeps.is_empty());
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
            AssemblyEvent::LateRadialsDiscarded { elevation_number: 3, count: 5 },
            Instant::now(),
        );
        assert!(!changed);
        assert_eq!(state.event_log_len(), 1, "LateRadialsDiscarded must reach the event log");

        state.apply_event(AssemblyEvent::MissingStartChunk, Instant::now());
        assert_eq!(state.event_log_len(), 2, "MissingStartChunk must reach the event log");
    }

    #[test]
    fn report_forwards_to_the_event_log() {
        let state = app_state();
        assert_eq!(state.event_log_len(), 0);
        state.report(Event::UnrecognizedKeySuffix { key: "KDOX/166/x".to_string() });
        assert_eq!(state.event_log_len(), 1);
    }
}
