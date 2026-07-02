/// radar-viz — render decoded NEXRAD chunk data as a PNG PPI image.
///
/// Usage:
///   radar-viz [OPTIONS] <input-dir>
///
/// Products: DREF (reflectivity), DVEL (velocity), DSW (spectrum width),
///           DZDR (differential reflectivity), DPHI (differential phase),
///           DRHO (correlation coefficient)
use std::{env, fs, path::{Path, PathBuf}, process};

use nexrad_decoder::{MomentKind, Radial, parse_radial_stream};
use radar_workstation::decompress_chunk;

mod color_table;
mod overlay;
mod render;

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

    let mut available_tilts: Vec<u8> =
        all_radials.iter().map(|r| r.elevation_number).collect();
    available_tilts.sort_unstable();
    available_tilts.dedup();

    let tilt_radials: Vec<Radial> = all_radials
        .into_iter()
        .filter(|r| r.elevation_number == args.tilt)
        .collect();

    if tilt_radials.is_empty() {
        return Err(format!(
            "tilt {} not found; available: {:?}",
            args.tilt, available_tilts
        ));
    }

    let has_product = tilt_radials.iter().any(|r| r.moments.contains_key(&args.product));
    if !has_product {
        let mut available: Vec<&str> = tilt_radials
            .iter()
            .flat_map(|r| r.moments.keys())
            .map(|k| moment_kind_name(*k))
            .collect();
        available.sort_unstable();
        available.dedup();
        return Err(format!(
            "product {} not present in tilt {}; available: {:?}",
            moment_kind_name(args.product),
            args.tilt,
            available
        ));
    }

    let color_table = color_table_for(args.product);
    let range_km = args.range.unwrap_or_else(|| default_range_km(args.product));

    // Extract site coordinates from the RVOL block present on any radial.
    let site_lat = tilt_radials
        .iter()
        .find_map(|r| r.volume_constants.as_ref().map(|vc| vc.latitude as f64))
        .unwrap_or(0.0);
    let site_lon = tilt_radials
        .iter()
        .find_map(|r| r.volume_constants.as_ref().map(|vc| vc.longitude as f64))
        .unwrap_or(0.0);

    eprintln!(
        "Rendering {} radials, tilt {}, product {}, range {} km, {}×{} px → {}",
        tilt_radials.len(),
        args.tilt,
        moment_kind_name(args.product),
        range_km,
        args.size,
        args.size,
        args.output.display()
    );

    let mut img = render_ppi(&tilt_radials, args.product, &color_table, range_km, args.size);

    // Overlays drawn back-to-front: counties (subtlest) → states → coastlines (most prominent).
    if let Some(ref data_dir) = args.data_dir {
        let overlay_specs: &[(&str, [u8; 4])] = &[
            ("ne_10m_admin_2_counties_lakes.shp", [70,  70,  70,  255]),
            ("ne_10m_admin_1_states_provinces.shp", [150, 150, 150, 255]),
            ("ne_10m_coastline.shp",                [210, 210, 210, 255]),
        ];

        for (filename, color) in overlay_specs {
            let shp_path = data_dir.join(filename);
            if !shp_path.exists() {
                eprintln!("overlay not found, skipping: {}", shp_path.display());
                continue;
            }
            match OverlayLayer::from_path(&shp_path, *color) {
                Ok(layer) => draw_overlay(&mut img, &layer, site_lat, site_lon, range_km, args.size),
                Err(e) => eprintln!("overlay warning: {e}"),
            }
        }
    }

    img.save(&args.output)
        .map_err(|e| format!("failed to write PNG: {e}"))?;

    eprintln!("Done.");
    Ok(())
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
        let decompressed =
            decompress_chunk(&data).map_err(|e| format!("decompress {name}: {e}"))?;
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

fn color_table_for(product: MomentKind) -> ColorTable {
    match product {
        MomentKind::Ref => ColorTable::nws_reflectivity(),
        MomentKind::Vel => ColorTable::nws_velocity(),
        MomentKind::Sw  => ColorTable::spectrum_width(),
        _               => ColorTable::nws_reflectivity(),
    }
}

fn default_range_km(product: MomentKind) -> f32 {
    match product {
        MomentKind::Vel | MomentKind::Sw => 115.0,
        _ => 230.0,
    }
}

fn moment_kind_name(k: MomentKind) -> &'static str {
    match k {
        MomentKind::Ref => "DREF",
        MomentKind::Vel => "DVEL",
        MomentKind::Sw  => "DSW",
        MomentKind::Zdr => "DZDR",
        MomentKind::Phi => "DPHI",
        MomentKind::Rho => "DRHO",
        MomentKind::Cfp => "DCFP",
    }
}

// ── Argument parsing ──────────────────────────────────────────────────────────

struct Args {
    input: PathBuf,
    product: MomentKind,
    tilt: u8,
    range: Option<f32>,
    size: u32,
    output: PathBuf,
    data_dir: Option<PathBuf>,
}

const USAGE: &str = "\
Usage: radar-viz [OPTIONS] <input-dir>

Options:
  --product DREF|DVEL|DSW|DZDR|DPHI|DRHO   default: DREF
  --tilt <n>                                 default: 1
  --range <km>                               default: 230 (DREF), 115 (DVEL/DSW)
  --size <pixels>                            default: 800
  --output <path>                            default: out.png
  --data <dir>                               directory containing Natural Earth .shp files";

fn parse_args() -> Result<Args, String> {
    let mut input: Option<PathBuf> = None;
    let mut product = MomentKind::Ref;
    let mut tilt: u8 = 1;
    let mut range: Option<f32> = None;
    let mut size: u32 = 800;
    let mut output: Option<PathBuf> = None;
    let mut data_dir: Option<PathBuf> = None;

    let mut iter = env::args().skip(1).peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--product" => {
                let val = iter.next().ok_or("--product requires a value")?;
                product = parse_product(&val)?;
            }
            "--tilt" => {
                let val = iter.next().ok_or("--tilt requires a value")?;
                tilt = val.parse::<u8>().map_err(|_| format!("invalid tilt: {val}"))?;
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

    Ok(Args { input, product, tilt, range, size, output, data_dir })
}

fn parse_product(s: &str) -> Result<MomentKind, String> {
    match s.to_uppercase().as_str() {
        "DREF" | "REF" | "Z" | "DBZ" => Ok(MomentKind::Ref),
        "DVEL" | "VEL" | "V"         => Ok(MomentKind::Vel),
        "DSW"  | "SW"  | "W"         => Ok(MomentKind::Sw),
        "DZDR" | "ZDR"               => Ok(MomentKind::Zdr),
        "DPHI" | "PHI" | "KDP"       => Ok(MomentKind::Phi),
        "DRHO" | "RHO" | "CC"        => Ok(MomentKind::Rho),
        _ => Err(format!("unknown product '{s}'; expected DREF/DVEL/DSW/DZDR/DPHI/DRHO")),
    }
}
