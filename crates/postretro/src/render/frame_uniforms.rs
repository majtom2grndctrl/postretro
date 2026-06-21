// The shared group-0 frame uniform layout: FrameUniforms, its byte packer, the
// lighting/shadow isolation enums, and the uniform-size/flag constants.
// See: context/lib/rendering_pipeline.md §4

use super::*;

pub(crate) const UNIFORM_SIZE: usize = 128;

/// Bit 0 of `Uniforms.sdf_shadow_flags` — an SDF atlas is loaded, so the
/// half-res factor target holds valid per-light visibility slices and the
/// forward should sample (bilateral-upsample) it. When clear (legacy PRL / no
/// SDF atlas) the forward skips the upsample and per-light visibility defaults
/// to fully lit. The per-light slices (R/G/B/A) are read directly via
/// `slice_for_visibility`; they are not individually flag-gated.
pub const SDF_SHADOW_FLAG_ATLAS_PRESENT: u32 = 1 << 0;

/// Debug selector for the SDF shadow path. Mirrors the `LightingIsolation`
/// pattern: panel-only dropdown, encoded into the per-frame uniform.
///
/// - `On` applies the per-light SDF visibility multiply normally (gated on the
///   atlas-present flag, `SDF_SHADOW_FLAG_ATLAS_PRESENT`).
/// - `Off` forces per-light SDF visibility to 1.0 (no SDF factor applied).
///   Shadow-map (enemy) shadows are unaffected — they don't run through the SDF
///   multiply in the first place.
/// - `Visualize` replaces the shaded fragment color with a grayscale view of
///   the per-light slice 0 (R channel) shadow factor — interpretable for
///   spotting artifacts without needing a separate march-step heatmap binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
#[repr(u32)]
pub enum SdfShadowMode {
    On = 0,
    Off = 1,
    Visualize = 2,
    // TEMP DEBUG: SDF shadow path visualization. Encodes the per-pixel OUTCOME
    // of the primary (slot 0) light's `trace_shadow` as an RGB code instead of a
    // visibility float, displayed directly (no bilateral upsample). Diagnostic
    // only — remove with the rest of the `// TEMP DEBUG:` markers.
    VisualizeDebugPaths = 3,
    // TEMP DEBUG: SDF shadow path visualization. Encodes the reconstructed
    // GEOMETRIC SURFACE NORMAL (the exact `reconstruct_normal` result the
    // normal-offset shadow fix marches from) as RGB = normal*0.5+0.5, displayed
    // directly (no bilateral upsample). Lets us confirm the reconstructed normal
    // is sane at edges/corners vs garbage. Diagnostic only — remove with the
    // rest of the `// TEMP DEBUG:` markers.
    VisualizeNormals = 4,
}

impl SdfShadowMode {
    /// All variants in display order. Used by the debug UI dropdown.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub const ALL_VARIANTS: [SdfShadowMode; 5] = [
        SdfShadowMode::On,
        SdfShadowMode::Off,
        SdfShadowMode::Visualize,
        // TEMP DEBUG: SDF shadow path visualization.
        SdfShadowMode::VisualizeDebugPaths,
        // TEMP DEBUG: SDF shadow path visualization.
        SdfShadowMode::VisualizeNormals,
    ];

    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            SdfShadowMode::On => "On",
            SdfShadowMode::Off => "Off",
            SdfShadowMode::Visualize => "Visualize",
            // TEMP DEBUG: SDF shadow path visualization.
            SdfShadowMode::VisualizeDebugPaths => "Visualize: debug paths",
            // TEMP DEBUG: SDF shadow path visualization.
            SdfShadowMode::VisualizeNormals => "Visualize: normals",
        }
    }
}

/// Lighting-term isolation mode for leak/bleed debugging.
/// The ambient floor always contributes so interior geometry is never pitch black.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Variants beyond Normal are selected via the Diagnostics panel dropdown (dev-tools feature).
// The keyboard cycle chord was removed; the panel is the only trigger.
#[allow(dead_code)]
#[repr(u32)]
pub enum LightingIsolation {
    Normal = 0,
    NoLightmap = 1,
    DirectOnly = 2,
    IndirectOnly = 3,
    AmbientOnly = 4,
    LightmapOnly = 5,
    StaticSHOnly = 6,
    AnimatedDeltaOnly = 7,
    DynamicOnly = 8,
    SpecularOnly = 9,
}

impl LightingIsolation {
    /// All variants in display order. Used by the debug UI dropdown.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub const ALL_VARIANTS: [LightingIsolation; 10] = [
        LightingIsolation::Normal,
        LightingIsolation::NoLightmap,
        LightingIsolation::DirectOnly,
        LightingIsolation::IndirectOnly,
        LightingIsolation::AmbientOnly,
        LightingIsolation::LightmapOnly,
        LightingIsolation::StaticSHOnly,
        LightingIsolation::AnimatedDeltaOnly,
        LightingIsolation::DynamicOnly,
        LightingIsolation::SpecularOnly,
    ];

    #[allow(dead_code)]
    pub fn cycle(self) -> Self {
        match self {
            LightingIsolation::Normal => LightingIsolation::NoLightmap,
            LightingIsolation::NoLightmap => LightingIsolation::DirectOnly,
            LightingIsolation::DirectOnly => LightingIsolation::IndirectOnly,
            LightingIsolation::IndirectOnly => LightingIsolation::AmbientOnly,
            LightingIsolation::AmbientOnly => LightingIsolation::LightmapOnly,
            LightingIsolation::LightmapOnly => LightingIsolation::StaticSHOnly,
            LightingIsolation::StaticSHOnly => LightingIsolation::AnimatedDeltaOnly,
            LightingIsolation::AnimatedDeltaOnly => LightingIsolation::DynamicOnly,
            LightingIsolation::DynamicOnly => LightingIsolation::SpecularOnly,
            LightingIsolation::SpecularOnly => LightingIsolation::Normal,
        }
    }

    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            LightingIsolation::Normal => "Normal (all terms)",
            LightingIsolation::NoLightmap => "NoLightmap (all terms except static lightmap)",
            LightingIsolation::DirectOnly => "DirectOnly (lightmap + dynamic + specular)",
            LightingIsolation::IndirectOnly => "IndirectOnly (SH + specular)",
            LightingIsolation::AmbientOnly => "AmbientOnly (ambient floor only)",
            LightingIsolation::LightmapOnly => "LightmapOnly (static lightmap)",
            LightingIsolation::StaticSHOnly => "StaticSHOnly (static SH indirect)",
            LightingIsolation::AnimatedDeltaOnly => "AnimatedDeltaOnly (animated SH delta)",
            LightingIsolation::DynamicOnly => "DynamicOnly (dynamic direct lights)",
            LightingIsolation::SpecularOnly => "SpecularOnly (specular only)",
        }
    }
}

/// Isolation mode for the DYNAMIC (entity / billboard) baked-static-direct SH
/// path. Separate, 3-state instrument — NOT the 10-variant `LightingIsolation`
/// (which controls the forward/static pass and stays independent so the
/// dynamic-vs-static parity comparison still works). Encoded as the enum's
/// `u32` repr into the mesh `DynamicDirectParams` uniform (binding 16) and the
/// tail of the group-0 `Uniforms` (billboard).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Selected via the Diagnostics panel; dev-tools only.
#[allow(dead_code)]
#[repr(u32)]
pub enum DynamicDirectIsolation {
    /// indirect + scale * direct.
    Combined = 0,
    /// scale * direct only.
    DirectOnly = 1,
    /// indirect only (direct suppressed).
    IndirectOnly = 2,
}

impl DynamicDirectIsolation {
    /// All variants in display order. Used by the debug UI dropdown.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub const ALL_VARIANTS: [DynamicDirectIsolation; 3] = [
        DynamicDirectIsolation::Combined,
        DynamicDirectIsolation::DirectOnly,
        DynamicDirectIsolation::IndirectOnly,
    ];

    #[allow(dead_code)]
    pub fn cycle(self) -> Self {
        match self {
            DynamicDirectIsolation::Combined => DynamicDirectIsolation::DirectOnly,
            DynamicDirectIsolation::DirectOnly => DynamicDirectIsolation::IndirectOnly,
            DynamicDirectIsolation::IndirectOnly => DynamicDirectIsolation::Combined,
        }
    }

    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            DynamicDirectIsolation::Combined => "Combined (indirect + scale·direct)",
            DynamicDirectIsolation::DirectOnly => "DirectOnly (scale·direct)",
            DynamicDirectIsolation::IndirectOnly => "IndirectOnly (indirect)",
        }
    }
}

pub(crate) struct FrameUniforms {
    pub(crate) view_proj: Mat4,
    pub(crate) camera_position: Vec3,
    pub(crate) ambient_floor: f32,
    pub(crate) light_count: u32,
    pub(crate) time: f32,
    pub(crate) lighting_isolation: LightingIsolation,
    pub(crate) indirect_scale: f32,
    /// Bitset of `SDF_SHADOW_FLAG_*` controlling the forward shader's SDF
    /// shadow-factor multiplies. Bit 0 gates the animated-baked term; bit 1
    /// gates the static-lightmap term (independent because the static-term
    /// multiply must skip a shadowed-mode lightmap to avoid double shadows).
    pub(crate) sdf_shadow_flags: u32,
    /// `SdfShadowMode` debug selector (Task 6). Encoded as the enum's `u32`
    /// repr (0=On, 1=Off, 2=Visualize). Overlays the per-term flags above:
    /// `Off` forces both SDF multiplies to 1.0; `Visualize` replaces the
    /// shaded color output with a grayscale R-channel view.
    pub(crate) sdf_shadow_mode: SdfShadowMode,
    /// Dev toggle: force per-light SDF visibility to 1.0 in the forward shader.
    /// Drives the "no double-count" visual A/B — with every sdf light fully
    /// lit, the additive per-light diffuse must reproduce the pre-change
    /// render (disjoint sets guarantee no re-weighting). Encoded as a u32
    /// (0 = normal, non-zero = forced) into the uniform's first pad slot.
    pub(crate) sdf_force_visibility_one: bool,
    /// DYNAMIC baked-static-direct SH scale (0..1). Multiplies the direct term
    /// for the billboard path (the mesh path reads its own copy from the
    /// group-4 `DynamicDirectParams`). Repurposes the former `_sdf_pad1` slot.
    pub(crate) dynamic_direct_scale: f32,
    /// DYNAMIC-direct isolation mode (billboard path). Separate from
    /// `lighting_isolation`. Lands in a fresh trailing 16-byte row.
    pub(crate) dynamic_direct_isolation: DynamicDirectIsolation,
    /// Whether a baked DIRECT SH section is present. When false the dynamic
    /// shaders skip the direct sample (direct = 0), falling back to
    /// indirect-only. Owned here (and mirrored in the mesh uniform).
    pub(crate) has_direct: bool,
}

pub(crate) fn build_uniform_data(u: &FrameUniforms) -> [u8; UNIFORM_SIZE] {
    let mut bytes = [0u8; UNIFORM_SIZE];
    let cols = u.view_proj.to_cols_array();
    for (i, val) in cols.iter().enumerate() {
        let off = i * 4;
        bytes[off..off + 4].copy_from_slice(&val.to_ne_bytes());
    }
    bytes[64..68].copy_from_slice(&u.camera_position.x.to_ne_bytes());
    bytes[68..72].copy_from_slice(&u.camera_position.y.to_ne_bytes());
    bytes[72..76].copy_from_slice(&u.camera_position.z.to_ne_bytes());
    bytes[76..80].copy_from_slice(&u.ambient_floor.to_ne_bytes());
    bytes[80..84].copy_from_slice(&u.light_count.to_ne_bytes());
    bytes[84..88].copy_from_slice(&u.time.to_ne_bytes());
    let isolation: u32 = u.lighting_isolation as u32;
    bytes[88..92].copy_from_slice(&isolation.to_ne_bytes());
    bytes[92..96].copy_from_slice(&u.indirect_scale.to_ne_bytes());
    bytes[96..100].copy_from_slice(&u.sdf_shadow_flags.to_ne_bytes());
    let mode: u32 = u.sdf_shadow_mode as u32;
    bytes[100..104].copy_from_slice(&mode.to_ne_bytes());
    let force_vis: u32 = u.sdf_force_visibility_one as u32;
    bytes[104..108].copy_from_slice(&force_vis.to_ne_bytes());
    bytes[108..112].copy_from_slice(&u.dynamic_direct_scale.to_ne_bytes());
    let dyn_iso: u32 = u.dynamic_direct_isolation as u32;
    bytes[112..116].copy_from_slice(&dyn_iso.to_ne_bytes());
    let has_direct: u32 = u.has_direct as u32;
    bytes[116..120].copy_from_slice(&has_direct.to_ne_bytes());
    // 120..128 stays zero — explicit pad rounding the tail row to 16 bytes.
    bytes
}
