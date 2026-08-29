// Reference geometry pass (S4-W5 §8): range rings, azimuth spokes, and the
// site marker, as a LineList in world (metre) coordinates. Pan and zoom are
// the uniform — the vertex buffer is built once at startup.

struct View {
    center_m: vec2<f32>,
    m_per_px: f32,
    _pad0: f32,
    viewport: vec2<f32>,
    _pad1: vec2<f32>,
};

@group(0) @binding(0) var<uniform> view: View;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@location(0) world: vec2<f32>, @location(1) color: vec4<f32>) -> VsOut {
    var out: VsOut;
    let x = (world.x - view.center_m.x) / view.m_per_px / (view.viewport.x * 0.5);
    let y = (world.y - view.center_m.y) / view.m_per_px / (view.viewport.y * 0.5);
    out.clip = vec4<f32>(x, y, 0.0, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
