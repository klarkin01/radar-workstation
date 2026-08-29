//! View state and every screen↔world coordinate transform (S4-W4 §7.1).
//!
//! **Spatial stability (FR-NI-4, S4-g).** Nothing in this module reads
//! `AppState` or a `StateSnapshot`, and — by the rule stated in `render`'s
//! module doc — no function that *does* take a snapshot may take `&mut
//! ViewState`. `ViewState` is mutated only by the functions here, called
//! only from `render::input`. `view_state_is_unchanged_by_any_sequence_of_
//! state_updates` in `render` guards that boundary; the tests here guard the
//! transforms themselves.
//!
//! **Coordinate spaces.** *World* is metres offset from the active radar
//! site: `+x` east, `+y` north, `(0, 0)` the site. *Screen* is pixels with
//! the origin at the top-left, `+x` right, `+y` **down** — so the world→
//! screen map flips y. The azimuthal-equidistant projection itself lives in
//! `compute`/the shader; at the site-centred scales this stage draws, world
//! metres *are* projected metres.

use radar_workstation::compute::DisplayProduct;

/// Minimum metres-per-pixel (maximum zoom-in). At ~60 m/px one 250 m gate
/// spans a little over four pixels; finer than that just magnifies
/// quantisation, not data.
pub const MIN_M_PER_PX: f64 = 60.0;
/// Maximum metres-per-pixel (maximum zoom-out). At 3000 m/px roughly 600 km
/// spans the short axis of any window this stage supports — past the
/// longest measured WSR-88D tilt, so zooming out further only adds
/// background.
pub const MAX_M_PER_PX: f64 = 3000.0;

/// The conventional Level II display range: 230 km. The default view fits
/// this to the window's shorter half-extent, and the reference layer draws
/// an emphasised ring here.
pub const DEFAULT_RANGE_M: f64 = 230_000.0;

pub type Viewport = (f32, f32);

/// The per-frame camera the GPU passes need: where the view is centred, how
/// zoomed in it is, and how big the surface is. Bundled so the draw calls
/// stay under a sane argument count.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub center_m: (f64, f64),
    pub m_per_px: f64,
    pub viewport: Viewport,
}

impl Camera {
    pub fn from_view(v: &ViewState, viewport: Viewport) -> Self {
        Self { center_m: v.center_m, m_per_px: v.m_per_px, viewport }
    }
}

/// Render-loop-owned view state (ADR-0018, Q4): never enters `AppState`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewState {
    /// World point at the centre of the viewport, metres from the site.
    pub center_m: (f64, f64),
    pub m_per_px: f64,
    pub product: DisplayProduct,
    /// The operator's selected elevation *number* (not angle). `0` until the
    /// first sweep arrives and sets the default (§3.8).
    pub elevation_number: u8,
    pub show_reference: bool,
}

impl ViewState {
    /// The initial view: reflectivity, no elevation chosen yet, site-centred,
    /// 230 km fitted to the shorter window half-extent.
    pub fn initial(viewport: Viewport, product: DisplayProduct) -> Self {
        Self {
            center_m: (0.0, 0.0),
            m_per_px: fit_range(DEFAULT_RANGE_M, viewport),
            product,
            elevation_number: 0,
            show_reference: true,
        }
    }

    /// Reset navigation to the default (site-centred, 230 km) **without**
    /// touching the selected product or elevation — `Home` (§7.2).
    pub fn reset_navigation(&mut self, viewport: Viewport) {
        self.center_m = (0.0, 0.0);
        self.m_per_px = fit_range(DEFAULT_RANGE_M, viewport);
    }
}

/// Metres-per-pixel that fits `range_m` (a radius) to half the shorter
/// viewport dimension.
pub fn fit_range(range_m: f64, viewport: Viewport) -> f64 {
    let short = viewport.0.min(viewport.1).max(1.0) as f64;
    (range_m / (short / 2.0)).clamp(MIN_M_PER_PX, MAX_M_PER_PX)
}

/// Screen pixel → world metres.
pub fn screen_to_world(px: f32, py: f32, v: &ViewState, viewport: Viewport) -> (f64, f64) {
    let (vw, vh) = (viewport.0 as f64, viewport.1 as f64);
    let wx = v.center_m.0 + (px as f64 - vw / 2.0) * v.m_per_px;
    let wy = v.center_m.1 - (py as f64 - vh / 2.0) * v.m_per_px;
    (wx, wy)
}

/// World metres → screen pixel.
pub fn world_to_screen(world: (f64, f64), v: &ViewState, viewport: Viewport) -> (f32, f32) {
    let (vw, vh) = (viewport.0 as f64, viewport.1 as f64);
    let sx = vw / 2.0 + (world.0 - v.center_m.0) / v.m_per_px;
    let sy = vh / 2.0 - (world.1 - v.center_m.1) / v.m_per_px;
    (sx as f32, sy as f32)
}

/// Scroll the camera by `(dx, dy)` screen pixels: what was at screen
/// `(x, y)` is afterwards at `(x - dx, y - dy)`.
pub fn pan_by_pixels(v: &mut ViewState, dx: f32, dy: f32) {
    v.center_m.0 += dx as f64 * v.m_per_px;
    v.center_m.1 -= dy as f64 * v.m_per_px;
}

/// Zoom by `factor` (< 1 zooms in) about the cursor: the world point under
/// `(cursor_px, cursor_py)` is unchanged by the call. Clamped to
/// `[MIN_M_PER_PX, MAX_M_PER_PX]`.
pub fn zoom_about(v: &mut ViewState, cursor_px: f32, cursor_py: f32, factor: f64, viewport: Viewport) {
    let anchor = screen_to_world(cursor_px, cursor_py, v, viewport);
    v.m_per_px = (v.m_per_px * factor).clamp(MIN_M_PER_PX, MAX_M_PER_PX);
    let (vw, vh) = (viewport.0 as f64, viewport.1 as f64);
    v.center_m.0 = anchor.0 - (cursor_px as f64 - vw / 2.0) * v.m_per_px;
    v.center_m.1 = anchor.1 + (cursor_py as f64 - vh / 2.0) * v.m_per_px;
}

/// A window resize reveals or hides area; it never rescales the image
/// (FR-NI-4). The world point at the viewport centre is `center_m` by
/// construction and `m_per_px` is untouched, so this is a deliberate no-op
/// — kept as a named function so the guarantee has a test and a call site.
pub fn on_resize(_v: &mut ViewState, _old: Viewport, _new: Viewport) {}

#[cfg(test)]
mod tests {
    use super::*;

    const VP: Viewport = (1280.0, 800.0);

    fn view() -> ViewState {
        ViewState::initial(VP, DisplayProduct::Reflectivity)
    }

    #[test]
    fn screen_world_round_trip() {
        let v = view();
        for &(px, py) in &[(0.0, 0.0), (640.0, 400.0), (1279.0, 799.0), (123.0, 456.0)] {
            let (wx, wy) = screen_to_world(px, py, &v, VP);
            let (bx, by) = world_to_screen((wx, wy), &v, VP);
            assert!((bx - px).abs() < 0.01 && (by - py).abs() < 0.01, "({px},{py}) -> ({bx},{by})");
        }
    }

    #[test]
    fn viewport_centre_maps_to_center_m() {
        let mut v = view();
        v.center_m = (12_345.0, -6_789.0);
        let w = screen_to_world(VP.0 / 2.0, VP.1 / 2.0, &v, VP);
        assert!((w.0 - 12_345.0).abs() < 1e-6 && (w.1 - -6_789.0).abs() < 1e-6);
    }

    #[test]
    fn north_is_up() {
        let v = view();
        let above = screen_to_world(VP.0 / 2.0, VP.1 / 2.0 - 100.0, &v, VP);
        assert!(above.1 > 0.0, "a pixel above centre must be north (+y) of the site");
    }

    #[test]
    fn zoom_about_keeps_the_cursor_world_point_fixed() {
        let mut v = view();
        let (cx, cy) = (900.0, 250.0);
        let before = screen_to_world(cx, cy, &v, VP);
        zoom_about(&mut v, cx, cy, 0.5, VP);
        let after = screen_to_world(cx, cy, &v, VP);
        assert!((before.0 - after.0).abs() < 1.0 && (before.1 - after.1).abs() < 1.0, "{before:?} vs {after:?}");
    }

    #[test]
    fn zoom_clamps_hold_at_both_ends() {
        let mut v = view();
        for _ in 0..100 {
            zoom_about(&mut v, 640.0, 400.0, 0.5, VP);
        }
        assert!((v.m_per_px - MIN_M_PER_PX).abs() < 1e-6);
        for _ in 0..100 {
            zoom_about(&mut v, 640.0, 400.0, 2.0, VP);
        }
        assert!((v.m_per_px - MAX_M_PER_PX).abs() < 1e-6);
    }

    #[test]
    fn pan_moves_the_camera_the_expected_direction() {
        let mut v = view();
        let (x0, _) = v.center_m;
        pan_by_pixels(&mut v, 100.0, 0.0);
        assert!(v.center_m.0 > x0, "panning the camera +x must increase center east");
        let (_, y0) = v.center_m;
        pan_by_pixels(&mut v, 0.0, 100.0); // screen y is down = south
        assert!(v.center_m.1 < y0, "panning the camera +y (down) must move center south");
    }

    #[test]
    fn on_resize_preserves_centre_and_scale() {
        let mut v = view();
        v.center_m = (5_000.0, 7_000.0);
        let scale = v.m_per_px;
        on_resize(&mut v, VP, (1920.0, 1080.0));
        assert_eq!(v.center_m, (5_000.0, 7_000.0));
        assert_eq!(v.m_per_px, scale);
    }

    #[test]
    fn reset_navigation_leaves_product_and_elevation_alone() {
        let mut v = view();
        v.product = DisplayProduct::Velocity;
        v.elevation_number = 4;
        v.center_m = (99_000.0, 99_000.0);
        v.reset_navigation(VP);
        assert_eq!(v.center_m, (0.0, 0.0));
        assert_eq!(v.product, DisplayProduct::Velocity);
        assert_eq!(v.elevation_number, 4);
    }

    #[test]
    fn default_view_fits_230_km_to_the_short_axis() {
        let v = view();
        // 230 km should reach the top/bottom edge (short axis = 800).
        let edge = screen_to_world(VP.0 / 2.0, 0.0, &v, VP);
        assert!((edge.1 - DEFAULT_RANGE_M).abs() < 1_000.0, "top edge at ~230 km, got {}", edge.1);
    }
}
