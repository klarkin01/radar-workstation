use std::process::ExitCode;
use std::sync::Arc;

use radar_workstation::compute::DisplayProduct;
use radar_workstation::config;
use radar_workstation::ingest::s3_poll::IngestStatus;
use radar_workstation::pipeline::Pipeline;
use radar_workstation::state::{AppState, RetentionPolicy};

mod cli;
mod headless;
mod render;

/// Tokio worker threads (S2-c §4.4). This stage's ingest workload is one
/// poll every ~5 s, one BZ2 decompression per chunk, and one decode — not a
/// throughput problem. The default (one worker per core) would allocate
/// eight-plus threads per instance against NFR-P-1's four-simultaneous-
/// instances requirement and "Lightweight by Design".
///
/// Re-measured at Stage 4 (S4-W1 §4.2) with the winit/wgpu render loop
/// competing for cores: the render loop runs on the **main** thread (winit
/// takes it), `Gpu::new` is the only place it blocks on the tokio runtime,
/// and steady-state rendering does no async work at all — so 2 ingest
/// workers plus the main render thread is still the right shape. rayon
/// remains deferred (ADR-0005 erratum; S3-f).
const RUNTIME_WORKER_THREADS: usize = 2;

fn main() -> ExitCode {
    let args = match cli::parse(std::env::args_os()) {
        cli::ParseOutcome::Help => {
            cli::print_help();
            return ExitCode::SUCCESS;
        }
        cli::ParseOutcome::Version => {
            println!("radar-workstation {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        cli::ParseOutcome::Error(message) => {
            eprintln!("radar-workstation: {message}");
            cli::print_usage();
            return ExitCode::from(2);
        }
        cli::ParseOutcome::Args(args) => args,
    };

    // §6.4: --config PATH overrides the default XDG location. Neither
    // resolving means configuration simply doesn't persist this run
    // (HOME unset, e.g. on a service account) — FR-CP-3's "must not
    // prevent startup" applies to a missing directory too, not just a
    // missing file.
    let config_path = args.config_path.clone().or_else(|| radar_workstation::paths::config_dir().map(|dir| dir.join("config")));
    let (cfg, config_events) = match &config_path {
        Some(path) => config::load(path),
        None => (config::Config::default(), Vec::new()),
    };

    // Site resolution order: CLI argument -> config `site` -> usage error
    // (§6.4). Only an explicit in-UI site change (Stage 7) writes `site`
    // back to the config file — this run never does, whichever source won.
    let site = match cli::resolve_site(args.site.as_deref(), cfg.site) {
        Ok(site) => site,
        Err(cli::SiteResolutionError::UnknownCliSite(id)) => {
            eprintln!("radar-workstation: unknown site {id:?}");
            cli::print_usage();
            return ExitCode::from(2);
        }
        Err(cli::SiteResolutionError::NoSiteSpecified) => {
            cli::print_usage();
            return ExitCode::from(2);
        }
    };

    println!("Radar Workstation v{} starting — site {} ({})", env!("CARGO_PKG_VERSION"), site.id, site.name);

    // Deliberately not `#[tokio::main]` (S2-c): Stage 4's winit event loop
    // takes over the main thread and, on some platforms, never gives it
    // back. The runtime is built explicitly and its handle moved into the
    // render app so `Pipeline::shutdown` can still be awaited on the way out.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_io()
        .enable_time()
        .worker_threads(RUNTIME_WORKER_THREADS)
        .thread_name("rw-io")
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("radar-workstation: failed to start async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    let retention_policy = retention_policy_from_config(&cfg);
    let (status_tx, status_rx) = tokio::sync::watch::channel(IngestStatus::default());
    let state = Arc::new(AppState::new(site, status_rx, retention_policy));
    // Anything config::load found wrong with the file (a bad line, an
    // unknown site, a clamped interval) is reported now that there's an
    // AppState to report it into — config loading itself never fails.
    for event in config_events {
        state.report(event);
    }

    let pipeline = Pipeline::spawn(runtime.handle(), site, Arc::clone(&state), status_tx, cfg.poll_interval);

    let exit = if args.headless {
        // The one call Stage 2 left as a placeholder for the render loop —
        // now a supported mode (S4-e), not a stub.
        headless::run(&state, retention_policy);
        ExitCode::SUCCESS
    } else {
        run_render(Arc::clone(&state), site, runtime.handle().clone(), &cfg, config_path.as_deref())
    };

    runtime.block_on(pipeline.shutdown());
    exit
}

/// Builds the operator's retention policy (ADR-0030, FR-DA-10) from the two
/// `history.*` config keys. `history.budget_mb = 0` is a first-class
/// setting — it means [`RetentionPolicy::DISABLED`], the Stage 5 footprint
/// — and wins over any configured frame count, since a zero byte budget can
/// never actually retain more than the newest frame regardless of what was
/// asked for.
fn retention_policy_from_config(cfg: &config::Config) -> RetentionPolicy {
    use radar_workstation::state::history::{DEFAULT_HISTORY_BUDGET_BYTES, DEFAULT_HISTORY_FRAMES};

    match cfg.history_budget_mb {
        Some(0) => RetentionPolicy::DISABLED,
        Some(mb) => RetentionPolicy {
            frames: cfg.history_frames.unwrap_or(DEFAULT_HISTORY_FRAMES),
            budget_bytes: mb * 1024 * 1024,
        },
        None => RetentionPolicy {
            frames: cfg.history_frames.unwrap_or(DEFAULT_HISTORY_FRAMES),
            budget_bytes: DEFAULT_HISTORY_BUDGET_BYTES,
        },
    }
}

/// Runs the render loop and persists window geometry / active product on a
/// clean shutdown (§10). A window/GPU failure exits non-zero naming
/// `--headless` — never a silent fall-back to the headless log (S4-e).
fn run_render(
    state: Arc<AppState>,
    site: &'static radar_workstation::sites::Site,
    handle: tokio::runtime::Handle,
    cfg: &config::Config,
    config_path: Option<&std::path::Path>,
) -> ExitCode {
    const DEFAULT_W: u32 = 1280;
    const DEFAULT_H: u32 = 800;

    let initial_size = (cfg.window_width.unwrap_or(DEFAULT_W), cfg.window_height.unwrap_or(DEFAULT_H));
    let initial_product = cfg.view_product.unwrap_or(DisplayProduct::Reflectivity);
    let initial_show_highways = cfg.show_highways.unwrap_or(true);
    let initial_show_reference = cfg.show_reference.unwrap_or(true);

    let persisted = match render::run(
        state,
        site,
        handle,
        initial_size,
        initial_product,
        initial_show_highways,
        initial_show_reference,
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("radar-workstation: {e}");
            // `--headless` is the remedy only when the display/GPU could not
            // be brought up at all. A mid-session presentation loss or an
            // event-loop failure reports its own cause (ADR-0024 §S5).
            use render::RenderError::*;
            if matches!(e, Surface(_) | NoAdapter(_) | Device(_) | NoPresentableAdapter { .. }) {
                eprintln!(
                    "radar-workstation: run with --headless to use the workstation without a display."
                );
            }
            return ExitCode::FAILURE;
        }
    };

    if let Some(path) = config_path {
        let mut changes: Vec<(String, String)> = Vec::new();
        if cfg.window_width != Some(persisted.width) {
            changes.push((config::WINDOW_WIDTH_KEY.to_string(), persisted.width.to_string()));
        }
        if cfg.window_height != Some(persisted.height) {
            changes.push((config::WINDOW_HEIGHT_KEY.to_string(), persisted.height.to_string()));
        }
        if cfg.view_product != Some(persisted.product) {
            changes.push((config::VIEW_PRODUCT_KEY.to_string(), persisted.product.to_string()));
        }
        if cfg.show_highways != Some(persisted.show_highways) {
            changes.push((config::VIEW_HIGHWAYS_KEY.to_string(), persisted.show_highways.to_string()));
        }
        if cfg.show_reference != Some(persisted.show_reference) {
            changes.push((config::VIEW_REFERENCE_KEY.to_string(), persisted.show_reference.to_string()));
        }
        if !changes.is_empty() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = config::save(path, &changes) {
                eprintln!("radar-workstation: could not save configuration: {e}");
            }
        }
    }

    ExitCode::SUCCESS
}
