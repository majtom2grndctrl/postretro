// UI SDF ring / arc shader. A six-vertex bounding quad expands each instance;
// the fragment stage cuts an anti-aliased annulus and optional clockwise wedge.
// Angles use the UI convention: 0 = straight up, positive = clockwise in
// Y-down device space. See context/lib/ui.md.

struct UiUbo {
    viewport_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> ubo: UiUbo;

struct Instance {
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) radius: f32,
    @location(3) thickness: f32,
    @location(4) start_angle: f32,
    @location(5) sweep: f32,
    @location(6) depth: f32,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) device_pos: vec2<f32>,
    @location(1) rect_center: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) radius: f32,
    @location(4) thickness: f32,
    @location(5) start_angle: f32,
    @location(6) sweep: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32, inst: Instance) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let device_pos = inst.rect.xy + corners[vid] * inst.rect.zw;
    let ndc = vec2<f32>(
        device_pos.x / ubo.viewport_size.x * 2.0 - 1.0,
        1.0 - device_pos.y / ubo.viewport_size.y * 2.0,
    );
    var out: VsOut;
    out.position = vec4<f32>(ndc, inst.depth, 1.0);
    out.device_pos = device_pos;
    out.rect_center = inst.rect.xy + inst.rect.zw * 0.5;
    out.color = inst.color;
    out.radius = inst.radius;
    out.thickness = inst.thickness;
    out.start_angle = inst.start_angle;
    out.sweep = inst.sweep;
    return out;
}

const TAU: f32 = 6.283185307179586;
const FULL_CIRCLE_EPSILON: f32 = 0.0001;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let relative = in.device_pos - in.rect_center;
    let radial_distance = length(relative);
    let inner_radius = max(0.0, in.radius - in.thickness);
    let radial_aa = max(fwidth(radial_distance), 0.75);
    let outer_coverage = 1.0 - smoothstep(
        in.radius - radial_aa,
        in.radius + radial_aa,
        radial_distance,
    );
    let inner_coverage = smoothstep(
        inner_radius - radial_aa,
        inner_radius + radial_aa,
        radial_distance,
    );

    // atan2(x, -y) puts 0 at up and increases clockwise in UI's Y-down space.
    let angle = atan2(relative.x, -relative.y);
    let normalized_angle = select(angle + TAU, angle, angle >= 0.0);
    let delta = normalized_angle - in.start_angle
        - floor((normalized_angle - in.start_angle) / TAU) * TAU;
    let angular_aa = max(fwidth(delta), 0.0001);
    var wedge_coverage = 1.0;
    if (in.sweep < TAU - FULL_CIRCLE_EPSILON) {
        let start_coverage = smoothstep(0.0, angular_aa, delta);
        let end_coverage = 1.0 - smoothstep(
            in.sweep - angular_aa,
            in.sweep + angular_aa,
            delta,
        );
        wedge_coverage = min(start_coverage, end_coverage);
    }
    return vec4<f32>(in.color.rgb, in.color.a * outer_coverage * inner_coverage * wedge_coverage);
}
