use std::path::Path;

use shapefile::Shape;

use crate::png_out::Raster;

/// A collection of projected polyline parts ready to draw.
pub struct OverlayLayer {
    /// Each element is one contiguous part: a sequence of (lon, lat) pairs.
    parts: Vec<Vec<(f64, f64)>>,
    pub color: [u8; 4],
}

impl OverlayLayer {
    /// Load geometry from a shapefile. Handles both Polyline and Polygon shape types;
    /// polygon rings are treated as closed polylines (all rings, including holes, are drawn,
    /// since lake shores and island coastlines are useful radar context).
    pub fn from_path(path: &Path, color: [u8; 4]) -> Result<Self, String> {
        let shapes = shapefile::read_shapes(path)
            .map_err(|e| format!("read {}: {e}", path.display()))?;

        let mut parts: Vec<Vec<(f64, f64)>> = Vec::new();

        for shape in shapes {
            match shape {
                Shape::Polyline(pl) => {
                    for part in pl.parts() {
                        if part.len() >= 2 {
                            parts.push(part.iter().map(|p| (p.x, p.y)).collect());
                        }
                    }
                }
                Shape::Polygon(pg) => {
                    for ring in pg.rings() {
                        let pts: &[shapefile::Point] = ring.as_ref();
                        if pts.len() >= 2 {
                            parts.push(pts.iter().map(|p| (p.x, p.y)).collect());
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(Self { parts, color })
    }
}

/// Draw a loaded overlay onto `img` using the same coordinate system as `render_ppi`.
pub fn draw_overlay(
    img: &mut Raster,
    layer: &OverlayLayer,
    site_lat: f64,
    site_lon: f64,
    range_km: f32,
    size: u32,
) {
    let cx = size as f32 / 2.0;
    let cy = size as f32 / 2.0;
    let pixels_per_km = cx / range_km;
    let color = layer.color;

    for part in &layer.parts {
        let pixels: Vec<(i32, i32)> = part
            .iter()
            .map(|&(lon, lat)| {
                // The production az-eq projection (compute::geometry,
                // S5-c/ADR-0025 §4 erratum) — this used to be a private
                // copy here; deleting it and calling the production
                // function is what makes ADR-0025 §4's DRY claim true.
                // Metres -> km at this call site, since radar-viz's raster
                // math is in km/f32.
                let (x_m, y_m) = radar_workstation::compute::geometry::az_eq_project(site_lat, site_lon, lat, lon);
                let (x_km, y_km) = (x_m / 1000.0, y_m / 1000.0);
                let px = (cx + x_km as f32 * pixels_per_km).round() as i32;
                let py = (cy - y_km as f32 * pixels_per_km).round() as i32;
                (px, py)
            })
            .collect();

        for seg in pixels.windows(2) {
            draw_line(img, seg[0].0, seg[0].1, seg[1].0, seg[1].1, size, color);
        }
    }
}

/// Bresenham line draw with per-pixel bounds clipping.
/// Trivially rejects segments that lie entirely outside one edge of the image.
fn draw_line(
    img: &mut Raster,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    size: u32,
    color: [u8; 4],
) {
    let (w, h) = (size as i32, size as i32);
    if x0 < 0 && x1 < 0 { return; }
    if x0 >= w && x1 >= w { return; }
    if y0 < 0 && y1 < 0 { return; }
    if y0 >= h && y1 >= h { return; }

    let (mut x, mut y) = (x0, y0);
    let dx = (x1 - x0).abs();
    let dy = (y1 - y0).abs();
    let sx: i32 = if x0 < x1 { 1 } else { -1 };
    let sy: i32 = if y0 < y1 { 1 } else { -1 };
    let mut err = dx - dy;

    loop {
        if x >= 0 && x < w && y >= 0 && y < h {
            img.put_pixel(x as u32, y as u32, color);
        }
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 > -dy {
            err -= dy;
            x += sx;
        }
        if e2 < dx {
            err += dx;
            y += sy;
        }
    }
}
