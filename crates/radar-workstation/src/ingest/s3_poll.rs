use std::time::Duration;

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

/// The chunk bucket keys objects as `SITE/<volume-sequence>/<timestamp>-<n>-<kind>`
/// (confirmed live 2026-07-31 — see `docs/plans/dependency-inventory-remediation.md`,
/// W1 Results; corrects an earlier assumption of a `SITE/YYYY/MM/DD/HH/...` calendar
/// layout). `<volume-sequence>` is an unpadded, monotonically increasing integer
/// scoped to the site, incrementing once per volume scan; it is not derivable from
/// wall-clock time. Its lexical order does **not** match numeric order across digit
/// widths (`"78"` sorts after `"709"`), so it cannot be used as a flat `start-after`
/// anchor the way a zero-padded or timestamp-based key could. `S3Poller` therefore
/// tracks position as a volume-sequence number, discovered by listing the bucket's
/// first-level `CommonPrefixes` (`list_volume_folders`), and only uses lexical
/// `start-after` ordering *within* one volume's directory, where the fixed-width
/// `<timestamp>-<n>-<kind>` names do sort chronologically.
pub struct S3Poller {
    site_id: String,
    client: http_ingest::Client,
    /// Volume-sequence number of the last volume whose `-E` chunk has been seen
    /// (i.e. fully delivered). `None` only before the first poll has run.
    last_completed_volume: Option<u64>,
    /// `start-after` key within the volume currently being drained
    /// (`last_completed_volume + 1`). Reset to `None` whenever that volume
    /// completes and the poller moves on to the next one.
    last_seen_key: Option<String>,
}

impl S3Poller {
    pub fn new(site_id: impl Into<String>, client: http_ingest::Client) -> Self {
        Self { site_id: site_id.into(), client, last_completed_volume: None, last_seen_key: None }
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

        let baseline = match self.last_completed_volume {
            Some(v) => v,
            None => {
                let folders = self.list_volume_folders(&prefix).await?;
                // Cold start: anchor one behind the newest known volume, so the
                // very next poll fetches the current volume rather than
                // replaying the full 24-hour retention window (E-10).
                cold_start_baseline(&folders)
            }
        };
        let volume = baseline + 1;
        let volume_prefix = format!("{prefix}{volume}/");

        let start_after = self.last_seen_key.clone();
        let (keys, _pages) =
            self.list_all_keys_after(&volume_prefix, start_after.as_deref()).await?;

        if keys.is_empty() {
            self.last_completed_volume = Some(baseline);
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
        // requests on a single connection, no connection pool). last_seen_key
        // and last_completed_volume only advance once every fetch in the
        // batch has succeeded, so a transient fetch failure causes the whole
        // batch to be retried on the next poll rather than silently skipping
        // chunks whose keys were already passed. With sequential fetching
        // this falls out naturally: the first `?` aborts before either
        // assignment below.
        let mut envelopes = Vec::with_capacity(to_fetch.len());
        let mut saw_end = false;
        for (key, kind) in &to_fetch {
            let raw_bytes = self.client.get_object(key).await.map_err(PollError::Http)?;
            saw_end |= *kind == ChunkKind::End;
            envelopes.push(ChunkEnvelope { kind: *kind, raw_bytes });
        }

        self.last_seen_key = keys.into_iter().next_back();
        if saw_end {
            // Volume complete: move on to the next volume-sequence directory
            // next poll. Known gap, not fixed here (mirrors the pre-existing
            // permanently-404ing-key issue): if `volume + 1` never appears —
            // e.g. an RDA restart skips sequence numbers, as observed live —
            // the poller stalls waiting on a directory that will never exist.
            self.last_completed_volume = Some(volume);
            self.last_seen_key = None;
        } else {
            self.last_completed_volume = Some(baseline);
        }

        Ok(envelopes)
    }

    /// Lists the numeric volume-sequence subdirectories directly under `prefix`
    /// (e.g. `KDOX/166/`, `KDOX/167/`), using S3's `delimiter=/` so the response
    /// is `CommonPrefixes` rather than a flat, potentially enormous object listing.
    async fn list_volume_folders(&mut self, prefix: &str) -> Result<Vec<u64>, PollError> {
        let mut folders = Vec::new();
        let mut continuation_token: Option<String> = None;

        loop {
            let body = self
                .client
                .list_prefix(prefix, None, continuation_token.as_deref(), Some("/"))
                .await
                .map_err(PollError::Http)?;

            let page = parse_list_xml(&body)?;
            for common_prefix in &page.common_prefixes {
                match parse_volume_folder(prefix, common_prefix) {
                    Some(volume) => folders.push(volume),
                    None => {
                        eprintln!("[s3_poll] unrecognized volume folder, skipping: {common_prefix}")
                    }
                }
            }

            if !page.is_truncated {
                break;
            }
            continuation_token = page.next_token;
        }

        Ok(folders)
    }

    /// Lists every key under `prefix` (one volume-sequence directory) that sorts
    /// after `start_after`, paging via continuation token. `None` lists the whole
    /// directory. Keys inside one volume directory use a fixed-width timestamp
    /// plus zero-padded sequence number, so lexical order matches chronological
    /// order here — unlike the volume-sequence directory names themselves
    /// (see `list_volume_folders`).
    ///
    /// Returns the collected keys plus the number of S3 list pages fetched — the
    /// page count is measurement instrumentation for the W1 cold-start timing
    /// harness (see `mod tests`), not needed by `poll_once` itself.
    async fn list_all_keys_after(
        &mut self,
        prefix: &str,
        start_after: Option<&str>,
    ) -> Result<(Vec<String>, usize), PollError> {
        let mut all_keys = Vec::new();
        let mut continuation_token: Option<String> = None;
        let mut first_page = true;
        let mut pages = 0usize;

        loop {
            // start-after is only valid on the first page; continuation-token
            // encodes position implicitly on subsequent pages and must not
            // be combined with it.
            let body = self
                .client
                .list_prefix(
                    prefix,
                    if first_page { start_after } else { None },
                    continuation_token.as_deref(),
                    None,
                )
                .await
                .map_err(PollError::Http)?;
            pages += 1;

            let page = parse_list_xml(&body)?;
            all_keys.extend(page.keys);

            if !page.is_truncated {
                break;
            }
            continuation_token = page.next_token;
            first_page = false;
        }

        Ok((all_keys, pages))
    }
}

/// Cold-start anchor: given the full current set of volume-sequence numbers,
/// returns the baseline that should seed `last_completed_volume` — one behind
/// the newest, so the very next poll fetches exactly the newest (current or
/// just-completed) volume rather than replaying the full retention window
/// (E-10). Pure function so it is testable offline; `folders` empty (brand
/// new site, no data yet) yields `0`, so the poller waits on volume `1`.
fn cold_start_baseline(folders: &[u64]) -> u64 {
    folders.iter().copied().max().unwrap_or(0).saturating_sub(1)
}

/// Parses the numeric volume-sequence directory name out of a `CommonPrefixes`
/// entry like `KDOX/166/`, given the enclosing `KDOX/` prefix. `None` for
/// anything that isn't a plain unsigned integer directory — defensive, since
/// the bucket's exact layout is observed behavior, not a documented contract.
fn parse_volume_folder(prefix: &str, common_prefix: &str) -> Option<u64> {
    common_prefix.strip_prefix(prefix)?.strip_suffix('/')?.parse().ok()
}

/// Which of the tags we care about the reader is currently inside. Tracked as
/// an enum rather than an owned tag-name `String` so that ignored tags (the
/// vast majority in a listing response) cost neither an allocation nor a copy.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ListTag {
    Key,
    IsTruncated,
    NextContinuationToken,
    /// A `<Prefix>` nested inside `<CommonPrefixes>`. Distinct from the
    /// top-level `<Prefix>` (which just echoes the request's `prefix` query
    /// param and must not be captured) — see `in_common_prefixes` below.
    CommonPrefix,
    Other,
}

struct ListPage {
    keys: Vec<String>,
    common_prefixes: Vec<String>,
    is_truncated: bool,
    next_token: Option<String>,
}

/// Resolves a `quick-xml` 0.41+ `Event::GeneralRef` (`&entity;`) to its character.
/// Numeric references (`&#38;`, `&#x26;`) are handled by `BytesRef::resolve_char_ref`;
/// XML defines exactly five built-in named entities (there is no DTD here to define
/// more), so anything else is a genuine, rejected framing violation rather than
/// something to guess at.
fn resolve_general_ref(bytes_ref: &quick_xml::events::BytesRef) -> Result<char, quick_xml::Error> {
    if let Some(ch) = bytes_ref.resolve_char_ref()? {
        return Ok(ch);
    }
    match bytes_ref.decode()?.as_ref() {
        "amp" => Ok('&'),
        "lt" => Ok('<'),
        "gt" => Ok('>'),
        "apos" => Ok('\''),
        "quot" => Ok('"'),
        other => Err(quick_xml::escape::EscapeError::UnrecognizedEntity(0..0, other.to_string()).into()),
    }
}

fn parse_list_xml(body: &[u8]) -> Result<ListPage, PollError> {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    let mut common_prefixes: Vec<String> = Vec::new();
    let mut is_truncated = false;
    let mut next_token: Option<String> = None;
    let mut in_tag = ListTag::Other;
    let mut in_common_prefixes = false;
    // Accumulates one element's text across possibly several Text/GeneralRef
    // events — quick-xml 0.41 emits entity references (`&amp;` etc.) as
    // separate GeneralRef events rather than pre-expanding them inline, so a
    // single element's content can no longer be assumed to arrive as one
    // Text event.
    let mut current_text = String::new();

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).map_err(PollError::Xml)? {
            Event::Start(e) => {
                current_text.clear();
                in_tag = match e.name().as_ref() {
                    b"CommonPrefixes" => {
                        in_common_prefixes = true;
                        ListTag::Other
                    }
                    b"Key" => ListTag::Key,
                    b"IsTruncated" => ListTag::IsTruncated,
                    b"NextContinuationToken" => ListTag::NextContinuationToken,
                    b"Prefix" if in_common_prefixes => ListTag::CommonPrefix,
                    _ => ListTag::Other,
                };
            }
            // Only the tags above ever need the text node materialized; every
            // other tag's text (Name, Size, LastModified, ...) is skipped
            // without allocating.
            Event::Text(e) if in_tag != ListTag::Other => {
                current_text.push_str(&e.decode().map_err(|err| PollError::Xml(err.into()))?);
            }
            Event::GeneralRef(e) if in_tag != ListTag::Other => {
                current_text.push(resolve_general_ref(&e).map_err(PollError::Xml)?);
            }
            Event::End(e) => {
                match in_tag {
                    ListTag::Key => keys.push(std::mem::take(&mut current_text)),
                    ListTag::CommonPrefix => common_prefixes.push(std::mem::take(&mut current_text)),
                    ListTag::IsTruncated => is_truncated = current_text == "true",
                    ListTag::NextContinuationToken => {
                        next_token = Some(std::mem::take(&mut current_text))
                    }
                    ListTag::Other => {}
                }
                if e.name().as_ref() == b"CommonPrefixes" {
                    in_common_prefixes = false;
                }
                in_tag = ListTag::Other;
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(ListPage { keys, common_prefixes, is_truncated, next_token })
}

fn chunk_kind_from_key(key: &str) -> Option<ChunkKind> {
    match key.as_bytes().last()? {
        b'S' => Some(ChunkKind::Start),
        b'I' => Some(ChunkKind::Intermediate),
        b'E' => Some(ChunkKind::End),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn chunk_kind_from_known_suffixes() {
        assert_eq!(chunk_kind_from_key("KDOX/166/20260728-095259-001-S"), Some(ChunkKind::Start));
        assert_eq!(chunk_kind_from_key("KDOX/166/20260728-095259-002-I"), Some(ChunkKind::Intermediate));
        assert_eq!(chunk_kind_from_key("KDOX/166/20260728-095259-079-E"), Some(ChunkKind::End));
        assert_eq!(chunk_kind_from_key("KDOX/166/20260728-095259-079-X"), None);
        assert_eq!(chunk_kind_from_key(""), None);
    }

    #[test]
    fn cold_start_baseline_of_empty_set_is_zero() {
        assert_eq!(cold_start_baseline(&[]), 0);
    }

    #[test]
    fn cold_start_baseline_is_one_behind_the_newest() {
        assert_eq!(cold_start_baseline(&[5]), 4);
        assert_eq!(cold_start_baseline(&[3, 9, 7]), 8);
        // Unpadded lexical order would put "78" after "709"; the baseline
        // must be computed numerically, not from listing order.
        assert_eq!(cold_start_baseline(&[709, 78, 79]), 708);
    }

    #[test]
    fn cold_start_baseline_does_not_underflow_at_volume_one() {
        assert_eq!(cold_start_baseline(&[1]), 0);
    }

    #[test]
    fn parse_volume_folder_accepts_plain_integer_directories() {
        assert_eq!(parse_volume_folder("KDOX/", "KDOX/166/"), Some(166));
        assert_eq!(parse_volume_folder("KDOX/", "KDOX/1/"), Some(1));
    }

    #[test]
    fn parse_volume_folder_rejects_non_numeric_or_malformed_entries() {
        assert_eq!(parse_volume_folder("KDOX/", "KDOX/166"), None); // no trailing slash
        assert_eq!(parse_volume_folder("KDOX/", "KDOX/abc/"), None); // not an integer
        assert_eq!(parse_volume_folder("KDOX/", "OTHER/166/"), None); // wrong prefix
    }

    #[test]
    fn parse_list_xml_extracts_keys() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Name>unidata-nexrad-level2-chunks</Name>
  <Prefix>KDOX/166/</Prefix>
  <KeyCount>2</KeyCount>
  <MaxKeys>1000</MaxKeys>
  <IsTruncated>false</IsTruncated>
  <Contents>
    <Key>KDOX/166/20260728-095259-001-S</Key>
    <Size>12345</Size>
  </Contents>
  <Contents>
    <Key>KDOX/166/20260728-095259-002-I</Key>
    <Size>98765</Size>
  </Contents>
</ListBucketResult>"#;

        let page = parse_list_xml(xml).unwrap();
        assert_eq!(page.keys, vec![
            "KDOX/166/20260728-095259-001-S",
            "KDOX/166/20260728-095259-002-I",
        ]);
        assert!(page.common_prefixes.is_empty());
        assert!(!page.is_truncated);
        assert!(page.next_token.is_none());
    }

    #[test]
    fn parse_list_xml_handles_truncated() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <IsTruncated>true</IsTruncated>
  <NextContinuationToken>abc123==</NextContinuationToken>
  <Contents>
    <Key>KDOX/166/20260728-095259-001-S</Key>
    <Size>1</Size>
  </Contents>
</ListBucketResult>"#;

        let page = parse_list_xml(xml).unwrap();
        assert_eq!(page.keys.len(), 1);
        assert!(page.is_truncated);
        assert_eq!(page.next_token.as_deref(), Some("abc123=="));
    }

    #[test]
    fn parse_list_xml_extracts_common_prefixes_but_not_the_top_level_echo() {
        // The top-level <Prefix> just echoes the request's `prefix` query
        // param and must never be treated as a volume folder — only
        // <Prefix> tags nested inside <CommonPrefixes> are real folders.
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Name>unidata-nexrad-level2-chunks</Name>
  <Prefix>KDOX/</Prefix>
  <Delimiter>/</Delimiter>
  <IsTruncated>false</IsTruncated>
  <CommonPrefixes>
    <Prefix>KDOX/165/</Prefix>
  </CommonPrefixes>
  <CommonPrefixes>
    <Prefix>KDOX/166/</Prefix>
  </CommonPrefixes>
</ListBucketResult>"#;

        let page = parse_list_xml(xml).unwrap();
        assert!(page.keys.is_empty());
        assert_eq!(page.common_prefixes, vec!["KDOX/165/", "KDOX/166/"]);
    }

    #[test]
    fn parse_list_xml_resolves_entity_references_split_across_events() {
        // quick-xml 0.41 emits `&amp;`, `&#38;` etc. as separate GeneralRef
        // events rather than pre-expanding them inline within Text — a key
        // containing one of these arrives as multiple events that must be
        // reassembled, not just the first Text event's content.
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Contents>
    <Key>KDOX/166/a&amp;b&#38;c&#x26;d</Key>
    <Size>1</Size>
  </Contents>
</ListBucketResult>"#;

        let page = parse_list_xml(xml).unwrap();
        assert_eq!(page.keys, vec!["KDOX/166/a&b&c&d"]);
    }

    #[test]
    fn parse_list_xml_rejects_unrecognized_entity() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Contents>
    <Key>KDOX/166/a&nbsp;b</Key>
  </Contents>
</ListBucketResult>"#;

        assert!(parse_list_xml(xml).is_err());
    }

    // W1 cold-start acquisition latency (E-03 + E-10). Live-network timing
    // tests, `#[ignore]`d so `cargo test` never dials out. Run with:
    //
    //   cargo test -p radar-workstation -- --ignored --nocapture
    //
    // Each test prints one structured line so results can be pasted into
    // docs/plans/dependency-inventory-remediation.md's Results section. Kept
    // as unit tests in this module (not `tests/`) because `poll_once`,
    // `list_all_keys_after`, and `list_volume_folders` are private —
    // measuring through a public seam would mean re-implementing this logic,
    // which would violate DRY and measure a copy of the code rather than
    // the code itself. Same rationale as `http-ingest`'s wire tests living
    // in `src/` (ADR-0014 erratum item 5).

    const LIVE_TEST_SITE: &str = "KDOX";

    #[tokio::test]
    #[ignore]
    async fn cold_start_listing_size() {
        let client = http_ingest::Client::new(BUCKET_HOST).expect("client");
        let mut poller = S3Poller::new(LIVE_TEST_SITE, client);
        let prefix = format!("{}/", poller.site_id);

        let start = Instant::now();
        let folders = poller.list_volume_folders(&prefix).await.expect("list folders");
        let t_list = start.elapsed();

        let newest = folders.iter().copied().max();
        println!(
            "cold_start_listing_size: volume_folders={} newest_volume={newest:?} t_list={t_list:?}",
            folders.len()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn cold_start_poll_once_latency() {
        let client = http_ingest::Client::new(BUCKET_HOST).expect("client");
        let mut poller = S3Poller::new(LIVE_TEST_SITE, client);

        let total_start = Instant::now();
        let envelopes = poller.poll_once().await.expect("poll_once");
        let total = total_start.elapsed();

        println!(
            "cold_start_poll_once_latency: volume={:?} chunks={} total={total:?}",
            poller.last_completed_volume,
            envelopes.len()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn steady_state_poll_latency() {
        let client = http_ingest::Client::new(BUCKET_HOST).expect("client");
        let mut poller = S3Poller::new(LIVE_TEST_SITE, client);

        let _first = poller.poll_once().await.expect("poll 1 (cold start)");

        tokio::time::sleep(POLL_INTERVAL).await;
        let start2 = Instant::now();
        let second = poller.poll_once().await.expect("poll 2");
        let t2 = start2.elapsed();

        tokio::time::sleep(POLL_INTERVAL).await;
        let start3 = Instant::now();
        let third = poller.poll_once().await.expect("poll 3");
        let t3 = start3.elapsed();

        println!(
            "steady_state_poll_latency: poll2_chunks={} poll2_time={t2:?} poll3_chunks={} poll3_time={t3:?}",
            second.len(),
            third.len()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn keepalive_amortization() {
        let mut client = http_ingest::Client::new(BUCKET_HOST).expect("client");
        let listing = client
            .list_prefix(&format!("{LIVE_TEST_SITE}/"), None, None, None)
            .await
            .expect("list");
        let page = parse_list_xml(&listing).expect("parse");
        let key = page.keys.first().expect("at least one key in listing");

        let first_start = Instant::now();
        let _ = client.get_object(key).await.expect("first fetch");
        let first = first_start.elapsed();

        let mut subsequent = Vec::new();
        for _ in 0..4 {
            let start = Instant::now();
            let _ = client.get_object(key).await.expect("subsequent fetch");
            subsequent.push(start.elapsed());
        }

        println!("keepalive_amortization: first={first:?} subsequent={subsequent:?}");
    }
}
