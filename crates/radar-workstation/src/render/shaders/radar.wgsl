// Radar pass (ADR-0023). One full-screen triangle; the fragment shader maps
// each pixel back to (ground range, azimuth), converts ground range to slant
// range with the 4/3-earth model, indexes the R8 grid with textureLoad, and
// does exactly one LUT lookup. No colour arithmetic anywhere — the LUT is
// pre-compiled on the CPU (compute::palette::compile_lut) and sRGB-corrected
// there if the surface needs it.

struct View {
    center_m: vec2<f32>,
    m_per_px: f32,
    _pad0: f32,
    viewport: vec2<f32>,
    _pad1: vec2<f32>,
    azimuth_count: u32,
    gate_count: u32,
    first_gate_m: f32,
    gate_width_m: f32,
    elevation_rad: f32,
    is_ground_range: u32,
    _pad2: vec2<f32>,
};

struct Lut {
    colors: array<vec4<f32>, 256>,
};

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<uniform> lut: Lut;
@group(1) @binding(0) var grid: texture_2d<u32>;

// 4/3-effective-earth radius, metres — the exact constant in
// compute::geometry (KE_A = 4/3 * 6_371_000).
const KE_A: f32 = 8494666.6667;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    // Oversized triangle covering the whole clip volume.
    let x = f32(i32(vi) / 2) * 4.0 - 1.0;
    let y = f32(i32(vi) & 1) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    // 1. pixel -> world offset from the site, metres. Screen y is down;
    //    north is +y, so the y term is negated.
    let dx = (frag.x - view.viewport.x * 0.5) * view.m_per_px + view.center_m.x;
    let dy = -(frag.y - view.viewport.y * 0.5) * view.m_per_px + view.center_m.y;

    let ground = sqrt(dx * dx + dy * dy);

    // 2. azimuth: atan2(x, y) so 0deg = North, increasing clockwise. This is
    //    the SAME convention as radar-viz's render_grid_ppi and
    //    compute::grid::azimuth_slot. Do not "fix" it to atan2(y, x).
    var az_deg = degrees(atan2(dx, dy));
    if (az_deg < 0.0) {
        az_deg = az_deg + 360.0;
    }

    // 3. ground range -> slant range (compute::geometry::slant_range_and_height's
    //    closed form). Skipped for a derived product, whose gate axis IS
    //    ground range (compute::derived).
    var axis_range = ground;
    if (view.is_ground_range == 0u) {
        let phi = ground / KE_A;
        let denom = cos(view.elevation_rad + phi);
        if (abs(denom) < 1e-6) {
            discard;
        }
        axis_range = KE_A * sin(phi) / denom;
    }

    // 4. gate index along the axis.
    let gate = i32(floor((axis_range - view.first_gate_m) / view.gate_width_m));
    if (gate < 0 || gate >= i32(view.gate_count)) {
        discard;
    }

    // 5. azimuth slot: floor(az / spacing), NEVER round. The centre-vs-
    //    leading-edge binning rule was measured, not assumed — see
    //    compute::grid's top-level doc comment. round() here silently
    //    rotates every image by a quarter of a bin.
    let spacing = 360.0 / f32(view.azimuth_count);
    var slot = i32(floor(az_deg / spacing));
    slot = slot % i32(view.azimuth_count);
    if (slot < 0) {
        slot = slot + i32(view.azimuth_count);
    }

    // 6. one textureLoad, one LUT lookup. R8Uint + textureLoad makes
    //    filtering structurally impossible — correct for a grid whose cell
    //    values 0 and 1 are sentinels (ADR-0020).
    let cell = textureLoad(grid, vec2<i32>(gate, slot), 0).r;
    return lut.colors[cell];
}
