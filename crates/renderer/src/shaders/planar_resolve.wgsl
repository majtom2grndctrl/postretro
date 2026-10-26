// Planar resolve: each selected-plane fluid fragment's mirror sample, written (c·X, c).
// See: context/lib/rendering_pipeline.md §7.3a

// Composed with fluid_motion.wgsl and fluid_reflection_ray.wgsl, as the
// reflection pass composes them, so a pixel-style cell takes the same offset,
// tilted normal and ray SSR and the probe term take. Mirrors
// `postretro_render_cpu::planar_tilt`; each planar_ function here is pinned
// to it. Coverage, D and R are the only weights: c is 0 where the mirror
// holds no content, else the distance fades, and the tilt moves only where
// the layer samples. Planar binds no probe resource.

// --- Group 0: camera uniforms (shared with forward, fluid and reflection) ---
// Field offsets mirror the Rust `Uniforms` writer; see fluid_surface.wgsl.
struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_position: vec3<f32>,
    ambient_floor: f32,
    light_count: u32,
    time: f32,
    light_term_mask: u32,
    indirect_scale: f32,
    sdf_shadow_flags: u32,
    sdf_shadow_mode: u32,
    sdf_force_visibility_one: u32,
    direct_scale: f32,
    has_scatter: u32,
    has_direct: u32,
    total_light_count: u32,
    _spec_shadowmask_flags_inert: u32,
};
@group(0) @binding(0) var<uniform> uniforms: Uniforms;

// --- Group 1: the face's world material, read for its texel grid only ---
@group(1) @binding(0) var base_texture: texture_2d<f32>;

// --- Group 2: the planar inputs and the mirror's colour and depth ---
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
@group(2) @binding(0) var<uniform> planar: PlanarInputs;
@group(2) @binding(1) var mirror_color: texture_2d<f32>;
@group(2) @binding(2) var mirror_depth: texture_depth_2d;

// The smallest clip w a projection keeps: a placement clamp, never a test.
const PLANAR_W_FLOOR: f32 = 1.0e-6;

struct ResolveVertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct ResolveVertex {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

// No baked light: the resolve reads only position, the flat normal and the
// texel grid, as the reflection pass's minimal vertex entry.
@vertex
fn vs_planar_resolve(in: ResolveVertexInput) -> ResolveVertex {
    var out: ResolveVertex;
    out.clip_position = planar.main_vp * vec4<f32>(in.position, 1.0);
    out.world_position = in.position;
    out.world_normal = in.normal;
    out.uv = in.uv;
    return out;
}

// Absolute-metre fade from `limit`, over `band`; a zero limit keeps 1.
fn planar_distance_fade(metres: f32, limit: f32, band: f32) -> f32 {
    if limit <= 0.0 {
        return 1.0;
    }
    return clamp((limit - metres) / max(band, 0.001), 0.0, 1.0);
}

// A pixel-style cell's reflection: its unpulled centre, its tilted normal and
// its ray, from the shared helpers SSR's pass composes.
struct PlanarTiltRay {
    cell_centre: vec3<f32>,
    cell_normal: vec3<f32>,
    tilted_ray: vec3<f32>,
};

// The cell's offset is the albedo's hashed shimmer and rings (never drift or
// the wave); its centre is never held to a probe box, which planar never
// reads: the origin is the unpulled centre the composite projects.
fn planar_tilt_ray(texel_uv: vec2<f32>, world_position: vec3<f32>, flat_normal: vec3<f32>,
                   axes: FluidTexelAxes, eye: vec3<f32>, time: f32, texels_per_pixel: f32,
                   offset_scale: i32) -> PlanarTiltRay {
    let offset = fluid_displayed_offset(vec2<i32>(floor(texel_uv)), time, texels_per_pixel)
        * offset_scale;
    let cell_centre = fluid_reflection_cell_centre(texel_uv, world_position, axes);
    let cell_normal = fluid_reflection_cell_normal(flat_normal, axes, offset);
    let view_vector = normalize(eye - cell_centre);
    let reflected_ray = fluid_reflection_mirror(view_vector, cell_normal);
    let tilted_ray = fluid_reflection_above_horizon(reflected_ray);
    return PlanarTiltRay(cell_centre, cell_normal, tilted_ray);
}

// The mirror texel showing real-world `point`: its clip w floored and its
// position clamped to the target, a placement, never a decision, so a point
// off the mirror frustum or behind the mirror eye lands on the target's edge.
fn planar_mirror_texel(point: vec3<f32>) -> vec2<i32> {
    let mirror_clip = planar.mirror_vp * vec4<f32>(point, 1.0);
    let mirror_ndc = mirror_clip.xy / max(mirror_clip.w, PLANAR_W_FLOOR);
    let size = planar.extent.xy;
    let at = floor(vec2<f32>(mirror_ndc.x * 0.5 + 0.5, 0.5 - mirror_ndc.y * 0.5) * size);
    return vec2<i32>(clamp(at, vec2<f32>(0.0), size - vec2<f32>(1.0)));
}

// The real-world point mirror texel `texel` shows at its stored `depth`.
fn planar_mirror_surface(texel: vec2<i32>, depth: f32) -> vec3<f32> {
    let centre = (vec2<f32>(texel) + vec2<f32>(0.5)) / planar.extent.xy;
    let surface_h = planar.inverse_mirror_vp
        * vec4<f32>(centre.x * 2.0 - 1.0, 1.0 - centre.y * 2.0, depth, 1.0);
    return surface_h.xyz / surface_h.w;
}

// Coverage: the clear (unmasked) and the mask's far value (masked, nothing
// drawn) hold no mirror content.
fn planar_covered(depth: f32) -> bool {
    return depth > 0.0 && depth < 1.0;
}

// X = P + r'·d: the tilted ray from the cell centre, out to the distance the
// mirror depth gives at the untilted texel, never a preset distance.
fn planar_tilted_point(tilt: PlanarTiltRay, untilted: vec2<i32>, untilted_depth: f32) -> vec3<f32> {
    let reach = distance(planar_mirror_surface(untilted, untilted_depth), tilt.cell_centre);
    return tilt.cell_centre + tilt.tilted_ray * reach;
}

// Mirror texel `texel` as the layer stores it: c from its coverage, D at the
// surface it shows and R at the receiving fragment `receiver`, so R cuts
// where the mask does. The composite reads a pixel-style cell at its
// centre's texel, so every cell still reads one value.
fn planar_layer_value(texel: vec2<i32>, receiver: vec3<f32>) -> vec4<f32> {
    let depth = textureLoad(mirror_depth, texel, 0);
    if !planar_covered(depth) {
        return vec4<f32>(0.0);
    }
    let surface = planar_mirror_surface(texel, depth);
    let c = planar_distance_fade(distance(planar.mirror_eye.xyz, surface), planar.limits.x,
        planar.limits.y)
        * planar_distance_fade(distance(planar.main_eye.xyz, receiver), planar.limits.z,
        planar.limits.w);
    // An Rgba16Float texel may hold Inf or a negative value: clamped, so a
    // bilinear read of the layer never meets Inf x 0.
    let colour = clamp(textureLoad(mirror_color, texel, 0).rgb, vec3<f32>(0.0),
        vec3<f32>(65504.0));
    return vec4<f32>(colour * c, c);
}

@fragment
fn fs_planar_resolve(in: ResolveVertex) -> @location(0) vec4<f32> {
    // Derivatives in uniform control flow, before either style's branch.
    let ddx = dpdx(in.uv);
    let ddy = dpdy(in.uv);
    let ddx_world = dpdx(in.world_position);
    let ddy_world = dpdy(in.world_position);
    let dims = vec2<f32>(textureDimensions(base_texture, 0));
    // Per layer texel here; scaled to per scene pixel, as the reflection pass
    // scales its own, so both layers fade and tilt the same cells.
    let scale = planar.extent.xy / planar.extent.zw;
    let texels_per_pixel = max(length(ddx * dims * scale.x), length(ddy * dims * scale.y));
    // Smooth tilts nothing: the fragment's own mirror texel.
    if planar.reflection_style == FLUID_REFLECTION_STYLE_SMOOTH {
        return planar_layer_value(planar_mirror_texel(in.world_position), in.world_position);
    }
    let axes = fluid_reflection_texel_axes(ddx * dims, ddy * dims, ddx_world, ddy_world);
    let tilt = planar_tilt_ray(in.uv * dims, in.world_position, normalize(in.world_normal), axes,
        uniforms.camera_position, uniforms.time, texels_per_pixel, planar.offset_scale);
    // The untilted texel's depth gives d; with no content there, c is 0.
    let untilted = planar_mirror_texel(tilt.cell_centre);
    let untilted_depth = textureLoad(mirror_depth, untilted, 0);
    if !planar_covered(untilted_depth) {
        return vec4<f32>(0.0);
    }
    return planar_layer_value(planar_mirror_texel(planar_tilted_point(tilt, untilted,
        untilted_depth)), in.world_position);
}
