// Map underlay / site marker pass (S5-W4 §7, S5-W5 §8): a LineList in world
// (metre) coordinates, one draw per layer, colour supplied by the uniform
// rather than a per-vertex attribute (S5-d — four constants are not worth
// doubling the vertex buffer's cost). The vertex transform is the same
// three lines as reference.wgsl's; duplicated rather than shared through a
// WGSL include mechanism that would exist for three lines (S5-d).

struct View {
    center_m: vec2<f32>,
    m_per_px: f32,
    _pad0: f32,
    viewport: vec2<f32>,
    _pad1: vec2<f32>,
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> view: View;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_main(@location(0) world: vec2<f32>) -> VsOut {
    var out: VsOut;
    let x = (world.x - view.center_m.x) / view.m_per_px / (view.viewport.x * 0.5);
    let y = (world.y - view.center_m.y) / view.m_per_px / (view.viewport.y * 0.5);
    out.clip = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return view.color;
}
