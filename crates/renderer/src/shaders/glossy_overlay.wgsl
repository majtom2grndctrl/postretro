// The glossy world overlay: an Equal-depth redraw of glossy buckets that adds
// W x (R - D_static) over forward's colour, R the bound probe's reflection
// coloured as the texel's highlight, W its Fresnel x `_s` x sharpness fade x
// capture-shadow weight, and D_static forward's static diffuse at the same
// pixel. Concatenated with glossy_surface.wgsl (the camera and world-material
// groups and the world vertex), glossy_overlay_shading.wgsl (where the fragment
// reflects from and about which normal: the reflection style's point, forward's
// Surface Depth march and normal map), material_shading.wgsl (the tint rule
// forward's highlight reads), surface_depth.wgsl (forward's march),
// reflection_probe_blur.wgsl (the exponent -> mip function that picks the
// colour mips R reads), reflection_probe_lobes.wgsl, fluid_motion.wgsl,
// fluid_reflection_ray.wgsl and fluid_reflection.wgsl, whose probe search,
// hemisphere, live lighting and Fresnel it reuses by name. Mirrors render-cpu
// `glossy_reflection` and `reflection_probe_mips`.
// See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces)

// --- Group 2: the level's probe records, read only: the colour array, the
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
@group(2) @binding(0) var fluid_probe_array: texture_2d_array<f32>;
@group(2) @binding(1) var<uniform> fluid_probes: FluidProbeTable;
@group(2) @binding(2) var fluid_probe_distance: texture_2d_array<f32>;
@group(2) @binding(3) var<uniform> fluid_probe_sums: FluidProbeSums;

// --- Group 3: the frame's inputs: forward's static diffuse, read at the
// fragment's own pixel, and the linear-clamp sampler the probe reads take
// (named as the fluid shader names the one it borrows). D_static's alpha is
// the coplanar-mover guard's mark: 0 as forward clears and writes it, 1 where
// this frame's mover guard pass found a mover.
@group(3) @binding(0) var static_diffuse: texture_2d<f32>;
@group(3) @binding(1) var sh_atlas_sampler: sampler;

// Water's reflectance scale (0.02 x surfaceReflectivity): an untinted texel's
// head-on base of 0.04, raised by its tint to water's mirror cap, 1.
const GLOSSY_REFLECTIVITY_UNTINTED: f32 = 2.0;
const GLOSSY_REFLECTIVITY_FULL_TINT: f32 = 50.0;
// The sharpness fade: no weight at or below the floor exponent, full weight
// at or above the full-weight exponent, linear in log2 of the exponent between.
const GLOSSY_SHARPNESS_FLOOR_EXPONENT: f32 = 8.0;
const GLOSSY_FULL_WEIGHT_EXPONENT: f32 = 16.0;

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
    out.world_normal = oct_decode(in.normal_oct);
    out.probe = in.probe;
    out.uv = in.base_uv;
    // forward.wgsl's tangent decode, statement for statement.
    let sign_bit = in.tangent_packed.y & 0x8000u;
    let v_15bit = in.tangent_packed.y & 0x7FFFu;
    let v_16bit = v_15bit * 65535u / 32767u;
    out.world_tangent = oct_decode(vec2<u32>(in.tangent_packed.x, v_16bit));
    out.bitangent_sign = select(-1.0, 1.0, sign_bit != 0u);
    return out;
}

// The sharpness fade. Mirrors level-format `glossy_sharpness_fade`.
fn glossy_sharpness_fade(exponent: f32) -> f32 {
    let floor = log2(GLOSSY_SHARPNESS_FLOOR_EXPONENT);
    let full = log2(GLOSSY_FULL_WEIGHT_EXPONENT);
    return clamp((log2(exponent) - floor) / (full - floor), 0.0, 1.0);
}

// What one read of a texel's material says about its reflection: `_s` x the
// sharpness fade, water's reflectivity raised by the tint, and the colour the
// texel's highlight takes (the tint rule's for a tinted texel, raw albedo
// without `_t`), and the exponent its highlight shades at, which picks the
// colour mips R reads. A texel without smoothness shades at the minimum
// exponent, below the floor, so it reflects nothing. Mirrors render-cpu
// `glossy_texel`.
struct GlossyTexel {
    mask: f32,
    reflectivity: f32,
    colour: vec3<f32>,
    exponent: f32,
};

fn glossy_texel(slot: vec4<f32>, albedo: vec3<f32>) -> GlossyTexel {
    let texel = specular_texel(slot, SPECULAR_EXPONENT_MIN);
    return GlossyTexel(
        texel.intensity * glossy_sharpness_fade(texel.exponent),
        select(GLOSSY_REFLECTIVITY_UNTINTED,
            mix(GLOSSY_REFLECTIVITY_UNTINTED, GLOSSY_REFLECTIVITY_FULL_TINT, texel.tint),
            texel.tinted),
        select(albedo, specular_tint(albedo, texel.tint), texel.tinted),
        texel.exponent,
    );
}

// F, the texel's unshadowed weight: water's Schlick Fresnel at the texel's
// reflectivity, times its mask. Mirrors render-cpu `glossy_fresnel`.
fn glossy_fresnel(texel: GlossyTexel, cos_theta: f32) -> f32 {
    return fluid_reflection_fresnel(cos_theta, texel.reflectivity) * texel.mask;
}

// The signed addition over forward's colour: F x colour x [c x X + (1 - c) x
// s x R] - W x D_static at W = F x [c + (1 - c) x s], capped at 1, so the
// output is forward + W x (R - D_static), R coloured as the highlight. `screen`
// is the floor layer's premultiplied (c x X, c); zero (the probe term alone)
// where no screen-space layer runs. s = 0 at c = 0 adds exactly nothing: the
// capture shadow is withheld, never filled. Mirrors render-cpu
// `glossy_composite`. IP rule (owner review to change, rendering_pipeline.md
// §7.3a): s scales the probe's share alone, so F x colour x c x X is the
// same at every s.
fn glossy_composite(static_diffuse: vec3<f32>, probe: vec3<f32>, shadow: f32,
                    screen: vec4<f32>, fresnel: f32, colour: vec3<f32>) -> vec3<f32> {
    let confidence = screen.a;
    let probe_share = (1.0 - confidence) * shadow;
    let reflected = colour * (fresnel * (screen.rgb + probe * probe_share));
    let weight = min(fresnel * (confidence + probe_share), 1.0);
    return reflected - static_diffuse * weight;
}

// The texel's material at this fragment, read as forward reads it: albedo and
// the specular slot through the Post Retro reconstruction at the UV forward
// shades (its march's, under Surface Depth). `ddx` and `ddy` come from uniform
// control flow.
fn glossy_overlay_texel(uv: vec2<f32>, ddx: vec2<f32>, ddy: vec2<f32>) -> GlossyTexel {
    let albedo = sample_post_retro(base_texture, aniso_sampler, uv, ddx, ddy).rgb;
    let slot = sample_post_retro(spec_texture, aniso_sampler, uv, ddx, ddy);
    return glossy_texel(slot, albedo);
}

// The probe term before the material's weight: the bound probe's live-lit
// capture and its capture-shadow weight s. s is 0, and nothing is added, for
// an unbound face, a face seen from behind, or a mask with an all-terms bit
// off (the capture bakes every term together). The hemisphere is the face's
// own, from its flat normal: up-facing faces read above the horizon as water
// does, walls, ceilings and slopes about their normal.
// IP rule (owner review to change, rendering_pipeline.md §7.3a): the probe
// comes from the face's binding alone, by position; the layer derives from
// it inside `glossy_overlay_sample`, and no lookup-direction value picks a
// probe or its hemisphere. No history, no precomputed intersection, and no
// tile is written.
fn glossy_overlay_probe(in: GlossyVertexOutput, surface: GlossySurface,
                        exponent: f32) -> FluidReflectionSample {
    if in.probe == FLUID_REFLECTION_NO_PROBE
        || !fluid_reflection_seen_from_above(surface.outward, surface.view)
        || !fluid_reflection_mask_on(uniforms.light_term_mask) {
        return FluidReflectionSample(vec3<f32>(0.0), 0.0);
    }
    return glossy_overlay_sample(in.probe, surface.origin, surface.view, surface.normal,
        fluid_reflection_hemisphere_axis(surface.outward), exponent);
}

// Water's `fluid_reflection_sample` with the ray kept in the face's
// hemisphere and the colour tap blurred by the texel's exponent: the search
// runs first and ends; only its lookup and weight reach the colour mips and
// the sums blend. The layer comes from the face's probe index alone. Mirrors
// render-cpu `glossy_probe_term`.
fn glossy_overlay_sample(probe: u32, origin: vec3<f32>, view: vec3<f32>, normal: vec3<f32>,
                         axis: vec3<f32>, exponent: f32) -> FluidReflectionSample {
    let ray = fluid_reflection_above_hemisphere(fluid_reflection_mirror(view, normal), axis);
    let layer = fluid_probes.probes[probe].layer;
    let search = fluid_reflection_search(origin, ray, fluid_probes.probes[probe].box_min,
        fluid_probes.probes[probe].box_max, fluid_probes.probes[probe].capture_point, axis,
        fluid_probes.tile.xy, layer);
    let capture = glossy_overlay_probe_colour(search.lookup, layer, exponent);
    return FluidReflectionSample(capture * fluid_reflection_ratio_from_sums(probe, search.lookup),
        search.weight);
}

// The probe colour toward `direction` blurred by `exponent`: the two colour
// mips the exponent -> mip function falls between, each read at its own
// tile's UV (interiors do not nest, so one UV cannot address two mips),
// blended. Mirrors render-cpu `reflection_probe_mip_colour`. IP rule (owner
// review to change, rendering_pipeline.md §7.3a): the mips were prefiltered
// at compile time and are only sampled here; the exponent picks a mip, never
// a probe, and the blur reads no capture-shadow weight or distance tile.
fn glossy_overlay_probe_colour(direction: vec3<f32>, layer: u32, exponent: f32) -> vec3<f32> {
    let count = textureNumLevels(fluid_probe_array);
    let mip = reflection_probe_exponent_mip(exponent, count);
    let lower = u32(mip);
    let upper = min(lower + 1u, count - 1u);
    return mix(glossy_overlay_probe_mip(direction, layer, lower),
        glossy_overlay_probe_mip(direction, layer, upper), mip - f32(lower));
}

// Colour mip `mip` toward `direction`, at that mip's own tile UV and level.
fn glossy_overlay_probe_mip(direction: vec3<f32>, layer: u32, mip: u32) -> vec3<f32> {
    let uv = fluid_reflection_tile_uv(direction, fluid_probes.tile.x >> mip, fluid_probes.tile.y);
    return textureSampleLevel(fluid_probe_array, sh_atlas_sampler, uv, layer, f32(mip)).rgb;
}

// Forward's D_static at this fragment's own pixel, and in its alpha the
// coplanar-mover guard's mark, in one load.
fn glossy_overlay_static_diffuse(frag_xy: vec2<f32>) -> vec4<f32> {
    return textureLoad(static_diffuse, vec2<u32>(frag_xy), 0);
}

// Whether this frame's mover guard marked this fragment's own pixel: a mover
// shaded it, so the mover pass's output stands there. Only a mover face
// coplanar with the glossy face reaches here; one in front fails Equal.
fn glossy_overlay_mover_covered(d_static: vec4<f32>) -> bool {
    return d_static.a > 0.5;
}

// The overlay's addition at this fragment for a texel of nonzero weight:
// Fresnel from the surface's view against the flat face normal, as water's,
// so a pixel-style texel is one uniform block. A pixel this frame's movers
// cover adds nothing; its mark is read with D_static, in the same load, after
// the search, which holds fewer registers than an early read on the RDNA Mac
// target and adds only work no pixel keeps (a covered pixel is rare: a mover
// face coplanar with the glossy face).
fn glossy_overlay_delta(in: GlossyVertexOutput, texel: GlossyTexel,
                        surface: GlossySurface) -> vec3<f32> {
    let fresnel = glossy_fresnel(texel, dot(surface.view, surface.outward));
    let probe = glossy_overlay_probe(in, surface, texel.exponent);
    let d_static = glossy_overlay_static_diffuse(in.clip_position.xy);
    if glossy_overlay_mover_covered(d_static) {
        return vec3<f32>(0.0);
    }
    return glossy_composite(d_static.rgb, probe.capture, probe.weight, vec4<f32>(0.0), fresnel,
        texel.colour);
}

// One style's overlay addition: forward's march picks the texel (in the
// pipeline built with it), a texel of zero weight (no `_s`, at or below the
// sharpness floor) adds nothing and reads nothing more, and the style picks
// where the reflection starts.
fn glossy_overlay_shade(in: GlossyVertexOutput, pixel: bool, march: bool,
                        normal_map: bool) -> vec3<f32> {
    let derivatives = glossy_overlay_derivatives(in);
    let depth = glossy_overlay_depth(in, derivatives, march);
    let texel = glossy_overlay_texel(depth.uv, derivatives.ddx, derivatives.ddy);
    if texel.mask <= 0.0 {
        return vec3<f32>(0.0);
    }
    let surface = glossy_overlay_surface(in, derivatives, depth, pixel, normal_map);
    return glossy_overlay_delta(in, texel, surface);
}

// Added over forward's colour (src x 1 + dst x 1): forward + W x (R - D_static),
// so dynamic light, highlights and emissive never dim, and W = 0 adds zero.
// One entry per `render.reflections.style` and material form, all built at
// full init: pixel reflects once per albedo texel, from its texel-centre
// point; smooth per fragment. `fs_pixel` and `fs_smooth` march and sample the
// normal map; the `_flat` entries draw materials that never march, the
// `_plain` entries those that also bind the neutral normal placeholder.
@fragment
fn fs_pixel(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(glossy_overlay_shade(in, true, true, true), 0.0);
}

@fragment
fn fs_smooth(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(glossy_overlay_shade(in, false, true, true), 0.0);
}

@fragment
fn fs_pixel_flat(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(glossy_overlay_shade(in, true, false, true), 0.0);
}

@fragment
fn fs_smooth_flat(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(glossy_overlay_shade(in, false, false, true), 0.0);
}

@fragment
fn fs_pixel_plain(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(glossy_overlay_shade(in, true, false, false), 0.0);
}

@fragment
fn fs_smooth_plain(in: GlossyVertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(glossy_overlay_shade(in, false, false, false), 0.0);
}
