//! egui chrome (S4-W6 §9): status bar, colour-scale legend, cursor readout,
//! loading indicator, and a key-help overlay. Drawn last, on top of the wgpu
//! output, every frame (rendering.md). Quiet: dark, no shadows, one accent
//! colour — the Instrument Principle applied to the UI itself.

use std::time::Instant;

use radar_workstation::assembly::VolumeId;
use radar_workstation::compute::grid::azimuth_slot;
use radar_workstation::compute::palette::Palette;
use radar_workstation::compute::{geometry, DisplayProduct, SweepGrid};
use radar_workstation::ingest::s3_poll::{IngestState, IngestStatus};
use radar_workstation::sites::Site;
use radar_workstation::state::StateSnapshot;

use radar_workstation::time::{format_utc, unix_secs_from_nexrad, utc_from_nexrad};
use radar_workstation::vcp;

use super::labels::PlacedLabel;
use super::view::{self, ViewState};
use super::reference;

/// Accent colour for states that want the operator's eye (a stall, an
/// active error). Everything else is greyscale.
const ACCENT: egui::Color32 = egui::Color32::from_rgb(255, 176, 64);

/// Identity of the sweep actually on screen — the only honest source for a
/// data-age readout (plan §1.4). Both fields come from the same
/// `DisplaySweep`, in one `find`, so the identity and its VCP cannot drift
/// apart.
#[derive(Debug, Clone, Copy)]
pub struct DisplayedScan {
    pub volume: VolumeId,
    pub vcp_number: u16,
}

pub struct ChromeInput<'a> {
    pub site: &'static Site,
    pub snapshot: &'a StateSnapshot,
    pub view: &'a ViewState,
    /// The grid for the selected (product, elevation), if the current
    /// snapshot has one. `None` renders "no data on this cut" (§3.8) — the
    /// selection is *not* rewritten.
    pub selected_grid: Option<&'a SweepGrid>,
    pub selected_elevation_deg: Option<f32>,
    pub displayed_scan: Option<DisplayedScan>,
    pub palette: Option<&'a Palette>,
    /// Cursor position in world metres, when the pointer is over the map.
    pub cursor_world: Option<(f64, f64)>,
    pub recent_event: Option<String>,
    pub show_help: bool,
    /// Monotonic clock, for poll health. Must not become wall-clock-sensitive.
    pub now: Instant,
    /// Wall-clock UTC seconds since the Unix epoch, for the data-age readout.
    /// A pre-epoch clock maps to `0`.
    pub now_unix: i64,
    /// Physical viewport size, for placing range-ring labels in screen space.
    pub viewport: (f32, f32),
    /// This frame's declutter-selected city/site labels (§9.3), already in
    /// screen space — computed once per `redraw` by
    /// `render::labels::select`, not here.
    pub placed_labels: &'a [PlacedLabel],
}

/// What the grid holds under the cursor (§9.3). The two sentinels ADR-0020
/// preserved through gridding exist so this readout can tell them apart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CursorSample {
    NoData,
    RangeFolded,
    Value(f32),
    OutsideCoverage,
}

/// Sample `grid` at a ground range / azimuth. Pure — unit-tested without a
/// window. Uses the same slant-range conversion and azimuth binning as the
/// shader and `compute`.
pub fn sample_at(grid: &SweepGrid, ground_m: f64, az_deg: f64) -> CursorSample {
    let axis_m = if matches!(grid.product, DisplayProduct::EchoTops | DisplayProduct::Vil) {
        ground_m
    } else {
        geometry::slant_range_and_height(ground_m, grid.elevation_deg as f64).0
    };
    if grid.gate_width_m == 0 {
        return CursorSample::OutsideCoverage;
    }
    let gate = ((axis_m - grid.first_gate_m as f64) / grid.gate_width_m as f64).floor();
    if !gate.is_finite() || gate < 0.0 || gate >= grid.gate_count as f64 {
        return CursorSample::OutsideCoverage;
    }
    let slot = azimuth_slot(az_deg.rem_euclid(360.0) as f32, grid.azimuth_count);
    match grid.cell(slot, gate as u16) {
        0 => CursorSample::NoData,
        1 => CursorSample::RangeFolded,
        raw => CursorSample::Value((raw as f32 - grid.offset) / grid.scale),
    }
}

fn age_secs(from: Option<Instant>, now: Instant) -> Option<u64> {
    from.map(|t| now.saturating_duration_since(t).as_secs())
}

/// Age of the data on screen, in seconds of wall-clock UTC. `None` when
/// nothing is displayed. A volume timestamped in the future (clock skew, or a
/// corrupt header) clamps to 0 rather than reading as negative.
fn data_age_secs(displayed: Option<DisplayedScan>, now_unix: i64) -> Option<i64> {
    displayed.map(|scan| {
        let scan_unix = unix_secs_from_nexrad(scan.volume.julian_date, scan.volume.scan_time_ms);
        (now_unix - scan_unix).max(0)
    })
}

/// "42s", "7m 12s", "21h 03m" — coarser as the number gets larger, because at
/// 21 hours the seconds are noise.
fn format_age(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Past two nominal VCP cycles the display is no longer current in any
/// operationally useful sense (plan §2.6).
fn age_is_alarming(secs: i64, vcp_number: u16) -> bool {
    let limit = 2 * vcp::nominal_volume_duration(vcp_number).as_secs() as i64;
    secs > limit
}

fn ingest_label(status: &IngestStatus) -> (String, bool) {
    match status.state {
        IngestState::Polling => ("polling".to_string(), false),
        IngestState::Retrying { attempts } => (format!("retrying ({attempts})"), false),
        IngestState::Stalled => ("STALLED".to_string(), true),
        IngestState::ReAnchoring => ("re-anchoring".to_string(), false),
    }
}

pub fn draw(ui: &mut egui::Ui, input: &ChromeInput) {
    let ctx = ui.ctx().clone();
    status_bar(ui, input);
    if let Some(palette) = input.palette {
        legend(ui, palette, input.selected_grid, input.view.product);
    }
    if input.view.show_reference {
        ring_labels(&ctx, input);
    }
    // City/site labels (layer 9, §9.3): no toggle, unlike range rings — a
    // site marker has no way to be read without its identifier, and FR-DR-3
    // only marks layer 5 (highways) toggleable.
    city_labels(&ctx, input.placed_labels);
    if input.snapshot.sweeps.is_empty() {
        loading_indicator(&ctx, input);
    } else {
        cursor_readout(&ctx, input);
    }
    if input.show_help {
        help_overlay(&ctx);
    }
}

/// Range-ring labels ("50", "100", …, "230 km"), drawn by egui at screen
/// positions from `view::world_to_screen` so text never touches the wgpu
/// side (§8).
fn ring_labels(ctx: &egui::Context, input: &ChromeInput) {
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::new("ring_labels")));
    let color = egui::Color32::from_rgba_unmultiplied(180, 190, 205, 150);
    for (world, label) in reference::ring_labels() {
        let (sx, sy) = view::world_to_screen((world[0] as f64, world[1] as f64), input.view, input.viewport);
        if sx < 0.0 || sy < 0.0 || sx > input.viewport.0 || sy > input.viewport.1 {
            continue;
        }
        painter.text(
            egui::pos2(sx, sy),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(11.0),
            color,
        );
    }
}

/// City names and radar site identifiers (layer 9, §9.3), already placed in
/// screen space by `render::labels::select`. Two `painter.text` calls per
/// label — a near-black copy offset by `(1, 1)` under the light text — so a
/// name stays legible over saturated reflectivity. That doubles ADR-0028
/// Measurement 3's per-label cost (500 labels: 0.108 ms -> ~0.22 ms against
/// a 16.7 ms frame budget), a trade worth stating here so a later reader
/// does not "simplify" the shadow away.
fn city_labels(ctx: &egui::Context, placed: &[PlacedLabel]) {
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::new("city_labels")));
    let shadow = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 190);
    let text_color = egui::Color32::from_rgba_unmultiplied(215, 220, 230, 235);
    let font = egui::FontId::proportional(12.0);
    for label in placed {
        // The same left-bottom anchor at `point + (5, -3)` that
        // `render::labels::candidate_box` reserved screen space for.
        let pos = egui::pos2(label.screen.0 + 5.0, label.screen.1 - 3.0);
        painter.text(pos + egui::vec2(1.0, 1.0), egui::Align2::LEFT_BOTTOM, label.text, font.clone(), shadow);
        painter.text(pos, egui::Align2::LEFT_BOTTOM, label.text, font.clone(), text_color);
    }
}

fn status_bar(root: &mut egui::Ui, input: &ChromeInput) {
    egui::Panel::bottom("status_bar").show(root, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("{} — {}", input.site.id, input.site.name));
            ui.separator();
            ui.label(input.view.product.to_string());
            ui.separator();
            match (input.selected_grid, input.selected_elevation_deg) {
                (Some(_), Some(deg)) => {
                    ui.label(format!("el {} ({:.2}°)", input.view.elevation_number, deg));
                }
                _ => {
                    ui.colored_label(ACCENT, format!("el {} — no data on this cut", input.view.elevation_number));
                }
            }
            ui.separator();

            // Data age — now minus the *displayed scan's own* scan time, the
            // one honest freshness number (plan §1.4).
            match (input.displayed_scan, data_age_secs(input.displayed_scan, input.now_unix)) {
                (Some(scan), Some(age)) => {
                    ui.label(format_utc(utc_from_nexrad(
                        scan.volume.julian_date,
                        scan.volume.scan_time_ms,
                    )));
                    ui.separator();
                    let text = format!("age {}", format_age(age));
                    if age_is_alarming(age, scan.vcp_number) {
                        ui.colored_label(ACCENT, text);
                    } else {
                        ui.label(text);
                    }
                }
                _ => {
                    ui.label("no data yet");
                }
            }
            ui.separator();

            // Poll health — kept, but no longer the freshness number.
            match age_secs(input.snapshot.ingest.last_success, input.now) {
                Some(secs) => ui.label(format!("poll {secs}s ago")),
                None => ui.label("poll: none yet"),
            };
            ui.separator();

            let (label, alert) = ingest_label(&input.snapshot.ingest);
            if alert {
                ui.colored_label(ACCENT, label);
            } else {
                ui.label(label);
            }

            if let Some(event) = &input.recent_event {
                ui.separator();
                ui.colored_label(ACCENT, event);
            }
        });
    });
}

fn legend(root: &mut egui::Ui, palette: &Palette, grid: Option<&SweepGrid>, product: DisplayProduct) {
    egui::Panel::right("legend").resizable(false).exact_size(84.0).show(root, |ui| {
        ui.add_space(4.0);
        if !palette.units.is_empty() {
            ui.label(&palette.units);
        }
        let Some((lo, hi)) = palette.threshold_range() else { return };
        let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, ui.available_height() - 60.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        let steps = rect.height().max(1.0) as usize;
        for i in 0..steps {
            let t = i as f32 / steps as f32;
            let value = hi - t * (hi - lo); // top = high
            let [r, g, b, a] = palette.sample(value);
            let y = rect.top() + t * rect.height();
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(rect.left(), y), egui::vec2(rect.width(), 2.0)),
                0.0,
                egui::Color32::from_rgba_unmultiplied(r, g, b, a),
            );
        }
        // Tick labels at Step: intervals (fallback: 4 divisions).
        let step = palette.step.filter(|s| *s > 0.0).unwrap_or((hi - lo) / 4.0);
        let mut v = lo;
        while v <= hi + 0.001 {
            let frac = (hi - v) / (hi - lo);
            let y = rect.top() + frac * rect.height();
            painter.text(
                egui::pos2(rect.right() + 4.0, y),
                egui::Align2::LEFT_CENTER,
                format!("{v:.0}"),
                egui::FontId::proportional(11.0),
                ui.visuals().text_color(),
            );
            v += step;
        }

        // Velocity: state the fold limit Q9 promised (§9.2).
        if product == DisplayProduct::Velocity {
            ui.add_space(6.0);
            if let Some(nyq) = grid.and_then(|g| g.nyquist_velocity_mps) {
                ui.label(format!("±{nyq:.1} m/s"));
            }
            ui.horizontal(|ui| {
                let (sw, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                let [r, g, b, a] = palette.range_folded;
                ui.painter().rect_filled(sw, 2.0, egui::Color32::from_rgba_unmultiplied(r, g, b, a));
                ui.label("RF");
            });
        }
    });
}

fn cursor_readout(ctx: &egui::Context, input: &ChromeInput) {
    let (Some((wx, wy)), Some(pos)) = (input.cursor_world, ctx.pointer_latest_pos()) else { return };
    let ground_m = (wx * wx + wy * wy).sqrt();
    let az_deg = wx.atan2(wy).to_degrees().rem_euclid(360.0);
    let (slant_m, height_m) = geometry::slant_range_and_height(
        ground_m,
        input.selected_grid.map(|g| g.elevation_deg as f64).unwrap_or(0.5),
    );
    let _ = slant_m;
    let height_kft = height_m / 0.3048 / 1000.0;

    let value_line = match input.selected_grid.map(|g| sample_at(g, ground_m, az_deg)) {
        Some(CursorSample::Value(v)) => {
            let units = input.palette.map(|p| p.units.as_str()).unwrap_or("");
            format!("{v:.1} {units}")
        }
        Some(CursorSample::NoData) => "ND".to_string(),
        Some(CursorSample::RangeFolded) => "RF".to_string(),
        Some(CursorSample::OutsideCoverage) | None => "—".to_string(),
    };

    egui::Area::new(egui::Id::new("cursor_readout"))
        .fixed_pos(pos + egui::vec2(16.0, 16.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(format!("{:.1} km  {:03.0}°", ground_m / 1000.0, az_deg));
                ui.label(format!("{height_kft:.1} kft"));
                ui.label(value_line);
            });
        });
}

fn loading_indicator(ctx: &egui::Context, input: &ChromeInput) {
    egui::Area::new(egui::Id::new("loading"))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.heading(format!("Waiting for {} …", input.site.id));
                    let (label, _) = ingest_label(&input.snapshot.ingest);
                    ui.label(label);
                });
            });
        });
}

fn help_overlay(ctx: &egui::Context) {
    egui::Window::new("Keys")
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            for (k, v) in [
                ("1–7", "product: reflectivity, velocity, spectrum width, ZDR, CC, echo tops, VIL"),
                ("PageUp / PageDown", "next / previous elevation"),
                ("Arrows", "pan"),
                ("+ / -  or  wheel", "zoom (wheel zooms about the cursor)"),
                ("Left-drag", "pan"),
                ("Home", "reset view (selection kept)"),
                ("R", "toggle range rings / spokes"),
                ("H", "toggle highways"),
                ("F1 or ?", "toggle this overlay"),
                ("Ctrl+Q", "quit"),
            ] {
                ui.horizontal(|ui| {
                    ui.monospace(k);
                    ui.label(v);
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn grid(product: DisplayProduct) -> Arc<SweepGrid> {
        // 4 azimuths, 4 gates, 1 km gates starting at 0. cell(slot,gate).
        let mut cells = vec![2u8; 16];
        cells[0] = 0; // slot 0 gate 0 -> ND
        cells[1] = 1; // slot 0 gate 1 -> RF
        Arc::new(SweepGrid {
            product,
            azimuth_count: 4,
            gate_count: 4,
            first_gate_m: 0,
            gate_width_m: 1000,
            elevation_number: 1,
            elevation_deg: 0.5,
            nyquist_velocity_mps: Some(8.0),
            scale: 2.0,
            offset: 66.0,
            cells,
            filled_azimuths: 4,
        })
    }

    #[test]
    fn sample_distinguishes_nd_rf_and_value() {
        let g = grid(DisplayProduct::Reflectivity);
        // slot 0 is azimuth [0,90); pick az 10. gate 0 ~ ground 0.
        assert_eq!(sample_at(&g, 100.0, 10.0), CursorSample::NoData);
        // gate 1 ~ 1 km ground (slant ~ 1 km at 0.5 deg).
        assert_eq!(sample_at(&g, 1_000.0, 10.0), CursorSample::RangeFolded);
        // gate 2 ~ 2 km -> raw 2 -> (2 - 66)/2 = -32.0
        assert_eq!(sample_at(&g, 2_000.0, 10.0), CursorSample::Value(-32.0));
    }

    #[test]
    fn sample_beyond_the_last_gate_is_outside_coverage() {
        let g = grid(DisplayProduct::Reflectivity);
        assert_eq!(sample_at(&g, 500_000.0, 0.0), CursorSample::OutsideCoverage);
    }

    #[test]
    fn derived_products_sample_on_the_ground_axis() {
        let g = grid(DisplayProduct::EchoTops);
        // ground 2 km -> gate 2 directly, no slant correction.
        assert_eq!(sample_at(&g, 2_000.0, 10.0), CursorSample::Value(-32.0));
    }

    #[test]
    fn age_secs_is_none_before_the_first_success() {
        assert_eq!(age_secs(None, Instant::now()), None);
    }

    fn scan(julian_date: u16, scan_time_ms: u32, vcp_number: u16) -> DisplayedScan {
        DisplayedScan { volume: VolumeId { julian_date, scan_time_ms }, vcp_number }
    }

    #[test]
    fn data_age_is_measured_from_the_displayed_scan_not_the_last_poll() {
        // Parent §1.4: a volume scanned 2026-09-03T03:08Z, read
        // 2026-09-04T00:55Z. The poll readout would still say ~3 s; the honest
        // number is ~21 h, and it alarms.
        let now_unix = 20_700 * 86_400 + (55 * 60); // 2026-09-04T00:55:00Z
        let displayed = scan(20_700, 3 * 3_600_000 + 8 * 60_000, 35); // 2026-09-03T03:08Z
        let age = data_age_secs(Some(displayed), now_unix).unwrap();
        assert!((78_000..79_000).contains(&age), "expected ~21.8 h, got {age}s");
        assert!(age_is_alarming(age, displayed.vcp_number));
    }

    #[test]
    fn data_age_of_a_future_timestamped_volume_clamps_to_zero() {
        let now_unix = 20_700 * 86_400;
        let displayed = scan(20_701, 0, 35); // a full day in the future
        assert_eq!(data_age_secs(Some(displayed), now_unix), Some(0));
    }

    #[test]
    fn data_age_is_none_when_no_scan_is_displayed() {
        assert_eq!(data_age_secs(None, 20_700 * 86_400), None);
    }

    #[test]
    fn age_is_alarming_past_two_nominal_vcp_cycles() {
        // VCP 35: nominal cycle 7 min, so the alarm threshold is 14 min.
        assert!(age_is_alarming(14 * 60 + 1, 35));
    }

    #[test]
    fn age_is_not_alarming_within_one_cycle() {
        assert!(!age_is_alarming(6 * 60, 35));
        assert!(!age_is_alarming(13 * 60, 35));
    }

    #[test]
    fn format_age_switches_units_at_the_right_magnitudes() {
        assert_eq!(format_age(42), "42s");
        assert_eq!(format_age(432), "7m 12s");
        assert_eq!(format_age(21 * 3600 + 3 * 60 + 40), "21h 03m");
        assert_eq!(format_age(-5), "0s");
    }
}
