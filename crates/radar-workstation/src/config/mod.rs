//! Configuration persistence (S2-W4, FR-CP-1/2/3, ADR-0019). A
//! workspace-local `key = value` parser (S2-f), not `toml` + `serde` — see
//! ADR-0019 for the reasoning: this project's answer to "an untrusted-input
//! parser on a must-not-crash path" has consistently been to own it and
//! fuzz it, same as `http-ingest` and `nexrad-decoder`.
//!
//! Loading a config **never returns an error**. [`load`] returns
//! `(Config, Vec<Event>)` — an unparseable line, a bad value, an unknown
//! site, or a missing/unreadable file are all reported as typed events and
//! produce a default, never a fatal error. FR-CP-3 ("must start
//! successfully with a missing or corrupt configuration file") is
//! therefore a property of this function's signature, not a code path
//! someone has to remember to test.

mod parse;
mod save;

use std::path::Path;
use std::time::Duration;

use crate::event::Event;
use crate::ingest::s3_poll;
use crate::sites::{self, Site};

pub use save::save;

/// FR-DA-2's configurable polling interval is clamped to this range.
/// Un-clamped, a small value would hammer a public S3 bucket.
pub const POLL_INTERVAL_MIN: Duration = Duration::from_secs(2);
pub const POLL_INTERVAL_MAX: Duration = Duration::from_secs(60);

const SITE_KEY: &str = "site";
const POLL_INTERVAL_KEY: &str = "ingest.poll_interval_seconds";

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// `None` if the file had no `site` key, an unparseable one, or an
    /// unknown site ID — the caller (CLI site resolution, §6.4) decides
    /// what to do about that; this module's job is only to say cleanly
    /// what the *file* said.
    pub site: Option<&'static Site>,
    pub poll_interval: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self { site: None, poll_interval: s3_poll::DEFAULT_POLL_INTERVAL }
    }
}

/// Loads configuration from `path`.
///
/// - Missing file: silent defaults — the expected first-run case, not an
///   error, and not reported.
/// - Any other read failure (permissions, not a regular file, etc.):
///   defaults, plus one `Event::ConfigUnreadable`.
/// - Malformed content (bad lines, bad values, an unknown site, an
///   out-of-range interval): the affected field falls back to its default,
///   and each problem is reported as its own typed [`Event`]. Every other
///   valid key still takes effect — one bad line does not throw away the
///   rest of the file.
pub fn load(path: &Path) -> (Config, Vec<Event>) {
    let mut events = Vec::new();

    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Config::default(), events),
        Err(e) => {
            events.push(Event::ConfigUnreadable { path: path.display().to_string(), reason: e.to_string() });
            return (Config::default(), events);
        }
    };

    let (pairs, skipped_lines) = parse::parse(&text);
    events.extend(skipped_lines.into_iter().map(|line| Event::ConfigLineUnparseable { line }));

    let mut config = Config::default();
    for (key, value) in pairs {
        apply_key(&mut config, &key, value, &mut events);
    }

    (config, events)
}

fn apply_key(config: &mut Config, key: &str, value: String, events: &mut Vec<Event>) {
    match key {
        SITE_KEY => match sites::by_id(&value) {
            Some(site) => config.site = Some(site),
            None => events.push(Event::ConfigUnknownSite { site: value }),
        },
        POLL_INTERVAL_KEY => match value.parse::<u64>() {
            Ok(secs) => {
                let requested = Duration::from_secs(secs);
                let clamped = requested.clamp(POLL_INTERVAL_MIN, POLL_INTERVAL_MAX);
                if clamped != requested {
                    events.push(Event::ConfigPollIntervalClamped { requested_secs: secs, clamped_secs: clamped.as_secs() });
                }
                config.poll_interval = clamped;
            }
            Err(_) => events.push(Event::ConfigValueInvalid { key: key.to_string(), value }),
        },
        // Unrecognized but syntactically valid keys are ignored, not
        // reported: they may be a newer version's setting this binary
        // doesn't know about yet (NFR-P-1's multi-instance, multi-version
        // file sharing), and `save` already preserves them untouched.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config_path(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("radar-workstation-config-load-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir.join("config")
    }

    #[test]
    fn missing_file_yields_defaults_with_no_events() {
        let path = temp_config_path("missing-file");
        let _ = std::fs::remove_file(&path);

        let (config, events) = load(&path);
        assert_eq!(config, Config::default());
        assert!(events.is_empty(), "a missing config file is the expected first run, not an error");
    }

    #[test]
    fn round_trips_a_valid_file() {
        let path = temp_config_path("round-trip");
        std::fs::write(&path, "site = KDOX\ningest.poll_interval_seconds = 10\n").unwrap();

        let (config, events) = load(&path);
        assert!(events.is_empty());
        assert_eq!(config.site.map(|s| s.id), Some("KDOX"));
        assert_eq!(config.poll_interval, Duration::from_secs(10));
    }

    #[test]
    fn unknown_site_falls_back_to_default_and_is_reported() {
        let path = temp_config_path("unknown-site");
        std::fs::write(&path, "site = ZZZZ\n").unwrap();

        let (config, events) = load(&path);
        assert!(config.site.is_none());
        assert!(matches!(events.as_slice(), [Event::ConfigUnknownSite { site }] if site == "ZZZZ"));
    }

    #[test]
    fn poll_interval_below_minimum_is_clamped_and_reported() {
        let path = temp_config_path("clamp-low");
        std::fs::write(&path, "ingest.poll_interval_seconds = 1\n").unwrap();

        let (config, events) = load(&path);
        assert_eq!(config.poll_interval, POLL_INTERVAL_MIN);
        assert!(matches!(
            events.as_slice(),
            [Event::ConfigPollIntervalClamped { requested_secs: 1, clamped_secs }] if *clamped_secs == POLL_INTERVAL_MIN.as_secs()
        ));
    }

    #[test]
    fn poll_interval_above_maximum_is_clamped_and_reported() {
        let path = temp_config_path("clamp-high");
        std::fs::write(&path, "ingest.poll_interval_seconds = 999\n").unwrap();

        let (config, _events) = load(&path);
        assert_eq!(config.poll_interval, POLL_INTERVAL_MAX);
    }

    #[test]
    fn unparseable_poll_interval_falls_back_to_default_and_is_reported() {
        let path = temp_config_path("bad-interval-value");
        std::fs::write(&path, "ingest.poll_interval_seconds = not_a_number\n").unwrap();

        let (config, events) = load(&path);
        assert_eq!(config.poll_interval, s3_poll::DEFAULT_POLL_INTERVAL);
        assert!(matches!(events.as_slice(), [Event::ConfigValueInvalid { key, .. }] if key == POLL_INTERVAL_KEY));
    }

    #[test]
    fn malformed_lines_are_reported_but_do_not_prevent_valid_keys_from_loading() {
        let path = temp_config_path("partial-corruption");
        std::fs::write(&path, "this is garbage\nsite = KDOX\n= no key\n").unwrap();

        let (config, events) = load(&path);
        assert_eq!(config.site.map(|s| s.id), Some("KDOX"));
        assert_eq!(events.len(), 2, "both malformed lines reported, one per line");
        assert!(events.iter().all(|e| matches!(e, Event::ConfigLineUnparseable { .. })));
    }

    #[test]
    fn binary_garbage_file_yields_defaults_and_is_reported_not_a_panic() {
        let path = temp_config_path("binary-garbage");
        // Invalid UTF-8: a lone continuation byte can never start a valid
        // UTF-8 sequence, so `read_to_string` fails cleanly.
        std::fs::write(&path, [0xFFu8, 0xFE, 0x00, 0x01, 0x02]).unwrap();

        let (config, events) = load(&path);
        assert_eq!(config, Config::default());
        assert!(matches!(events.as_slice(), [Event::ConfigUnreadable { .. }]));
    }

    #[test]
    fn duplicate_keys_never_panic_and_use_the_last_value() {
        let path = temp_config_path("duplicate-keys");
        std::fs::write(&path, "site = KDOX\nsite = KTLH\n").unwrap();

        let (config, _events) = load(&path);
        assert_eq!(config.site.map(|s| s.id), Some("KTLH"));
    }

    /// §9's "time to load and parse configuration at startup (should be
    /// sub-millisecond)" measurement, kept as a regression guard rather
    /// than a one-off: a realistic-sized file loading in well under 50ms
    /// is the property that matters for the < 2s first-render budget
    /// (Stage 4 onward), not a specific number. Run with `--nocapture` to
    /// see the measured value.
    #[test]
    fn load_of_a_realistic_file_is_fast() {
        let path = temp_config_path("timing");
        std::fs::write(&path, "# comment\nsite = KDOX\ningest.poll_interval_seconds = 5\n").unwrap();

        let start = std::time::Instant::now();
        let (_config, _events) = load(&path);
        let elapsed = start.elapsed();

        println!("config::load of a realistic file took {elapsed:?}");
        assert!(elapsed < Duration::from_millis(50), "load took {elapsed:?}, expected well under 50ms");
    }

    #[test]
    fn a_ten_megabyte_single_line_never_panics() {
        let path = temp_config_path("huge-line");
        let mut contents = "site = KDOX\n".to_string();
        contents.push_str(&format!("some_unknown_key = {}\n", "a".repeat(10 * 1024 * 1024)));
        std::fs::write(&path, &contents).unwrap();

        let (config, _events) = load(&path);
        assert_eq!(config.site.map(|s| s.id), Some("KDOX"));
    }
}
