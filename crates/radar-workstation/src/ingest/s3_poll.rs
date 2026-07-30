use std::time::{Duration, SystemTime, UNIX_EPOCH};

use quick_xml::events::Event;
use quick_xml::Reader;
use tokio::sync::mpsc;

use crate::chunk::ChunkKind;

use super::ChunkEnvelope;

/// Hostname `Client::new` should be given to construct the `http_ingest::Client`
/// passed into `S3Poller::new`. Not used internally — `Client` already knows
/// its own host once constructed.
pub const BUCKET_HOST: &str = "unidata-nexrad-level2-chunks.s3.amazonaws.com";
const POLL_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum PollError {
    Http(http_ingest::Error),
    Xml(quick_xml::Error),
}

impl std::fmt::Display for PollError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP error: {e}"),
            Self::Xml(e) => write!(f, "XML parse error: {e}"),
        }
    }
}

impl std::error::Error for PollError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(e) => Some(e),
            Self::Xml(e) => Some(e),
        }
    }
}

pub struct S3Poller {
    site_id: String,
    client: http_ingest::Client,
    /// Synthetic or real S3 key used as `start-after` on the next list request.
    /// Initialized to the current-hour directory prefix so startup does not replay
    /// historical chunks from earlier in the hour.
    last_seen_key: String,
}

impl S3Poller {
    pub fn new(site_id: impl Into<String>, client: http_ingest::Client) -> Self {
        let site_id = site_id.into();
        let last_seen_key = current_hour_anchor(&site_id);
        Self { site_id, client, last_seen_key }
    }

    /// Runs the polling loop, sending each new chunk over `tx`.
    /// Returns when `tx` is closed (receiver dropped).
    /// Transient S3 errors are logged and skipped; the loop never exits on its own.
    pub async fn run(mut self, tx: mpsc::Sender<ChunkEnvelope>) {
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        loop {
            interval.tick().await;
            match self.poll_once().await {
                Ok(envelopes) => {
                    for envelope in envelopes {
                        if tx.send(envelope).await.is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    // TODO: replace with structured logging once a logging crate is added
                    eprintln!("[s3_poll] {e}");
                }
            }
        }
    }

    async fn poll_once(&mut self) -> Result<Vec<ChunkEnvelope>, PollError> {
        let prefix = format!("{}/", self.site_id);
        let last_seen_key = self.last_seen_key.clone();
        let keys = self.list_keys_after(&prefix, &last_seen_key).await?;

        if keys.is_empty() {
            return Ok(vec![]);
        }

        let mut to_fetch: Vec<(String, ChunkKind)> = Vec::with_capacity(keys.len());
        for key in &keys {
            match chunk_kind_from_key(key) {
                Some(kind) => to_fetch.push((key.clone(), kind)),
                None => eprintln!("[s3_poll] unrecognized key suffix, skipping: {key}"),
            }
        }

        // Fetched sequentially (ADR-0014: the http-ingest client serializes
        // requests on a single connection, no connection pool) and only
        // last_seen_key advances once every fetch in the batch has
        // succeeded, so that a transient fetch failure causes the whole
        // batch to be retried on the next poll rather than silently
        // skipping chunks whose keys were already passed. With sequential
        // fetching this falls out naturally: the first `?` aborts before
        // the assignment below.
        let mut envelopes = Vec::with_capacity(to_fetch.len());
        for (key, kind) in &to_fetch {
            let raw_bytes = self.client.get_object(key).await.map_err(PollError::Http)?;
            envelopes.push(ChunkEnvelope { kind: *kind, raw_bytes });
        }

        self.last_seen_key = keys.into_iter().next_back().expect("checked non-empty above");
        Ok(envelopes)
    }

    async fn list_keys_after(
        &mut self,
        prefix: &str,
        start_after: &str,
    ) -> Result<Vec<String>, PollError> {
        let mut all_keys = Vec::new();
        let mut continuation_token: Option<String> = None;
        let mut first_page = true;

        loop {
            // start-after is only valid on the first page; continuation-token
            // encodes position implicitly on subsequent pages and must not
            // be combined with it.
            let body = self
                .client
                .list_prefix(prefix, first_page.then_some(start_after), continuation_token.as_deref())
                .await
                .map_err(PollError::Http)?;

            let (page_keys, is_truncated, next_token) = parse_list_xml(&body)?;
            all_keys.extend(page_keys);

            if !is_truncated {
                break;
            }
            continuation_token = next_token;
            first_page = false;
        }

        Ok(all_keys)
    }
}

/// Which of the tags we care about the reader is currently inside. Tracked as
/// an enum rather than an owned tag-name `String` so that ignored tags (the
/// vast majority in a listing response) cost neither an allocation nor a copy.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ListTag {
    Key,
    IsTruncated,
    NextContinuationToken,
    Other,
}

fn parse_list_xml(body: &[u8]) -> Result<(Vec<String>, bool, Option<String>), PollError> {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    let mut is_truncated = false;
    let mut next_token: Option<String> = None;
    let mut in_tag = ListTag::Other;

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).map_err(PollError::Xml)? {
            Event::Start(e) => {
                in_tag = match e.name().as_ref() {
                    b"Key" => ListTag::Key,
                    b"IsTruncated" => ListTag::IsTruncated,
                    b"NextContinuationToken" => ListTag::NextContinuationToken,
                    _ => ListTag::Other,
                };
            }
            // Only the three tags above ever need the text node materialized;
            // every other tag's text (Name, Size, LastModified, ...) is skipped
            // without allocating.
            Event::Text(e) => match in_tag {
                ListTag::Key => keys.push(e.unescape().map_err(PollError::Xml)?.into_owned()),
                ListTag::IsTruncated => {
                    is_truncated = e.unescape().map_err(PollError::Xml)?.as_ref() == "true";
                }
                ListTag::NextContinuationToken => {
                    next_token = Some(e.unescape().map_err(PollError::Xml)?.into_owned());
                }
                ListTag::Other => {}
            },
            Event::End(_) => in_tag = ListTag::Other,
            Event::Eof => break,
            _ => {}
        }
    }

    Ok((keys, is_truncated, next_token))
}

fn chunk_kind_from_key(key: &str) -> Option<ChunkKind> {
    match key.as_bytes().last()? {
        b'S' => Some(ChunkKind::Start),
        b'I' => Some(ChunkKind::Intermediate),
        b'E' => Some(ChunkKind::End),
        _ => None,
    }
}

/// Returns a synthetic S3 key that sorts before all real keys in the current UTC hour.
/// Used as `start-after` on startup to anchor the stream to live data without
/// replaying earlier chunks from the same hour.
fn current_hour_anchor(site_id: &str) -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (year, month, day, hour) = unix_to_utc_parts(secs);
    format!("{site_id}/{year:04}/{month:02}/{day:02}/{hour:02}/")
}

/// Converts a Unix timestamp (seconds since 1970-01-01 00:00:00 UTC) to
/// (year, month, day, hour) using Howard Hinnant's Gregorian calendar algorithm.
fn unix_to_utc_parts(secs: u64) -> (u32, u32, u32, u32) {
    let days = (secs / 86400) as u32;
    let hour = ((secs % 86400) / 3600) as u32;

    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    (y, m, d, hour)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_kind_from_known_suffixes() {
        assert_eq!(chunk_kind_from_key("KDOX/2026/07/02/00/KDOX_20260702_000248_S"), Some(ChunkKind::Start));
        assert_eq!(chunk_kind_from_key("KDOX/2026/07/02/00/KDOX_20260702_000300_I"), Some(ChunkKind::Intermediate));
        assert_eq!(chunk_kind_from_key("KDOX/2026/07/02/00/KDOX_20260702_000600_E"), Some(ChunkKind::End));
        assert_eq!(chunk_kind_from_key("KDOX/2026/07/02/00/KDOX_20260702_000600_X"), None);
        assert_eq!(chunk_kind_from_key(""), None);
    }

    #[test]
    fn hour_anchor_sorts_before_real_keys() {
        let anchor = current_hour_anchor("KDOX");
        // A real key for the same hour must sort after the bare prefix
        let real_key = format!("{}_KDOX_20260702_000000_S", &anchor);
        assert!(anchor < real_key);
    }

    #[test]
    fn unix_to_utc_known_values() {
        // 2026-07-02 00:00:00 UTC = 1782950400
        let (y, mo, d, h) = unix_to_utc_parts(1_782_950_400);
        assert_eq!((y, mo, d, h), (2026, 7, 2, 0));

        // 2026-07-02 13:45:00 UTC
        let (y, mo, d, h) = unix_to_utc_parts(1_782_950_400 + 13 * 3600 + 45 * 60);
        assert_eq!((y, mo, d, h), (2026, 7, 2, 13));
    }

    #[test]
    fn parse_list_xml_extracts_keys() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Name>unidata-nexrad-level2-chunks</Name>
  <Prefix>KDOX/</Prefix>
  <KeyCount>2</KeyCount>
  <MaxKeys>1000</MaxKeys>
  <IsTruncated>false</IsTruncated>
  <Contents>
    <Key>KDOX/2026/07/02/00/KDOX_20260702_000248_S</Key>
    <Size>12345</Size>
  </Contents>
  <Contents>
    <Key>KDOX/2026/07/02/00/KDOX_20260702_000300_I</Key>
    <Size>98765</Size>
  </Contents>
</ListBucketResult>"#;

        let (keys, is_truncated, next_token) = parse_list_xml(xml).unwrap();
        assert_eq!(keys, vec![
            "KDOX/2026/07/02/00/KDOX_20260702_000248_S",
            "KDOX/2026/07/02/00/KDOX_20260702_000300_I",
        ]);
        assert!(!is_truncated);
        assert!(next_token.is_none());
    }

    #[test]
    fn parse_list_xml_handles_truncated() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <IsTruncated>true</IsTruncated>
  <NextContinuationToken>abc123==</NextContinuationToken>
  <Contents>
    <Key>KDOX/2026/07/02/00/KDOX_20260702_000248_S</Key>
    <Size>1</Size>
  </Contents>
</ListBucketResult>"#;

        let (keys, is_truncated, next_token) = parse_list_xml(xml).unwrap();
        assert_eq!(keys.len(), 1);
        assert!(is_truncated);
        assert_eq!(next_token.as_deref(), Some("abc123=="));
    }
}
