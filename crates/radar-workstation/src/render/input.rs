//! winit key/pointer events → intent (S4-W4 §7.2). The key map is a pure
//! function over `winit::keyboard::KeyCode` so it is unit-testable without a
//! window or an event loop. `render::mod` translates each [`Action`] into a
//! `view`/selection change and a redraw request.
//!
//! Bindings follow GR2Analyst where it has a convention: `PageUp`/`PageDown`
//! for tilt, arrows for pan. Do not swap them.

use winit::keyboard::KeyCode;

use radar_workstation::compute::DisplayProduct;

/// One viewport-relative pan step per key press (§7.2: "one-eighth viewport
/// per press").
pub const PAN_STEP_FRACTION: f32 = 0.125;
/// Zoom-in / zoom-out multipliers applied to `m_per_px` per key press or
/// wheel notch.
pub const ZOOM_IN_FACTOR: f64 = 0.8;
pub const ZOOM_OUT_FACTOR: f64 = 1.25;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    SelectProduct(DisplayProduct),
    /// Next / previous elevation among those present, in angle order (§3.8:
    /// the selection is never silently rewritten — if the target tilt is
    /// absent the status bar says so).
    ElevationUp,
    ElevationDown,
    /// Pan the camera by a fraction of the viewport in each axis; `+fx` is
    /// east, `+fy` is south (screen-down).
    Pan { fx: f32, fy: f32 },
    ZoomIn,
    ZoomOut,
    ResetView,
    ToggleReference,
    ToggleHelp,
    Quit,
}

/// Map a physical key (plus whether Ctrl is held) to an [`Action`]. `None`
/// for keys this stage does not bind.
pub fn action_for_key(code: KeyCode, ctrl: bool) -> Option<Action> {
    if ctrl {
        return match code {
            KeyCode::KeyQ => Some(Action::Quit),
            _ => None,
        };
    }
    let product_by_index = |i: usize| DisplayProduct::ALL.get(i).copied().map(Action::SelectProduct);
    match code {
        KeyCode::Digit1 => product_by_index(0),
        KeyCode::Digit2 => product_by_index(1),
        KeyCode::Digit3 => product_by_index(2),
        KeyCode::Digit4 => product_by_index(3),
        KeyCode::Digit5 => product_by_index(4),
        KeyCode::Digit6 => product_by_index(5),
        KeyCode::Digit7 => product_by_index(6),
        KeyCode::PageUp => Some(Action::ElevationUp),
        KeyCode::PageDown => Some(Action::ElevationDown),
        KeyCode::ArrowLeft => Some(Action::Pan { fx: -PAN_STEP_FRACTION, fy: 0.0 }),
        KeyCode::ArrowRight => Some(Action::Pan { fx: PAN_STEP_FRACTION, fy: 0.0 }),
        KeyCode::ArrowUp => Some(Action::Pan { fx: 0.0, fy: -PAN_STEP_FRACTION }),
        KeyCode::ArrowDown => Some(Action::Pan { fx: 0.0, fy: PAN_STEP_FRACTION }),
        KeyCode::Equal | KeyCode::NumpadAdd => Some(Action::ZoomIn),
        KeyCode::Minus | KeyCode::NumpadSubtract => Some(Action::ZoomOut),
        KeyCode::Home => Some(Action::ResetView),
        KeyCode::KeyR => Some(Action::ToggleReference),
        KeyCode::F1 => Some(Action::ToggleHelp),
        _ => None,
    }
}

/// A typed character (winit `Key::Character`) → [`Action`], for the bindings
/// that are more naturally a glyph than a physical key: `?` for help, `+`
/// for zoom-in on keyboards where it is not an unshifted `Equal`.
pub fn action_for_char(s: &str) -> Option<Action> {
    match s {
        "?" => Some(Action::ToggleHelp),
        "+" => Some(Action::ZoomIn),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_one_through_seven_select_products_in_display_order() {
        assert_eq!(action_for_key(KeyCode::Digit1, false), Some(Action::SelectProduct(DisplayProduct::Reflectivity)));
        assert_eq!(action_for_key(KeyCode::Digit2, false), Some(Action::SelectProduct(DisplayProduct::Velocity)));
        assert_eq!(action_for_key(KeyCode::Digit6, false), Some(Action::SelectProduct(DisplayProduct::EchoTops)));
        assert_eq!(action_for_key(KeyCode::Digit7, false), Some(Action::SelectProduct(DisplayProduct::Vil)));
    }

    #[test]
    fn page_keys_change_tilt_and_arrows_pan() {
        assert_eq!(action_for_key(KeyCode::PageUp, false), Some(Action::ElevationUp));
        assert_eq!(action_for_key(KeyCode::PageDown, false), Some(Action::ElevationDown));
        assert!(matches!(action_for_key(KeyCode::ArrowLeft, false), Some(Action::Pan { fx, .. }) if fx < 0.0));
        assert!(matches!(action_for_key(KeyCode::ArrowDown, false), Some(Action::Pan { fy, .. }) if fy > 0.0));
    }

    #[test]
    fn zoom_reset_toggle_and_quit_bindings() {
        assert_eq!(action_for_key(KeyCode::Equal, false), Some(Action::ZoomIn));
        assert_eq!(action_for_key(KeyCode::Minus, false), Some(Action::ZoomOut));
        assert_eq!(action_for_key(KeyCode::Home, false), Some(Action::ResetView));
        assert_eq!(action_for_key(KeyCode::KeyR, false), Some(Action::ToggleReference));
        assert_eq!(action_for_key(KeyCode::F1, false), Some(Action::ToggleHelp));
        assert_eq!(action_for_char("?"), Some(Action::ToggleHelp));
    }

    #[test]
    fn quit_requires_ctrl() {
        assert_eq!(action_for_key(KeyCode::KeyQ, false), None);
        assert_eq!(action_for_key(KeyCode::KeyQ, true), Some(Action::Quit));
    }

    #[test]
    fn ctrl_suppresses_ordinary_bindings() {
        assert_eq!(action_for_key(KeyCode::Digit1, true), None);
        assert_eq!(action_for_key(KeyCode::ArrowLeft, true), None);
    }

    #[test]
    fn unbound_keys_map_to_nothing() {
        assert_eq!(action_for_key(KeyCode::KeyZ, false), None);
        assert_eq!(action_for_key(KeyCode::Digit8, false), None);
    }
}
