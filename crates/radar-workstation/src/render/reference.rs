//! Reference geometry (S4-W5 §8): range rings, azimuth spokes, and a site
//! marker, drawn as a `LineList` in world (metre) coordinates through the
//! same kind of view uniform the radar pass uses. Built once at startup;
//! pan and zoom are the uniform. Instrument Principle — this exists to let
//! the operator read the radar image, not to be looked at, so it is 1 px
//! and quiet.

/// Range rings drawn every 50 km out to this range.
const RING_STEP_KM: f32 = 50.0;
const RING_MAX_KM: f32 = 300.0;
/// The conventional Level II display range — emphasised, and the number a
/// reader can check the projection scale against by eye.
pub const EMPHASIS_RING_KM: f32 = 230.0;
const SPOKE_STEP_DEG: f32 = 30.0;
const SPOKE_INNER_KM: f32 = 50.0;
/// World km for a site-marker cross's arm. `pub(super)` so `render::overlay`
/// draws non-active site markers as the same symbol at a different emphasis
/// (§8) without a second copy of the constant (DRY).
pub(super) const MARKER_ARM_KM: f32 = 4.0;
const RING_SEGMENTS: usize = 240;

const RING_COLOR: [f32; 4] = [0.52, 0.54, 0.60, 0.45];
const EMPHASIS_COLOR: [f32; 4] = [0.72, 0.78, 0.85, 0.75];
const SPOKE_COLOR: [f32; 4] = [0.45, 0.47, 0.52, 0.35];
const MARKER_COLOR: [f32; 4] = [0.95, 0.95, 1.0, 0.95];

const REF_UNIFORM_SIZE: usize = 32;

fn polar(range_m: f32, az_deg: f32) -> [f32; 2] {
    let (s, c) = az_deg.to_radians().sin_cos();
    // azimuth 0 = north (+y), increasing clockwise — the same convention as
    // the radar shader and compute::grid.
    [range_m * s, range_m * c]
}

/// Every ring this layer draws: the 50 km steps out to `RING_MAX_KM`, plus
/// the emphasised 230 km ring (which is not a multiple of the step). Each
/// paired with whether it is the emphasis ring.
fn ring_radii_km() -> Vec<(f32, bool)> {
    let mut rings: Vec<(f32, bool)> = Vec::new();
    let mut km = RING_STEP_KM;
    while km <= RING_MAX_KM + 0.1 {
        rings.push((km, false));
        km += RING_STEP_KM;
    }
    rings.push((EMPHASIS_RING_KM, true));
    rings.sort_by(|a, b| a.0.total_cmp(&b.0));
    rings
}

/// `(world_position, label)` for each ring, for egui to draw (§8: text stays
/// out of the wgpu side). Placed on the 45° bearing so they never sit on a
/// spoke.
pub fn ring_labels() -> Vec<([f32; 2], String)> {
    ring_radii_km()
        .into_iter()
        .map(|(km, emphasis)| {
            let label = if emphasis { format!("{km:.0} km") } else { format!("{km:.0}") };
            (polar(km * 1000.0, 45.0), label)
        })
        .collect()
}

fn ring_vertices(out: &mut Vec<Vertex>, range_m: f32, color: [f32; 4]) {
    let mut prev = polar(range_m, 0.0);
    for i in 1..=RING_SEGMENTS {
        let az = i as f32 / RING_SEGMENTS as f32 * 360.0;
        let next = polar(range_m, az);
        out.push(Vertex { pos: prev, color });
        out.push(Vertex { pos: next, color });
        prev = next;
    }
}

#[derive(Clone, Copy)]
struct Vertex {
    pos: [f32; 2],
    color: [f32; 4],
}

fn build_vertices() -> Vec<Vertex> {
    let mut v = Vec::new();

    for (km, emphasis) in ring_radii_km() {
        let color = if emphasis { EMPHASIS_COLOR } else { RING_COLOR };
        ring_vertices(&mut v, km * 1000.0, color);
    }

    let mut az = 0.0;
    while az < 360.0 - 0.1 {
        v.push(Vertex { pos: polar(SPOKE_INNER_KM * 1000.0, az), color: SPOKE_COLOR });
        v.push(Vertex { pos: polar(RING_MAX_KM * 1000.0, az), color: SPOKE_COLOR });
        az += SPOKE_STEP_DEG;
    }

    let arm = MARKER_ARM_KM * 1000.0;
    v.push(Vertex { pos: [-arm, 0.0], color: MARKER_COLOR });
    v.push(Vertex { pos: [arm, 0.0], color: MARKER_COLOR });
    v.push(Vertex { pos: [0.0, -arm], color: MARKER_COLOR });
    v.push(Vertex { pos: [0.0, arm], color: MARKER_COLOR });

    v
}

fn vertex_bytes(vertices: &[Vertex]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vertices.len() * 24);
    for vert in vertices {
        for f in vert.pos.iter().chain(vert.color.iter()) {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
    }
    bytes
}

fn ref_uniform_bytes(center_m: (f64, f64), m_per_px: f64, viewport: (f32, f32)) -> [u8; REF_UNIFORM_SIZE] {
    let f32s: [f32; 8] = [
        center_m.0 as f32,
        center_m.1 as f32,
        m_per_px as f32,
        0.0,
        viewport.0,
        viewport.1,
        0.0,
        0.0,
    ];
    let mut out = [0u8; REF_UNIFORM_SIZE];
    for (i, v) in f32s.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

pub struct ReferenceRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    vertex_buffer: wgpu::Buffer,
    vertex_count: u32,
}

impl ReferenceRenderer {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        use wgpu::util::DeviceExt;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("reference shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/reference.wgsl").into()),
        });

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("reference uniform"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("reference view uniform"),
            size: REF_UNIFORM_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("reference bind"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("reference pipeline layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("reference pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 24,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 },
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 8, shader_location: 1 },
                    ],
                })],
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
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let vertices = build_vertices();
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("reference vertices"),
            contents: &vertex_bytes(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        Self { pipeline, bind_group, uniform, vertex_buffer, vertex_count: vertices.len() as u32 }
    }

    pub fn draw(&self, queue: &wgpu::Queue, pass: &mut wgpu::RenderPass<'_>, camera: super::view::Camera) {
        queue.write_buffer(&self.uniform, 0, &ref_uniform_bytes(camera.center_m, camera.m_per_px, camera.viewport));
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..self.vertex_count, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_labels_include_the_emphasis_ring_with_units() {
        let labels = ring_labels();
        assert!(labels.iter().any(|(_, l)| l == "230 km"), "{labels:?}");
        assert!(labels.iter().any(|(_, l)| l == "50"));
        assert!(labels.iter().any(|(_, l)| l == "300"));
    }

    #[test]
    fn polar_puts_zero_azimuth_due_north() {
        let p = polar(10_000.0, 0.0);
        assert!(p[0].abs() < 1e-3 && (p[1] - 10_000.0).abs() < 1e-3, "{p:?}");
        let e = polar(10_000.0, 90.0);
        assert!((e[0] - 10_000.0).abs() < 1e-3 && e[1].abs() < 1e-3, "90 deg is due east: {e:?}");
    }

    #[test]
    fn vertex_buffer_is_non_empty_and_stride_aligned() {
        let v = build_vertices();
        assert!(v.len() > RING_SEGMENTS * 2);
        assert_eq!(vertex_bytes(&v).len() % 24, 0);
    }
}
