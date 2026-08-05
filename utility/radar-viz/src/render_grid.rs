//! Grid render path (S3-W5 §8.1): draws a `SweepGrid` through a compiled
//! `ColorLut` — grid → LUT → PNG — instead of `render.rs`'s
//! radial-list-plus-nearest-radial-search path. The two rendering the same
//! fixture, product, and sweep should be visually indistinguishable apart
//! from interpolation at the seams; that comparison is the check that
//! catches an azimuth-binning error, an off-by-one in the gate index, or a
//! byte-order mistake in the LUT — none of which any unit test in
//! `compute::grid` would notice, because those tests never touch a screen
//! raster.
//!
//! Unlike `render.rs`'s `nearest_radial` (which exists because it renders
//! onto a *screen* raster from a sparse, possibly-gapped radial list), this
//! path does no search: a polar grid already has a slot for every azimuth
//! the antenna could have filled, so mapping a screen pixel to a cell is a
//! direct lookup via `compute::grid::azimuth_slot` — the exact rule
//! Stage 3's gridding uses, reused rather than re-derived.

use radar_workstation::compute::grid::{azimuth_slot, SweepGrid};
use radar_workstation::compute::palette::ColorLut;

use crate::png_out::Raster;

pub fn render_grid_ppi(grid: &SweepGrid, lut: &ColorLut, range_km: f32, size: u32) -> Raster {
    let bg = [15u8, 15, 15, 255];
    let mut img = Raster::filled(size, size, bg);

    if range_km <= 0.0 || size == 0 || grid.azimuth_count == 0 || grid.gate_count == 0 || grid.gate_width_m == 0 {
        return img;
    }

    let cx = size as f32 / 2.0;
    let cy = size as f32 / 2.0;
    let km_per_pixel = range_km / (size as f32 / 2.0);
    let first_km = grid.first_gate_m as f32 / 1000.0;
    let width_km = grid.gate_width_m as f32 / 1000.0;

    for y in 0..size {
        for x in 0..size {
            let x_km = (x as f32 - cx) * km_per_pixel;
            let y_km = (cy - y as f32) * km_per_pixel; // N=up, so y axis is flipped

            let range = (x_km * x_km + y_km * y_km).sqrt();
            if range > range_km {
                continue;
            }

            let gate = ((range - first_km) / width_km) as i32;
            if gate < 0 || gate as u16 >= grid.gate_count {
                continue;
            }

            // atan2(x, y) → azimuth with 0°=N, increasing clockwise —
            // matches render.rs's convention exactly, so the two paths are
            // comparable pixel-for-pixel.
            let mut az = x_km.atan2(y_km).to_degrees();
            if az < 0.0 {
                az += 360.0;
            }
            let slot = azimuth_slot(az, grid.azimuth_count);

            let color = lut[grid.cell(slot, gate as u16) as usize];
            if color[3] > 0 {
                img.put_pixel(x, y, color);
            }
        }
    }

    img
}
