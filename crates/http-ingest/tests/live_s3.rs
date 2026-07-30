//! Live validation against real S3 (ADR-0014 Phase 3). All tests here are
//! `#[ignore]`d — they are not part of default CI. Run with:
//!
//!   cargo test -p http-ingest -- --ignored
//!
//! `http-ingest` deliberately has no knowledge of the NEXRAD chunk format
//! (that's `radar-workstation`'s concern, per ADR-0014's scope boundaries),
//! so these tests check only transport-level properties: non-empty XML with
//! at least one key, non-empty object bytes, and successful keepalive reuse.

use http_ingest::Client;

const CHUNKS_HOST: &str = "unidata-nexrad-level2-chunks.s3.amazonaws.com";
const ARCHIVE_HOST: &str = "unidata-nexrad-level2.s3.amazonaws.com";

/// Minimal, allocation-light scrape for a single `<Tag>value</Tag>` — good
/// enough for a live test that doesn't want an XML parser dependency.
fn first_tag(xml: &[u8], tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let text = std::str::from_utf8(xml).ok()?;
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(text[start..end].to_string())
}

#[tokio::test]
#[ignore]
async fn list_prefix_against_chunks_bucket_returns_at_least_one_key() {
    let mut client = Client::new(CHUNKS_HOST).unwrap();
    let body = client.list_prefix("KDOX/", None, None).await.unwrap();
    assert!(first_tag(&body, "Key").is_some(), "expected at least one <Key> in {body:?}");
}

#[tokio::test]
#[ignore]
async fn get_object_on_first_listed_key_returns_bytes() {
    let mut client = Client::new(CHUNKS_HOST).unwrap();
    let listing = client.list_prefix("KDOX/", None, None).await.unwrap();
    let key = first_tag(&listing, "Key").expect("listing should contain at least one key");

    let bytes = client.get_object(&key).await.unwrap();
    assert!(!bytes.is_empty(), "object body for {key} should not be empty");
}

#[tokio::test]
#[ignore]
async fn two_sequential_get_object_calls_succeed_over_one_connection() {
    let mut client = Client::new(CHUNKS_HOST).unwrap();
    let listing = client.list_prefix("KDOX/", None, None).await.unwrap();
    let key = first_tag(&listing, "Key").expect("listing should contain at least one key");

    let first = client.get_object(&key).await.unwrap();
    let second = client.get_object(&key).await.unwrap();
    assert_eq!(first.as_ref(), second.as_ref(), "fetching the same key twice should be idempotent");
}

/// The one test that can catch an encoding bug the unit tests would agree
/// with: a real S3 continuation token exercises `=`, `+`, `/` against the
/// live service, not just our own parser's understanding of RFC 3986.
#[tokio::test]
#[ignore]
async fn list_prefix_with_continuation_token_from_a_truncated_page_succeeds() {
    let mut client = Client::new(CHUNKS_HOST).unwrap();
    let first_page = client.list_prefix("KDOX/", None, None).await.unwrap();

    let is_truncated = first_tag(&first_page, "IsTruncated").as_deref() == Some("true");
    assert!(is_truncated, "expected the unfiltered KDOX/ prefix to be truncated (>1000 keys)");
    let token = first_tag(&first_page, "NextContinuationToken")
        .expect("truncated listing should carry a NextContinuationToken");

    let second_page = client.list_prefix("KDOX/", None, Some(&token)).await.unwrap();
    assert!(first_tag(&second_page, "Key").is_some(), "second page should contain keys too");
}

#[tokio::test]
#[ignore]
async fn archive_bucket_answers_both_request_shapes() {
    // The archive bucket's key layout is `YYYY/MM/DD/SITE/...` — the reverse
    // of the chunks bucket's `SITE/YYYY/MM/DD/...` — confirmed live
    // (2026-07-29). Worth carrying forward to whatever `ChunkSource`
    // eventually consumes this bucket (ADR-0014 open question 2).
    let mut client = Client::new(ARCHIVE_HOST).unwrap();
    let listing = client.list_prefix("2026/07/29/KDOX/", None, None).await.unwrap();
    let key = first_tag(&listing, "Key").expect("archive listing should contain at least one key");

    let bytes = client.get_object(&key).await.unwrap();
    assert!(!bytes.is_empty(), "archive object body for {key} should not be empty");
}
