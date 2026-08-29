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

use super::time::{format_utc, utc_from_nexrad};
use super::view::{self, ViewState};
use super::reference;

/// Accent colour for states that want the operator's eye (a stall, an
/// active error). Everything else is greyscale.
const ACCENT: egui::Color32 = egui::Color32::from_rgb(255, 176, 64);

pub struct ChromeInput<'a> {
    pub site: &'static Site,
    pub snapshot: &'a StateSnapshot,
    pub view: &'a ViewState,
    /// The grid for the selected (product, elevation), if the current
    /// snapshot has one. `None` renders "no data on this cut" (§3.8) — the
    /// selection is *not* rewritten.
    pub selected_grid: Option<&'a SweepGrid>,
    pub selected_elevation_deg: Option<f32>,
    pub displayed_volume: Option<VolumeId>,
    pub palette: Option<&'a Palette>,
    /// Cursor position in world metres, when the pointer is over the map.
    pub cursor_world: Option<(f64, f64)>,
    pub recent_event: Option<String>,
    pub show_help: bool,
    pub now: Instant,
    /// Physical viewport size, for placing range-ring labels in screen space.
    pub viewport: (f32, f32),
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

            if let Some(vid) = input.displayed_volume {
                ui.label(format_utc(utc_from_nexrad(vid.julian_date, vid.scan_time_ms)));
                ui.separator();
            }

            match age_secs(input.snapshot.ingest.last_success, input.now) {
                Some(secs) => ui.label(format!("updated {secs}s ago")),
                None => ui.label("no data yet"),
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
}
