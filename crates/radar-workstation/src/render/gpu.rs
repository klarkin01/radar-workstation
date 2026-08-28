//! GPU device, queue, and surface setup (S4-W1 §4.3; adapter selection
//! ADR-0024). Modern wgpu creates a surface *safely* from an `Arc<Window>`
//! that carries the raw-handle traits with a `'static` lifetime — there is
//! no `unsafe` block anywhere in `render/` (NFR-SEC-5, BC-9).
//!
//! Adapter selection does **not** express a power preference (ADR-0024
//! supersedes S4-a's `PowerPreference::LowPower`). It enumerates every
//! Vulkan/GL adapter, ranks them by which one drives a connected display
//! ([`super::adapter`]), and then *verifies the prediction* by bringing a
//! surface up on each candidate in order. The first that configures and
//! yields a frame wins; exhausting the list is
//! [`RenderError::NoPresentableAdapter`]. `RADAR_GPU` overrides the ranking
//! (but not the bring-up check).

use std::sync::{Arc, Mutex};

use winit::window::Window;

use super::adapter::{self, AdapterFacts, GpuOverride, SelectionReason};

/// Everything the frame needs from wgpu. `config` is mutable — resize and
/// `SurfaceError::{Lost, Outdated}` reconfigure it.
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    /// Whether the chosen surface format is `*_UNORM_SRGB` — the LUT builder
    /// (`render::radar`) needs this to decide sRGB→linear conversion (§6.4).
    pub surface_is_srgb: bool,
    /// The device's real `max_texture_dimension_2d`. Grid uploads guard
    /// against it (§4.3) rather than letting wgpu's validation panic on
    /// older GL hardware.
    pub max_texture_dimension: u32,
    /// The adapter that won selection and passed bring-up — its `backend`,
    /// `name`, and `pci_address` are what §14 and the startup diagnostic
    /// line record, and what [`RenderError::PresentationLost`] names.
    pub adapter: AdapterFacts,
    /// Why this adapter was chosen — for the one-line startup diagnostic
    /// (ADR-0024 §S6).
    pub selection_reason: SelectionReason,
    /// Set by the uncaptured-error handler. The late-failure guard in
    /// `render::mod` reads it after a reconfigure so a post-init validation
    /// error becomes an accurate [`RenderError::PresentationLost`] rather
    /// than `Exit Failure: 1` (ADR-0024 §4.4).
    uncaptured_error: Arc<Mutex<Option<String>>>,
}

/// Every way starting the render loop can fail. `main` uses the variant to
/// decide whether `--headless` is actually the remedy (ADR-0024 §2, E1/E3).
#[derive(Debug)]
pub enum RenderError {
    /// The OS window or the wgpu surface object could not be created.
    Surface(String),
    /// No GPU adapters were enumerated at all.
    NoAdapter(String),
    /// An adapter was found but the device could not be created.
    Device(String),
    /// Every ranked adapter failed the bring-up check (§4.3), or `RADAR_GPU`
    /// named an adapter that does not exist.
    NoPresentableAdapter { tried: Vec<String> },
    /// The surface stopped being presentable *after* a successful init —
    /// the signature of a wrong adapter choice on a hybrid-GPU system, where
    /// the compositor rejects the swapchain dmabufs asynchronously (§4.4).
    PresentationLost { adapter: String },
    /// The winit event loop itself failed to build or run. `--headless` is
    /// not the remedy for this.
    EventLoop(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Surface(e) => write!(f, "could not create a render surface: {e}"),
            Self::NoAdapter(e) => write!(f, "no usable GPU adapter: {e}"),
            Self::Device(e) => write!(f, "could not create a GPU device: {e}"),
            Self::NoPresentableAdapter { tried } => {
                writeln!(f, "no GPU adapter could present to this window. Tried:")?;
                for entry in tried {
                    writeln!(f, "  - {entry}")?;
                }
                write!(
                    f,
                    "set RADAR_GPU to a PCI address (0000:01:00.0), an adapter-name substring, \
                     or 'discrete' / 'integrated' to override adapter selection"
                )
            }
            Self::PresentationLost { adapter } => write!(
                f,
                "the GPU stopped being able to present partway through the session \
                 (adapter: {adapter}). On a hybrid-GPU system this usually means the \
                 selected GPU does not drive the display — set RADAR_GPU to the PCI \
                 address or name of the GPU that does"
            ),
            Self::EventLoop(e) => write!(f, "the windowing event loop failed: {e}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// `name (backend, pci)` — the form used in [`RenderError::NoPresentableAdapter`].
fn describe(f: &AdapterFacts) -> String {
    format!("{} ({:?}, {})", f.name, f.backend, adapter::pci_or_dash(&f.pci_address))
}

impl Gpu {
    /// Enumerate, rank, and bring up an adapter (ADR-0024). `with_env()`
    /// carries `WGPU_BACKEND` only — the adapter override is `RADAR_GPU`,
    /// handled here, not by wgpu.
    pub async fn new(window: Arc<Window>) -> Result<Self, RenderError> {
        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = wgpu::Backends::VULKAN | wgpu::Backends::GL;
        // Honour `WGPU_BACKEND` from the environment (e.g. `WGPU_BACKEND=gl`
        // to force the fallback). This is `InstanceDescriptor::with_env`'s
        // *only* effect on selection — it does not read a power preference.
        instance_desc = instance_desc.with_env();
        let backends = instance_desc.backends;
        let instance = wgpu::Instance::new(instance_desc);

        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| RenderError::Surface(e.to_string()))?;

        let adapters = instance.enumerate_adapters(backends).await;
        if adapters.is_empty() {
            return Err(RenderError::NoAdapter(
                "no Vulkan or GL adapter was enumerated for this window".to_string(),
            ));
        }

        // Project each adapter to the pure facts `adapter::rank` needs.
        let facts: Vec<AdapterFacts> = adapters
            .iter()
            .enumerate()
            .map(|(index, a)| {
                let info = a.get_info();
                let caps = surface.get_capabilities(a);
                AdapterFacts {
                    index,
                    name: info.name,
                    pci_address: info.device_pci_bus_id,
                    vendor: info.vendor,
                    device: info.device,
                    device_type: info.device_type,
                    backend: info.backend,
                    presents_to_surface: !caps.formats.is_empty(),
                }
            })
            .collect();

        // The kernel's view of which PCI device drives a connected output.
        // Best effort — an empty list just means `rank` falls back to its
        // unknown-environment tiebreak.
        let displays = adapter::discover_displays(std::path::Path::new("/sys"));

        let (order, forced) = match std::env::var("RADAR_GPU")
            .ok()
            .as_deref()
            .and_then(GpuOverride::parse)
        {
            Some(over) => {
                let picks = over.select(&facts).map_err(|u| {
                    let mut tried =
                        vec![format!("RADAR_GPU set to {} — no adapter matched", u.requested)];
                    tried.extend(u.available.into_iter().map(|a| format!("available: {a}")));
                    RenderError::NoPresentableAdapter { tried }
                })?;
                (picks, true)
            }
            None => (adapter::rank(&facts, &displays), false),
        };

        if order.is_empty() {
            return Err(RenderError::NoPresentableAdapter {
                tried: facts.iter().map(describe).collect(),
            });
        }

        // Verification by bring-up (§4.3): ranking predicts compositor
        // behaviour, so it is checked, not trusted. Walk the candidates;
        // the first that requests a device, configures the surface without
        // a validation error, and yields one frame wins.
        //
        // Honest limitation: a compositor-side dmabuf-import rejection is
        // asynchronous and does *not* surface here — this loop catches
        // device-request and configure/acquire failures. Correct *selection*
        // is what prevents the dmabuf case; `render::mod`'s late-failure
        // guard is the backstop for what neither catches.
        let mut tried = Vec::new();
        for &index in &order {
            match Self::bring_up(&adapters[index], &surface, &window).await {
                Ok(built) => {
                    let reason = if forced {
                        SelectionReason::ForcedByOverride
                    } else {
                        adapter::reason_for(&facts[index], &displays)
                    };
                    let uncaptured_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
                    let sink = Arc::clone(&uncaptured_error);
                    built.device.on_uncaptured_error(Arc::new(move |err| {
                        eprintln!("[radar-workstation] wgpu error: {err}");
                        if let Ok(mut slot) = sink.lock() {
                            *slot = Some(err.to_string());
                        }
                    }));
                    return Ok(Self {
                        device: built.device,
                        queue: built.queue,
                        surface,
                        config: built.config,
                        surface_is_srgb: built.surface_is_srgb,
                        max_texture_dimension: built.max_texture_dimension,
                        adapter: facts[index].clone(),
                        selection_reason: reason,
                        uncaptured_error,
                    });
                }
                Err(why) => tried.push(format!("{} — {why}", describe(&facts[index]))),
            }
        }
        Err(RenderError::NoPresentableAdapter { tried })
    }

    /// Try to stand a working device + configured surface up on one adapter.
    /// Any failure is a `String` reason so the bring-up loop can move on.
    async fn bring_up(
        adapter: &wgpu::Adapter,
        surface: &wgpu::Surface<'static>,
        window: &Window,
    ) -> Result<BroughtUp, String> {
        // downlevel_defaults so the GL fallback (older hardware) runs this
        // unchanged. Its max_texture_dimension_2d is 2048; the largest
        // measured grid is 1832 gates — it fits, but §4.3's upload guard is
        // mandatory, not defensive padding.
        let limits = wgpu::Limits::downlevel_defaults();
        let max_texture_dimension = limits.max_texture_dimension_2d;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("radar-workstation device"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|e| format!("device request failed: {e}"))?;

        // Route uncaptured errors to stderr during bring-up so a validation
        // failure outside the scopes below is not a process-killing panic.
        device.on_uncaptured_error(Arc::new(|err| {
            eprintln!("[radar-workstation] wgpu error (adapter bring-up): {err}");
        }));

        let caps = surface.get_capabilities(adapter);
        let format = caps
            .formats
            .first()
            .copied()
            .ok_or_else(|| "surface has no compatible format on this adapter".to_string())?;
        let surface_is_srgb = format.is_srgb();

        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
        };

        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        surface.configure(&device, &config);
        if let Some(err) = scope.pop().await {
            return Err(format!("surface configuration rejected: {err}"));
        }

        // Acquire one frame. Some incompatibilities (a queue-family mismatch
        // on a nested compositor) only appear on acquire, not on configure.
        // Drop the texture without presenting.
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let acquired = surface.get_current_texture();
        let acquire_ok = matches!(
            acquired,
            wgpu::CurrentSurfaceTexture::Success(_) | wgpu::CurrentSurfaceTexture::Suboptimal(_)
        );
        drop(acquired);
        if let Some(err) = scope.pop().await {
            return Err(format!("surface frame acquisition raised a validation error: {err}"));
        }
        if !acquire_ok {
            return Err("the adapter could not produce a frame for this window".to_string());
        }

        Ok(BroughtUp { device, queue, config, surface_is_srgb, max_texture_dimension })
    }

    /// Take the last uncaptured wgpu error, if any — the late-failure guard
    /// (§4.4) checks this after a reconfigure.
    pub fn take_uncaptured_error(&self) -> Option<String> {
        self.uncaptured_error.lock().ok().and_then(|mut slot| slot.take())
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    /// Reconfigure the surface with the current config — the recovery path
    /// for `SurfaceError::{Lost, Outdated}`.
    pub fn reconfigure(&mut self) {
        self.surface.configure(&self.device, &self.config);
    }
}

/// The pieces `bring_up` produces on success.
struct BroughtUp {
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    surface_is_srgb: bool,
    max_texture_dimension: u32,
}
