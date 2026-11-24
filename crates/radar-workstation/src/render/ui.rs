//! egui chrome (S4-W6 §9): status bar, timeline scrubber (Stage 6a Part C),
//! colour-scale legend, cursor readout, loading indicator, and a key-help
//! overlay. Drawn last, on top of the wgpu output, every frame
//! (rendering.md). Quiet: dark, no shadows, one accent colour for alerts and
//! one for the timeline — the Instrument Principle applied to the UI itself.
//!
//! Everything that depends on *which* data is on screen reads the frame's
//! one [`Resolved`] (ADR-0031 D-4), so the radar draw, the status bar, the
//! legend and the cursor readout can never disagree about it.

use std::time::Instant;

use radar_workstation::assembly::VolumeId;
use radar_workstation::compute::cut::CutKey;
use radar_workstation::compute::grid::azimuth_slot;
use radar_workstation::compute::palette::Palette;
use radar_workstation::compute::{geometry, DisplayProduct, SweepGrid};
use radar_workstation::ingest::s3_poll::{IngestState, IngestStatus};
use radar_workstation::sites::Site;
use radar_workstation::state::StateSnapshot;

use radar_workstation::time::{format_utc, unix_secs_from_nexrad, utc_from_nexrad};
use radar_workstation::vcp;

use super::labels::PlacedLabel;
use super::reference;
use super::timeline::{self, DisplayedScan, ModeKind, Resolved};
use super::view::{self, ViewState};

/// Accent colour for states that want the operator's eye (a stall, an
/// active error). Everything else is greyscale.
const ACCENT: egui::Color32 = egui::Color32::from_rgb(255, 176, 64);

/// The timeline's own colour (ADR-0031 D-9): the NOT-LIVE badge, the
/// scrubber's position marker and timeline notices. Deliberately unlike
/// [`ACCENT`] — being in the past on purpose is not an alert.
const TIMELINE: egui::Color32 = egui::Color32::from_rgb(96, 200, 255);

pub struct ChromeInput<'a> {
    pub site: &'static Site,
    pub snapshot: &'a StateSnapshot,
    pub view: &'a ViewState,
    /// This frame's timeline resolution. `resolved.grid == None` renders
    /// "no data" (§3.8, D-4) — the selection is *not* rewritten.
    pub resolved: &'a Resolved,
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

/// What the operator did to the chrome this frame — the render loop applies
/// it to `ViewState` after the frame (operator input, D-12).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChromeOutput {
    /// A scrubber click or drag landed on this frame (D-4: pin it).
    pub scrub_to: Option<VolumeId>,
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
    let axis_m = if grid.product.is_derived() {
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

/// The age readout (D-9): always the **live feed** (`resolved.live_scan`),
/// with the two-VCP-cycle alert, labelled `live age` when the display is
/// not live. Being in the past on purpose never fires the alert.
fn age_label(r: &Resolved, now_unix: i64) -> Option<(String, bool)> {
    let scan = r.live_scan?;
    let age = data_age_secs(Some(scan), now_unix)?;
    let prefix = if r.kind == ModeKind::Live { "age" } else { "live age" };
    Some((format!("{prefix} {}", format_age(age)), age_is_alarming(age, scan.vcp_number)))
}

fn scan_unix(volume: VolumeId) -> i64 {
    unix_secs_from_nexrad(volume.julian_date, volume.scan_time_ms)
}

/// `21:04Z` — a volume named in a sentence.
fn hhmm(volume: VolumeId) -> String {
    let (_, _, _, h, m, _) = utc_from_nexrad(volume.julian_date, volume.scan_time_ms);
    format!("{h:02}:{m:02}Z")
}

/// The mode badge (FR-TL-6, D-9): `LIVE`, or `PAUSED` / `LOOP` with the
/// displayed volume's full UTC time and how far it is behind live.
fn badge_text(r: &Resolved) -> String {
    let label = match r.kind {
        ModeKind::Live => return "LIVE".to_string(),
        ModeKind::Paused => "PAUSED",
        ModeKind::Playing => "LOOP",
    };
    let Some(scan) = r.scan else { return label.to_string() };
    let time = format_utc(utc_from_nexrad(scan.volume.julian_date, scan.volume.scan_time_ms));
    let live = r.live_scan.map(|s| s.volume).or(r.positions.last().map(|p| p.volume));
    match live {
        Some(live) => {
            let behind_min = (scan_unix(live) - scan_unix(scan.volume)).max(0) / 60;
            format!("{label} {time} \u{2212}{behind_min} min")
        }
        None => format!("{label} {time}"),
    }
}

fn product_label(product: DisplayProduct) -> String {
    product.to_string().to_uppercase()
}

fn selection_label(product: DisplayProduct, cut: Option<CutKey>) -> String {
    match cut {
        Some(c) if !product.is_derived() => format!("{} at {:.1}\u{b0}", product_label(product), c.degrees()),
        _ => product_label(product),
    }
}

/// The tilt field: commanded cut, then the measured angle of what is drawn.
fn tilt_label(product: DisplayProduct, r: &Resolved) -> String {
    if product.is_derived() {
        return "volume".to_string();
    }
    match (r.cut, &r.grid) {
        (Some(c), Some(g)) => format!("{:.1}\u{b0} ({:.2}\u{b0} meas)", c.degrees(), g.elevation_deg),
        (Some(c), None) => format!("{:.1}\u{b0}", c.degrees()),
        (None, _) => "\u{2014}".to_string(),
    }
}

/// "No VEL at 0.5° in the 21:04Z volume" (D-4): the selection is absent at
/// this position. `None` when there is a grid, or no data at all yet.
fn absence_message(product: DisplayProduct, r: &Resolved) -> Option<String> {
    if r.grid.is_some() || r.positions.is_empty() {
        return None;
    }
    let what = format!("No {}", selection_label(product, r.cut));
    Some(match (r.kind, r.scan) {
        (ModeKind::Live, _) | (_, None) => what,
        (_, Some(scan)) => format!("{what} in the {} volume", hhmm(scan.volume)),
    })
}

/// D-7 / D-8 notices, in the timeline colour.
fn timeline_notices(input: &ChromeInput) -> Vec<String> {
    let r = input.resolved;
    let mut out = Vec::new();
    if let Some(pinned) = r.pinned_evicted {
        out.push(format!("Pinned {} volume aged out; showing oldest retained", hhmm(pinned)));
    }
    if input.view.timeline.play_refused {
        out.push(format!(
            "Loop needs \u{2265} 2 volumes with {}; {} in the loop window",
            selection_label(input.view.product, r.cut),
            timeline::playable_count(r)
        ));
    }
    out
}

fn ingest_label(status: &IngestStatus) -> (String, bool) {
    match status.state {
        IngestState::Polling => ("polling".to_string(), false),
        IngestState::Retrying { attempts } => (format!("retrying ({attempts})"), false),
        IngestState::Stalled => ("STALLED".to_string(), true),
        IngestState::ReAnchoring => ("re-anchoring".to_string(), false),
    }
}

pub fn draw(ui: &mut egui::Ui, input: &ChromeInput) -> ChromeOutput {
    let ctx = ui.ctx().clone();
    status_bar(ui, input);
    // Added after the status bar, so egui stacks it directly above it.
    let scrub_to = scrubber(ui, input.resolved);
    if let Some(palette) = input.palette {
        legend(ui, palette, input.resolved.grid.as_deref(), input.view.product);
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
    ChromeOutput { scrub_to }
}

/// The timeline row (8.1/8.2): one tick per retained frame, hollow where the
/// frame lacks the selection (D-4), the loop window shaded, the frame on
/// screen ringed. Shown only once two frames are retained — with
/// `history.budget_mb = 0` there is never a row. A click pins the frame
/// under the pointer and a drag scrubs; the panel is an egui area, so egui
/// consumes the press and the map does not pan.
fn scrubber(root: &mut egui::Ui, r: &Resolved) -> Option<VolumeId> {
    if r.positions.len() < 2 {
        return None;
    }
    let mut picked = None;
    egui::Panel::bottom("timeline").show(root, |ui| {
        // Click and drag, but *not* `FOCUSABLE` (which `Sense::click_and_drag`
        // adds): a focused widget makes egui-winit consume every key press,
        // so one scrubber click would silently disable the keyboard map.
        let sense = egui::Sense::CLICK | egui::Sense::DRAG;
        let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 16.0), sense);
        let painter = ui.painter_at(rect);
        let n = r.positions.len();
        let step = rect.width() / n as f32;
        let x_of = |i: usize| rect.left() + (i as f32 + 0.5) * step;

        let first = r.positions.iter().position(|p| p.in_loop);
        let last = r.positions.iter().rposition(|p| p.in_loop);
        if let (Some(first), Some(last)) = (first, last) {
            let span = egui::Rect::from_x_y_ranges(x_of(first) - step / 2.0..=x_of(last) + step / 2.0, rect.y_range());
            painter.rect_filled(span, 3.0, egui::Color32::from_rgba_unmultiplied(96, 200, 255, 28));
        }

        let tick = egui::Color32::from_gray(190);
        let current = timeline::current_index(r);
        for (i, p) in r.positions.iter().enumerate() {
            let c = egui::pos2(x_of(i), rect.center().y);
            if p.satisfies {
                painter.circle_filled(c, 3.5, tick);
            } else {
                painter.circle_stroke(c, 3.5, egui::Stroke::new(1.0, tick));
            }
            if Some(i) == current {
                painter.circle_stroke(c, 6.5, egui::Stroke::new(1.5, TIMELINE));
            }
        }

        if response.clicked() || response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                let i = ((pos.x - rect.left()) / step).floor().clamp(0.0, (n - 1) as f32) as usize;
                picked = Some(r.positions[i].volume);
            }
        }
    });
    picked
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
    let r = input.resolved;
    let product = input.view.product;
    egui::Panel::bottom("status_bar").show(root, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("{} — {}", input.site.id, input.site.name));
            ui.separator();
            ui.label(product.to_string());
            ui.separator();
            ui.label(tilt_label(product, r));
            ui.separator();

            // FR-TL-6: always visible. NOT-LIVE is filled in the timeline
            // colour so it cannot be mistaken for live at a glance.
            let badge = egui::RichText::new(badge_text(r)).strong();
            if r.kind == ModeKind::Live {
                ui.label(badge);
            } else {
                ui.label(badge.color(egui::Color32::BLACK).background_color(TIMELINE));
            }
            ui.separator();

            // Scan time: the *displayed* volume. Age: the *live feed*, with
            // the stale alert depending on nothing else (D-9, FR-TL-7).
            match r.scan {
                Some(scan) => {
                    ui.label(format_utc(utc_from_nexrad(scan.volume.julian_date, scan.volume.scan_time_ms)));
                }
                None => {
                    ui.label("no data yet");
                }
            }
            if let Some((text, alarming)) = age_label(r, input.now_unix) {
                ui.separator();
                if alarming {
                    ui.colored_label(ACCENT, text);
                } else {
                    ui.label(text);
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

            if let Some(message) = absence_message(product, r) {
                ui.separator();
                ui.colored_label(ACCENT, message);
            }
            for notice in timeline_notices(input) {
                ui.separator();
                ui.colored_label(TIMELINE, notice);
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
        input.resolved.grid.as_ref().map(|g| g.elevation_deg as f64).unwrap_or(0.5),
    );
    let _ = slant_m;
    let height_kft = height_m / 0.3048 / 1000.0;

    let value_line = match input.resolved.grid.as_ref().map(|g| sample_at(g, ground_m, az_deg)) {
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
                ("PageUp / PageDown", "next / previous elevation angle"),
                ("Arrows", "pan"),
                ("+ / -  or  wheel", "zoom (wheel zooms about the cursor)"),
                ("Left-drag", "pan"),
                ("Home", "reset view (selection kept)"),
                ("R", "toggle range rings / spokes"),
                ("H", "toggle highways"),
                ("Space", "play / pause the loop"),
                ("[ / ]", "step back / forward one volume (] past the newest returns to live)"),
                ("End", "jump to live"),
                (", / .", "loop one volume shorter / longer (2–64)"),
                ("Click / drag timeline", "pin / scrub a volume"),
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
            cut_deg: 0.5,
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

    // --- timeline chrome (Stage 6a Part C, plan §8.5) ---

    /// A volume `minutes_ago` before `now_unix`, on julian day 20_700.
    fn volume_ago(now_unix: i64, minutes_ago: i64) -> VolumeId {
        let day_start = (20_700 - 1) as i64 * 86_400;
        VolumeId { julian_date: 20_700, scan_time_ms: ((now_unix - minutes_ago * 60 - day_start) * 1000) as u32 }
    }

    const NOW: i64 = (20_700 - 1) * 86_400 + 18 * 3600; // 18:00Z on day 20_700

    fn resolved(kind: ModeKind, displayed_minutes_ago: i64, live_minutes_ago: i64) -> Resolved {
        Resolved {
            kind,
            scan: Some(DisplayedScan { volume: volume_ago(NOW, displayed_minutes_ago), vcp_number: 35 }),
            live_scan: Some(DisplayedScan { volume: volume_ago(NOW, live_minutes_ago), vcp_number: 35 }),
            ..Resolved::default()
        }
    }

    #[test]
    fn live_age_ignores_the_pinned_frames_age() {
        let r = resolved(ModeKind::Paused, 50, 2);
        let (text, alarming) = age_label(&r, NOW).unwrap();
        assert_eq!(text, "live age 2m 00s", "measured from the live feed, not the 50-minute-old pinned volume");
        assert!(!alarming);
    }

    #[test]
    fn the_stale_alert_never_fires_on_a_paused_fresh_feed() {
        // A volume hours old on screen, on purpose; the feed itself is fresh.
        for kind in [ModeKind::Paused, ModeKind::Playing] {
            let (_, alarming) = age_label(&resolved(kind, 300, 1), NOW).unwrap();
            assert!(!alarming, "{kind:?}: being in the past on purpose is not stale data");
        }
        let (_, alarming) = age_label(&resolved(ModeKind::Paused, 1, 300), NOW).unwrap();
        assert!(alarming, "a stale live feed still alarms in every mode");
    }

    #[test]
    fn age_is_labelled_plainly_when_live() {
        let (text, _) = age_label(&resolved(ModeKind::Live, 2, 2), NOW).unwrap();
        assert_eq!(text, "age 2m 00s");
    }

    #[test]
    fn badge_text_in_each_mode() {
        assert_eq!(badge_text(&resolved(ModeKind::Live, 2, 2)), "LIVE");
        assert_eq!(badge_text(&resolved(ModeKind::Paused, 20, 2)), "PAUSED 2026-09-03 17:40:00Z \u{2212}18 min");
        assert_eq!(badge_text(&resolved(ModeKind::Playing, 20, 2)), "LOOP 2026-09-03 17:40:00Z \u{2212}18 min");
        assert_eq!(badge_text(&Resolved { kind: ModeKind::Paused, ..Resolved::default() }), "PAUSED");
    }

    #[test]
    fn absence_names_the_product_cut_and_volume_and_never_shows_when_there_is_a_grid() {
        let mut r = resolved(ModeKind::Paused, 56, 2);
        r.cut = Some(radar_workstation::compute::cut::cut_key(0.5));
        r.positions = vec![timeline::Position { volume: volume_ago(NOW, 56), satisfies: false, in_loop: true }];
        assert_eq!(
            absence_message(DisplayProduct::Velocity, &r).as_deref(),
            Some("No VEL at 0.5\u{b0} in the 17:04Z volume")
        );
        r.kind = ModeKind::Live;
        assert_eq!(absence_message(DisplayProduct::Velocity, &r).as_deref(), Some("No VEL at 0.5\u{b0}"));
        r.grid = Some(grid(DisplayProduct::Velocity));
        assert_eq!(absence_message(DisplayProduct::Velocity, &r), None);
    }
}
