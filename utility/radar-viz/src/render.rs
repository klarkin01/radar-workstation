use std::cmp::Ordering;

use nexrad_decoder::{ProductKind, Radial};

use crate::color_table::ColorTable;
use crate::png_out::Raster;

pub type PpiImage = Raster;

/// Maximum azimuthal gap (degrees) before a pixel is treated as no-data.
/// Guards against incorrectly coloring pixels in incomplete scans.
const MAX_AZ_GAP_DEG: f32 = 1.0;

/// Render a plan-position-indicator (PPI) image for one sweep.
///
/// Uses inverse mapping: for each output pixel, compute the corresponding
/// (range, azimuth) in radar coordinates, look up the nearest radial, then
/// look up the gate value. North is up; azimuth increases clockwise.
pub fn render_ppi(
    radials: &[Radial],
    product: ProductKind,
    color_table: &ColorTable,
    range_km: f32,
    size: u32,
) -> PpiImage {
    let bg = [15u8, 15, 15, 255];
    let mut img = Raster::filled(size, size, bg);

    if radials.is_empty() || range_km <= 0.0 || size == 0 {
        return img;
    }

    let mut sorted: Vec<&Radial> = radials.iter().collect();
    sorted.sort_by(|a, b| {
        a.azimuth_deg
            .partial_cmp(&b.azimuth_deg)
            .unwrap_or(Ordering::Equal)
    });

    let cx = size as f32 / 2.0;
    let cy = size as f32 / 2.0;
    let km_per_pixel = range_km / (size as f32 / 2.0);

    for y in 0..size {
        for x in 0..size {
            let x_km = (x as f32 - cx) * km_per_pixel;
            let y_km = (cy - y as f32) * km_per_pixel; // N=up, so y axis is flipped

            let range = (x_km * x_km + y_km * y_km).sqrt();
            if range > range_km {
                continue;
            }

            // atan2(x, y) → azimuth with 0°=N, increasing clockwise
            let mut az = x_km.atan2(y_km).to_degrees();
            if az < 0.0 {
                az += 360.0;
            }

            let (radial, az_dist) = nearest_radial(&sorted, az);
            if az_dist > MAX_AZ_GAP_DEG {
                continue;
            }

            let color = match radial.products.get(&product) {
                None => color_table.no_data,
                Some(moment) => {
                    let first_km = moment.first_gate_m as f32 / 1000.0;
                    let width_km = moment.gate_width_m as f32 / 1000.0;
                    if width_km <= 0.0 {
                        color_table.no_data
                    } else {
                        let gate = ((range - first_km) / width_km) as i32;
                        if gate < 0 || gate as u16 >= moment.gate_count {
                            color_table.no_data
                        } else {
                            // Raw value 1 is the ICD range-fold flag; 0 is below SNR.
                            // Check raw before physical_value() so they render differently.
                            if moment.raw_gate(gate as usize) == Some(1) {
                                color_table.range_fold
                            } else {
                                color_table.lookup(moment.physical_value(gate as usize))
                            }
                        }
                    }
                }
            };

            // Only write non-transparent colors; transparent entries fall through to background.
            if color[3] > 0 {
                img.put_pixel(x, y, color);
            }
        }
    }

    img
}

/// Returns the nearest radial to `az` and the angular distance to it.
fn nearest_radial<'a>(sorted: &[&'a Radial], az: f32) -> (&'a Radial, f32) {
    let n = sorted.len();
    let pos = sorted.partition_point(|r| r.azimuth_deg < az);

    // At the 0°/360° boundary, one candidate may wrap around.
    let candidates = if pos == 0 || pos == n {
        [n - 1, 0]
    } else {
        [pos - 1, pos]
    };

    candidates
        .iter()
        .map(|&i| {
            let r = sorted[i];
            (r, az_diff(r.azimuth_deg, az))
        })
        .min_by(|(_, da), (_, db)| da.partial_cmp(db).unwrap_or(Ordering::Equal))
        .unwrap()
}

fn az_diff(a: f32, b: f32) -> f32 {
    let d = (a - b).abs();
    if d > 180.0 { 360.0 - d } else { d }
}
