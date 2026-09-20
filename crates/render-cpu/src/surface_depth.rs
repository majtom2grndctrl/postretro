//! Surface Depth (texel-space parallax): CPU reference for the shader DDA and
//! the per-material uniform packing it reads.
//!
//! See: context/lib/rendering_pipeline.md §7.3 (Surface Depth).
//!
//! The height field baked into the specular slot's G channel is **depth below
//! the surface**, piecewise-constant per texel — a grid of boxes whose lattice
//! is the same texel grid `sample_post_retro` snaps albedo to. Marching it is
//! therefore an exact 2D DDA (Amanatides-Woo) over that grid, not a fixed-step
//! raymarch: within one texel the solid's top is a single depth, so the only
//! events are "cross a side wall" and "meet the top".
//!
//! Everything in this module is GPU-free, and it is the authority for the
//! march itself, the packed material layout, and every tuning constant that
//! the WGSL snippet `shaders/surface_depth.wgsl` mirrors — the renderer's rule
//! is that data logic stays testable without a GPU. Constants defined here are
//! pinned against the shader text by `render/tests/surface_depth_tests.rs`.
//!
//! One rule is GPU-only and this module does NOT mirror it: the texel→meters
//! conversion in `surface_depth.wgsl` (`texels_per_m`, `texel_rate`, and the
//! `depth_scale_m` derivation gated on `SURFACE_DEPTH_TEXEL_MODE`) reads a
//! per-fragment UV Jacobian that only exists mid-shader, so
//! [`march_surface_depth`] below takes the already-converted
//! `depth_scale_meters` as a parameter rather than deriving it. That is an
//! accepted gap, not an oversight: Surface Depth is a purely graphical carve —
//! collision still walks the true brush plane — so a wrong conversion is a
//! visible depth error on screen, never a corrupted game-logic value, and
//! there is no save data or netcode downstream of it to silently corrupt.

use glam::Vec3;
use postretro_render_data::material::SurfaceDepth;

/// Uniform-word bit offset of the packed base mip level.
pub const SURFACE_DEPTH_BASE_MIP_SHIFT: u32 = 8;
/// Bit width (and therefore ceiling) of the packed base mip level.
pub const SURFACE_DEPTH_BASE_MIP_MASK: u32 = 0xF;
/// Packed flag: the bound specular slot really is a two-channel surface map.
pub const SURFACE_DEPTH_HAS_DEPTH_BIT: u32 = 1 << 12;
/// Uniform-word bit offset of the packed per-fragment self-shadow budget.
///
/// The budget is per-MATERIAL data rather than a shader constant because the
/// player-facing on/off switch (design D5) is applied by rewriting this buffer,
/// not by compiling a shader variant — this engine has no variant system. Bit
/// 13..16 stay free between the has-depth flag and this field.
pub const SURFACE_DEPTH_SHADOW_BUDGET_SHIFT: u32 = 16;
/// Bit width (and therefore ceiling) of the packed self-shadow budget.
pub const SURFACE_DEPTH_SHADOW_BUDGET_MASK: u32 = 0xF;
/// Ceiling on the packed step count (one byte).
pub const SURFACE_DEPTH_MAX_STEPS: u32 = 0xFF;

/// The mip level the DDA reads the surface map at today.
///
/// D6.2: this must reach the shader as a *parameter*, never a hardcoded `0` in
/// WGSL. Asset streaming will drop top mips; when it does, this value moves and
/// a hardcoded `0` would silently read non-resident data. The fade-to-flat LOD
/// is driven off the dimensions at this level for the same reason, so a
/// streamed-out surface map flattens gracefully instead of popping.
pub const SURFACE_DEPTH_RESIDENT_BASE_MIP: u32 = 0;

/// Screen-space LOD (in base-mip texels per pixel, log2) at which the carve
/// starts fading toward flat.
pub const SURFACE_DEPTH_FADE_LOD_START: f32 = 1.0;
/// LOD span over which the carve fades from full to flat.
pub const SURFACE_DEPTH_FADE_LOD_RANGE: f32 = 2.0;
/// Fraction of a material's fade distance spent ramping down. The ramp ends
/// exactly at `fade_distance_meters`, so the march is skipped beyond it.
pub const SURFACE_DEPTH_FADE_DISTANCE_FRACTION: f32 = 0.25;
/// How much a fully-deep hit darkens the SH indirect term.
///
/// Indirect only. SH probes sit at ~1 m spacing and know nothing of the
/// receiver's own geometry (`rendering_pipeline.md` §4), so cobblestone-scale
/// self-occlusion is a fact no other source owns — this is not double-counting
/// a light.
pub const SURFACE_DEPTH_AO_STRENGTH: f32 = 0.75;
/// Per-fragment cap on dynamic-light self-shadow marches while the feature is
/// `On`. `Off` drops it to zero — see [`SurfaceDepthQuality`].
pub const SURFACE_DEPTH_SHADOW_LIGHT_BUDGET: u32 = 2;
/// Depth slack, in meters, before a self-shadow march calls a texel occluding.
/// Plateaus share exact quantized values, so equality must read as lit.
pub const SURFACE_DEPTH_SHADOW_BIAS_M: f32 = 1.0e-4;
/// How far past a crossed texel boundary a side hit shifts its texture-sample
/// UV, in texels. Half a texel lands on the entered texel's center line, so
/// `sample_post_retro` reads the stone's own color rather than blending across
/// the boundary it just hit.
pub const SURFACE_DEPTH_SIDE_UV_BIAS_TEXELS: f32 = 0.5;

/// Degenerate-geometry floor. Below it the UV→world Jacobian is unusable and
/// the fragment stays flat.
const SURFACE_DEPTH_EPS: f32 = 1.0e-9;
/// Floor on the UV Jacobian's determinant.
///
/// It is a PRODUCT of two UV derivatives, so it is naturally tiny — at close
/// range a 1 k texture can put it near `1e-9`, which is why this is NOT
/// `SURFACE_DEPTH_EPS`. It only has to catch a genuinely singular chart (which
/// makes the solve `0/0`); the honest check is the scale range below, which
/// also traps the NaN and infinity a near-singular solve produces.
pub const SURFACE_DEPTH_DET_EPS: f32 = 1.0e-20;
/// Sanity range for world meters per UV unit. Outside it the solved frame is
/// not describing a real brush face.
pub const SURFACE_DEPTH_MAX_UV_SCALE_M: f32 = 1.0e6;
/// Cosine floor between the view ray and the surface normal. Below it the
/// fragment is a sub-pixel sliver seen edge-on and the march has nothing to say.
pub const SURFACE_DEPTH_MIN_DESCENT: f32 = 1.0e-3;

/// `x` strictly greater than `lo`. A NaN answers `false`, which is the point:
/// every degenerate test in this module is written as `!above(..)` so a NaN
/// falls out to the flat path instead of slipping through a `<=` comparison and
/// poisoning the march. (Also keeps `clippy::neg_cmp_op_on_partial_ord` happy,
/// which would otherwise flag the `!(x > lo)` this replaces.)
fn above(x: f32, lo: f32) -> bool {
    x > lo
}

/// `x` strictly inside the open range `(lo, hi)`. NaN and infinity both answer
/// `false`.
fn within(x: f32, lo: f32, hi: f32) -> bool {
    x > lo && x < hi
}

/// The fields the material uniform's trailing `march` word carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SurfaceDepthMarch {
    /// Hard cap on texels the view-ray DDA may walk.
    pub max_steps: u32,
    /// Resident base mip the DDA reads the surface map at (D6.2).
    pub base_mip: u32,
    /// The bound specular slot really is a two-channel surface map.
    pub has_depth: bool,
    /// Per-fragment cap on dynamic-light self-shadow marches. Zero means the
    /// fragment never runs the second (shadow) DDA at all.
    pub shadow_light_budget: u32,
}

/// Pack the trailing `march` word of the material uniform.
///
/// Every field is clamped rather than rejected: a nonsense value must degrade
/// to a bounded march, never to an out-of-range `textureLoad` or an unbounded
/// loop.
pub fn pack_surface_depth_march(march: SurfaceDepthMarch) -> u32 {
    let steps = march.max_steps.min(SURFACE_DEPTH_MAX_STEPS);
    let mip = march.base_mip.min(SURFACE_DEPTH_BASE_MIP_MASK);
    let shadow = march
        .shadow_light_budget
        .min(SURFACE_DEPTH_SHADOW_BUDGET_MASK);
    steps
        | (mip << SURFACE_DEPTH_BASE_MIP_SHIFT)
        | (shadow << SURFACE_DEPTH_SHADOW_BUDGET_SHIFT)
        | if march.has_depth {
            SURFACE_DEPTH_HAS_DEPTH_BIT
        } else {
            0
        }
}

/// Unpack the `march` word. The shader performs the identical decode.
pub fn unpack_surface_depth_march(packed: u32) -> SurfaceDepthMarch {
    SurfaceDepthMarch {
        max_steps: packed & SURFACE_DEPTH_MAX_STEPS,
        base_mip: (packed >> SURFACE_DEPTH_BASE_MIP_SHIFT) & SURFACE_DEPTH_BASE_MIP_MASK,
        has_depth: (packed & SURFACE_DEPTH_HAS_DEPTH_BIT) != 0,
        shadow_light_budget: (packed >> SURFACE_DEPTH_SHADOW_BUDGET_SHIFT)
            & SURFACE_DEPTH_SHADOW_BUDGET_MASK,
    }
}

/// Re-exported so the shader-parity test and the uniform resolve read the depth
/// unit and its ceiling from one place. Both live in `render-data`, beside the
/// authoring tables they select; see [`SURFACE_DEPTH_TEXEL_MODE`] for which unit
/// ships and why. The rationale for clamping at all lives on
/// [`SurfaceDepthUniform::resolve`], which is where it is applied.
pub use postretro_render_data::material::{
    SURFACE_DEPTH_MAX_METERS, SURFACE_DEPTH_MAX_TEXELS, SURFACE_DEPTH_TEXEL_MODE,
    surface_depth_is_texel_relative, surface_depth_max_authored,
};

/// Player-facing Surface Depth switch (design D5).
///
/// Two states, not a graded tier ladder: the feature is a per-fragment cost a
/// weak GPU either can or cannot afford, and a middle setting that kept the
/// march but capped its budget never changed the carve DEPTH — it only made the
/// march resolve short at grazing angles. That is a cost lever priced in
/// artifacts, so the ladder collapsed to on/off.
///
/// The state is applied by rewriting the per-material uniform BUFFER
/// (`queue.write_buffer`) — never by rebuilding bind groups (which would
/// allocate during gameplay, against `resource_management.md` §8.2) and never
/// by growing the 128-byte group-0 `Uniforms` ABI. That is why every knob it
/// turns lives in this module's packed material row.
///
/// The derivation is here, in the GPU-free crate, rather than in the renderer's
/// GPU call path, so it is unit-testable without an adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SurfaceDepthQuality {
    /// Force flat. The material resolves to [`SurfaceDepthUniform::FLAT`], so
    /// the uniform's second row is the historical all-zero and the render is
    /// byte-identical to the pre-Surface-Depth engine at zero cost.
    Off,
    /// The full effect: the per-prefix values from `Material::surface_depth()`
    /// verbatim, with the full self-shadow budget. The default; the setting
    /// exists as an escape hatch, not as an opt-in.
    #[default]
    On,
}

impl SurfaceDepthQuality {
    /// Every state, in presentation order.
    ///
    /// This crate carries no serde and does not depend on `postretro-entities`,
    /// so nothing here can pin the list against the `options.surfaceDepthQuality`
    /// enum set that `engine_state_catalog` declares. The two are kept in step by
    /// `postretro`, which depends on both — see the chokepoint in
    /// `startup/render_profile.rs`, whose `match` has no `_` arm so a new state is
    /// a compile error there rather than a silent degrade.
    pub const ALL: [Self; 2] = [Self::Off, Self::On];

    /// Per-fragment dynamic-light self-shadow budget this state allows.
    pub const fn shadow_light_budget(self) -> u32 {
        match self {
            // The second DDA is the expensive part, and it is the part a
            // struggling GPU pays for per light per fragment.
            Self::Off => 0,
            Self::On => SURFACE_DEPTH_SHADOW_LIGHT_BUDGET,
        }
    }

    /// Apply this state to one material's prefix-driven tuning.
    ///
    /// `Off` returns [`SurfaceDepth::FLAT`] unconditionally, so the packed row
    /// is all zero and the shader's existing has-depth branch skips the march.
    /// `On` is the identity: the per-prefix values reach the GPU unmodified.
    pub fn apply(self, depth: SurfaceDepth) -> SurfaceDepth {
        match self {
            Self::Off => SurfaceDepth::FLAT,
            Self::On => depth,
        }
    }
}

/// Everything the bind-group builder must decide about one material's carve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceDepthUniform {
    pub depth: SurfaceDepth,
    /// Set from the *loaded* specular slot's format, not from the material
    /// prefix: a prefix that wants depth but whose `.prm` has no `_h` sibling
    /// must skip the march rather than walk an all-zero field.
    pub has_depth: bool,
    /// Resident base mip the DDA reads at (D6.2).
    pub base_mip: u32,
    /// Per-fragment dynamic-light self-shadow budget, from the player's
    /// on/off switch. Zero means the fragment never runs the shadow DDA.
    pub shadow_light_budget: u32,
}

impl SurfaceDepthUniform {
    /// The flat case: no carve, and the shader's has-depth branch is clear.
    pub const FLAT: Self = Self {
        depth: SurfaceDepth::FLAT,
        has_depth: false,
        base_mip: SURFACE_DEPTH_RESIDENT_BASE_MIP,
        shadow_light_budget: 0,
    };

    /// Resolve a material's prefix-driven tuning against the player's on/off
    /// switch and against what actually loaded.
    ///
    /// `quality` is the persisted player setting (design D5); it is applied
    /// FIRST, so `Off` collapses to [`Self::FLAT`] before any other decision is
    /// made. `specular_is_surface_map` comes from the bound texture's format.
    /// `requested_base_mip` is the residency decision — today always
    /// [`SURFACE_DEPTH_RESIDENT_BASE_MIP`], later whatever streaming has kept
    /// resident — and is clamped against `specular_mip_count` so the DDA can
    /// never `textureLoad` past the end of the uploaded chain.
    ///
    /// The authored depth is also rejected if non-finite and clamped to
    /// [`surface_depth_max_authored`]. `SurfaceDepth::depth_meters` is a public
    /// field and `is_enabled()` only tests `> 0.0`, which admits `+inf`; an
    /// infinite scale makes the march's `solid` a NaN on every zero-depth texel
    /// — the most common texel in a cobblestone map — and a NaN compares false
    /// against both hit rules. The march terminates on its integer budget
    /// regardless, so this is defence in depth rather than the only guard, but
    /// it is what keeps the ceiling a real constraint on the value reaching the
    /// GPU instead of an assertion about one hardcoded table.
    pub fn resolve(
        depth: SurfaceDepth,
        quality: SurfaceDepthQuality,
        specular_is_surface_map: bool,
        specular_mip_count: u32,
        requested_base_mip: u32,
    ) -> Self {
        let mut tuned = quality.apply(depth);
        if !specular_is_surface_map || !tuned.is_enabled() {
            return Self::FLAT;
        }
        // `is_enabled()` rejects NaN, zero and negative but admits `+inf`.
        if !tuned.depth_meters.is_finite() {
            return Self::FLAT;
        }
        // Cap against whichever unit the field is carrying.
        tuned.depth_meters = tuned.depth_meters.min(surface_depth_max_authored());
        let top_level = specular_mip_count.saturating_sub(1);
        Self {
            depth: tuned,
            has_depth: true,
            base_mip: requested_base_mip.min(top_level),
            shadow_light_budget: quality.shadow_light_budget(),
        }
    }

    /// The packed trailing word this resolves to.
    pub fn march_word(self) -> u32 {
        pack_surface_depth_march(SurfaceDepthMarch {
            max_steps: self.depth.max_steps,
            base_mip: self.base_mip,
            has_depth: self.has_depth,
            shadow_light_budget: self.shadow_light_budget,
        })
    }
}

/// Distance fade: 1 near, 0 at and beyond `fade_distance_meters`.
pub fn surface_depth_distance_fade(distance_meters: f32, fade_distance_meters: f32) -> f32 {
    if fade_distance_meters <= 0.0 {
        return 0.0;
    }
    let ramp = (fade_distance_meters * SURFACE_DEPTH_FADE_DISTANCE_FRACTION).max(SURFACE_DEPTH_EPS);
    ((fade_distance_meters - distance_meters) / ramp).clamp(0.0, 1.0)
}

/// Residency/LOD fade: 1 while base-mip texels are still several pixels wide,
/// 0 once they are far below pixel size.
///
/// `lod` is `log2` of the fragment's footprint measured in texels **of the
/// resident base mip**, so a streamed-out surface map (smaller dimensions at
/// its base level) fades out rather than popping.
pub fn surface_depth_lod_fade(lod: f32) -> f32 {
    (1.0 - (lod - SURFACE_DEPTH_FADE_LOD_START) / SURFACE_DEPTH_FADE_LOD_RANGE).clamp(0.0, 1.0)
}

/// `log2` of the fragment's footprint in base-mip texels.
pub fn surface_depth_lod(ddx_uv: [f32; 2], ddy_uv: [f32; 2], dims: [f32; 2]) -> f32 {
    let len = |d: [f32; 2]| ((d[0] * dims[0]).powi(2) + (d[1] * dims[1]).powi(2)).sqrt();
    len(ddx_uv).max(len(ddy_uv)).max(SURFACE_DEPTH_EPS).log2()
}

/// Combined fade. The shader multiplies the material's depth by this, so a
/// faded surface *flattens* — it never snaps between marched and unmarched.
pub fn surface_depth_fade(distance_meters: f32, fade_distance_meters: f32, lod: f32) -> f32 {
    surface_depth_distance_fade(distance_meters, fade_distance_meters)
        .min(surface_depth_lod_fade(lod))
}

/// Ambient occlusion factor for the SH indirect term from a hit's depth.
///
/// `fade` is the same value `surface_depth_fade` returned for this fragment.
/// Both depth arguments are already post-fade, so their ratio is the raw texel
/// value at every fade and the factor is what actually makes the occlusion
/// degrade with the carve instead of popping at the fade boundary.
pub fn surface_depth_ambient_occlusion(
    hit_depth_meters: f32,
    depth_scale_meters: f32,
    fade: f32,
) -> f32 {
    if depth_scale_meters <= SURFACE_DEPTH_EPS {
        return 1.0;
    }
    1.0 - SURFACE_DEPTH_AO_STRENGTH
        * fade
        * (hit_depth_meters / depth_scale_meters).clamp(0.0, 1.0)
}

/// Orthonormal surface frame derived from screen-space derivatives rather than
/// from the baked tangent.
///
/// Deriving it here rather than from `world_tangent` is deliberate: the march
/// walks the UV grid, so its axes must be the UV axes and its scale must be the
/// same world-units-per-UV-unit the meters→UV conversion uses. Taking both from
/// one Jacobian makes that consistency structural. The baked tangent still owns
/// normal mapping, which is a different (and authored) tangent space.
///
/// Assumes the UV parameterization is locally orthogonal, which holds for brush
/// faces (TrenchBroom emits scale/rotate/offset UV axes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceDepthBasis {
    /// Unit world direction of increasing `u`, projected into the tangent plane.
    pub tangent: Vec3,
    /// Unit world direction of increasing `v`, projected into the tangent plane.
    pub bitangent: Vec3,
    /// Geometric surface normal (never the shading normal).
    pub normal: Vec3,
    /// UV units per world meter along `u` and `v`.
    pub uv_per_meter: [f32; 2],
}

/// Solve `∂W/∂u` and `∂W/∂v` from the screen-space derivative pairs and build
/// the surface frame. Returns `None` for a degenerate fragment (collapsed UV
/// chart, vanishing derivatives) — the caller then stays flat.
pub fn surface_depth_basis(
    ddx_uv: [f32; 2],
    ddy_uv: [f32; 2],
    ddx_world: Vec3,
    ddy_world: Vec3,
    normal: Vec3,
) -> Option<SurfaceDepthBasis> {
    // [Wx] = [ux vx][Wu]
    // [Wy]   [uy vy][Wv]
    // See `above` / `within`: NaN must fall out, not slip through.
    let det = ddx_uv[0] * ddy_uv[1] - ddx_uv[1] * ddy_uv[0];
    if !above(det.abs(), SURFACE_DEPTH_DET_EPS) {
        return None;
    }
    let w_u = (ddx_world * ddy_uv[1] - ddy_world * ddx_uv[1]) / det;
    let w_v = (ddy_world * ddx_uv[0] - ddx_world * ddy_uv[0]) / det;

    let scale_u = w_u.length();
    let scale_v = w_v.length();
    let usable = |s: f32| within(s, SURFACE_DEPTH_EPS, SURFACE_DEPTH_MAX_UV_SCALE_M);
    if !usable(scale_u) || !usable(scale_v) {
        return None;
    }

    // Project out the normal: interpolation leaves the derivatives slightly
    // off the tangent plane, and a side-wall normal that is not perpendicular
    // to the surface normal reads as a lighting seam.
    let t = w_u - normal * w_u.dot(normal);
    let b = w_v - normal * w_v.dot(normal);
    if !above(t.length_squared(), SURFACE_DEPTH_EPS)
        || !above(b.length_squared(), SURFACE_DEPTH_EPS)
    {
        return None;
    }

    Some(SurfaceDepthBasis {
        tangent: t.normalize(),
        bitangent: b.normalize(),
        normal,
        uv_per_meter: [1.0 / scale_u, 1.0 / scale_v],
    })
}

/// UV advance per meter of descent for the view ray.
///
/// `view_to_eye` points from the surface toward the camera. Returns `None` for
/// an edge-on fragment, where the ray never descends and the march is
/// meaningless.
pub fn surface_depth_view_ray(basis: &SurfaceDepthBasis, view_to_eye: Vec3) -> Option<[f32; 2]> {
    let descent = view_to_eye.dot(basis.normal);
    if !above(descent, SURFACE_DEPTH_MIN_DESCENT) {
        return None;
    }
    let into = -view_to_eye;
    Some([
        into.dot(basis.tangent) * basis.uv_per_meter[0] / descent,
        into.dot(basis.bitangent) * basis.uv_per_meter[1] / descent,
    ])
}

/// UV advance per meter of *rise* for a shadow ray toward `to_light`.
/// `None` when the light is at or below the surface plane.
pub fn surface_depth_light_ray(basis: &SurfaceDepthBasis, to_light: Vec3) -> Option<[f32; 2]> {
    let rise = to_light.dot(basis.normal);
    if !above(rise, SURFACE_DEPTH_EPS) {
        return None;
    }
    Some([
        to_light.dot(basis.tangent) * basis.uv_per_meter[0] / rise,
        to_light.dot(basis.bitangent) * basis.uv_per_meter[1] / rise,
    ])
}

/// A piecewise-constant depth field: one `[0, 1]` value per texel, tiled
/// (`AddressMode::Repeat`, no atlas — marching `base_uv` freely is safe).
#[derive(Debug, Clone, Copy)]
pub struct SurfaceDepthField<'a> {
    pub width: i32,
    pub height: i32,
    /// Row-major, `width * height` entries, the specular slot's G channel.
    pub depth_unorm: &'a [f32],
    /// `floor(h * levels) / levels`; `0` leaves the stored value alone.
    pub quantize_levels: u32,
}

impl SurfaceDepthField<'_> {
    /// Wrapped, quantized fetch. Quantization is one ALU op in the shader and
    /// does not affect the DDA's exactness — that comes from per-texel
    /// constancy, not from the value being on a plateau.
    pub fn sample(&self, x: i32, y: i32) -> f32 {
        let wrap = |v: i32, n: i32| {
            let m = v % n;
            if m < 0 { m + n } else { m }
        };
        let x = wrap(x, self.width);
        let y = wrap(y, self.height);
        let raw = self.depth_unorm[(y * self.width + x) as usize];
        if self.quantize_levels == 0 {
            return raw;
        }
        let levels = self.quantize_levels as f32;
        (raw * levels).floor() / levels
    }
}

/// Which face the view ray landed on. Exactly five outcomes — the field is a
/// grid of axis-aligned boxes, so there is nothing else to hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceDepthFace {
    /// The top of a texel's solid, or the untouched plane on a flat field.
    Top,
    /// A side wall whose outward normal is `-u`.
    NegU,
    /// A side wall whose outward normal is `+u`.
    PosU,
    /// A side wall whose outward normal is `-v`.
    NegV,
    /// A side wall whose outward normal is `+v`.
    PosV,
}

impl SurfaceDepthFace {
    /// The face normal in the `(tangent, bitangent, normal)` frame.
    pub const fn normal_texel(self) -> [f32; 3] {
        match self {
            SurfaceDepthFace::Top => [0.0, 0.0, 1.0],
            SurfaceDepthFace::NegU => [-1.0, 0.0, 0.0],
            SurfaceDepthFace::PosU => [1.0, 0.0, 0.0],
            SurfaceDepthFace::NegV => [0.0, -1.0, 0.0],
            SurfaceDepthFace::PosV => [0.0, 1.0, 0.0],
        }
    }

    pub const fn is_top(self) -> bool {
        matches!(self, SurfaceDepthFace::Top)
    }
}

/// Result of one view-ray march.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceDepthHit {
    /// UV to sample the material's textures at — biased half a texel past a
    /// crossed boundary on a side hit.
    pub uv: [f32; 2],
    /// UV of the hit point itself, unbiased. Self-shadow marches start here.
    pub march_uv: [f32; 2],
    /// Depth below the true plane, in meters. Always `<= depth_scale_meters`:
    /// the carve is inward-only, so the displaced surface never exceeds its
    /// real plane and collision stays correct with no work.
    pub depth_meters: f32,
    pub face: SurfaceDepthFace,
    /// Texels walked. Only ever interesting for budgeting.
    pub steps: u32,
}

/// March the view ray through the texel grid.
///
/// Per texel `T` the solid begins at `D(T) = sample(T) * depth_scale_meters`.
/// Within `T` the ray spans `[z_enter, z_exit]`:
/// * `z_enter >= D(T)` — the ray was already inside the solid on entry, so it
///   hit the SIDE wall it entered through.
/// * `z_exit > D(T)` — it meets the TOP at `D(T)`.
/// * otherwise — the texel is entirely below the ray; step on.
///
/// The first texel is entered through the plane itself, so its "entry face" is
/// the geometric top. That makes an all-zero field resolve on the first
/// iteration at depth 0 with the geometric normal and the original UV — the
/// exact no-op a material without an `_h` sibling must produce.
///
/// Termination is structural: the budget test is an integer comparison on the
/// iteration count, so the loop exits after at most `max_steps` iterations
/// whatever the field samples to — including a NaN, which compares false
/// against both hit rules. The depth bound is a separate guarantee:
/// `D <= depth_scale_meters` everywhere, so once `z_enter` reaches the scale the
/// first rule fires on its own and the budget never comes into it.
///
/// `max_steps` therefore only bites at grazing angles, and when it does the hit
/// resolves at `z_enter` — the last crossed boundary — so the reported UV stays
/// inside the region the march actually walked.
pub fn march_surface_depth(
    field: &SurfaceDepthField<'_>,
    uv0: [f32; 2],
    dir_uv_per_meter: [f32; 2],
    depth_scale_meters: f32,
    max_steps: u32,
) -> SurfaceDepthHit {
    let dims = [field.width as f32, field.height as f32];
    let p0 = [uv0[0] * dims[0], uv0[1] * dims[1]];
    // Texels per meter of descent.
    let dir = [dir_uv_per_meter[0] * dims[0], dir_uv_per_meter[1] * dims[1]];
    let mut cell = [p0[0].floor() as i32, p0[1].floor() as i32];

    const FAR: f32 = f32::MAX;
    let mut t_max = [FAR, FAR];
    let mut t_delta = [FAR, FAR];
    let mut step = [0i32, 0i32];
    for axis in 0..2 {
        if dir[axis].abs() <= SURFACE_DEPTH_EPS {
            continue;
        }
        let positive = dir[axis] > 0.0;
        step[axis] = if positive { 1 } else { -1 };
        let boundary = if positive {
            (cell[axis] + 1) as f32
        } else {
            cell[axis] as f32
        };
        t_max[axis] = (boundary - p0[axis]) / dir[axis];
        t_delta[axis] = (1.0 / dir[axis]).abs();
    }

    let steps_allowed = max_steps.max(1);
    let mut z_enter = 0.0f32;
    let mut entry_face = SurfaceDepthFace::Top;
    let mut entry_bias = [0.0f32, 0.0f32];
    let mut walked = 0u32;

    let (depth_meters, face, bias) = loop {
        let solid = field.sample(cell[0], cell[1]) * depth_scale_meters;
        let z_exit = t_max[0].min(t_max[1]);
        if z_enter >= solid {
            break (z_enter, entry_face, entry_bias);
        }
        if z_exit > solid {
            break (solid, SurfaceDepthFace::Top, [0.0, 0.0]);
        }
        // Budget exhausted with the ray still in open space. Resolve HERE, at
        // the last boundary the walk actually crossed.
        //
        // The obvious alternative — treating this texel as unbounded so the TOP
        // rule fires — resolves at its full `solid` depth, and the sample point
        // is `p0 + dir * hit_depth` where `dir` is texels per METER OF DESCENT.
        // At a grazing angle that lands the albedo, normal and specular samples
        // tens of texels past anything the march visited, so a TIGHTER budget
        // produced a LARGER artifact — exactly backwards for the knob whose job
        // is to make the effect cheaper. Stopping at `z_enter` keeps the
        // sample inside the walked region, and on the first iteration it IS the
        // flat result (depth 0, geometric normal, original UV), so a budget too
        // small to march degrades toward flat rather than toward an arbitrary
        // texel.
        //
        // This is also what makes termination structural rather than a property
        // of the sampled values: the test is INTEGER, so the loop exits after
        // `max_steps` iterations whatever `solid` is — including a NaN, which
        // compares false against both hit rules.
        if walked + 1 >= steps_allowed {
            // The TOP face, not the entry face — see the WGSL mirror: `z_enter`
            // is above the solid in both the cell just left and the one just
            // entered, so no wall exists at this depth to report.
            break (z_enter, SurfaceDepthFace::Top, [0.0, 0.0]);
        }
        if t_max[0] <= t_max[1] {
            cell[0] += step[0];
            z_enter = t_max[0];
            t_max[0] += t_delta[0];
            entry_face = if step[0] > 0 {
                SurfaceDepthFace::NegU
            } else {
                SurfaceDepthFace::PosU
            };
            entry_bias = [step[0] as f32 * SURFACE_DEPTH_SIDE_UV_BIAS_TEXELS, 0.0];
        } else {
            cell[1] += step[1];
            z_enter = t_max[1];
            t_max[1] += t_delta[1];
            entry_face = if step[1] > 0 {
                SurfaceDepthFace::NegV
            } else {
                SurfaceDepthFace::PosV
            };
            entry_bias = [0.0, step[1] as f32 * SURFACE_DEPTH_SIDE_UV_BIAS_TEXELS];
        }
        walked += 1;
    };

    let hit = [p0[0] + dir[0] * depth_meters, p0[1] + dir[1] * depth_meters];
    SurfaceDepthHit {
        uv: [(hit[0] + bias[0]) / dims[0], (hit[1] + bias[1]) / dims[1]],
        march_uv: [hit[0] / dims[0], hit[1] / dims[1]],
        depth_meters,
        face,
        steps: walked,
    }
}

/// Self-shadow one dynamic light: a second, shorter DDA from the hit point
/// toward the light. Returns 1.0 lit, 0.0 occluded.
///
/// Legitimate against DYNAMIC lights only — the bake knows nothing about
/// dynamic bodies (`rendering_pipeline.md` §4: "the runtime owns only the facts
/// that involve a dynamic body"). There is deliberately NO march against baked
/// static light: the lightmap owns static-onto-static occlusion at 4 cm/texel
/// and competing with it risks the no-double-counting invariant.
///
/// The texel the ray starts in is never tested, so a hit never shadows itself:
/// a top hit sits exactly on that texel's solid, and a side hit sits on its
/// wall (and a light behind that wall is already culled by `NdotL <= 0`, since
/// the wall normal IS the shading normal).
pub fn surface_depth_light_visibility(
    field: &SurfaceDepthField<'_>,
    hit_uv: [f32; 2],
    hit_depth_meters: f32,
    light_uv_per_meter: [f32; 2],
    depth_scale_meters: f32,
    max_steps: u32,
) -> f32 {
    if hit_depth_meters <= SURFACE_DEPTH_SHADOW_BIAS_M {
        return 1.0;
    }
    let dims = [field.width as f32, field.height as f32];
    let p0 = [hit_uv[0] * dims[0], hit_uv[1] * dims[1]];
    // Texels per meter of rise.
    let dir = [
        light_uv_per_meter[0] * dims[0],
        light_uv_per_meter[1] * dims[1],
    ];
    let mut cell = [p0[0].floor() as i32, p0[1].floor() as i32];

    const FAR: f32 = f32::MAX;
    let mut t_max = [FAR, FAR];
    let mut t_delta = [FAR, FAR];
    let mut step = [0i32, 0i32];
    for axis in 0..2 {
        if dir[axis].abs() <= SURFACE_DEPTH_EPS {
            continue;
        }
        let positive = dir[axis] > 0.0;
        step[axis] = if positive { 1 } else { -1 };
        let boundary = if positive {
            (cell[axis] + 1) as f32
        } else {
            cell[axis] as f32
        };
        t_max[axis] = (boundary - p0[axis]) / dir[axis];
        t_delta[axis] = (1.0 / dir[axis]).abs();
    }

    for _ in 0..max_steps.max(1) {
        // Rising past the hit depth means the ray has left the carved band.
        if t_max[0].min(t_max[1]) >= hit_depth_meters {
            return 1.0;
        }
        let risen;
        if t_max[0] <= t_max[1] {
            cell[0] += step[0];
            risen = t_max[0];
            t_max[0] += t_delta[0];
        } else {
            cell[1] += step[1];
            risen = t_max[1];
            t_max[1] += t_delta[1];
        }
        let solid = field.sample(cell[0], cell[1]) * depth_scale_meters;
        if hit_depth_meters - risen > solid + SURFACE_DEPTH_SHADOW_BIAS_M {
            return 0.0;
        }
    }
    1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(width: i32, height: i32, values: &[f32]) -> SurfaceDepthField<'_> {
        SurfaceDepthField {
            width,
            height,
            depth_unorm: values,
            quantize_levels: 0,
        }
    }

    // -- Packing --

    fn march(max_steps: u32, base_mip: u32, has_depth: bool, budget: u32) -> SurfaceDepthMarch {
        SurfaceDepthMarch {
            max_steps,
            base_mip,
            has_depth,
            shadow_light_budget: budget,
        }
    }

    #[test]
    fn march_word_round_trips_every_packed_field() {
        for fields in [
            march(24, 0, true, 2),
            march(1, 7, true, 0),
            march(255, 15, false, 15),
        ] {
            assert_eq!(
                unpack_surface_depth_march(pack_surface_depth_march(fields)),
                fields
            );
        }
    }

    #[test]
    fn march_word_clamps_rather_than_wrapping() {
        let fields =
            unpack_surface_depth_march(pack_surface_depth_march(march(9999, 99, true, 9999)));
        assert_eq!(fields.max_steps, SURFACE_DEPTH_MAX_STEPS);
        assert_eq!(fields.base_mip, SURFACE_DEPTH_BASE_MIP_MASK);
        assert_eq!(fields.shadow_light_budget, SURFACE_DEPTH_SHADOW_BUDGET_MASK);
        assert!(fields.has_depth);
        // A clamped field must never bleed into its neighbours.
        assert_eq!(
            pack_surface_depth_march(march(9999, 99, true, 9999)),
            SURFACE_DEPTH_MAX_STEPS
                | (SURFACE_DEPTH_BASE_MIP_MASK << SURFACE_DEPTH_BASE_MIP_SHIFT)
                | SURFACE_DEPTH_HAS_DEPTH_BIT
                | (SURFACE_DEPTH_SHADOW_BUDGET_MASK << SURFACE_DEPTH_SHADOW_BUDGET_SHIFT),
        );
    }

    #[test]
    fn a_material_without_a_surface_map_resolves_flat() {
        let carving = postretro_render_data::material::Material::Concrete.surface_depth();
        assert!(carving.is_enabled());
        let resolved = SurfaceDepthUniform::resolve(
            carving,
            SurfaceDepthQuality::On,
            false,
            11,
            SURFACE_DEPTH_RESIDENT_BASE_MIP,
        );
        assert_eq!(resolved, SurfaceDepthUniform::FLAT);
        assert!(!resolved.has_depth);
        assert_eq!(resolved.march_word() & SURFACE_DEPTH_HAS_DEPTH_BIT, 0);
    }

    #[test]
    fn a_flat_prefix_with_a_surface_map_still_resolves_flat() {
        let flat = postretro_render_data::material::Material::Glass.surface_depth();
        assert_eq!(
            SurfaceDepthUniform::resolve(
                flat,
                SurfaceDepthQuality::On,
                true,
                11,
                SURFACE_DEPTH_RESIDENT_BASE_MIP
            ),
            SurfaceDepthUniform::FLAT
        );
    }

    fn resolve_on(carving: SurfaceDepth, mip_count: u32, requested: u32) -> SurfaceDepthUniform {
        SurfaceDepthUniform::resolve(carving, SurfaceDepthQuality::On, true, mip_count, requested)
    }

    #[test]
    fn base_mip_never_exceeds_the_uploaded_chain() {
        let carving = postretro_render_data::material::Material::Concrete.surface_depth();
        // A single-level chain (the 1x1 placeholder shape) must still be a
        // legal textureLoad level, whatever residency asks for.
        assert_eq!(resolve_on(carving, 1, 0).base_mip, 0);
        assert_eq!(resolve_on(carving, 0, 0).base_mip, 0);
        assert_eq!(resolve_on(carving, 1, 9).base_mip, 0);
        // A streamed-down chain keeps the level streaming asked for.
        assert_eq!(resolve_on(carving, 11, 3).base_mip, 3);
        assert_eq!(resolve_on(carving, 4, 9).base_mip, 3);
    }

    #[test]
    fn a_requested_base_mip_survives_the_packed_word() {
        let carving = postretro_render_data::material::Material::Concrete.surface_depth();
        let fields = unpack_surface_depth_march(resolve_on(carving, 11, 3).march_word());
        assert_eq!(fields.base_mip, 3);
        assert!(fields.has_depth);
    }

    // -- Player on/off switch (D5) --

    #[test]
    fn the_switch_defaults_to_on_because_the_feature_ships_enabled() {
        assert_eq!(SurfaceDepthQuality::default(), SurfaceDepthQuality::On);
    }

    #[test]
    fn the_switch_has_exactly_two_states() {
        // D5 is a cost lever, not a quality ladder: a third state would have to
        // earn its keep visually, and the one that existed never changed the
        // carve depth at all.
        assert_eq!(SurfaceDepthQuality::ALL.len(), 2);
        assert_eq!(
            SurfaceDepthQuality::ALL,
            [SurfaceDepthQuality::Off, SurfaceDepthQuality::On]
        );
    }

    #[test]
    fn off_forces_every_carving_material_flat() {
        for material in [
            postretro_render_data::material::Material::Concrete,
            postretro_render_data::material::Material::Metal,
            postretro_render_data::material::Material::Grate,
            postretro_render_data::material::Material::Wood,
            postretro_render_data::material::Material::Default,
        ] {
            let carving = material.surface_depth();
            assert!(carving.is_enabled(), "{material:?} must carve at On");
            assert_eq!(
                SurfaceDepthQuality::Off.apply(carving),
                SurfaceDepth::FLAT,
                "{material:?} must be forced flat at Off"
            );
            let resolved = SurfaceDepthUniform::resolve(
                carving,
                SurfaceDepthQuality::Off,
                true,
                11,
                SURFACE_DEPTH_RESIDENT_BASE_MIP,
            );
            assert_eq!(resolved, SurfaceDepthUniform::FLAT);
            assert_eq!(
                resolved.march_word(),
                0,
                "{material:?}: Off must pack the historical all-zero march word"
            );
        }
    }

    #[test]
    fn on_is_exactly_the_material_prefix_values() {
        for material in [
            postretro_render_data::material::Material::Concrete,
            postretro_render_data::material::Material::Metal,
            postretro_render_data::material::Material::Grate,
            postretro_render_data::material::Material::Wood,
            postretro_render_data::material::Material::Default,
        ] {
            let carving = material.surface_depth();
            assert_eq!(
                SurfaceDepthQuality::On.apply(carving),
                carving,
                "{material:?}: On must pass the per-prefix tuning through unmodified"
            );
            let resolved = resolve_on(carving, 11, SURFACE_DEPTH_RESIDENT_BASE_MIP);
            assert_eq!(resolved.depth, carving);
            assert_eq!(
                resolved.shadow_light_budget, SURFACE_DEPTH_SHADOW_LIGHT_BUDGET,
                "{material:?}: On gets the full self-shadow budget"
            );
            assert!(resolved.has_depth);
        }
    }

    #[test]
    fn only_on_budgets_a_self_shadow_march() {
        assert_eq!(SurfaceDepthQuality::Off.shadow_light_budget(), 0);
        assert_eq!(
            SurfaceDepthQuality::On.shadow_light_budget(),
            SURFACE_DEPTH_SHADOW_LIGHT_BUDGET
        );
        // The budget must survive its packed field without clamping.
        const { assert!(SURFACE_DEPTH_SHADOW_LIGHT_BUDGET <= SURFACE_DEPTH_SHADOW_BUDGET_MASK) };
    }

    #[test]
    fn a_flat_material_is_switch_independent() {
        // Glass and Neon are flat by intent; neither state may make them carve,
        // and both must produce the identical all-zero row.
        let flat = postretro_render_data::material::Material::Glass.surface_depth();
        for quality in SurfaceDepthQuality::ALL {
            assert_eq!(quality.apply(flat), SurfaceDepth::FLAT);
            assert_eq!(
                SurfaceDepthUniform::resolve(
                    flat,
                    quality,
                    true,
                    11,
                    SURFACE_DEPTH_RESIDENT_BASE_MIP
                ),
                SurfaceDepthUniform::FLAT
            );
        }
    }

    // -- Field sampling --

    #[test]
    fn sampling_wraps_like_address_mode_repeat() {
        let values = [0.0, 0.25, 0.5, 0.75];
        let f = field(2, 2, &values);
        assert_eq!(f.sample(0, 0), 0.0);
        assert_eq!(f.sample(2, 2), 0.0);
        assert_eq!(f.sample(-1, -1), 0.75);
        assert_eq!(f.sample(-2, 3), 0.5);
    }

    #[test]
    fn quantization_snaps_neighbours_onto_shared_plateaus() {
        let values = [0.30, 0.31, 0.67, 0.68];
        let f = SurfaceDepthField {
            width: 2,
            height: 2,
            depth_unorm: &values,
            quantize_levels: 3,
        };
        assert_eq!(f.sample(0, 0), f.sample(1, 0));
        assert_eq!(f.sample(0, 1), f.sample(1, 1));
        assert!((f.sample(0, 0) - 0.0).abs() < 1e-6);
        assert!((f.sample(0, 1) - 2.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn zero_levels_leaves_the_stored_value_untouched() {
        let values = [0.31415];
        assert_eq!(field(1, 1, &values).sample(0, 0), 0.31415);
    }

    // -- The march --

    #[test]
    fn an_all_zero_field_is_an_exact_no_op() {
        // This is the degradation contract: a material with no `_h` sibling
        // binds an R8 specular, WGSL expands it to (r, 0, 0, 1), and .g == 0.
        let values = [0.0; 16];
        let f = field(4, 4, &values);
        let hit = march_surface_depth(&f, [0.3, 0.7], [0.4, -0.2], 0.02, 24);
        assert_eq!(hit.depth_meters, 0.0);
        assert_eq!(hit.face, SurfaceDepthFace::Top);
        assert_eq!(hit.uv, [0.3, 0.7]);
        assert_eq!(hit.march_uv, [0.3, 0.7]);
        assert_eq!(hit.steps, 0);
    }

    #[test]
    fn a_straight_down_ray_lands_on_its_own_texel_top() {
        let values = [0.5, 0.0, 0.0, 0.0];
        let f = field(2, 2, &values);
        let hit = march_surface_depth(&f, [0.25, 0.25], [0.0, 0.0], 0.02, 24);
        assert!((hit.depth_meters - 0.01).abs() < 1e-6);
        assert_eq!(hit.face, SurfaceDepthFace::Top);
        assert_eq!(hit.uv, [0.25, 0.25]);
    }

    #[test]
    fn a_raised_neighbour_is_hit_on_its_side_wall() {
        // Texel (0,0) is a deep pit, texel (1,0) is flush with the plane.
        // Marching in +u from inside the pit must hit (1,0)'s -u wall at the
        // exact texel boundary, not the top of the stone.
        let values = [1.0, 0.0, 1.0, 0.0];
        let f = field(2, 2, &values);
        let depth_scale = 0.02;
        // One texel of +u travel per 0.02 m of descent: the ray reaches the
        // boundary at u = 0.5 having descended the full scale.
        let hit = march_surface_depth(&f, [0.25, 0.25], [0.5 / depth_scale, 0.0], depth_scale, 24);
        assert_eq!(hit.face, SurfaceDepthFace::NegU);
        assert!(
            (hit.march_uv[0] - 0.5).abs() < 1e-6,
            "side hit must land exactly on the texel boundary, got {}",
            hit.march_uv[0]
        );
        assert!(hit.depth_meters < depth_scale);
        // The sample UV is biased into the texel that was entered, so the
        // stone's own albedo reads on its wall.
        assert!(hit.uv[0] > hit.march_uv[0]);
        assert!((hit.uv[0] - (0.5 + 0.25)).abs() < 1e-6);
    }

    #[test]
    fn a_side_hit_normal_opposes_the_direction_of_travel() {
        // Pit on the left, flush stone on the right: travelling +u hits the
        // stone's -u wall.
        let right_stone = [1.0, 0.0, 1.0, 0.0];
        let forward = march_surface_depth(
            &field(2, 2, &right_stone),
            [0.25, 0.25],
            [25.0, 0.0],
            0.02,
            24,
        );
        assert_eq!(forward.face, SurfaceDepthFace::NegU);
        assert_eq!(forward.face.normal_texel(), [-1.0, 0.0, 0.0]);

        // Mirrored: stone on the left, travelling -u hits its +u wall.
        let left_stone = [0.0, 1.0, 0.0, 1.0];
        let backward = march_surface_depth(
            &field(2, 2, &left_stone),
            [0.75, 0.25],
            [-25.0, 0.0],
            0.02,
            24,
        );
        assert_eq!(backward.face, SurfaceDepthFace::PosU);
        assert_eq!(backward.face.normal_texel(), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn v_axis_crossings_produce_v_walls() {
        let values = [1.0, 1.0, 0.0, 0.0];
        let f = field(2, 2, &values);
        let hit = march_surface_depth(&f, [0.25, 0.25], [0.0, 25.0], 0.02, 24);
        assert_eq!(hit.face, SurfaceDepthFace::NegV);
        assert_eq!(hit.face.normal_texel(), [0.0, -1.0, 0.0]);
    }

    #[test]
    fn the_carve_never_exceeds_the_material_depth() {
        // Inward-only is the load-bearing property: the displaced surface must
        // never leave its real plane, so collision stays correct for free.
        let values = [1.0, 0.9, 0.8, 1.0, 0.7, 1.0, 0.95, 0.6, 1.0];
        let f = field(3, 3, &values);
        let depth_scale = 0.02;
        for i in 0..40 {
            let angle = i as f32 * 0.157;
            let dir = [angle.cos() * 40.0, angle.sin() * 40.0];
            let hit = march_surface_depth(&f, [0.4, 0.6], dir, depth_scale, 24);
            assert!(
                hit.depth_meters >= 0.0 && hit.depth_meters <= depth_scale + 1e-6,
                "depth {} outside [0, {depth_scale}]",
                hit.depth_meters
            );
        }
    }

    /// A starved march must sample inside the region it actually walked.
    ///
    /// Budget exhaustion used to resolve as a TOP hit at the current texel's
    /// full `solid` depth. The sample point is `p0 + dir * depth` and `dir` is
    /// texels per meter of DESCENT, so at a grazing angle that landed the
    /// albedo, normal and specular samples tens of texels past anything the
    /// march had visited — so a tighter per-material step cap made the artifact
    /// larger rather than smaller.
    #[test]
    fn a_starved_march_samples_inside_the_walked_region() {
        let values = [0.4, 0.9, 0.2, 1.0, 0.55, 0.05, 0.7, 0.3, 0.85];
        let f = field(3, 3, &values);
        let uv0 = [0.13, 0.77];
        let dims = [f.width as f32, f.height as f32];
        let budget = 12u32;
        let mut starved = 0u32;
        for i in 0..200 {
            let angle = i as f32 * 0.0314;
            // Deliberately grazing: thousands of texels per meter of descent,
            // so a full-depth resolve would be ~100 texels out on a 3x3 field.
            let dir = [angle.cos() * 5000.0, angle.sin() * 5000.0];
            let hit = march_surface_depth(&f, uv0, dir, 0.02, budget);
            // The DDA advances one axis per step, so after `steps` crossings the
            // hit is at most that many texels away in L1, plus the partial cell
            // a legitimate top hit resolves inside.
            let away = (hit.march_uv[0] - uv0[0]).abs() * dims[0]
                + (hit.march_uv[1] - uv0[1]).abs() * dims[1];
            assert!(
                away <= hit.steps as f32 + 2.0,
                "sampled {away} texels away after {} steps (budget {budget})",
                hit.steps
            );
            if hit.steps + 1 >= budget {
                starved += 1;
            }
        }
        assert!(
            starved > 0,
            "no ray in the sweep exhausted the budget, so this proves nothing"
        );
    }

    /// A starved march must not invent a face the field does not contain.
    ///
    /// Budget exhaustion used to resolve with the ENTRY face — the wall of the
    /// boundary just crossed. But the loop only reaches the budget test after
    /// BOTH hit rules failed, which means the ray entered this cell ABOVE its
    /// solid: there is no wall at that depth to report. On a uniform field,
    /// which has no side walls anywhere, it still returned one.
    ///
    /// A phantom side face is not cosmetic. `hit_top` goes false, so the
    /// consumer drops the normal map, engages the geometric-plane light gate,
    /// and starts a self-shadow march from a point in open air — all across a
    /// view-angle isoline that sweeps as the camera turns.
    #[test]
    fn a_starved_march_reports_no_face_the_field_does_not_have() {
        // Uniform field: every texel carves to full depth, so the only face
        // anywhere in it is the top.
        let values = [1.0f32; 9];
        let f = field(3, 3, &values);
        let hit = march_surface_depth(&f, [0.13, 0.77], [5000.0, 0.0], 0.02, 8);
        assert!(
            hit.steps + 1 >= 8,
            "the setup must actually starve the march; it walked {}",
            hit.steps
        );
        assert!(
            hit.face.is_top(),
            "a uniform field has only top faces; the march reported {:?}",
            hit.face
        );
        assert_eq!(
            hit.uv, hit.march_uv,
            "a top resolution carries no side bias, so the sample UV is the hit UV",
        );
    }

    #[test]
    fn the_march_terminates_within_the_step_budget_at_any_angle() {
        let values = [0.4, 0.9, 0.2, 1.0, 0.55, 0.05, 0.7, 0.3, 0.85];
        let f = field(3, 3, &values);
        for i in 0..200 {
            let angle = i as f32 * 0.0314;
            // Deliberately grazing: thousands of texels per meter of descent.
            let dir = [angle.cos() * 5000.0, angle.sin() * 5000.0];
            let hit = march_surface_depth(&f, [0.13, 0.77], dir, 0.02, 12);
            assert!(
                hit.steps < 12,
                "walked {} texels with a budget of 12",
                hit.steps
            );
            assert!(hit.depth_meters.is_finite());
        }
    }

    #[test]
    fn a_flush_texel_stops_the_ray_at_the_plane() {
        // D == 0 on the entry texel: z_enter (0) >= D (0) fires the side rule,
        // but the first texel's entry face is the plane itself, so this must
        // resolve as a geometric-normal top hit at depth 0.
        let values = [0.0, 1.0, 1.0, 1.0];
        let f = field(2, 2, &values);
        let hit = march_surface_depth(&f, [0.25, 0.25], [25.0, 0.0], 0.02, 24);
        assert_eq!(hit.depth_meters, 0.0);
        assert_eq!(hit.face, SurfaceDepthFace::Top);
    }

    #[test]
    fn a_zero_step_budget_still_resolves_a_hit() {
        let values = [0.5, 0.5, 0.5, 0.5];
        let f = field(2, 2, &values);
        let hit = march_surface_depth(&f, [0.25, 0.25], [25.0, 0.0], 0.02, 0);
        assert!(hit.depth_meters.is_finite());
        assert!(hit.depth_meters <= 0.02 + 1e-6);
    }

    // -- Self-shadowing --

    #[test]
    fn a_pit_floor_is_shadowed_by_the_wall_beside_it() {
        // Texel (0,0) is a pit at full depth, (1,0) is flush stone. A light
        // arriving from +u must be blocked by (1,0)'s wall.
        let values = [1.0, 0.0, 1.0, 0.0];
        let f = field(2, 2, &values);
        let depth_scale = 0.02;
        let visibility = surface_depth_light_visibility(
            &f,
            [0.25, 0.25],
            depth_scale,
            [0.5 / depth_scale, 0.0],
            depth_scale,
            12,
        );
        assert_eq!(visibility, 0.0);
    }

    #[test]
    fn a_pit_floor_is_lit_when_the_light_clears_the_wall() {
        // Same geometry, but the light is steep enough that the ray rises out
        // of the band before it reaches the neighbouring stone.
        let values = [1.0, 0.0, 1.0, 0.0];
        let f = field(2, 2, &values);
        let depth_scale = 0.02;
        let visibility = surface_depth_light_visibility(
            &f,
            [0.25, 0.25],
            depth_scale,
            [0.05 / depth_scale, 0.0],
            depth_scale,
            12,
        );
        assert_eq!(visibility, 1.0);
    }

    #[test]
    fn a_top_hit_never_shadows_itself() {
        let values = [0.5; 4];
        let f = field(2, 2, &values);
        let depth_scale = 0.02;
        for i in 0..32 {
            let angle = i as f32 * 0.196;
            let visibility = surface_depth_light_visibility(
                &f,
                [0.25, 0.25],
                0.5 * depth_scale,
                [angle.cos() * 30.0, angle.sin() * 30.0],
                depth_scale,
                12,
            );
            assert_eq!(visibility, 1.0, "flat plateau must be fully lit");
        }
    }

    #[test]
    fn a_hit_on_the_plane_skips_the_shadow_march() {
        let values = [1.0; 4];
        let f = field(2, 2, &values);
        assert_eq!(
            surface_depth_light_visibility(&f, [0.25, 0.25], 0.0, [30.0, 0.0], 0.02, 12),
            1.0
        );
    }

    // -- Fade, AO, basis --

    #[test]
    fn distance_fade_reaches_zero_at_the_material_fade_distance() {
        assert_eq!(surface_depth_distance_fade(0.0, 10.0), 1.0);
        assert_eq!(surface_depth_distance_fade(7.5, 10.0), 1.0);
        assert!((surface_depth_distance_fade(8.75, 10.0) - 0.5).abs() < 1e-6);
        assert_eq!(surface_depth_distance_fade(10.0, 10.0), 0.0);
        assert_eq!(surface_depth_distance_fade(50.0, 10.0), 0.0);
    }

    #[test]
    fn a_zero_fade_distance_is_always_flat() {
        assert_eq!(surface_depth_distance_fade(0.0, 0.0), 0.0);
    }

    #[test]
    fn lod_fade_flattens_once_texels_go_sub_pixel() {
        assert_eq!(surface_depth_lod_fade(0.0), 1.0);
        assert_eq!(surface_depth_lod_fade(SURFACE_DEPTH_FADE_LOD_START), 1.0);
        assert_eq!(
            surface_depth_lod_fade(SURFACE_DEPTH_FADE_LOD_START + SURFACE_DEPTH_FADE_LOD_RANGE),
            0.0
        );
        assert!((surface_depth_lod_fade(2.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn lod_is_measured_in_base_mip_texels() {
        // One texel per pixel is lod 0; a chain streamed down to half
        // dimensions reads one lod lower and therefore fades later.
        let lod_full =
            surface_depth_lod([1.0 / 1024.0, 0.0], [0.0, 1.0 / 1024.0], [1024.0, 1024.0]);
        assert!(lod_full.abs() < 1e-5);
        let lod_streamed =
            surface_depth_lod([1.0 / 1024.0, 0.0], [0.0, 1.0 / 1024.0], [512.0, 512.0]);
        assert!((lod_streamed + 1.0).abs() < 1e-5);
        assert!(surface_depth_lod_fade(lod_streamed) >= surface_depth_lod_fade(lod_full));
    }

    #[test]
    fn the_combined_fade_takes_the_stricter_of_the_two() {
        assert_eq!(surface_depth_fade(0.0, 10.0, 9.0), 0.0);
        assert_eq!(surface_depth_fade(50.0, 10.0, 0.0), 0.0);
        assert_eq!(surface_depth_fade(0.0, 10.0, 0.0), 1.0);
    }

    /// A non-finite authored depth must never reach the GPU.
    ///
    /// `is_enabled()` is `> 0.0`, which rejects NaN and zero but ADMITS `+inf`.
    /// An infinite scale makes the march's `solid` a NaN on every zero-depth
    /// texel, and a NaN compares false against both hit rules.
    #[test]
    fn a_non_finite_authored_depth_resolves_flat() {
        for bad in [f32::INFINITY, f32::NAN] {
            let depth = SurfaceDepth {
                depth_meters: bad,
                quantize_levels: 8,
                max_steps: 16,
                fade_distance_meters: 10.0,
            };
            let resolved = SurfaceDepthUniform::resolve(depth, SurfaceDepthQuality::On, true, 4, 0);
            assert_eq!(
                resolved,
                SurfaceDepthUniform::FLAT,
                "an authored depth of {bad} must resolve flat, not reach the shader",
            );
        }
    }

    /// The ceiling is a real constraint on the value that reaches the GPU, not
    /// an assertion about one hardcoded table.
    #[test]
    fn an_over_deep_authored_depth_is_clamped_to_the_ceiling() {
        let ceiling = surface_depth_max_authored();
        let depth = SurfaceDepth {
            depth_meters: ceiling * 10.0,
            quantize_levels: 8,
            max_steps: 16,
            fade_distance_meters: 10.0,
        };
        let resolved = SurfaceDepthUniform::resolve(depth, SurfaceDepthQuality::On, true, 4, 0);
        assert!(
            resolved.depth.depth_meters <= ceiling,
            "{} exceeds the ceiling {ceiling}",
            resolved.depth.depth_meters,
        );
        // Still carving — the clamp bounds the depth, it does not disable it.
        assert!(resolved.has_depth);
    }

    #[test]
    fn ambient_occlusion_darkens_only_with_depth() {
        assert_eq!(surface_depth_ambient_occlusion(0.0, 0.02, 1.0), 1.0);
        assert!(
            (surface_depth_ambient_occlusion(0.02, 0.02, 1.0)
                - (1.0 - SURFACE_DEPTH_AO_STRENGTH))
                .abs()
                < 1e-6
        );
        // A flat material can never darken anything.
        assert_eq!(surface_depth_ambient_occlusion(0.0, 0.0, 1.0), 1.0);
    }

    /// The occlusion must reach its flat value CONTINUOUSLY as the fade closes.
    ///
    /// Both depth arguments are post-fade, so their ratio is fade-invariant: a
    /// fragment at the deepest texel reports the same ratio at every fade. Left
    /// unscaled, occlusion stayed at full strength right up to the boundary and
    /// then snapped to 1.0 when the resolve returned the flat result — up to a
    /// 4x step in the indirect term, sweeping across the floor with the camera
    /// because the LOD half of the fade is a moving isoline.
    #[test]
    fn ambient_occlusion_fades_out_with_the_carve() {
        let full = surface_depth_ambient_occlusion(0.02, 0.02, 1.0);
        assert!(full < 1.0, "a fully faded-in deep hit must occlude");

        // Walking the fade to zero must walk the occlusion to 1.0, monotonically.
        let mut previous = full;
        for step in 1..=10u8 {
            let fade = 1.0 - f32::from(step) / 10.0_f32;
            // Post-fade inputs: the hit stays at the bottom of a shallower carve.
            let scale = 0.02 * fade;
            let ao = surface_depth_ambient_occlusion(scale, scale, fade);
            // STRICTLY weaker. `>=` passes on a CONSTANT function, which is
            // exactly what the unfaded formula was: every iteration returned
            // the same 0.25 and this loop saw nothing wrong.
            assert!(
                ao > previous,
                "occlusion must weaken STRICTLY as the carve fades: {ao} is not above {previous} at fade {fade}"
            );
            previous = ao;
        }

        // And it must ARRIVE at the flat value, not merely approach it, so there
        // is no step where the resolve hands off to `surface_depth_flat`.
        assert_eq!(surface_depth_ambient_occlusion(0.0, 0.0, 0.0), 1.0);
        let nearly_gone = surface_depth_ambient_occlusion(0.02 * 1e-4, 0.02 * 1e-4, 1e-4);
        assert!(
            (nearly_gone - 1.0).abs() < 1e-3,
            "occlusion at the fade boundary must be within a hair of flat, got {nearly_gone}"
        );
    }

    #[test]
    fn the_basis_recovers_world_units_per_uv_unit() {
        // A face whose texture spans 2 m per UV unit along u and 4 m along v.
        let normal = Vec3::Z;
        let basis = surface_depth_basis(
            [0.01, 0.0],
            [0.0, 0.005],
            Vec3::new(0.02, 0.0, 0.0),
            Vec3::new(0.0, 0.02, 0.0),
            normal,
        )
        .expect("well-formed derivatives must yield a basis");
        assert!((basis.uv_per_meter[0] - 0.5).abs() < 1e-5);
        assert!((basis.uv_per_meter[1] - 0.25).abs() < 1e-5);
        assert!((basis.tangent - Vec3::X).length() < 1e-5);
        assert!((basis.bitangent - Vec3::Y).length() < 1e-5);
    }

    #[test]
    fn a_close_range_fragment_keeps_its_basis() {
        // Regression: the determinant is a PRODUCT of two UV derivatives, so at
        // close range on a 1k texture it lands near 1e-9. A floor of
        // SURFACE_DEPTH_EPS there would have killed the effect exactly when the
        // player is nose-to-the-wall and it matters most.
        let du = 5.0e-5_f32;
        let basis = surface_depth_basis(
            [du, 0.0],
            [0.0, du],
            Vec3::new(1.0e-4, 0.0, 0.0),
            Vec3::new(0.0, 1.0e-4, 0.0),
            Vec3::Z,
        );
        assert!(
            basis.is_some(),
            "a determinant of {} must not read as a singular chart",
            du * du
        );
    }

    #[test]
    fn a_nan_derivative_yields_no_basis() {
        assert!(
            surface_depth_basis(
                [f32::NAN, 0.0],
                [0.0, 0.01],
                Vec3::new(0.02, 0.0, 0.0),
                Vec3::new(0.0, 0.02, 0.0),
                Vec3::Z,
            )
            .is_none()
        );
        assert!(
            surface_depth_basis(
                [0.01, 0.0],
                [0.0, 0.01],
                Vec3::new(f32::NAN, 0.0, 0.0),
                Vec3::new(0.0, 0.02, 0.0),
                Vec3::Z,
            )
            .is_none()
        );
    }

    #[test]
    fn a_collapsed_uv_chart_yields_no_basis() {
        assert!(
            surface_depth_basis(
                [0.0, 0.0],
                [0.0, 0.0],
                Vec3::new(0.02, 0.0, 0.0),
                Vec3::new(0.0, 0.02, 0.0),
                Vec3::Z,
            )
            .is_none()
        );
    }

    #[test]
    fn an_edge_on_fragment_has_no_view_ray() {
        let basis = SurfaceDepthBasis {
            tangent: Vec3::X,
            bitangent: Vec3::Y,
            normal: Vec3::Z,
            uv_per_meter: [1.0, 1.0],
        };
        assert!(surface_depth_view_ray(&basis, Vec3::X).is_none());
        assert!(surface_depth_view_ray(&basis, -Vec3::Z).is_none());
        assert!(surface_depth_view_ray(&basis, Vec3::Z).is_some());
        // A sliver below the cosine floor is dropped rather than marched with a
        // ray that travels kilometres sideways per metre of depth.
        let sliver = Vec3::new(1.0, 0.0, SURFACE_DEPTH_MIN_DESCENT * 0.5).normalize();
        assert!(surface_depth_view_ray(&basis, sliver).is_none());
    }

    #[test]
    fn a_head_on_view_ray_does_not_shift_uv() {
        let basis = SurfaceDepthBasis {
            tangent: Vec3::X,
            bitangent: Vec3::Y,
            normal: Vec3::Z,
            uv_per_meter: [2.0, 2.0],
        };
        let dir = surface_depth_view_ray(&basis, Vec3::Z).expect("head-on ray");
        assert_eq!(dir, [0.0, 0.0]);
    }

    #[test]
    fn a_grazing_view_ray_advances_uv_faster_than_a_steep_one() {
        let basis = SurfaceDepthBasis {
            tangent: Vec3::X,
            bitangent: Vec3::Y,
            normal: Vec3::Z,
            uv_per_meter: [1.0, 1.0],
        };
        let steep = surface_depth_view_ray(&basis, Vec3::new(0.2, 0.0, 1.0).normalize()).unwrap();
        let grazing = surface_depth_view_ray(&basis, Vec3::new(4.0, 0.0, 1.0).normalize()).unwrap();
        assert!(grazing[0].abs() > steep[0].abs());
    }

    #[test]
    fn a_light_below_the_plane_has_no_shadow_ray() {
        let basis = SurfaceDepthBasis {
            tangent: Vec3::X,
            bitangent: Vec3::Y,
            normal: Vec3::Z,
            uv_per_meter: [1.0, 1.0],
        };
        assert!(surface_depth_light_ray(&basis, -Vec3::Z).is_none());
        assert!(surface_depth_light_ray(&basis, Vec3::Z).is_some());
    }

    // -- Texel→meters conversion (GPU-only; mirrored here for one assertion) --

    /// Reproduces `surface_depth.wgsl`'s texel→meters conversion and its
    /// `SURFACE_DEPTH_MAX_METERS` clamp (search that name in the WGSL file,
    /// currently the `depth_scale_m` derivation a few lines above the
    /// `SURFACE_DEPTH_MIN_DESCENT` check). This is test-local scaffolding, not
    /// the CPU mirror the module header explains the owner declined to build —
    /// it exists only so the regression test below can assert the property the
    /// shader's clamp defends, since `march_surface_depth` never derives this
    /// value itself (it takes `depth_scale_meters` as a parameter).
    #[cfg(test)]
    fn shader_depth_scale_m(carve_request_texels: f32, texels_per_m: [f32; 2], fade: f32) -> f32 {
        let texel_rate = (texels_per_m[0] * texels_per_m[1])
            .max(SURFACE_DEPTH_EPS)
            .sqrt();
        let depth_scale_m = carve_request_texels / texel_rate;
        depth_scale_m.min(SURFACE_DEPTH_MAX_METERS * fade)
    }

    /// Regression: in texel mode the authored ceiling (`SURFACE_DEPTH_MAX_TEXELS`)
    /// bounds a TEXEL COUNT, not meters. Nothing re-imposed the meters ceiling
    /// after the shader's per-fragment divide by the texel rate, so a
    /// coarsely-scaled face (few texels per meter) resolved an unbounded depth.
    /// Fixed by clamping `depth_scale_m` to `SURFACE_DEPTH_MAX_METERS * fade`
    /// in `surface_depth.wgsl` after the divide; this asserts the clamped
    /// result never exceeds the meters ceiling, for a range of plausible texel
    /// rates including a coarsely-scaled face (a 128px texture tiled over 4 m,
    /// giving ~32 texels/m).
    #[test]
    fn deepest_authored_texel_count_resolves_within_the_meters_ceiling_at_any_texel_rate() {
        assert!(
            surface_depth_is_texel_relative(),
            "this regression is specific to texel-relative authoring; revisit \
             if SURFACE_DEPTH_TEXEL_MODE ever flips back to meters",
        );
        let deepest_authored = surface_depth_max_authored(); // texels, in TEXEL mode
        // `surface_depth_max_authored()` reads texels in this mode; the meters
        // ceiling the clamp defends is the other branch of the same function,
        // `SURFACE_DEPTH_MAX_METERS` itself (read from source, not restated).
        let meters_ceiling = SURFACE_DEPTH_MAX_METERS;
        let fade = 1.0;

        // Plausible per-axis texel rates, from a coarsely-scaled face (a 128px
        // texture tiled over 4 m, ~32 texels/m — the case that actually
        // triggered the unbounded depth) up through finely-tiled walls.
        for texel_rate_axis in [1.0_f32, 4.0, 8.0, 16.0, 32.0, 128.0, 512.0, 4096.0] {
            let texels_per_m = [texel_rate_axis, texel_rate_axis];
            let depth_scale_m = shader_depth_scale_m(deepest_authored, texels_per_m, fade);
            assert!(
                depth_scale_m <= meters_ceiling + 1e-6,
                "deepest authored value ({deepest_authored} texels) at {texel_rate_axis} \
                 texels/m resolved to {depth_scale_m} m, above the {meters_ceiling} m ceiling",
            );
        }

        // Anisotropic scaling: the shader's rate is the geometric mean of the
        // two axes, so a face that is finely tiled on one axis and coarsely on
        // the other must still clamp.
        let depth_scale_m = shader_depth_scale_m(deepest_authored, [2.0, 4096.0], fade);
        assert!(
            depth_scale_m <= meters_ceiling + 1e-6,
            "anisotropic face resolved to {depth_scale_m} m, above the {meters_ceiling} m ceiling",
        );
    }
}
