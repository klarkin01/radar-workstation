//! Volume history retention (ADR-0030, Stage 6a Part B). Pure: no I/O, no
//! clock read beyond the `now` parameter callers already inject the same way
//! `state::apply` does.
//!
//! **Why a frame is one volume, holding only its own sweeps** (ADR-0030
//! §3.1). A volume is the only unit at which "the next frame" is
//! well-defined — animating a tilt means stepping the same elevation number
//! across volumes — and Echo Tops/VIL are only defined at a volume boundary.
//! Today's live view merges a tilt forward across a volume boundary so a
//! closing volume never blanks the display (FR-DA-3, ADR-0012); if a
//! *retained* frame inherited that merge, a played-back frame would show a
//! tilt the radar did not scan at that time, presented as if it had. So a
//! [`Frame`] holds **only** the sweeps its own volume closed, and the merged
//! live view ([`live_sweeps`]) is a read-time fold over the ring instead —
//! one source of truth, no memo to drift.
//!
//! **Measured 2026-09-04** (ADR-0030 §1, `utility/radar-viz --path budget`):
//! a whole volume's gridded output is ~28-40 MB depending on site/VCP — see
//! [`DEFAULT_HISTORY_BUDGET_BYTES`]'s doc comment for the exact numbers this
//! sizes against. That is the reason retention is bounded by *both* a frame
//! count and a byte budget, not just a count (§3.3): volume size varies with
//! VCP and site, and a count alone is an unbounded memory commitment driven
//! by incoming data.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use crate::assembly::VolumeId;
use crate::compute::cut::{self, CutKey};
use crate::compute::{DisplayProduct, SweepGrid};
use crate::event::Event;

use super::{DisplaySweep, VolumeSummary};

/// One volume's own gridded output — the unit of history (ADR-0030).
///
/// A frame holds **only** the sweeps its volume actually closed; it never
/// borrows a tilt from its predecessor. Carry-forward for the live display
/// is a read-time fold ([`live_sweeps`]), not a mutation, so a frame that is
/// played back shows what the radar saw at that time and nothing else.
#[derive(Debug, Clone)]
pub struct Frame {
    pub volume: VolumeId,
    pub vcp_number: u16,
    /// When this frame's first sweep was applied.
    pub first_applied: Instant,
    /// This volume's own closed sweeps, by elevation number.
    pub sweeps: BTreeMap<u8, DisplaySweep>,
    /// Echo Tops / VIL, once this volume closed `Complete`.
    pub derived: BTreeMap<DisplayProduct, Arc<SweepGrid>>,
    /// `Some` once this volume closed `VolumeStatus::Complete`.
    pub complete: Option<VolumeSummary>,
    bytes: usize,
}

fn sweep_bytes(sweep: &DisplaySweep) -> usize {
    sweep.grids.iter().map(|g| g.byte_len()).sum()
}

fn derived_bytes(derived: &BTreeMap<DisplayProduct, Arc<SweepGrid>>) -> usize {
    derived.values().map(|g| g.byte_len()).sum()
}

impl Frame {
    /// A frame is normally created only by `FrameRing::head_mut`; this stays
    /// `pub` for test helpers (in this crate and in `render`'s own test
    /// suite, a different crate) that need to build one directly rather
    /// than drive a whole ring.
    pub fn new(volume: VolumeId, vcp_number: u16, now: Instant) -> Self {
        Self {
            volume,
            vcp_number,
            first_applied: now,
            sweeps: BTreeMap::new(),
            derived: BTreeMap::new(),
            complete: None,
            bytes: 0,
        }
    }

    /// This frame's total gridded footprint, maintained incrementally on
    /// every insert — the ring's budget arithmetic (`FrameRing::trim`) must
    /// not be O(grids) per applied event.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Replaces the entry for this sweep's elevation number, adjusting
    /// `bytes` by the difference. A repeated elevation *number* within one
    /// volume is a re-closure (SAILS/MRLE), not a second tilt — the same
    /// "replace, don't merge" rule `state::apply` already enforced before
    /// frames existed.
    pub fn insert_sweep(&mut self, sweep: DisplaySweep) {
        let new_bytes = sweep_bytes(&sweep);
        let old_bytes = self.sweeps.get(&sweep.elevation_number).map(sweep_bytes).unwrap_or(0);
        self.sweeps.insert(sweep.elevation_number, sweep);
        self.bytes = self.bytes + new_bytes - old_bytes;
    }

    /// Replaces the derived-product set wholesale (ADR-0012's rule,
    /// unchanged): Echo Tops and VIL are properties of one whole volume, so
    /// a partial merge would mix two volumes' tilts.
    pub fn set_derived(&mut self, grids: Vec<Arc<SweepGrid>>) {
        let old_bytes = derived_bytes(&self.derived);
        self.derived.clear();
        for grid in grids {
            self.derived.insert(grid.product, grid);
        }
        let new_bytes = derived_bytes(&self.derived);
        self.bytes = self.bytes + new_bytes - old_bytes;
    }

    /// This frame's own grid for `product` at `cut` (ADR-0031 D-3): the
    /// derived grid for Echo Tops/VIL (which have no tilt, so `cut` is
    /// ignored), else [`cut::best_at_cut`] over this volume's own sweeps.
    /// `None` when this volume did not scan it — never a substitute.
    pub fn grid_at_cut(&self, product: DisplayProduct, cut: Option<CutKey>) -> Option<&Arc<SweepGrid>> {
        if product.is_derived() {
            return self.derived.get(&product);
        }
        let grids = self.sweeps.values().flat_map(|s| s.grids.iter()).map(|g| (self.volume, g));
        cut::best_at_cut(grids, product, cut?).map(|(_, g)| g)
    }
}

/// The GPU cache key's tilt component (`render::radar::grid_key`): `None`
/// for Echo Tops/VIL (one per volume), `Some(elevation_number)` for a base
/// product. Keyed by elevation *number*, not cut, because two grids can
/// share a cut within one frame (a split cut's two halves, a SAILS
/// re-closure) and a cache key must name exactly one grid.
pub fn key_elevation(product: DisplayProduct, elevation_number: u8) -> Option<u8> {
    (!product.is_derived()).then_some(elevation_number)
}

/// Operator-set retention bounds (ADR-0030 §3.3). Configuration, not view
/// state: it reaches `AppState` at construction (`RadarState::new`) and
/// never flows backwards from the render loop (ADR-0018).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub frames: usize,
    pub budget_bytes: usize,
}

/// ADR-0030 §3.5's proposed default: a ~56-minute loop at the measured
/// VCP-35 frame size (~40 MB), landing a ~470 MB instance. Confirm this
/// trade before shipping it — `history.budget_mb = 96` restores a ~300 MB
/// instance with a ~2-frame loop, and `0` restores the Stage 5 footprint
/// exactly ([`RetentionPolicy::DISABLED`]).
pub const DEFAULT_HISTORY_FRAMES: usize = 12;
/// 320 MB. Measured frame sizes (`utility/radar-viz --path budget`,
/// 2026-09-04): KDOX VCP 35 ~39.8 MB, KFWS VCP 12 ~28.2 MB, KHGX VCP 212
/// ~30.8 MB — so 320 MB buys roughly 8 frames of the largest measured
/// pattern before the byte budget, not the frame count, becomes the
/// binding constraint (ADR-0030 §1, §3.5).
pub const DEFAULT_HISTORY_BUDGET_BYTES: usize = 320 * 1024 * 1024;

impl RetentionPolicy {
    /// The Stage 5 footprint: one frame, no byte headroom for a second. A
    /// first-class, tested setting (ADR-0030 §3.3) — `history.budget_mb = 0`
    /// in configuration maps to this, and the newest frame is still never
    /// evicted.
    pub const DISABLED: Self = Self { frames: 1, budget_bytes: 0 };
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self { frames: DEFAULT_HISTORY_FRAMES, budget_bytes: DEFAULT_HISTORY_BUDGET_BYTES }
    }
}

/// A volume older than every retained frame arrived, and no retained frame
/// matches it either. Inserting it would land behind the ring's head and
/// break the oldest-to-newest ordering invariant every other function here
/// relies on — reported as `Event::LateVolumeDiscarded`, not applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LateVolume;

/// The retained volumes, oldest first. `Arc<Frame>` so a snapshot costs one
/// refcount bump per frame whatever each frame holds (ADR-0030 §3.2); the
/// newest frame is mutated through `Arc::make_mut`, which copies only while
/// a snapshot is outstanding.
pub struct FrameRing {
    frames: VecDeque<Arc<Frame>>,
    policy: RetentionPolicy,
    bytes: usize,
    /// Total frames ever pushed, never decremented by eviction — this is
    /// what lets `trim` tell "fewer than `policy.frames` retained because
    /// the operator just started" apart from "fewer than `policy.frames`
    /// retained because the byte budget evicted one."
    total_pushed: usize,
    /// Whether the byte budget is *currently* the binding constraint —
    /// tracked so `trim` reports `Event::HistoryBudgetBound` on the rising
    /// edge only (ADR-0030 §3.3), not once per eviction in steady state.
    budget_bound: bool,
}

impl FrameRing {
    pub fn new(policy: RetentionPolicy) -> Self {
        Self { frames: VecDeque::new(), policy, bytes: 0, total_pushed: 0, budget_bound: false }
    }

    /// The frame for `volume`: the newest frame if `volume` matches it, a
    /// **new** frame pushed if `volume` is newer than the newest, or the
    /// matching existing frame if one is retained. `Err(LateVolume)` if
    /// `volume` is older than the newest and no retained frame matches —
    /// creating a frame behind the head would break the ring's ordering
    /// invariant, and the caller reports it.
    pub fn head_mut(&mut self, volume: VolumeId, vcp_number: u16, now: Instant) -> Result<&mut Frame, LateVolume> {
        let is_new_head = match self.frames.back() {
            Some(newest) => volume > newest.volume,
            None => true,
        };
        if is_new_head {
            self.frames.push_back(Arc::new(Frame::new(volume, vcp_number, now)));
            self.total_pushed += 1;
        }
        let idx = self.frames.iter().position(|f| f.volume == volume).ok_or(LateVolume)?;
        Ok(Arc::make_mut(&mut self.frames[idx]))
    }

    /// The frame for `volume`, without creating one if none is retained —
    /// used by `VolumeClosed`, which must never conjure a frame for a
    /// volume that closed without a single gridded sweep.
    pub fn frame_mut(&mut self, volume: VolumeId) -> Option<&mut Frame> {
        let idx = self.frames.iter().position(|f| f.volume == volume)?;
        Some(Arc::make_mut(&mut self.frames[idx]))
    }

    fn recompute_bytes(&mut self) {
        self.bytes = self.frames.iter().map(|f| f.bytes()).sum();
    }

    /// Direct access to the underlying ring, for `AppState::snapshot`'s live
    /// folds ([`live_sweeps`], [`live_derived`]) — the one consumer that
    /// needs the raw `VecDeque` in place rather than an owned copy.
    pub fn as_deque(&self) -> &VecDeque<Arc<Frame>> {
        &self.frames
    }

    /// Evict from the front while `len > policy.frames || bytes >
    /// policy.budget_bytes`, **never** below one frame — a budget too small
    /// for one volume must degrade to "no history", never to "no display".
    /// Returns `Some(Event::HistoryBudgetBound { .. })` on the rising edge
    /// of "the budget, not the frame count, is what bit" and stays silent
    /// on the falling edge and every steady-state call in between.
    pub fn trim(&mut self) -> Option<Event> {
        self.recompute_bytes();
        while self.frames.len() > 1 && (self.frames.len() > self.policy.frames || self.bytes > self.policy.budget_bytes)
        {
            self.frames.pop_front();
            self.recompute_bytes();
        }

        // The byte budget is what's binding, rather than "we simply haven't
        // accumulated policy.frames volumes yet", exactly when we've pushed
        // at least that many frames but hold fewer than that many now.
        let now_bound = self.total_pushed >= self.policy.frames && self.frames.len() < self.policy.frames;
        let was_bound = self.budget_bound;
        self.budget_bound = now_bound;

        if now_bound && !was_bound {
            Some(Event::HistoryBudgetBound {
                frames_retained: self.frames.len(),
                requested_frames: self.policy.frames,
                bytes: self.bytes,
            })
        } else {
            None
        }
    }

    pub fn newest(&self) -> Option<&Arc<Frame>> {
        self.frames.back()
    }

    pub fn oldest(&self) -> Option<&Arc<Frame>> {
        self.frames.front()
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<Frame>> {
        self.frames.iter()
    }

    /// Every retained frame, oldest → newest — the whole cost of
    /// `AppState::snapshot`'s history: N `Arc` clones (refcount bumps), not
    /// N frames' worth of grids.
    pub fn snapshot_frames(&self) -> Vec<Arc<Frame>> {
        self.frames.iter().cloned().collect()
    }

    /// Site change (ADR-0030 §3.4): a frame's grids are polar grids around a
    /// specific site, so there is nothing in the old ring that could be
    /// correctly displayed under the new one.
    pub fn clear(&mut self) {
        self.frames.clear();
        self.bytes = 0;
        self.total_pushed = 0;
        self.budget_bound = false;
    }
}

/// The live merged view: newest sweep per elevation number, folding the ring
/// newest-first and **stopping at the first frame whose VCP differs from
/// the newest frame's**. That stop is what reproduces Stage 2's
/// `sweeps.clear()`-on-VCP-change without deleting any history (ADR-0030
/// §3.4). Sorted by elevation number, as `RadarState.sweeps` always was.
/// Takes the ring (`&VecDeque`) or a snapshot's frames (`&Vec`) alike.
pub fn live_sweeps<'a, I>(frames: I) -> Vec<DisplaySweep>
where
    I: IntoIterator<Item = &'a Arc<Frame>>,
    I::IntoIter: DoubleEndedIterator,
{
    let mut newest_first = frames.into_iter().rev().peekable();
    let Some(newest_vcp) = newest_first.peek().map(|f| f.vcp_number) else {
        return Vec::new();
    };
    let mut out: BTreeMap<u8, DisplaySweep> = BTreeMap::new();
    for frame in newest_first.take_while(|f| f.vcp_number == newest_vcp) {
        for (elevation_number, sweep) in &frame.sweeps {
            out.entry(*elevation_number).or_insert_with(|| sweep.clone());
        }
    }
    out.into_values().collect()
}

/// The frame the live view's Echo Tops / VIL come from: the newest frame
/// that has them, within the newest frame's VCP. Same stop rule as
/// [`live_sweeps`], same reason.
pub fn live_derived_source<'a, I>(frames: I) -> Option<&'a Arc<Frame>>
where
    I: IntoIterator<Item = &'a Arc<Frame>>,
    I::IntoIter: DoubleEndedIterator,
{
    let mut newest_first = frames.into_iter().rev().peekable();
    let newest_vcp = newest_first.peek()?.vcp_number;
    newest_first.take_while(|f| f.vcp_number == newest_vcp).find(|f| !f.derived.is_empty())
}

/// The live derived-product view: [`live_derived_source`]'s grids.
pub fn live_derived<'a, I>(frames: I) -> Vec<Arc<SweepGrid>>
where
    I: IntoIterator<Item = &'a Arc<Frame>>,
    I::IntoIter: DoubleEndedIterator,
{
    live_derived_source(frames).map(|f| f.derived.values().cloned().collect()).unwrap_or_default()
}

/// The live view's grid for `product` at `cut`, with the volume it came from
/// (ADR-0031 D-3/D-4): [`cut::best_at_cut`] over the live fold `sweeps` for
/// a base product, or the [`live_derived_source`] frame's grid for a
/// derived one. The same rule as [`Frame::grid_at_cut`], over the fold.
pub fn live_grid_at_cut<'a>(
    frames: &'a [Arc<Frame>],
    sweeps: &'a [DisplaySweep],
    product: DisplayProduct,
    cut: Option<CutKey>,
) -> Option<(VolumeId, &'a Arc<SweepGrid>)> {
    if product.is_derived() {
        let frame = live_derived_source(frames)?;
        return frame.derived.get(&product).map(|g| (frame.volume, g));
    }
    let grids = sweeps.iter().flat_map(|s| s.grids.iter().map(move |g| (s.volume, g)));
    cut::best_at_cut(grids, product, cut?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    fn volume_id(scan_time_ms: u32) -> VolumeId {
        VolumeId { julian_date: 20_000, scan_time_ms }
    }

    fn grid(product: DisplayProduct, elevation_number: u8, cells: usize) -> Arc<SweepGrid> {
        Arc::new(SweepGrid {
            product,
            azimuth_count: 4,
            gate_count: cells as u16 / 4,
            first_gate_m: 0,
            gate_width_m: 250,
            elevation_number,
            elevation_deg: elevation_number as f32 * 0.5,
            cut_deg: elevation_number as f32 * 0.5,
            nyquist_velocity_mps: Some(8.0),
            scale: 2.0,
            offset: 66.0,
            cells: vec![0u8; cells],
            filled_azimuths: 0,
        })
    }

    fn sweep(elevation_number: u8, volume: VolumeId, vcp_number: u16, grids: Vec<Arc<SweepGrid>>) -> DisplaySweep {
        DisplaySweep {
            elevation_number,
            elevation_deg: elevation_number as f32 * 0.5,
            cut_deg: elevation_number as f32 * 0.5,
            volume,
            vcp_number,
            received: t0(),
            grids,
        }
    }

    #[test]
    fn a_frames_byte_count_tracks_its_grids() {
        let mut frame = Frame::new(volume_id(100), 35, t0());
        frame.insert_sweep(sweep(1, volume_id(100), 35, vec![grid(DisplayProduct::Reflectivity, 1, 16)]));
        assert_eq!(frame.bytes(), 16);
        frame.insert_sweep(sweep(2, volume_id(100), 35, vec![grid(DisplayProduct::Reflectivity, 2, 32)]));
        assert_eq!(frame.bytes(), 48);

        frame.set_derived(vec![grid(DisplayProduct::EchoTops, 0, 8)]);
        assert_eq!(frame.bytes(), 56);
    }

    #[test]
    fn replacing_a_sweep_in_a_frame_does_not_double_count() {
        let mut frame = Frame::new(volume_id(100), 35, t0());
        frame.insert_sweep(sweep(1, volume_id(100), 35, vec![grid(DisplayProduct::Reflectivity, 1, 16)]));
        frame.insert_sweep(sweep(1, volume_id(100), 35, vec![grid(DisplayProduct::Reflectivity, 1, 40)]));
        assert_eq!(frame.bytes(), 40, "replacing elevation 1 must not double-count the old grid");
    }

    #[test]
    fn the_newest_frame_is_never_evicted_even_under_a_zero_budget() {
        let mut ring = FrameRing::new(RetentionPolicy::DISABLED);
        ring.head_mut(volume_id(100), 35, t0()).unwrap().insert_sweep(sweep(
            1,
            volume_id(100),
            35,
            vec![grid(DisplayProduct::Reflectivity, 1, 1_000_000)],
        ));
        ring.trim();
        assert_eq!(ring.len(), 1, "one frame must survive even far over budget");

        ring.head_mut(volume_id(200), 35, t0()).unwrap().insert_sweep(sweep(
            1,
            volume_id(200),
            35,
            vec![grid(DisplayProduct::Reflectivity, 1, 1_000_000)],
        ));
        ring.trim();
        assert_eq!(ring.len(), 1, "the ring must still hold exactly the newest frame");
        assert_eq!(ring.newest().unwrap().volume, volume_id(200));
    }

    #[test]
    fn the_frame_count_bound_evicts_the_oldest_first() {
        let policy = RetentionPolicy { frames: 2, budget_bytes: usize::MAX };
        let mut ring = FrameRing::new(policy);
        for t in [100, 200, 300] {
            ring.head_mut(volume_id(t), 35, t0()).unwrap().insert_sweep(sweep(
                1,
                volume_id(t),
                35,
                vec![grid(DisplayProduct::Reflectivity, 1, 16)],
            ));
            ring.trim();
        }
        assert_eq!(ring.len(), 2);
        assert_eq!(ring.oldest().unwrap().volume, volume_id(200));
        assert_eq!(ring.newest().unwrap().volume, volume_id(300));
    }

    #[test]
    fn the_byte_budget_evicts_before_the_frame_count_when_frames_are_large() {
        let policy = RetentionPolicy { frames: 10, budget_bytes: 100 };
        let mut ring = FrameRing::new(policy);
        for t in [100, 200, 300] {
            ring.head_mut(volume_id(t), 35, t0()).unwrap().insert_sweep(sweep(
                1,
                volume_id(t),
                35,
                vec![grid(DisplayProduct::Reflectivity, 1, 60)],
            ));
            ring.trim();
        }
        assert!(ring.len() < 3, "the byte budget must evict before the frame count does: len={}", ring.len());
        assert!(ring.bytes() <= 100 || ring.len() == 1);
    }

    #[test]
    fn the_budget_bound_event_is_edge_triggered() {
        let policy = RetentionPolicy { frames: 2, budget_bytes: 100 };
        let mut ring = FrameRing::new(policy);
        let mut bound_events = 0;
        for t in [100, 200, 300, 400, 500] {
            ring.head_mut(volume_id(t), 35, t0()).unwrap().insert_sweep(sweep(
                1,
                volume_id(t),
                35,
                vec![grid(DisplayProduct::Reflectivity, 1, 60)],
            ));
            if ring.trim().is_some() {
                bound_events += 1;
            }
        }
        assert_eq!(bound_events, 1, "a steady-state ring at the budget must report once, not once per eviction");
    }

    #[test]
    fn a_volume_older_than_the_head_and_not_retained_is_rejected() {
        let mut ring = FrameRing::new(RetentionPolicy { frames: 1, budget_bytes: usize::MAX });
        ring.head_mut(volume_id(200), 35, t0()).unwrap();
        assert!(ring.head_mut(volume_id(100), 35, t0()).is_err());
    }

    #[test]
    fn a_late_sweep_for_a_retained_frame_lands_in_that_frame_not_the_head() {
        let mut ring = FrameRing::new(RetentionPolicy { frames: 3, budget_bytes: usize::MAX });
        ring.head_mut(volume_id(100), 35, t0()).unwrap();
        ring.head_mut(volume_id(200), 35, t0()).unwrap();
        ring.head_mut(volume_id(100), 35, t0())
            .unwrap()
            .insert_sweep(sweep(3, volume_id(100), 35, vec![grid(DisplayProduct::Reflectivity, 3, 16)]));
        assert_eq!(ring.newest().unwrap().volume, volume_id(200));
        assert!(ring.newest().unwrap().sweeps.is_empty(), "the late sweep must not land in the head");
        let old_frame = ring.iter().find(|f| f.volume == volume_id(100)).unwrap();
        assert!(old_frame.sweeps.contains_key(&3));
    }

    #[test]
    fn live_sweeps_folds_newest_first_and_carries_elevations_forward() {
        let mut ring = FrameRing::new(RetentionPolicy { frames: 4, budget_bytes: usize::MAX });
        ring.head_mut(volume_id(100), 35, t0()).unwrap().insert_sweep(sweep(
            1,
            volume_id(100),
            35,
            vec![grid(DisplayProduct::Reflectivity, 1, 16)],
        ));
        ring.head_mut(volume_id(100), 35, t0()).unwrap().insert_sweep(sweep(
            2,
            volume_id(100),
            35,
            vec![grid(DisplayProduct::Reflectivity, 2, 16)],
        ));
        ring.head_mut(volume_id(200), 35, t0()).unwrap().insert_sweep(sweep(
            1,
            volume_id(200),
            35,
            vec![grid(DisplayProduct::Reflectivity, 1, 16)],
        ));

        let frames: VecDeque<Arc<Frame>> = ring.iter().cloned().collect();
        let sweeps = live_sweeps(&frames);
        assert_eq!(sweeps.len(), 2, "FR-DA-3: elevation 2 must carry forward from the older frame");
        assert_eq!(sweeps.iter().find(|s| s.elevation_number == 1).unwrap().volume, volume_id(200));
        assert_eq!(sweeps.iter().find(|s| s.elevation_number == 2).unwrap().volume, volume_id(100));
    }

    #[test]
    fn live_sweeps_stops_at_a_vcp_boundary() {
        let mut ring = FrameRing::new(RetentionPolicy { frames: 4, budget_bytes: usize::MAX });
        ring.head_mut(volume_id(100), 35, t0()).unwrap().insert_sweep(sweep(
            1,
            volume_id(100),
            35,
            vec![grid(DisplayProduct::Reflectivity, 1, 16)],
        ));
        ring.head_mut(volume_id(100), 35, t0()).unwrap().insert_sweep(sweep(
            2,
            volume_id(100),
            35,
            vec![grid(DisplayProduct::Reflectivity, 2, 16)],
        ));
        ring.head_mut(volume_id(200), 212, t0()).unwrap().insert_sweep(sweep(
            1,
            volume_id(200),
            212,
            vec![grid(DisplayProduct::Reflectivity, 1, 16)],
        ));

        let frames: VecDeque<Arc<Frame>> = ring.iter().cloned().collect();
        let sweeps = live_sweeps(&frames);
        assert_eq!(sweeps.len(), 1, "old VCP's elevations must be dropped on a VCP change");
        assert_eq!(sweeps[0].elevation_number, 1);
        assert_eq!(sweeps[0].volume, volume_id(200));
    }

    #[test]
    fn a_vcp_change_does_not_evict_the_old_patterns_frames() {
        let mut ring = FrameRing::new(RetentionPolicy { frames: 4, budget_bytes: usize::MAX });
        ring.head_mut(volume_id(100), 35, t0()).unwrap().insert_sweep(sweep(
            1,
            volume_id(100),
            35,
            vec![grid(DisplayProduct::Reflectivity, 1, 16)],
        ));
        ring.trim();
        ring.head_mut(volume_id(200), 212, t0()).unwrap().insert_sweep(sweep(
            1,
            volume_id(200),
            212,
            vec![grid(DisplayProduct::Reflectivity, 1, 16)],
        ));
        ring.trim();
        assert_eq!(ring.len(), 2, "a VCP change alone must not evict anything");
    }

    #[test]
    fn live_derived_comes_from_the_newest_frame_that_has_it() {
        let mut ring = FrameRing::new(RetentionPolicy { frames: 4, budget_bytes: usize::MAX });
        ring.head_mut(volume_id(100), 35, t0()).unwrap().set_derived(vec![grid(DisplayProduct::Vil, 0, 16)]);
        ring.head_mut(volume_id(200), 35, t0()).unwrap();

        let frames: VecDeque<Arc<Frame>> = ring.iter().cloned().collect();
        let derived = live_derived(&frames);
        assert_eq!(derived.len(), 1, "the newest frame has no derived products yet; fall back to the older one");
        assert_eq!(derived[0].product, DisplayProduct::Vil);
    }

    #[test]
    fn grid_at_cut_reads_only_this_frames_own_sweeps() {
        let mut frame = Frame::new(volume_id(100), 35, t0());
        frame.insert_sweep(sweep(1, volume_id(100), 35, vec![grid(DisplayProduct::Reflectivity, 1, 16)]));
        assert!(frame.grid_at_cut(DisplayProduct::Reflectivity, Some(cut::cut_key(0.5))).is_some());
        assert!(frame.grid_at_cut(DisplayProduct::Reflectivity, Some(cut::cut_key(1.0))).is_none(), "absent cut");
        assert!(frame.grid_at_cut(DisplayProduct::Velocity, Some(cut::cut_key(0.5))).is_none(), "absent product");
        assert!(frame.grid_at_cut(DisplayProduct::Reflectivity, None).is_none(), "no cut selected");
    }

    #[test]
    fn grid_at_cut_of_a_derived_product_ignores_the_cut() {
        let mut frame = Frame::new(volume_id(100), 35, t0());
        frame.set_derived(vec![grid(DisplayProduct::Vil, 0, 16)]);
        assert!(frame.grid_at_cut(DisplayProduct::Vil, None).is_some());
        assert!(frame.grid_at_cut(DisplayProduct::Vil, Some(cut::cut_key(8.0))).is_some());
    }

    #[test]
    fn live_grid_at_cut_reports_the_volume_a_carried_forward_grid_came_from() {
        let mut ring = FrameRing::new(RetentionPolicy { frames: 4, budget_bytes: usize::MAX });
        ring.head_mut(volume_id(100), 35, t0())
            .unwrap()
            .insert_sweep(sweep(2, volume_id(100), 35, vec![grid(DisplayProduct::Reflectivity, 2, 16)]));
        ring.head_mut(volume_id(200), 35, t0())
            .unwrap()
            .insert_sweep(sweep(1, volume_id(200), 35, vec![grid(DisplayProduct::Reflectivity, 1, 16)]));
        let frames = ring.snapshot_frames();
        let sweeps = live_sweeps(&frames);

        let (v, _) = live_grid_at_cut(&frames, &sweeps, DisplayProduct::Reflectivity, Some(cut::cut_key(1.0))).unwrap();
        assert_eq!(v, volume_id(100), "elevation 2 (1.0°) is carried forward from the older frame");
        let (v, _) = live_grid_at_cut(&frames, &sweeps, DisplayProduct::Reflectivity, Some(cut::cut_key(0.5))).unwrap();
        assert_eq!(v, volume_id(200));
    }

    #[test]
    fn live_derived_source_names_the_frame_live_derived_reads() {
        let mut ring = FrameRing::new(RetentionPolicy { frames: 4, budget_bytes: usize::MAX });
        ring.head_mut(volume_id(100), 35, t0()).unwrap().set_derived(vec![grid(DisplayProduct::Vil, 0, 16)]);
        ring.head_mut(volume_id(200), 35, t0()).unwrap();
        let frames = ring.snapshot_frames();
        assert_eq!(live_derived_source(&frames).map(|f| f.volume), Some(volume_id(100)));
        let sweeps = live_sweeps(&frames);
        let (v, _) = live_grid_at_cut(&frames, &sweeps, DisplayProduct::Vil, None).unwrap();
        assert_eq!(v, volume_id(100));
    }
}
