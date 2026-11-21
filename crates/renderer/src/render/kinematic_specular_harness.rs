// GPU readback harness for the mover pass's selected-static specular: each
// style pipeline's runtime light loop (`accumulate_pixel_direct`,
// `accumulate_smooth_direct`) and post-loop composition (`mover_pixel_shade`,
// `mover_smooth_shade`), the pixel-style point (`mover_specular_point`) and the
// per-light texel term (`mover_texel_specular`), beside frozen copies of the
// 1c7fc9e3a and 74fcfd4b9 loops. Its material path reads a real albedo and
// specular slot and shades sharpness and tint as the mover's `fs_pixel` and
// `fs_smooth` do.
// See: context/lib/rendering_pipeline.md §4 (Texel-centre highlight), §7.3
//
// Intentional exception to testing_guide.md §3 "No GPU context in tests": the
// functions are WGSL, so verifying them means running them. Every production
// function is spliced in by name from its shader, the Surface Depth march runs
// for real over a small height texture, and only the bindings, the pool-shadow
// samplers and the entry points are harness code. The harness self-skips
// without an adapter and says so; a skipped run proves nothing. Set
// `POSTRETRO_REQUIRE_GPU` to any non-empty value other than `0` to make a
// missing adapter fail instead.

use super::gpu_test_harness::{GpuCtx, wgsl_const_line, wgsl_function, wgsl_item};
use wgpu::util::DeviceExt;

use super::specular_slot_fixtures::{MaterialTextures, SlotFormat, white_material};

pub(super) const KINEMATIC_WGSL: &str = include_str!("../shaders/kinematic_brush.wgsl");
const MATERIAL_SHADING_WGSL: &str = include_str!("../shaders/material_shading.wgsl");
const SURFACE_DEPTH_WGSL: &str = include_str!("../shaders/surface_depth.wgsl");
const LIGHT_FALLOFF_WGSL: &str = include_str!("../shaders/light_falloff.wgsl");
const LIGHT_EVAL_WGSL: &str = include_str!("../shaders/light_eval.wgsl");
const CURVE_EVAL_WGSL: &str = include_str!("../shaders/curve_eval.wgsl");
const SHADOW_SAMPLE_WGSL: &str = include_str!("../shaders/shadow_sample.wgsl");
const FROZEN_WGSL: &str = include_str!("kinematic_specular_frozen.wgsl");
const FROZEN_PRETINT_WGSL: &str = include_str!("kinematic_specular_frozen_pretint.wgsl");

/// `LATTICE`² albedo (and height) texels magnified onto `TARGET`² pixels.
pub(super) const TARGET: u32 = 64;
pub(super) const LATTICE: u32 = 8;
pub(super) const PIXELS_PER_TEXEL: u32 = TARGET / LATTICE;
const UV_PER_PIXEL: f64 = 1.0 / TARGET as f64;
/// The floor's world size per UV unit along x and z: one texel is 0.5 m.
pub(super) const METRES_PER_UV: f64 = 4.0;
/// Every fragment's UV sits this far past its pixel centre, as in the forward
/// harness. Every plane mapping is dyadic, so one texel's fragments form its
/// point to the bit.
const SUBPIXEL_OFFSET: f64 = 0.25;

/// Authored height bytes of the checker: raised and sunk texels alternate.
const HEIGHT_RAISED: u8 = 192;
const HEIGHT_SUNK: u8 = 64;
/// Signed height fractions of those bytes, `(h - 128) / 128`.
pub(super) const RAISED: f32 = 0.5;
pub(super) const SUNK: f32 = -0.5;
/// Relief in albedo texels; at 2 texels per metre this is 0.125 m.
const RELIEF_TEXELS: f32 = 0.25;
pub(super) const DEPTH_SCALE_M: f32 = 0.125;
const MAX_STEPS: u32 = 32;
const SHADOW_BUDGET: u32 = 2;
const HAS_DEPTH_BIT: u32 = 0x1000;

const LIGHT_POINT: u32 = 0;
const LIGHT_SPOT: u32 = 1;
const LIGHT_DIRECTIONAL: u32 = 2;
const NO_SLOT: u32 = u32::MAX;

const ENTRY: &str = r#"
struct Harness {
    // xyz eye, w specular exponent.
    eye_and_exponent: vec4<f32>,
    // xyz the fixed shading normal, w set to use it on every face.
    normal: vec4<f32>,
    // x output, y shadow pattern on, z observe, w use_specular.
    modes: vec4<u32>,
    // x use_dynamic, y the smooth pipeline's loop and composition.
    flags: vec4<u32>,
    // xy UV per pixel along x and y.
    uv_per_pixel: vec4<f32>,
    // x material path (0 none), y the oracle takes the decoded exponent.
    material_modes: vec4<u32>,
};
@group(0) @binding(0) var<storage, read> lights: array<GpuLight>;
@group(0) @binding(1) var<storage, read> light_influence: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> scripted_light_descriptors: array<AnimationDescriptor>;
@group(0) @binding(3) var<storage, read> anim_samples: array<f32>;
@group(0) @binding(4) var<uniform> kinematic_light_params: KinematicLightParams;
@group(0) @binding(5) var<uniform> light_space_matrices: LightSpaceMatrices;
@group(0) @binding(6) var spec_texture: texture_2d<f32>;
@group(0) @binding(7) var<uniform> material: MaterialUniform;
@group(0) @binding(8) var<uniform> harness: Harness;
@group(0) @binding(9) var base_texture: texture_2d<f32>;
@group(0) @binding(10) var aniso_sampler: sampler;

// Stand-ins for the light the mover's `fs_main` sums ahead of its loop and
// for its glow.
const HARNESS_LIGHT_BEFORE_LOOP: vec3<f32> = vec3<f32>(0.21, 0.17, 0.13);
const HARNESS_EMISSIVE: vec3<f32> = vec3<f32>(0.02, 0.0, 0.04);
const HARNESS_EMISSIVE_STRENGTH: f32 = 1.5;

// 1, 0.5 or 0 by (x + 2y) mod 3 of the fragment's own pixel, read back from
// its plane position; `fragment_shadow` mirrors it.
fn harness_shadow(world_pos: vec3<f32>) -> f32 {
    if harness.modes.y == 0u {
        return 1.0;
    }
    let pixel = vec2<u32>(floor(world_pos.xz / (METRES_PER_UV * UV_PER_PIXEL)));
    let k = (pixel.x + 2u * pixel.y) % 3u;
    return select(select(0.0, 0.5, k == 1u), 1.0, k == 0u);
}

// The loop's pool-shadow samplers, stubbed: every slotted light reads the
// fragment's own pattern value at the plane position the loop passes.
fn sample_point_shadow_with_static(slot: u32, cache: i32, light_pos: vec3<f32>, world_pos: vec3<f32>,
                                   n: vec3<f32>, bias: f32, range: f32) -> f32 {
    return harness_shadow(world_pos);
}
fn sample_point_shadow(slot: u32, light_pos: vec3<f32>, world_pos: vec3<f32>, n: vec3<f32>,
                       bias: f32, range: f32) -> f32 {
    return harness_shadow(world_pos);
}
fn sample_spot_shadow_with_static(slot: u32, cache: i32, light_pos: vec3<f32>, world_pos: vec3<f32>,
                                  n: vec3<f32>, bias: f32, proj: mat4x4<f32>) -> f32 {
    return harness_shadow(world_pos);
}
fn sample_spot_shadow(slot: u32, light_pos: vec3<f32>, world_pos: vec3<f32>, n: vec3<f32>,
                      bias: f32, proj: mat4x4<f32>) -> f32 {
    return harness_shadow(world_pos);
}

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32(vertex & 1u), f32((vertex >> 1u) & 1u));
    return vec4<f32>(corner * 4.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // The mover's `fs_main` up to its light loop, on a magnified floor facing
    // +Y with no normal map.
    let uv = (position.xy + vec2<f32>(SUBPIXEL_OFFSET)) * harness.uv_per_pixel.xy;
    let world = vec3<f32>(uv.x, 0.0, uv.y) * METRES_PER_UV;
    let ddx = dpdx(uv);
    let ddy = dpdy(uv);
    let ddx_world = dpdx(world);
    let ddy_world = dpdy(world);
    let mesh_n = vec3<f32>(0.0, 1.0, 0.0);
    let eye = harness.eye_and_exponent.xyz;
    let view_vector = eye - world;
    let view_distance = length(view_vector);
    let V = normalize(view_vector);
    let depth = surface_depth_resolve(uv, world, mesh_n, V, view_distance, ddx, ddy, ddx_world, ddy_world);
    var n = select(depth.normal, mesh_n, depth.hit_top);
    if harness.normal.w != 0.0 {
        n = harness.normal.xyz;
    }
    let smooth_style = harness.flags.y != 0u;
    // Smooth evaluates every input per fragment: light from the hit, view
    // from the fragment.
    var spec_point = SpecularPoint(depth.world_position, V);
    if !smooth_style {
        spec_point = mover_specular_point(
            depth, uv, world, mesh_n, V, eye,
            vec2<f32>(textureDimensions(base_texture, 0)), ddx, ddy, ddx_world, ddy_world,
        );
    }
    let exponent = harness.eye_and_exponent.w;
    let use_specular = harness.modes.w != 0u;
    let use_dynamic = harness.flags.x != 0u;
    let emissive = HARNESS_EMISSIVE * HARNESS_EMISSIVE_STRENGTH;

    var shade: vec3<f32>;
    var observed_material = vec4<f32>(0.0);
    if harness.material_modes.x != MATERIAL_NONE {
        // The mover's beauty entry from its slot read on: one sample of the
        // slot, the loop, the albedo read after it, the entry's composition.
        let slot = sample_post_retro(spec_texture, aniso_sampler, depth.uv, ddx, ddy);
        let prefix_exponent = max(material.shininess, 1.0);
        if harness.material_modes.x == MATERIAL_ORACLE {
            // Before tint: R intensity, the prefix exponent (a `_p` slot's
            // decoded one when asked), the raw-albedo multiply.
            let exponent = select(
                prefix_exponent,
                specular_texel_exponent(slot.b, prefix_exponent),
                harness.material_modes.y != 0u,
            );
            var dynamic: vec3<f32>;
            if smooth_style {
                dynamic = frozen_accumulate_dynamic_direct(
                    world, n, mesh_n, V, exponent, slot.r, use_dynamic, use_specular, depth,
                );
            } else {
                dynamic = pretint_accumulate_dynamic_direct(
                    world, n, mesh_n, spec_point, exponent, slot.r, use_dynamic, use_specular,
                    false, depth,
                );
            }
            var lighting = HARNESS_LIGHT_BEFORE_LOOP;
            lighting = lighting + dynamic;
            let base_color = sample_post_retro(base_texture, aniso_sampler, depth.uv, ddx, ddy);
            shade = base_color.rgb * lighting + HARNESS_EMISSIVE * HARNESS_EMISSIVE_STRENGTH;
        } else {
            let spec_texel = specular_texel(slot, prefix_exponent);
            // What the loop sums apart from diffuse, and what the entry's
            // composition alone makes of it (zero light, zero glow).
            var raw: vec3<f32>;
            var added: vec3<f32>;
            let base_color = sample_post_retro(base_texture, aniso_sampler, depth.uv, ddx, ddy);
            if smooth_style {
                let dynamic = accumulate_smooth_direct(
                    world, n, mesh_n, V, spec_texel.exponent, spec_texel.intensity,
                    use_dynamic, use_specular, depth,
                );
                shade = mover_smooth_shade(
                    base_color.rgb, HARNESS_LIGHT_BEFORE_LOOP, dynamic, emissive, spec_texel,
                );
                raw = dynamic.specular;
                added = mover_smooth_shade(
                    base_color.rgb, vec3<f32>(0.0), MoverSmoothDirect(vec3<f32>(0.0), raw),
                    vec3<f32>(0.0), spec_texel,
                );
            } else {
                let dynamic = accumulate_pixel_direct(
                    world, n, mesh_n, spec_point, spec_texel.exponent, spec_texel.intensity,
                    spec_texel.tinted, use_dynamic, use_specular, depth,
                );
                shade = mover_pixel_shade(
                    base_color.rgb, HARNESS_LIGHT_BEFORE_LOOP, dynamic, emissive, spec_texel,
                );
                raw = dynamic.tinted_specular;
                added = mover_pixel_shade(
                    base_color.rgb, vec3<f32>(0.0), MoverPixelDirect(vec3<f32>(0.0), raw),
                    vec3<f32>(0.0), spec_texel,
                );
            }
            switch harness.modes.z {
                case 3u: {
                    observed_material = vec4<f32>(
                        spec_texel.exponent,
                        spec_texel.intensity,
                        select(0.0, 1.0, spec_texel.tinted),
                        spec_texel.tint,
                    );
                }
                case 4u: { observed_material = base_color; }
                case 5u: { observed_material = vec4<f32>(raw, 1.0); }
                case 6u: { observed_material = vec4<f32>(added, 1.0); }
                case 7u: { observed_material = slot; }
                default: {}
            }
        }
    } else if harness.modes.x == OUTPUT_FROZEN {
        shade = frozen_accumulate_dynamic_direct(
            world, n, mesh_n, V, exponent, 1.0, use_dynamic, use_specular, depth,
        );
    } else if harness.modes.x == OUTPUT_TEXEL_TERM {
        // Light 0 as the loop hands a selected-static record to the term: its
        // own color and aim, and this fragment's shadow.
        let light = lights[0];
        shade = mover_texel_specular(
            light, light.color_and_falloff_model.xyz, light.direction_and_range.xyz, spec_point,
            n, mesh_n, depth.hit_top, harness_shadow(world), exponent, 1.0,
        );
    } else if smooth_style {
        // The smooth loop through its own composition at unit albedo, no
        // light ahead of the loop and no glow: diffuse plus specular.
        let untinted = SpecularTexel(1.0, exponent, false, 0.0);
        shade = mover_smooth_shade(
            vec3<f32>(1.0), vec3<f32>(0.0),
            accumulate_smooth_direct(world, n, mesh_n, V, exponent, 1.0, use_dynamic, use_specular, depth),
            vec3<f32>(0.0), untinted,
        );
    } else {
        shade = accumulate_pixel_direct(
            world, n, mesh_n, spec_point, exponent, 1.0, false, use_dynamic, use_specular, depth,
        ).lit;
    }

    // Observation: the result, the point, or the texel the fragment shows.
    var face = 0.0;
    if !depth.hit_top {
        face = 1.0 + select(select(2.0, 3.0, depth.normal.z < 0.0), select(0.0, 1.0, depth.normal.x < 0.0), abs(depth.normal.x) > 0.5);
    }
    let texel = post_retro_texel_uv(depth.uv, vec2<f32>(LATTICE), ddx, ddy) * LATTICE;
    switch harness.modes.z {
        case 1u: { return vec4<f32>(spec_point.position, depth.height_m); }
        case 2u: { return vec4<f32>(texel, face, select(0.0, 1.0, depth.carved)); }
        case 3u, 4u, 5u, 6u, 7u: { return observed_material; }
        default: { return vec4<f32>(shade, 1.0); }
    }
}
"#;

fn shader_source() -> String {
    let consts = [
        (KINEMATIC_WGSL, "SHADOWMASK_META_VEC4S_PER_RECORD"),
        (SHADOW_SAMPLE_WGSL, "MOVER_RECEIVER_BIAS_SCALE"),
        (MATERIAL_SHADING_WGSL, "SPECULAR_POINT_AXES_EPSILON"),
        (MATERIAL_SHADING_WGSL, "SPECULAR_EXPONENT_MIN"),
        (MATERIAL_SHADING_WGSL, "SPECULAR_EXPONENT_MAX"),
        (MATERIAL_SHADING_WGSL, "SPECULAR_ROUGHNESS4_FLOOR"),
        (MATERIAL_SHADING_WGSL, "SPECULAR_SMOOTHNESS_MARKER"),
        (MATERIAL_SHADING_WGSL, "SPECULAR_TINT_MARKER"),
        (MATERIAL_SHADING_WGSL, "SPECULAR_TINT_SCALE"),
        (MATERIAL_SHADING_WGSL, "SPECULAR_HUE_EPSILON"),
    ]
    .map(|(source, name)| wgsl_const_line(source, name))
    .join("\n");
    let harness_consts = format!(
        "const LATTICE: f32 = {:?};\n\
         const UV_PER_PIXEL: f32 = {UV_PER_PIXEL:?};\n\
         const METRES_PER_UV: f32 = {METRES_PER_UV:?};\n\
         const SUBPIXEL_OFFSET: f32 = {SUBPIXEL_OFFSET:?};\n\
         const OUTPUT_FROZEN: u32 = {};\n\
         const OUTPUT_TEXEL_TERM: u32 = {};\n\
         const MATERIAL_NONE: u32 = {};\n\
         const MATERIAL_ORACLE: u32 = {};\n",
        f64::from(LATTICE),
        Output::Frozen as u32,
        Output::TexelTerm as u32,
        MaterialPath::None as u32,
        MaterialPath::Oracle as u32,
    );
    let items = [
        wgsl_item(KINEMATIC_WGSL, "struct GpuLight {"),
        wgsl_item(KINEMATIC_WGSL, "struct AnimationDescriptor {"),
        wgsl_item(KINEMATIC_WGSL, "struct KinematicLightParams {"),
        wgsl_item(KINEMATIC_WGSL, "struct MaterialUniform {"),
        wgsl_item(KINEMATIC_WGSL, "struct LightSpaceMatrices {"),
        wgsl_item(MATERIAL_SHADING_WGSL, "struct SpecularPoint {"),
        wgsl_item(MATERIAL_SHADING_WGSL, "struct SpecularTexel {"),
        wgsl_item(KINEMATIC_WGSL, "struct MoverPixelDirect {"),
        wgsl_item(KINEMATIC_WGSL, "struct MoverSmoothDirect {"),
        wgsl_item(KINEMATIC_WGSL, "struct MoverLightDrive {"),
        wgsl_function(KINEMATIC_WGSL, "mover_specular_point"),
        wgsl_function(KINEMATIC_WGSL, "mover_light_geometry"),
        wgsl_function(KINEMATIC_WGSL, "mover_texel_specular"),
        wgsl_function(KINEMATIC_WGSL, "mover_light_cache_layer"),
        wgsl_function(KINEMATIC_WGSL, "mover_light_drive"),
        wgsl_function(KINEMATIC_WGSL, "mover_light_shadow"),
        wgsl_function(KINEMATIC_WGSL, "mover_light_diffuse"),
        wgsl_function(KINEMATIC_WGSL, "mover_light_n_dot_l"),
        wgsl_function(KINEMATIC_WGSL, "accumulate_pixel_direct"),
        wgsl_function(KINEMATIC_WGSL, "accumulate_smooth_direct"),
        wgsl_function(KINEMATIC_WGSL, "mover_pixel_shade"),
        wgsl_function(KINEMATIC_WGSL, "mover_smooth_shade"),
        wgsl_function(MATERIAL_SHADING_WGSL, "specular_texel_point"),
        wgsl_function(MATERIAL_SHADING_WGSL, "post_retro_texel_uv"),
        wgsl_function(MATERIAL_SHADING_WGSL, "blinn_phong"),
        wgsl_function(MATERIAL_SHADING_WGSL, "specular_texel"),
        wgsl_function(MATERIAL_SHADING_WGSL, "specular_texel_exponent"),
        wgsl_function(MATERIAL_SHADING_WGSL, "specular_tint"),
        wgsl_function(KINEMATIC_WGSL, "sample_post_retro"),
    ]
    .join("\n\n");
    format!(
        "{consts}\n{harness_consts}\n{items}\n\n{SURFACE_DEPTH_WGSL}\n{LIGHT_FALLOFF_WGSL}\n\
         {LIGHT_EVAL_WGSL}\n{CURVE_EVAL_WGSL}\n{FROZEN_WGSL}\n{FROZEN_PRETINT_WGSL}\n{ENTRY}"
    )
}

/// A runtime light record as the mover loop reads it, with its influence
/// sphere. Every light is shadowed through the harness's stub samplers unless
/// `shadowed` is false.
#[derive(Clone, Copy, Debug)]
pub(super) struct MoverLight {
    pub kind: u32,
    pub position: [f32; 3],
    pub color: [f32; 3],
    pub aim: [f32; 3],
    pub range: f32,
    pub inner_radians: f32,
    pub outer_radians: f32,
    pub shadowed: bool,
    /// Above 1e30 the loop skips the influence test.
    pub influence: f32,
}

impl MoverLight {
    /// A white, linear-falloff, shadowed point light.
    pub fn point(position: [f32; 3], range: f32) -> Self {
        Self {
            kind: LIGHT_POINT,
            position,
            color: [1.0; 3],
            aim: [0.0; 3],
            range,
            inner_radians: 0.0,
            outer_radians: 0.0,
            shadowed: true,
            influence: f32::MAX,
        }
    }

    /// A white, linear-falloff, shadowed spot, its cone between the two
    /// half-angles.
    pub fn spot(position: [f32; 3], aim: [f32; 3], inner_degrees: f32, outer_degrees: f32) -> Self {
        Self {
            kind: LIGHT_SPOT,
            aim,
            inner_radians: inner_degrees.to_radians(),
            outer_radians: outer_degrees.to_radians(),
            ..Self::point(position, 10.0)
        }
    }

    /// A white directional light along `aim`.
    pub fn directional(aim: [f32; 3]) -> Self {
        Self {
            kind: LIGHT_DIRECTIONAL,
            aim,
            shadowed: false,
            ..Self::point([0.0, 10.0, 0.0], 0.0)
        }
    }

    pub fn is_spot(&self) -> bool {
        self.kind == LIGHT_SPOT
    }

    pub fn is_directional(&self) -> bool {
        self.kind == LIGHT_DIRECTIONAL
    }

    pub fn cos_outer(&self) -> f32 {
        self.outer_radians.cos()
    }

    fn bytes(self) -> Vec<u8> {
        let slot = |kind| {
            if self.shadowed && self.kind == kind {
                0
            } else {
                NO_SLOT
            }
        };
        [
            self.position[0],
            self.position[1],
            self.position[2],
            f32::from_bits(self.kind),
            self.color[0],
            self.color[1],
            self.color[2],
            // Linear falloff.
            f32::from_bits(0),
            self.aim[0],
            self.aim[1],
            self.aim[2],
            self.range,
            self.inner_radians,
            self.outer_radians,
            f32::from_bits(slot(LIGHT_SPOT)),
            f32::from_bits(slot(LIGHT_POINT)),
        ]
        .iter()
        .flat_map(|f| f.to_ne_bytes())
        .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Style {
    Pixel,
    Smooth,
}

/// What the fragment evaluates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Output {
    /// The style's loop: pixel's lit sum, or smooth's diffuse plus specular
    /// through its composition at unit albedo.
    Loop = 0,
    /// The frozen 1c7fc9e3a loop.
    Frozen = 1,
    /// `mover_texel_specular` for light 0 alone, as the loop calls it.
    TexelTerm = 2,
}

/// What the fragment writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Observe {
    /// The output's RGB.
    Shade = 0,
    /// The specular point, and the hit's height above the plane in `w`.
    Point = 1,
    /// The reconstructed albedo texel coordinate of the marched UV, the face
    /// (0 top, 1..=4 a side) and whether the march carved.
    Texel = 2,
    /// Material path: the decoded slot (exponent, intensity, tinted 0 or 1,
    /// tint).
    SpecularTexel = 3,
    /// Material path: the albedo, sampled after the loop.
    Albedo = 4,
    /// Material path: the loop's specular sum the composition colours: pixel's
    /// tinted sum, or smooth's one specular sum.
    RawTinted = 5,
    /// Material path: what the entry's composition adds for that sum alone.
    TintedOut = 6,
    /// Material path: the specular slot's filtered sample.
    Slot = 7,
}

/// Whether, and through which wiring, the fragment reads a real material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MaterialPath {
    /// The bare loop at the case's exponent and unit intensity (`Output`).
    None = 0,
    /// The style's beauty entry: decode, its loop, its composition.
    Current = 1,
    /// `fs_main` before tint: the slot's R, the prefix exponent, the
    /// raw-albedo multiply; smooth through the 1c7fc9e3a loop, pixel through
    /// the 74fcfd4b9 one.
    Oracle = 2,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct MoverCase {
    pub style: Style,
    pub output: Output,
    pub eye: [f32; 3],
    pub exponent: f32,
    pub march: bool,
    pub shadow_pattern: bool,
    /// Shade every face with this normal instead of the hit's own.
    pub fixed_normal: Option<[f32; 3]>,
    pub use_specular: bool,
    /// The descriptor-free dynamic prefix and the descriptor-bearing prefix
    /// (dynamic plus animated tail); records past it are selected-static.
    pub dynamic_light_count: u32,
    pub scripted_light_count: u32,
    pub uv_per_pixel: [f32; 2],
    pub material: MaterialPath,
    /// The oracle shades a `_p` slot at its decoded exponent.
    pub oracle_decodes: bool,
    /// The material uniform's 32 bytes, in place of the harness's own (whose
    /// shininess is `exponent`).
    pub material_uniform: Option<[u8; 32]>,
}

impl MoverCase {
    /// Pixel style, every light selected-static, the eye off the floor's low
    /// corner.
    pub fn new(style: Style, output: Output) -> Self {
        Self {
            style,
            output,
            eye: [-1.0, 3.0, -1.0],
            exponent: 8.0,
            march: false,
            shadow_pattern: false,
            fixed_normal: None,
            use_specular: true,
            dynamic_light_count: 0,
            scripted_light_count: 0,
            uv_per_pixel: [UV_PER_PIXEL as f32; 2],
            material: MaterialPath::None,
            oracle_decodes: false,
            material_uniform: None,
        }
    }

    /// The material path under `style`, every light selected-static.
    pub fn material(style: Style, path: MaterialPath) -> Self {
        Self {
            material: path,
            ..Self::new(style, Output::Loop)
        }
    }

    fn harness_bytes(self, observe: Observe) -> Vec<u8> {
        let smooth = u32::from(self.style == Style::Smooth);
        let normal = self.fixed_normal.unwrap_or([0.0; 3]);
        let floats = [
            self.eye[0],
            self.eye[1],
            self.eye[2],
            self.exponent,
            normal[0],
            normal[1],
            normal[2],
            if self.fixed_normal.is_some() {
                1.0
            } else {
                0.0
            },
        ];
        let words = [
            self.output as u32,
            u32::from(self.shadow_pattern),
            observe as u32,
            u32::from(self.use_specular),
            1,
            smooth,
            0,
            0,
        ];
        let mut out: Vec<u8> = floats.iter().flat_map(|f| f.to_ne_bytes()).collect();
        out.extend(words.iter().flat_map(|w| w.to_ne_bytes()));
        for f in [self.uv_per_pixel[0], self.uv_per_pixel[1], 0.0, 0.0] {
            out.extend(f.to_ne_bytes());
        }
        for w in [self.material as u32, u32::from(self.oracle_decodes), 0, 0] {
            out.extend(w.to_ne_bytes());
        }
        out
    }

    fn params_bytes(self, light_count: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(32);
        out.extend(light_count.to_ne_bytes());
        out.extend(0.3f32.to_ne_bytes());
        out.extend(0u32.to_ne_bytes());
        out.extend(0.0f32.to_ne_bytes());
        out.extend(self.dynamic_light_count.to_ne_bytes());
        out.extend(self.scripted_light_count.to_ne_bytes());
        out.extend(1.0f32.to_ne_bytes());
        out.extend(0u32.to_ne_bytes());
        out
    }

    fn material_bytes(self) -> Vec<u8> {
        if let Some(bytes) = self.material_uniform {
            return bytes.to_vec();
        }
        let (meters, march) = if self.march {
            (
                RELIEF_TEXELS,
                MAX_STEPS | HAS_DEPTH_BIT | (SHADOW_BUDGET << 16),
            )
        } else {
            (0.0, 0)
        };
        let mut out: Vec<u8> = [self.exponent, 0.0, RAISED, SUNK, meters, 1000.0, 0.0]
            .iter()
            .flat_map(|f| f.to_ne_bytes())
            .collect();
        out.extend(march.to_ne_bytes());
        out
    }
}

/// The checker's signed height fraction at texel (tx, ty).
pub(super) fn texel_height(tx: u32, ty: u32) -> f32 {
    if (tx + ty).is_multiple_of(2) {
        RAISED
    } else {
        SUNK
    }
}

/// The per-fragment shadow `harness_shadow` applies when the pattern is on.
pub(super) fn fragment_shadow(x: u32, y: u32) -> f32 {
    match (x + 2 * y) % 3 {
        0 => 1.0,
        1 => 0.5,
        _ => 0.0,
    }
}

/// A fragment's plane position, as the harness forms it.
pub(super) fn fragment_world(x: u32, y: u32) -> [f64; 3] {
    let axis =
        |pixel: u32| (f64::from(pixel) + 0.5 + SUBPIXEL_OFFSET) * UV_PER_PIXEL * METRES_PER_UV;
    [axis(x), 0.0, axis(y)]
}

/// The animated-tail descriptor the frozen comparison drives: an active,
/// looping brightness curve over `ANIM_SAMPLES`.
const ANIM_SAMPLES: [f32; 4] = [0.5, 1.0, 0.75, 0.25];

fn descriptor_bytes(active: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(48);
    out.extend(1.0f32.to_ne_bytes());
    out.extend(0.0f32.to_ne_bytes());
    out.extend(0u32.to_ne_bytes());
    out.extend(if active { 4u32 } else { 0 }.to_ne_bytes());
    for c in [1.0f32, 1.0, 1.0] {
        out.extend(c.to_ne_bytes());
    }
    out.extend(0u32.to_ne_bytes());
    out.extend(0u32.to_ne_bytes());
    out.extend(u32::from(active).to_ne_bytes());
    out.extend(0u32.to_ne_bytes());
    out.extend(0u32.to_ne_bytes());
    out
}

/// Render the magnified floor under `case` with `lights`; one RGBA per pixel,
/// row-major, holding what `observe` asks for.
pub(super) fn render(
    ctx: &GpuCtx,
    lights: &[MoverLight],
    case: MoverCase,
    observe: Observe,
) -> Vec<[f32; 4]> {
    let material = height_material(ctx);
    render_material(ctx, lights, case, observe, &material)
}

/// [`render`] over `material`'s albedo and specular slot.
pub(super) fn render_material(
    ctx: &GpuCtx,
    lights: &[MoverLight],
    case: MoverCase,
    observe: Observe,
    material: &MaterialTextures,
) -> Vec<[f32; 4]> {
    let device = &ctx.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("kinematic_specular_test shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source().into()),
    });
    let storage = |label: &str, contents: &[u8]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents,
            usage: wgpu::BufferUsages::STORAGE,
        })
    };
    let uniform = |label: &str, contents: &[u8]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents,
            usage: wgpu::BufferUsages::UNIFORM,
        })
    };
    let light_count = lights.len() as u32;
    let light_bytes: Vec<u8> = lights.iter().flat_map(|light| light.bytes()).collect();
    // One influence sphere per record, then two metadata rows per record past
    // the dynamic prefix; the stub samplers ignore the cache layer they name.
    let mut influence: Vec<u8> = lights
        .iter()
        .flat_map(|light| {
            [
                light.position[0],
                light.position[1],
                light.position[2],
                light.influence,
            ]
        })
        .flat_map(f32::to_ne_bytes)
        .collect();
    influence.resize(influence.len() + lights.len() * 2 * 16, 0);
    let descriptors: Vec<u8> = (0..light_count.max(1))
        .flat_map(|i| {
            descriptor_bytes(i >= case.dynamic_light_count && i < case.scripted_light_count)
        })
        .collect();
    let samples: Vec<u8> = ANIM_SAMPLES.iter().flat_map(|f| f.to_ne_bytes()).collect();
    let buffers = [
        storage("lights", &light_bytes),
        storage("influence", &influence),
        storage("descriptors", &descriptors),
        storage("anim samples", &samples),
        uniform("params", &case.params_bytes(light_count)),
        uniform("light space", &vec![0u8; 96 * 64]),
    ];
    let material_uniform = uniform("material", &case.material_bytes());
    let harness = uniform("harness", &case.harness_bytes(observe));

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("kinematic_specular_test pipeline"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba32Float,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    let mut entries: Vec<wgpu::BindGroupEntry> = buffers
        .iter()
        .enumerate()
        .map(|(binding, buffer)| wgpu::BindGroupEntry {
            binding: binding as u32,
            resource: buffer.as_entire_binding(),
        })
        .collect();
    entries.extend([
        wgpu::BindGroupEntry {
            binding: 6,
            resource: wgpu::BindingResource::TextureView(&material.specular),
        },
        wgpu::BindGroupEntry {
            binding: 7,
            resource: material_uniform.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: 8,
            resource: harness.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: 9,
            resource: wgpu::BindingResource::TextureView(&material.base),
        },
        wgpu::BindGroupEntry {
            binding: 10,
            resource: wgpu::BindingResource::Sampler(&material.sampler),
        },
    ]);
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("kinematic_specular_test bind group"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries,
    });

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("kinematic_specular_test target"),
        size: wgpu::Extent3d {
            width: TARGET,
            height: TARGET,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    let row_bytes = (TARGET * 16).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("kinematic_specular_test readback"),
        size: u64::from(row_bytes * TARGET),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("kinematic_specular_test pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: -1.0,
                        g: -1.0,
                        b: -1.0,
                        a: -1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..4, 0..1);
    }
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(TARGET),
            },
        },
        wgpu::Extent3d {
            width: TARGET,
            height: TARGET,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| {
        result.expect("map kinematic_specular_test readback");
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll kinematic_specular_test device");
    let bytes = slice
        .get_mapped_range()
        .expect("buffer mapped for readback");
    let mut pixels = Vec::with_capacity((TARGET * TARGET) as usize);
    for row in bytes.chunks(row_bytes as usize) {
        for texel in row[..(TARGET * 16) as usize].as_chunks::<16>().0 {
            pixels.push(std::array::from_fn(|c| {
                f32::from_ne_bytes(texel[c * 4..c * 4 + 4].try_into().unwrap())
            }));
        }
    }
    drop(bytes);
    readback.unmap();
    pixels
}

/// The height checker as the bake stores it: `Rg8Unorm`, R intensity, G the
/// inverted height byte; under a white albedo.
fn height_material(ctx: &GpuCtx) -> MaterialTextures {
    white_material(ctx, LATTICE, SlotFormat::Rg8, &|tx, ty| {
        [255, checker_height_byte(tx, ty), 0, 255]
    })
}

/// The stored G byte of the checker's texel (tx, ty): `255 - authored`.
pub(super) fn checker_height_byte(tx: u32, ty: u32) -> u8 {
    255 - if texel_height(tx, ty) == RAISED {
        HEIGHT_RAISED
    } else {
        HEIGHT_SUNK
    }
}
