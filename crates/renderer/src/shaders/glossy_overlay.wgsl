// The glossy world overlay: an Equal-depth redraw of glossy buckets that adds
// W x (R - D_static) over forward's colour, R the bound probe's reflection and
// D_static forward's static diffuse at the same pixel. Concatenated with
// reflection_probe_lobes.wgsl, fluid_motion.wgsl, fluid_reflection_ray.wgsl and
// fluid_reflection.wgsl, whose probe term it reuses by name.
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

// --- Group 1: the level's probe records, read only: the colour array, the
// share table, the distance array and the live-lighting sums the sums pass
// wrote this frame. The fluid shader's structs member for member.
struct FluidProbeLight {
    reference: vec3<f32>,
    roster_index: u32,
    shares: array<vec4<u32>, 2>,
};
struct FluidProbe {
    box_min: vec3<f32>,
    _pad0: u32,
    box_max: vec3<f32>,
    _pad1: u32,
    capture_point: vec3<f32>,
    layer: u32,
    lights: array<FluidProbeLight, 4>,
};
struct FluidProbeTable {
    // x = tile dimension, y = tile border, z = probe count, w spare.
    tile: vec4<u32>,
    probes: array<FluidProbe, 256>,
};
// IP rule (owner review to change, rendering_pipeline.md §7.3a): only
// probe-bound faces of geometry with fixed position and orientation read the
// sums; glossy buckets are static world faces.
struct FluidProbeSums {
    lobes: array<vec4<f32>, 1536>,
};
@group(1) @binding(0) var fluid_probe_array: texture_2d_array<f32>;
@group(1) @binding(1) var<uniform> fluid_probes: FluidProbeTable;
@group(1) @binding(2) var fluid_probe_distance: texture_2d_array<f32>;
@group(1) @binding(3) var<uniform> fluid_probe_sums: FluidProbeSums;

// --- Group 2: the frame's inputs: forward's static diffuse, read at the
// fragment's own pixel, and the linear-clamp sampler the probe reads take
// (named as the fluid shader names the one it borrows).
@group(2) @binding(0) var static_diffuse: texture_2d<f32>;
@group(2) @binding(1) var sh_atlas_sampler: sampler;

// Water's reflectance scale (0.02 x surfaceReflectivity): a head-on base of
// 0.04 for an untinted texel.
const GLOSSY_OVERLAY_REFLECTIVITY: f32 = 2.0;

struct GlossyVertexInput {
    @location(0) position: vec3<f32>,
    @location(2) normal_oct: vec2<u32>,
    // The world probe stream: the face's bound probe, written once at
    // install from id 54's world bindings, or the no-probe value.
    @location(3) probe: u32,
};

struct GlossyVertexOutput {
    // Forward's exact clip-position expression over the same uniform, so the
    // Equal test passes on exactly the pixels forward shaded.
    @invariant @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) @interpolate(flat) normal: vec3<f32>,
    @location(2) @interpolate(flat) probe: u32,
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

// sh_sample.wgsl's `oct_encode_unquantized`, statement for statement: the
// probe tiles' octahedral mapping, which fluid_reflection.wgsl reads through.
fn oct_encode_unquantized(dir_in: vec3<f32>) -> vec2<f32> {
    let dir = normalize(dir_in);
    var p = dir.xy / max(abs(dir.x) + abs(dir.y) + abs(dir.z), 1.0e-6);
    if (dir.z < 0.0) {
        let old = p;
        p = vec2<f32>(
            (1.0 - abs(old.y)) * select(-1.0, 1.0, old.x >= 0.0),
            (1.0 - abs(old.x)) * select(-1.0, 1.0, old.y >= 0.0),
        );
    }
    return p * 0.5 + vec2<f32>(0.5);
}

@vertex
fn vs_main(in: GlossyVertexInput) -> GlossyVertexOutput {
    var out: GlossyVertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.world_position = in.position;
    // The provoking vertex's normal: a face's vertices share its plane's.
    out.normal = oct_decode(in.normal_oct);
    out.probe = in.probe;
    return out;
}

// The probe term's weight and reflection, before the static diffuse they
// replace. W is 0, and nothing is added, for an unbound face, a face seen
// from behind, or a mask with an all-terms bit off (the capture bakes every
// term together).
struct GlossyOverlayTerm {
    weight: f32,
    reflection: vec3<f32>,
};

// IP rule (owner review to change, rendering_pipeline.md §7.3a): the probe
// comes from the face's binding alone, by position; the layer derives from
// it inside `fluid_reflection_sample`, and no lookup-direction value picks a
// probe. No history, no precomputed intersection, and no tile is written.
fn glossy_overlay_term(in: GlossyVertexOutput) -> GlossyOverlayTerm {
    let normal = normalize(in.normal);
    let view = normalize(uniforms.camera_position - in.world_position);
    if in.probe == FLUID_REFLECTION_NO_PROBE
        || !fluid_reflection_seen_from_above(normal, view)
        || !fluid_reflection_mask_on(uniforms.light_term_mask) {
        return GlossyOverlayTerm(0.0, vec3<f32>(0.0));
    }
    let probe = fluid_reflection_sample(in.probe, in.world_position, view, normal);
    let fresnel = fluid_reflection_fresnel(dot(view, normal), GLOSSY_OVERLAY_REFLECTIVITY);
    return GlossyOverlayTerm(fresnel * probe.weight, probe.capture);
}

// Forward's D_static at this fragment's own pixel.
fn glossy_overlay_static_diffuse(frag_xy: vec2<f32>) -> vec3<f32> {
    return textureLoad(static_diffuse, vec2<u32>(frag_xy), 0).rgb;
}

// Added over forward's colour (src x 1 + dst x 1): forward + W x (R - D_static),
// so dynamic light, highlights and emissive never dim, and W = 0 adds zero.
@fragment
fn fs_main(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    let term = glossy_overlay_term(in);
    let d_static = glossy_overlay_static_diffuse(in.clip_position.xy);
    return vec4<f32>(term.weight * (term.reflection - d_static), 0.0);
}
