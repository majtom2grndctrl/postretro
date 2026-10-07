//! The per-material uniform: the packed `march` word, the player's on/off
//! switch, and the resolve that decides what one material uploads.

use super::height::SurfaceRelief;
use postretro_render_data::material::{SurfaceDepth, surface_depth_max_authored};

/// Uniform-word bit offset of the packed base mip level.
pub const SURFACE_DEPTH_BASE_MIP_SHIFT: u32 = 8;
/// Bit width (and therefore ceiling) of the packed base mip level.
pub const SURFACE_DEPTH_BASE_MIP_MASK: u32 = 0xF;
/// Packed flag: the bound specular slot really is a two-channel surface map.
///
/// This bit is the ONLY guard against the R8 placeholder. That placeholder
/// reads `g = 0`, which under the signed encoding is maximum RAISE, not flat —
/// a march that ran on it would lift the whole face by its full depth.
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

/// Per-fragment cap on dynamic-light self-shadow marches while the feature is
/// `On`. `Off` drops it to zero — see [`SurfaceDepthQuality`].
pub const SURFACE_DEPTH_SHADOW_LIGHT_BUDGET: u32 = 2;

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

/// Whether the shader marches this material at all: the has-depth bit is set
/// and the authored depth is positive. Mirrors WGSL `surface_depth_has_map`.
///
/// Nothing else may stand in for the bit. A non-surface-map slot reads `g = 0`,
/// which is maximum raise.
pub fn surface_depth_has_map(march_word: u32, depth_meters: f32) -> bool {
    (march_word & SURFACE_DEPTH_HAS_DEPTH_BIT) != 0 && depth_meters > 0.0
}

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
    /// the uniform's surface-depth bytes are the historical all-zero and the
    /// render is byte-identical to the pre-Surface-Depth engine at zero cost.
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

/// Everything the bind-group builder must decide about one material's march.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceDepthUniform {
    pub depth: SurfaceDepth,
    /// Set from the *loaded* specular slot's format, not from the material
    /// prefix: a prefix that wants depth but whose `.prm` has no `_h` sibling
    /// must skip the march rather than walk the placeholder.
    pub has_depth: bool,
    /// Resident base mip the DDA reads at (D6.2).
    pub base_mip: u32,
    /// Per-fragment dynamic-light self-shadow budget, from the player's
    /// on/off switch. Zero means the fragment never runs the shadow DDA.
    pub shadow_light_budget: u32,
    /// The material's relief band, QUANTIZED with its level count. These are
    /// the two floats at uniform bytes 8..16.
    pub relief: SurfaceRelief,
}

impl SurfaceDepthUniform {
    /// The flat case: no march, and the shader's has-depth branch is clear.
    pub const FLAT: Self = Self {
        depth: SurfaceDepth::FLAT,
        has_depth: false,
        base_mip: SURFACE_DEPTH_RESIDENT_BASE_MIP,
        shadow_light_budget: 0,
        relief: SurfaceRelief::FLAT,
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
    /// never `textureLoad` past the end of the uploaded chain. `relief` is the
    /// RAW band measured from that slot at load; it is quantized here with the
    /// material's level count. An empty band resolves flat (P4): an
    /// all-mid-gray map uploads exactly what no map does.
    ///
    /// The authored depth is also rejected if non-finite and clamped to
    /// [`surface_depth_max_authored`]. `SurfaceDepth::depth_meters` is a public
    /// field and `is_enabled()` only tests `> 0.0`, which admits `+inf`; an
    /// infinite scale turns every plane-height texel's `solid` into a NaN,
    /// which compares false against both hit rules. The march terminates on
    /// its integer budget regardless, so this is defence in depth rather than
    /// the only guard, but it keeps the ceiling a real constraint on the value
    /// reaching the GPU.
    pub fn resolve(
        depth: SurfaceDepth,
        quality: SurfaceDepthQuality,
        specular_is_surface_map: bool,
        specular_mip_count: u32,
        requested_base_mip: u32,
        relief: SurfaceRelief,
    ) -> Self {
        let mut tuned = quality.apply(depth);
        if !specular_is_surface_map || !tuned.is_enabled() {
            return Self::FLAT;
        }
        // `is_enabled()` rejects NaN, zero and negative but admits `+inf`.
        if !tuned.depth_meters.is_finite() {
            return Self::FLAT;
        }
        let band = relief.quantized(tuned.quantize_levels as f32);
        if band.is_flat() {
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
            relief: band,
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
