//! XDG base directory paths (S2-W4 §6.1, S2-g). Computed with `std::env`
//! directly rather than the `directories`/`dirs` crate: the application is
//! Linux-only (`REQUIREMENTS.md`), so this is three environment variables
//! with documented fallbacks — a few dozen lines, against a crate whose
//! value is cross-platform behavior this project does not need.

use std::env;
use std::path::PathBuf;

const APP_DIR_NAME: &str = "radar-workstation";

/// `$XDG_CONFIG_HOME/radar-workstation`, falling back to
/// `$HOME/.config/radar-workstation`. `None` if neither can be resolved —
/// `HOME` unset is a real condition on service accounts, and callers must
/// degrade to running with defaults and persisting nothing, never panic.
pub fn config_dir() -> Option<PathBuf> {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

/// `$XDG_CACHE_HOME/radar-workstation`, falling back to
/// `$HOME/.cache/radar-workstation`. Stage 5's tile cache (FR-MU-5) uses
/// this; no caller yet.
pub fn cache_dir() -> Option<PathBuf> {
    xdg_dir("XDG_CACHE_HOME", ".cache")
}

/// `$XDG_DATA_HOME/radar-workstation`, falling back to
/// `$HOME/.local/share/radar-workstation`. Stage 3's user color tables
/// (FR-CT-3) use this; no caller yet.
pub fn data_dir() -> Option<PathBuf> {
    xdg_dir("XDG_DATA_HOME", ".local/share")
}

/// Resolves one XDG base directory: `$<xdg_var>/radar-workstation` if
/// `xdg_var` is set to a non-empty **absolute** path (the XDG Base
/// Directory spec requires an absolute path — a relative value is invalid
/// and must be ignored, not joined blindly onto an unknown base), else
/// `$HOME/<home_fallback>/radar-workstation`, else `None`.
fn xdg_dir(xdg_var: &str, home_fallback: &str) -> Option<PathBuf> {
    if let Some(value) = env::var_os(xdg_var) {
        let path = PathBuf::from(value);
        if !path.as_os_str().is_empty() && path.is_absolute() {
            return Some(path.join(APP_DIR_NAME));
        }
        // Set but empty or relative: invalid per spec. Fall through to the
        // $HOME-based default rather than treating it as usable.
    }
    let home = PathBuf::from(env::var_os("HOME")?);
    if !home.is_absolute() {
        return None;
    }
    Some(home.join(home_fallback).join(APP_DIR_NAME))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Every test in this module mutates process-wide environment
    /// variables (`std::env::set_var`), which is inherently a shared,
    /// racy resource across parallel test threads. Serializing through
    /// one lock — rather than relying on `cargo test`'s default thread
    /// count — is what makes these tests deterministic.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        vars: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvGuard {
        fn capture(names: &[&'static str]) -> Self {
            Self { vars: names.iter().map(|&n| (n, env::var_os(n))).collect() }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in &self.vars {
                match value {
                    Some(v) => env::set_var(name, v),
                    None => env::remove_var(name),
                }
            }
        }
    }

    const VARS: &[&str] = &["XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "HOME"];

    #[test]
    fn xdg_var_set_to_absolute_path_is_used() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = EnvGuard::capture(VARS);

        env::set_var("XDG_CONFIG_HOME", "/tmp/xdg-config-test");
        assert_eq!(config_dir(), Some(PathBuf::from("/tmp/xdg-config-test/radar-workstation")));
    }

    #[test]
    fn falls_back_to_home_when_xdg_var_unset() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = EnvGuard::capture(VARS);

        env::remove_var("XDG_CONFIG_HOME");
        env::set_var("HOME", "/home/testuser");
        assert_eq!(config_dir(), Some(PathBuf::from("/home/testuser/.config/radar-workstation")));

        env::remove_var("XDG_CACHE_HOME");
        assert_eq!(cache_dir(), Some(PathBuf::from("/home/testuser/.cache/radar-workstation")));

        env::remove_var("XDG_DATA_HOME");
        assert_eq!(data_dir(), Some(PathBuf::from("/home/testuser/.local/share/radar-workstation")));
    }

    #[test]
    fn relative_xdg_var_is_ignored_in_favor_of_home_fallback() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = EnvGuard::capture(VARS);

        env::set_var("XDG_CONFIG_HOME", "relative/path");
        env::set_var("HOME", "/home/testuser");
        assert_eq!(
            config_dir(),
            Some(PathBuf::from("/home/testuser/.config/radar-workstation")),
            "a relative XDG_CONFIG_HOME is invalid per spec and must not be joined blindly"
        );
    }

    #[test]
    fn empty_xdg_var_is_ignored_in_favor_of_home_fallback() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = EnvGuard::capture(VARS);

        env::set_var("XDG_CONFIG_HOME", "");
        env::set_var("HOME", "/home/testuser");
        assert_eq!(config_dir(), Some(PathBuf::from("/home/testuser/.config/radar-workstation")));
    }

    #[test]
    fn no_home_and_no_xdg_var_degrades_to_none_not_a_panic() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = EnvGuard::capture(VARS);

        env::remove_var("XDG_CONFIG_HOME");
        env::remove_var("XDG_CACHE_HOME");
        env::remove_var("XDG_DATA_HOME");
        env::remove_var("HOME");

        assert_eq!(config_dir(), None);
        assert_eq!(cache_dir(), None);
        assert_eq!(data_dir(), None);
    }

    #[test]
    fn relative_home_with_no_xdg_var_degrades_to_none() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = EnvGuard::capture(VARS);

        env::remove_var("XDG_CONFIG_HOME");
        env::set_var("HOME", "relative/home");
        assert_eq!(config_dir(), None);
    }
}
