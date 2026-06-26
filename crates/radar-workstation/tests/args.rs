use radar_workstation::resolve_sample_url;
use std::env;
use std::sync::{Mutex, OnceLock};

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn resolve_sample_url_reads_cli_flag_value() {
    let args = ["program", "--sample-url", "https://example.com/radar"];

    assert_eq!(
        resolve_sample_url(args),
        Some("https://example.com/radar".to_string())
    );
}

#[test]
fn resolve_sample_url_reads_environment_variable_when_cli_is_missing() {
    let _guard = env_lock().lock().expect("env lock should be available");
    let previous = env::var("RADAR_SAMPLE_URL").ok();
    env::remove_var("RADAR_SAMPLE_URL");
    assert_eq!(resolve_sample_url(["program"]), None);

    env::set_var("RADAR_SAMPLE_URL", "https://example.com/env-radar");
    assert_eq!(
        resolve_sample_url(["program"]),
        Some("https://example.com/env-radar".to_string())
    );

    match previous {
        Some(value) => env::set_var("RADAR_SAMPLE_URL", value),
        None => env::remove_var("RADAR_SAMPLE_URL"),
    }
}
