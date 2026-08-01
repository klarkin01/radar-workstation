//! Line-preserving, atomic config save (S2-W4 §6.3). Save is
//! read-modify-write at the line level: re-read the file, replace the
//! value on lines whose key this instance changed, append genuinely new
//! keys, and leave every other line — comments, blank lines, unknown keys,
//! other instances' settings — byte-identical. Then write to a temporary
//! file in the same directory and rename over the target, so a crash or a
//! full disk mid-write can never leave a truncated config (Stability as
//! Ethics).
//!
//! Two problems, one solution: FR-CP-2 lets the user hand-edit the file,
//! including comments, and rewriting it wholesale from a struct would
//! erase them. NFR-P-1 expects four instances running at once, and a
//! wholesale rewrite means the last one to save clobbers every setting the
//! others changed.

use std::path::{Path, PathBuf};

use super::parse::line_key;

/// Writes `changes` into the config file at `path`, preserving every line
/// that doesn't define one of the given keys. A key with no matching
/// existing line is appended as a new line. Creates the file if it doesn't
/// exist yet — the parent directory must already exist (callers create it
/// via `paths::config_dir` plus `create_dir_all`, not this function).
pub fn save(path: &Path, changes: &[(String, String)]) -> std::io::Result<()> {
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };

    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    let mut applied = vec![false; changes.len()];

    for line in &mut lines {
        let Some(existing_key) = line_key(line).map(str::to_string) else { continue };
        for (idx, (key, value)) in changes.iter().enumerate() {
            if !applied[idx] && *key == existing_key {
                *line = format!("{key} = {value}");
                applied[idx] = true;
                break;
            }
        }
    }

    for (idx, (key, value)) in changes.iter().enumerate() {
        if !applied[idx] {
            lines.push(format!("{key} = {value}"));
        }
    }

    let mut contents = lines.join("\n");
    if !contents.is_empty() {
        contents.push('\n');
    }

    write_atomic(path, &contents)
}

/// Writes `contents` to a temporary file beside `path` and renames it into
/// place. `rename` within the same filesystem is atomic, so a reader never
/// observes a partially-written file, and a crash mid-write leaves only the
/// temp file behind, never a truncated `path`. The temp name includes the
/// process ID so concurrent instances (NFR-P-1) never collide on it.
fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "config path has no parent directory"))?;
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("config");
    let tmp_path: PathBuf = parent.join(format!(".{file_name}.tmp.{}", std::process::id()));

    std::fs::write(&tmp_path, contents)?;
    std::fs::rename(&tmp_path, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "radar-workstation-config-save-test-{}-{}",
            std::process::id(),
            name
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir.join("config")
    }

    #[test]
    fn creates_a_new_file_when_none_exists() {
        let path = temp_config_path("new-file");
        let _ = std::fs::remove_file(&path);

        save(&path, &[("site".to_string(), "KDOX".to_string())]).expect("save");
        let contents = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(contents, "site = KDOX\n");
    }

    #[test]
    fn updates_an_existing_key_in_place() {
        let path = temp_config_path("update-in-place");
        std::fs::write(&path, "site = KDOX\ningest.poll_interval_seconds = 5\n").unwrap();

        save(&path, &[("site".to_string(), "KTLH".to_string())]).expect("save");
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "site = KTLH\ningest.poll_interval_seconds = 5\n");
    }

    #[test]
    fn appends_a_genuinely_new_key() {
        let path = temp_config_path("append-new-key");
        std::fs::write(&path, "site = KDOX\n").unwrap();

        save(&path, &[("ingest.poll_interval_seconds".to_string(), "10".to_string())]).expect("save");
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "site = KDOX\ningest.poll_interval_seconds = 10\n");
    }

    #[test]
    fn comments_survive_a_save() {
        let path = temp_config_path("comments-survive");
        std::fs::write(&path, "# a helpful comment\nsite = KDOX\n\n# another one\n").unwrap();

        save(&path, &[("site".to_string(), "KTLH".to_string())]).expect("save");
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "# a helpful comment\nsite = KTLH\n\n# another one\n");
    }

    #[test]
    fn unknown_keys_survive_a_save() {
        let path = temp_config_path("unknown-keys-survive");
        std::fs::write(&path, "site = KDOX\nsome_future_key = value\n").unwrap();

        save(&path, &[("site".to_string(), "KTLH".to_string())]).expect("save");
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "site = KTLH\nsome_future_key = value\n");
    }

    #[test]
    fn atomic_write_leaves_no_partial_file_and_no_temp_file_behind() {
        let path = temp_config_path("atomic-write");
        std::fs::write(&path, "site = KDOX\n").unwrap();

        save(&path, &[("site".to_string(), "KTLH".to_string())]).expect("save");

        let dir = path.parent().unwrap();
        let leftover_temp_files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftover_temp_files.is_empty(), "temp file must be renamed away, not left behind");
    }

    /// Simulates two concurrent instances (NFR-P-1) each changing a
    /// different key: write A, write B, re-read — both changes must
    /// survive, neither clobbering the other. Sequential here (this test
    /// isn't exercising true OS-level concurrency), but it directly tests
    /// the property that makes concurrent saves safe: each save only ever
    /// touches the line(s) for the key(s) it changed.
    #[test]
    fn concurrent_instance_simulation_preserves_both_changes() {
        let path = temp_config_path("concurrent-simulation");
        std::fs::write(&path, "site = KDOX\ningest.poll_interval_seconds = 5\n").unwrap();

        // Instance A changes `site`.
        save(&path, &[("site".to_string(), "KTLH".to_string())]).expect("save A");
        // Instance B changes `ingest.poll_interval_seconds`, independently.
        save(&path, &[("ingest.poll_interval_seconds".to_string(), "10".to_string())]).expect("save B");

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "site = KTLH\ningest.poll_interval_seconds = 10\n");
    }
}
