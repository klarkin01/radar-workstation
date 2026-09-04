//! The old plaintext-loopback tests here relied on `reqwest` accepting plain
//! `http://127.0.0.1:PORT`. The `http-ingest`-backed client is HTTPS-only
//! with a compile-time host allowlist (ADR-0014), so three of the four
//! original tests can no longer be driven against a local server; they
//! become `#[ignore]`d live tests against real S3 instead. This is a real
//! coverage reduction for a `utility/`-only crate (dev-only per CLAUDE.md),
//! traded for the security properties the allowlist buys everywhere else —
//! see docs/plans/0014-http-ingest-implementation.md §7.3.

use nexrad_sample::{download_sample, split_s3_url, AcquisitionError};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn create_output_path(name: &str) -> (PathBuf, tempfile::TempDir) {
    let temp_dir = tempdir().expect("temp dir should be created");
    let output_path = temp_dir.path().join(name);
    (output_path, temp_dir)
}

fn assert_output_file_does_not_exist(output_path: &Path) {
    assert!(!output_path.exists(), "output file should not be created on failure");
}

#[tokio::test]
#[ignore]
async fn download_sample_writes_a_file_for_a_successful_response() {
    let url = "https://unidata-nexrad-level2-chunks.s3.amazonaws.com/KDOX/1/20260727-164425-001-S";
    let (output_path, _temp_dir) = create_output_path("sample.bin");

    let result = download_sample(url, &output_path).await;

    assert!(result.is_ok(), "download should succeed for a real key: {result:?}");
    assert_eq!(result.unwrap(), output_path);
    assert!(output_path.exists());
    assert!(!fs::read(&output_path).expect("file should be readable").is_empty());
}

#[tokio::test]
#[ignore]
async fn download_sample_returns_an_error_for_a_non_success_status() {
    let url = "https://unidata-nexrad-level2-chunks.s3.amazonaws.com/KDOX/does-not-exist";
    let (output_path, _temp_dir) = create_output_path("missing.bin");

    let result = download_sample(url, &output_path).await;

    assert_eq!(result.unwrap_err(), AcquisitionError::BadStatusCode(404));
    assert_output_file_does_not_exist(&output_path);
}

#[test]
fn download_sample_returns_an_error_for_invalid_urls() {
    assert!(matches!(split_s3_url("not a valid url"), Err(AcquisitionError::InvalidUrl(_))));

    // http:// gets its own distinct message rather than falling through to
    // the generic "missing scheme" case.
    assert!(matches!(
        split_s3_url("http://unidata-nexrad-level2-chunks.s3.amazonaws.com/key"),
        Err(AcquisitionError::InvalidUrl(_))
    ));

    // Allowlisted host, but no key — a syntax failure, not a network call.
    assert!(matches!(
        split_s3_url("https://unidata-nexrad-level2-chunks.s3.amazonaws.com/"),
        Err(AcquisitionError::InvalidUrl(_))
    ));

    // A non-allowlisted host passes syntax (split_s3_url doesn't check the
    // allowlist) but is rejected once `Bucket::from_host` sees it — the
    // successor to the old `Client::new(host)` rejection now that `S3Client`
    // takes a `Bucket`, not a hostname (ADR-0026 §2).
    let (host, _key) = split_s3_url("https://evil.com/key").unwrap();
    assert_eq!(http_ingest::Bucket::from_host(host), None);
}
