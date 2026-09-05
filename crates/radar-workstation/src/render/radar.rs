//! The radar pass (S4-W3 §6): one R8Uint texture per gridded product/sweep,
//! one 256-entry LUT uniform per (product, effective scale/offset), and a
//! single full-screen-triangle draw of the *selected* grid per frame
//! (ADR-0023).
//!
//! Product and sweep switching are pure selection changes in
//! `render::ViewState` — they never touch this module's caches, which is
//! what makes FR-RP-7's "zero texture uploads across a switch" true by
//! construction. `plan_sync` is extracted as a pure function so that claim
//! is testable without a GPU (§11.1).

use std::collections::HashMap;
use std::sync::Arc;

use wgpu::util::DeviceExt;

use radar_workstation::assembly::VolumeId;
use radar_workstation::compute::palette::{compile_lut, ColorLut, Palette};
use radar_workstation::compute::{DisplayProduct, SweepGrid};
use radar_workstation::event::Event;
use radar_workstation::state::history::{self, Frame};
use radar_workstation::state::StateSnapshot;

/// A cache key: which frame (by volume), which product, and — for a base
/// product — which elevation. Derived products (Echo Tops / VIL) are one
/// per volume and key as `None` (ADR-0030 §3.7): keying by volume, not just
/// product/elevation, is what lets one `(product, elevation)` selection be
/// resident from *every* retained frame at once, which is what playback
/// reads.
pub type GridKey = (VolumeId, DisplayProduct, Option<u8>);

/// The key a grid lives under, within `volume`'s frame. Shares its
/// base-vs-derived rule with [`Frame::grid`]'s lookup
/// ([`history::key_elevation`]) rather than matching it a second time.
pub fn grid_key(volume: VolumeId, grid: &SweepGrid) -> GridKey {
    (volume, grid.product, history::key_elevation(grid.product, grid.elevation_number))
}

/// GPU budget for the *history tail* — every retained frame besides the
/// newest, at one selected grid apiece. 128 MB target (`rendering.md`),
/// less the ADR-0029 overlay's 11.46 MB, less the newest frame's own grids
/// (~40 MB at the measured worst case, always resident in full — see
/// [`residency`]) leaves roughly 76 MB for the tail: about 60 frames of one
/// super-resolution reflectivity grid (~1.26 MB), so the GPU is not what
/// bounds the loop (ADR-0030 §3.7) — `history.budget_mb` is.
const HISTORY_GPU_BUDGET_BYTES: usize = 76 * 1024 * 1024;

/// What must be GPU-resident this frame (ADR-0030 §3.7): every grid of the
/// newest frame — so a product or elevation switch still uploads nothing
/// for the *displayed* frame (FR-RP-7) — plus the selected
/// `(product, elevation_number)` grid of every other retained frame, so
/// playback of that selection runs upload-free once resident. Oldest
/// frames drop out first when `budget_bytes` binds; the newest frame is
/// never dropped. Pure: no device, no queue, no `ViewState`.
///
/// This assumes the newest retained frame is the one being displayed —
/// true today by construction, since there is no pinning (Part B has no
/// timeline). Stage 6a Part C must revisit this rule when a `Pinned`
/// selection exists: the frame that must stay fully resident is then the
/// *pinned* one, not necessarily the newest.
pub fn residency(
    frames: &[Arc<Frame>],
    product: DisplayProduct,
    elevation_number: u8,
    budget_bytes: usize,
) -> Vec<(GridKey, Arc<SweepGrid>)> {
    let Some((newest, older)) = frames.split_last() else {
        return Vec::new();
    };

    let mut resident = Vec::new();
    for sweep in newest.sweeps.values() {
        for grid in &sweep.grids {
            resident.push((grid_key(newest.volume, grid), Arc::clone(grid)));
        }
    }
    for grid in newest.derived.values() {
        resident.push((grid_key(newest.volume, grid), Arc::clone(grid)));
    }

    // `older` is oldest → newest (as `StateSnapshot::frames` always is), so
    // dropping from the front below is "oldest dropped first" by
    // construction — not a separate sort.
    let elevation_key = history::key_elevation(product, elevation_number);
    let mut tail: Vec<(GridKey, Arc<SweepGrid>)> = older
        .iter()
        .filter_map(|frame| {
            frame.grid(product, elevation_key).map(|grid| (grid_key(frame.volume, grid), Arc::clone(grid)))
        })
        .collect();

    let mut tail_bytes: usize = tail.iter().map(|(_, g)| g.byte_len()).sum();
    while tail_bytes > budget_bytes && !tail.is_empty() {
        let (_, dropped) = tail.remove(0);
        tail_bytes -= dropped.byte_len();
    }

    resident.extend(tail);
    resident
}

/// Uploads are rate-limited to this many per frame (ADR-0030 §3.7):
/// switching product with a full history tail resident would otherwise
/// upload the whole tail in one frame (~15 MB at the proposed default
/// retention). At 4 grids (~5 MB) the tail fills over a few frames instead
/// of stalling one — well under a 16.6 ms budget.
const MAX_UPLOADS_PER_FRAME: usize = 4;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncPlan {
    pub to_upload: Vec<GridKey>,
    pub to_evict: Vec<GridKey>,
    /// `true` when more of `residency`'s set still needs uploading beyond
    /// `max_uploads` — the caller should request an immediate redraw rather
    /// than wait for the idle tick, so the tail fills promptly.
    pub more_pending: bool,
}

/// Decide which grids to (re)upload and which cache entries to drop, given
/// what is cached and what `residency` says should be resident. A grid is
/// re-uploaded only when it is new or its `Arc` identity changed — holding
/// the `Arc` in the cache is what makes `Arc::ptr_eq` a sound identity test
/// (§6.2). `to_upload` is ordered newest-frame-first (the newest volume
/// among `entries`) so the displayed frame is never the one that waits;
/// evictions are never rate-limited — freeing is free, and a bounded cache
/// must be allowed to shrink promptly. Pure: no device, no queue.
pub fn plan_sync(
    cached: &HashMap<GridKey, Arc<SweepGrid>>,
    entries: &[(GridKey, Arc<SweepGrid>)],
    max_uploads: usize,
) -> SyncPlan {
    let newest_volume = entries.iter().map(|(key, _)| key.0).max();

    let mut needs_upload: Vec<GridKey> = entries
        .iter()
        .filter(|(key, grid)| !matches!(cached.get(key), Some(existing) if Arc::ptr_eq(existing, grid)))
        .map(|(key, _)| *key)
        .collect();
    needs_upload.sort_by_key(|key| (Some(key.0) != newest_volume, *key));

    let more_pending = needs_upload.len() > max_uploads;
    needs_upload.truncate(max_uploads);

    let mut to_evict: Vec<GridKey> =
        cached.keys().filter(|key| !entries.iter().any(|(ek, _)| ek == *key)).copied().collect();
    to_evict.sort();

    SyncPlan { to_upload: needs_upload, to_evict, more_pending }
}

// --- sRGB (§6.4): convert the LUT once on the CPU, never per pixel ---

fn srgb_to_linear(c: u8) -> f32 {
    let s = c as f32 / 255.0;
    if s <= 0.040_45 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// A `ColorLut` (sRGB bytes) as the `array<vec4<f32>, 256>` the shader
/// binds. RGB is converted sRGB→linear when the surface format is
/// `*_UNORM_SRGB` (the hardware re-encodes on write); alpha stays linear.
pub fn lut_uniform_bytes(lut: &ColorLut, surface_is_srgb: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(256 * 16);
    for entry in lut {
        let rgb = |c: u8| if surface_is_srgb { srgb_to_linear(c) } else { c as f32 / 255.0 };
        for channel in &entry[..3] {
            out.extend_from_slice(&rgb(*channel).to_le_bytes());
        }
        out.extend_from_slice(&(entry[3] as f32 / 255.0).to_le_bytes());
    }
    out
}

// --- shader uniform (std140-ish, 16-byte aligned, 64 bytes) ---

/// Serialised `View` uniform matching `shaders/radar.wgsl` field-for-field.
/// Written by hand rather than transmuted (`bytemuck` is not a dependency,
/// and `render/` carries no `unsafe` — NFR-SEC-5, BC-9).
const VIEW_UNIFORM_SIZE: usize = 64;

fn view_uniform_bytes(
    grid: &SweepGrid,
    center_m: (f64, f64),
    m_per_px: f64,
    viewport: (f32, f32),
) -> [u8; VIEW_UNIFORM_SIZE] {
    let is_ground = matches!(grid.product, DisplayProduct::EchoTops | DisplayProduct::Vil);
    let f32s: [f32; 12] = [
        center_m.0 as f32,
        center_m.1 as f32,
        m_per_px as f32,
        0.0, // _pad0
        viewport.0,
        viewport.1,
        0.0, // _pad1.x
        0.0, // _pad1.y
        f32::from_bits(grid.azimuth_count as u32),
        f32::from_bits(grid.gate_count as u32),
        grid.first_gate_m as f32,
        grid.gate_width_m as f32,
    ];
    let tail_f32s: [f32; 4] =
        [grid.elevation_deg.to_radians(), f32::from_bits(is_ground as u32), 0.0, 0.0];

    let mut out = [0u8; VIEW_UNIFORM_SIZE];
    for (i, v) in f32s.iter().chain(tail_f32s.iter()).enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

// --- caches ---

struct CachedGrid {
    source: Arc<SweepGrid>,
    texture_view: wgpu::TextureView,
}

/// Cap on distinct LUTs held (§6.3). Velocity's effective scale/offset vary
/// per sweep, so this is not "one per product" — but an unbounded cache
/// keyed on float bits in a multi-hour process is a leak with a friendly
/// name (the pattern `RetainedGridSetBounded` names for the event log).
const LUT_CACHE_CAP: usize = 32;

struct CachedLut {
    buffer: wgpu::Buffer,
}

pub struct RadarRenderer {
    pipeline: wgpu::RenderPipeline,
    grid_bind_layout: wgpu::BindGroupLayout,
    uniform_bind_layout: wgpu::BindGroupLayout,
    view_buffer: wgpu::Buffer,
    grids: HashMap<GridKey, CachedGrid>,
    luts: HashMap<(DisplayProduct, u32, u32), CachedLut>,
    surface_is_srgb: bool,
    max_texture_dimension: u32,
}

impl RadarRenderer {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat, surface_is_srgb: bool, max_texture_dimension: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("radar shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/radar.wgsl").into()),
        });

        let uniform_bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("radar uniforms"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let grid_bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("radar grid texture"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Uint,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("radar pipeline layout"),
            bind_group_layouts: &[Some(&uniform_bind_layout), Some(&grid_bind_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("radar pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let view_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("radar view uniform"),
            size: VIEW_UNIFORM_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            grid_bind_layout,
            uniform_bind_layout,
            view_buffer,
            grids: HashMap::new(),
            luts: HashMap::new(),
            surface_is_srgb,
            max_texture_dimension,
        }
    }

    /// Upload textures for every grid `residency` says should be resident
    /// this frame for `(product, elevation_number)`, evict what it dropped,
    /// and return any events (a grid too large for the device, the LUT
    /// cache bounded) plus whether the upload budget still has work left
    /// (§3.7) — the caller requests an immediate redraw when it does, so
    /// the tail fills over a few frames rather than stalling one. Runs only
    /// when `(revision, product, elevation_number)` changed — the caller
    /// checks.
    pub fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        snapshot: &StateSnapshot,
        product: DisplayProduct,
        elevation_number: u8,
        palettes: &std::collections::BTreeMap<DisplayProduct, Palette>,
    ) -> (Vec<Event>, bool) {
        let mut events = Vec::new();
        let entries = residency(&snapshot.frames, product, elevation_number, HISTORY_GPU_BUDGET_BYTES);
        let cached: HashMap<GridKey, Arc<SweepGrid>> =
            self.grids.iter().map(|(k, v)| (*k, Arc::clone(&v.source))).collect();
        let plan = plan_sync(&cached, &entries, MAX_UPLOADS_PER_FRAME);

        for key in plan.to_evict {
            self.grids.remove(&key);
        }

        for key in plan.to_upload {
            let Some((_, grid)) = entries.iter().find(|(k, _)| *k == key) else { continue };
            if grid.gate_count as u32 > self.max_texture_dimension
                || grid.azimuth_count as u32 > self.max_texture_dimension
            {
                events.push(Event::DegenerateGateGeometry {
                    product: grid.product,
                    elevation_number: grid.elevation_number,
                });
                continue;
            }
            // Make sure this grid's LUT exists too, evicting if capped —
            // before the upload, which moves `grid` (the fix for §2.2: the
            // cache now stores the `Arc` it was handed, not a fresh deep
            // copy under a fresh `Arc`, so `Arc::ptr_eq` above is a sound
            // identity test on the *next* call).
            if let Some(palette) = palettes.get(&grid.product) {
                if let Some(event) = self.ensure_lut(device, grid, palette) {
                    events.push(event);
                }
            }
            let cached_grid = upload_grid(device, queue, Arc::clone(grid));
            self.grids.insert(key, cached_grid);
        }

        (events, plan.more_pending)
    }

    fn ensure_lut(&mut self, device: &wgpu::Device, grid: &SweepGrid, palette: &Palette) -> Option<Event> {
        let key = (grid.product, grid.scale.to_bits(), grid.offset.to_bits());
        if self.luts.contains_key(&key) {
            return None;
        }
        let mut event = None;
        if self.luts.len() >= LUT_CACHE_CAP {
            self.luts.clear();
            event = Some(Event::RetainedGridSetBounded { dropped_elevation_number: 0 });
        }
        let lut = compile_lut(palette, grid.scale, grid.offset);
        let bytes = lut_uniform_bytes(&lut, self.surface_is_srgb);
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("radar LUT"),
            contents: &bytes,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        self.luts.insert(key, CachedLut { buffer });
        event
    }

    /// Record the radar draw into `pass`. `grid` is the selected
    /// product/elevation's grid, already synced; `volume` identifies which
    /// frame it came from (ADR-0030 — the cache key now includes it). No-op
    /// if the grid or its LUT is missing (the caller shows "no data on this
    /// cut" in the status bar).
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
        grid: &SweepGrid,
        volume: VolumeId,
        camera: super::view::Camera,
    ) {
        let key = grid_key(volume, grid);
        let Some(cached) = self.grids.get(&key) else { return };
        let lut_key = (grid.product, grid.scale.to_bits(), grid.offset.to_bits());
        let Some(lut) = self.luts.get(&lut_key) else { return };

        let uniform = view_uniform_bytes(grid, camera.center_m, camera.m_per_px, camera.viewport);
        queue.write_buffer(&self.view_buffer, 0, &uniform);

        let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("radar uniform bind"),
            layout: &self.uniform_bind_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.view_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: lut.buffer.as_entire_binding() },
            ],
        });
        let grid_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("radar grid bind"),
            layout: &self.grid_bind_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&cached.texture_view) }],
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &uniform_bind, &[]);
        pass.set_bind_group(1, &grid_bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
impl RadarRenderer {
    /// Exposes exactly the plan `sync` would compute against the *current*
    /// cache, without mutating anything — lets a test call `sync` once,
    /// then assert a second, hypothetical call would upload nothing. This
    /// is the only way to observe the §2.2 regression from outside the
    /// module: a pure test can't reach it, because it's a property of what
    /// `sync` already put in the cache, not of `plan_sync` in isolation.
    fn plan_for_test(&self, snapshot: &StateSnapshot, product: DisplayProduct, elevation_number: u8) -> SyncPlan {
        let entries = residency(&snapshot.frames, product, elevation_number, HISTORY_GPU_BUDGET_BYTES);
        let cached: HashMap<GridKey, Arc<SweepGrid>> =
            self.grids.iter().map(|(k, v)| (*k, Arc::clone(&v.source))).collect();
        plan_sync(&cached, &entries, usize::MAX)
    }
}

/// Uploads `grid` and stores the very `Arc` the caller handed in — not a
/// deep copy under a fresh `Arc` (the §2.2 defect: `plan_sync`'s
/// `Arc::ptr_eq` identity test can only ever be sound if the cache holds
/// the same allocation `residency` produced, so a later call sees "already
/// uploaded" instead of re-uploading everything on every revision).
fn upload_grid(device: &wgpu::Device, queue: &wgpu::Queue, grid: Arc<SweepGrid>) -> CachedGrid {
    let size = wgpu::Extent3d {
        width: grid.gate_count as u32,
        height: grid.azimuth_count as u32,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("radar grid"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Uint,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    // write_texture stages internally and does not impose the 256-byte row
    // alignment copy_buffer_to_texture would (§6.1) — tightly packed rows.
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &grid.cells,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(grid.gate_count as u32),
            rows_per_image: Some(grid.azimuth_count as u32),
        },
        size,
    );
    CachedGrid { source: grid, texture_view: texture.create_view(&wgpu::TextureViewDescriptor::default()) }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use super::super::view::Camera;
    use radar_workstation::compute::SweepGrid;

    fn grid(product: DisplayProduct, elevation_number: u8) -> Arc<SweepGrid> {
        Arc::new(SweepGrid {
            product,
            azimuth_count: 4,
            gate_count: 4,
            first_gate_m: 0,
            gate_width_m: 250,
            elevation_number,
            elevation_deg: elevation_number as f32 * 0.5,
            nyquist_velocity_mps: Some(8.0),
            scale: 2.0,
            offset: 66.0,
            cells: vec![0u8; 16],
            filled_azimuths: 0,
        })
    }

    fn volume(scan_time_ms: u32) -> VolumeId {
        VolumeId { julian_date: 20_000, scan_time_ms }
    }

    /// A retained frame holding one sweep — the unit `residency` operates
    /// on. Built through the same `Frame`/`DisplaySweep` types the library
    /// uses, not a hand-rolled stand-in.
    fn frame_with_sweep(scan_time_ms: u32, vcp_number: u16, elevation_number: u8, grids: Vec<Arc<SweepGrid>>) -> Arc<Frame> {
        let v = volume(scan_time_ms);
        let mut frame = Frame::new(v, vcp_number, std::time::Instant::now());
        frame.insert_sweep(radar_workstation::state::DisplaySweep {
            elevation_number,
            elevation_deg: elevation_number as f32 * 0.5,
            volume: v,
            vcp_number,
            received: std::time::Instant::now(),
            grids,
        });
        Arc::new(frame)
    }

    #[test]
    fn derived_products_key_without_an_elevation() {
        let v = volume(100);
        assert_eq!(grid_key(v, &grid(DisplayProduct::EchoTops, 0)), (v, DisplayProduct::EchoTops, None));
        assert_eq!(grid_key(v, &grid(DisplayProduct::Reflectivity, 3)), (v, DisplayProduct::Reflectivity, Some(3)));
    }

    #[test]
    fn nothing_to_do_when_the_cache_already_holds_the_same_arcs() {
        let v = volume(100);
        let r1 = grid(DisplayProduct::Reflectivity, 1);
        let v1 = grid(DisplayProduct::Velocity, 1);
        let cached: HashMap<GridKey, Arc<SweepGrid>> =
            [(grid_key(v, &r1), Arc::clone(&r1)), (grid_key(v, &v1), Arc::clone(&v1))].into_iter().collect();
        let entries = vec![(grid_key(v, &r1), r1), (grid_key(v, &v1), v1)];
        let plan = plan_sync(&cached, &entries, usize::MAX);
        assert!(plan.to_upload.is_empty());
        assert!(plan.to_evict.is_empty());
        assert!(!plan.more_pending);
    }

    #[test]
    fn a_product_switch_and_a_sweep_switch_upload_nothing() {
        // FR-RP-7: the snapshot's grid set is identical before and after a
        // selection change — only ViewState moves — so plan_sync is empty.
        let v = volume(100);
        let grids = [
            grid(DisplayProduct::Reflectivity, 1),
            grid(DisplayProduct::Reflectivity, 2),
            grid(DisplayProduct::Velocity, 1),
        ];
        let cached: HashMap<GridKey, Arc<SweepGrid>> =
            grids.iter().map(|g| (grid_key(v, g), Arc::clone(g))).collect();
        let entries: Vec<_> = grids.iter().map(|g| (grid_key(v, g), Arc::clone(g))).collect();
        let plan = plan_sync(&cached, &entries, usize::MAX);
        assert!(plan.to_upload.is_empty(), "a switch must upload nothing: {plan:?}");
        assert!(plan.to_evict.is_empty());
    }

    #[test]
    fn a_replaced_arc_is_re_uploaded_and_a_dropped_key_is_evicted() {
        let v = volume(100);
        let old_r = grid(DisplayProduct::Reflectivity, 1);
        let stale_v = grid(DisplayProduct::Velocity, 1);
        let cached: HashMap<GridKey, Arc<SweepGrid>> =
            [(grid_key(v, &old_r), old_r), (grid_key(v, &stale_v), stale_v)].into_iter().collect();

        let new_r = grid(DisplayProduct::Reflectivity, 1); // same key, different Arc
        let entries = vec![(grid_key(v, &new_r), new_r)];
        let plan = plan_sync(&cached, &entries, usize::MAX);
        assert_eq!(plan.to_upload, vec![(v, DisplayProduct::Reflectivity, Some(1))]);
        assert_eq!(plan.to_evict, vec![(v, DisplayProduct::Velocity, Some(1))]);
    }

    #[test]
    fn residency_holds_every_grid_of_the_newest_frame() {
        let newest =
            frame_with_sweep(200, 35, 1, vec![grid(DisplayProduct::Reflectivity, 1), grid(DisplayProduct::Velocity, 1)]);
        let entries = residency(&[newest], DisplayProduct::Reflectivity, 1, usize::MAX);
        assert_eq!(entries.len(), 2, "every grid of the newest frame must be resident, not just the selection");
    }

    #[test]
    fn residency_holds_only_the_selected_grid_of_older_frames() {
        let older =
            frame_with_sweep(100, 35, 1, vec![grid(DisplayProduct::Reflectivity, 1), grid(DisplayProduct::Velocity, 1)]);
        let newest = frame_with_sweep(200, 35, 1, vec![grid(DisplayProduct::Reflectivity, 1)]);
        let entries = residency(&[older, newest], DisplayProduct::Reflectivity, 1, usize::MAX);
        assert_eq!(entries.len(), 2, "newest frame's one grid + the older frame's selected grid");
        assert!(entries.iter().all(|(k, _)| k.1 == DisplayProduct::Reflectivity));
    }

    #[test]
    fn residency_drops_the_oldest_frames_first_under_the_budget() {
        let oldest = frame_with_sweep(100, 35, 1, vec![grid(DisplayProduct::Reflectivity, 1)]); // 16 bytes
        let middle = frame_with_sweep(200, 35, 1, vec![grid(DisplayProduct::Reflectivity, 1)]); // 16 bytes
        let newest = frame_with_sweep(300, 35, 1, vec![grid(DisplayProduct::Reflectivity, 1)]); // always resident
        // Budget for the tail fits exactly one 16-byte grid.
        let entries = residency(&[oldest, middle, newest], DisplayProduct::Reflectivity, 1, 16);
        let volumes: Vec<VolumeId> = entries.iter().map(|(k, _)| k.0).collect();
        assert!(volumes.contains(&volume(300)), "the newest frame must always be present");
        assert!(!volumes.contains(&volume(100)), "the oldest retained frame must drop first under budget");
        assert!(volumes.contains(&volume(200)), "the frame nearer the newest should survive when only one tail slot fits");
    }

    #[test]
    fn residency_never_drops_the_newest_frame() {
        let older = frame_with_sweep(100, 35, 1, vec![grid(DisplayProduct::Reflectivity, 1)]);
        let newest =
            frame_with_sweep(200, 35, 1, vec![grid(DisplayProduct::Reflectivity, 1), grid(DisplayProduct::Velocity, 1)]);
        let entries = residency(&[older, newest], DisplayProduct::Reflectivity, 1, 0);
        assert_eq!(entries.len(), 2, "the newest frame's grids must stay resident even at a zero tail budget");
        assert!(entries.iter().all(|(k, _)| k.0 == volume(200)));
    }

    #[test]
    fn residency_of_a_derived_product_selects_it_from_every_frame() {
        let mut older = Frame::new(volume(100), 35, std::time::Instant::now());
        older.set_derived(vec![grid(DisplayProduct::Vil, 0)]);
        let mut newest = Frame::new(volume(200), 35, std::time::Instant::now());
        newest.set_derived(vec![grid(DisplayProduct::Vil, 0)]);
        let entries = residency(&[Arc::new(older), Arc::new(newest)], DisplayProduct::Vil, 0, usize::MAX);
        assert_eq!(entries.len(), 2, "a derived-product selection must form a loop across every retained frame");
        assert!(entries.iter().all(|(k, _)| k.1 == DisplayProduct::Vil && k.2.is_none()));
    }

    #[test]
    fn a_selection_absent_from_an_older_frame_is_simply_absent() {
        let older = frame_with_sweep(100, 35, 1, vec![grid(DisplayProduct::Velocity, 1)]); // no reflectivity
        let newest = frame_with_sweep(200, 35, 1, vec![grid(DisplayProduct::Reflectivity, 1)]);
        let entries = residency(&[older, newest], DisplayProduct::Reflectivity, 1, usize::MAX);
        assert_eq!(entries.len(), 1, "the older frame has no matching grid: simply absent, no placeholder, no panic");
    }

    #[test]
    fn walking_the_selection_across_every_frame_uploads_nothing_once_resident() {
        // §1 acceptance criterion 6: what Part C's playback will do.
        let frames: Vec<Arc<Frame>> =
            (0..5).map(|i| frame_with_sweep(100 + i * 100, 35, 1, vec![grid(DisplayProduct::Reflectivity, 1)])).collect();
        let entries = residency(&frames, DisplayProduct::Reflectivity, 1, usize::MAX);
        assert_eq!(entries.len(), frames.len(), "every frame's selected grid must be resident under an unbounded budget");

        let mut cached: HashMap<GridKey, Arc<SweepGrid>> = HashMap::new();
        let plan = plan_sync(&cached, &entries, usize::MAX);
        for key in &plan.to_upload {
            let grid = entries.iter().find(|(k, _)| k == key).unwrap().1.clone();
            cached.insert(*key, grid);
        }

        // Playback walks the *selection* across frames — it never changes
        // what `residency` says should be resident, so recomputing the plan
        // against the very same entries (what every later frame of
        // playback does) must upload nothing.
        let plan = plan_sync(&cached, &entries, usize::MAX);
        assert!(plan.to_upload.is_empty(), "playback must be upload-free once the tail is resident: {plan:?}");
    }

    #[test]
    fn a_product_switch_uploads_the_tail_at_the_rate_limit_and_reports_more_pending() {
        let frames: Vec<Arc<Frame>> =
            (0..6).map(|i| frame_with_sweep(100 + i * 100, 35, 1, vec![grid(DisplayProduct::Reflectivity, 1)])).collect();
        let entries = residency(&frames, DisplayProduct::Reflectivity, 1, usize::MAX);
        let cached: HashMap<GridKey, Arc<SweepGrid>> = HashMap::new(); // nothing cached yet, as if just switched product
        let plan = plan_sync(&cached, &entries, 4);
        assert_eq!(plan.to_upload.len(), 4, "the rate limit must cap uploads in one call");
        assert!(plan.more_pending, "more of the tail still needs uploading");
    }

    #[test]
    fn the_newest_frames_grids_are_uploaded_before_the_tail() {
        let older: Vec<Arc<Frame>> =
            (0..5).map(|i| frame_with_sweep(100 + i * 100, 35, 1, vec![grid(DisplayProduct::Reflectivity, 1)])).collect();
        let newest =
            frame_with_sweep(1000, 35, 1, vec![grid(DisplayProduct::Reflectivity, 1), grid(DisplayProduct::Velocity, 1)]);
        let mut frames = older;
        frames.push(Arc::clone(&newest));
        let entries = residency(&frames, DisplayProduct::Reflectivity, 1, usize::MAX);
        let cached: HashMap<GridKey, Arc<SweepGrid>> = HashMap::new();
        let plan = plan_sync(&cached, &entries, 2);
        assert!(
            plan.to_upload.iter().all(|k| k.0 == newest.volume),
            "the newest frame's grids must upload before any of the tail: {plan:?}"
        );
    }

    #[test]
    fn lut_uniform_bytes_are_1024_floats_and_linearise_when_srgb() {
        let mut lut: ColorLut = [[0u8; 4]; 256];
        lut[10] = [255, 128, 0, 255];
        let linear = lut_uniform_bytes(&lut, true);
        assert_eq!(linear.len(), 256 * 16);
        // entry 10, R channel: srgb 255 -> linear 1.0
        let r = f32::from_le_bytes(linear[10 * 16..10 * 16 + 4].try_into().unwrap());
        assert!((r - 1.0).abs() < 1e-4, "srgb 255 -> linear 1.0, got {r}");
        // G channel: srgb 128/255 ~ 0.502 -> linear ~0.216
        let g = f32::from_le_bytes(linear[10 * 16 + 4..10 * 16 + 8].try_into().unwrap());
        assert!((g - 0.2158).abs() < 1e-3, "srgb 0.502 -> linear ~0.216, got {g}");
        // alpha is always linear
        let a = f32::from_le_bytes(linear[10 * 16 + 12..10 * 16 + 16].try_into().unwrap());
        assert!((a - 1.0).abs() < 1e-6);
    }

    #[test]
    fn lut_uniform_bytes_pass_through_when_not_srgb() {
        let mut lut: ColorLut = [[0u8; 4]; 256];
        lut[5] = [128, 128, 128, 200];
        let raw = lut_uniform_bytes(&lut, false);
        let r = f32::from_le_bytes(raw[5 * 16..5 * 16 + 4].try_into().unwrap());
        assert!((r - 128.0 / 255.0).abs() < 1e-6, "no sRGB conversion, got {r}");
    }

    #[test]
    fn view_uniform_bytes_pack_geometry_at_the_wgsl_offsets() {
        let g = grid(DisplayProduct::Reflectivity, 2);
        let bytes = view_uniform_bytes(&g, (100.0, -50.0), 62.5, (1280.0, 800.0));
        assert_eq!(bytes.len(), VIEW_UNIFORM_SIZE);
        let f = |off: usize| f32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        let u = |off: usize| u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        assert_eq!(f(0), 100.0, "center_m.x at offset 0");
        assert_eq!(f(8), 62.5, "m_per_px at offset 8");
        assert_eq!(f(16), 1280.0, "viewport.x at offset 16");
        assert_eq!(u(32), 4, "azimuth_count at offset 32");
        assert_eq!(u(36), 4, "gate_count at offset 36");
        assert_eq!(u(52), 0, "is_ground_range 0 for a base product at offset 52");

        let derived = grid(DisplayProduct::EchoTops, 0);
        let db = view_uniform_bytes(&derived, (0.0, 0.0), 1.0, (1.0, 1.0));
        assert_eq!(u32::from_le_bytes(db[52..56].try_into().unwrap()), 1, "is_ground_range 1 for a derived product");
    }

    // --- GPU offscreen tests (§11.2). `#[ignore]`d: GitHub Actions has no
    // GPU. Run manually with:
    //   cargo test -p radar-workstation --bins -- --ignored --nocapture
    // The full pixel-for-pixel comparison against `utility/radar-viz`'s
    // `render_grid_ppi` (§11.2 test 2) is NOT implemented here: `render` is a
    // binary-side module (S4-f) and `radar-viz` — which owns that CPU
    // reference renderer — cannot reach it. See stage-4-first-pixels.md §16.

    fn try_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster_block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            ..Default::default()
        }))
        .ok()?;
        pollster_block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("radar test device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        }))
        .ok()
    }

    /// A tiny blocking executor so the test needs no async runtime — and no
    /// `unsafe` (the `std::task::Wake` trait and `Box::pin` are the safe
    /// path; `render/` carries no `unsafe`, test code included).
    fn pollster_block_on<F: std::future::Future>(fut: F) -> F::Output {
        use std::sync::Arc;
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

    #[test]
    #[ignore = "requires a GPU adapter"]
    fn offscreen_radar_pass_draws_data_and_leaves_no_data_transparent() {
        let Some((device, queue)) = try_device() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut renderer = RadarRenderer::new(&device, format, false, 2048);

        // A grid that is all "real data" (raw 200) so the whole disc paints.
        let mut g = (*grid(DisplayProduct::Reflectivity, 1)).clone();
        g.cells = vec![200u8; g.cells.len()];
        let g = Arc::new(g);

        let (palettes, _) = radar_workstation::compute::palette::load_all();
        let snapshot = fake_snapshot(vec![g.clone()]);
        renderer.sync(&device, &queue, &snapshot, DisplayProduct::Reflectivity, 1, &palettes);

        // The direct regression for §2.2: syncing the very same snapshot a
        // second time must find everything already cached under the same
        // `Arc` identity, and therefore plan zero uploads — no pure test can
        // catch this, since it is a property of what `sync` actually put in
        // the cache the first time.
        let second_plan = renderer.plan_for_test(&snapshot, DisplayProduct::Reflectivity, 1);
        assert!(second_plan.to_upload.is_empty(), "a synced grid must never be re-uploaded: {second_plan:?}");

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
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("test pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let camera = Camera { center_m: (0.0, 0.0), m_per_px: 20.0, viewport: (size as f32, size as f32) };
            renderer.draw(&device, &queue, &mut pass, &g, volume(0), camera);
        }
        let readback = read_texture(&device, &queue, encoder, &target, size);

        // Centre pixel is inside the disc and carries data -> not background.
        let centre = pixel(&readback, size, size / 2, size / 2);
        assert_ne!(centre, [0, 0, 0, 255], "centre pixel should carry radar data, not background");
    }

    /// A `StateSnapshot` carrying one frame — the shape every consumer sees
    /// post-ADR-0030 — built from the same `Frame`/`DisplaySweep` types the
    /// library uses, so this stays a single definition of "one frame with
    /// these grids" rather than a second hand-rolled one.
    fn fake_snapshot(grids: Vec<Arc<SweepGrid>>) -> radar_workstation::state::StateSnapshot {
        let frame = frame_with_sweep(0, 35, 1, grids);
        radar_workstation::state::StateSnapshot {
            site: radar_workstation::sites::by_id("KDOX").unwrap(),
            sweeps: frame.sweeps.values().cloned().collect(),
            derived: vec![],
            last_complete: None,
            frames: vec![frame],
            revision: 1,
            ingest: radar_workstation::ingest::s3_poll::IngestStatus::default(),
        }
    }

    fn read_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mut encoder: wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        size: u32,
    ) -> Vec<u8> {
        let bytes_per_row = (size * 4).next_multiple_of(256);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (bytes_per_row * size) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(size),
                },
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
}
