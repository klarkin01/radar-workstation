//! Low-level line-oriented parser for the `key = value` config format
//! (S2-f, ADR-0019). Untrusted input on a must-not-crash path (FR-CP-3) —
//! same posture as `http-ingest`'s response parser and `nexrad-decoder`'s
//! block parser: owned by this project, not a dependency, and fuzzed (see
//! `tests/config_hardening.rs`).
//!
//! Format: `# comment` lines, blank lines, and `key = value` lines. Keys
//! are dot-separated segments of ASCII alphanumerics/underscore
//! (`ingest.poll_interval_seconds`), used for grouping only — this parser
//! does not build a nested structure, it returns a flat ordered list.
//! Values are whatever text follows `=`, trimmed, with no further typing:
//! callers (`config::load`) parse each specific key's value into whatever
//! type that key needs and decide what "invalid" means for it.

/// Parses `text` into an ordered list of `(key, value)` pairs, plus the
/// 1-indexed line numbers of every line that was neither blank, a
/// comment, nor a valid `key = value` pair. Never panics, never returns an
/// `Err` — malformed input is data to report, not a reason to stop.
///
/// Duplicate keys: the last occurrence wins, silently. A config file is
/// user-edited text, not a wire protocol — a duplicate key is far more
/// likely to be an accidental paste than something worth flagging.
pub fn parse(text: &str) -> (Vec<(String, String)>, Vec<usize>) {
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut skipped = Vec::new();

    for (idx, raw_line) in text.lines().enumerate() {
        let line_number = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            skipped.push(line_number);
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if !is_valid_key(key) {
            skipped.push(line_number);
            continue;
        }

        match pairs.iter_mut().find(|(k, _)| k == key) {
            Some((_, existing_value)) => *existing_value = value.to_string(),
            None => pairs.push((key.to_string(), value.to_string())),
        }
    }

    (pairs, skipped)
}

/// The key a single line defines, if it's a non-comment, non-blank,
/// syntactically valid `key = value` line. Used by `save` to find which
/// existing line (if any) defines a given key — shares this notion of
/// "valid key" with [`parse`] so the two never disagree about what counts
/// as one.
pub(super) fn line_key(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let (key, _) = trimmed.split_once('=')?;
    let key = key.trim();
    is_valid_key(key).then_some(key)
}

/// One or more dot-separated segments, each a non-empty run of ASCII
/// alphanumerics/underscore. Rejects anything else (leading/trailing dots,
/// empty segments, non-ASCII) as unparseable rather than accepting a key
/// that could never match a known config field anyway.
fn is_valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.split('.').all(|segment| !segment.is_empty() && segment.chars().all(is_key_char))
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_key_value_pairs() {
        let (pairs, skipped) = parse("site = KDOX\ningest.poll_interval_seconds = 5\n");
        assert_eq!(pairs, vec![
            ("site".to_string(), "KDOX".to_string()),
            ("ingest.poll_interval_seconds".to_string(), "5".to_string()),
        ]);
        assert!(skipped.is_empty());
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        let (pairs, skipped) = parse("# a comment\n\nsite = KDOX\n   # indented comment\n");
        assert_eq!(pairs, vec![("site".to_string(), "KDOX".to_string())]);
        assert!(skipped.is_empty());
    }

    #[test]
    fn trims_whitespace_around_key_and_value() {
        let (pairs, _) = parse("  site   =   KDOX  \n");
        assert_eq!(pairs, vec![("site".to_string(), "KDOX".to_string())]);
    }

    #[test]
    fn last_duplicate_key_wins() {
        let (pairs, skipped) = parse("site = KDOX\nsite = KTLH\n");
        assert_eq!(pairs, vec![("site".to_string(), "KTLH".to_string())]);
        assert!(skipped.is_empty());
    }

    #[test]
    fn reports_lines_with_no_equals_sign() {
        let (pairs, skipped) = parse("this is not valid\nsite = KDOX\n");
        assert!(pairs.iter().any(|(k, _)| k == "site"));
        assert_eq!(skipped, vec![1]);
    }

    #[test]
    fn reports_equals_with_empty_or_invalid_key() {
        let (_, skipped) = parse("= no key\nbad key! = value\n");
        assert_eq!(skipped, vec![1, 2]);
    }

    #[test]
    fn accepts_dotted_grouping_keys() {
        let (pairs, skipped) = parse("ingest.poll_interval_seconds = 10\n");
        assert_eq!(pairs, vec![("ingest.poll_interval_seconds".to_string(), "10".to_string())]);
        assert!(skipped.is_empty());
    }

    #[test]
    fn value_may_itself_contain_an_equals_sign() {
        let (pairs, _) = parse("placefile_url = https://example.com/x?a=1\n");
        assert_eq!(pairs, vec![("placefile_url".to_string(), "https://example.com/x?a=1".to_string())]);
    }

    #[test]
    fn empty_value_is_accepted() {
        let (pairs, skipped) = parse("site =\n");
        assert_eq!(pairs, vec![("site".to_string(), String::new())]);
        assert!(skipped.is_empty());
    }

    #[test]
    fn handles_a_ten_megabyte_single_line_without_panicking() {
        let huge_value = "a".repeat(10 * 1024 * 1024);
        let text = format!("site = {huge_value}");
        let (pairs, skipped) = parse(&text);
        assert_eq!(pairs, vec![("site".to_string(), huge_value)]);
        assert!(skipped.is_empty());
    }

    #[test]
    fn line_key_matches_parse_for_valid_lines() {
        assert_eq!(line_key("site = KDOX"), Some("site"));
        assert_eq!(line_key("  ingest.poll_interval_seconds = 5  "), Some("ingest.poll_interval_seconds"));
    }

    #[test]
    fn line_key_is_none_for_comments_blanks_and_invalid_lines() {
        assert_eq!(line_key("# comment"), None);
        assert_eq!(line_key(""), None);
        assert_eq!(line_key("   "), None);
        assert_eq!(line_key("not a key value line"), None);
        assert_eq!(line_key("bad key! = value"), None);
    }
}
