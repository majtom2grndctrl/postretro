// GPU readback harness for forward.wgsl's world static specular: the owner and
// SDF sums (`world_static_specular`) and the pixel-style point that feeds them
// (`specular_texel_point`), beside a frozen copy of the 1c7fc9e3a function. Its
// material harness reads a real albedo and specular slot and shades sharpness
// and tint as forward `fs_main` does.
// See: context/lib/rendering_pipeline.md §4 (Static specular ownership,
// Texel-centre highlight)
//
// Intentional exception to testing_guide.md §3 "No GPU context in tests": the
// functions are WGSL, so verifying them means running them. Every production
// function is spliced in by name from its shader; only the entry points are
// harness code. The harness self-skips without an adapter and says so; a
// skipped run proves nothing. Set `POSTRETRO_REQUIRE_GPU` to any non-empty
// value other than `0` to make a missing adapter fail instead.

use super::gpu_test_harness::{GpuCtx, wgsl_const_line, wgsl_function, wgsl_item};
use super::specular_slot_fixtures::MaterialTextures;
use postretro_render_cpu::frame_uniforms::{
    SPEC_SHADOWMASK_FLAG_FORCE_ONE, SPEC_SHADOWMASK_FLAG_STYLE_SMOOTH,
};

pub(super) const FORWARD_WGSL: &str = include_str!("../shaders/forward.wgsl");
pub(super) const MATERIAL_SHADING_WGSL: &str = include_str!("../shaders/material_shading.wgsl");
const LIGHT_FALLOFF_WGSL: &str = include_str!("../shaders/light_falloff.wgsl");
const SDF_LIGHT_SELECT_WGSL: &str = include_str!("../shaders/sdf_light_select.wgsl");
const FROZEN_WGSL: &str = include_str!("world_specular_frozen.wgsl");

const TEXEL_BYTES: u32 = 16;

/// The one-row owner target's width.
pub(super) const WIDTH: u32 = 16;
pub(super) const EMPTY: u16 = 0xFFFF;
pub(super) const NO_OWNERS: [u16; 4] = [EMPTY; 4];

/// The texel target: `LATTICE`² albedo texels magnified onto
/// `TEXEL_TARGET`² pixels, `PIXELS_PER_TEXEL` to a side.
pub(super) const TEXEL_TARGET: u32 = 64;
pub(super) const LATTICE: u32 = 8;
pub(super) const PIXELS_PER_TEXEL: u32 = TEXEL_TARGET / LATTICE;
/// UV per pixel, so the floor spans UV [0, 1).
const UV_PER_PIXEL: f64 = 1.0 / TEXEL_TARGET as f64;
/// The floor's world size per UV unit, along x and z.
const METRES_PER_UV: f64 = 4.0;
/// Every fragment's UV sits this many pixels past its pixel centre, so the
/// column just left of each texel edge lands inside the seam band. Every
/// mapping is dyadic, so the GPU forms each fragment's point exactly and the
/// fragments of one texel agree to the bit.
const SUBPIXEL_OFFSET: f64 = 0.25;

const SPEC_LIGHT_TYPE_SPOT: f32 = 1.0;
const SPEC_LIGHT_TYPE_DIRECTIONAL: f32 = 2.0;

const ENTRY_VERTEX: &str = r#"
struct HarnessVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) owners: vec4<u32>,
};

@vertex
fn vs_main(@location(0) ndc: vec2<f32>, @location(1) owners: vec4<u32>) -> HarnessVaryings {
    var out: HarnessVaryings;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.owners = owners;
    return out;
}
"#;

/// The shadowing inputs both harnesses share. `spec_shadowmask_flags` keeps
/// forward's `Uniforms` name: the spliced flag readers resolve it lexically.
const CASE_SHADOWING_FIELDS: &str = r#"
    mask: vec4<f32>,
    sdf_factor: vec4<f32>,
    sdf_indices: vec4<u32>,
    sdf_count: u32,
    owners_readable: u32,
    spec_shadowmask_flags: u32,
    sdf_force_lit: u32,
"#;

const PRELUDE_TAIL: &str = r#"
@group(0) @binding(0) var<uniform> uniforms: Case;
@group(0) @binding(1) var<storage, read> spec_lights: array<SpecLight>;

struct SpecLight {
    position_and_range: vec4<f32>,
    color_and_pad:      vec4<f32>,
    cone_dir_and_type:  vec4<f32>,
    cone_cos:           vec4<f32>,
};

fn harness_sdf_selection() -> SdfLightSelection {
    var sel: SdfLightSelection;
    sel.indices = array<u32, 4>(
        uniforms.sdf_indices.x,
        uniforms.sdf_indices.y,
        uniforms.sdf_indices.z,
        uniforms.sdf_indices.w,
    );
    sel.count = uniforms.sdf_count;
    return sel;
}
"#;

/// The production functions world static specular runs, by name.
fn production_specular_source() -> String {
    let consts = [
        (FORWARD_WGSL, "SPEC_OWNER_EMPTY"),
        (FORWARD_WGSL, "SPEC_OWNER_CHANNELS"),
        (FORWARD_WGSL, "SPEC_LIGHT_TYPE_DIRECTIONAL"),
        (FORWARD_WGSL, "SPEC_SHADOWMASK_FLAG_FORCE_ONE"),
        (FORWARD_WGSL, "SPEC_SHADOWMASK_FLAG_STYLE_SMOOTH"),
        (MATERIAL_SHADING_WGSL, "SPECULAR_POINT_AXES_EPSILON"),
    ]
    .map(|(source, name)| wgsl_const_line(source, name))
    .join("\n");
    let helpers = [
        wgsl_item(SDF_LIGHT_SELECT_WGSL, "struct SdfLightSelection {"),
        wgsl_item(MATERIAL_SHADING_WGSL, "struct SpecularPoint {"),
        wgsl_function(MATERIAL_SHADING_WGSL, "specular_texel_point"),
        wgsl_function(MATERIAL_SHADING_WGSL, "post_retro_texel_uv"),
        wgsl_function(FORWARD_WGSL, "specular_style_smooth"),
        wgsl_function(FORWARD_WGSL, "world_static_specular"),
        wgsl_function(FORWARD_WGSL, "world_static_light_specular"),
        wgsl_function(FORWARD_WGSL, "positional_cone_aim"),
        wgsl_function(FORWARD_WGSL, "spec_shadowmask_forced_one"),
        wgsl_function(FORWARD_WGSL, "shadowmask_channel_value"),
        wgsl_function(FORWARD_WGSL, "slice_for_visibility"),
        wgsl_function(FORWARD_WGSL, "cone_attenuation_cos"),
        wgsl_function(MATERIAL_SHADING_WGSL, "blinn_phong"),
        wgsl_function(LIGHT_FALLOFF_WGSL, "light_eval_falloff"),
    ]
    .join("\n\n");
    format!("{consts}\n{helpers}\n")
}

/// The one-row owner harness: column c sits at world x = (c + 0.5 - 8) * 0.25
/// on a floor facing +Y, shaded per pixel with V straight up.
fn owner_shader_source() -> String {
    let entry = r#"
@fragment
fn fs_main(in: HarnessVaryings) -> @location(0) vec4<f32> {
    let world = vec3<f32>((in.position.x - 8.0) * 0.25, 0.0, 0.0);
    let up = vec3<f32>(0.0, 1.0, 0.0);
    let specular = world_static_specular(
        world,
        up,
        up,
        8.0,
        1.0,
        in.owners,
        uniforms.mask,
        uniforms.owners_readable != 0u,
        harness_sdf_selection(),
        uniforms.sdf_factor,
        uniforms.sdf_force_lit != 0u,
    );
    return vec4<f32>(specular, 1.0);
}
"#;
    format!(
        "struct Case {{{CASE_SHADOWING_FIELDS}}};\n{PRELUDE_TAIL}\n{}\n{ENTRY_VERTEX}\n{entry}",
        production_specular_source()
    )
}

/// The texel harness: a magnified floor facing +Y whose fragments carry UV,
/// a texel grid and derivatives, through forward `fs_main`'s wiring of the
/// point builder, or through the frozen 1c7fc9e3a function.
fn texel_shader_source() -> String {
    let fields = r#"
    // xyz eye, w specular exponent.
    eye_and_exponent: vec4<f32>,
    // xy march shift in texels; z texels added per column mod 3.
    march: vec4<f32>,
    // x geometry, y per-fragment visibility on, z frozen path.
    modes: vec4<u32>,
"#;
    let consts = format!(
        "const LATTICE: f32 = {:?};\n\
         const UV_PER_PIXEL: f32 = {UV_PER_PIXEL:?};\n\
         const METRES_PER_UV: f32 = {METRES_PER_UV:?};\n\
         const SUBPIXEL_OFFSET: f32 = {SUBPIXEL_OFFSET:?};\n\
         const GEOMETRY_EDGE_ON: u32 = {};\n\
         const GEOMETRY_COLLAPSED_UV: u32 = {};\n",
        f64::from(LATTICE),
        Geometry::EdgeOn as u32,
        Geometry::CollapsedUv as u32,
    );
    let entry = r#"
// 1, 0.5 or 0 by (x + 2y) mod 3; `fragment_visibility` mirrors it.
fn harness_fragment_visibility(pixel: vec2<f32>) -> f32 {
    let k = (u32(pixel.x) + 2u * u32(pixel.y)) % 3u;
    return select(select(0.0, 0.5, k == 1u), 1.0, k == 0u);
}

@fragment
fn fs_main(in: HarnessVaryings) -> @location(0) vec4<f32> {
    let plane_uv = (in.position.xy + vec2<f32>(SUBPIXEL_OFFSET)) * UV_PER_PIXEL;
    let world = vec3<f32>(plane_uv.x, 0.0, plane_uv.y) * METRES_PER_UV;
    var uv = plane_uv;
    if uniforms.modes.x == GEOMETRY_EDGE_ON {
        // The face projects to a line: both UV derivatives align.
        uv = vec2<f32>(plane_uv.x + plane_uv.y);
    } else if uniforms.modes.x == GEOMETRY_COLLAPSED_UV {
        uv = vec2<f32>(0.3, 0.7);
    }
    let ddx = dpdx(uv);
    let ddy = dpdy(uv);
    let ddx_world = dpdx(world);
    let ddy_world = dpdy(world);
    // A march moves the shading UV off the fragment's own, by a different
    // amount per column as a parallax march does.
    let march = uniforms.march.xy
        + vec2<f32>(f32(u32(in.position.x) % 3u) * uniforms.march.z, 0.0);
    let shade_uv = uv + march / LATTICE;

    var mask = uniforms.mask;
    var sdf_factor = uniforms.sdf_factor;
    if uniforms.modes.y != 0u {
        let visibility = harness_fragment_visibility(in.position.xy);
        mask = mask * visibility;
        sdf_factor = sdf_factor * visibility;
    }
    let up = vec3<f32>(0.0, 1.0, 0.0);
    let eye = uniforms.eye_and_exponent.xyz;
    let exponent = uniforms.eye_and_exponent.w;
    let owners_readable = uniforms.owners_readable != 0u;
    let sdf_force_lit = uniforms.sdf_force_lit != 0u;
    var specular: vec3<f32>;
    if uniforms.modes.z != 0u {
        specular = frozen_fs_main_static_specular(
            world,
            eye,
            up,
            exponent,
            1.0,
            in.owners,
            mask,
            owners_readable,
            harness_sdf_selection(),
            sdf_factor,
            sdf_force_lit,
        );
    } else {
        // forward fs_main's wiring.
        let view_vector = eye - world;
        let V = normalize(view_vector);
        let spec_point = specular_texel_point(
            specular_style_smooth(),
            SpecularPoint(world, V),
            uv,
            shade_uv,
            world,
            eye,
            vec2<f32>(LATTICE),
            ddx,
            ddy,
            ddx_world,
            ddy_world,
        );
        specular = world_static_specular(
            spec_point.position,
            up,
            spec_point.view,
            exponent,
            1.0,
            in.owners,
            mask,
            owners_readable,
            harness_sdf_selection(),
            sdf_factor,
            sdf_force_lit,
        );
    }
    return vec4<f32>(specular, 1.0);
}
"#;
    format!(
        "struct Case {{{CASE_SHADOWING_FIELDS}{fields}}};\n{PRELUDE_TAIL}\n{consts}\n{}\n{FROZEN_WGSL}\n{ENTRY_VERTEX}\n{entry}",
        production_specular_source()
    )
}

/// A spec light as `pack_spec_lights` lays it out.
#[derive(Clone, Copy)]
pub(super) struct Light {
    pub position: [f32; 3],
    pub range: f32,
    pub color: [f32; 3],
    pub sdf: bool,
    pub aim: [f32; 3],
    pub kind: f32,
    pub cos_inner: f32,
    pub cos_outer: f32,
}

impl Light {
    /// A linear-falloff point light 2 m above the edge between the two faces
    /// of the owner target.
    pub fn point(color: [f32; 3]) -> Self {
        Self {
            position: [0.0, 2.0, 0.0],
            range: 10.0,
            color,
            sdf: false,
            aim: [0.0; 3],
            kind: 0.0,
            cos_inner: 1.0,
            cos_outer: -1.0,
        }
    }

    pub fn at(self, position: [f32; 3], range: f32) -> Self {
        Self {
            position,
            range,
            ..self
        }
    }

    pub fn directional(origin: [f32; 3], aim: [f32; 3]) -> Self {
        Self {
            position: origin,
            range: 0.0,
            color: [1.0, 1.0, 1.0],
            sdf: false,
            aim,
            kind: SPEC_LIGHT_TYPE_DIRECTIONAL,
            cos_inner: 1.0,
            cos_outer: -1.0,
        }
    }

    /// A white linear-falloff spot, its cone between the two half-angles.
    pub fn spot(position: [f32; 3], aim: [f32; 3], inner_degrees: f32, outer_degrees: f32) -> Self {
        Self {
            position,
            range: 10.0,
            color: [1.0, 1.0, 1.0],
            sdf: false,
            aim,
            kind: SPEC_LIGHT_TYPE_SPOT,
            cos_inner: inner_degrees.to_radians().cos(),
            cos_outer: outer_degrees.to_radians().cos(),
        }
    }

    fn bytes(self) -> Vec<u8> {
        let floats = [
            self.position[0],
            self.position[1],
            self.position[2],
            self.range,
            self.color[0],
            self.color[1],
            self.color[2],
            if self.sdf { 1.0 } else { 0.0 },
            self.aim[0],
            self.aim[1],
            self.aim[2],
            self.kind,
            self.cos_inner,
            self.cos_outer,
            0.0,
            // Linear falloff.
            0.0,
        ];
        floats.iter().flat_map(|f| f.to_ne_bytes()).collect()
    }
}

/// Columns `[x0, x1)` of the target, all of one face's owners.
pub(super) struct Face {
    pub columns: (u32, u32),
    pub owners: [u16; 4],
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Case {
    pub mask: [f32; 4],
    pub sdf_factor: [f32; 4],
    pub sdf_indices: [u32; 4],
    pub sdf_count: u32,
    pub owners_readable: bool,
    pub force_one: bool,
    pub sdf_force_lit: bool,
}

impl Default for Case {
    fn default() -> Self {
        Self {
            mask: [1.0; 4],
            sdf_factor: [1.0; 4],
            sdf_indices: [u32::MAX; 4],
            sdf_count: 0,
            owners_readable: true,
            force_one: false,
            sdf_force_lit: false,
        }
    }
}

impl Case {
    /// `CASE_SHADOWING_FIELDS`, with `style_flags` joining the force-one bit.
    fn bytes(self, style_flags: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        for f in self.mask.iter().chain(&self.sdf_factor) {
            out.extend_from_slice(&f.to_ne_bytes());
        }
        let force_one = if self.force_one {
            SPEC_SHADOWMASK_FLAG_FORCE_ONE
        } else {
            0
        };
        let words = [
            self.sdf_indices[0],
            self.sdf_indices[1],
            self.sdf_indices[2],
            self.sdf_indices[3],
            self.sdf_count,
            u32::from(self.owners_readable),
            force_one | style_flags,
            u32::from(self.sdf_force_lit),
        ];
        for word in words {
            out.extend_from_slice(&word.to_ne_bytes());
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Style {
    Pixel,
    Smooth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Geometry {
    /// UV and world both follow the pixel grid.
    Plane = 0,
    /// Both UV derivatives align.
    EdgeOn = 1,
    /// One UV for every fragment.
    CollapsedUv = 2,
}

/// A march moving each fragment's shading UV `shift` texels, plus `stagger`
/// texels per column mod 3 along u.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct March {
    pub shift: [f32; 2],
    pub stagger: f32,
}

impl March {
    pub const OFF: Self = Self {
        shift: [0.0, 0.0],
        stagger: 0.0,
    };
    /// Lands most fragments a whole texel and up to half a texel along u
    /// from their own.
    pub const ON: Self = Self {
        shift: [1.0, 0.0],
        stagger: 0.25,
    };
}

#[derive(Clone, Copy)]
pub(super) struct TexelCase {
    pub shadowing: Case,
    pub style: Style,
    pub eye: [f32; 3],
    pub exponent: f32,
    pub march: March,
    pub geometry: Geometry,
    pub per_fragment_visibility: bool,
    /// Through the frozen 1c7fc9e3a function instead of today's wiring.
    pub frozen: bool,
}

impl TexelCase {
    pub fn new(style: Style) -> Self {
        Self {
            shadowing: Case::default(),
            style,
            eye: [-1.0, 3.0, -1.0],
            exponent: 8.0,
            march: March::OFF,
            geometry: Geometry::Plane,
            per_fragment_visibility: false,
            frozen: false,
        }
    }

    fn bytes(self) -> Vec<u8> {
        let style = match self.style {
            Style::Pixel => 0,
            Style::Smooth => SPEC_SHADOWMASK_FLAG_STYLE_SMOOTH,
        };
        let mut out = self.shadowing.bytes(style);
        let floats = [
            self.eye[0],
            self.eye[1],
            self.eye[2],
            self.exponent,
            self.march.shift[0],
            self.march.shift[1],
            self.march.stagger,
            0.0,
        ];
        for f in floats {
            out.extend_from_slice(&f.to_ne_bytes());
        }
        let modes = [
            self.geometry as u32,
            u32::from(self.per_fragment_visibility),
            u32::from(self.frozen),
            0,
        ];
        for word in modes {
            out.extend_from_slice(&word.to_ne_bytes());
        }
        out
    }
}

/// The per-fragment visibility `harness_fragment_visibility` applies.
pub(super) fn fragment_visibility(x: u32, y: u32) -> f32 {
    match (x + 2 * y) % 3 {
        0 => 1.0,
        1 => 0.5,
        _ => 0.0,
    }
}

/// The reconstructed albedo texel coordinate `post_retro_texel_uv` gives a
/// plane fragment's shading UV, in texels, ×64 so every value is an integer:
/// `(64·(seam + aa))` per axis. Fragments sharing a key share a point.
pub(super) fn reconstructed_texel_key(x: u32, y: u32, march: March) -> (i64, i64) {
    let seam_width = UV_PER_PIXEL * f64::from(LATTICE);
    let axis = |pixel: u32, shift: f64| {
        let uv = (f64::from(pixel) + 0.5 + SUBPIXEL_OFFSET) * UV_PER_PIXEL;
        let texel = uv * f64::from(LATTICE) + shift;
        let seam = (texel + 0.5).floor();
        let aa = ((texel - seam) / seam_width).clamp(-0.5, 0.5);
        ((seam + aa) * 64.0).round() as i64
    };
    let stagger = f64::from(x % 3) * f64::from(march.stagger);
    (
        axis(x, f64::from(march.shift[0]) + stagger),
        axis(y, f64::from(march.shift[1])),
    )
}

/// Whether a key is a texel centre (outside the seam band) along each axis.
pub(super) fn key_is_centre(key: i64) -> bool {
    key.rem_euclid(64) == 32
}

/// Render `faces` with `lights` under `case`; one RGB specular sum per column.
pub(super) fn render(ctx: &GpuCtx, lights: &[Light], faces: &[Face], case: Case) -> Vec<[f32; 3]> {
    draw(
        ctx,
        &owner_shader_source(),
        (WIDTH, 1),
        &case.bytes(0),
        lights,
        faces,
    )
}

/// Render the texel target under `case`, one face owned by `owners`; one RGB
/// specular sum per pixel, row-major.
pub(super) fn render_texels(
    ctx: &GpuCtx,
    lights: &[Light],
    owners: [u16; 4],
    case: TexelCase,
) -> Vec<[f32; 3]> {
    draw(
        ctx,
        &texel_shader_source(),
        (TEXEL_TARGET, TEXEL_TARGET),
        &case.bytes(),
        lights,
        &[Face {
            columns: (0, TEXEL_TARGET),
            owners,
        }],
    )
}

// --- Material harness: the slot's sharpness and tint through forward fs_main ---

/// The production functions forward `fs_main` reads its material slots and
/// shades sharpness and tint with, by name.
fn production_material_source() -> String {
    let consts = [
        "SPECULAR_EXPONENT_MIN",
        "SPECULAR_EXPONENT_MAX",
        "SPECULAR_ROUGHNESS4_FLOOR",
        "SPECULAR_SMOOTHNESS_MARKER",
        "SPECULAR_TINT_MARKER",
        "SPECULAR_TINT_SCALE",
        "SPECULAR_HUE_EPSILON",
    ]
    .map(|name| wgsl_const_line(MATERIAL_SHADING_WGSL, name))
    .join("\n");
    let items = [
        wgsl_item(MATERIAL_SHADING_WGSL, "struct SpecularTexel {"),
        wgsl_function(MATERIAL_SHADING_WGSL, "specular_texel"),
        wgsl_function(MATERIAL_SHADING_WGSL, "specular_texel_exponent"),
        wgsl_function(MATERIAL_SHADING_WGSL, "specular_tint"),
        wgsl_function(FORWARD_WGSL, "sample_post_retro"),
        wgsl_function(FORWARD_WGSL, "sample_color"),
    ]
    .join("\n\n");
    format!("{consts}\n{items}\n")
}

/// The material harness: a floor facing +Y whose fragments sample a real
/// albedo and specular slot, then shade them through forward `fs_main`'s
/// wiring, or through the wiring before tint (the oracle).
fn material_shader_source() -> String {
    let fields = r#"
    // xyz eye, w the prefix exponent (`max(material.shininess, 1.0)`).
    eye_and_exponent: vec4<f32>,
    // xy UV per pixel along x and y.
    uv_per_pixel: vec4<f32>,
    // x path, y the oracle takes the decoded exponent, z observe,
    // w use_specular.
    material_modes: vec4<u32>,
"#;
    let consts = format!(
        "const METRES_PER_UV: f32 = {METRES_PER_UV:?};\n\
         const SUBPIXEL_OFFSET: f32 = {SUBPIXEL_OFFSET:?};\n\
         const PATH_ORACLE: u32 = {};\n\
         const OBSERVE_TEXEL: u32 = {};\n\
         const OBSERVE_ALBEDO: u32 = {};\n\
         const OBSERVE_RAW_SPECULAR: u32 = {};\n\
         const OBSERVE_SPECULAR_OUT: u32 = {};\n\
         const OBSERVE_SLOT: u32 = {};\n",
        MaterialPath::Oracle as u32,
        MaterialObserve::Texel as u32,
        MaterialObserve::Albedo as u32,
        MaterialObserve::RawSpecular as u32,
        MaterialObserve::SpecularOut as u32,
        MaterialObserve::Slot as u32,
    );
    let entry = r#"
@group(0) @binding(2) var base_texture: texture_2d<f32>;
@group(0) @binding(3) var spec_texture: texture_2d<f32>;
@group(0) @binding(4) var aniso_sampler: sampler;

// Stand-ins for the lighting fs_main sums around specular: ambient, indirect
// and static direct before it, the dynamic loop after it, and the glow.
fn harness_light_before_specular(pixel: vec2<f32>) -> vec3<f32> {
    return vec3<f32>(0.21, 0.17, 0.13) + vec3<f32>(f32(u32(pixel.x) % 7u) * 0.013);
}
const HARNESS_LIGHT_AFTER_SPECULAR: vec3<f32> = vec3<f32>(0.05, 0.07, 0.03);
const HARNESS_EMISSIVE: vec3<f32> = vec3<f32>(0.02, 0.0, 0.04);

@fragment
fn fs_main(in: HarnessVaryings) -> @location(0) vec4<f32> {
    let uv = (in.position.xy + vec2<f32>(SUBPIXEL_OFFSET)) * uniforms.uv_per_pixel.xy;
    let world = vec3<f32>(uv.x, 0.0, uv.y) * METRES_PER_UV;
    let ddx = dpdx(uv);
    let ddy = dpdy(uv);
    let ddx_world = dpdx(world);
    let ddy_world = dpdy(world);
    let up = vec3<f32>(0.0, 1.0, 0.0);
    let eye = uniforms.eye_and_exponent.xyz;
    let prefix_exponent = uniforms.eye_and_exponent.w;
    let V = normalize(eye - world);
    let owners_readable = uniforms.owners_readable != 0u;
    let sdf_force_lit = uniforms.sdf_force_lit != 0u;
    let use_specular = uniforms.material_modes.w != 0u;
    let base_color = sample_color(base_texture, uv, ddx, ddy);
    let slot = sample_color(spec_texture, uv, ddx, ddy);

    var total_light = harness_light_before_specular(in.position.xy);
    if uniforms.material_modes.x == PATH_ORACLE {
        // fs_main before tint: R intensity, the prefix exponent (a `_p`
        // slot's decoded one when asked), the raw-albedo multiply.
        let exponent = select(
            prefix_exponent,
            specular_texel_exponent(slot.b, prefix_exponent),
            uniforms.material_modes.y != 0u,
        );
        var specular = vec3<f32>(0.0);
        if use_specular {
            if specular_style_smooth() {
                specular = frozen_fs_main_static_specular(
                    world, eye, up, exponent, slot.r, in.owners, uniforms.mask,
                    owners_readable, harness_sdf_selection(), uniforms.sdf_factor, sdf_force_lit,
                );
            } else {
                let point = specular_texel_point(
                    false, SpecularPoint(world, V), uv, uv, world, eye,
                    vec2<f32>(textureDimensions(base_texture, 0)), ddx, ddy, ddx_world, ddy_world,
                );
                specular = world_static_specular(
                    point.position, up, point.view, exponent, slot.r, in.owners, uniforms.mask,
                    owners_readable, harness_sdf_selection(), uniforms.sdf_factor, sdf_force_lit,
                );
            }
        }
        total_light = total_light + specular;
        total_light = total_light + HARNESS_LIGHT_AFTER_SPECULAR;
        let rgb = base_color.rgb * total_light + HARNESS_EMISSIVE;
        return vec4<f32>(rgb, 1.0);
    }

    // forward fs_main's wiring.
    var specular_sum = vec3<f32>(0.0);
    var tinted_specular = vec3<f32>(0.0);
    var raw_specular = vec3<f32>(0.0);
    var observed_texel = vec4<f32>(0.0);
    if use_specular {
        let spec_texel = specular_texel(slot, prefix_exponent);
        observed_texel = vec4<f32>(
            spec_texel.exponent,
            spec_texel.intensity,
            select(0.0, 1.0, spec_texel.tinted),
            spec_texel.tint,
        );
        let spec_point = specular_texel_point(
            specular_style_smooth(), SpecularPoint(world, V), uv, uv, world, eye,
            vec2<f32>(textureDimensions(base_texture, 0)), ddx, ddy, ddx_world, ddy_world,
        );
        specular_sum = world_static_specular(
            spec_point.position, up, spec_point.view, spec_texel.exponent, spec_texel.intensity,
            in.owners, uniforms.mask, owners_readable, harness_sdf_selection(),
            uniforms.sdf_factor, sdf_force_lit,
        );
        raw_specular = specular_sum;
        if spec_texel.tinted {
            tinted_specular = specular_sum * specular_tint(base_color.rgb, spec_texel.tint);
            specular_sum = vec3<f32>(0.0);
        }
    }
    total_light = total_light + specular_sum;
    total_light = total_light + HARNESS_LIGHT_AFTER_SPECULAR;
    let rgb = base_color.rgb * total_light + HARNESS_EMISSIVE + tinted_specular;
    let observe = uniforms.material_modes.z;
    if observe == OBSERVE_TEXEL {
        return observed_texel;
    } else if observe == OBSERVE_ALBEDO {
        return base_color;
    } else if observe == OBSERVE_RAW_SPECULAR {
        return vec4<f32>(raw_specular, 1.0);
    } else if observe == OBSERVE_SPECULAR_OUT {
        return vec4<f32>(base_color.rgb * specular_sum + tinted_specular, 1.0);
    } else if observe == OBSERVE_SLOT {
        return slot;
    }
    return vec4<f32>(rgb, 1.0);
}
"#;
    format!(
        "struct Case {{{CASE_SHADOWING_FIELDS}{fields}}};\n{PRELUDE_TAIL}\n{consts}\n{}\n{}\n{FROZEN_WGSL}\n{ENTRY_VERTEX}\n{entry}",
        production_specular_source(),
        production_material_source(),
    )
}

/// Which wiring the material harness shades through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MaterialPath {
    /// Forward `fs_main` today: decode, tint split.
    Current = 0,
    /// `fs_main` before tint: the slot's R, the prefix exponent and the
    /// raw-albedo multiply; smooth through the frozen 1c7fc9e3a function.
    Oracle = 1,
}

/// What the material harness writes per pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MaterialObserve {
    /// The shaded colour (RGB, alpha 1).
    Shade = 0,
    /// The decoded slot: exponent, intensity, tinted (0 or 1), tint.
    Texel = 1,
    /// The sampled albedo.
    Albedo = 2,
    /// The specular sum before the tint split.
    RawSpecular = 3,
    /// What specular adds to the shaded colour: the albedo times an untinted
    /// sum, or the tinted highlight.
    SpecularOut = 4,
    /// The specular slot's filtered sample.
    Slot = 5,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct MaterialCase {
    pub shadowing: Case,
    pub style: Style,
    pub eye: [f32; 3],
    pub prefix_exponent: f32,
    pub uv_per_pixel: [f32; 2],
    pub use_specular: bool,
    pub path: MaterialPath,
    /// The oracle shades a `_p` slot at its decoded exponent.
    pub oracle_decodes: bool,
}

impl MaterialCase {
    /// `LATTICE`² texels magnified onto the target, as the texel harness.
    pub fn new(style: Style) -> Self {
        Self {
            shadowing: Case::default(),
            style,
            eye: [-1.0, 3.0, -1.0],
            prefix_exponent: 8.0,
            uv_per_pixel: [UV_PER_PIXEL as f32; 2],
            use_specular: true,
            path: MaterialPath::Current,
            oracle_decodes: false,
        }
    }

    fn bytes(self, observe: MaterialObserve) -> Vec<u8> {
        let style = match self.style {
            Style::Pixel => 0,
            Style::Smooth => SPEC_SHADOWMASK_FLAG_STYLE_SMOOTH,
        };
        let mut out = self.shadowing.bytes(style);
        let floats = [
            self.eye[0],
            self.eye[1],
            self.eye[2],
            self.prefix_exponent,
            self.uv_per_pixel[0],
            self.uv_per_pixel[1],
            0.0,
            0.0,
        ];
        for f in floats {
            out.extend_from_slice(&f.to_ne_bytes());
        }
        let modes = [
            self.path as u32,
            u32::from(self.oracle_decodes),
            observe as u32,
            u32::from(self.use_specular),
        ];
        for word in modes {
            out.extend_from_slice(&word.to_ne_bytes());
        }
        out
    }
}

/// Render the material harness over the texel target, one face owned by
/// `owners`, sampling `material`; one RGBA per pixel, row-major.
pub(super) fn render_material(
    ctx: &GpuCtx,
    lights: &[Light],
    owners: [u16; 4],
    material: &MaterialTextures,
    case: MaterialCase,
    observe: MaterialObserve,
) -> Vec<[f32; 4]> {
    draw_rgba(
        ctx,
        &material_shader_source(),
        (TEXEL_TARGET, TEXEL_TARGET),
        &case.bytes(observe),
        lights,
        &[Face {
            columns: (0, TEXEL_TARGET),
            owners,
        }],
        Some(material),
    )
}

/// Draw `faces`, each a run of whole columns, into a `width`×`height`
/// `Rgba32Float` target; returns every pixel's RGB, row-major.
fn draw(
    ctx: &GpuCtx,
    source: &str,
    (width, height): (u32, u32),
    uniform: &[u8],
    lights: &[Light],
    faces: &[Face],
) -> Vec<[f32; 3]> {
    draw_rgba(ctx, source, (width, height), uniform, lights, faces, None)
        .into_iter()
        .map(|[r, g, b, a]| {
            assert_eq!(a, 1.0, "every pixel must be covered by a face");
            [r, g, b]
        })
        .collect()
}

/// [`draw`], binding `material` at bindings 2..=4 when given, returning every
/// pixel's RGBA.
fn draw_rgba(
    ctx: &GpuCtx,
    source: &str,
    (width, height): (u32, u32),
    uniform: &[u8],
    lights: &[Light],
    faces: &[Face],
    material: Option<&MaterialTextures>,
) -> Vec<[f32; 4]> {
    use wgpu::util::DeviceExt;

    let device = &ctx.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("world_specular_test shader"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });

    let mut positions: Vec<u8> = Vec::new();
    let mut owners: Vec<u8> = Vec::new();
    for face in faces {
        let x0 = face.columns.0 as f32 / width as f32 * 2.0 - 1.0;
        let x1 = face.columns.1 as f32 / width as f32 * 2.0 - 1.0;
        for [x, y] in [
            [x0, -1.0],
            [x1, -1.0],
            [x1, 1.0],
            [x0, -1.0],
            [x1, 1.0],
            [x0, 1.0],
        ] {
            positions.extend_from_slice(&f32::to_ne_bytes(x));
            positions.extend_from_slice(&f32::to_ne_bytes(y));
            for owner in face.owners {
                owners.extend_from_slice(&owner.to_ne_bytes());
            }
        }
    }
    let vertex_count = faces.len() as u32 * 6;
    let position_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("world_specular_test positions"),
        contents: &positions,
        usage: wgpu::BufferUsages::VERTEX,
    });
    let owner_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("world_specular_test owners"),
        contents: &owners,
        usage: wgpu::BufferUsages::VERTEX,
    });
    let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("world_specular_test case"),
        contents: uniform,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let light_bytes: Vec<u8> = lights.iter().flat_map(|light| light.bytes()).collect();
    let light_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("world_specular_test spec lights"),
        contents: &light_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });

    let target_format = wgpu::TextureFormat::Rgba32Float;
    let position_attributes = [wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x2,
        offset: 0,
        shader_location: 0,
    }];
    // The owner stream's layout: one Uint16x4 per vertex in its own buffer.
    let owner_attributes = [wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Uint16x4,
        offset: 0,
        shader_location: 1,
    }];
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("world_specular_test pipeline"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[
                Some(wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &position_attributes,
                }),
                Some(wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &owner_attributes,
                }),
            ],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    let mut entries = vec![
        wgpu::BindGroupEntry {
            binding: 0,
            resource: uniforms.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: 1,
            resource: light_buffer.as_entire_binding(),
        },
    ];
    if let Some(material) = material {
        entries.extend([
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&material.base),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&material.specular),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&material.sampler),
            },
        ]);
    }
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("world_specular_test bind group"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries,
    });

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("world_specular_test target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: target_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    let row_bytes = (width * TEXEL_BYTES).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("world_specular_test readback"),
        size: u64::from(row_bytes * height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("world_specular_test pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: -1.0,
                        g: -1.0,
                        b: -1.0,
                        a: 0.0,
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
        pass.set_vertex_buffer(0, position_buffer.slice(..));
        pass.set_vertex_buffer(1, owner_buffer.slice(..));
        pass.draw(0..vertex_count, 0..1);
    }
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| {
        result.expect("map world_specular_test readback");
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll world_specular_test device");
    let bytes = slice
        .get_mapped_range()
        .expect("buffer mapped for readback");
    let mut texels = Vec::with_capacity((width * height) as usize);
    for row in bytes.chunks(row_bytes as usize) {
        for texel in row[..(width * TEXEL_BYTES) as usize]
            .as_chunks::<{ TEXEL_BYTES as usize }>()
            .0
        {
            texels.push(std::array::from_fn(|c| {
                f32::from_ne_bytes(texel[c * 4..c * 4 + 4].try_into().unwrap())
            }));
        }
    }
    drop(bytes);
    readback.unmap();
    texels
}
