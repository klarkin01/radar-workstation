/// radar-viz — render decoded NEXRAD chunk data as a PNG PPI image.
///
/// Usage:
///   radar-viz [OPTIONS] <input-dir>
///
/// Products: DREF (reflectivity), DVEL (velocity), DSW (spectrum width),
///           DZDR (differential reflectivity), DPHI (differential phase),
///           DRHO (correlation coefficient); `--path grid` additionally
///           accepts ECHO_TOPS and VIL (S3-W5), which have no radial-path
///           equivalent — they are volume-derived, not per-sweep moments.
///
/// `--path radial` (default) renders directly from decoded radials with a
/// nearest-radial search, using this tool's own hand-rolled colour tables —
/// the harness Stages 0-2 validated the decoder against. `--path grid`
/// renders through `compute::grid`/`compute::palette` — the same gridding
/// and colour-mapping code the application itself uses (S3-W1/W3) — so the
/// two paths, drawing the same fixture, are the visual cross-check S3-W5
/// calls for.
use std::collections::BTreeMap;
use std::sync::Arc;
use std::{env, fs, path::{Path, PathBuf}, process};

use nexrad_decoder::{parse_radial_stream, ProductKind, Radial, Sweep};
use radar_workstation::compute::{derived, grid, palette, DisplayProduct};
use radar_workstation::{decompress_chunk, detect_chunk_kind};

mod color_table;
mod overlay;
mod png_out;
mod render;
mod render_grid;

use color_table::ColorTable;
use overlay::{draw_overlay, OverlayLayer};
use render::render_ppi;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;

    let all_radials = load_volume(&args.input)?;
    if all_radials.is_empty() {
        return Err("no radials decoded from input files".into());
    }

    match args.path {
        RenderPath::Radial => run_radial_path(&args, all_radials),
        RenderPath::Grid => run_grid_path(&args, all_radials),
    }
}

// ── Radial path (pre-existing: decoded radials → nearest-radial PPI) ──────────

fn run_radial_path(args: &Args, all_radials: Vec<Radial>) -> Result<(), String> {
    let ProductArg::Base(product) = args.product else {
        return Err("ECHO_TOPS/VIL have no radial-path rendering (they are volume-derived, \
                     not a per-sweep moment) — pass --path grid"
            .into());
    };

    let mut available_sweeps: Vec<u8> = all_radials.iter().map(|r| r.elevation_number).collect();
    available_sweeps.sort_unstable();
    available_sweeps.dedup();

    let sweep_radials: Vec<Radial> =
        all_radials.into_iter().filter(|r| r.elevation_number == args.sweep).collect();

    if sweep_radials.is_empty() {
        return Err(format!("sweep {} not found; available: {:?}", args.sweep, available_sweeps));
    }

    let has_product = sweep_radials.iter().any(|r| r.products.contains_key(&product));
    if !has_product {
        let mut available: Vec<&str> =
            sweep_radials.iter().flat_map(|r| r.products.keys()).map(|k| moment_kind_name(*k)).collect();
        available.sort_unstable();
        available.dedup();
        return Err(format!(
            "product {} not present in sweep {}; available: {:?}",
            moment_kind_name(product),
            args.sweep,
            available
        ));
    }

    let color_table = color_table_for(product);
    let range_km = args.range.unwrap_or_else(|| default_range_km(product));

    let (site_lat, site_lon) = site_lat_lon(&sweep_radials);

    eprintln!(
        "Rendering {} radials, sweep {}, product {}, range {} km, {}×{} px → {}",
        sweep_radials.len(),
        args.sweep,
        moment_kind_name(product),
        range_km,
        args.size,
        args.size,
        args.output.display()
    );

    let mut img = render_ppi(&sweep_radials, product, &color_table, range_km, args.size);
    draw_overlays(&mut img, args, site_lat, site_lon, range_km);

    png_out::write_png(&img, &args.output).map_err(|e| format!("failed to write PNG: {e}"))?;
    eprintln!("Done.");
    Ok(())
}

// ── Grid path (S3-W5: SweepGrid → ColorLut PPI, same code the app uses) ───────

fn run_grid_path(args: &Args, all_radials: Vec<Radial>) -> Result<(), String> {
    let (site_lat, site_lon) = site_lat_lon(&all_radials);
    let mut by_elevation = group_by_elevation(all_radials);

    let (sweep_grid, display_product) = match args.product {
        ProductArg::Base(kind) => {
            let display_product = display_product_for(kind).ok_or_else(|| {
                format!(
                    "{} has no grid-path mapping — PHI/CFP are deferred from v1.0 (Q8)",
                    moment_kind_name(kind)
                )
            })?;
            let radials = by_elevation.remove(&args.sweep).ok_or_else(|| {
                let available: Vec<u8> = by_elevation.keys().copied().collect();
                format!("sweep {} not found; available: {:?}", args.sweep, available)
            })?;
            let sweep = build_sweep(args.sweep, radials);
            let (grid, events) = grid::grid_sweep(&sweep, display_product).ok_or_else(|| {
                format!("{display_product} not present (or ungriddable) on sweep {}", args.sweep)
            })?;
            for event in &events {
                eprintln!("grid warning: {event}");
            }
            (grid, display_product)
        }
        ProductArg::EchoTops | ProductArg::Vil => {
            let wanted =
                if matches!(args.product, ProductArg::EchoTops) { DisplayProduct::EchoTops } else { DisplayProduct::Vil };

            let mut ref_grids = Vec::new();
            for (elevation_number, radials) in by_elevation {
                let sweep = build_sweep(elevation_number, radials);
                if let Some((grid, _events)) = grid::grid_sweep(&sweep, DisplayProduct::Reflectivity) {
                    ref_grids.push(Arc::new(grid));
                }
            }
            let (derived_grids, _events) = derived::compute_derived(&ref_grids);
            let grid = derived_grids
                .into_iter()
                .find(|g| g.product == wanted)
                .ok_or_else(|| format!("{wanted} could not be derived (need at least one reflectivity tilt)"))?;
            (Arc::try_unwrap(grid).unwrap_or_else(|shared| (*shared).clone()), wanted)
        }
    };

    let palette = palette::bundled_default(display_product);
    let lut = palette::compile_lut(&palette, sweep_grid.scale, sweep_grid.offset);
    let range_km = args.range.unwrap_or_else(|| default_range_km_for(display_product));

    eprintln!(
        "Rendering grid {}x{} ({} azimuths filled), product {}, range {} km, {}×{} px → {}",
        sweep_grid.azimuth_count,
        sweep_grid.gate_count,
        sweep_grid.filled_azimuths,
        display_product,
        range_km,
        args.size,
        args.size,
        args.output.display()
    );

    let mut img = render_grid::render_grid_ppi(&sweep_grid, &lut, range_km, args.size);
    draw_overlays(&mut img, args, site_lat, site_lon, range_km);

    png_out::write_png(&img, &args.output).map_err(|e| format!("failed to write PNG: {e}"))?;
    eprintln!("Done.");
    Ok(())
}

fn site_lat_lon(radials: &[Radial]) -> (f64, f64) {
    let lat = radials.iter().find_map(|r| r.site_parameters.as_ref().map(|vc| vc.latitude as f64)).unwrap_or(0.0);
    let lon = radials.iter().find_map(|r| r.site_parameters.as_ref().map(|vc| vc.longitude as f64)).unwrap_or(0.0);
    (lat, lon)
}

fn group_by_elevation(radials: Vec<Radial>) -> BTreeMap<u8, Vec<Radial>> {
    let mut by_elevation: BTreeMap<u8, Vec<Radial>> = BTreeMap::new();
    for radial in radials {
        by_elevation.entry(radial.elevation_number).or_default().push(radial);
    }
    by_elevation
}

/// A `Sweep` built directly from already-decoded radials — the same
/// synthetic-construction convention `radar-workstation`'s own compute-layer
/// tests use (`compute::test_support`), since this tool already has real
/// decoded `Radial`s in hand and gains nothing from going through
/// `VolumeAssembler` just to reassemble what it already has.
fn build_sweep(elevation_number: u8, radials: Vec<Radial>) -> Sweep {
    let elevation_deg = radials.first().map(|r| r.elevation_deg).unwrap_or(0.0);
    let nyquist_velocity_mps = radials.iter().find_map(|r| r.nyquist_velocity_mps);
    let unambiguous_range_km = radials.iter().find_map(|r| r.unambiguous_range_km);
    Sweep { elevation_number, elevation_deg, nyquist_velocity_mps, unambiguous_range_km, radials, complete: true }
}

fn display_product_for(kind: ProductKind) -> Option<DisplayProduct> {
    DisplayProduct::BASE.iter().find(|(_, k)| *k == kind).map(|(p, _)| *p)
}

fn draw_overlays(img: &mut png_out::Raster, args: &Args, site_lat: f64, site_lon: f64, range_km: f32) {
    // Overlays drawn back-to-front: counties (subtlest) → states → coastlines (most prominent).
    let Some(ref data_dir) = args.data_dir else { return };
    let overlay_specs: &[(&str, [u8; 4])] = &[
        ("ne_10m_admin_2_counties_lakes.shp", [70, 70, 70, 255]),
        ("ne_10m_admin_1_states_provinces.shp", [150, 150, 150, 255]),
        ("ne_10m_coastline.shp", [210, 210, 210, 255]),
    ];

    for (filename, color) in overlay_specs {
        let shp_path = data_dir.join(filename);
        if !shp_path.exists() {
            eprintln!("overlay not found, skipping: {}", shp_path.display());
            continue;
        }
        match OverlayLayer::from_path(&shp_path, *color) {
            Ok(layer) => draw_overlay(img, &layer, site_lat, site_lon, range_km, args.size),
            Err(e) => eprintln!("overlay warning: {e}"),
        }
    }
}

// ── Volume loading ────────────────────────────────────────────────────────────

fn load_volume(dir: &Path) -> Result<Vec<Radial>, String> {
    let files = chunk_files_in(dir)?;
    if files.is_empty() {
        return Err(format!(
            "no chunk files (-S/-I/-E) found in {}",
            dir.display()
        ));
    }

    let mut all_radials = Vec::new();
    for path in &files {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let data = fs::read(path).map_err(|e| format!("read {name}: {e}"))?;
        let kind = detect_chunk_kind(&data).map_err(|e| format!("detect {name}: {e}"))?;
        let decompressed =
            decompress_chunk(&data, kind).map_err(|e| format!("decompress {name}: {e}"))?;
        let mut radials = parse_radial_stream(&decompressed)
            .map_err(|e| format!("parse {name}: {e}"))?;
        all_radials.append(&mut radials);
    }
    Ok(all_radials)
}

fn chunk_files_in(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.ends_with("-S") || name.ends_with("-I") || name.ends_with("-E")
        })
        .collect();
    files.sort();
    Ok(files)
}

// ── Color tables and product metadata ────────────────────────────────────────

fn color_table_for(product: ProductKind) -> ColorTable {
    match product {
        ProductKind::Ref => ColorTable::nws_reflectivity(),
        ProductKind::Vel => ColorTable::nws_velocity(),
        ProductKind::SpectrumWidth  => ColorTable::spectrum_width(),
        _               => ColorTable::nws_reflectivity(),
    }
}

fn default_range_km(product: ProductKind) -> f32 {
    match product {
        ProductKind::Vel | ProductKind::SpectrumWidth => 115.0,
        _ => 230.0,
    }
}

fn default_range_km_for(product: DisplayProduct) -> f32 {
    match product {
        DisplayProduct::Velocity | DisplayProduct::SpectrumWidth => 115.0,
        _ => 230.0,
    }
}

fn moment_kind_name(k: ProductKind) -> &'static str {
    match k {
        ProductKind::Ref => "DREF",
        ProductKind::Vel => "DVEL",
        ProductKind::SpectrumWidth  => "DSW",
        ProductKind::Zdr => "DZDR",
        ProductKind::Phi => "DPHI",
        ProductKind::Rho => "DRHO",
        ProductKind::Cfp => "DCFP",
    }
}

// ── Argument parsing ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum RenderPath {
    Radial,
    Grid,
}

#[derive(Clone, Copy)]
enum ProductArg {
    Base(ProductKind),
    EchoTops,
    Vil,
}

struct Args {
    input: PathBuf,
    product: ProductArg,
    sweep: u8,
    range: Option<f32>,
    size: u32,
    output: PathBuf,
    data_dir: Option<PathBuf>,
    path: RenderPath,
}

const USAGE: &str = "\
Usage: radar-viz [OPTIONS] <input-dir>

Options:
  --path radial|grid                         default: radial (see this file's module doc comment)
  --product DREF|DVEL|DSW|DZDR|DPHI|DRHO|ECHO_TOPS|VIL
                                              default: DREF (ECHO_TOPS/VIL require --path grid)
  --sweep <n>                                 default: 1
  --range <km>                               default: 230 (DREF/ZDR/PHI/RHO/tops/VIL), 115 (VEL/SW)
  --size <pixels>                            default: 800
  --output <path>                            default: out.png
  --data <dir>                               directory containing Natural Earth .shp files";

fn parse_args() -> Result<Args, String> {
    let mut input: Option<PathBuf> = None;
    let mut product = ProductArg::Base(ProductKind::Ref);
    let mut sweep: u8 = 1;
    let mut range: Option<f32> = None;
    let mut size: u32 = 800;
    let mut output: Option<PathBuf> = None;
    let mut data_dir: Option<PathBuf> = None;
    let mut path = RenderPath::Radial;

    let mut iter = env::args().skip(1).peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--product" => {
                let val = iter.next().ok_or("--product requires a value")?;
                product = parse_product_arg(&val)?;
            }
            "--path" => {
                let val = iter.next().ok_or("--path requires a value")?;
                path = parse_path(&val)?;
            }
            "--sweep" => {
                let val = iter.next().ok_or("--sweep requires a value")?;
                sweep = val.parse::<u8>().map_err(|_| format!("invalid sweep: {val}"))?;
            }
            "--range" => {
                let val = iter.next().ok_or("--range requires a value")?;
                range = Some(val.parse::<f32>().map_err(|_| format!("invalid range: {val}"))?);
            }
            "--size" => {
                let val = iter.next().ok_or("--size requires a value")?;
                size = val.parse::<u32>().map_err(|_| format!("invalid size: {val}"))?;
            }
            "--output" => {
                let val = iter.next().ok_or("--output requires a value")?;
                output = Some(PathBuf::from(val));
            }
            "--data" => {
                let val = iter.next().ok_or("--data requires a value")?;
                data_dir = Some(PathBuf::from(val));
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                process::exit(0);
            }
            s if s.starts_with("--") => {
                return Err(format!("unknown option: {s}\n{USAGE}"));
            }
            _ => {
                if input.is_some() {
                    return Err(format!("unexpected argument: {arg}\n{USAGE}"));
                }
                input = Some(PathBuf::from(arg));
            }
        }
    }

    let input = input.ok_or_else(|| USAGE.to_string())?;
    let output = output.unwrap_or_else(|| PathBuf::from("out.png"));

    Ok(Args { input, product, sweep, range, size, output, data_dir, path })
}

fn parse_path(s: &str) -> Result<RenderPath, String> {
    match s {
        "radial" => Ok(RenderPath::Radial),
        "grid" => Ok(RenderPath::Grid),
        other => Err(format!("unknown --path '{other}'; expected radial or grid")),
    }
}

fn parse_product_arg(s: &str) -> Result<ProductArg, String> {
    match s.to_uppercase().as_str() {
        "ECHO_TOPS" | "ECHOTOPS" | "ET" => Ok(ProductArg::EchoTops),
        "VIL" => Ok(ProductArg::Vil),
        other => parse_product(other).map(ProductArg::Base),
    }
}

fn parse_product(s: &str) -> Result<ProductKind, String> {
    match s.to_uppercase().as_str() {
        "DREF" | "REF" | "Z" | "DBZ" => Ok(ProductKind::Ref),
        "DVEL" | "VEL" | "V"         => Ok(ProductKind::Vel),
        "DSW"  | "SW"  | "W"         => Ok(ProductKind::SpectrumWidth),
        "DZDR" | "ZDR"               => Ok(ProductKind::Zdr),
        "DPHI" | "PHI" | "KDP"       => Ok(ProductKind::Phi),
        "DRHO" | "RHO" | "CC"        => Ok(ProductKind::Rho),
        _ => Err(format!("unknown product '{s}'; expected DREF/DVEL/DSW/DZDR/DPHI/DRHO/ECHO_TOPS/VIL")),
    }
}
