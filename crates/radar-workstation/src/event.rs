//! Workspace-local typed event reporting (S1-W3c). No logging crate: every
//! error this project produces needs to reach a status watch channel as
//! typed data anyway (see [`crate::ingest::s3_poll::IngestStatus`]), so a
//! string log line would be a second mechanism carrying the same
//! information. This keeps the production dependency graph untouched;
//! revisit at Stage 4 if `RUST_LOG`-style filtering across ten-plus
//! subsystems makes the cost worth it.

use crate::ingest::s3_poll::PollError;

#[derive(Debug)]
pub enum Event {
    PollFailed(PollError),
    UnrecognizedKeySuffix { key: String },
    UnrecognizedVolumeFolder { entry: String },
    ReAnchored { from_volume: u64, to_volume: u64, empty_polls: u32 },
    AdvancedPastStalledVolume { from_volume: u64, to_volume: u64, empty_polls: u32 },
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
        }
    }
}

/// The one place an [`Event`] becomes text. Until a UI-facing consumer
/// exists (Stage 2+), this is the only sink — extend this function, or add
/// a channel-based one alongside it, rather than reaching for `eprintln!`
/// elsewhere.
pub fn log_to_stderr(event: &Event) {
    eprintln!("[radar-workstation] {event}");
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
}
