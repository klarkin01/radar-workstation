use std::time::{Duration, Instant};

use quick_xml::events::Event;
use quick_xml::Reader;
use tokio::sync::{mpsc, watch};

use crate::chunk::ChunkKind;
use crate::state::AppState;

use super::volume_seq::{self, VolumeSeq, VolumeWindow};
use super::ChunkEnvelope;

pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Empty polls, with no key ever seen for the current target volume,
/// before re-listing volume folders to check for a skipped sequence
/// number (E-observed live: 79→90, 92→165, 195→268). 12 polls × 5s poll
/// interval ≈ 60s — a normal gap between chunks within a volume is a
/// handful of empty polls; a genuinely absent directory produces an
/// unbounded run of them.
const REANCHOR_EMPTY_POLLS: u32 = 12;
/// Empty polls, after real data was seen for the current target volume,
/// before giving up on it and advancing to the next one. 60 polls × 5s ≈
/// 5 minutes — longer than the re-anchor threshold because real data
/// having arrived means this is very likely the volume's actual final
/// chunk being late or lost, not an absent directory, and a false-positive
/// here abandons in-progress data. The assembly layer's own watchdog (not
/// this poller) is what eventually marks that volume `TimedOut`; this
/// threshold only stops the poller from polling a directory forever.
const STUCK_MID_VOLUME_EMPTY_POLLS: u32 = 60;
/// Consecutive `poll_once` failures (not empty polls — actual errors: HTTP,
/// connection, XML parsing) before the published status escalates from
/// `Retrying` to `Stalled`. 5 × the 5s poll interval ≈ 25s of unbroken
/// failure — `http-ingest` already retries once at the connection layer
/// (ADR-0014), so a poller-level failure here means that retry also
/// failed; this threshold is about repeated failures over time, not a
/// single dropped connection, and does not add a second retry loop on top
/// of the client's.
const STALLED_AFTER_CONSECUTIVE_ERRORS: u32 = 5;

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

/// A typed classification of what went wrong, for a status bar to key off
/// of directly rather than parsing a formatted string. Deliberately
/// coarser than `PollError`/`http_ingest::Error` — just the distinctions a
/// UI plausibly cares about ("no network" vs. "S3 returned an error status"
/// vs. "the listing response itself was malformed").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestErrorKind {
    /// Connect, TLS, timeout, or an unexpected connection close.
    Network,
    /// An HTTP-level error status from S3 itself.
    Http { status: u16 },
    /// The XML listing response didn't parse.
    MalformedListing,
    /// A framing violation, oversized body, or other protocol-level issue
    /// that isn't simply "no network."
    Protocol,
}

impl From<&PollError> for IngestErrorKind {
    fn from(e: &PollError) -> Self {
        match e {
            PollError::Http(http_ingest::Error::Http { status }) => Self::Http { status: *status },
            PollError::Http(
                http_ingest::Error::Connect(_)
                | http_ingest::Error::Tls(_)
                | http_ingest::Error::Timeout { .. }
                | http_ingest::Error::Closed,
            ) => Self::Network,
            PollError::Http(_) => Self::Protocol,
            PollError::Xml(_) => Self::MalformedListing,
        }
    }
}

/// The poller's current health, as a `watch` channel value (S1-W3b, FR-DA-5).
/// `watch` rather than `mpsc`: a status bar wants the *latest* status every
/// frame, not a queue of every transition, and a `watch` receiver cannot
/// apply backpressure to the poller if nothing is reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestState {
    Polling,
    /// `attempts` consecutive `poll_once` failures so far, below the
    /// `Stalled` threshold.
    Retrying { attempts: u32 },
    /// `poll_once` has failed `STALLED_AFTER_CONSECUTIVE_ERRORS` times in a
    /// row.
    Stalled,
    /// `apply_recovery` (S1-W3a) is re-anchoring or advancing past a
    /// stalled volume this poll.
    ReAnchoring,
}

#[derive(Debug, Clone)]
pub struct IngestStatus {
    pub state: IngestState,
    pub last_success: Option<Instant>,
    /// Typed, not a formatted string — see `IngestErrorKind`.
    pub last_error: Option<IngestErrorKind>,
    /// The last volume-sequence number whose `-E` chunk was seen (i.e. fully
    /// delivered). `None` until the first volume completes — no longer a `0`
    /// sentinel, and no longer republished as `new_target - 1` on a re-anchor
    /// (plan §2.3).
    pub current_volume: Option<VolumeSeq>,
}

impl Default for IngestStatus {
    fn default() -> Self {
        Self {
            state: IngestState::Polling,
            last_success: None,
            last_error: None,
            current_volume: None,
        }
    }
}

/// The chunk bucket keys objects as `SITE/<volume-sequence>/<timestamp>-<n>-<kind>`
/// (confirmed live 2026-07-31 — see `docs/plans/dependency-inventory-remediation.md`,
/// W1 Results; corrects an earlier assumption of a `SITE/YYYY/MM/DD/HH/...` calendar
/// layout). `<volume-sequence>` is an unpadded **cyclic counter over `1..=999`**,
/// scoped to the site, incrementing once per volume scan and rolling `999 → 1`; it
/// is *not* monotonically increasing (the wrong assumption was written here first
/// and then coded to — plan §1, parent A1) and is not derivable from wall-clock
/// time. See [`super::volume_seq`]'s module doc for the measurement. Its lexical
/// order does **not** match numeric order across digit widths (`"78"` sorts after
/// `"709"`), so it cannot be used as a flat `start-after` anchor the way a
/// zero-padded or timestamp-based key could. `S3Poller` therefore tracks position
/// as a [`VolumeSeq`], discovered by listing the bucket's first-level
/// `CommonPrefixes` (`list_volume_folders`), and only uses lexical `start-after`
/// ordering *within* one volume's directory, where the fixed-width
/// `<timestamp>-<n>-<kind>` names do sort chronologically.
pub struct S3Poller {
    site_id: String,
    client: http_ingest::S3Client,
    /// The volume-sequence directory currently being drained. `None` only
    /// before the first poll resolves it (cold path). Separate from
    /// `last_completed_volume` so neither has to serve double duty (plan §2.3).
    target: Option<VolumeSeq>,
    /// Volume-sequence number of the last volume whose `-E` chunk has been seen
    /// (i.e. fully delivered). `None` until the first volume completes; this is
    /// the honest "fully delivered" number published in `IngestStatus`.
    last_completed_volume: Option<VolumeSeq>,
    /// `start-after` key within `target`. Reset to `None` whenever that volume
    /// completes and the poller moves on to the next one.
    last_seen_key: Option<String>,
    /// Empty polls in a row for the volume currently being drained. Reset
    /// to 0 whenever a key is seen or the target volume changes.
    consecutive_empty_polls: u32,
    /// Whether any key has been seen yet for the volume currently being
    /// drained. Distinguishes "this directory doesn't exist yet" (a
    /// skipped sequence number) from "stuck partway through a real
    /// volume" (S1-W3a) — both otherwise look identical (zero keys
    /// returned).
    seen_any_key_in_current_volume: bool,
    /// Consecutive `poll_once` failures. Reset to 0 on any success.
    consecutive_errors: u32,
    status_tx: watch::Sender<IngestStatus>,
}

impl S3Poller {
    /// `status_tx` is external, not owned/created here (S2-W2) — `AppState`
    /// holds a `watch::Receiver<IngestStatus>` (FR-DA-5) that must survive
    /// across pipeline restarts, but a panicking `S3Poller` does not: the
    /// supervisor (`pipeline::supervise`) rebuilds a fresh `S3Poller` on
    /// every restart, and every one of those must publish into the *same*
    /// long-lived channel for `AppState`'s receiver to keep working.
    /// `watch::Sender` is cheaply `Clone`, so callers hand each new poller
    /// a clone of one sender created once, up front.
    pub fn new(site_id: impl Into<String>, client: http_ingest::S3Client, status_tx: watch::Sender<IngestStatus>) -> Self {
        Self {
            site_id: site_id.into(),
            client,
            target: None,
            last_completed_volume: None,
            last_seen_key: None,
            consecutive_empty_polls: 0,
            seen_any_key_in_current_volume: false,
            consecutive_errors: 0,
            status_tx,
        }
    }

    /// Runs the polling loop, sending each new chunk over `tx` and
    /// publishing health updates to the receiver from [`Self::status`].
    /// Returns when `tx` is closed (receiver dropped) or `shutdown`
    /// publishes `true` (S2-W2 §4.2) — checked promptly via `select!`
    /// rather than only at the next poll interval boundary, so shutdown is
    /// deterministic instead of lazy. Errors are classified and published
    /// rather than causing the loop to exit; it never stops on its own.
    pub async fn run(
        mut self,
        tx: mpsc::Sender<ChunkEnvelope>,
        app_state: std::sync::Arc<AppState>,
        mut shutdown: watch::Receiver<bool>,
        poll_interval: Duration,
    ) {
        let mut interval = tokio::time::interval(poll_interval);
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = shutdown.wait_for(|s| *s) => return,
            }
            match self.poll_once(&app_state).await {
                Ok(envelopes) => {
                    self.consecutive_errors = 0;
                    let current_volume = self.last_completed_volume;
                    self.status_tx.send_modify(|s| {
                        s.state = IngestState::Polling;
                        s.last_success = Some(Instant::now());
                        s.current_volume = current_volume;
                    });
                    for envelope in envelopes {
                        if tx.send(envelope).await.is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    self.consecutive_errors += 1;
                    let kind = IngestErrorKind::from(&e);
                    let ingest_state = if self.consecutive_errors >= STALLED_AFTER_CONSECUTIVE_ERRORS {
                        IngestState::Stalled
                    } else {
                        IngestState::Retrying { attempts: self.consecutive_errors }
                    };
                    self.status_tx.send_modify(|s| {
                        s.state = ingest_state;
                        s.last_error = Some(kind);
                    });
                    app_state.report(crate::event::Event::PollFailed(e));
                }
            }
        }
    }

    async fn poll_once(&mut self, app_state: &AppState) -> Result<Vec<ChunkEnvelope>, PollError> {
        let prefix = format!("{}/", self.site_id);

        let target = match self.target {
            Some(t) => t,
            None => {
                let folders = self.list_volume_folders(app_state, &prefix).await?;
                self.report_ambiguity(app_state, &folders);
                // Cold start: anchor to the newest retained volume (partial,
                // and therefore the lowest-latency choice) rather than
                // replaying the full retention window (E-10).
                let t = cold_start_target(&folders);
                self.target = Some(t);
                t
            }
        };
        let volume_prefix = format!("{prefix}{target}/");

        let start_after = self.last_seen_key.clone();
        let (keys, _pages) =
            self.list_all_keys_after(&volume_prefix, start_after.as_deref()).await?;

        if keys.is_empty() {
            self.consecutive_empty_polls += 1;
            self.apply_recovery(app_state, &prefix, target).await?;
            return Ok(vec![]);
        }

        self.consecutive_empty_polls = 0;
        self.seen_any_key_in_current_volume = true;

        let mut to_fetch: Vec<(String, ChunkKind)> = Vec::with_capacity(keys.len());
        for key in &keys {
            match chunk_kind_from_key(key) {
                Some(kind) => to_fetch.push((key.clone(), kind)),
                None => app_state.report(crate::event::Event::UnrecognizedKeySuffix { key: key.clone() }),
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
            // next poll — `succ` rolls `999 → 1`. If that directory never
            // appears (an RDA restart skips sequence numbers, as observed
            // live) apply_recovery's re-anchor path (S1-W3a) is what keeps
            // the poller from stalling on a directory that will never exist.
            self.last_completed_volume = Some(target);
            self.target = Some(target.succ());
            self.last_seen_key = None;
            self.seen_any_key_in_current_volume = false;
        }

        Ok(envelopes)
    }

    /// Skipped-volume and stuck-mid-volume recovery (S1-W3a). Called only
    /// when `poll_once` saw zero keys this poll. Re-listing the bucket's
    /// volume folders costs a measured ~196ms (dependency-inventory-
    /// remediation.md §2.1), so this only does it when
    /// `REANCHOR_EMPTY_POLLS` have passed with no key seen at all for
    /// `target_volume` — and throttles back to checking again only every
    /// `REANCHOR_EMPTY_POLLS` polls thereafter, not on every poll.
    async fn apply_recovery(
        &mut self,
        app_state: &AppState,
        prefix: &str,
        target: VolumeSeq,
    ) -> Result<(), PollError> {
        let poll_state = PollState {
            current_target: target,
            consecutive_empty_polls: self.consecutive_empty_polls,
            seen_any_key_in_current_volume: self.seen_any_key_in_current_volume,
        };

        let should_relist = !poll_state.seen_any_key_in_current_volume
            && poll_state.consecutive_empty_polls >= REANCHOR_EMPTY_POLLS;
        let listing = if should_relist {
            let folders = self.list_volume_folders(app_state, prefix).await?;
            self.report_ambiguity(app_state, &folders);
            Some(folders)
        } else {
            None
        };

        let empty_polls = poll_state.consecutive_empty_polls;
        match next_target(&poll_state, listing.as_deref()) {
            PollAction::Continue => {
                if should_relist {
                    // Checked and found no gap; wait another full
                    // REANCHOR_EMPTY_POLLS before checking again.
                    self.consecutive_empty_polls = 0;
                }
            }
            PollAction::ReAnchor { new_target } => {
                app_state.report(crate::event::Event::ReAnchored {
                    from_volume: target,
                    to_volume: new_target,
                    empty_polls,
                });
                self.begin_recovery_at(new_target);
            }
            PollAction::AdvancePastStuckVolume { new_target } => {
                app_state.report(crate::event::Event::AdvancedPastStalledVolume {
                    from_volume: target,
                    to_volume: new_target,
                    empty_polls,
                });
                self.begin_recovery_at(new_target);
            }
        }

        Ok(())
    }

    /// The field/status resets common to both recovery actions (plan §2.3):
    /// point `target` at `new_target`, drop the in-volume position, and
    /// publish `ReAnchoring`. Deliberately does **not** touch
    /// `last_completed_volume` or `current_volume` — a re-anchor does not
    /// deliver a volume, so the published "fully delivered" number must not
    /// move (today's code republishes `new_target - 1` for a volume that was
    /// never delivered).
    fn begin_recovery_at(&mut self, new_target: VolumeSeq) {
        self.status_tx.send_modify(|s| s.state = IngestState::ReAnchoring);
        self.target = Some(new_target);
        self.last_seen_key = None;
        self.consecutive_empty_polls = 0;
        self.seen_any_key_in_current_volume = false;
    }

    /// Reports [`Event::VolumeSequenceOrderAmbiguous`] once per listing when
    /// the retained arc has no gap wide enough to trust as the retention
    /// boundary ([`volume_seq::MIN_TRUSTED_BOUNDARY_GAP`]). The pick is still
    /// deterministic; this only tells the operator the assumption behind it
    /// has weakened. Done here, not in the pure `VolumeWindow`, so the pure
    /// type does no I/O.
    fn report_ambiguity(&self, app_state: &AppState, folders: &[VolumeSeq]) {
        if let Some(window) = VolumeWindow::from_listing(folders) {
            if window.largest_gap() < volume_seq::MIN_TRUSTED_BOUNDARY_GAP {
                app_state.report(crate::event::Event::VolumeSequenceOrderAmbiguous {
                    largest_gap: window.largest_gap(),
                    folder_count: folders.len(),
                });
            }
        }
    }

    /// Lists the numeric volume-sequence subdirectories directly under `prefix`
    /// (e.g. `KDOX/166/`, `KDOX/167/`), using S3's `delimiter=/` so the response
    /// is `CommonPrefixes` rather than a flat, potentially enormous object listing.
    async fn list_volume_folders(
        &mut self,
        app_state: &AppState,
        prefix: &str,
    ) -> Result<Vec<VolumeSeq>, PollError> {
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
                    None => app_state
                        .report(crate::event::Event::UnrecognizedVolumeFolder { entry: common_prefix.clone() }),
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

/// The volume directory a cold start should begin draining: the newest
/// retained volume (partial, and therefore the lowest-latency choice — see
/// plan §2.4), or [`VolumeSeq::MIN`] when the site has no data at all. "Newest"
/// is [`VolumeWindow`]'s wrap-aware pick, which degenerates to the numeric max
/// for an unwrapped listing — so the pre-wrap behaviour is preserved verbatim.
/// Pure function, testable offline.
fn cold_start_target(folders: &[VolumeSeq]) -> VolumeSeq {
    VolumeWindow::from_listing(folders).map_or(VolumeSeq::MIN, |w| w.newest())
}

/// Minimal state `next_target` needs to decide what to do about an empty
/// poll. Deliberately narrower than all of `S3Poller` — same spirit as
/// `cold_start_target` being a pure, offline-testable function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PollState {
    /// The volume-sequence directory `poll_once` was draining this cycle.
    current_target: VolumeSeq,
    consecutive_empty_polls: u32,
    seen_any_key_in_current_volume: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PollAction {
    /// Keep polling `current_target`; nothing has crossed a threshold.
    Continue,
    /// A gap was found: `current_target` will never produce data because a
    /// later volume already exists. Anchor to `new_target` instead.
    ReAnchor { new_target: VolumeSeq },
    /// `current_target` produced real data but has now stalled past the
    /// longer threshold. Give up on it and move to `new_target`.
    AdvancePastStuckVolume { new_target: VolumeSeq },
}

/// Pure decision function for S1-W3a's skipped-volume and
/// stuck-mid-volume recovery. `listing` is the volume-sequence folders from a
/// fresh `list_volume_folders` call — `Some` only when the
/// caller actually performed that (costly) listing this poll, which itself
/// only happens when `state` already indicates a re-anchor check is due
/// (see `S3Poller::apply_recovery`). Passing `None` when nothing was
/// fetched avoids pretending a decision was made from data that was never
/// gathered.
fn next_target(state: &PollState, listing: Option<&[VolumeSeq]>) -> PollAction {
    if !state.seen_any_key_in_current_volume && state.consecutive_empty_polls >= REANCHOR_EMPTY_POLLS {
        if let Some(folders) = listing {
            if let Some(window) = VolumeWindow::from_listing(folders) {
                // Recovery lands on a *complete* volume, one behind the newest
                // — unlike the cold start, which takes the partial newest for
                // latency (plan §2.4). Never re-anchor backwards *in time*:
                // across a wrap the correct move is backwards numerically,
                // which is why this compares positions in the retained arc,
                // not integers.
                let candidate = window.newest().pred();
                if window.is_after(candidate, state.current_target) {
                    return PollAction::ReAnchor { new_target: candidate };
                }
            }
        }
        return PollAction::Continue;
    }

    if state.seen_any_key_in_current_volume && state.consecutive_empty_polls >= STUCK_MID_VOLUME_EMPTY_POLLS {
        return PollAction::AdvancePastStuckVolume { new_target: state.current_target.succ() };
    }

    PollAction::Continue
}

/// Parses the numeric volume-sequence directory name out of a `CommonPrefixes`
/// entry like `KDOX/166/`, given the enclosing `KDOX/` prefix. `None` for
/// anything that isn't a plain unsigned integer directory — defensive, since
/// the bucket's exact layout is observed behavior, not a documented contract.
fn parse_volume_folder(prefix: &str, common_prefix: &str) -> Option<VolumeSeq> {
    let n: u64 = common_prefix.strip_prefix(prefix)?.strip_suffix('/')?.parse().ok()?;
    VolumeSeq::new(n)
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

    /// A throwaway `AppState` for tests that exercise `S3Poller`'s private
    /// methods directly and need something to report events into but don't
    /// otherwise care about its contents.
    fn test_app_state() -> AppState {
        let (_tx, rx) = tokio::sync::watch::channel(IngestStatus::default());
        AppState::new(crate::sites::by_id("KDOX").expect("KDOX in bundled table"), rx)
    }

    #[test]
    fn chunk_kind_from_known_suffixes() {
        assert_eq!(chunk_kind_from_key("KDOX/166/20260728-095259-001-S"), Some(ChunkKind::Start));
        assert_eq!(chunk_kind_from_key("KDOX/166/20260728-095259-002-I"), Some(ChunkKind::Intermediate));
        assert_eq!(chunk_kind_from_key("KDOX/166/20260728-095259-079-E"), Some(ChunkKind::End));
        assert_eq!(chunk_kind_from_key("KDOX/166/20260728-095259-079-X"), None);
        assert_eq!(chunk_kind_from_key(""), None);
    }

    fn seq(n: u64) -> VolumeSeq {
        VolumeSeq::new(n).unwrap()
    }

    /// The 2026-09-04T00:55Z KDOX listing (volume_seq's module doc), rebuilt
    /// from its three contiguous runs so this fixture is the measurement.
    fn measured_kdox_wrap() -> Vec<VolumeSeq> {
        (1..=199).chain(std::iter::once(659)).chain(749..=999).map(seq).collect()
    }

    #[test]
    fn cold_start_target_of_an_empty_listing_is_the_first_sequence_number() {
        assert_eq!(cold_start_target(&[]), VolumeSeq::MIN);
    }

    #[test]
    fn cold_start_target_is_the_newest_retained_volume() {
        assert_eq!(cold_start_target(&[seq(5)]), seq(5));
        assert_eq!(cold_start_target(&[seq(3), seq(9), seq(7)]), seq(9));
        // Unpadded lexical order would sort "9" after "78"/"79"; the pick is
        // numeric, not from listing order.
        assert_eq!(cold_start_target(&[seq(9), seq(78), seq(79)]), seq(79));
    }

    #[test]
    fn cold_start_target_of_a_single_volume_one_folder_is_volume_one() {
        assert_eq!(cold_start_target(&[seq(1)]), VolumeSeq::MIN);
    }

    #[test]
    fn cold_start_target_across_the_measured_wrap_is_the_live_volume() {
        // Parent §1.2 verbatim: a numeric max would return 999 (03:08Z data);
        // the wrap-aware pick returns 199, the live volume seconds old.
        assert_eq!(cold_start_target(&measured_kdox_wrap()), seq(199));
    }

    #[test]
    fn the_volume_after_the_last_sequence_number_is_the_first() {
        // The poller's advance site (`saw_end` in `poll_once`): parent §1.3's
        // `1000` target can no longer be formed.
        assert_eq!(VolumeSeq::MAX.succ(), VolumeSeq::MIN);
    }

    fn poll_state(current_target: u64, consecutive_empty_polls: u32, seen_any_key: bool) -> PollState {
        PollState {
            current_target: seq(current_target),
            consecutive_empty_polls,
            seen_any_key_in_current_volume: seen_any_key,
        }
    }

    #[test]
    fn normal_empty_polls_below_threshold_do_not_reanchor() {
        let state = poll_state(90, REANCHOR_EMPTY_POLLS - 1, false);
        assert_eq!(next_target(&state, None), PollAction::Continue);
        // Even if a listing happened to be available, still below
        // threshold means it must not be used.
        assert_eq!(next_target(&state, Some(&[seq(90)])), PollAction::Continue);
    }

    #[test]
    fn gap_in_listing_reanchors_forward() {
        // Volume 90 never materializes; 92 through 165 do (E-observed gap).
        let state = poll_state(90, REANCHOR_EMPTY_POLLS, false);
        let folders: Vec<VolumeSeq> = (92..=165).map(seq).collect();
        assert_eq!(
            next_target(&state, Some(&folders)),
            PollAction::ReAnchor { new_target: seq(164) }
        );
    }

    #[test]
    fn no_gap_in_listing_does_not_reanchor() {
        // The directory just hasn't appeared yet — normal cold-tail wait.
        let state = poll_state(90, REANCHOR_EMPTY_POLLS, false);
        assert_eq!(next_target(&state, Some(&[seq(89)])), PollAction::Continue);
    }

    #[test]
    fn reanchor_never_moves_backwards_past_current_target() {
        // A stale/short listing must never suggest going backwards.
        let state = poll_state(200, REANCHOR_EMPTY_POLLS, false);
        assert_eq!(next_target(&state, Some(&[seq(5), seq(10), seq(50)])), PollAction::Continue);
    }

    #[test]
    fn reanchor_does_not_move_backwards_in_time_across_a_wrap() {
        // Just after a wrap: current_target is 5, but the listing still holds
        // the pre-wrap tail. The pre-wrap folders are numerically larger but
        // older in time, so the re-anchor must not chase them backwards.
        let state = poll_state(5, REANCHOR_EMPTY_POLLS, false);
        let folders: Vec<VolumeSeq> = (1..=5).chain(700..=999).map(seq).collect();
        assert_eq!(next_target(&state, Some(&folders)), PollAction::Continue);
    }

    #[test]
    fn reanchor_crosses_the_wrap_to_the_live_volume() {
        // The failure mode that had no recovery path at all (parent §1.3):
        // stuck at 998, but the live volume is 199 (post-wrap). The correct
        // move is numerically backwards, forwards in time — to newest.pred().
        let state = poll_state(998, REANCHOR_EMPTY_POLLS, false);
        assert_eq!(
            next_target(&state, Some(&measured_kdox_wrap())),
            PollAction::ReAnchor { new_target: seq(198) }
        );
    }

    #[test]
    fn stuck_mid_volume_advances_only_after_the_longer_threshold() {
        let state = poll_state(90, STUCK_MID_VOLUME_EMPTY_POLLS - 1, true);
        assert_eq!(next_target(&state, None), PollAction::Continue);

        let state = poll_state(90, STUCK_MID_VOLUME_EMPTY_POLLS, true);
        assert_eq!(next_target(&state, None), PollAction::AdvancePastStuckVolume { new_target: seq(91) });
    }

    #[test]
    fn advance_past_stuck_volume_wraps_at_the_end_of_the_cycle() {
        let state = poll_state(999, STUCK_MID_VOLUME_EMPTY_POLLS, true);
        assert_eq!(
            next_target(&state, None),
            PollAction::AdvancePastStuckVolume { new_target: VolumeSeq::MIN }
        );
    }

    #[test]
    fn stuck_mid_volume_does_not_trigger_the_reanchor_path() {
        // seen_any_key_in_current_volume=true means this is the
        // stuck-mid-volume case, not the never-saw-a-key case, even past
        // the (smaller) reanchor threshold.
        let state = poll_state(90, REANCHOR_EMPTY_POLLS, true);
        assert_eq!(next_target(&state, Some(&[seq(92), seq(93)])), PollAction::Continue);
    }

    #[test]
    fn parse_volume_folder_accepts_plain_integer_directories() {
        assert_eq!(parse_volume_folder("KDOX/", "KDOX/166/"), Some(seq(166)));
        assert_eq!(parse_volume_folder("KDOX/", "KDOX/1/"), Some(seq(1)));
    }

    #[test]
    fn parse_volume_folder_rejects_non_numeric_or_malformed_entries() {
        assert_eq!(parse_volume_folder("KDOX/", "KDOX/166"), None); // no trailing slash
        assert_eq!(parse_volume_folder("KDOX/", "KDOX/abc/"), None); // not an integer
        assert_eq!(parse_volume_folder("KDOX/", "OTHER/166/"), None); // wrong prefix
        assert_eq!(parse_volume_folder("KDOX/", "KDOX/1000/"), None); // outside the 1..=999 cycle
    }

    #[test]
    fn ingest_error_kind_classifies_network_failures() {
        assert_eq!(
            IngestErrorKind::from(&PollError::Http(http_ingest::Error::Connect(
                std::io::Error::other("refused")
            ))),
            IngestErrorKind::Network
        );
        assert_eq!(
            IngestErrorKind::from(&PollError::Http(http_ingest::Error::Timeout {
                phase: http_ingest::Phase::Connect
            })),
            IngestErrorKind::Network
        );
        assert_eq!(IngestErrorKind::from(&PollError::Http(http_ingest::Error::Closed)), IngestErrorKind::Network);
    }

    #[test]
    fn ingest_error_kind_classifies_http_status_and_malformed_listing() {
        assert_eq!(
            IngestErrorKind::from(&PollError::Http(http_ingest::Error::Http { status: 503 })),
            IngestErrorKind::Http { status: 503 }
        );

        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Contents>
    <Key>KDOX/166/a&nbsp;b</Key>
  </Contents>
</ListBucketResult>"#;
        let err = match parse_list_xml(xml) {
            Err(e) => e,
            Ok(_) => panic!("unrecognized entity should fail to parse"),
        };
        assert_eq!(IngestErrorKind::from(&err), IngestErrorKind::MalformedListing);
    }

    #[test]
    fn new_poller_publishes_default_polling_status() {
        let client = http_ingest::S3Client::new(http_ingest::Bucket::Chunks);
        let (status_tx, status_rx) = watch::channel(IngestStatus::default());
        let _poller = S3Poller::new("KDOX", client, status_tx);
        let status = status_rx.borrow().clone();
        assert_eq!(status.state, IngestState::Polling);
        assert!(status.last_success.is_none());
        assert!(status.last_error.is_none());
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
        let client = http_ingest::S3Client::new(http_ingest::Bucket::Chunks);
        let (status_tx, _status_rx) = watch::channel(IngestStatus::default());
        let mut poller = S3Poller::new(LIVE_TEST_SITE, client, status_tx);
        let prefix = format!("{}/", poller.site_id);
        let app_state = test_app_state();

        let start = Instant::now();
        let folders = poller.list_volume_folders(&app_state, &prefix).await.expect("list folders");
        let t_list = start.elapsed();

        let window = VolumeWindow::from_listing(&folders);
        println!(
            "cold_start_listing_size: volume_folders={} window={window:?} t_list={t_list:?}",
            folders.len()
        );
    }

    #[tokio::test]
    #[ignore]
    async fn cold_start_poll_once_latency() {
        let client = http_ingest::S3Client::new(http_ingest::Bucket::Chunks);
        let (status_tx, _status_rx) = watch::channel(IngestStatus::default());
        let mut poller = S3Poller::new(LIVE_TEST_SITE, client, status_tx);
        let app_state = test_app_state();

        let total_start = Instant::now();
        let envelopes = poller.poll_once(&app_state).await.expect("poll_once");
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
        let client = http_ingest::S3Client::new(http_ingest::Bucket::Chunks);
        let (status_tx, _status_rx) = watch::channel(IngestStatus::default());
        let mut poller = S3Poller::new(LIVE_TEST_SITE, client, status_tx);
        let app_state = test_app_state();

        let _first = poller.poll_once(&app_state).await.expect("poll 1 (cold start)");

        tokio::time::sleep(DEFAULT_POLL_INTERVAL).await;
        let start2 = Instant::now();
        let second = poller.poll_once(&app_state).await.expect("poll 2");
        let t2 = start2.elapsed();

        tokio::time::sleep(DEFAULT_POLL_INTERVAL).await;
        let start3 = Instant::now();
        let third = poller.poll_once(&app_state).await.expect("poll 3");
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
        let mut client = http_ingest::S3Client::new(http_ingest::Bucket::Chunks);
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

    /// S2-W3 (§5.3): lists the chunk bucket's top-level `CommonPrefixes`
    /// (site codes) and diffs them against the bundled site table
    /// (`crate::sites::all()`), reusing this module's own listing/pagination
    /// machinery rather than re-implementing it. Reports mismatches in
    /// either direction rather than asserting equality — the chunk bucket
    /// only reflects sites that have delivered a chunk within its retention
    /// window (observed ~48 h), and a handful of bundled sites (overseas
    /// DoD-operated radars) may not publish to it at all, so a mismatch
    /// here is a prompt for a human to investigate, not necessarily a bug.
    /// Turns "is our site list current?" from an opinion into a command —
    /// run with `cargo test -p radar-workstation -- --ignored --nocapture
    /// bucket_site_prefixes_match_bundled_site_list`.
    #[tokio::test]
    #[ignore]
    async fn bucket_site_prefixes_match_bundled_site_list() {
        let mut client = http_ingest::S3Client::new(http_ingest::Bucket::Chunks);
        let mut bucket_sites = std::collections::BTreeSet::new();
        let mut continuation_token: Option<String> = None;

        loop {
            let body = client
                .list_prefix("", None, continuation_token.as_deref(), Some("/"))
                .await
                .expect("list bucket root");
            let page = parse_list_xml(&body).expect("parse bucket root listing");
            for prefix in &page.common_prefixes {
                if let Some(site) = prefix.strip_suffix('/') {
                    bucket_sites.insert(site.to_string());
                }
            }
            if !page.is_truncated {
                break;
            }
            continuation_token = page.next_token;
        }

        let bundled: std::collections::BTreeSet<&str> = crate::sites::all().iter().map(|s| s.id).collect();
        let missing_from_table: Vec<&String> =
            bucket_sites.iter().filter(|s| !bundled.contains(s.as_str())).collect();
        let missing_from_bucket: Vec<&&str> =
            bundled.iter().filter(|s| !bucket_sites.contains(**s)).collect();

        println!(
            "bucket_site_prefixes_match_bundled_site_list: bucket_sites={} bundled_sites={} \
             in_bucket_not_in_table={missing_from_table:?} in_table_not_in_bucket={missing_from_bucket:?}",
            bucket_sites.len(),
            bundled.len(),
        );
    }

    /// S1-W3a, against the real bucket. Forces the poller's target one
    /// volume past whatever currently exists (indistinguishable, from the
    /// poller's perspective, from a target that will never materialize —
    /// a genuine permanent gap can't be manufactured against a live site
    /// that's actually producing volumes) and polls in a loop at the real
    /// 5s cadence. The property under test is the one the historical bug
    /// violated: the poller must not get stuck polling a directory
    /// forever. It doesn't matter here whether the forced volume resolves
    /// by arriving normally or via `apply_recovery`'s re-anchor path (that
    /// specific branch is what the pure `next_target` unit tests above
    /// verify); what matters live is that `last_completed_volume` reaches
    /// or passes `forced_target` within a generous deadline rather than
    /// stalling. Slow — bounded by how long the site's VCP takes to
    /// produce another volume or two, not by a fixed poll count.
    #[tokio::test]
    #[ignore]
    async fn does_not_stall_when_forced_past_the_newest_real_volume() {
        let client = http_ingest::S3Client::new(http_ingest::Bucket::Chunks);
        let (status_tx, _status_rx) = watch::channel(IngestStatus::default());
        let mut poller = S3Poller::new(LIVE_TEST_SITE, client, status_tx);
        let prefix = format!("{}/", poller.site_id);
        let app_state = test_app_state();

        let folders = poller.list_volume_folders(&app_state, &prefix).await.expect("list folders");
        let newest_at_start =
            VolumeWindow::from_listing(&folders).expect("at least one volume folder").newest();
        // One past the newest real volume — indistinguishable, from the
        // poller's perspective, from a target that will never materialize.
        let forced_target = newest_at_start.succ();
        poller.target = Some(forced_target);
        poller.last_completed_volume = Some(newest_at_start);

        let start = Instant::now();
        let deadline = Duration::from_secs(20 * 60);
        let mut interval = tokio::time::interval(DEFAULT_POLL_INTERVAL);

        // "Did not stall" == the target moved off `forced_target`, whether by
        // the volume arriving normally or by `apply_recovery` re-anchoring.
        let progressed = loop {
            interval.tick().await;
            poller.poll_once(&app_state).await.expect("poll_once");
            if poller.target != Some(forced_target) {
                break true;
            }
            if start.elapsed() > deadline {
                break false;
            }
        };

        println!(
            "does_not_stall_when_forced_past_the_newest_real_volume: newest_at_start={newest_at_start} \
             forced_target={forced_target} progressed={progressed} elapsed={:?} \
             target={:?} last_completed_volume={:?}",
            start.elapsed(),
            poller.target,
            poller.last_completed_volume
        );
        assert!(progressed, "poller stalled past {deadline:?} without leaving {forced_target}");
    }

    /// Parses a chunk key's fixed-width `YYYYMMDD-HHMMSS` prefix and returns
    /// how many seconds ago that was (negative if it is in the future).
    fn newest_key_age_secs(key: &str) -> i64 {
        let name = key.rsplit('/').next().expect("key has a final path component");
        let mut parts = name.split('-');
        let date = parts.next().expect("YYYYMMDD field");
        let time = parts.next().expect("HHMMSS field");
        let civil = (
            date[0..4].parse().expect("year"),
            date[4..6].parse().expect("month"),
            date[6..8].parse().expect("day"),
            time[0..2].parse().expect("hour"),
            time[2..4].parse().expect("minute"),
            time[4..6].parse().expect("second"),
        );
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("wall clock after the epoch")
            .as_secs() as i64;
        now - crate::time::unix_secs_from_civil(civil)
    }

    /// Parent A5. Lists the real volume folders, asserts every one is inside
    /// the observed `1..=999` cycle (the tripwire for the §2.2 assumption),
    /// and asserts the cold-start anchor's newest object is within one VCP
    /// cycle of now — a generous bound that still fails hard on a wrap-
    /// anchoring regression (a ~21-hour-old anchor).
    #[tokio::test]
    #[ignore]
    async fn cold_start_anchor_is_within_one_vcp_cycle_of_now() {
        let client = http_ingest::S3Client::new(http_ingest::Bucket::Chunks);
        let (status_tx, _status_rx) = watch::channel(IngestStatus::default());
        let mut poller = S3Poller::new(LIVE_TEST_SITE, client, status_tx);
        let prefix = format!("{}/", poller.site_id);
        let app_state = test_app_state();

        let folders = poller.list_volume_folders(&app_state, &prefix).await.expect("list folders");
        for f in &folders {
            assert!(
                (1..=999).contains(&f.get()),
                "folder {f} is outside the observed 1..=999 cycle — the bucket's numbering changed"
            );
        }

        let window = VolumeWindow::from_listing(&folders).expect("at least one folder");
        let anchor = cold_start_target(&folders);
        let (keys, _pages) = poller
            .list_all_keys_after(&format!("{prefix}{anchor}/"), None)
            .await
            .expect("list anchor keys");
        let newest_key = keys.last().expect("anchor directory has at least one key");
        let age = newest_key_age_secs(newest_key);
        let bound = 2 * crate::vcp::nominal_volume_duration(35).as_secs() as i64;

        println!(
            "cold_start_anchor_is_within_one_vcp_cycle_of_now: folder_count={} oldest={} newest={} \
             largest_gap={} anchor={anchor} newest_key={newest_key} age={age}s bound={bound}s",
            folders.len(),
            window.oldest(),
            window.newest(),
            window.largest_gap(),
        );
        assert!(
            age.abs() <= bound,
            "cold-start anchor {anchor} is {age}s old (bound {bound}s) — a wrap-anchoring regression"
        );
    }
}
