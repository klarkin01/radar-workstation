//! Workspace-local typed event reporting (S1-W3c). No logging crate: every
//! error this project produces needs to reach a status watch channel as
//! typed data anyway (see [`crate::ingest::s3_poll::IngestStatus`]), so a
//! string log line would be a second mechanism carrying the same
//! information. This keeps the production dependency graph untouched;
//! revisit at Stage 4 if `RUST_LOG`-style filtering across ten-plus
//! subsystems makes the cost worth it.

use crate::assembly::VolumeId;
use crate::compute::DisplayProduct;
use crate::ingest::s3_poll::PollError;
use crate::ingest::volume_seq::VolumeSeq;

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
    ReAnchored { from_volume: VolumeSeq, to_volume: VolumeSeq, empty_polls: u32 },
    AdvancedPastStalledVolume { from_volume: VolumeSeq, to_volume: VolumeSeq, empty_polls: u32 },
    /// The retained volume-sequence folders did not contain a gap large
    /// enough to identify the retention boundary confidently, so "newest"
    /// was resolved from a smaller gap than the bucket's observed behaviour
    /// predicts. Reported, not fatal — the poller still picks deterministically.
    VolumeSequenceOrderAmbiguous { largest_gap: u32, folder_count: usize },
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

    // --- compute layer (S3-W1/W2, `compute::grid`) ---
    /// A sweep's modal `azimuth_spacing_code` was neither 1 (super-res) nor
    /// 2 (standard-res); the azimuth count was inferred from radial count
    /// instead.
    UnknownAzimuthSpacingCode { elevation_number: u8, inferred_azimuth_count: u16 },
    /// A sweep's moment geometry for `product` was degenerate (zero gate
    /// width or zero gate count) and could not be gridded.
    DegenerateGateGeometry { product: DisplayProduct, elevation_number: u8 },
    /// One or more radials carried gate geometry for `product` inconsistent
    /// with the sweep's chosen geometry and were skipped rather than
    /// partially copied into a row sized for different geometry.
    InconsistentGateGeometry { product: DisplayProduct, elevation_number: u8, skipped: usize },
    /// Two or more radials scattered into the same azimuth slot for
    /// `product` (commonly at the 0°/360° seam); the last writer won.
    DuplicateAzimuthSlot { product: DisplayProduct, elevation_number: u8, count: usize },
    /// The compute layer's retained-reflectivity-grid set (held across an
    /// accumulating volume for Echo Tops/VIL) exceeded its bound; the
    /// oldest tilt was dropped rather than letting a stuck volume leak.
    RetainedGridSetBounded { dropped_elevation_number: u8 },

    // --- volume history retention (Stage 6a Part B, `state::history`, ADR-0030) ---
    /// The byte budget, not the requested frame count, is what bounds the
    /// history — the loop is shorter than the operator asked for. Edge-
    /// triggered: reported when the constraint starts binding, not per
    /// eviction.
    HistoryBudgetBound { frames_retained: usize, requested_frames: usize, bytes: usize },
    /// A sweep or derived set arrived for a volume older than every
    /// retained frame, and no retained frame matched it either. Discarded:
    /// landing it behind the ring's head would break the ordering
    /// invariant every other function relies on. Observability, not data
    /// (ADR-0012's rule table).
    LateVolumeDiscarded { volume: VolumeId },

    // --- colour tables (S3-W3, `compute::palette`) ---
    /// A `.pal` line was neither a comment, blank, nor a recognized,
    /// well-formed directive.
    PaletteLineUnparseable { product: DisplayProduct, line: usize },
    /// A user override palette exists but could not be read (permissions,
    /// not a regular file, etc.) — distinct from simply not existing, which
    /// is the expected common case and is not reported.
    UserPaletteUnreadable { product: DisplayProduct, path: String, reason: String },
    /// A user override palette was readable but produced no usable colour
    /// entries; the bundled default was used instead.
    UserPaletteMalformed { product: DisplayProduct, path: String },

    // --- map underlay bundle (S5, `overlay`) ---
    /// The compiled-in overlay bundle failed validation; no map underlay is
    /// drawn. Cannot happen for a bundle this project generated — handled
    /// anyway (Stability as Ethics).
    OverlayBundleInvalid { reason: &'static str },
    /// A bundle layer carried a kind this build does not know; it was
    /// skipped rather than treated as an error, so a future bundle can add
    /// a layer without a version bump breaking an older binary.
    OverlayLayerUnknownKind { kind: u32 },
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
            Self::VolumeSequenceOrderAmbiguous { largest_gap, folder_count } => write!(
                f,
                "volume-sequence folders ({folder_count}) had no gap wide enough (largest {largest_gap}) \
                 to identify the retention boundary confidently; newest volume resolved from a smaller gap"
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
            Self::UnknownAzimuthSpacingCode { elevation_number, inferred_azimuth_count } => write!(
                f,
                "elevation {elevation_number}: unrecognized modal azimuth spacing code, inferred \
                 {inferred_azimuth_count} azimuths from radial count"
            ),
            Self::DegenerateGateGeometry { product, elevation_number } => write!(
                f,
                "elevation {elevation_number}: {product} has degenerate gate geometry (zero width or \
                 zero count), not gridded"
            ),
            Self::InconsistentGateGeometry { product, elevation_number, skipped } => write!(
                f,
                "elevation {elevation_number}: {skipped} {product} radial(s) had inconsistent gate \
                 geometry, skipped rather than gridded"
            ),
            Self::DuplicateAzimuthSlot { product, elevation_number, count } => write!(
                f,
                "elevation {elevation_number}: {count} {product} radial(s) landed on an \
                 already-filled azimuth slot, last writer kept"
            ),
            Self::RetainedGridSetBounded { dropped_elevation_number } => write!(
                f,
                "retained reflectivity grid set exceeded its bound, dropped elevation \
                 {dropped_elevation_number}"
            ),
            Self::HistoryBudgetBound { frames_retained, requested_frames, bytes } => write!(
                f,
                "history.budget_mb is binding before history.frames: retaining {frames_retained} of \
                 {requested_frames} requested volumes ({} MiB)",
                bytes / (1024 * 1024)
            ),
            Self::LateVolumeDiscarded { volume } => write!(
                f,
                "discarded a sweep/derived update for volume {volume:?}, older than every retained frame"
            ),
            Self::PaletteLineUnparseable { product, line } => {
                write!(f, "{product} palette line {line} is not valid, skipping")
            }
            Self::UserPaletteUnreadable { product, path, reason } => {
                write!(f, "user {product} palette {path} unreadable: {reason}, using bundled default")
            }
            Self::UserPaletteMalformed { product, path } => {
                write!(f, "user {product} palette {path} had no usable colour entries, using bundled default")
            }
            Self::OverlayBundleInvalid { reason } => {
                write!(f, "compiled-in overlay bundle failed validation ({reason}), no map underlay drawn")
            }
            Self::OverlayLayerUnknownKind { kind } => {
                write!(f, "overlay bundle layer kind {kind} is not recognized by this build, skipping")
            }
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

    /// The most recent `max` events, newest last, each formatted through
    /// [`Event`]'s `Display` impl — `Event` is not `Clone`, and the status
    /// bar (S4-W6 §9.1, NFR-ST-3) only ever needs the text. Formatting
    /// happens while the caller holds `AppState`'s mutex; keep `max` small.
    pub fn recent(&self, max: usize) -> Vec<(std::time::Instant, String)> {
        let skip = self.entries.len().saturating_sub(max);
        self.entries.iter().skip(skip).map(|(at, event)| (*at, event.to_string())).collect()
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
        let event = Event::ReAnchored {
            from_volume: VolumeSeq::new(79).unwrap(),
            to_volume: VolumeSeq::new(165).unwrap(),
            empty_polls: 12,
        };
        let text = event.to_string();
        assert!(text.contains("79"));
        assert!(text.contains("165"));
        assert!(text.contains("12"));

        let bound = Event::HistoryBudgetBound { frames_retained: 8, requested_frames: 12, bytes: 320 * 1024 * 1024 };
        let text = bound.to_string();
        assert!(text.contains('8'));
        assert!(text.contains("12"));
        assert!(text.contains("320"));

        let discarded = Event::LateVolumeDiscarded { volume: VolumeId { julian_date: 20_000, scan_time_ms: 100 } };
        let text = discarded.to_string();
        assert!(text.contains("20000") || text.contains("20_000"));
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
