/// decode_sample — decode a directory of NEXRAD chunk files and print a summary.
///
/// Usage:
///   decode_sample [DIR]      decode all chunks in DIR
///   decode_sample            scans downloads/ for chunk directories, decodes each
///
/// Prints per-chunk statistics: chunk kind, site, VCP, radial count, tilt inventory,
/// and moment/gate geometry. Intended for end-to-end pipeline validation during
/// development; not a production binary.
use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    process,
};

use nexrad_decoder::{parse_radial_stream, MomentKind, RadialStatus};
use radar_workstation::{decompress_chunk, detect_chunk_kind, ChunkKind};

fn main() {
    let mut args = env::args().skip(1);
    let dirs: Vec<PathBuf> = if let Some(arg) = args.next() {
        vec![PathBuf::from(arg)]
    } else {
        // Default: find subdirectories of downloads/
        find_volume_dirs(Path::new("downloads"))
    };

    if dirs.is_empty() {
        eprintln!("No chunk directories found. Pass a directory or put volumes in downloads/.");
        process::exit(1);
    }

    let mut had_error = false;
    for dir in &dirs {
        if let Err(e) = decode_dir(dir) {
            eprintln!("error decoding {}: {e}", dir.display());
            had_error = true;
        }
    }
    if had_error {
        process::exit(1);
    }
}

fn find_volume_dirs(downloads: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(downloads) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn chunk_files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.ends_with("-S") || name.ends_with("-I") || name.ends_with("-E")
        })
        .collect();
    files.sort();
    files
}

fn decode_dir(dir: &Path) -> Result<(), String> {
    let files = chunk_files_in(dir);
    if files.is_empty() {
        return Err("no chunk files found".into());
    }

    println!("=== {} ({} chunks) ===", dir.display(), files.len());

    // Tilt inventory across the whole volume: elevation_number → (mean_angle, radials, moments)
    let mut volume_tilts: HashMap<u8, TiltStats> = HashMap::new();
    let mut volume_radials = 0usize;

    for path in &files {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let data = fs::read(path).map_err(|e| format!("read {name}: {e}"))?;
        let kind = detect_chunk_kind(&data).map_err(|e| format!("detect {name}: {e}"))?;
        let decompressed =
            decompress_chunk(&data).map_err(|e| format!("decompress {name}: {e}"))?;
        let radials =
            parse_radial_stream(&decompressed).map_err(|e| format!("parse {name}: {e}"))?;

        let kind_label = match kind {
            ChunkKind::Start => "S",
            ChunkKind::Intermediate => "I",
            ChunkKind::End => "E",
        };

        print!("  [{kind_label}] {name:30}  raw {:6}B  decomp {:7}B", data.len(), decompressed.len());

        if radials.is_empty() {
            println!("  (metadata only)");
            continue;
        }

        volume_radials += radials.len();

        let status_counts = count_statuses(&radials);
        let status_str = format_status_counts(&status_counts);
        println!("  {} radials  {status_str}", radials.len());

        // Accumulate per-tilt stats for the volume summary
        for r in &radials {
            let tilt = volume_tilts.entry(r.elevation_number).or_default();
            tilt.elevation_angles.push(r.elevation_deg);
            tilt.radial_count += 1;
            for (kind, md) in &r.moments {
                tilt.moments.entry(*kind).or_insert(md.gate_count);
            }
        }
    }

    // Volume summary
    if volume_radials > 0 {
        let moment_labels = [
            (MomentKind::Ref, "DREF"),
            (MomentKind::Vel, "DVEL"),
            (MomentKind::Sw,  "DSW "),
            (MomentKind::Zdr, "DZDR"),
            (MomentKind::Phi, "DPHI"),
            (MomentKind::Rho, "DRHO"),
            (MomentKind::Cfp, "DCFP"),
        ];

        println!();
        println!("  Volume: {volume_radials} total radials across {} tilts", volume_tilts.len());
        println!("  {:>4}  {:>6}  {:>7}  moments", "Tilt", "ElAngle", "Radials");
        let mut tilt_order: Vec<u8> = volume_tilts.keys().copied().collect();
        tilt_order.sort_unstable();
        for el_num in &tilt_order {
            let tilt = &volume_tilts[el_num];
            let mean = tilt.elevation_angles.iter().sum::<f32>() / tilt.elevation_angles.len() as f32;
            let moments: Vec<String> = moment_labels
                .iter()
                .filter(|(k, _)| tilt.moments.contains_key(k))
                .map(|(k, name)| format!("{}({}g)", name, tilt.moments[k]))
                .collect();
            println!("  {:>4}  {:>6.2}°  {:>7}  {}", el_num, mean, tilt.radial_count, moments.join("  "));
        }
    }
    println!();

    Ok(())
}

#[derive(Default)]
struct TiltStats {
    elevation_angles: Vec<f32>,
    radial_count: u32,
    moments: HashMap<MomentKind, u16>,
}

fn count_statuses(radials: &[nexrad_decoder::Radial]) -> [u32; 6] {
    let mut counts = [0u32; 6];
    for r in radials {
        let i = match r.radial_status {
            RadialStatus::StartOfElevation     => 0,
            RadialStatus::Intermediate         => 1,
            RadialStatus::EndOfElevation       => 2,
            RadialStatus::StartOfVolume        => 3,
            RadialStatus::EndOfVolume          => 4,
            RadialStatus::StartOfElevationSails => 5,
        };
        counts[i] += 1;
    }
    counts
}

fn format_status_counts(counts: &[u32; 6]) -> String {
    let labels = ["SOEl", "Int", "EOEl", "SOVol", "EOVol", "SAILS"];
    counts
        .iter()
        .zip(labels.iter())
        .filter(|(&n, _)| n > 0)
        .map(|(&n, &label)| format!("{label}:{n}"))
        .collect::<Vec<_>>()
        .join(" ")
}
