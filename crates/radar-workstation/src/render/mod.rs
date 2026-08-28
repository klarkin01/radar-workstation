//! The render loop (S4). Owns the winit event loop, the wgpu surface, and
//! the two-pass frame; reads `AppState` once per frame through
//! `snapshot()` and holds no lock. This is the module `headless::run` was a
//! placeholder for (§3.5 keeps `headless` reachable behind `--headless`).
//!
//! **Spatial stability (FR-NI-4, S4-g).** `ViewState` lives in
//! [`view`] and is mutated only by [`view`] functions called from
//! [`input`]. No function that takes a `StateSnapshot` takes `&mut
//! ViewState`; `tests::view_state_is_unchanged_by_any_sequence_of_state_updates`
//! guards that boundary.
//!
//! **Frame pacing (§3.3).** Redraw on input, resize, egui's own repaint
//! request, or an idle tick (2 Hz) when `revision` or the time-derived
//! chrome text changed. `PresentMode::Fifo` so an interaction burst cannot
//! spin the GPU past the display rate.

pub mod adapter;
pub mod gpu;
pub mod input;
pub mod radar;
pub mod reference;
pub mod time;
pub mod ui;
pub mod view;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, PhysicalKey};
use winit::window::{Window, WindowId};

use radar_workstation::compute::palette::{self, Palette};
use radar_workstation::compute::{DisplayProduct, SweepGrid};
use radar_workstation::sites::Site;
use radar_workstation::state::{AppState, StateSnapshot};

use self::gpu::Gpu;
use self::input::Action;

pub use self::gpu::RenderError;
use self::radar::RadarRenderer;
use self::reference::ReferenceRenderer;
use self::view::ViewState;

const IDLE_TICK: Duration = Duration::from_millis(500);
/// Dark, near-black — layer 1 of FR-DR-3.
const BACKGROUND: wgpu::Color = wgpu::Color { r: 0.043, g: 0.047, b: 0.055, a: 1.0 };

/// View-derived values worth persisting on a clean shutdown (§10). The
/// render loop owns these; `main` compares against the loaded config and
/// saves only what changed.
pub struct PersistedView {
    pub width: u32,
    pub height: u32,
    pub product: DisplayProduct,
}

/// Run the render loop until the window closes or `Ctrl+Q`. Returns the
/// final view-derived values for `main` to persist. An `Err` carries its own
/// cause (ADR-0024 §S5): `main` suggests `--headless` only for the variants
/// where it is actually the remedy, and it must not silently fall back to
/// headless mode.
pub fn run(
    state: Arc<AppState>,
    site: &'static Site,
    runtime: tokio::runtime::Handle,
    initial_size: (u32, u32),
    initial_product: DisplayProduct,
) -> Result<PersistedView, RenderError> {
    let event_loop = EventLoop::new().map_err(|e| RenderError::EventLoop(e.to_string()))?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let (palettes, palette_events) = palette::load_all();
    for event in palette_events {
        state.report(event);
    }

    let mut app = App {
        state,
        runtime,
        site,
        palettes,
        window: None,
        gpu: None,
        egui_ctx: egui::Context::default(),
        egui_state: None,
        egui_renderer: None,
        radar: None,
        reference: None,
        view: ViewState::initial((initial_size.0 as f32, initial_size.1 as f32), initial_product),
        initial_size,
        last_uploaded_revision: None,
        last_drawn_revision: None,
        last_chrome_text: String::new(),
        show_help: false,
        cursor_pos: None,
        drag_anchor: None,
        next_deadline: Instant::now(),
        first_frame_at: None,
        reconfigure_count: 0,
        fatal_error: None,
    };

    let loop_result = event_loop.run_app(&mut app);

    // A fatal error raised from inside the loop (a clean init failure, or the
    // late-failure guard's `PresentationLost`) is always the more informative
    // cause.
    if let Some(err) = app.fatal_error {
        return Err(err);
    }
    if let Err(e) = loop_result {
        // The event loop itself returned `Err`. If that happened within
        // seconds of a successful GPU init, it is the hybrid-GPU
        // dmabuf-rejection signature (§1, §4.4): the compositor imports the
        // first swapchain buffer, fails, and tears the Wayland connection
        // down — asynchronously, so neither bring-up nor the reconfigure
        // guard catches it first. Report it as what it is, naming the
        // adapter and the `RADAR_GPU` hint, not as a bare `Exit Failure: 1`.
        if let (Some(adapter), Some(first_frame)) =
            (app.gpu.as_ref().map(|g| g.adapter.clone()), app.first_frame_at)
        {
            if first_frame.elapsed() < PRESENTATION_LOSS_GRACE {
                return Err(RenderError::PresentationLost {
                    adapter: format!(
                        "{} ({:?}, {})",
                        adapter.name,
                        adapter.backend,
                        adapter::pci_or_dash(&adapter.pci_address)
                    ),
                });
            }
        }
        return Err(RenderError::EventLoop(e.to_string()));
    }

    let size = app.window.as_ref().map(|w| w.inner_size()).unwrap_or(PhysicalSize::new(initial_size.0, initial_size.1));
    Ok(PersistedView { width: size.width, height: size.height, product: app.view.product })
}

struct App {
    state: Arc<AppState>,
    runtime: tokio::runtime::Handle,
    site: &'static Site,
    palettes: BTreeMap<DisplayProduct, Palette>,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    radar: Option<RadarRenderer>,
    reference: Option<ReferenceRenderer>,
    view: ViewState,
    initial_size: (u32, u32),
    last_uploaded_revision: Option<u64>,
    last_drawn_revision: Option<u64>,
    last_chrome_text: String,
    show_help: bool,
    cursor_pos: Option<PhysicalPosition<f64>>,
    drag_anchor: Option<(f64, f64)>,
    next_deadline: Instant,
    /// When the first frame was attempted — the late-failure guard (§4.4)
    /// only fires on a reconfigure burst within the first 2 s.
    first_frame_at: Option<Instant>,
    /// Reconfigures triggered by `SurfaceError::{Lost, Outdated}`. More than
    /// three inside the startup window is a presentation-lost failure.
    reconfigure_count: u32,
    /// A fatal error raised from inside the event loop — `run` returns it
    /// after `run_app` unwinds (§S5, §4.4).
    fatal_error: Option<RenderError>,
}

/// The late-failure guard's thresholds (§4.4).
const STARTUP_WINDOW: Duration = Duration::from_secs(2);
const MAX_STARTUP_RECONFIGURES: u32 = 3;
/// If the event loop returns `Err` within this long of the first frame, treat
/// it as a presentation loss (the adapter cannot drive this compositor's
/// surface) rather than a generic event-loop failure.
const PRESENTATION_LOSS_GRACE: Duration = Duration::from_secs(10);

impl App {
    fn viewport(&self) -> (f32, f32) {
        self.gpu
            .as_ref()
            .map(|g| (g.config.width as f32, g.config.height as f32))
            .unwrap_or((self.initial_size.0 as f32, self.initial_size.1 as f32))
    }

    fn apply_action(&mut self, action: Action, event_loop: &ActiveEventLoop) {
        let vp = self.viewport();
        match action {
            Action::SelectProduct(p) => self.view.product = p,
            Action::ElevationUp => self.step_elevation(1),
            Action::ElevationDown => self.step_elevation(-1),
            Action::Pan { fx, fy } => view::pan_by_pixels(&mut self.view, fx * vp.0, fy * vp.1),
            Action::ZoomIn => view::zoom_about(&mut self.view, vp.0 / 2.0, vp.1 / 2.0, input::ZOOM_IN_FACTOR, vp),
            Action::ZoomOut => view::zoom_about(&mut self.view, vp.0 / 2.0, vp.1 / 2.0, input::ZOOM_OUT_FACTOR, vp),
            Action::ResetView => self.view.reset_navigation(vp),
            Action::ToggleReference => self.view.show_reference = !self.view.show_reference,
            Action::ToggleHelp => self.show_help = !self.show_help,
            Action::Quit => event_loop.exit(),
        }
    }

    /// Move the selection by `dir` steps through the elevations present, in
    /// angle order. Never changes the selection to something absent silently
    /// — if there is nothing to step to, the selection stays put (§3.8).
    fn step_elevation(&mut self, dir: i32) {
        let snapshot = self.state.snapshot();
        let mut elevations: Vec<(u8, f32)> =
            snapshot.sweeps.iter().map(|s| (s.elevation_number, s.elevation_deg)).collect();
        elevations.sort_by(|a, b| a.1.total_cmp(&b.1));
        if elevations.is_empty() {
            return;
        }
        let current = elevations.iter().position(|(n, _)| *n == self.view.elevation_number);
        let next_idx = match current {
            Some(i) => i as i32 + dir,
            None => 0,
        };
        if let Some((n, _)) = next_idx.try_into().ok().and_then(|i: usize| elevations.get(i)) {
            self.view.elevation_number = *n;
        }
    }

    /// The grid `Arc` for the current selection, plus its angle. Also sets
    /// the first-run default (§3.8). Returns an owned `Arc` clone (a
    /// refcount bump) so the caller has no borrow entangled with `self`.
    fn resolve_selection(&mut self, snapshot: &StateSnapshot) -> (Option<Arc<SweepGrid>>, Option<f32>) {
        if self.view.elevation_number == 0 {
            if let Some(lowest) =
                snapshot.sweeps.iter().min_by(|a, b| a.elevation_deg.total_cmp(&b.elevation_deg))
            {
                self.view.elevation_number = lowest.elevation_number;
            }
        }

        if matches!(self.view.product, DisplayProduct::EchoTops | DisplayProduct::Vil) {
            let g = snapshot.derived.iter().find(|g| g.product == self.view.product).cloned();
            let deg = g.as_ref().map(|g| g.elevation_deg);
            return (g, deg);
        }

        let sweep = snapshot.sweeps.iter().find(|s| s.elevation_number == self.view.elevation_number);
        let grid = sweep.and_then(|s| s.grids.iter().find(|g| g.product == self.view.product).cloned());
        (grid, sweep.map(|s| s.elevation_deg))
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() || self.gpu.is_none() {
            return;
        }
        self.first_frame_at.get_or_insert_with(Instant::now);

        let state = Arc::clone(&self.state);
        let snapshot = state.snapshot();
        let (selected_grid, selected_deg) = self.resolve_selection(&snapshot);

        let window = self.window.clone().unwrap();
        let egui_ctx = self.egui_ctx.clone();
        let view = self.view;
        let show_help = self.show_help;
        let site = self.site;
        let cursor_pos = self.cursor_pos;
        let recent = state.recent_events(1).pop().map(|(_, s)| s);
        let displayed_volume = snapshot
            .sweeps
            .iter()
            .find(|s| s.elevation_number == view.elevation_number)
            .map(|s| s.volume);
        let palette = self.palettes.get(&view.product);

        let gpu = self.gpu.as_mut().unwrap();
        let egui_state = self.egui_state.as_mut().unwrap();
        let egui_renderer = self.egui_renderer.as_mut().unwrap();
        let radar = self.radar.as_mut().unwrap();
        let reference = self.reference.as_ref().unwrap();

        if self.last_uploaded_revision != Some(snapshot.revision) {
            for event in radar.sync(&gpu.device, &gpu.queue, &snapshot, &self.palettes) {
                state.report(event);
            }
            self.last_uploaded_revision = Some(snapshot.revision);
        }
        self.last_drawn_revision = Some(snapshot.revision);

        let surface_texture = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                gpu.reconfigure();
                // Late-failure guard (§4.4). A wrong adapter choice on a
                // hybrid-GPU box shows up here: the compositor rejects the
                // swapchain dmabufs, the surface goes Lost, and reconfigure
                // runs against a dying wl_surface. Do not attempt re-
                // selection — the Wayland connection is already unusable —
                // just turn `Exit Failure: 1` into an accurate message.
                self.reconfigure_count += 1;
                let validation_error = gpu.take_uncaptured_error();
                let within_startup = self
                    .first_frame_at
                    .map(|t| t.elapsed() < STARTUP_WINDOW)
                    .unwrap_or(true);
                if validation_error.is_some()
                    || (within_startup && self.reconfigure_count > MAX_STARTUP_RECONFIGURES)
                {
                    self.fatal_error = Some(RenderError::PresentationLost {
                        adapter: format!(
                            "{} ({:?}, {})",
                            gpu.adapter.name,
                            gpu.adapter.backend,
                            adapter::pci_or_dash(&gpu.adapter.pci_address)
                        ),
                    });
                    event_loop.exit();
                }
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => return,
        };
        let target_view = surface_texture.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let vp = (gpu.config.width as f32, gpu.config.height as f32);
        let pixels_per_point = window.scale_factor() as f32;

        // --- egui frame ---
        let raw_input = egui_state.take_egui_input(&window);
        let cursor_world = cursor_pos.map(|p| view::screen_to_world(p.x as f32, p.y as f32, &view, vp));

        let chrome = ui::ChromeInput {
            site,
            snapshot: &snapshot,
            view: &view,
            selected_grid: selected_grid.as_deref(),
            selected_elevation_deg: selected_deg,
            displayed_volume,
            palette,
            cursor_world,
            recent_event: recent,
            show_help,
            now: Instant::now(),
            viewport: vp,
        };
        let full_output = egui_ctx.run_ui(raw_input, |ui| ui::draw(ui, &chrome));
        egui_state.handle_platform_output(&window, full_output.platform_output);
        let tris = egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);

        let mut encoder =
            gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        for (id, deltas) in &full_output.textures_delta.set {
            for delta in deltas {
                egui_renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
            }
        }
        let screen_desc = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [gpu.config.width, gpu.config.height],
            pixels_per_point,
        };
        egui_renderer.update_buffers(&gpu.device, &gpu.queue, &mut encoder, &tris, &screen_desc);

        // Pass 1: clear to background, radar (layer 6), reference (layer 8-ish).
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(BACKGROUND), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let camera = view::Camera::from_view(&view, vp);
            if let Some(grid) = selected_grid.as_deref() {
                radar.draw(&gpu.device, &gpu.queue, &mut pass, grid, camera);
            }
            if view.show_reference {
                reference.draw(&gpu.queue, &mut pass, camera);
            }
        }

        // Pass 2: egui chrome (layer 10).
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            egui_renderer.render(&mut pass, &tris, &screen_desc);
        }

        for id in &full_output.textures_delta.free {
            egui_renderer.free_texture(id);
        }

        gpu.queue.submit([encoder.finish()]);
        window.pre_present_notify();
        gpu.queue.present(surface_texture);
        // A frame presented cleanly — drop any latched uncaptured error so
        // the late-failure guard (§4.4) only ever sees one raised *by* the
        // reconfigure it is checking, not a stale one from earlier.
        let _ = gpu.take_uncaptured_error();

        // Frame pacing: honour egui's own repaint request when it is sooner
        // than the idle tick (§3.3 — the most likely defect in this stage).
        let egui_delay = full_output
            .viewport_output
            .values()
            .map(|v| v.repaint_delay)
            .min()
            .unwrap_or(IDLE_TICK);
        self.next_deadline = Instant::now() + egui_delay.min(IDLE_TICK);
    }

    /// The time-derived chrome text, compared frame to frame so the data-age
    /// readout ticks even when `revision` is unchanged (§5).
    fn chrome_age_text(&self, snapshot: &StateSnapshot) -> String {
        match snapshot.ingest.last_success {
            Some(t) => format!("{}", Instant::now().saturating_duration_since(t).as_secs()),
            None => "none".to_string(),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(format!("Radar Workstation — {} ({})", self.site.id, self.site.name))
            .with_inner_size(PhysicalSize::new(self.initial_size.0, self.initial_size.1));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.fatal_error = Some(RenderError::Surface(e.to_string()));
                event_loop.exit();
                return;
            }
        };

        // Some GPU init failures (a surface/adapter queue-family mismatch on
        // a nested or headless compositor) reach us as an *uncaptured* wgpu
        // validation error, i.e. a panic, not a `Result`. `panic = "unwind"`
        // (release profile) lets us catch it and fail cleanly — §3.5's
        // "exit non-zero naming --headless", never a silent degrade.
        let init = {
            let window = window.clone();
            let handle = self.runtime.clone();
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || handle.block_on(Gpu::new(window))))
        };
        let gpu = match init {
            Ok(Ok(g)) => g,
            Ok(Err(e)) => {
                self.fatal_error = Some(e);
                event_loop.exit();
                return;
            }
            Err(_) => {
                self.fatal_error = Some(RenderError::Device(
                    "the GPU rejected the render surface (this can happen under a nested or \
                     headless compositor)"
                        .to_string(),
                ));
                event_loop.exit();
                return;
            }
        };
        // §S6: name the adapter and *why* it was chosen, so the next report
        // of a hybrid-GPU selection problem is one line long.
        eprintln!(
            "[radar-workstation] GPU: {} ({:?}, {}) — {}; surface {:?} (sRGB: {})",
            gpu.adapter.name,
            gpu.adapter.backend,
            adapter::pci_or_dash(&gpu.adapter.pci_address),
            gpu.selection_reason,
            gpu.config.format,
            gpu.surface_is_srgb,
        );

        let egui_state = egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            Some(gpu.max_texture_dimension as usize),
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            &gpu.device,
            gpu.config.format,
            egui_wgpu::RendererOptions {
                msaa_samples: 1,
                depth_stencil_format: None,
                dithering: gpu.surface_is_srgb,
                predictable_texture_filtering: false,
            },
        );
        let radar = RadarRenderer::new(&gpu.device, gpu.config.format, gpu.surface_is_srgb, gpu.max_texture_dimension);
        let reference = ReferenceRenderer::new(&gpu.device, gpu.config.format);

        self.window = Some(window.clone());
        self.gpu = Some(gpu);
        self.egui_state = Some(egui_state);
        self.egui_renderer = Some(egui_renderer);
        self.radar = Some(radar);
        self.reference = Some(reference);
        window.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // egui gets first refusal on every event.
        if let (Some(state), Some(window)) = (self.egui_state.as_mut(), self.window.clone()) {
            let response = state.on_window_event(&window, &event);
            if response.repaint {
                window.request_redraw();
            }
            if response.consumed {
                return;
            }
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                let old = self.viewport();
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.resize(size.width, size.height);
                }
                view::on_resize(&mut self.view, old, (size.width as f32, size.height as f32));
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event: key_event, .. } => {
                if key_event.state != ElementState::Pressed {
                    return;
                }
                let ctrl = self
                    .egui_ctx
                    .input(|i| i.modifiers.ctrl);
                let action = match key_event.physical_key {
                    PhysicalKey::Code(code) => input::action_for_key(code, ctrl),
                    _ => None,
                }
                .or_else(|| match &key_event.logical_key {
                    Key::Character(s) => input::action_for_char(s.as_str()),
                    _ => None,
                });
                if let Some(action) = action {
                    self.apply_action(action, event_loop);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let (Some(anchor), true) = (self.drag_anchor, self.cursor_pos.is_some()) {
                    let dx = (position.x - anchor.0) as f32;
                    let dy = (position.y - anchor.1) as f32;
                    view::pan_by_pixels(&mut self.view, -dx, -dy);
                    self.drag_anchor = Some((position.x, position.y));
                }
                self.cursor_pos = Some(position);
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor_pos = None;
                self.drag_anchor = None;
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                self.drag_anchor = match state {
                    ElementState::Pressed => self.cursor_pos.map(|p| (p.x, p.y)),
                    ElementState::Released => None,
                };
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(p) => p.y / 40.0,
                };
                if scroll.abs() > f64::EPSILON {
                    let vp = self.viewport();
                    let (cx, cy) = self.cursor_pos.map(|p| (p.x as f32, p.y as f32)).unwrap_or((vp.0 / 2.0, vp.1 / 2.0));
                    let factor = if scroll > 0.0 { input::ZOOM_IN_FACTOR } else { input::ZOOM_OUT_FACTOR };
                    view::zoom_about(&mut self.view, cx, cy, factor, vp);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Idle tick: request a redraw when the data revision moved or the
        // time-derived chrome text (data age) changed since the last frame —
        // this is what keeps the "updated N s ago" readout live without a
        // signalling path from the applier (§3.3, §5).
        let snapshot = self.state.snapshot();
        let age_text = self.chrome_age_text(&snapshot);
        let revision_changed = self.last_drawn_revision != Some(snapshot.revision);
        if revision_changed || age_text != self.last_chrome_text {
            self.last_chrome_text = age_text;
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
        // `next_deadline` is refreshed at the end of each frame (honouring
        // egui's own repaint request when sooner). If it has already passed
        // — no frame ran this cycle — fall back to the 2 Hz idle tick rather
        // than spinning.
        let now = Instant::now();
        if self.next_deadline <= now {
            self.next_deadline = now + IDLE_TICK;
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_deadline));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use tokio::sync::watch;

    use radar_workstation::assembly::{AssemblyEvent, VolumeId};
    use radar_workstation::compute::{DisplayProduct, StateUpdate, SweepGrid};
    use radar_workstation::ingest::s3_poll::IngestStatus;
    use radar_workstation::state::{AppState, VolumeSummary};

    use super::view::ViewState;

    fn grid(product: DisplayProduct, el: u8) -> Arc<SweepGrid> {
        Arc::new(SweepGrid {
            product,
            azimuth_count: 4,
            gate_count: 4,
            first_gate_m: 0,
            gate_width_m: 250,
            elevation_number: el,
            elevation_deg: el as f32 * 0.5,
            nyquist_velocity_mps: Some(8.0),
            scale: 2.0,
            offset: 66.0,
            cells: vec![0u8; 16],
            filled_azimuths: 0,
        })
    }

    /// FR-NI-4 / S4-g: no sequence of state updates can move the view. The
    /// guarantee is structural — nothing wires `ViewState` to `AppState` —
    /// and this test is the boundary marker for the next contributor.
    #[tokio::test]
    async fn view_state_is_unchanged_by_any_sequence_of_state_updates() {
        let (_tx, rx) = watch::channel(IngestStatus::default());
        let state = AppState::new(radar_workstation::sites::by_id("KDOX").unwrap(), rx);

        let mut view = ViewState::initial((1280.0, 800.0), DisplayProduct::Reflectivity);
        super::view::pan_by_pixels(&mut view, 137.0, -88.0);
        super::view::zoom_about(&mut view, 400.0, 300.0, 0.37, (1280.0, 800.0));
        view.product = DisplayProduct::Velocity;
        view.elevation_number = 5;
        view.show_reference = false;
        let before = view;

        let vol = VolumeId { julian_date: 20_000, scan_time_ms: 1 };
        for el in [1u8, 2, 3, 6] {
            state.apply_event(
                StateUpdate::SweepGridded {
                    elevation_number: el,
                    elevation_deg: el as f32 * 0.5,
                    volume: vol,
                    vcp_number: 35,
                    grids: vec![grid(DisplayProduct::Reflectivity, el)],
                },
                Instant::now(),
            );
        }
        state.apply_event(
            StateUpdate::DerivedComputed {
                volume: vol,
                vcp_number: 35,
                grids: vec![grid(DisplayProduct::EchoTops, 0)],
            },
            Instant::now(),
        );
        state.apply_event(
            StateUpdate::VolumeClosed {
                summary: VolumeSummary {
                    volume: vol,
                    vcp_number: 35,
                    status: nexrad_decoder::VolumeStatus::Complete,
                    latitude: 38.8,
                    longitude: -75.4,
                    site_amsl_m: 15,
                },
            },
            Instant::now(),
        );
        // A VCP change that drops the elevation set.
        state.apply_event(
            StateUpdate::SweepGridded {
                elevation_number: 1,
                elevation_deg: 0.5,
                volume: VolumeId { julian_date: 20_000, scan_time_ms: 2 },
                vcp_number: 212,
                grids: vec![grid(DisplayProduct::Reflectivity, 1)],
            },
            Instant::now(),
        );
        state.apply_event(StateUpdate::Info(AssemblyEvent::MissingStartChunk), Instant::now());

        assert_eq!(view, before, "no state update may perturb ViewState");
    }
}
