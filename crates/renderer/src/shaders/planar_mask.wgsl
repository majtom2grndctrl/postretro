// Planar water mask: the selected plane's visible water within R, marked in mirror depth.
// See: context/lib/rendering_pipeline.md §7.3a

// Planar passes bind no probe resource: the mask reads the main view's own
// depth for occlusion and the main eye for R, nothing else. Both entry
// points are pinned whole by text snapshot, so no cut lands unreviewed.

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
    // The mod's reflection style: FLUID_REFLECTION_STYLE_SMOOTH, else pixel.
    reflection_style: u32,
    // The displayed offset's multiplier: 1, or 0 where a test forces it to zero.
    offset_scale: i32,
    // Padding to whole vec4s; carries nothing.
    spare: vec2<f32>,
};
@group(0) @binding(0) var<uniform> planar: PlanarInputs;
@group(0) @binding(1) var scene_depth: texture_depth_2d;

// The smallest clip w a projection keeps: a placement clamp, never a test.
const PLANAR_W_FLOOR: f32 = 1.0e-6;
// Depth slack before water counts as behind the opaque scene at its pixel.
const PLANAR_MASK_DEPTH_SLACK: f32 = 1.0e-6;

struct MaskVertex {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
};

@vertex
fn vs_planar_mask(@location(0) position: vec3<f32>) -> MaskVertex {
    var out: MaskVertex;
    let clip = planar.main_vp * vec4<f32>(position, 1.0);
    // The mirror target is the main screen mirrored in x: a plane point lands
    // on the mirror texel above its own reflection.
    out.clip_position = vec4<f32>(-clip.x, clip.y, clip.z, clip.w);
    out.world_position = position;
    return out;
}

struct MaskFragment {
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_planar_mask(in: MaskVertex) -> MaskFragment {
    let clip = planar.main_vp * vec4<f32>(in.world_position, 1.0);
    let ndc = clip.xyz / max(clip.w, PLANAR_W_FLOOR);
    let scene = planar.extent.zw;
    let last = vec2<i32>(scene) - vec2<i32>(1);
    let pixel = clamp(vec2<i32>(floor(vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * scene)),
        vec2<i32>(0), last);
    // Occlusion: water behind the opaque scene at its own pixel shows nothing.
    if ndc.z > textureLoad(scene_depth, pixel, 0) + PLANAR_MASK_DEPTH_SLACK {
        discard;
    }
    // Receiver reach, distance only.
    if planar.limits.z > 0.0 && distance(planar.main_eye.xyz, in.world_position) >= planar.limits.z {
        discard;
    }
    var out: MaskFragment;
    out.depth = 1.0;
    return out;
}
