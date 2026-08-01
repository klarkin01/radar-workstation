//! Workspace-local typed event reporting (S1-W3c). No logging crate: every
//! error this project produces needs to reach a status watch channel as
//! typed data anyway (see [`crate::ingest::s3_poll::IngestStatus`]), so a
//! string log line would be a second mechanism carrying the same
//! information. This keeps the production dependency graph untouched;
//! revisit at Stage 4 if `RUST_LOG`-style filtering across ten-plus
//! subsystems makes the cost worth it.

use crate::ingest::s3_poll::PollError;

/// Which supervised unit an event pertains to (S2-W2 §4.3). One variant
/// today: the poller/assembly/applier trio is supervised together, not
/// independently — see `pipeline`'s top-level doc comment for why. Later
/// stages (tile fetching, placefile polling) that don't share this trio's
/// channel coupling can add their own variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    IngestPipeline,
}

impl std::fmt::Display for TaskKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IngestPipeline => write!(f, "ingest pipeline"),
        }
    }
}

#[derive(Debug)]
pub enum Event {
    PollFailed(PollError),
    UnrecognizedKeySuffix { key: String },
    UnrecognizedVolumeFolder { entry: String },
    ReAnchored { from_volume: u64, to_volume: u64, empty_polls: u32 },
    AdvancedPastStalledVolume { from_volume: u64, to_volume: u64, empty_polls: u32 },
    /// Mirrors `assembly::AssemblyEvent::LateRadialsDiscarded` — reported by
    /// `AppState::apply_event` (S2-W1/W2), not by the pure `state::apply`.
    LateRadialsDiscarded { elevation_number: u8, count: usize },
    /// Mirrors `assembly::AssemblyEvent::MissingStartChunk`.
    MissingStartChunk,
    /// A pipeline task's `JoinHandle` resolved to a panic. The supervisor
    /// (S2-W2 §4.3) always follows this with a `TaskRestarted` once the
    /// backoff elapses.
    TaskPanicked { task: TaskKind },
    TaskRestarted { task: TaskKind, after: std::time::Duration },
    /// The config file exists but couldn't be read (permissions, not a
    /// regular file, etc.) — distinct from simply not existing yet, which
    /// is the expected first-run case and is not reported at all (FR-CP-3).
    ConfigUnreadable { path: String, reason: String },
    /// A line was neither blank, a comment, nor a valid `key = value` pair.
    ConfigLineUnparseable { line: usize },
    /// The `site` key's value doesn't match any bundled site ID.
    ConfigUnknownSite { site: String },
    /// A key's value didn't parse as that key's expected type.
    ConfigValueInvalid { key: String, value: String },
    /// `ingest.poll_interval_seconds` was outside `[POLL_INTERVAL_MIN,
    /// POLL_INTERVAL_MAX]` and was clamped rather than rejected outright —
    /// an unclamped small value would hammer a public S3 bucket.
    ConfigPollIntervalClamped { requested_secs: u64, clamped_secs: u64 },
}

impl std::fmt::Display for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PollFailed(e) => write!(f, "poll failed: {e}"),
            Self::UnrecognizedKeySuffix { key } => {
                write!(f, "unrecognized key suffix, skipping: {key}")
            }
            Self::UnrecognizedVolumeFolder { entry } => {
                write!(f, "unrecognized volume folder, skipping: {entry}")
            }
            Self::ReAnchored { from_volume, to_volume, empty_polls } => write!(
                f,
                "re-anchored from volume {from_volume} to {to_volume} after {empty_polls} \
                 empty polls with no key ever seen"
            ),
            Self::AdvancedPastStalledVolume { from_volume, to_volume, empty_polls } => write!(
                f,
                "advanced past stalled volume {from_volume} to {to_volume} after \
                 {empty_polls} empty polls (assembly watchdog will mark it TimedOut)"
            ),
            Self::LateRadialsDiscarded { elevation_number, count } => write!(
                f,
                "discarded {count} late radial(s) for already-closed elevation {elevation_number}"
            ),
            Self::MissingStartChunk => write!(f, "missing start (-S) chunk for the current volume"),
            Self::TaskPanicked { task } => write!(f, "pipeline task {task} panicked"),
            Self::TaskRestarted { task, after } => {
                write!(f, "pipeline task {task} restarting after {after:?}")
            }
            Self::ConfigUnreadable { path, reason } => write!(f, "config file {path} unreadable: {reason}"),
            Self::ConfigLineUnparseable { line } => write!(f, "config line {line} is not valid, skipping"),
            Self::ConfigUnknownSite { site } => {
                write!(f, "config site {site:?} is not in the bundled site list, using default")
            }
            Self::ConfigValueInvalid { key, value } => {
                write!(f, "config key {key} has an invalid value {value:?}, using default")
            }
            Self::ConfigPollIntervalClamped { requested_secs, clamped_secs } => write!(
                f,
                "config ingest.poll_interval_seconds={requested_secs} clamped to {clamped_secs}"
            ),
        }
    }
}

/// The one place an [`Event`] becomes text. `AppState::report` (S2-W1) is
/// now the primary caller — it forwards every event here *and* into the
/// bounded in-memory log below, so this stays the single formatting path
/// rather than growing a second one. Call sites with no `Arc<AppState>` in
/// scope (unit tests, poller internals before wiring) may still call this
/// directly.
pub fn log_to_stderr(event: &Event) {
    eprintln!("[radar-workstation] {event}");
}

/// How many recent events [`EventLog`] retains. An unbounded diagnostic
/// buffer in a process that runs for hours during an active weather event is
/// a memory leak with a friendly name (S2-W1 §3.4) — this is deliberately
/// small, since its only consumer so far is NFR-ST-3's future status bar,
/// which only ever wants the most recent handful.
const EVENT_LOG_CAPACITY: usize = 64;

/// A bounded ring buffer of recently reported events, each timestamped at
/// the moment it was pushed. Lives behind `AppState`'s own `Mutex` — this
/// type itself holds no lock.
pub struct EventLog {
    entries: std::collections::VecDeque<(std::time::Instant, Event)>,
}

impl EventLog {
    pub fn new() -> Self {
        Self { entries: std::collections::VecDeque::with_capacity(EVENT_LOG_CAPACITY) }
    }

    pub fn push(&mut self, event: Event) {
        if self.entries.len() == EVENT_LOG_CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back((std::time::Instant::now(), event));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for EventLog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_formats_are_human_readable() {
        let event = Event::ReAnchored { from_volume: 79, to_volume: 165, empty_polls: 12 };
        let text = event.to_string();
        assert!(text.contains("79"));
        assert!(text.contains("165"));
        assert!(text.contains("12"));
    }

    fn sample_event() -> Event {
        Event::UnrecognizedKeySuffix { key: "KDOX/166/x".to_string() }
    }

    #[test]
    fn event_log_starts_empty() {
        let log = EventLog::new();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn event_log_grows_up_to_capacity() {
        let mut log = EventLog::new();
        for _ in 0..EVENT_LOG_CAPACITY {
            log.push(sample_event());
        }
        assert_eq!(log.len(), EVENT_LOG_CAPACITY);
    }

    #[test]
    fn event_log_evicts_oldest_past_capacity() {
        let mut log = EventLog::new();
        for _ in 0..(EVENT_LOG_CAPACITY + 10) {
            log.push(sample_event());
        }
        assert_eq!(log.len(), EVENT_LOG_CAPACITY, "must stay bounded, not grow unboundedly");
    }
}
