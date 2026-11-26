//! Offscreen visual verification for the map vector pass (§12.2, §12.3) and
//! for those lines composited over radar data (ADR-0032).
//! `#[ignore]`d — this environment (and CI) has no presentable display, but
//! *offscreen* rendering with read-back works, the same path Stage 4 used
//! to verify the radar pass (`render::radar::tests`). Run with:
//!
//!   cargo test -p radar-workstation --bins offscreen -- --ignored --nocapture
//!
//! §12.3's three renders write binary PPM (`P6`) files to `target/` — PPM
//! because `render/` is binary-side and cannot reach `radar-viz`'s PNG
//! encoder (ADR-0022/S4-f), and a PPM writer is ten lines with no
//! dependency.

#![cfg(test)]

use std::collections::BTreeMap;
use std::sync::Arc;

use radar_workstation::compute::cut::cut_key;
use radar_workstation::compute::{palette, DisplayProduct, SweepGrid};
use radar_workstation::sites;

use super::overlay::OverlayRenderer;
use super::radar::{RadarRenderer, Selection};
use super::reference::ReferenceRenderer;
use super::test_support::{frame, snapshot, volume};
use super::view::{Camera, ViewState};
use super::Scene;

fn try_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster_block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .ok()?;
    pollster_block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("overlay offscreen test device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::MemoryUsage,
        trace: wgpu::Trace::Off,
    }))
    .ok()
}

/// A tiny blocking executor, same shape as `render::radar::tests`' copy —
/// no async runtime, no `unsafe`.
fn pollster_block_on<F: std::future::Future>(fut: F) -> F::Output {
    use std::task::{Context, Poll, Wake};

    struct Noop;
    impl Wake for Noop {
        fn wake(self: Arc<Self>) {}
    }

    let waker = Arc::new(Noop).into();
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(fut);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

fn read_texture(device: &wgpu::Device, queue: &wgpu::Queue, mut encoder: wgpu::CommandEncoder, texture: &wgpu::Texture, size: u32) -> Vec<u8> {
    let bytes_per_row = (size * 4).next_multiple_of(256);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (bytes_per_row * size) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(size) },
        },
        wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).ok();
    let mapped = slice.get_mapped_range().unwrap();
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        let src = (y * bytes_per_row) as usize;
        let dst = (y * size * 4) as usize;
        out[dst..dst + (size * 4) as usize].copy_from_slice(&mapped[src..src + (size * 4) as usize]);
    }
    out
}

fn pixel(buf: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * size + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

/// Writes `rgba` (from `read_texture`) as a binary PPM (`P6`) — alpha is
/// dropped, which is fine for a visual check.
fn write_ppm(path: &std::path::Path, rgba: &[u8], size: u32) {
    use std::io::Write;
    let mut out = Vec::with_capacity(rgba.len());
    out.extend_from_slice(format!("P6\n{size} {size}\n255\n").as_bytes());
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&px[0..3]);
    }
    let mut file = std::fs::File::create(path).expect("create ppm output file");
    file.write_all(&out).expect("write ppm");
}

/// The cut every composite render's synthetic sweep sits on.
const COMPOSITE_CUT_DEG: f32 = 0.5;

/// A synthetic sweep reaching 230 km, split into equal azimuth sectors, one
/// per entry of `physical` (in the product's units) — so one render puts
/// the map lines over every listed value. Every cell is real data, so the
/// whole disc is opaque.
fn sector_grid(product: DisplayProduct, scale: f32, offset: f32, physical: &[f32]) -> Arc<SweepGrid> {
    const AZIMUTHS: u16 = 360;
    const GATES: u16 = 920; // 920 × 250 m = 230 km
    let raws: Vec<u8> = physical.iter().map(|v| (v * scale + offset).round() as u8).collect();
    let mut cells = Vec::with_capacity(AZIMUTHS as usize * GATES as usize);
    for a in 0..AZIMUTHS as usize {
        let raw = raws[a * raws.len() / AZIMUTHS as usize];
        cells.extend(std::iter::repeat_n(raw, GATES as usize));
    }
    Arc::new(SweepGrid {
        product,
        azimuth_count: AZIMUTHS,
        gate_count: GATES,
        first_gate_m: 0,
        gate_width_m: 250,
        elevation_number: 1,
        elevation_deg: COMPOSITE_CUT_DEG + 0.02,
        cut_deg: COMPOSITE_CUT_DEG,
        nyquist_velocity_mps: Some(30.0),
        scale,
        offset,
        cells,
        filled_azimuths: AZIMUTHS,
    })
}

/// Pass 1 exactly as a frame records it (`render::Scene`), optionally over
/// `radar_grid` (synced through the real `RadarRenderer` with the bundled
/// palette) and optionally without the map vectors, for a with/without
/// comparison.
fn render_pass1_scene(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: u32,
    camera: Camera,
    site_id: &str,
    radar_grid: Option<&Arc<SweepGrid>>,
    draw_overlay: bool,
) -> Vec<u8> {
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let site = sites::by_id(site_id).expect("site must be bundled");
    let bundle = radar_workstation::overlay::bundled().expect("committed bundle must parse");
    let (projected, events) = radar_workstation::overlay::project(bundle, site);
    assert!(events.is_empty(), "unexpected overlay events: {events:?}");
    let overlay = OverlayRenderer::new(device, format, &projected, site);
    let reference = ReferenceRenderer::new(device, format);
    let mut radar = RadarRenderer::new(device, format, false, 2048);
    if let Some(grid) = radar_grid {
        let mut f = (*frame(0, 35, &[(grid.elevation_number, grid.cut_deg, &[grid.product])])).clone();
        let mut sweep = f.sweeps[&grid.elevation_number].clone();
        sweep.grids = vec![Arc::clone(grid)];
        f.insert_sweep(sweep);
        let palettes = BTreeMap::from([(grid.product, palette::bundled_default(grid.product))]);
        let selection =
            Selection { product: grid.product, cut: Some(cut_key(grid.cut_deg)), focus: None, displayed: Some((volume(0), grid)) };
        let (events, more_pending) = radar.sync(device, queue, &snapshot(vec![Arc::new(f)]), &selection, &palettes);
        assert!(events.is_empty() && !more_pending, "synthetic grid must sync in one call: {events:?}");
    }

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen scene target"),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("offscreen scene pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        Scene {
            radar: &mut radar,
            radar_grid: radar_grid.map(|g| (&**g, volume(0))),
            overlay: draw_overlay.then_some(&overlay),
            reference: Some(&reference),
            show_highways: true,
        }
        .draw(device, queue, &mut pass, camera);
    }
    read_texture(device, queue, encoder, &target, size)
}

// --- §12.2: GPU-gated unit tests for the overlay pass itself ---

#[test]
#[ignore = "requires a GPU adapter"]
fn offscreen_overlay_pass_draws_lines() {
    let Some((device, queue)) = try_device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;

    // A synthetic two-layer Projected: a horizontal line through the
    // origin (kind 1) and a vertical line (kind 2).
    let projected = radar_workstation::overlay::Projected {
        vertices: vec![[-50_000.0, 0.0], [50_000.0, 0.0], [0.0, -50_000.0], [0.0, 50_000.0]],
        indices: vec![0, 1, 2, 3],
        layers: vec![
            radar_workstation::overlay::ProjectedLayer { kind: 1, index_range: 0..2 },
            radar_workstation::overlay::ProjectedLayer { kind: 2, index_range: 2..4 },
        ],
        labels: vec![],
    };
    let site = sites::by_id("KDOX").unwrap();
    let overlay = OverlayRenderer::new(&device, format, &projected, site);

    let size = 64u32;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test target"),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let camera = Camera { center_m: (0.0, 0.0), m_per_px: 2_000.0, viewport: (size as f32, size as f32) };
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("test pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        overlay.draw(&queue, &mut pass, camera, true);
    }
    let readback = read_texture(&device, &queue, encoder, &target, size);

    // The horizontal line runs along row `size/2`, the vertical line along
    // column `size/2` — checking the whole row/column (rather than the
    // exact centre pixel, which sits on a rasterisation edge case for a
    // 1 px line through an exact viewport centre) is robust to which side
    // of that edge case the line lands on.
    let row_has_line = (0..size).any(|x| pixel(&readback, size, x, size / 2) != [0, 0, 0, 255]);
    let col_has_line = (0..size).any(|y| pixel(&readback, size, size / 2, y) != [0, 0, 0, 255]);
    let corner = pixel(&readback, size, 4, 4);
    assert!(row_has_line, "expected the horizontal line's colour somewhere in the centre row");
    assert!(col_has_line, "expected the vertical line's colour somewhere in the centre column");
    assert_eq!(corner, [0, 0, 0, 255], "expected background far from either line");
}

/// §3.4's hazard, in a test: `Queue::write_buffer` is ordered relative to
/// *submission*, not relative to draw calls already recorded — writing one
/// shared uniform buffer twice in a frame gives both draws the second
/// value. Two layers with distinct colours, read back one pixel from each.
#[test]
#[ignore = "requires a GPU adapter"]
fn overlay_layers_use_distinct_uniform_buffers() {
    let Some((device, queue)) = try_device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let site = sites::by_id("KDOX").unwrap();
    let bundle = radar_workstation::overlay::bundled().expect("committed bundle must parse");
    let (projected, _events) = radar_workstation::overlay::project(bundle, site);
    let overlay = OverlayRenderer::new(&device, format, &projected, site);

    let size = 256u32;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("uniform-distinctness target"),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    // The default 230 km site-centred view — confirmed (§12.3's KDOX
    // render) to have county, state, coastline and road geometry all
    // visible at once; a tight zoom risks a window with only one layer
    // nearby, which would fail this test for the wrong reason.
    let m_per_px = super::view::fit_range(super::view::DEFAULT_RANGE_M, (size as f32, size as f32));
    let camera = Camera { center_m: (0.0, 0.0), m_per_px, viewport: (size as f32, size as f32) };
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("uniform-distinctness pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        overlay.draw(&queue, &mut pass, camera, true);
    }
    let readback = read_texture(&device, &queue, encoder, &target, size);

    // Every non-background pixel's colour must be one of the four layer
    // colours (or their alpha-blended-over-black variants) — never all the
    // same *single* colour, which is what the write-after-record hazard
    // would produce (every layer painting in whichever colour was written
    // last).
    let mut distinct_nonzero: std::collections::HashSet<[u8; 4]> = std::collections::HashSet::new();
    for chunk in readback.chunks_exact(4) {
        let px = [chunk[0], chunk[1], chunk[2], chunk[3]];
        if px != [0, 0, 0, 255] {
            distinct_nonzero.insert(px);
        }
    }
    assert!(
        distinct_nonzero.len() > 1,
        "expected more than one distinct non-background colour (counties/states/coastline/roads \
         each have their own colour); the write-after-record hazard collapses them all to one"
    );
}

// --- §12.3: the three visual checks, writing PPM to target/ ---

fn write_scene_ppm(site_id: &str, m_per_px: f64, out_name: &str) {
    let Some((device, queue)) = try_device() else {
        eprintln!("no GPU adapter; skipping {out_name}");
        return;
    };
    let size = 512u32;
    let camera = Camera { center_m: (0.0, 0.0), m_per_px, viewport: (size as f32, size as f32) };
    let rgba = render_pass1_scene(&device, &queue, size, camera, site_id, None, true);
    write_target_ppm(&rgba, size, out_name);
}

fn write_target_ppm(rgba: &[u8], size: u32, out_name: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target").join(out_name);
    write_ppm(&path, rgba, size);
    eprintln!("wrote {}", path.display());
}

#[test]
#[ignore = "requires a GPU adapter; writes target/*.ppm for manual inspection"]
fn kdox_230km_no_radar_data() {
    // 230 km fitted to a 512x512 viewport's short axis.
    let view = ViewState::initial((512.0, 512.0), DisplayProduct::Reflectivity, super::timeline::Timeline::from_config(&Default::default()));
    write_scene_ppm("KDOX", view.m_per_px, "overlay_kdox_230km.ppm");
}

#[test]
#[ignore = "requires a GPU adapter; writes target/*.ppm for manual inspection"]
fn krlx_60_m_per_px_max_zoom() {
    write_scene_ppm("KRLX", super::view::MIN_M_PER_PX, "overlay_krlx_60mpx.ppm");
}

#[test]
#[ignore = "requires a GPU adapter; writes target/*.ppm for manual inspection"]
fn pabc_230km_road_less_site() {
    let view = ViewState::initial((512.0, 512.0), DisplayProduct::Reflectivity, super::timeline::Timeline::from_config(&Default::default()));
    write_scene_ppm("PABC", view.m_per_px, "overlay_pabc_230km.ppm");
}

// --- ADR-0032: the map vectors composite *over* the radar image ---

/// Renders the KDOX 230 km scene over strong reflectivity (45–75 dBZ) and
/// over a full-range velocity sweep, with and without the map vectors.
/// Every pixel of the disc is opaque radar data, so under the Stage 5
/// order (vectors under the radar) the two renders would be identical
/// inside it; here the lines must change a substantial number of pixels,
/// by a clearly visible amount. Writes both composites to `target/` for
/// the eye check ADR-0032 §3 records.
#[test]
#[ignore = "requires a GPU adapter; writes target/*.ppm for manual inspection"]
fn map_lines_read_over_strong_echoes_and_velocity() {
    let Some((device, queue)) = try_device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let size = 512u32;
    let m_per_px = super::view::fit_range(super::view::DEFAULT_RANGE_M, (size as f32, size as f32));
    let camera = Camera { center_m: (0.0, 0.0), m_per_px, viewport: (size as f32, size as f32) };
    let cases: [(DisplayProduct, f32, f32, &[f32], &str); 2] = [
        (DisplayProduct::Reflectivity, 2.0, 66.0, &[45.0, 50.0, 55.0, 60.0, 65.0, 70.0, 75.0], "composite_kdox_ref_strong.ppm"),
        (DisplayProduct::Velocity, 2.0, 129.0, &[-60.0, -40.0, -20.0, -5.0, 5.0, 20.0, 40.0, 60.0], "composite_kdox_vel.ppm"),
    ];
    for (product, scale, offset, values, out_name) in cases {
        let grid = sector_grid(product, scale, offset, values);
        let with = render_pass1_scene(&device, &queue, size, camera, "KDOX", Some(&grid), true);
        let without = render_pass1_scene(&device, &queue, size, camera, "KDOX", Some(&grid), false);
        write_target_ppm(&with, size, out_name);

        // Largest per-channel difference at every pixel inside the disc
        // (radar data under every one) that the vectors touched. Outside it
        // the lines draw over the background in either order.
        let disc_px = (super::view::DEFAULT_RANGE_M / m_per_px) as f32 - 2.0;
        let centre = size as f32 / 2.0;
        let mut deltas: Vec<u8> = (0..size * size)
            .filter(|i| ((i % size) as f32 - centre).hypot((i / size) as f32 - centre) < disc_px)
            .map(|i| {
                let (a, b) = (pixel(&with, size, i % size, i / size), pixel(&without, size, i % size, i / size));
                (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap()
            })
            .filter(|&d| d > 0)
            .collect();
        deltas.sort_unstable();
        let median = deltas.get(deltas.len() / 2).copied().unwrap_or(0);
        eprintln!("{product:?}: {} disc pixels changed by the map vectors, median channel delta {median}", deltas.len());
        assert!(deltas.len() > 2_000, "{product:?}: map vectors hidden by the radar ({} pixels changed)", deltas.len());
        assert!(median >= 60, "{product:?}: map vectors barely visible over the radar (median delta {median})");
    }
}
