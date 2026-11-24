//! The chunk bucket's `<volume-sequence>` directory name is a **cyclic
//! counter over `1..=999`**, not a monotonically increasing integer. This is
//! observed behaviour, not a documented contract — the bucket's key layout
//! has only ever been established by direct measurement (see
//! `docs/plans/stage-6a-time-handling.md` §1.1).
//!
//! Measured against `unidata-nexrad-level2-chunks`, prefix `KDOX/`,
//! `delimiter=/`, at 2026-09-04T00:55Z: 451 `CommonPrefixes` in three
//! contiguous runs — `[(1, 199), (659, 659), (749, 999)]`. `KDOX/999/`'s
//! newest object was 2026-09-03T03:08:14Z and `KDOX/1/`'s was
//! 2026-09-03T03:13:46Z, so the counter rolled `999 → 1` at ~2026-09-03T03:13Z.
//! Both sides of the wrap appear in one listing because objects outlive the
//! wrap (retention is observed at ~48 h).
//!
//! Two sequence numbers therefore have **no ordering** without a reference
//! point: 999 is not "after" 1 across a wrap. [`VolumeSeq`] deliberately does
//! not implement `Ord`/`PartialOrd`. Ordering is supplied by [`VolumeWindow`],
//! which recovers the retained arc of the cycle from one listing (as the two
//! sides of its largest cyclic gap) and orders by position within that arc.

/// A chunk-bucket volume-sequence number: a cyclic counter over `1..=999`,
/// **not** a monotonically increasing integer (see the module doc).
/// Deliberately no `Ord`/`PartialOrd` — see [`VolumeWindow`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VolumeSeq(u16);

impl VolumeSeq {
    /// The lowest sequence number the bucket ever uses.
    pub const MIN: VolumeSeq = VolumeSeq(1);
    /// The highest sequence number before the counter rolls back to [`MIN`](Self::MIN).
    pub const MAX: VolumeSeq = VolumeSeq(999);
    /// The number of distinct sequence numbers in one cycle. Private: callers
    /// reason about the cycle through `succ`/`pred` and [`VolumeWindow`], not
    /// by doing modular arithmetic themselves.
    const CYCLE: u32 = 999;

    /// `None` outside `1..=999`.
    pub fn new(n: u64) -> Option<Self> {
        if (1..=Self::CYCLE as u64).contains(&n) {
            Some(VolumeSeq(n as u16))
        } else {
            None
        }
    }

    /// The raw sequence number.
    pub fn get(self) -> u16 {
        self.0
    }

    /// The next sequence number, wrapping `999 → 1`.
    pub fn succ(self) -> Self {
        if self.0 == Self::MAX.0 {
            Self::MIN
        } else {
            VolumeSeq(self.0 + 1)
        }
    }

    /// The previous sequence number, wrapping `1 → 999`.
    pub fn pred(self) -> Self {
        if self.0 == Self::MIN.0 {
            Self::MAX
        } else {
            VolumeSeq(self.0 - 1)
        }
    }
}

impl std::fmt::Display for VolumeSeq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The retained arc of the cycle, recovered from one listing: everything
/// between `oldest` and `newest` going forward, wrap included. The arc's ends
/// are the two sides of the largest cyclic gap (plan §2.2) — the retained set
/// always occupies a contiguous arc with holes in it, so the newest volume is
/// the element immediately *before* the largest gap and the oldest is the one
/// immediately *after* it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeWindow {
    oldest: VolumeSeq,
    newest: VolumeSeq,
    largest_gap: u32,
}

/// The smallest boundary gap [`VolumeWindow::from_listing`] trusts as a real
/// retention edge. The cycle is 999 volumes and retention is ~48 h; even at
/// the fastest VCP (~4.2 min/volume) 48 h is ~686 volumes, leaving a boundary
/// gap of ~313. The largest gap between *skipped* sequence numbers observed
/// live is 73 (`92→165`). 180 sits an order of magnitude clear of the latter
/// and comfortably below the former. A listing whose largest gap falls below
/// this is still resolved deterministically, but the poller reports it (plan
/// §2.2) because the assumption above has stopped holding.
pub const MIN_TRUSTED_BOUNDARY_GAP: u32 = 180;

impl VolumeWindow {
    /// Recovers the retained arc from one listing of volume-sequence folders.
    /// Sorts and dedups a copy. `None` for an empty slice. A one-element slice
    /// yields `oldest == newest` and `largest_gap == CYCLE`.
    ///
    /// Degenerates correctly: an unwrapped single run yields exactly the
    /// numeric max as `newest` (the boundary gap is then the wrap gap), so the
    /// pre-wrap cold-start behaviour is preserved verbatim.
    pub fn from_listing(listing: &[VolumeSeq]) -> Option<Self> {
        let mut v: Vec<u16> = listing.iter().map(|s| s.0).collect();
        v.sort_unstable();
        v.dedup();
        let first = *v.first()?;
        let last = *v.last()?;

        if v.len() == 1 {
            let only = VolumeSeq(first);
            return Some(Self { oldest: only, newest: only, largest_gap: VolumeSeq::CYCLE });
        }

        // The wrap gap (from the numeric max forward to the numeric min) is
        // the boundary of an unwrapped listing; any internal gap that beats it
        // is the real retention boundary of a wrapped one.
        let mut newest = last;
        let mut oldest = first;
        let mut largest_gap = first as u32 + VolumeSeq::CYCLE - last as u32;
        for w in v.windows(2) {
            let gap = (w[1] - w[0]) as u32;
            if gap > largest_gap {
                largest_gap = gap;
                newest = w[0];
                oldest = w[1];
            }
        }

        Some(Self { oldest: VolumeSeq(oldest), newest: VolumeSeq(newest), largest_gap })
    }

    pub fn newest(&self) -> VolumeSeq {
        self.newest
    }

    pub fn oldest(&self) -> VolumeSeq {
        self.oldest
    }

    /// The largest cyclic gap in the listing this window was built from, so
    /// the caller can report an ambiguous boundary without the pure function
    /// doing I/O (see [`MIN_TRUSTED_BOUNDARY_GAP`]).
    pub fn largest_gap(&self) -> u32 {
        self.largest_gap
    }

    /// Forward distance (number of `succ` steps) from `oldest` to `v`, on the
    /// cycle. Total — defined for values outside the listing too, including
    /// the not-yet-created `newest().succ()`.
    pub fn position(&self, v: VolumeSeq) -> u32 {
        (v.0 as u32 + VolumeSeq::CYCLE - self.oldest.0 as u32) % VolumeSeq::CYCLE
    }

    /// Whether `a` is genuinely newer than `b`: a small, positive number of
    /// `succ` steps leads from `b` to `a` (RFC 1982-style serial comparison).
    /// Across a wrap the newer volume is numerically *smaller*, which a plain
    /// integer compare gets backwards; this compares positions in the retained
    /// arc instead. The `oldest` offset cancels, so this is independent of
    /// which folder happens to be oldest.
    pub fn is_after(&self, a: VolumeSeq, b: VolumeSeq) -> bool {
        let forward = (self.position(a) + VolumeSeq::CYCLE - self.position(b)) % VolumeSeq::CYCLE;
        forward != 0 && forward < VolumeSeq::CYCLE / 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 2026-09-04T00:55Z KDOX listing, rebuilt from its three contiguous
    /// runs so the fixture *is* the measurement.
    fn measured_kdox_wrap() -> Vec<VolumeSeq> {
        (1..=199)
            .chain(std::iter::once(659))
            .chain(749..=999)
            .map(|n| VolumeSeq::new(n).unwrap())
            .collect()
    }

    fn window(listing: &[VolumeSeq]) -> VolumeWindow {
        VolumeWindow::from_listing(listing).expect("non-empty listing")
    }

    fn seq(n: u64) -> VolumeSeq {
        VolumeSeq::new(n).unwrap()
    }

    #[test]
    fn successor_of_the_last_sequence_number_is_the_first() {
        assert_eq!(VolumeSeq::MAX.succ(), VolumeSeq::MIN);
        assert_eq!(seq(500).succ(), seq(501));
    }

    #[test]
    fn predecessor_of_the_first_sequence_number_is_the_last() {
        assert_eq!(VolumeSeq::MIN.pred(), VolumeSeq::MAX);
        assert_eq!(seq(500).pred(), seq(499));
    }

    #[test]
    fn out_of_range_sequence_numbers_are_rejected() {
        assert!(VolumeSeq::new(0).is_none());
        assert!(VolumeSeq::new(1000).is_none());
        assert!(VolumeSeq::new(u64::MAX).is_none());
        assert!(VolumeSeq::new(1).is_some());
        assert!(VolumeSeq::new(999).is_some());
    }

    #[test]
    fn the_measured_listing_has_the_prefix_count_that_was_observed() {
        assert_eq!(measured_kdox_wrap().len(), 451);
    }

    #[test]
    fn window_over_the_measured_kdox_wrap_picks_the_post_wrap_volume() {
        let w = window(&measured_kdox_wrap());
        assert_eq!(w.newest(), seq(199));
        assert_eq!(w.oldest(), seq(659));
    }

    #[test]
    fn window_over_a_single_unwrapped_run_agrees_with_numeric_max() {
        let listing: Vec<VolumeSeq> = (100..=500).map(seq).collect();
        let w = window(&listing);
        assert_eq!(w.newest(), seq(500));
        assert_eq!(w.oldest(), seq(100));
    }

    #[test]
    fn window_of_one_folder_is_that_folder() {
        let w = window(&[seq(42)]);
        assert_eq!(w.newest(), seq(42));
        assert_eq!(w.oldest(), seq(42));
        assert_eq!(w.largest_gap(), VolumeSeq::CYCLE);
    }

    #[test]
    fn window_of_an_empty_listing_is_none() {
        assert!(VolumeWindow::from_listing(&[]).is_none());
    }

    #[test]
    fn skipped_sequence_numbers_inside_the_arc_do_not_split_it() {
        // The live-observed 92→165 skip (a gap of 73) must not become the
        // boundary: the real retention edge (the wrap gap here) is far larger.
        let listing: Vec<VolumeSeq> = (1..=92).chain(165..=700).map(seq).collect();
        let w = window(&listing);
        assert_eq!(w.newest(), seq(700));
        assert!(w.largest_gap() >= MIN_TRUSTED_BOUNDARY_GAP);
    }

    #[test]
    fn order_is_by_position_in_the_arc_not_by_integer() {
        let w = window(&measured_kdox_wrap());
        assert!(w.is_after(seq(1), seq(999)));
        assert!(!w.is_after(seq(999), seq(1)));
    }

    #[test]
    fn a_volume_that_does_not_exist_yet_is_after_the_newest_retained_one() {
        let w = window(&measured_kdox_wrap());
        assert!(w.is_after(w.newest().succ(), w.newest()));
    }

    #[test]
    fn a_listing_with_no_trusted_boundary_gap_is_still_resolved_deterministically() {
        // A near-full cycle: the largest gap (the wrap gap) is only ~50.
        let listing: Vec<VolumeSeq> = (1..=950).map(seq).collect();
        let w = VolumeWindow::from_listing(&listing).expect("still resolves");
        assert_eq!(w.newest(), seq(950));
        assert!(w.largest_gap() < MIN_TRUSTED_BOUNDARY_GAP);
    }
}
