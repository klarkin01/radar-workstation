use std::process::ExitCode;
use std::sync::Arc;

use radar_workstation::config;
use radar_workstation::ingest::s3_poll::IngestStatus;
use radar_workstation::pipeline::Pipeline;
use radar_workstation::state::AppState;

mod cli;
mod headless;

/// Tokio worker threads (S2-c §4.4). This stage's workload is one poll
/// every ~5s, one BZ2 decompression per chunk, and one decode — not a
/// throughput problem. The default (one worker per core) would allocate
/// eight-plus threads per instance against NFR-P-1's four-simultaneous-
/// instances requirement and "Lightweight by Design". rayon gets its own
/// pool at Stage 3, sized separately; re-measure this constant at Stage 4
/// when a real render loop is competing for cores.
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
    let config_path = args.config_path.or_else(|| radar_workstation::paths::config_dir().map(|dir| dir.join("config")));
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

    // Deliberately not `#[tokio::main]` (S2-c): Stage 4's winit/egui event
    // loop takes over the main thread and, on some platforms, never gives
    // it back. Building the runtime explicitly and reserving the main
    // thread now is what makes that step a two-line change later instead
    // of an archaeology exercise.
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

    let (status_tx, status_rx) = tokio::sync::watch::channel(IngestStatus::default());
    let state = Arc::new(AppState::new(site, status_rx));
    // Anything config::load found wrong with the file (a bad line, an
    // unknown site, a clamped interval) is reported now that there's an
    // AppState to report it into — config loading itself never fails.
    for event in config_events {
        state.report(event);
    }

    let pipeline = Pipeline::spawn(runtime.handle(), site, Arc::clone(&state), status_tx, cfg.poll_interval);

    // Stage 2: a headless loop that prints state transitions and exits on
    // stdin EOF. This is the one call Stage 4 replaces with the real event
    // loop — see `headless`'s doc comment.
    headless::run(&state);

    runtime.block_on(pipeline.shutdown());
    ExitCode::SUCCESS
}
