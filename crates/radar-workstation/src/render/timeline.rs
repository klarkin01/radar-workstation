//! The timeline (Stage 6a Part C, ADR-0031): the operator-visible time axis
//! over the frames ADR-0030 retains. Pure — no window, no GPU, no clock read
//! beyond the `now` its callers pass — so every row of the plan's §4 test
//! matrix is an ordinary unit test.
//!
//! **Resolution (D-4).** [`resolve`] turns `(Timeline, product, cut,
//! snapshot)` into one [`Resolved`] per frame, which every consumer reads:
//! the radar draw, residency, the status bar, the scrubber and the cursor
//! readout. `Live` shows the live fold, exactly as before. Every other
//! position shows **only that frame's own sweeps**. A frame where the D-3
//! rule finds nothing shows no radar data and says so. Nothing is ever
//! substituted.
//!
//! **ViewState rule (D-12).** [`Timeline`] lives in `ViewState`. It changes
//! on operator input ([`step`], [`toggle_play`], [`jump_to_live`],
//! [`adjust_loop`], [`pin`]) and on the playback clock ([`tick`]) while
//! `Playing` — never on a state update. `resolve` takes `&Timeline`, and
//! every mutator takes a `&Resolved`, never a snapshot.

use std::sync::Arc;
use std::time::{Duration, Instant};

use radar_workstation::assembly::VolumeId;
use radar_workstation::compute::cut::{self, CutKey};
use radar_workstation::compute::{DisplayProduct, SweepGrid};
use radar_workstation::config::{self, Config};
use radar_workstation::state::history::{self, Frame};
use radar_workstation::state::StateSnapshot;

/// Where the display is on the time axis (FR-TL-1). Every launch starts
/// `Live`; the mode is never persisted (D-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Live,
    Paused(VolumeId),
    /// `next_step` is when [`tick`] next advances `at` (D-11).
    Playing { at: VolumeId, next_step: Instant },
}

/// [`Mode`] without its payload — what the badge and residency branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModeKind {
    #[default]
    Live,
    Paused,
    Playing,
}

impl Mode {
    pub fn kind(self) -> ModeKind {
        match self {
            Mode::Live => ModeKind::Live,
            Mode::Paused(_) => ModeKind::Paused,
            Mode::Playing { .. } => ModeKind::Playing,
        }
    }

    /// The frame this mode is positioned on, or `None` for `Live`.
    fn position(self) -> Option<VolumeId> {
        match self {
            Mode::Live => None,
            Mode::Paused(v) | Mode::Playing { at: v, .. } => Some(v),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeline {
    pub mode: Mode,
    /// The loop window is the newest `loop_frames` retained frames (D-7).
    pub loop_frames: usize,
    pub frame_interval: Duration,
    /// Extra hold on the window's last frame before wrapping.
    pub dwell: Duration,
    /// Play was refused because fewer than two frames in the window carry
    /// the selection (D-7). Cleared by the next timeline action.
    pub play_refused: bool,
}

impl Timeline {
    /// A `Live` timeline (D-7: every launch starts live).
    pub fn new(loop_frames: usize, frame_interval: Duration, dwell: Duration) -> Self {
        let loop_frames = loop_frames.clamp(*LOOP_RANGE.start(), *LOOP_RANGE.end());
        Self { mode: Mode::Live, loop_frames, frame_interval, dwell, play_refused: false }
    }

    /// The D-13 config keys, defaulted where absent.
    pub fn from_config(cfg: &Config) -> Self {
        Self::new(
            cfg.timeline_loop_frames.unwrap_or(config::TIMELINE_LOOP_FRAMES_DEFAULT),
            Duration::from_millis(cfg.timeline_frame_ms.unwrap_or(config::TIMELINE_FRAME_MS_DEFAULT)),
            Duration::from_millis(cfg.timeline_dwell_ms.unwrap_or(config::TIMELINE_DWELL_MS_DEFAULT)),
        )
    }
}

const LOOP_RANGE: std::ops::RangeInclusive<usize> = config::TIMELINE_LOOP_FRAMES_RANGE;

/// Identity of a volume on screen: what the scan-time, age and badge
/// readouts are computed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayedScan {
    pub volume: VolumeId,
    pub vcp_number: u16,
}

/// One retained frame as the scrubber and the step/play functions see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub volume: VolumeId,
    /// The frame carries the selection under D-3. A frame that does not is
    /// drawn hollow and skipped by step and play (D-4).
    pub satisfies: bool,
    /// Inside the newest-`loop_frames` window (D-7).
    pub in_loop: bool,
}

/// One frame's resolution of the timeline against a snapshot (D-4).
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    pub kind: ModeKind,
    /// The cut in effect: the operator's, or the lowest live cut until they
    /// choose one (the first-sweep default, resolved here rather than
    /// written into `ViewState` — D-12).
    pub cut: Option<CutKey>,
    /// The frame shown when not `Live` (after the D-8 eviction fallback).
    pub focus: Option<VolumeId>,
    /// The grid to draw, or `None` for "no data" — never a substitute.
    pub grid: Option<Arc<SweepGrid>>,
    /// The frame `grid` lives in (the GPU cache key's volume).
    pub source: Option<VolumeId>,
    /// The volume on screen: `focus`'s frame, or `source` when `Live`.
    pub scan: Option<DisplayedScan>,
    /// What `Live` would show — the live feed the age readout and its stale
    /// alert always measure (D-9).
    pub live_scan: Option<DisplayedScan>,
    pub live_source: Option<VolumeId>,
    /// Every retained frame, oldest → newest.
    pub positions: Vec<Position>,
    /// The distinct cuts PageUp/PageDown walk: the live fold's when `Live`,
    /// the focus frame's otherwise.
    pub cuts: Vec<CutKey>,
    /// `Some(pinned)` when the paused frame aged out of the ring (D-8).
    pub pinned_evicted: Option<VolumeId>,
}

fn scan_of(frame: &Frame) -> DisplayedScan {
    DisplayedScan { volume: frame.volume, vcp_number: frame.vcp_number }
}

/// D-4 / D-8. Pure: reads `snapshot`, never mutates the timeline.
pub fn resolve(tl: &Timeline, product: DisplayProduct, view_cut: Option<CutKey>, snapshot: &StateSnapshot) -> Resolved {
    let frames = &snapshot.frames;
    let live_cuts = cut::distinct_cuts(snapshot.sweeps.iter().flat_map(|s| s.grids.iter()));
    let cut = view_cut.or_else(|| live_cuts.first().copied());

    let find_frame = |v: VolumeId| frames.iter().find(|f| f.volume == v);
    let live = history::live_grid_at_cut(frames, &snapshot.sweeps, product, cut);
    let live_source = live.map(|(v, _)| v);
    let live_scan = live_source.and_then(find_frame).map(|f| scan_of(f));

    let satisfies = |f: &Frame| f.grid_at_cut(product, cut).is_some();
    let window_start = frames.len().saturating_sub(tl.loop_frames);
    let positions = frames
        .iter()
        .enumerate()
        .map(|(i, f)| Position { volume: f.volume, satisfies: satisfies(f), in_loop: i >= window_start })
        .collect();

    let kind = tl.mode.kind();
    let Some(position) = tl.mode.position() else {
        return Resolved {
            kind,
            cut,
            focus: None,
            grid: live.map(|(_, g)| Arc::clone(g)),
            source: live_source,
            scan: live_scan,
            live_scan,
            live_source,
            positions,
            cuts: live_cuts,
            pinned_evicted: None,
        };
    };

    // D-8: a position whose frame aged out resolves to the oldest frame
    // that satisfies D-3 (else the oldest) — without touching the timeline.
    let (frame, evicted) = match find_frame(position) {
        Some(f) => (Some(f), None),
        None => {
            let fallback = frames.iter().find(|f| satisfies(f)).or(frames.first());
            (fallback, (kind == ModeKind::Paused).then_some(position))
        }
    };
    let grid = frame.and_then(|f| f.grid_at_cut(product, cut)).cloned();
    Resolved {
        kind,
        cut,
        focus: frame.map(|f| f.volume),
        source: grid.as_ref().and(frame.map(|f| f.volume)),
        grid,
        scan: frame.map(|f| scan_of(f)),
        live_scan,
        live_source,
        positions,
        cuts: frame.map(|f| cut::distinct_cuts(f.sweeps.values().flat_map(|s| s.grids.iter()))).unwrap_or_default(),
        pinned_evicted: evicted,
    }
}

fn index_of(r: &Resolved, v: VolumeId) -> Option<usize> {
    r.positions.iter().position(|p| p.volume == v)
}

/// The frame the operator is on: the focus when not `Live`; when `Live`,
/// the frame holding the displayed grid (else the newest). The scrubber
/// marks it and step/pause start from it.
pub fn current_index(r: &Resolved) -> Option<usize> {
    let v = r.focus.or(r.live_source).or(r.positions.last().map(|p| p.volume))?;
    index_of(r, v)
}

/// Indices in the loop window that satisfy D-3, ascending.
fn playable(r: &Resolved) -> Vec<usize> {
    r.positions.iter().enumerate().filter(|(_, p)| p.in_loop && p.satisfies).map(|(i, _)| i).collect()
}

/// How many frames in the loop window carry the selection — the count the
/// status bar states when Play is refused (D-7).
pub fn playable_count(r: &Resolved) -> usize {
    playable(r).len()
}

/// `frame_interval`, plus `dwell` on the window's last playable frame (D-7).
fn hold(tl: &Timeline, window: &[usize], i: usize) -> Duration {
    if window.last() == Some(&i) {
        tl.frame_interval + tl.dwell
    } else {
        tl.frame_interval
    }
}

/// Pin the frame currently displayed, so the image does not change (D-4).
/// From `Live` that is the frame holding the displayed grid.
pub fn pause(tl: &mut Timeline, r: &Resolved) {
    tl.play_refused = false;
    if let Some(i) = current_index(r) {
        tl.mode = Mode::Paused(r.positions[i].volume);
    }
}

/// Pin `volume` — a scrubber click or drag (D-4).
pub fn pin(tl: &mut Timeline, volume: VolumeId) {
    tl.play_refused = false;
    tl.mode = Mode::Paused(volume);
}

/// `[` (`forward = false`) / `]` (`forward = true`): one satisfying frame
/// back or forward across the whole ring, no wrap (D-7). Stops playback.
/// `]` past the newest frame returns to `Live`; `[` at the oldest stays.
pub fn step(tl: &mut Timeline, r: &Resolved, forward: bool) {
    tl.play_refused = false;
    let live = tl.mode == Mode::Live;
    if forward && live {
        return;
    }
    let cur = current_index(r);
    let target = match (cur, forward) {
        (Some(c), false) => (0..c).rev().find(|&i| r.positions[i].satisfies),
        (Some(c), true) => (c + 1..r.positions.len()).find(|&i| r.positions[i].satisfies),
        (None, _) => None,
    };
    match target {
        Some(i) => tl.mode = Mode::Paused(r.positions[i].volume),
        None if forward => tl.mode = Mode::Live,
        None if matches!(tl.mode, Mode::Playing { .. }) => pause(tl, r),
        None => {}
    }
}

/// `Space` (D-7). Playing → paused on the displayed frame. Otherwise play
/// the loop window from the pinned frame if it is in the window, else from
/// the window's oldest — unless fewer than two of its frames carry the
/// selection, in which case nothing changes and `play_refused` is set.
pub fn toggle_play(tl: &mut Timeline, r: &Resolved, now: Instant) {
    if matches!(tl.mode, Mode::Playing { .. }) {
        pause(tl, r);
        return;
    }
    let window = playable(r);
    if window.len() < 2 {
        tl.play_refused = true;
        return;
    }
    tl.play_refused = false;
    let pinned = match tl.mode {
        Mode::Paused(_) => r.focus.and_then(|v| index_of(r, v)).filter(|i| window.contains(i)),
        _ => None,
    };
    let start = pinned.unwrap_or(window[0]);
    tl.mode = Mode::Playing { at: r.positions[start].volume, next_step: now + hold(tl, &window, start) };
}

/// `End`: back to `Live`.
pub fn jump_to_live(tl: &mut Timeline) {
    tl.play_refused = false;
    tl.mode = Mode::Live;
}

/// `,` / `.`: one frame shorter / longer, within 2..=64 (D-6).
pub fn adjust_loop(tl: &mut Timeline, delta: i32) {
    tl.play_refused = false;
    let next = tl.loop_frames as i64 + delta as i64;
    tl.loop_frames = next.clamp(*LOOP_RANGE.start() as i64, *LOOP_RANGE.end() as i64) as usize;
}

/// The playback clock (D-11), once per redraw. Returns the next deadline
/// while `Playing` and `None` otherwise — a stopped timeline adds no
/// redraws (FR-TL-9). Advances to the next playable frame in the window,
/// wrapping after the last; a newly arrived volume simply extends the
/// window, so the pass in progress is not interrupted. Fewer than two
/// playable frames (the selection changed under it) pauses in place.
pub fn tick(tl: &mut Timeline, r: &Resolved, now: Instant) -> Option<Instant> {
    let Mode::Playing { at, next_step } = tl.mode else { return None };
    if now < next_step {
        return Some(next_step);
    }
    let window = playable(r);
    if window.len() < 2 {
        tl.mode = Mode::Paused(r.focus.unwrap_or(at));
        return None;
    }
    let next = index_of(r, at).and_then(|c| window.iter().copied().find(|&i| i > c)).unwrap_or(window[0]);
    let next_step = now + hold(tl, &window, next);
    tl.mode = Mode::Playing { at: r.positions[next].volume, next_step };
    Some(next_step)
}

/// The frame residency keeps fully resident (D-5): the pinned frame while
/// `Paused`, the newest (`None`) while `Live` or `Playing`.
pub fn residency_focus(r: &Resolved) -> Option<VolumeId> {
    match r.kind {
        ModeKind::Paused => r.focus,
        ModeKind::Live | ModeKind::Playing => None,
    }
}

/// PageUp (`dir > 0`) / PageDown: the next distinct cut above / below
/// `current` among `cuts` (ascending). From no cut, or a cut this source
/// lacks, it moves to the nearest one in the direction pressed. `None` when
/// there is nothing to move to — the selection stays put (§3.8).
pub fn step_cut(cuts: &[CutKey], current: Option<CutKey>, dir: i32) -> Option<CutKey> {
    let Some(current) = current else { return cuts.first().copied() };
    if dir > 0 {
        cuts.iter().copied().find(|&c| c > current)
    } else {
        cuts.iter().copied().rev().find(|&c| c < current)
    }
}

/// What a future layer 7 (placefiles) reads to know which time to draw
/// (D-10). The only timeline accessor outside the display path.
#[allow(dead_code)] // Stage 6 is the first caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeContext {
    Live,
    /// Unix seconds of the displayed volume's start time.
    At(i64),
}

#[allow(dead_code)] // Stage 6 is the first caller.
pub fn time_context(r: &Resolved) -> TimeContext {
    match (r.kind, r.scan) {
        (ModeKind::Live, _) | (_, None) => TimeContext::Live,
        (_, Some(scan)) => TimeContext::At(radar_workstation::time::unix_secs_from_nexrad(
            scan.volume.julian_date,
            scan.volume.scan_time_ms,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{frame, frame_with_derived, snapshot, volume};
    use super::*;
    use radar_workstation::compute::cut::cut_key;

    const REF: DisplayProduct = DisplayProduct::Reflectivity;
    const VEL: DisplayProduct = DisplayProduct::Velocity;

    fn timeline() -> Timeline {
        Timeline::new(12, Duration::from_millis(250), Duration::from_millis(1500))
    }

    fn at(mode: Mode) -> Timeline {
        Timeline { mode, ..timeline() }
    }

    /// Four VCP 35 frames, each with REF+VEL at 0.5° and REF at 1.3°.
    fn four_frames() -> StateSnapshot {
        snapshot((1..=4).map(|i| frame(i * 100, 35, &[(1, 0.5, &[REF, VEL]), (2, 1.3, &[REF])])).collect())
    }

    fn cut05() -> Option<CutKey> {
        Some(cut_key(0.5))
    }

    // --- resolve ---

    #[test]
    fn live_resolves_to_the_live_fold() {
        let snap = four_frames();
        let r = resolve(&timeline(), REF, cut05(), &snap);
        let (v, g) = history::live_grid_at_cut(&snap.frames, &snap.sweeps, REF, cut05()).unwrap();
        assert!(Arc::ptr_eq(r.grid.as_ref().unwrap(), g), "Live must draw exactly the live fold's grid");
        assert_eq!(r.source, Some(v));
        assert_eq!(r.live_scan, r.scan);
        assert_eq!(r.kind, ModeKind::Live);
    }

    #[test]
    fn a_paused_frame_shows_only_its_own_sweeps() {
        // Frame 200 never closed 1.3°; the live fold would carry 100's
        // forward. Paused on 200, it must not.
        let snap = snapshot(vec![
            frame(100, 35, &[(1, 0.5, &[REF]), (2, 1.3, &[REF])]),
            frame(200, 35, &[(1, 0.5, &[REF])]),
        ]);
        let r = resolve(&at(Mode::Paused(volume(200))), REF, Some(cut_key(1.3)), &snap);
        assert!(r.grid.is_none(), "nothing carried from another volume (FR-TL-2)");
        assert_eq!(r.focus, Some(volume(200)));
        assert_eq!(r.scan.map(|s| s.volume), Some(volume(200)), "the volume on screen is still named");
        assert!(!r.positions[1].satisfies, "drawn hollow");

        let live = resolve(&timeline(), REF, Some(cut_key(1.3)), &snap);
        assert_eq!(live.source, Some(volume(100)), "Live still carries forward, as before");
    }

    #[test]
    fn a_pinned_frame_that_aged_out_resolves_to_the_oldest_satisfying_and_says_so() {
        let snap = snapshot(vec![
            frame(200, 35, &[(1, 0.5, &[VEL])]),
            frame(300, 35, &[(1, 0.5, &[REF])]),
            frame(400, 35, &[(1, 0.5, &[REF])]),
        ]);
        let tl = at(Mode::Paused(volume(100)));
        let r = resolve(&tl, REF, cut05(), &snap);
        assert_eq!(r.focus, Some(volume(300)), "oldest frame that carries REF at 0.5°");
        assert_eq!(r.pinned_evicted, Some(volume(100)));
        assert_eq!(r.kind, ModeKind::Paused, "the display stays paused");
        assert_eq!(tl.mode, Mode::Paused(volume(100)), "resolve never mutates the timeline");
    }

    #[test]
    fn a_paused_volume_stays_on_screen_when_backfill_inserts_older_frames_behind_it() {
        // ADR-0033: backfill inserts at the front of the ring, shifting every
        // index. The focus is a VolumeId, so the display must not move.
        let three: Vec<_> = [200, 300, 400].into_iter().map(|t| frame(t, 35, &[(1, 0.5, &[REF])])).collect();
        let mut four = vec![frame(100, 35, &[(1, 0.5, &[REF])])];
        four.extend(three.iter().cloned());
        let tl = at(Mode::Paused(volume(300)));

        let before = resolve(&tl, REF, cut05(), &snapshot(three));
        let after = resolve(&tl, REF, cut05(), &snapshot(four));
        assert_eq!(before.focus, Some(volume(300)));
        assert_eq!(after.focus, Some(volume(300)), "still on the volume the operator paused on");
        assert_eq!(after.source, before.source);
        assert_eq!(after.pinned_evicted, None);
        assert_eq!(current_index(&after), Some(2), "its index moved; it did not");
    }

    #[test]
    fn a_derived_product_resolves_per_frame() {
        let snap = snapshot(vec![
            frame_with_derived(100, 35, &[DisplayProduct::Vil]),
            frame_with_derived(200, 35, &[DisplayProduct::EchoTops]),
        ]);
        let r = resolve(&at(Mode::Paused(volume(100))), DisplayProduct::Vil, None, &snap);
        assert!(r.grid.is_some());
        assert_eq!(r.source, Some(volume(100)));
        assert_eq!(r.positions.iter().map(|p| p.satisfies).collect::<Vec<_>>(), vec![true, false]);
    }

    #[test]
    fn the_default_cut_is_the_lowest_live_cut() {
        let r = resolve(&timeline(), REF, None, &four_frames());
        assert_eq!(r.cut, cut05());
        assert_eq!(r.cuts, vec![cut_key(0.5), cut_key(1.3)]);
    }

    #[test]
    fn the_loop_window_is_the_newest_loop_frames() {
        let r = resolve(&Timeline { loop_frames: 2, ..timeline() }, REF, cut05(), &four_frames());
        assert_eq!(r.positions.iter().map(|p| p.in_loop).collect::<Vec<_>>(), vec![false, false, true, true]);
    }

    // --- step / pause ---

    #[test]
    fn step_forward_past_the_newest_returns_to_live() {
        let snap = four_frames();
        let mut tl = at(Mode::Paused(volume(400)));
        let r = resolve(&tl, REF, cut05(), &snap);
        step(&mut tl, &r, true);
        assert_eq!(tl.mode, Mode::Live);
    }

    #[test]
    fn step_back_at_the_oldest_stays() {
        let snap = four_frames();
        let mut tl = at(Mode::Paused(volume(100)));
        let r = resolve(&tl, REF, cut05(), &snap);
        step(&mut tl, &r, false);
        assert_eq!(tl.mode, Mode::Paused(volume(100)));
    }

    #[test]
    fn step_back_from_live_pins_the_frame_before_the_displayed_one() {
        let snap = four_frames();
        let mut tl = timeline();
        let r = resolve(&tl, REF, cut05(), &snap);
        step(&mut tl, &r, false);
        assert_eq!(tl.mode, Mode::Paused(volume(300)));
    }

    #[test]
    fn steps_skip_frames_lacking_the_selection() {
        let snap = snapshot(vec![
            frame(100, 35, &[(1, 0.5, &[REF])]),
            frame(200, 35, &[(1, 0.5, &[VEL])]),
            frame(300, 35, &[(1, 0.5, &[REF])]),
        ]);
        let mut tl = at(Mode::Paused(volume(300)));
        let r = resolve(&tl, REF, cut05(), &snap);
        step(&mut tl, &r, false);
        assert_eq!(tl.mode, Mode::Paused(volume(100)), "200 has no REF at 0.5° and is skipped");
    }

    #[test]
    fn pause_from_live_pins_the_frame_holding_the_displayed_sweep() {
        // The newest frame (300) has not closed 0.5° yet; Live carries 200's.
        let snap = snapshot(vec![
            frame(100, 35, &[(1, 0.5, &[REF])]),
            frame(200, 35, &[(1, 0.5, &[REF])]),
            frame(300, 35, &[(2, 1.3, &[REF])]),
        ]);
        let mut tl = timeline();
        let live = resolve(&tl, REF, cut05(), &snap);
        pause(&mut tl, &live);
        assert_eq!(tl.mode, Mode::Paused(volume(200)));
        let paused = resolve(&tl, REF, cut05(), &snap);
        assert!(Arc::ptr_eq(paused.grid.as_ref().unwrap(), live.grid.as_ref().unwrap()), "the image must not change on pause");
    }

    // --- play / tick ---

    #[test]
    fn play_from_live_starts_at_the_windows_oldest() {
        let snap = four_frames();
        let mut tl = Timeline { loop_frames: 3, ..timeline() };
        let now = Instant::now();
        let r = resolve(&tl, REF, cut05(), &snap);
        toggle_play(&mut tl, &r, now);
        assert!(matches!(tl.mode, Mode::Playing { at, next_step } if at == volume(200) && next_step == now + tl.frame_interval));
    }

    #[test]
    fn play_from_a_pinned_frame_in_the_window_starts_there() {
        let snap = four_frames();
        let mut tl = at(Mode::Paused(volume(300)));
        let r = resolve(&tl, REF, cut05(), &snap);
        toggle_play(&mut tl, &r, Instant::now());
        assert!(matches!(tl.mode, Mode::Playing { at, .. } if at == volume(300)));
    }

    #[test]
    fn the_last_frame_dwells_and_playback_wraps() {
        let snap = four_frames();
        let mut tl = Timeline { loop_frames: 2, ..timeline() };
        let t0 = Instant::now();
        let r = resolve(&tl, REF, cut05(), &snap);
        toggle_play(&mut tl, &r, t0);
        assert!(matches!(tl.mode, Mode::Playing { at, .. } if at == volume(300)));

        let t1 = t0 + tl.frame_interval;
        let r = resolve(&tl, REF, cut05(), &snap);
        let deadline = tick(&mut tl, &r, t1);
        assert!(matches!(tl.mode, Mode::Playing { at, .. } if at == volume(400)));
        assert_eq!(deadline, Some(t1 + tl.frame_interval + tl.dwell), "the newest frame dwells");

        let t2 = deadline.unwrap();
        let r = resolve(&tl, REF, cut05(), &snap);
        tick(&mut tl, &r, t2);
        assert!(matches!(tl.mode, Mode::Playing { at, .. } if at == volume(300)), "wraps to the window's oldest");
    }

    #[test]
    fn tick_before_the_deadline_changes_nothing() {
        let snap = four_frames();
        let mut tl = timeline();
        let t0 = Instant::now();
        let r = resolve(&tl, REF, cut05(), &snap);
        toggle_play(&mut tl, &r, t0);
        let before = tl;
        let r = resolve(&tl, REF, cut05(), &snap);
        assert_eq!(tick(&mut tl, &r, t0), Some(t0 + tl.frame_interval));
        assert_eq!(tl, before);
    }

    #[test]
    fn a_new_frame_extends_the_window_without_a_jump() {
        let mut tl = Timeline { loop_frames: 12, ..timeline() };
        let t0 = Instant::now();
        let snap = four_frames();
        let r = resolve(&tl, REF, cut05(), &snap);
        toggle_play(&mut tl, &r, t0);
        let r = resolve(&tl, REF, cut05(), &snap);
        let interval = tl.frame_interval;
        tick(&mut tl, &r, t0 + interval); // at 200

        let mut frames = snap.frames.clone();
        frames.push(frame(500, 35, &[(1, 0.5, &[REF])]));
        let grown = snapshot(frames);
        let r = resolve(&tl, REF, cut05(), &grown);
        tick(&mut tl, &r, t0 + interval * 2);
        assert!(matches!(tl.mode, Mode::Playing { at, .. } if at == volume(300)), "the pass continues where it was");
    }

    #[test]
    fn play_with_fewer_than_two_satisfying_frames_is_a_no_op_that_says_so() {
        let snap = snapshot(vec![frame(100, 35, &[(1, 0.5, &[VEL])]), frame(200, 35, &[(1, 0.5, &[REF])])]);
        let mut tl = timeline();
        let r = resolve(&tl, REF, cut05(), &snap);
        toggle_play(&mut tl, &r, Instant::now());
        assert_eq!(tl.mode, Mode::Live);
        assert!(tl.play_refused);
        assert_eq!(playable_count(&r), 1);
        jump_to_live(&mut tl);
        assert!(!tl.play_refused, "cleared by the next timeline action");
    }

    #[test]
    fn a_step_stops_playback() {
        let snap = four_frames();
        let mut tl = timeline();
        let r = resolve(&tl, REF, cut05(), &snap);
        toggle_play(&mut tl, &r, Instant::now()); // at 100
        let r = resolve(&tl, REF, cut05(), &snap);
        step(&mut tl, &r, true);
        assert_eq!(tl.mode, Mode::Paused(volume(200)));

        let r = resolve(&tl, REF, cut05(), &snap);
        toggle_play(&mut tl, &r, Instant::now()); // at 200
        let r = resolve(&tl, REF, cut05(), &snap);
        step(&mut tl, &r, false);
        assert_eq!(tl.mode, Mode::Paused(volume(100)));
    }

    #[test]
    fn a_stopped_timeline_yields_no_deadline() {
        let snap = four_frames();
        for mode in [Mode::Live, Mode::Paused(volume(200))] {
            let mut tl = at(mode);
            let r = resolve(&tl, REF, cut05(), &snap);
            assert_eq!(tick(&mut tl, &r, Instant::now()), None);
            assert_eq!(tl.mode, mode);
        }
    }

    #[test]
    fn space_while_playing_pauses_on_the_displayed_frame() {
        let snap = four_frames();
        let mut tl = timeline();
        let r = resolve(&tl, REF, cut05(), &snap);
        toggle_play(&mut tl, &r, Instant::now());
        let r = resolve(&tl, REF, cut05(), &snap);
        toggle_play(&mut tl, &r, Instant::now());
        assert_eq!(tl.mode, Mode::Paused(volume(100)));
    }

    // --- VCP change ---

    fn vcp_change() -> StateSnapshot {
        snapshot(vec![
            frame(100, 35, &[(1, 0.5, &[REF]), (9, 6.4, &[REF])]),
            frame(200, 35, &[(1, 0.5, &[REF]), (9, 6.4, &[REF])]),
            frame(300, 212, &[(1, 0.5, &[REF]), (14, 8.0, &[REF])]),
            frame(400, 212, &[(1, 0.5, &[REF]), (14, 8.0, &[REF])]),
        ])
    }

    #[test]
    fn a_loop_across_a_vcp_change_plays_both_sides_at_a_shared_cut() {
        let r = resolve(&timeline(), REF, cut05(), &vcp_change());
        assert_eq!(playable_count(&r), 4);
    }

    #[test]
    fn a_cut_only_the_new_pattern_flies_is_hollow_in_the_old_patterns_frames() {
        let snap = vcp_change();
        let mut tl = timeline();
        let r = resolve(&tl, REF, Some(cut_key(8.0)), &snap);
        assert_eq!(r.positions.iter().map(|p| p.satisfies).collect::<Vec<_>>(), vec![false, false, true, true]);

        let t0 = Instant::now();
        toggle_play(&mut tl, &r, t0);
        assert!(matches!(tl.mode, Mode::Playing { at, .. } if at == volume(300)));
        let t1 = t0 + tl.frame_interval;
        let r = resolve(&tl, REF, Some(cut_key(8.0)), &snap);
        let t2 = tick(&mut tl, &r, t1).unwrap();
        let r = resolve(&tl, REF, Some(cut_key(8.0)), &snap);
        tick(&mut tl, &r, t2);
        assert!(matches!(tl.mode, Mode::Playing { at, .. } if at == volume(300)), "VCP 35 frames are skipped on wrap");
    }

    // --- cuts, loop length, residency focus, time context ---

    #[test]
    fn page_up_walks_distinct_cuts_and_never_visits_a_split_cuts_second_half() {
        // Elevations 1 and 2 are one split cut at 0.5°.
        let snap = snapshot(vec![frame(100, 35, &[(1, 0.5, &[REF]), (2, 0.5, &[REF, VEL]), (3, 0.9, &[REF])])]);
        let r = resolve(&timeline(), REF, cut05(), &snap);
        assert_eq!(r.cuts, vec![cut_key(0.5), cut_key(0.9)]);
        assert_eq!(step_cut(&r.cuts, cut05(), 1), Some(cut_key(0.9)));
        assert_eq!(step_cut(&r.cuts, Some(cut_key(0.9)), 1), None, "nothing above: stay put");
        assert_eq!(step_cut(&r.cuts, Some(cut_key(0.9)), -1), cut05());
        assert_eq!(step_cut(&r.cuts, Some(cut_key(0.7)), 1), Some(cut_key(0.9)), "from an absent cut, the nearest above");
    }

    #[test]
    fn loop_length_is_bounded_to_2_through_64() {
        let mut tl = Timeline { loop_frames: 3, ..timeline() };
        adjust_loop(&mut tl, -1);
        adjust_loop(&mut tl, -1);
        assert_eq!(tl.loop_frames, 2);
        tl.loop_frames = 64;
        adjust_loop(&mut tl, 1);
        assert_eq!(tl.loop_frames, 64);
    }

    #[test]
    fn residency_focus_is_the_pinned_frame_only_while_paused() {
        let snap = four_frames();
        assert_eq!(residency_focus(&resolve(&at(Mode::Paused(volume(200))), REF, cut05(), &snap)), Some(volume(200)));
        assert_eq!(residency_focus(&resolve(&timeline(), REF, cut05(), &snap)), None);
        let playing = at(Mode::Playing { at: volume(200), next_step: Instant::now() });
        assert_eq!(residency_focus(&resolve(&playing, REF, cut05(), &snap)), None);
    }

    #[test]
    fn time_context_is_live_or_the_displayed_volumes_time() {
        let snap = four_frames();
        assert_eq!(time_context(&resolve(&timeline(), REF, cut05(), &snap)), TimeContext::Live);
        let v = volume(200);
        assert_eq!(
            time_context(&resolve(&at(Mode::Paused(v)), REF, cut05(), &snap)),
            TimeContext::At(radar_workstation::time::unix_secs_from_nexrad(v.julian_date, v.scan_time_ms))
        );
    }

    #[test]
    fn every_launch_starts_live() {
        assert_eq!(Timeline::from_config(&Config::default()).mode, Mode::Live);
        assert_eq!(Timeline::from_config(&Config::default()).loop_frames, config::TIMELINE_LOOP_FRAMES_DEFAULT);
    }
}
