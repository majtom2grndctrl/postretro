// Glossy world faces' shared surface: the camera and world-material groups,
// the world vertex a glossy redraw interpolates, and the texel read it shares
// with forward. Declares groups 0 and 1 only; glossy_overlay.wgsl composes it
// with its own groups and entries.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces)

// --- Group 0: the shared camera uniforms, forward's struct member for member.
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
    dynamic_direct_scale: f32,
    has_scatter: u32,
    has_direct: u32,
    total_light_count: u32,
    spec_shadowmask_flags: u32,
};
@group(0) @binding(0) var<uniform> uniforms: Uniforms;

// --- Group 1: the face's world material, forward's group 1 and bind group.
// Albedo and the specular slot give the texel's reflection weight and colour,
// read where forward reads its highlight's; the material uniform and the
// specular slot's height run forward's Surface Depth march, and the normal map
// shapes the normal the reflection mirrors about. Never the glow.
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(2) var spec_texture: texture_2d<f32>;
// forward.wgsl's `MaterialUniform`, member for member: the march reads its
// Surface Depth words, which the player's switch rewrites live.
struct MaterialUniform {
    shininess: f32,
    emissive_strength: f32,
    surface_depth_peak_raise: f32,
    surface_depth_trough: f32,
    surface_depth_meters: f32,
    surface_depth_fade_distance: f32,
    surface_depth_quantize_levels: f32,
    surface_depth_march: u32,
};
@group(1) @binding(3) var<uniform> material: MaterialUniform;
@group(1) @binding(4) var t_normal: texture_2d<f32>;
@group(1) @binding(5) var aniso_sampler: sampler;

struct GlossyVertexInput {
    @location(0) position: vec3<f32>,
    @location(1) base_uv: vec2<f32>,
    @location(2) normal_oct: vec2<u32>,
    // The world probe stream: the face's bound probe, written once at
    // install from id 54's world bindings, or the no-probe value.
    @location(3) probe: u32,
    @location(4) tangent_packed: vec2<u32>,
};

// Position, UV, normal and tangent interpolate as forward's do, so the march
// and the normal map read forward's inputs.
struct GlossyVertexOutput {
    // Forward's exact clip-position expression over the same uniform, so the
    // Equal test passes on exactly the pixels forward shaded.
    @invariant @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) @interpolate(flat) probe: u32,
    @location(3) uv: vec2<f32>,
    @location(4) world_tangent: vec3<f32>,
    @location(5) bitangent_sign: f32,
};

// forward.wgsl's `oct_decode`, statement for statement.
fn oct_decode(enc: vec2<u32>) -> vec3<f32> {
    let ox = f32(enc.x) / 65535.0 * 2.0 - 1.0;
    let oy = f32(enc.y) / 65535.0 * 2.0 - 1.0;
    let z = 1.0 - abs(ox) - abs(oy);
    var x: f32;
    var y: f32;
    if z < 0.0 {
        x = (1.0 - abs(oy)) * select(-1.0, 1.0, ox >= 0.0);
        y = (1.0 - abs(ox)) * select(-1.0, 1.0, oy >= 0.0);
    } else {
        x = ox;
        y = oy;
    }
    return normalize(vec3<f32>(x, y, z));
}

// forward.wgsl's `sample_post_retro`, statement for statement: the texel
// forward's highlight reads, read the same way.
fn sample_post_retro(tex: texture_2d<f32>, samp: sampler, uv: vec2<f32>,
                     ddx: vec2<f32>, ddy: vec2<f32>) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(tex, 0));
    let uv_recon = post_retro_texel_uv(uv, dims, ddx, ddy);
    return textureSampleGrad(tex, samp, uv_recon, ddx, ddy);
}
