// Planar resolve: each selected-plane fluid fragment's mirror texel, written (c·X, c).
// See: context/lib/rendering_pipeline.md §7.3a

// Coverage, D and R are the only weights: c is 0 where the mirror holds no
// content, else the distance fades. Planar binds no probe resource.

struct PlanarInputs {
    main_vp: mat4x4<f32>,
    mirror_vp: mat4x4<f32>,
    inverse_mirror_vp: mat4x4<f32>,
    // xyz: the main eye; w: the plane height.
    main_eye: vec4<f32>,
    // xyz: the mirror eye.
    mirror_eye: vec4<f32>,
    // D, D band, R, R band in metres; a zero limit is unlimited.
    limits: vec4<f32>,
    // Mirror target width and height, scene width and height.
    extent: vec4<f32>,
};
@group(0) @binding(0) var<uniform> planar: PlanarInputs;
@group(0) @binding(1) var mirror_color: texture_2d<f32>;
@group(0) @binding(2) var mirror_depth: texture_depth_2d;

// The smallest clip w a projection keeps: a placement clamp, never a test.
const PLANAR_W_FLOOR: f32 = 1.0e-6;

struct ResolveVertex {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
};

@vertex
fn vs_planar_resolve(@location(0) position: vec3<f32>) -> ResolveVertex {
    var out: ResolveVertex;
    out.clip_position = planar.main_vp * vec4<f32>(position, 1.0);
    out.world_position = position;
    return out;
}

// Absolute-metre fade from `limit`, over `band`; a zero limit keeps 1.
fn planar_distance_fade(metres: f32, limit: f32, band: f32) -> f32 {
    if limit <= 0.0 {
        return 1.0;
    }
    return clamp((limit - metres) / max(band, 0.001), 0.0, 1.0);
}

@fragment
fn fs_planar_resolve(in: ResolveVertex) -> @location(0) vec4<f32> {
    // The mirror texel this fragment's own world position projects to,
    // clamped to the target: a placement, not a decision.
    let mirror_clip = planar.mirror_vp * vec4<f32>(in.world_position, 1.0);
    let mirror_ndc = mirror_clip.xy / max(mirror_clip.w, PLANAR_W_FLOOR);
    let size = planar.extent.xy;
    let last = vec2<i32>(size) - vec2<i32>(1);
    let texel = clamp(vec2<i32>(floor(vec2<f32>(mirror_ndc.x * 0.5 + 0.5,
        0.5 - mirror_ndc.y * 0.5) * size)), vec2<i32>(0), last);
    let depth = textureLoad(mirror_depth, texel, 0);
    // Coverage: the clear (unmasked) or the mask's far value (masked, nothing
    // drawn) holds no mirror content.
    if depth <= 0.0 || depth >= 1.0 {
        return vec4<f32>(0.0);
    }
    let centre = (vec2<f32>(texel) + vec2<f32>(0.5)) / size;
    let surface_h = planar.inverse_mirror_vp
        * vec4<f32>(centre.x * 2.0 - 1.0, 1.0 - centre.y * 2.0, depth, 1.0);
    let surface = surface_h.xyz / surface_h.w;
    let c = planar_distance_fade(distance(planar.mirror_eye.xyz, surface), planar.limits.x,
        planar.limits.y)
        * planar_distance_fade(distance(planar.main_eye.xyz, in.world_position), planar.limits.z,
        planar.limits.w);
    // An Rgba16Float texel may hold Inf or a negative value: clamped, so a
    // bilinear read of the layer never meets Inf x 0.
    let colour = clamp(textureLoad(mirror_color, texel, 0).rgb, vec3<f32>(0.0),
        vec3<f32>(65504.0));
    return vec4<f32>(colour * c, c);
}
