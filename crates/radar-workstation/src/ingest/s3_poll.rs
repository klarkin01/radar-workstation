use std::time::{Duration, SystemTime, UNIX_EPOCH};

use quick_xml::events::Event;
use quick_xml::Reader;
use tokio::sync::mpsc;

use crate::chunk::ChunkKind;

use super::ChunkEnvelope;

const BUCKET_BASE: &str = "https://unidata-nexrad-level2-chunks.s3.amazonaws.com";
const POLL_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum PollError {
    Http(reqwest::Error),
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
    client: reqwest::Client,
    /// Synthetic or real S3 key used as `start-after` on the next list request.
    /// Initialized to the current-hour directory prefix so startup does not replay
    /// historical chunks from earlier in the hour.
    last_seen_key: String,
}

impl S3Poller {
    pub fn new(site_id: impl Into<String>, client: reqwest::Client) -> Self {
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
        let start_after = self.last_seen_key.clone();
        let keys = self.list_keys_after(&prefix, &start_after).await?;

        if keys.is_empty() {
            return Ok(vec![]);
        }

        // Fetch all chunks before advancing last_seen_key so that a transient fetch
        // failure causes the whole batch to be retried on the next poll rather than
        // silently skipping chunks whose keys were already passed.
        let mut envelopes = Vec::with_capacity(keys.len());
        for key in &keys {
            let Some(kind) = chunk_kind_from_key(key) else {
                eprintln!("[s3_poll] unrecognized key suffix, skipping: {key}");
                continue;
            };
            let raw_bytes = self.fetch_object(key).await?;
            envelopes.push(ChunkEnvelope { kind, raw_bytes });
        }

        self.last_seen_key = keys.last().unwrap().clone();
        Ok(envelopes)
    }

    async fn list_keys_after(
        &self,
        prefix: &str,
        start_after: &str,
    ) -> Result<Vec<String>, PollError> {
        let mut all_keys = Vec::new();
        let mut continuation_token: Option<String> = None;
        let mut first_page = true;

        loop {
            let mut params: Vec<(&str, String)> = vec![
                ("list-type", "2".to_owned()),
                ("prefix", prefix.to_owned()),
            ];
            // start-after is only valid on the first page; continuation-token encodes
            // position implicitly on subsequent pages and must not be combined with it.
            if first_page {
                params.push(("start-after", start_after.to_owned()));
            }
            if let Some(ref token) = continuation_token {
                params.push(("continuation-token", token.clone()));
            }

            let body = self
                .client
                .get(format!("{BUCKET_BASE}/"))
                .query(&params)
                .send()
                .await
                .map_err(PollError::Http)?
                .error_for_status()
                .map_err(PollError::Http)?
                .bytes()
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

    async fn fetch_object(&self, key: &str) -> Result<Vec<u8>, PollError> {
        let bytes = self
            .client
            .get(format!("{BUCKET_BASE}/{key}"))
            .send()
            .await
            .map_err(PollError::Http)?
            .error_for_status()
            .map_err(PollError::Http)?
            .bytes()
            .await
            .map_err(PollError::Http)?;
        Ok(bytes.to_vec())
    }
}

fn parse_list_xml(body: &[u8]) -> Result<(Vec<String>, bool, Option<String>), PollError> {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    let mut is_truncated = false;
    let mut next_token: Option<String> = None;
    let mut in_tag: Option<String> = None;

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).map_err(PollError::Xml)? {
            Event::Start(e) => {
                in_tag = Some(String::from_utf8_lossy(e.name().as_ref()).into_owned());
            }
            Event::Text(e) => {
                let text = e.unescape().map_err(PollError::Xml)?.into_owned();
                match in_tag.as_deref() {
                    Some("Key") => keys.push(text),
                    Some("IsTruncated") => is_truncated = text == "true",
                    Some("NextContinuationToken") => next_token = Some(text),
                    _ => {}
                }
            }
            Event::End(_) => in_tag = None,
            Event::Eof => break,
            _ => {}
        }
    }

    Ok((keys, is_truncated, next_token))
}

fn chunk_kind_from_key(key: &str) -> Option<ChunkKind> {
    match key.chars().last()? {
        'S' => Some(ChunkKind::Start),
        'I' => Some(ChunkKind::Intermediate),
        'E' => Some(ChunkKind::End),
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
