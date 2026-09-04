//! The map underlay pass (layers 3–5, S5-W4 §7) and non-active radar site
//! markers (layer 8, S5-W5 §8). One `wgpu` pipeline, one shared vertex
//! buffer and one shared index buffer for every geometry layer (counties,
//! states/provinces, coastline, primary roads), a per-layer draw range plus
//! a small per-layer uniform buffer for colour (S5-d — see that decision
//! for why colour is a uniform, not a vertex attribute, and why each layer
//! gets its *own* uniform buffer rather than one shared buffer written
//! per-draw). Site markers share the same pipeline and shader (position
//! only, colour from the uniform) since a third file for eight lines of
//! vertex generation would be worse than one more field on this struct.
//!
//! Both site ICAO labels and projected city labels are exposed as
//! [`labels::LabelCandidate`]s through [`OverlayRenderer::label_candidates`]
//! — text itself is drawn by `render::ui`, not here (§8, §9.3: text stays
//! out of the wgpu side, same as `render::reference::ring_labels`).

use wgpu::util::DeviceExt;

use radar_workstation::compute::geometry::az_eq_project;
use radar_workstation::overlay::Projected;
use radar_workstation::sites::{self, Site};

use super::labels::LabelCandidate;
use super::reference::MARKER_ARM_KM;
use super::view::Camera;

const COUNTIES_KIND: u32 = 1;
const STATES_KIND: u32 = 2;
const COASTLINE_KIND: u32 = 3;
const ROADS_KIND: u32 = 4;

/// Colours, in draw order (S5-W4 §7). These sit *under* the radar image and
/// must not compete with it — the Instrument Principle governs the table.
/// States/provinces and coastline share a colour deliberately: FR-DR-3
/// treats them as one compositing layer (layer 4), and they read as the
/// same class of feature to an operator.
const DRAW_ORDER: [(u32, [f32; 4]); 4] = [
    (COUNTIES_KIND, [0.36, 0.38, 0.43, 0.55]),
    (STATES_KIND, [0.58, 0.62, 0.70, 0.75]),
    (COASTLINE_KIND, [0.58, 0.62, 0.70, 0.75]),
    (ROADS_KIND, [0.72, 0.55, 0.30, 0.60]),
];

/// Dimmer than the active site's marker (`reference::MARKER_COLOR`) so the
/// two read as the same symbol at different emphasis (§8).
const SITE_MARKER_COLOR: [f32; 4] = [0.75, 0.78, 0.85, 0.55];

const OVERLAY_UNIFORM_SIZE: usize = 48;

fn overlay_uniform_bytes(camera: Camera, color: [f32; 4]) -> [u8; OVERLAY_UNIFORM_SIZE] {
    let mut out = [0u8; OVERLAY_UNIFORM_SIZE];
    let view_f32s: [f32; 8] = [
        camera.center_m.0 as f32,
        camera.center_m.1 as f32,
        camera.m_per_px as f32,
        0.0,
        camera.viewport.0,
        camera.viewport.1,
        0.0,
        0.0,
    ];
    for (i, v) in view_f32s.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    for (i, v) in color.iter().enumerate() {
        out[32 + i * 4..32 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

fn vertex_bytes(vertices: &[[f32; 2]]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vertices.len() * 8);
    for v in vertices {
        bytes.extend_from_slice(&v[0].to_le_bytes());
        bytes.extend_from_slice(&v[1].to_le_bytes());
    }
    bytes
}

fn index_bytes(indices: &[u32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(indices.len() * 4);
    for i in indices {
        bytes.extend_from_slice(&i.to_le_bytes());
    }
    bytes
}

struct GeometryLayer {
    kind: u32,
    index_range: std::ops::Range<u32>,
    color: [f32; 4],
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

pub struct OverlayRenderer {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    geometry_layers: Vec<GeometryLayer>,
    site_marker_vertex_buffer: wgpu::Buffer,
    site_marker_vertex_count: u32,
    site_marker_uniform: wgpu::Buffer,
    site_marker_bind_group: wgpu::BindGroup,
    /// Site ICAO labels (rank 0) followed by projected city labels (bake
    /// rank order) — already in the priority order `labels::select`
    /// requires (§3.7, §9.1).
    label_candidates: Vec<LabelCandidate>,
}

impl OverlayRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, projected: &Projected, site: &Site) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/overlay.wgsl").into()),
        });

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay uniform"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay pipeline layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: 0,
                        shader_location: 0,
                    }],
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::LineList, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("overlay vertices"),
            contents: &vertex_bytes(&projected.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("overlay indices"),
            contents: &index_bytes(&projected.indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let geometry_layers = DRAW_ORDER
            .into_iter()
            .map(|(kind, color)| {
                // A kind absent from `projected.layers` (no data at all —
                // ADR-0029 §7's 13 road-less sites, in the limit) gets an
                // empty range: `draw` skips it, not a validation error
                // (§7).
                let index_range =
                    projected.layers.iter().find(|l| l.kind == kind).map(|l| l.index_range.clone()).unwrap_or(0..0);
                let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("overlay layer uniform"),
                    size: OVERLAY_UNIFORM_SIZE as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("overlay layer bind"),
                    layout: &bind_layout,
                    entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
                });
                GeometryLayer { kind, index_range, color, uniform, bind_group }
            })
            .collect();

        // Site markers + their ICAO label candidates (§8), projected here
        // rather than carried in `Projected` — the site table, not the
        // bundle, is the source, and it is small enough (163 sites) that a
        // second projection pass at init costs nothing worth measuring.
        let arm_m = MARKER_ARM_KM * 1000.0;
        let mut marker_positions: Vec<[f32; 2]> = Vec::new();
        let mut label_candidates: Vec<LabelCandidate> = Vec::new();
        for other in sites::all() {
            if other.id == site.id {
                continue; // reference::ReferenceRenderer already draws this one
            }
            let (x, y) = az_eq_project(site.lat, site.lon, other.lat, other.lon);
            let (x, y) = (x as f32, y as f32);
            marker_positions.extend_from_slice(&[[x - arm_m, y], [x + arm_m, y], [x, y - arm_m], [x, y + arm_m]]);
            label_candidates.push(LabelCandidate { world: [x, y], rank: 0, text: other.id });
        }
        for label in &projected.labels {
            label_candidates.push(LabelCandidate { world: label.world, rank: label.rank, text: label.name });
        }

        let site_marker_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("site marker vertices"),
            contents: &vertex_bytes(&marker_positions),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let site_marker_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("site marker uniform"),
            size: OVERLAY_UNIFORM_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let site_marker_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("site marker bind"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: site_marker_uniform.as_entire_binding() }],
        });

        Self {
            pipeline,
            vertex_buffer,
            index_buffer,
            geometry_layers,
            site_marker_vertex_buffer,
            site_marker_vertex_count: marker_positions.len() as u32,
            site_marker_uniform,
            site_marker_bind_group,
            label_candidates,
        }
    }

    /// Layers 3–5: counties, states/provinces, coastline, and (when
    /// `show_highways`) primary roads — drawn before the radar pass.
    pub fn draw(&self, queue: &wgpu::Queue, pass: &mut wgpu::RenderPass<'_>, camera: Camera, show_highways: bool) {
        pass.set_pipeline(&self.pipeline);
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        for layer in &self.geometry_layers {
            if layer.kind == ROADS_KIND && !show_highways {
                continue;
            }
            if layer.index_range.is_empty() {
                continue; // ADR-0029 §7: a no-op, not a validation error.
            }
            queue.write_buffer(&layer.uniform, 0, &overlay_uniform_bytes(camera, layer.color));
            pass.set_bind_group(0, &layer.bind_group, &[]);
            pass.draw_indexed(layer.index_range.clone(), 0, 0..1);
        }
    }

    /// Layer 8: every bundled site except the active one, drawn after the
    /// radar pass and the reference geometry (§8).
    pub fn draw_site_markers(&self, queue: &wgpu::Queue, pass: &mut wgpu::RenderPass<'_>, camera: Camera) {
        if self.site_marker_vertex_count == 0 {
            return;
        }
        queue.write_buffer(&self.site_marker_uniform, 0, &overlay_uniform_bytes(camera, SITE_MARKER_COLOR));
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.site_marker_bind_group, &[]);
        pass.set_vertex_buffer(0, self.site_marker_vertex_buffer.slice(..));
        pass.draw(0..self.site_marker_vertex_count, 0..1);
    }

    /// Site ICAO labels (rank 0) followed by city labels, in the priority
    /// order `render::labels::select` requires (§3.7).
    pub fn label_candidates(&self) -> &[LabelCandidate] {
        &self.label_candidates
    }

    /// Total GPU buffer bytes this renderer holds — the §16 measurement
    /// against ADR-0029's 11.46 MB.
    pub fn buffer_bytes(&self) -> u64 {
        self.vertex_buffer.size() + self.index_buffer.size() + self.site_marker_vertex_buffer.size()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_uniform_bytes_pack_at_the_wgsl_offsets() {
        let camera =
            Camera { center_m: (100.0, -50.0), m_per_px: 62.5, viewport: (1280.0, 800.0) };
        let color = [0.1, 0.2, 0.3, 0.4];
        let bytes = overlay_uniform_bytes(camera, color);
        assert_eq!(bytes.len(), OVERLAY_UNIFORM_SIZE);
        let f = |off: usize| f32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        assert_eq!(f(0), 100.0, "center_m.x at offset 0");
        assert_eq!(f(4), -50.0, "center_m.y at offset 4");
        assert_eq!(f(8), 62.5, "m_per_px at offset 8");
        assert_eq!(f(16), 1280.0, "viewport.x at offset 16");
        assert_eq!(f(20), 800.0, "viewport.y at offset 20");
        assert_eq!(f(32), 0.1, "color.r at offset 32");
        assert_eq!(f(36), 0.2, "color.g at offset 36");
        assert_eq!(f(40), 0.3, "color.b at offset 40");
        assert_eq!(f(44), 0.4, "color.a at offset 44");
    }

    #[test]
    fn draw_order_covers_exactly_the_four_geometry_kinds() {
        let mut kinds: Vec<u32> = DRAW_ORDER.iter().map(|(k, _)| *k).collect();
        kinds.sort_unstable();
        assert_eq!(kinds, vec![1, 2, 3, 4]);
    }
}
