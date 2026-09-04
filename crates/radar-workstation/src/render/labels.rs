//! City and radar-site label placement (S5-W6 §9, ADR-0028 §5 as amended by
//! §3.7): a greedy, rank-ordered, screen-space collision cull. Pure — no
//! egui types, no window — unit-tested the same way [`super::view`] and
//! [`super::input`] are.
//!
//! **One pass for both label sources (S5-g).** Radar site ICAO identifiers
//! and city names are both egui text at `Order::Background`, competing for
//! the same screen space; running them through separate passes would let a
//! city name land on a site identifier, which is the one collision that
//! matters most (the operator navigates by the site markers). So the
//! caller hands [`select`] one candidate slice with site labels first, at
//! rank 0, ahead of every city — this function trusts that ordering rather
//! than re-deriving it, since re-sorting is the caller's job and `rank` is
//! already "a dense ascending ordering the runtime never interprets beyond
//! ordering" (ADR-0028 §2).
//!
//! **Brute-force greedy, not a uniform grid (S5-h).** The pass is
//! self-limiting at a few hundred *placed* labels regardless of source
//! density — collision testing is candidate-against-placed, not
//! candidate-against-candidate — so a spatial index buys nothing at this
//! scale (ADR-0028 Measurement 4).

use super::view::{self, ViewState, Viewport};

/// ADR-0028 Measurement 4's box approximation: a magnitude, not a final
/// layout constant. `CHAR_W` is an average glyph advance at the proportional
/// font size `render::ui` draws labels with.
pub const CHAR_W: f32 = 6.6;
pub const LINE_H: f32 = 14.0;
pub const PAD: f32 = 3.0;

/// One label a caller wants placed, in world metres. `rank` is carried for
/// callers/tests to construct inputs with — `select` itself trusts the
/// slice's order, it does not re-sort by `rank`.
#[derive(Debug, Clone, Copy)]
pub struct LabelCandidate {
    pub world: [f32; 2],
    pub rank: u16,
    pub text: &'static str,
}

/// A label that survived placement, in screen pixels, ready for
/// `render::ui` to draw with `painter.text`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedLabel {
    pub screen: (f32, f32),
    pub text: &'static str,
}

#[derive(Debug, Clone, Copy)]
struct ScreenBox {
    min: (f32, f32),
    max: (f32, f32),
}

/// The box a candidate would occupy at `screen`, anchored left-bottom at
/// `screen + (5, -3)` (§9.1).
fn candidate_box(screen: (f32, f32), text: &str) -> ScreenBox {
    let w = CHAR_W * text.chars().count() as f32 + 2.0 * PAD;
    let h = LINE_H + 2.0 * PAD;
    let min_x = screen.0 + 5.0;
    let max_y = screen.1 - 3.0;
    ScreenBox { min: (min_x, max_y - h), max: (min_x + w, max_y) }
}

fn overlaps(a: &ScreenBox, b: &ScreenBox) -> bool {
    a.min.0 < b.max.0 && a.max.0 > b.min.0 && a.min.1 < b.max.1 && a.max.1 > b.min.1
}

fn within(b: &ScreenBox, avail: [f32; 4]) -> bool {
    b.min.0 >= avail[0] && b.min.1 >= avail[1] && b.max.0 <= avail[2] && b.max.1 <= avail[3]
}

/// Greedy, rank-ordered, screen-space collision cull. `avail` is the
/// chrome-free rectangle `(min_x, min_y, max_x, max_y)` in physical pixels.
/// `candidates` must already be sorted by priority (site labels at rank 0,
/// then cities in bundle order — the bake already sorted them, so this
/// function does not re-sort). A candidate is culled against `avail`
/// *before* placement — a label that would sit under the status bar or the
/// legend is never placed, rather than placed and then hidden — then
/// rejected if it overlaps any already-placed box; otherwise it is placed.
pub fn select(candidates: &[LabelCandidate], view: &ViewState, viewport: Viewport, avail: [f32; 4]) -> Vec<PlacedLabel> {
    debug_assert!(
        candidates.windows(2).all(|w| w[0].rank <= w[1].rank),
        "candidates must already be sorted by rank ascending"
    );
    let mut placed_boxes: Vec<ScreenBox> = Vec::new();
    let mut out = Vec::new();
    for c in candidates {
        let screen = view::world_to_screen((c.world[0] as f64, c.world[1] as f64), view, viewport);
        let candidate = candidate_box(screen, c.text);
        if !within(&candidate, avail) {
            continue;
        }
        if placed_boxes.iter().any(|p| overlaps(p, &candidate)) {
            continue;
        }
        placed_boxes.push(candidate);
        out.push(PlacedLabel { screen, text: c.text });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use radar_workstation::compute::DisplayProduct;

    const VP: Viewport = (1280.0, 800.0);
    const AVAIL: [f32; 4] = [0.0, 0.0, 1280.0, 780.0]; // status bar takes the bottom 20px

    fn view() -> ViewState {
        ViewState::initial(VP, DisplayProduct::Reflectivity)
    }

    #[test]
    fn two_colliding_candidates_place_only_the_higher_rank() {
        let v = view();
        // Both project to (near) the same screen point.
        let candidates = [
            LabelCandidate { world: [0.0, 0.0], rank: 0, text: "HIGH" },
            LabelCandidate { world: [1.0, 0.0], rank: 1, text: "LOW" },
        ];
        let placed = select(&candidates, &v, VP, AVAIL);
        assert_eq!(placed.len(), 1, "{placed:?}");
        assert_eq!(placed[0].text, "HIGH");
    }

    #[test]
    fn candidates_outside_the_available_rect_are_never_placed() {
        let v = view();
        // Far off in world space -> screen position outside VP entirely.
        let candidates = [LabelCandidate { world: [10_000_000.0, 10_000_000.0], rank: 0, text: "OFF" }];
        let placed = select(&candidates, &v, VP, AVAIL);
        assert!(placed.is_empty());
    }

    #[test]
    fn a_label_that_fits_beside_a_placed_one_is_placed() {
        let v = view();
        // Two well-separated world points -> non-overlapping screen boxes.
        let candidates = [
            LabelCandidate { world: [0.0, 0.0], rank: 0, text: "A" },
            LabelCandidate { world: [50_000.0, 0.0], rank: 1, text: "B" },
        ];
        let placed = select(&candidates, &v, VP, AVAIL);
        assert_eq!(placed.len(), 2, "{placed:?}");
    }

    #[test]
    fn dense_synthetic_input_self_limits() {
        // ADR-0028 §6: the shipped bundle (19 labels in a KDOX PPI) will
        // essentially never make the pass reject anything, so this is the
        // test that actually exercises rejection.
        let v = view();
        let mut candidates = Vec::new();
        let mut rank = 0u16;
        for gx in 0..44 {
            for gy in 0..44 {
                // A grid far finer than one label's box, in world metres.
                let world = [(gx as f32 - 22.0) * 3_000.0, (gy as f32 - 22.0) * 3_000.0];
                candidates.push(LabelCandidate { world, rank, text: "XXXXXX" });
                rank = rank.saturating_add(1);
            }
        }
        assert_eq!(candidates.len(), 1936, "44x44 grid, close enough to ADR-0028 §6's '~2,000 candidates'");
        let placed = select(&candidates, &v, VP, AVAIL);
        assert!(placed.len() < candidates.len(), "a dense grid must not all fit");
        assert!(!placed.is_empty(), "at least the first candidates should place");
        for i in 0..placed.len() {
            for j in (i + 1)..placed.len() {
                let a = candidate_box(placed[i].screen, placed[i].text);
                let b = candidate_box(placed[j].screen, placed[j].text);
                assert!(!overlaps(&a, &b), "placed boxes {i} and {j} overlap: {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn selection_is_deterministic() {
        let v = view();
        let candidates: Vec<LabelCandidate> = (0..50)
            .map(|i| LabelCandidate { world: [i as f32 * 5_000.0, 0.0], rank: i, text: "SITE" })
            .collect();
        let a = select(&candidates, &v, VP, AVAIL);
        let b = select(&candidates, &v, VP, AVAIL);
        assert_eq!(a, b);
    }
}
