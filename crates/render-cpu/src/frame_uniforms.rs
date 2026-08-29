// Shared group-0 frame uniform layout, flags, and byte packing.
// See: context/lib/rendering_pipeline.md §4

use glam::{Mat4, Vec3};

pub const UNIFORM_SIZE: usize = 128;
pub const TOTAL_LIGHT_COUNT_OFFSET: u64 = 120;

/// Bit 0 of `Uniforms.sdf_shadow_flags` — an SDF atlas is loaded, so the
/// half-res factor target holds valid per-light visibility slices and the
/// forward should sample (bilateral-upsample) it. When clear (legacy PRL / no
/// SDF atlas) the forward skips the upsample and per-light visibility defaults
/// to fully lit. The per-light slices (R/G/B/A) are read directly via
/// `slice_for_visibility`; they are not individually flag-gated.
pub const SDF_SHADOW_FLAG_ATLAS_PRESENT: u32 = 1 << 0;

/// Debug selector for the SDF shadow path: panel-only dropdown, encoded into
/// the per-frame uniform.
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
    // Visualizes the static-light shadowmask world-receipt union subtraction
    // magnitude. Normal mode still applies the production subtraction.
    ShadowmaskUnion = 5,
    // Visualizes raw pool visibility for promoted static lights on world
    // receivers. The forward shader chooses the minimum across covering lights.
    ShadowmaskRawPoolVisibility = 6,
}

impl SdfShadowMode {
    /// All variants in display order. Used by the debug UI dropdown.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub const ALL_VARIANTS: [SdfShadowMode; 7] = [
        SdfShadowMode::On,
        SdfShadowMode::Off,
        SdfShadowMode::Visualize,
        // TEMP DEBUG: SDF shadow path visualization.
        SdfShadowMode::VisualizeDebugPaths,
        // TEMP DEBUG: SDF shadow path visualization.
        SdfShadowMode::VisualizeNormals,
        SdfShadowMode::ShadowmaskUnion,
        SdfShadowMode::ShadowmaskRawPoolVisibility,
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
            SdfShadowMode::ShadowmaskUnion => "Visualize: shadowmask union",
            SdfShadowMode::ShadowmaskRawPoolVisibility => "Visualize: shadowmask pool visibility",
        }
    }
}

/// Dev-tools-only gates for independently viewing the terms that light a
/// surface. Bit 7 remains reserved: emissive lights only their own material
/// and is intentionally outside this instrument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct LightTermMask(u32);

impl LightTermMask {
    pub const AMBIENT_FLOOR: Self = Self(1 << 0);
    pub const INDIRECT_STATIC: Self = Self(1 << 1);
    pub const INDIRECT_ANIMATED: Self = Self(1 << 2);
    pub const BAKED_DIRECT_STATIC: Self = Self(1 << 3);
    pub const BAKED_DIRECT_ANIMATED: Self = Self(1 << 4);
    pub const DYNAMIC_DIRECT: Self = Self(1 << 5);
    pub const SPECULAR: Self = Self(1 << 6);

    /// Every wired term. Bit 7 is reserved for the intentionally unwired
    /// emissive category and must not be included here.
    pub const ALL: Self = Self(0x7F);

    /// Terms in diagnostics display order.
    pub const ALL_TERMS: [Self; 7] = [
        Self::AMBIENT_FLOOR,
        Self::INDIRECT_STATIC,
        Self::INDIRECT_ANIMATED,
        Self::BAKED_DIRECT_STATIC,
        Self::BAKED_DIRECT_ANIMATED,
        Self::DYNAMIC_DIRECT,
        Self::SPECULAR,
    ];

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn contains(self, term: Self) -> bool {
        self.0 & term.0 != 0
    }

    pub fn set_enabled(&mut self, term: Self, enabled: bool) {
        if enabled {
            self.0 |= term.0;
        } else {
            self.0 &= !term.0;
        }
    }

    pub const fn label(self) -> &'static str {
        match self.0 {
            0x01 => "Ambient floor",
            0x02 => "Indirect — static",
            0x04 => "Indirect — animated",
            0x08 => "Baked direct — static",
            0x10 => "Baked direct — animated",
            0x20 => "Dynamic direct",
            0x40 => "Specular",
            _ => "Unknown lighting term",
        }
    }
}

impl Default for LightTermMask {
    fn default() -> Self {
        Self::ALL
    }
}

pub struct FrameUniforms {
    pub view_proj: Mat4,
    pub camera_position: Vec3,
    pub ambient_floor: f32,
    pub light_count: u32,
    pub time: f32,
    /// Bits 0..=6 are `LightTermMask`; byte 88..92 is a fixed group-0 ABI
    /// slot shared by the renderer and every shader mirror.
    pub light_term_mask: LightTermMask,
    pub indirect_scale: f32,
    /// Bitset of `SDF_SHADOW_FLAG_*` controlling the forward shader's SDF
    /// shadow path. Bit 0 (`SDF_SHADOW_FLAG_ATLAS_PRESENT`) marks a loaded
    /// atlas; when clear, the forward shader treats SDF visibility as fully lit.
    /// Other bits are unused.
    pub sdf_shadow_flags: u32,
    /// `SdfShadowMode` debug selector. Encoded as the enum's `u32` repr; keep
    /// the enum variants above as the source of truth for mode ids. Overlays
    /// the atlas-present flag above: `Off` forces SDF visibility to 1.0;
    /// visualization modes replace the shaded color output with diagnostic
    /// views.
    pub sdf_shadow_mode: SdfShadowMode,
    /// Dev toggle: force per-light SDF visibility to 1.0 in the forward shader.
    /// Drives the "no double-count" visual A/B — with every sdf light fully
    /// lit, the additive per-light diffuse must reproduce the pre-change
    /// render (disjoint sets guarantee no re-weighting). Encoded as a u32
    /// (0 = normal, non-zero = forced) into the uniform's first pad slot.
    pub sdf_force_visibility_one: bool,
    /// DYNAMIC baked-static-direct SH scale (0..1). Multiplies the direct term
    /// for the billboard path (the mesh path reads its own copy from the
    /// group-4 `DynamicDirectParams`). Repurposes the former `_sdf_pad1` slot.
    pub dynamic_direct_scale: f32,
    /// Whether the normal-free billboard direct-scatter volume is available.
    /// This is level-load fixed: the renderer chooses the real or dummy
    /// group-3 binding at load/reload and never switches it per frame.
    pub has_scatter: bool,
    /// Whether a baked DIRECT SH section is present. When false the dynamic
    /// shaders skip the direct sample (direct = 0), falling back to
    /// indirect-only. Owned here (and mirrored in the mesh uniform).
    pub has_direct: bool,
    /// Runtime direct-light records available to dynamic entity consumers.
    /// `light_count` remains the dynamic-tier count for the forward world path;
    /// this total additionally includes promoted static lights appended after
    /// the dynamic records.
    pub total_light_count: u32,
    /// Dev toggle: force static-light shadowmask visibility to 1.0 in the
    /// forward shader. Encoded as a u32 (0 = normal, non-zero = forced) in
    /// the trailing 124..128 uniform slot. It affects only world static
    /// specular; SDF and dynamic/mover paths have independent visibility.
    pub spec_shadowmask_force_one: bool,
}

pub fn build_uniform_data(u: &FrameUniforms) -> [u8; UNIFORM_SIZE] {
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
    bytes[88..92].copy_from_slice(&u.light_term_mask.bits().to_ne_bytes());
    bytes[92..96].copy_from_slice(&u.indirect_scale.to_ne_bytes());
    bytes[96..100].copy_from_slice(&u.sdf_shadow_flags.to_ne_bytes());
    let mode: u32 = u.sdf_shadow_mode as u32;
    bytes[100..104].copy_from_slice(&mode.to_ne_bytes());
    let force_vis: u32 = u.sdf_force_visibility_one as u32;
    bytes[104..108].copy_from_slice(&force_vis.to_ne_bytes());
    bytes[108..112].copy_from_slice(&u.dynamic_direct_scale.to_ne_bytes());
    let has_scatter: u32 = u.has_scatter as u32;
    bytes[112..116].copy_from_slice(&has_scatter.to_ne_bytes());
    let has_direct: u32 = u.has_direct as u32;
    bytes[116..120].copy_from_slice(&has_direct.to_ne_bytes());
    let total_off = TOTAL_LIGHT_COUNT_OFFSET as usize;
    bytes[total_off..total_off + 4].copy_from_slice(&u.total_light_count.to_ne_bytes());
    let spec_shadowmask_force_one: u32 = u.spec_shadowmask_force_one as u32;
    bytes[124..128].copy_from_slice(&spec_shadowmask_force_one.to_ne_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Mat4, Vec3};

    #[test]
    fn uniform_data_has_correct_size() {
        let data = build_uniform_data(&FrameUniforms {
            view_proj: Mat4::IDENTITY,
            camera_position: Vec3::ZERO,
            ambient_floor: 0.05,
            light_count: 0,
            time: 0.0,
            light_term_mask: LightTermMask::ALL,
            indirect_scale: 1.0,
            sdf_shadow_flags: 0,
            sdf_shadow_mode: SdfShadowMode::On,
            sdf_force_visibility_one: false,
            dynamic_direct_scale: 1.0,
            has_scatter: false,
            has_direct: false,
            total_light_count: 0,
            spec_shadowmask_force_one: false,
        });
        assert_eq!(data.len(), UNIFORM_SIZE);
    }

    #[test]
    fn light_term_mask_uses_only_the_seven_wired_bits() {
        assert_eq!(LightTermMask::ALL.bits(), 0x7F);
        assert_eq!(LightTermMask::ALL_TERMS.len(), 7);
        assert!(LightTermMask::ALL.contains(LightTermMask::AMBIENT_FLOOR));
        assert!(LightTermMask::ALL.contains(LightTermMask::SPECULAR));

        let mut mask = LightTermMask::ALL;
        mask.set_enabled(LightTermMask::DYNAMIC_DIRECT, false);
        assert_eq!(mask.bits(), 0x5F);
        assert!(!mask.contains(LightTermMask::DYNAMIC_DIRECT));
        assert_eq!(
            LightTermMask::BAKED_DIRECT_ANIMATED.label(),
            "Baked direct — animated"
        );
    }

    #[test]
    fn uniform_data_encodes_sdf_shadow_flags_at_correct_offset() {
        let data = build_uniform_data(&FrameUniforms {
            view_proj: Mat4::IDENTITY,
            camera_position: Vec3::ZERO,
            ambient_floor: 0.0,
            light_count: 0,
            time: 0.0,
            light_term_mask: LightTermMask::ALL,
            indirect_scale: 1.0,
            sdf_shadow_flags: SDF_SHADOW_FLAG_ATLAS_PRESENT,
            sdf_shadow_mode: SdfShadowMode::On,
            sdf_force_visibility_one: false,
            dynamic_direct_scale: 0.0,
            has_scatter: false,
            has_direct: false,
            total_light_count: 0,
            spec_shadowmask_force_one: false,
        });
        let flags = u32::from_ne_bytes(data[96..100].try_into().unwrap());
        assert_eq!(flags, SDF_SHADOW_FLAG_ATLAS_PRESENT);
        assert_eq!(
            u32::from_ne_bytes(data[100..104].try_into().unwrap()),
            SdfShadowMode::On as u32,
        );
        assert!(data[104..128].iter().all(|&b| b == 0));
    }

    #[test]
    fn uniform_data_encodes_sdf_force_visibility_one_at_correct_offset() {
        for (force, expected) in [(false, 0u32), (true, 1u32)] {
            let data = build_uniform_data(&FrameUniforms {
                view_proj: Mat4::IDENTITY,
                camera_position: Vec3::ZERO,
                ambient_floor: 0.0,
                light_count: 0,
                time: 0.0,
                light_term_mask: LightTermMask::ALL,
                indirect_scale: 1.0,
                sdf_shadow_flags: 0,
                sdf_shadow_mode: SdfShadowMode::On,
                sdf_force_visibility_one: force,
                dynamic_direct_scale: 0.0,
                has_scatter: false,
                has_direct: false,
                total_light_count: 0,
                spec_shadowmask_force_one: false,
            });
            assert_eq!(
                u32::from_ne_bytes(data[104..108].try_into().unwrap()),
                expected,
            );
            assert!(data[120..128].iter().all(|&b| b == 0));
        }
    }

    #[test]
    fn uniform_data_encodes_spec_shadowmask_force_one_at_correct_offset() {
        let data = build_uniform_data(&FrameUniforms {
            view_proj: Mat4::IDENTITY,
            camera_position: Vec3::ZERO,
            ambient_floor: 0.0,
            light_count: 0,
            time: 0.0,
            light_term_mask: LightTermMask::ALL,
            indirect_scale: 1.0,
            sdf_shadow_flags: 0,
            sdf_shadow_mode: SdfShadowMode::On,
            sdf_force_visibility_one: false,
            dynamic_direct_scale: 0.0,
            has_scatter: false,
            has_direct: false,
            total_light_count: 0,
            spec_shadowmask_force_one: true,
        });

        assert_eq!(u32::from_ne_bytes(data[124..128].try_into().unwrap()), 1,);
    }

    #[test]
    fn sdf_shadow_mode_round_trips_through_uniform() {
        for mode in SdfShadowMode::ALL_VARIANTS {
            let data = build_uniform_data(&FrameUniforms {
                view_proj: Mat4::IDENTITY,
                camera_position: Vec3::ZERO,
                ambient_floor: 0.0,
                light_count: 0,
                time: 0.0,
                light_term_mask: LightTermMask::ALL,
                indirect_scale: 1.0,
                sdf_shadow_flags: 0,
                sdf_shadow_mode: mode,
                sdf_force_visibility_one: false,
                dynamic_direct_scale: 0.0,
                has_scatter: false,
                has_direct: false,
                total_light_count: 0,
                spec_shadowmask_force_one: false,
            });
            let decoded = u32::from_ne_bytes(data[100..104].try_into().unwrap());
            assert_eq!(decoded, mode as u32);
            assert!(data[120..128].iter().all(|&b| b == 0));
        }
    }

    #[test]
    fn shadowmask_raw_pool_visibility_mode_uses_next_uniform_discriminant() {
        let data = build_uniform_data(&FrameUniforms {
            view_proj: Mat4::IDENTITY,
            camera_position: Vec3::ZERO,
            ambient_floor: 0.0,
            light_count: 0,
            time: 0.0,
            light_term_mask: LightTermMask::ALL,
            indirect_scale: 1.0,
            sdf_shadow_flags: 0,
            sdf_shadow_mode: SdfShadowMode::ShadowmaskRawPoolVisibility,
            sdf_force_visibility_one: false,
            dynamic_direct_scale: 0.0,
            has_scatter: false,
            has_direct: false,
            total_light_count: 0,
            spec_shadowmask_force_one: false,
        });

        assert_eq!(SdfShadowMode::ShadowmaskRawPoolVisibility as u32, 6);
        assert_eq!(u32::from_ne_bytes(data[100..104].try_into().unwrap()), 6);
    }

    #[test]
    fn uniform_data_keeps_mask_and_retired_tail_pad_at_fixed_group_zero_offsets() {
        let mut light_term_mask = LightTermMask::ALL;
        light_term_mask.set_enabled(LightTermMask::DYNAMIC_DIRECT, false);
        let data = build_uniform_data(&FrameUniforms {
            view_proj: Mat4::IDENTITY,
            camera_position: Vec3::ZERO,
            ambient_floor: 0.0,
            light_count: 0,
            time: 0.0,
            light_term_mask,
            indirect_scale: 1.0,
            sdf_shadow_flags: 0,
            sdf_shadow_mode: SdfShadowMode::On,
            sdf_force_visibility_one: false,
            dynamic_direct_scale: 0.25,
            has_scatter: true,
            has_direct: true,
            total_light_count: 11,
            spec_shadowmask_force_one: false,
        });
        let scale = f32::from_ne_bytes(data[108..112].try_into().unwrap());
        assert!((scale - 0.25).abs() < 1e-6);
        assert_eq!(
            u32::from_ne_bytes(data[88..92].try_into().unwrap()),
            light_term_mask.bits(),
            "LightTermMask must remain at the fixed group-0 88..92 ABI slot",
        );
        assert_eq!(u32::from_ne_bytes(data[112..116].try_into().unwrap()), 1);
        assert_eq!(u32::from_ne_bytes(data[116..120].try_into().unwrap()), 1);
        assert_eq!(u32::from_ne_bytes(data[120..124].try_into().unwrap()), 11);
        assert!(data[124..128].iter().all(|&b| b == 0));
    }

    #[test]
    fn uniform_data_encodes_view_proj_camera_and_lighting_fields() {
        let camera = Vec3::new(10.0, 20.0, 30.0);
        let ambient_floor = 0.125_f32;
        let light_count = 7_u32;
        let indirect_scale = 0.5_f32;
        let data = build_uniform_data(&FrameUniforms {
            view_proj: Mat4::IDENTITY,
            camera_position: camera,
            ambient_floor,
            light_count,
            time: 0.0,
            light_term_mask: LightTermMask::ALL,
            indirect_scale,
            sdf_shadow_flags: 0,
            sdf_shadow_mode: SdfShadowMode::On,
            sdf_force_visibility_one: false,
            dynamic_direct_scale: 1.0,
            has_scatter: false,
            has_direct: false,
            total_light_count: light_count,
            spec_shadowmask_force_one: false,
        });

        let mut floats = Vec::new();
        for chunk in data.chunks_exact(4).take(16) {
            floats.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        let identity = Mat4::IDENTITY.to_cols_array();
        for i in 0..16 {
            assert!((floats[i] - identity[i]).abs() < 1e-6);
        }

        assert_eq!(f32::from_ne_bytes(data[64..68].try_into().unwrap()), 10.0);
        assert_eq!(f32::from_ne_bytes(data[68..72].try_into().unwrap()), 20.0);
        assert_eq!(f32::from_ne_bytes(data[72..76].try_into().unwrap()), 30.0);
        assert!(
            (f32::from_ne_bytes(data[76..80].try_into().unwrap()) - ambient_floor).abs() < 1e-6
        );
        assert_eq!(
            u32::from_ne_bytes(data[80..84].try_into().unwrap()),
            light_count,
        );
        assert_eq!(f32::from_ne_bytes(data[84..88].try_into().unwrap()), 0.0);
        assert_eq!(
            u32::from_ne_bytes(data[88..92].try_into().unwrap()),
            LightTermMask::ALL.bits(),
            "light-term mask must retain the group-0 88..92 ABI slot",
        );
        assert!(
            (f32::from_ne_bytes(data[92..96].try_into().unwrap()) - indirect_scale).abs() < 1e-6
        );
    }

    #[test]
    fn uniform_data_encodes_script_time_as_gpu_time_field() {
        let script_time = 3.75_f32;
        let data = build_uniform_data(&FrameUniforms {
            view_proj: Mat4::IDENTITY,
            camera_position: Vec3::ZERO,
            ambient_floor: 0.0,
            light_count: 0,
            time: script_time,
            light_term_mask: LightTermMask::ALL,
            indirect_scale: 1.0,
            sdf_shadow_flags: 0,
            sdf_shadow_mode: SdfShadowMode::On,
            sdf_force_visibility_one: false,
            dynamic_direct_scale: 1.0,
            has_scatter: false,
            has_direct: false,
            total_light_count: 0,
            spec_shadowmask_force_one: false,
        });
        let t = f32::from_ne_bytes(data[84..88].try_into().unwrap());
        assert!((t - script_time).abs() < 1e-6);
    }
}
