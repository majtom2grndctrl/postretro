// Renderer: GPU init, texture upload, depth pre-pass + forward pipelines, and draw.
// See: context/lib/rendering_pipeline.md

pub mod animated_lightmap;
#[cfg(feature = "dev-tools")]
pub mod debug_lines;
#[cfg(feature = "dev-tools")]
pub mod debug_ui;
pub mod fog_pass;
pub mod frame_timing;
pub mod loaded_texture;
pub mod mesh_instances;
pub mod mesh_pass;
#[cfg(feature = "dev-tools")]
pub mod nav_diagnostics;
pub mod screen_effects;
pub mod sdf_atlas;
pub mod sdf_shadow;
pub mod sh_compose;
#[cfg(feature = "dev-tools")]
pub mod sh_diagnostics;
pub mod sh_volume;
pub mod smoke;
pub mod splash;
pub mod ui;

#[cfg(test)]
mod curve_eval_test;
#[cfg(test)]
mod sdf_light_select_test;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::compute_cull::ComputeCullPipeline;
use crate::geometry::BvhTree;
use crate::lighting::chunk_list::ChunkGrid;
use crate::lighting::influence::{self, LightInfluence};
use crate::lighting::lightmap::LightmapResources;
use crate::lighting::spec_buffer::{SPEC_LIGHT_SIZE, pack_spec_lights};
use crate::lighting::spot_shadow::SpotShadowPool;
use crate::lighting::{GPU_LIGHT_SIZE, pack_lights, pack_lights_with_slots_into};
use crate::material::Material;
use crate::prl::MapLight;
use crate::render::loaded_texture::{
    LoadedTexture, load_model_diffuse_texture, load_textures, placeholder_loaded_texture,
};
use crate::visibility::VisibleCells;
use postretro_level_format::alpha_lights::ALPHA_LIGHT_LEAF_UNASSIGNED;
use postretro_level_format::fog_cell_masks::union_active_mask;
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;

use fog_pass::FogPass;
use frame_timing::FrameTiming;
use screen_effects::ScreenEffectsPass;
use sdf_atlas::SdfAtlasResources;
use sdf_shadow::{SdfShadowFrameInputs, SdfShadowPass, SdfShadowShGrid};
use sh_compose::ShComposeResources;
use sh_volume::ShVolumeResources;
use smoke::SmokePass;

use crate::fx::smoke::SpriteFrame;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ClearColor {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl From<ClearColor> for wgpu::Color {
    fn from(color: ClearColor) -> Self {
        Self {
            r: color.r,
            g: color.g,
            b: color.b,
            a: color.a,
        }
    }
}

/// Derives the per-frame active fog-volume bitmask from the wider
/// fog-reachable leaf set produced by portal traversal.
///
/// - `fog_reachable` non-empty + masks present: OR each reachable leaf's mask.
/// - `fog_reachable` empty: portal isolation doesn't apply — empty world,
///   solid-leaf camera, exterior camera, or no-portals map. Every canonical
///   slot stays active.
/// - `fog_reachable` non-empty + masks absent: legacy-PRL fallback — keep all
///   canonical slots active so a PRL without section 31 still renders fog.
///
/// `camera_leaf`'s own fog mask bits are always unioned into the result when
/// masks are present, regardless of whether the camera leaf appears in
/// `fog_reachable`. Portal traversal can omit the camera leaf on transient
/// frames (e.g., grazing a portal seam); unioning prevents fog the camera is
/// inside from flickering off. Idempotent when the camera leaf is already in
/// `fog_reachable`.
///
/// Must be called after `FogPass::set_canonical_volumes`; before = 0
/// canonical count = 0 mask.
fn compute_fog_cell_mask(
    fog_reachable: &[u32],
    fog_cell_masks: Option<&[u32]>,
    canonical_volume_count: u32,
    camera_leaf: Option<u32>,
) -> u32 {
    let all_slots_mask = if canonical_volume_count >= 32 {
        u32::MAX
    } else {
        (1u32 << canonical_volume_count).wrapping_sub(1)
    };
    match (fog_reachable.is_empty(), fog_cell_masks) {
        // Empty fog_reachable: portal isolation doesn't apply — either the world is
        // empty (DrawAll arm), or a non-portal fallback ran (solid-leaf, exterior,
        // no-portals) and produced no fog_reachable set. All canonical slots active.
        (true, _) => all_slots_mask,
        // AND against `all_slots_mask` so reserved bits 16..32 in the baked
        // mask (or trailing bits past the loaded canonical count) cannot set
        // a phantom active slot the GPU buffer doesn't carry.
        //
        // Union in the camera leaf's fog mask: portal traversal can omit the
        // camera leaf from `fog_reachable` in transient frames (e.g., crossing
        // a portal boundary), but fog the camera is inside must remain active
        // to prevent flicker. Idempotent when the camera leaf is already in
        // `fog_reachable`.
        (false, Some(masks)) => {
            let mut active = union_active_mask(fog_reachable, masks);
            if let Some(cl) = camera_leaf {
                active |= masks.get(cl as usize).copied().unwrap_or(0);
            }
            active & all_slots_mask
        }
        // Culled visibility + missing baked masks: fall back to "all slots
        // visible" so a legacy PRL without section 31 still renders fog
        // — `live_mask` will gate density-zero slots either way.
        // Note: when `canonical_volume_count == 0`, `all_slots_mask == 0` here,
        // so `active_count` will be 0 after repack and the fog pass is skipped
        // correctly via the `FogPass::active()` guard. No phantom slots are
        // activated on a zero-volume level.
        (false, None) => all_slots_mask,
    }
}

/// Returns `true` when `aabbs` is empty — conservative for pre-`set_fog_aabbs` frames;
/// spots are discarded by `FogPass::active()` before reaching the raymarch anyway.
fn sphere_intersects_any_fog_aabb(center: Vec3, radius: f32, aabbs: &[(Vec3, Vec3)]) -> bool {
    if aabbs.is_empty() {
        return true;
    }
    let r2 = radius * radius;
    for (min, max) in aabbs {
        let clamped = center.clamp(*min, *max);
        let d = center - clamped;
        if d.length_squared() <= r2 {
            return true;
        }
    }
    false
}

// `curve_eval.wgsl` reads `anim_samples`; `sh_sample.wgsl` reads
// `sh_total_atlas`, `sh_depth_moments`, and `sh_grid`, all declared in
// `forward.wgsl`. WGSL resolves module-scope names regardless of textual order,
// so appending after is safe. `sh_sample.wgsl` owns the SH reconstruction +
// 8-corner blend symbols (`sample_sh_indirect_corners_pair`,
// `sample_sh_indirect_direct_corners`, `sample_sh_direct_corners_depth_aware`,
// `sample_sh_indirect_corners_depth_aware`, `sample_sh_indirect_corners_without_depth`,
// `sample_sh_indirect_corners_two_without_depth`) — forward must not redeclare them.
//
// `sdf_light_select.wgsl` is the LOAD-BEARING K-selection parity seam: the same
// source string is concatenated into the half-res SDF visibility pass
// (`sdf_shadow.rs`) so both pick the same `sdf`-tagged lights in the same order.
// It reads `spec_lights` / `chunk_grid` / `chunk_offsets` / `chunk_indices` by
// name — all already declared in `forward.wgsl` for the static-light loop — and
// declares no buffers of its own. Never reimplement the selection here.
//
// `light_eval.wgsl` owns the dynamic-tier per-light evaluation helpers
// (`light_eval_falloff`, `light_eval_cone_attenuation`,
// `light_eval_animated_direction`, `light_eval_scripted_intensity_scalar`) the
// runtime light loop calls — extracted so the skinned-mesh pass can mirror the
// same loop against its own group-2 bindings. It declares no buffers. Append
// ORDER dependency: `light_eval_animated_direction` calls
// `sample_color_catmull_rom` from `curve_eval.wgsl`, so the consumer must also
// append curve_eval (forward does, above). WGSL resolves module-scope names
// regardless of textual order, so the relative order of these two is free.
//
// `shadow_sample.wgsl` owns the runtime shadow-map samplers (`sample_spot_shadow`
// spot 2D-array PCF, `sample_point_shadow` point cube-array PCF) plus their
// bias/resolution constants and `cube_face_ndc_depth` — extracted so the
// skinned-mesh pass can mirror the same calls against its own group-2 b5–b8
// shadow bindings. It declares no bindings: it reads the group-5
// `spot_shadow_depth`, `spot_shadow_compare`, `light_space_matrices`, and
// `point_shadow_cube` declared in `forward.wgsl` by lexical name. The
// `// CUBE_SHADOW_BODY_BEGIN` / `// CUBE_SHADOW_BODY_END` markers around
// `sample_point_shadow`'s body travel WITH the body into this snippet, so
// `strip_point_shadow_cube` still neutralizes the cube path in the composed
// no-`CUBE_ARRAY_TEXTURES` source; the `// CUBE_SHADOW_BINDING` binding
// declaration stays with the consumer in `forward.wgsl`.
const SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/forward.wgsl"),
    "\n",
    include_str!("../shaders/curve_eval.wgsl"),
    "\n",
    include_str!("../shaders/sh_sample.wgsl"),
    "\n",
    include_str!("../shaders/sdf_light_select.wgsl"),
    "\n",
    include_str!("../shaders/light_eval.wgsl"),
    "\n",
    include_str!("../shaders/shadow_sample.wgsl"),
);

/// Derive the no-`CUBE_ARRAY_TEXTURES` variant of a group-5 shader (forward or
/// fog) from its single canonical source, so there is no second hand-maintained
/// copy to drift. Two localized edits, both keyed off marker comments embedded in
/// the WGSL:
///
/// 1. Strip the `point_shadow_cube` binding-5 declaration (the line tagged
///    `// CUBE_SHADOW_BINDING`), so the shader matches a group-5 BGL that omits
///    binding 5 — required, since a `CubeArray` BGL entry needs the feature.
/// 2. Neutralize `sample_point_shadow`: replace its body (delimited by
///    `// CUBE_SHADOW_BODY_BEGIN` / `// CUBE_SHADOW_BODY_END`) with `return 1.0;`
///    so the function references no stripped binding and every point light reads
///    as unshadowed. The fog shader has no such body (it never samples the cube),
///    so the body transform is a no-op there.
///
/// Panics (init-time, acceptable per the panic policy) if the binding marker is
/// absent — that means the shader and this transform have drifted, which must
/// fail loudly rather than ship a mis-bound pipeline. The body markers are
/// optional (fog omits them). `pub(super)` so the fog pass (`fog_pass.rs`) derives
/// its own no-cube variant from the SAME transform.
pub(super) fn strip_point_shadow_cube(source: &str) -> String {
    // 1. Drop the marked binding-5 declaration line.
    let without_binding: String = {
        let kept: Vec<&str> = source
            .lines()
            .filter(|line| !line.contains("// CUBE_SHADOW_BINDING"))
            .collect();
        assert!(
            kept.len() < source.lines().count(),
            "strip_point_shadow_cube: no `// CUBE_SHADOW_BINDING` line found — \
             shader and transform have drifted"
        );
        kept.join("\n")
    };

    // 2. Replace the cube-sampling function body with a no-shadow constant.
    const BEGIN: &str = "// CUBE_SHADOW_BODY_BEGIN";
    const END: &str = "// CUBE_SHADOW_BODY_END";
    match (without_binding.find(BEGIN), without_binding.find(END)) {
        (Some(begin), Some(end)) => {
            // `end` indexes the start of the END marker; include the marker line
            // itself in the replaced span so it does not linger.
            let end_line_end = without_binding[end..]
                .find('\n')
                .map(|n| end + n)
                .unwrap_or(without_binding.len());
            let mut out = String::with_capacity(without_binding.len());
            out.push_str(&without_binding[..begin]);
            out.push_str("return 1.0;");
            out.push_str(&without_binding[end_line_end..]);
            out
        }
        // Fog: no body markers, declaration strip alone suffices.
        (None, None) => without_binding,
        _ => panic!(
            "strip_point_shadow_cube: exactly one of the CUBE_SHADOW_BODY markers \
             is present — shader and transform have drifted"
        ),
    }
}

const WIREFRAME_SHADER_SOURCE: &str = include_str!("../shaders/wireframe.wgsl");

// Depth pre-pass: writes depth only (enables Equal depth compare → zero shading
// overdraw). The full-res lightmap-UV gbuffer MRT it once wrote was freed with
// the animated dominant-direction trace; the per-light SDF visibility pass keys
// on light position, not lightmap UV, so it has no color attachment now.
const DEPTH_PREPASS_SHADER_SOURCE: &str = include_str!("../shaders/depth_prepass.wgsl");

// Spot shadow: vertex-only; per-slot matrix selected via dynamic-offset uniform.
const SPOT_SHADOW_SHADER_SOURCE: &str = include_str!("../shaders/spot_shadow.wgsl");

// Pair index i → query slots [2i, 2i+1]. Labels vec keeps ordering and callsite indices in sync.
const TIMING_PAIR_CULL: usize = 0;
const TIMING_PAIR_ANIMATED_LM_COMPOSE: usize = 1;
const TIMING_PAIR_DEPTH_PREPASS: usize = 2;
const TIMING_PAIR_SDF_SHADOW: usize = 3;
const TIMING_PAIR_FORWARD: usize = 4;
const TIMING_PAIR_SH_COMPOSE: usize = 5;
const TIMING_PAIR_SMOKE: usize = 6;
const TIMING_PAIR_COUNT: usize = 7;

// Must match `Uniforms` in forward.wgsl and wireframe.wgsl (both bind the same buffer).
// std140: vec3<f32> aligns to 16 bytes; camera_position and ambient_floor share a slot.
//   0..64    view_proj  64..76   camera_position  76..80   ambient_floor
//   80..84   light_count  84..88  time  88..92   lighting_isolation  92..96  indirect_scale
//   96..100  sdf_shadow_flags  100..104 sdf_shadow_mode
//   104..108 sdf_force_visibility_one  108..112 dynamic_direct_scale
//   112..116 dynamic_direct_isolation  116..120 has_direct  120..128 _pad
// `sdf_shadow_flags` gates whether the forward samples the half-res SDF
// visibility target at all:
//   bit 0 = a baked SDF atlas is loaded, so the four RGBA channels hold valid
//           per-light visibility slices (K = 4). Set whenever the atlas loads.
// The per-light sdf-tag diffuse/specular terms read their visibility slices
// directly (no per-slice flag) — gated instead by `select_sdf_lights` returning
// lights for the fragment.
// `sdf_shadow_mode` overlays the debug selector; `sdf_force_visibility_one`
// is the dev "force visibility to 1.0" toggle for the no-double-count A/B.
// The dynamic-direct tail (Task 6 of baked-static-direct-sh): repurposes the
// old `_sdf_pad1` slot (108..112) for `dynamic_direct_scale`, then a fresh
// 16-byte row carries `dynamic_direct_isolation` + `has_direct` + padding.
// Only billboard.wgsl reads these (the mesh path uses its own group-4
// `DynamicDirectParams`); forward/wireframe declare them as inert tail so the
// shared 3-way byte contract (Rust writer + forward.wgsl + billboard.wgsl)
// keeps a single stride. Struct stride rounds to 128 (multiple of mat4 align).
const UNIFORM_SIZE: usize = 128;

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

struct FrameUniforms {
    view_proj: Mat4,
    camera_position: Vec3,
    ambient_floor: f32,
    light_count: u32,
    time: f32,
    lighting_isolation: LightingIsolation,
    indirect_scale: f32,
    /// Bitset of `SDF_SHADOW_FLAG_*` controlling the forward shader's SDF
    /// shadow-factor multiplies. Bit 0 gates the animated-baked term; bit 1
    /// gates the static-lightmap term (independent because the static-term
    /// multiply must skip a shadowed-mode lightmap to avoid double shadows).
    sdf_shadow_flags: u32,
    /// `SdfShadowMode` debug selector (Task 6). Encoded as the enum's `u32`
    /// repr (0=On, 1=Off, 2=Visualize). Overlays the per-term flags above:
    /// `Off` forces both SDF multiplies to 1.0; `Visualize` replaces the
    /// shaded color output with a grayscale R-channel view.
    sdf_shadow_mode: SdfShadowMode,
    /// Dev toggle: force per-light SDF visibility to 1.0 in the forward shader.
    /// Drives the "no double-count" visual A/B — with every sdf light fully
    /// lit, the additive per-light diffuse must reproduce the pre-change
    /// render (disjoint sets guarantee no re-weighting). Encoded as a u32
    /// (0 = normal, non-zero = forced) into the uniform's first pad slot.
    sdf_force_visibility_one: bool,
    /// DYNAMIC baked-static-direct SH scale (0..1). Multiplies the direct term
    /// for the billboard path (the mesh path reads its own copy from the
    /// group-4 `DynamicDirectParams`). Repurposes the former `_sdf_pad1` slot.
    dynamic_direct_scale: f32,
    /// DYNAMIC-direct isolation mode (billboard path). Separate from
    /// `lighting_isolation`. Lands in a fresh trailing 16-byte row.
    dynamic_direct_isolation: DynamicDirectIsolation,
    /// Whether a baked DIRECT SH section is present. When false the dynamic
    /// shaders skip the direct sample (direct = 0), falling back to
    /// indirect-only. Owned here (and mirrored in the mesh uniform).
    has_direct: bool,
}

fn build_uniform_data(u: &FrameUniforms) -> [u8; UNIFORM_SIZE] {
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

/// Minimum useful ambient. Default value seeded into the Diagnostics panel slider on first open.
pub const DEFAULT_AMBIENT_FLOOR: f32 = 0.001;

/// Full SH contribution weight — production default. Default value seeded into the Diagnostics panel slider on first open.
pub const DEFAULT_INDIRECT_SCALE: f32 = 1.0;

/// Full dynamic baked-static-direct SH weight — production default. Seeded into
/// the Diagnostics panel slider on first open.
pub const DEFAULT_DYNAMIC_DIRECT_SCALE: f32 = 1.0;

struct GpuTexture {
    bind_group: wgpu::BindGroup,
}

/// Hardware anisotropy cap for the Post Retro filtering pool. wgpu 29 requires
/// `anisotropy_clamp >= 1`; 16 is the common ceiling exposed by desktop adapters
/// and the visual point of diminishing returns for grazing-angle sharpness.
pub const POST_RETRO_ANISO_CLAMP: u16 = 16;

/// Highest valid LOD index for a chain of `mip_count` mips. The anisotropic
/// sampler pool clamps `lod_max` to this so no sampler reads past the uploaded chain.
fn mip_lod_max_clamp(mip_count: u32) -> f32 {
    mip_count.saturating_sub(1) as f32
}

/// Create the Post Retro filtering pool's sampler: fully Linear min/mag/mip
/// with `anisotropy_clamp = POST_RETRO_ANISO_CLAMP`, with a per-mip-count LOD
/// clamp. wgpu 29 validates that aniso > 1 requires all three filters to be
/// Linear. One sampler per distinct mip count is kept in
/// `Renderer::mip_count_aniso_samplers` so each material binds the clamp that
/// matches its uploaded mip chain. Bound in every material bind group
/// (binding 5).
fn create_mip_aniso_sampler(device: &wgpu::Device, mip_count: u32) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("Mip Texture Aniso Sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        lod_min_clamp: 0.0,
        lod_max_clamp: mip_lod_max_clamp(mip_count),
        anisotropy_clamp: POST_RETRO_ANISO_CLAMP,
        ..Default::default()
    })
}

fn build_material_bind_group(
    device: &wgpu::Device,
    texture_bind_group_layout: &wgpu::BindGroupLayout,
    loaded: &LoadedTexture,
    aniso_sampler: &wgpu::Sampler,
    material: Material,
    label_prefix: &str,
) -> wgpu::BindGroup {
    let uniform_bytes = build_material_uniform(material.shininess());
    let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(&format!("{label_prefix} Uniform")),
        contents: &uniform_bytes,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(&format!("{label_prefix} Bind Group")),
        layout: texture_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&loaded.diffuse_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&loaded.specular_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: uniform_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(&loaded.normal_view),
            },
            // Post Retro filtering: the anisotropic sampler paired with
            // in-shader texel-grid reconstruction in forward.wgsl.
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::Sampler(aniso_sampler),
            },
        ],
    })
}

// std140: trailing _pad forces size to 32 bytes to match WGSL `MaterialUniform`.
//   0..4  shininess   4..32  pad
const MATERIAL_UNIFORM_SIZE: usize = 32;

fn build_material_uniform(shininess: f32) -> [u8; MATERIAL_UNIFORM_SIZE] {
    let mut bytes = [0u8; MATERIAL_UNIFORM_SIZE];
    bytes[0..4].copy_from_slice(&shininess.to_le_bytes());
    bytes
}

/// Per-submesh draw assignment: the index of the *distinct* material this
/// submesh draws with (into [`SubmeshMaterialPlan::distinct_keys`]) and the
/// `start..end` index range it occupies in the merged buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SubmeshDraw {
    /// Index into `distinct_keys` — which deduped material bind group to bind.
    distinct: usize,
    /// `start..end` into the merged index buffer (what `draw_indexed` consumes).
    indices: std::ops::Range<u32>,
}

/// The GPU-free plan for drawing a multi-submesh model: the distinct material
/// keys to build a bind group for (first-seen order, deduped) and the per-submesh
/// assignment of (distinct material, index range), in submesh order.
///
/// First-seen dedup order keeps submesh 0's material at `distinct[0]`, so a
/// single-material model is the trivial special case of the multi-material path
/// (one-submesh ≡ one-distinct ≡ the whole model).
///
/// Factored out of the GPU resolve so the dedup + range bookkeeping is unit
/// testable without a `wgpu::Device`: a model reusing one material across N
/// primitives yields one distinct key and N draws; N distinct materials yield N
/// of each. The GPU layer ([`Renderer::resolve_skinned_model_material`]) builds
/// one bind group per distinct key, then pairs each submesh's range with its
/// (possibly shared) bind group in order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SubmeshMaterialPlan {
    /// The distinct material keys, in first-seen submesh order. One GPU material
    /// bind group is built per entry.
    distinct_keys: Vec<String>,
    /// One entry per submesh, in submesh order: which distinct key it uses and
    /// the index range it draws.
    draws: Vec<SubmeshDraw>,
}

/// Build the [`SubmeshMaterialPlan`] for a model's submeshes: dedup the material
/// keys (first-seen order) and assign each submesh to its distinct key + range.
/// Pure data logic — no GPU — so the dedup/range bookkeeping is unit-testable.
fn plan_submesh_materials(submeshes: &[crate::model::gltf_loader::Submesh]) -> SubmeshMaterialPlan {
    let mut distinct_keys: Vec<String> = Vec::new();
    let mut draws: Vec<SubmeshDraw> = Vec::with_capacity(submeshes.len());
    for sub in submeshes {
        let distinct = match distinct_keys.iter().position(|k| k == &sub.material_key) {
            Some(idx) => idx,
            None => {
                distinct_keys.push(sub.material_key.clone());
                distinct_keys.len() - 1
            }
        };
        draws.push(SubmeshDraw {
            distinct,
            indices: sub.indices.clone(),
        });
    }
    SubmeshMaterialPlan {
        distinct_keys,
        draws,
    }
}

/// Parse a 64-char hex blake3 cache key into 32 bytes. Returns the shared
/// all-zero placeholder sentinel on malformed input, so an absent/garbled model
/// material key degrades to a placeholder rather than panicking.
fn parse_blake3_key(hex: &str) -> [u8; 32] {
    let mut key = [0u8; 32];
    if hex.len() != 64 {
        return [0u8; 32];
    }

    for (byte, pair) in key.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
        let [high, low] = pair else {
            return [0u8; 32];
        };
        let (Some(high), Some(low)) = (ascii_hex_nibble(*high), ascii_hex_nibble(*low)) else {
            return [0u8; 32];
        };
        *byte = (high << 4) | low;
    }
    key
}

fn ascii_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Derive the glTF open path and the renderer cache handle for one skinned model
/// from its content-relative handle. These are DELIBERATELY decoupled: the file
/// opens from `content_root.join(model_rel)` (every other asset joins the content
/// root), but the cache key is the VERBATIM `model_rel` string — the
/// `MeshComponent.model` handle the spawn attaches and the per-frame planner
/// groups by, so a joined key would miss `models.get(&group.model)` and silently
/// drop every draw. Split out as a pure helper so the key/path contract is
/// unit-testable without a GPU device (`load_skinned_model` needs one).
fn resolve_model_open_path_and_handle(
    model_rel: &str,
    content_root: &Path,
) -> (std::path::PathBuf, crate::model::ModelHandle) {
    (
        content_root.join(model_rel),
        crate::model::ModelHandle::from(model_rel.to_string()),
    )
}

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Extent for the full-res depth pre-pass attachment. Recreated at the surface
/// size on resize. `0` is clamped to `1` to keep texture creation valid during
/// transient zero-size resize events.
fn prepass_attachment_extent(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: width.max(1),
        height: height.max(1),
        depth_or_array_layers: 1,
    }
}

fn create_depth_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let size = prepass_attachment_extent(width, height);

    let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Depth Texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });

    let view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
    (depth_texture, view)
}

// Group 0: per-frame uniforms (view/proj/time). One buffer entry, no textures.
// COMPUTE required: animated-lightmap compose reuses this BGL (same buffer;
// `uniforms.time` drives curve sampling). Dropping COMPUTE fails wgpu validation
// at compute pipeline creation.
fn uniform_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 1] {
    [wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::VERTEX
            | wgpu::ShaderStages::FRAGMENT
            | wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }]
}

// Group 1: 0=diffuse(sRGB), 2=specular(R8), 3=shininess, 4=normal(Rgba8Unorm,
// NOT sRGB; n = sample.rgb*2-1), 5=aniso_sampler (linear+anisotropic).
// Binding 1 is intentionally vacated (former nearest sampler); the aniso sampler
// stays at 5 — non-contiguous bindings are valid.
fn material_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 5] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 5,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
    ]
}

// Group 2: 0=dynamic lights, 1=influence volumes, 2=spec-only statics,
//          3=ChunkGridInfo, 4=chunk offsets, 5=chunk indices. All buffers, no
// textures.
fn lighting_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 6] {
    // Billboard hoists its static-specular and dynamic-light loops into the
    // vertex stage, so group 2 must be VERTEX-visible too. This is additive —
    // the forward (FRAGMENT) and fog (COMPUTE) pipelines still bind the same
    // group; wgpu validates the widened visibility at pipeline creation. The
    // mesh pipeline reuses only groups 0 and 1, so it is unaffected.
    let storage_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    [
        storage_entry(0),
        storage_entry(1),
        storage_entry(2),
        wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        storage_entry(4),
        storage_entry(5),
    ]
}

/// Count BGL entries that consume a `max_sampled_textures_per_shader_stage` slot
/// for the FRAGMENT stage: `BindingType::Texture` entries whose visibility
/// includes FRAGMENT. wgpu charges the limit against the BGL *entry* set of a
/// pipeline layout per stage, not against how many textures a shader actually
/// samples — so a fragment-visible texture entry counts even if no fragment
/// shader reads it. Example: billboard samples the SH direct atlas in the
/// VERTEX stage, but its BGL entry carries `VERTEX | FRAGMENT` visibility, so
/// it still counts against the fragment texture budget here.
#[cfg(debug_assertions)]
fn fragment_sampled_textures(entries: &[wgpu::BindGroupLayoutEntry]) -> u32 {
    entries
        .iter()
        .filter(|e| {
            e.visibility.contains(wgpu::ShaderStages::FRAGMENT)
                && matches!(e.ty, wgpu::BindingType::Texture { .. })
        })
        .count() as u32
}

/// Count BGL entries that consume a `max_storage_buffers_per_shader_stage` slot
/// for the VERTEX stage: `BindingType::Buffer { ty: Storage, .. }` entries whose
/// visibility includes VERTEX. wgpu charges this limit against the BGL *entry* set
/// of a pipeline layout per stage — a vertex-visible storage entry counts even if
/// no vertex shader actually reads it (exactly the over-broad-visibility trap that
/// hoisting billboard lighting into `vs_main` fell into). The downlevel/WebGPU
/// default ceiling is 8.
#[cfg(debug_assertions)]
fn vertex_storage_buffers(entries: &[wgpu::BindGroupLayoutEntry]) -> u32 {
    entries
        .iter()
        .filter(|e| {
            e.visibility.contains(wgpu::ShaderStages::VERTEX)
                && matches!(
                    e.ty,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { .. },
                        ..
                    }
                )
        })
        .count() as u32
}

/// Single source of truth for the billboard pipeline's VERTEX-stage storage-buffer
/// budget. Sums the vertex-visible storage entries across the exact BGLs that
/// compose the Billboard Pipeline Layout (see `SmokePass::new` for the matching
/// group order: 0 camera, 1 sheet, 2 lighting, 3 SH volume, 6 instance). GPU-free,
/// so it runs in unit tests and `Renderer::new` without a device.
///
/// Billboard lighting runs in `vs_main` (per-vertex SH indirect+direct,
/// static-specular, dynamic-diffuse); the group-6 instance storage buffer is
/// VERTEX-read. The genuinely vertex-read storage buffers are: group 2's five
/// (`lights`, `light_influence`, `spec_lights`, `chunk_offsets`, `chunk_indices`)
/// and group 6's one (`sprites`) — six total. The three group-3 anim/scripted-light
/// storage buffers are read only in the fragment/compute stages, so they must NOT
/// carry VERTEX visibility (see `sh_bind_group_layout_entries`); if they did, this
/// would report 9 and pipeline creation would fail on real GPUs with the
/// downlevel-default limit of 8.
#[cfg(debug_assertions)]
fn billboard_pipeline_vertex_storage_buffer_count() -> u32 {
    vertex_storage_buffers(&uniform_bind_group_layout_entries())
        + vertex_storage_buffers(&smoke::sprite_sheet_bind_group_layout_entries())
        + vertex_storage_buffers(&lighting_bind_group_layout_entries())
        + vertex_storage_buffers(&sh_volume::sh_bind_group_layout_entries())
        + vertex_storage_buffers(&smoke::sprite_instance_bind_group_layout_entries())
}

/// Single source of truth for the forward ("Textured") pipeline's sampled-texture
/// budget. Sums the fragment-visible texture entries across the exact BGLs that
/// compose the forward pipeline layout (see `create_pipeline_layout` for the
/// matching group order). GPU-free: every builder returns plain CPU structs, so
/// this runs in unit tests and at init without a device. Keeping the layout
/// creation and this count reading from the same builders prevents the two
/// sources of truth from drifting (the bug this guards against). Asserted in
/// `Renderer::new` and the
/// `forward_pipeline_sampled_texture_request_matches_bgl_definitions` test.
#[cfg(debug_assertions)]
fn forward_pipeline_sampled_texture_count(cube_array_supported: bool) -> u32 {
    // Groups 0 (uniform) and 2 (lighting) carry no textures, but include them so
    // adding a texture entry to either BGL is caught here automatically. Group 5's
    // count is feature-conditional: the cube-array point-shadow texture (binding 5)
    // is present only when `cube_array_supported` (14 total with it, 13 without).
    fragment_sampled_textures(&uniform_bind_group_layout_entries())
        + fragment_sampled_textures(&material_bind_group_layout_entries())
        + fragment_sampled_textures(&lighting_bind_group_layout_entries())
        + fragment_sampled_textures(&sh_volume::sh_bind_group_layout_entries())
        + fragment_sampled_textures(&crate::lighting::lightmap::bind_group_layout_entries())
        + fragment_sampled_textures(&SpotShadowPool::bind_group_layout_entries(
            cube_array_supported,
        ))
}

pub struct LevelGeometry<'a> {
    pub vertices: &'a [crate::geometry::WorldVertex],
    pub indices: &'a [u32],
    pub bvh: &'a BvhTree,
    pub lights: &'a [MapLight],
    pub light_influences: &'a [LightInfluence],
    /// `None` means no `OctahedralShVolumeSection`; renderer binds dummy
    /// 1×1 atlas resources and shader skips octahedral SH sampling.
    pub sh_volume: Option<&'a postretro_level_format::sh_volume::OctahedralShVolumeSection>,
    /// `None` → 1×1 white placeholder; bumped-Lambert falls back to flat white.
    pub lightmap: Option<&'a postretro_level_format::lightmap::LightmapSection>,
    /// `None` → `has_chunk_grid == 0`; shader iterates the full spec buffer.
    pub chunk_light_list:
        Option<&'a postretro_level_format::chunk_light_list::ChunkLightListSection>,
    /// `None` when the map has zero animated lights.
    pub animated_light_chunks:
        Option<&'a postretro_level_format::animated_light_chunks::AnimatedLightChunksSection>,
    /// `None` → 1×1 zero atlas bound on group 4.
    pub animated_light_weight_maps: Option<
        &'a postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection,
    >,
    /// `None` → compose pass falls back to a base→total copy.
    pub delta_sh_volumes:
        Option<&'a postretro_level_format::delta_sh_volumes::DeltaShVolumesSection>,
    /// Dense baked DIRECT static-light octahedral atlas sampled by the dynamic
    /// pipelines (mesh + billboard). `None` → renderer binds a 4×4 BC6H zero
    /// dummy and the dynamic shaders skip the direct sample (indirect-only).
    pub direct_sh_volume:
        Option<&'a postretro_level_format::direct_sh_volume::DirectShVolumeSection>,
    /// `None` → no SDF static-occluder atlas; runtime SDF shadow pass disabled.
    /// An empty-geometry section (zero grid dims) is treated the same way.
    pub sdf_atlas: Option<&'a postretro_level_format::sdf_atlas::SdfAtlasSection>,
    /// Whether the lightmap atlas was baked with the static-light visibility
    /// term included (Shadowed — `main`-equivalent) or removed (Unshadowed,
    /// Task 2a). The renderer surfaces this so the forward pass (Task 5)
    /// knows whether to multiply the SDF visibility factor into the static
    /// term. Defaults to `Shadowed` for legacy PRLs.
    pub lightmap_mode: crate::prl::LightmapMode,
    pub texture_materials: &'a [crate::material::Material],
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    is_surface_configured: bool,

    pipeline: wgpu::RenderPipeline,
    depth_prepass_pipeline: wgpu::RenderPipeline,
    /// `Some` when `POSTRETRO_GPU_TIMING=1` AND adapter supports `TIMESTAMP_QUERY`;
    /// `None` → no `timestamp_writes` attached to any pass.
    frame_timing: Option<FrameTiming>,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,

    /// Retained so `install_textures` can create material bind groups after init.
    texture_bind_group_layout: wgpu::BindGroupLayout,
    /// Retained so `install_level_geometry` can rebuild the lighting bind group.
    lighting_bind_group_layout: wgpu::BindGroupLayout,
    /// Post Retro linear+anisotropic samplers, one per distinct uploaded
    /// `mip_count`. Sampler descriptors are identical except for
    /// `lod_max_clamp = (mip_count - 1) as f32`. Keyed by
    /// `LoadedTexture::mip_count`. Engine-lifetime — persists across level
    /// reloads so re-installing the same mip chain reuses the existing sampler.
    /// Placeholders pick up the `1` entry seeded at construction. Every material
    /// binds its matching sampler at group-1 binding 5.
    mip_count_aniso_samplers: HashMap<u32, wgpu::Sampler>,
    /// Engine-lifetime owners of the loaded textures and views referenced by
    /// material bind groups. Replaced wholesale on every `install_textures`.
    /// Bind groups borrow these handles; dropping the vec invalidates them,
    /// so keep them resident for the level's lifetime.
    #[allow(dead_code)]
    loaded_textures: Vec<LoadedTexture>,
    /// `has_multi_draw_indirect` flag cached for `install_level_geometry`.
    has_multi_draw_indirect: bool,
    /// Per-texture material properties derived from texture names. Set by
    /// `install_level_geometry`; consumed by `install_textures` to populate
    /// per-material shininess uniforms.
    stored_texture_materials: Vec<Material>,
    /// Retained so `install_level_geometry` can pass it to `ShComposeResources`
    /// and `AnimatedLightmapResources` without recreating the layout inline.
    uniform_bind_group_layout: wgpu::BindGroupLayout,

    /// GPU half of the debug UI. Lazily constructed by `ensure_debug_ui_gpu`
    /// on first panel open; stays resident for the rest of the session.
    /// `None` until then; never allocated in a no-`dev-tools` build.
    #[cfg(feature = "dev-tools")]
    debug_ui_gpu: Option<debug_ui::DebugUiGpu>,

    /// Always bound; maps with zero lights get a 1-element dummy buffer —
    /// wgpu rejects zero-sized storage buffer bindings.
    lighting_bind_group: wgpu::BindGroup,
    light_count: u32,
    /// The frame's forward `Uniforms.time` value, cached by
    /// `update_per_frame_uniforms` so the skinned-mesh group-2 params uniform
    /// (`MeshLightParams.time`) is written from the SAME render-clock value the
    /// forward pass uses that frame. The scripted-light animated curves the mesh
    /// dynamic loop evaluates depend on this phase coherence.
    mesh_dynamic_time: f32,
    ambient_floor: f32,
    indirect_scale: f32,
    /// DYNAMIC baked-static-direct SH scale (0..1). Debug instrument for the
    /// entity/billboard direct term, independent of `indirect_scale`. Mirrors
    /// the `indirect_scale` knob — uploaded to the billboard group-0 tail and
    /// the mesh group-4 `DynamicDirectParams` each frame.
    dynamic_direct_scale: f32,
    /// Runtime SH probe-occlusion toggle. Default-on; `POSTRETRO_SH_FAST=1`
    /// seeds it off for benchmark/headless runs, and the diagnostics panel can
    /// flip it later. Uploaded through `ShGridInfo`.
    probe_occlusion_enabled: bool,

    /// Absent/disabled OctahedralShVolume → dummy 1×1 atlas resources;
    /// `has_sh_volume == 0` skips indirect sampling.
    sh_volume_resources: ShVolumeResources,

    /// Static-occluder SDF atlas + bind group. Owned by the renderer; the
    /// bind-group layout is consumed only by the SDF shadow pass — NOT
    /// bound by forward (forward gets only the shadow-factor texture in
    /// group 5). `present` is false when no SDF section is in the PRL;
    /// the shadow pass skips its dispatch in that case.
    sdf_atlas_resources: SdfAtlasResources,
    /// Half-resolution per-light SDF shadow pass. Always allocated.
    /// Dispatch is gated on `sdf_atlas_resources.present` and the active
    /// `SdfShadowMode`.
    sdf_shadow_pass: SdfShadowPass,
    /// Lightmap bake mode read from the PRL (records whether visibility was
    /// folded into the bake). Under the disjoint-direct design, `sdf` lights
    /// are excluded from `lm_irr` at bake time, so the forward pass never
    /// multiplies SDF visibility into the static-lightmap term; this field
    /// is retained only for legacy-PRL compatibility. Defaults to `Shadowed`
    /// so legacy PRLs decode without error.
    #[allow(dead_code)]
    lightmap_mode: crate::prl::LightmapMode,

    /// CPU mirror of animated-light delta volume placements, one entry per
    /// animated light. Empty when the map has no delta SH volumes. Sourced
    /// at level load from the same `DeltaShVolumesSection` `sh_compose` consumes;
    /// surfaced via `Renderer::sh_delta_volumes` for the SH diagnostic overlay.
    #[cfg(feature = "dev-tools")]
    sh_delta_volumes_meta: Vec<sh_volume::DeltaVolumeMeta>,

    /// Async readback of the composed SH atlas so irradiance probe markers
    /// reflect live (base + animated-delta) lighting. Rebuilt per level load.
    #[cfg(feature = "dev-tools")]
    sh_probe_readback: sh_diagnostics::ShProbeReadback,

    /// Dev-tools toggle: when set, `uniforms.time` is pinned to `frozen_time`,
    /// so all curve-driven animation (SH compose, animated lightmap, scripted
    /// lights) holds still — a debugging aid for isolating time-driven artifacts.
    #[cfg(feature = "dev-tools")]
    freeze_time: bool,
    /// Time held while `freeze_time` is set; tracks live time otherwise, so
    /// enabling the freeze holds whatever animation phase is currently showing.
    #[cfg(feature = "dev-tools")]
    frozen_time: f32,

    /// Composes base SH bands into the total bands consumers sample. Must run
    /// before the depth pre-pass so the storage→sampled barrier resolves first.
    sh_compose: ShComposeResources,

    /// Absent Lightmap section → 1×1 white/neutral placeholder; no shader branch.
    lightmap_resources: LightmapResources,

    animated_lightmap: animated_lightmap::AnimatedLightmapResources,

    #[allow(dead_code)]
    lights_buffer: wgpu::Buffer,
    /// Last bytes uploaded to `lights_buffer`. Reused each frame to skip a
    /// redundant `queue.write_buffer` when the packed bytes are unchanged.
    last_lights_upload: Vec<u8>,
    /// Scratch buffer for the fallback full-repack path. Used only when
    /// `last_lights_upload` is not yet sized to the current light set
    /// (first frame or light-count change). The hot path patches
    /// `last_lights_upload` in place via `patch_shadow_slots` — scratch
    /// is not touched in that branch.
    lights_pack_scratch: Vec<u8>,
    #[allow(dead_code)]
    level_lights: Vec<MapLight>,
    /// Candidate set for the spot-shadow pool — sourced from the FULL level
    /// light set filtered by `is_dynamic`. Dynamic-tier lights
    /// (`light_dynamic`/`light_dynamic_spot`) are pool-eligible so dynamic
    /// spotlights shadow static world occluders (pillars). The per-light
    /// `casts_entity_shadows` toggle (FGD `_cast_entity_shadows`) gates only
    /// whether moving-ENTITY occluders draw into the slot, not slot allocation.
    shadow_candidate_lights: Vec<MapLight>,
    /// Lights near zero are excluded from shadow slot ranking. Empty = no suppression.
    light_effective_brightness: Vec<f32>,
    /// Cached from `update_per_frame_uniforms` so the shadow pass can re-rank lights.
    last_camera_position: Vec3,
    /// Cached camera `view_proj` from `update_per_frame_uniforms`; the shadow
    /// pool derives camera frustum planes from it for cone-frustum culling.
    last_view_proj: Mat4,
    spot_shadow_pool: SpotShadowPool,
    /// Dynamic point-light cube-array shadow pool. `None` when the adapter lacks
    /// `CUBE_ARRAY_TEXTURES` — point shadows then cleanly off, spot unaffected.
    /// `Some` iff `cube_array_supported`, so its presence mirrors group-5 binding
    /// 5's presence in the shared BGL.
    cube_shadow_pool: Option<crate::lighting::cube_shadow::CubeShadowPool>,
    /// Per-(cube slot, face) light-space matrix uniforms, dynamic-offset like
    /// `shadow_vs_uniform_buffer`. Slot `slot*6 + face` carries that face's
    /// matrix; the skinned-depth pass selects it by dynamic offset.
    cube_shadow_vs_uniform_buffer: wgpu::Buffer,
    cube_shadow_vs_bind_group: wgpu::BindGroup,
    /// Dynamic-offset into a single buffer; offset selects the per-slot light-space matrix.
    shadow_vs_uniform_buffer: wgpu::Buffer,
    shadow_vs_bind_group: wgpu::BindGroup,
    shadow_depth_pipeline: wgpu::RenderPipeline,
    /// Rounded up to `min_uniform_buffer_offset_alignment`.
    shadow_vs_stride: u32,

    depth_view: wgpu::TextureView,

    /// Post-scene compositor seam: owns the `scene_color` offscreen target every
    /// gameplay scene/UI pass renders into, plus the resolve pass that blits it
    /// to the swapchain (the sole gameplay-path swapchain writer). Recreated on
    /// resize alongside `depth_view`. See `render/screen_effects.rs`.
    screen_effects: ScreenEffectsPass,

    /// GPU textures indexed by texture index.
    gpu_textures: Vec<GpuTexture>,
    bvh_leaves: Vec<crate::geometry::BvhLeaf>,
    /// `None` for maps with no BVH.
    compute_cull: Option<ComputeCullPipeline>,
    /// Per-slot cone cull for the spot-shadow depth passes. Sibling to
    /// `compute_cull`, sharing its read-only BVH node/leaf buffers. `None` for
    /// maps with no BVH (kept in lockstep with `compute_cull`).
    shadow_cull: Option<crate::shadow_cull::ShadowCullPipeline>,

    wireframe_pipeline: wgpu::RenderPipeline,
    wireframe_index_buffer: wgpu::Buffer,
    wireframe_index_count: u32,
    wireframe_cull_status_bgl: wgpu::BindGroupLayout,
    wireframe_enabled: bool,

    #[cfg(feature = "dev-tools")]
    debug_lines: debug_lines::DebugLineRenderer,
    /// Navmesh overlay toggle, flipped by `Alt+Shift+N`. Read at the emit call
    /// site to decide whether to push region/portal debug lines this frame.
    #[cfg(feature = "dev-tools")]
    show_navmesh: bool,

    lighting_isolation: LightingIsolation,

    /// DYNAMIC baked-static-direct SH isolation (combined / direct-only /
    /// indirect-only). Separate from `lighting_isolation` (the forward/static
    /// control), so the dynamic-vs-static parity comparison stays valid.
    dynamic_direct_isolation: DynamicDirectIsolation,

    /// Debug selector for the SDF static-occluder shadow path. Mirrors
    /// `lighting_isolation` — panel-only dropdown, surfaces through
    /// `FrameUniforms.sdf_shadow_mode`.
    sdf_shadow_mode: SdfShadowMode,

    /// Dev toggle: force per-light SDF visibility to 1.0 in the forward shader.
    /// Panel checkbox; surfaces through `FrameUniforms.sdf_force_visibility_one`.
    /// Drives the no-double-count visual A/B (forced-1.0 must match the
    /// pre-change render). Seeded from the `POSTRETRO_SDF_FORCE_VISIBILITY_ONE`
    /// env flag at construction so a headless/no-UI run can exercise it too.
    sdf_force_visibility_one: bool,

    /// Toggled by Alt+Shift+V; `true` = AutoVsync, `false` = AutoNoVsync.
    vsync_enabled: bool,

    has_geometry: bool,

    debug_frame: u64,
    debug_prev_bitmask: (u32, u32),
    debug_prev_vp_hash: u32,
    debug_prev_visible: (&'static str, usize),

    /// Idle (no draw) on maps with no registered collections. See §7.4.
    smoke_pass: SmokePass,

    /// Skinned-mesh forward pass. Idle (no draw) until a model is uploaded via
    /// `load_skinned_model` (driven by the level-load model sweep at level
    /// install, once per distinct `prop_mesh` model).
    mesh_pass: mesh_pass::MeshPass,

    /// Per-frame skinned-mesh instance list: surviving (model handle,
    /// interpolated transform, phase seed) tuples. Refilled each frame via
    /// `set_mesh_draws` from the render-frame mesh collector (which culls each
    /// entity via `mesh_pass::mesh_visible` against the frame's `VisibleCells` +
    /// the `LevelWorld`, then emits survivors at their interpolated transform).
    /// Empty when no mesh entity is visible. Planned into per-model draw groups
    /// + palette runs each frame by `mesh_instances::plan_mesh_frame`.
    mesh_draws: Vec<mesh_instances::MeshInstanceInput>,

    /// Reusable bone-palette scratch for per-frame per-instance sampling.
    /// `sample_clip` clears then refills it per instance, so steady-state frames
    /// allocate nothing. Lives on the renderer (not in the GPU pass) — it is
    /// CPU-side pose data the pass merely uploads.
    bone_palette_scratch: Vec<crate::model::BonePaletteEntry>,

    /// Wall-clock of the last palette/instance-overflow warning (render clock),
    /// for rate-limiting (mirrors `EmitterBridge`'s `last_warn_time`). Overflow
    /// drops the excess instances; the warning fires at most once per second.
    mesh_overflow_last_warn: f32,

    /// CPU-side count of skinned ENTITY occluder instances submitted into spot
    /// shadow slots last frame, summed across slots (each instance counted once
    /// per slot it casts into). Mirrors `shadow-cone-cull`'s submitted-instance
    /// counter — no GPU readback. Verifies the "enemy outside the cone is not
    /// drawn" acceptance criterion: an instance the per-light cone cull rejects
    /// is never added here. Reset to 0 at the start of the spot-shadow depth loop.
    spot_entity_occluders_submitted: u32,

    /// CPU-side count of skinned ENTITY occluder instances submitted into CUBE
    /// (point-light) shadow faces last frame, summed across all occupied slots ×
    /// 6 faces (each instance counted once per face it casts into). Mirrors
    /// `spot_entity_occluders_submitted` — no GPU readback. Verifies that
    /// entity occluders render only for `entity_occluder_eligible` point lights
    /// and only when their bound intersects a face frustum. Reset to 0 at the
    /// start of the cube-shadow depth loop.
    cube_entity_occluders_submitted: u32,

    /// Instanced UI quad / 9-slice pass for panels and images plus glyphon text.
    /// Built alongside `fog`; records the splash (splash phase) and an empty draw
    /// list on the gameplay path (`render_frame_indirect`). Owns all UI GPU state.
    ui: ui::UiPass,

    /// The active splash logo's natural reference size (logical-reference px,
    /// `[width, height]`), derived from the uploaded texture's decoded pixel dims.
    /// `Some` between `install_splash_from_loaded` and `clear_splash`; the splash
    /// descriptor tree is rebuilt each frame and this size threads into the
    /// measure seam (keyed by `splash::SPLASH_LOGO_ASSET`) so the logo `image`
    /// node sizes content-driven from the real asset. `None` (frame 0 before
    /// install, and after level handoff) records no splash quads.
    splash_logo_size: Option<[f32; 2]>,

    /// Key→bind-group registry for `image` widget assets (only the
    /// pre-registered splash logo key). `install_splash_from_loaded` registers
    /// the uploaded logo PNG under `splash::SPLASH_LOGO_ASSET`; the UI pass
    /// resolves image batches' asset keys through it. Cleared by `clear_splash`.
    ui_images: ui::UiImageRegistry,

    /// Once-per-frame published read snapshot: the splash version/tagline line
    /// and the gameplay-path descriptor tree. Set by the App via `set_ui_snapshot`
    /// just before each render call; read when the UI pass records. Stored here so
    /// both render signatures stay stable.
    ui_snapshot: ui::UiReadSnapshot,

    /// Active UI theme: the token table every descriptor tree resolves its
    /// color/spacing/font slots against at build time. Defaults to
    /// `UiTheme::engine_default()` at construction; `set_ui_theme` installs an
    /// override (e.g. from a mod's theme document) and bumps `ui_theme_generation`.
    /// Both render paths (`record_splash_ui`, the gameplay block) resolve against
    /// this same instance — the splash's literals resolve to themselves, so the
    /// splash output is unchanged.
    ui_theme: ui::theme::UiTheme,
    /// Monotonic UI theme generation, bumped by `set_ui_theme`. The retained
    /// gameplay tree records the generation it was built against; a bump
    /// invalidates the resolved tokens baked into it, so `layout_gameplay_tree`
    /// rebuilds the tree on the next frame even when the descriptor is unchanged.
    ui_theme_generation: u64,

    /// Volumetric fog raymarch + composite. Active only when the level has
    /// at least one fog volume uploaded; otherwise the dispatch + composite
    /// are skipped (see `FogPass::active`).
    fog: FogPass,

    /// Per-BSP-leaf bitmask of overlapping fog volumes, loaded from PRL section
    /// 31 at level load. When `Some`, the fog pass ORs the masks of visible
    /// leaves each frame to derive the active fog-volume set, culling volumes
    /// not reachable from the camera. When `None` (maps predating section 31 or
    /// maps with no fog volume entities), culling is disabled and
    /// `compute_fog_cell_mask` falls back to `all_slots_mask` — all canonical
    /// slots are treated as active.
    fog_cell_masks: Option<Vec<u32>>,

    /// (min, max) AABBs of fog volumes that are active this frame. Refreshed
    /// each frame via `set_fog_aabbs`; consumed by `collect_fog_spot_lights`
    /// to drop dynamic spots whose influence sphere can't scatter into any
    /// active volume. Empty list short-circuits to "pass everything" —
    /// conservative because the fog pass itself is gated by `FogPass::active`.
    active_fog_aabbs: Vec<(Vec3, Vec3)>,
}

impl Renderer {
    /// The adapter's `max_texture_dimension_2d` limit. Exposed for callers that
    /// need it to construct CPU-side helpers (e.g. egui-winit's `State::new`
    /// caps emitted texture sizes against this). Keeps wgpu types from leaking
    /// across the renderer boundary; only the scalar limit escapes.
    #[cfg(feature = "dev-tools")]
    pub fn max_texture_dimension_2d(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    /// Lazily constructs the egui-wgpu renderer on first panel open. Idempotent:
    /// subsequent calls are no-ops. The init log fires exactly once per
    /// session, used by the acceptance criteria to verify lazy init.
    #[cfg(feature = "dev-tools")]
    pub fn ensure_debug_ui_gpu(&mut self) {
        if self.debug_ui_gpu.is_none() {
            self.debug_ui_gpu = Some(debug_ui::DebugUiGpu::new(
                &self.device,
                self.surface_config.format,
            ));
            log::info!("[DebugUi] GPU renderer initialized");
        }
    }

    /// Records the egui overlay pass against the surface texture. Caller
    /// (`App`) has already tessellated the frame's shapes into `paint_jobs`;
    /// the view + screen descriptor are built here so the wgpu boundary stays
    /// inside the renderer module. Loads the existing swapchain color and
    /// stores it back — no depth attachment.
    ///
    /// Egui overlay runs in a separate command encoder submission after the
    /// world draw, using LoadOp::Load to composite on top. This deviates from
    /// the spec's "before frame_timing.encode_resolve" placement — threading a
    /// shared encoder across the renderer/App boundary was more complex than
    /// the benefit justified.
    #[cfg(feature = "dev-tools")]
    pub fn render_debug_ui(
        &mut self,
        surface_texture: &wgpu::SurfaceTexture,
        textures_delta: egui::TexturesDelta,
        paint_jobs: Vec<egui::ClippedPrimitive>,
        pixels_per_point: f32,
    ) -> Result<()> {
        let surface_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let screen_desc = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.surface_config.width, self.surface_config.height],
            pixels_per_point,
        };

        self.ensure_debug_ui_gpu();
        let gpu = self
            .debug_ui_gpu
            .as_mut()
            .expect("ensure_debug_ui_gpu populated debug_ui_gpu");

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("egui Encoder"),
            });

        for (id, image_delta) in &textures_delta.set {
            gpu.renderer
                .update_texture(&self.device, &self.queue, *id, image_delta);
        }
        let user_cmd_bufs = gpu.renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen_desc,
        );

        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui Overlay Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &surface_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            gpu.renderer
                .render(&mut pass.forget_lifetime(), &paint_jobs, &screen_desc);
        }

        for id in &textures_delta.free {
            gpu.renderer.free_texture(id);
        }

        self.queue.submit(
            user_cmd_bufs
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        Ok(())
    }

    /// Geometry and textures installed later via `install_level_geometry` / `install_textures`.
    pub fn new(window: &Arc<Window>) -> Result<Self> {
        // Dummy buffers until `install_level_geometry` replaces them.
        let geometry: Option<&LevelGeometry> = None;
        let size = window.inner_size();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let surface = instance
            .create_surface(window.clone())
            .context("failed to create wgpu surface")?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .context("no suitable GPU adapter found")?;

        log::info!("[Renderer] GPU adapter: {}", adapter.get_info().name);

        let downlevel = adapter.get_downlevel_capabilities();
        let has_multi_draw_indirect = downlevel
            .flags
            .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION);
        if has_multi_draw_indirect {
            log::info!("[Renderer] Indirect execution supported (multi_draw_indexed_indirect)");
        } else {
            log::info!(
                "[Renderer] Indirect execution not supported — using singular draw_indexed_indirect fallback"
            );
        }

        // Cube-array support gates the dynamic point-light shadow pool. Absent →
        // the cube pool is disabled (None) and point shadows are cleanly off; the
        // spot path is entirely unaffected (no panic, no validation error).
        let cube_array_supported = downlevel
            .flags
            .contains(wgpu::DownlevelFlags::CUBE_ARRAY_TEXTURES);
        if cube_array_supported {
            log::info!("[Renderer] Cube-array textures supported (dynamic point shadows enabled)");
        } else {
            log::info!(
                "[Renderer] Cube-array textures unsupported — dynamic point-light shadows disabled"
            );
        }

        // FrameTiming=None → zero runtime cost when timing isn't requested or supported.
        let adapter_features = adapter.features();
        let gpu_timing_requested =
            std::env::var("POSTRETRO_GPU_TIMING").ok().as_deref() == Some("1");
        let gpu_timing_supported = adapter_features.contains(wgpu::Features::TIMESTAMP_QUERY);
        let enable_gpu_timing = gpu_timing_requested && gpu_timing_supported;
        // BC5-compressed normal maps are a hard requirement (not optional like
        // GPU timing): the .prm baker emits BC5 normal slots unconditionally.
        let mut required_features = wgpu::Features::TEXTURE_COMPRESSION_BC;
        if enable_gpu_timing {
            required_features |= wgpu::Features::TIMESTAMP_QUERY;
        } else if gpu_timing_requested && !gpu_timing_supported {
            log::warn!(
                "[Renderer] POSTRETRO_GPU_TIMING=1 requested but adapter \
                 lacks TIMESTAMP_QUERY support — running without GPU timing"
            );
        }

        // The forward pass binds more sampled textures per stage than wgpu's
        // *default* request (4 bind groups) would carry, so we request the exact
        // count the pipelines need. This stays under the WebGPU spec floor of 16
        // (`wgpu::Limits::defaults().max_sampled_textures_per_shader_stage`), and
        // every targeted backend reports far higher (Metal/AMD = 128) — the
        // adapter pre-check below confirms the granted maximum still covers it.
        //
        // Derived (14 when CUBE_ARRAY is supported, 13 without) from the actual
        // BGLs that compose the forward pipeline layout, so it can never drift from
        // the real binding count:
        //   Group 1 — material (3): diffuse, specular, normal
        //   Group 3 — SH volume (3): octahedral atlas + depth-moments
        //                            + direct static-light atlas (billboard samples it in
        //                              the VERTEX stage; entry is VERTEX | FRAGMENT so it
        //                              counts against the fragment budget; forward/fog
        //                              carry the entry but never sample it)
        //   Group 4 — lightmap (4): static irradiance, static dominant-direction,
        //                           animated-contribution atlas, animated dominant-direction
        //   Group 5 — shadow (4 with CUBE_ARRAY, else 3): spot-shadow depth array (binding 0),
        //                           SDF shadow factor (binding 3), scene depth (binding 4),
        //                           point-light cube-array depth (binding 5; present only when
        //                           CUBE_ARRAY_TEXTURES is supported)
        // 16 is the WebGPU spec floor and wgpu's `Limits::default()` value; it is
        // also the hard ceiling on Metal (macOS) and is universally supported on
        // all desktop adapters. We use it as a fixed design budget rather than
        // deriving the exact binding count here — the unit test
        // `forward_pipeline_sampled_texture_request_matches_bgl_definitions`
        // verifies that the derived count stays within this budget independently.
        const REQUIRED_SAMPLED_TEXTURES: u32 = 16;
        // Pull the count helpers onto the runtime path (they are otherwise
        // test-only), so overflowing the budget trips here in debug builds.
        // debug-only because CI has no GPU: a release panic at pipeline creation
        // would be uncatchable, and the headless test covers the same invariant.
        // `#[cfg(debug_assertions)]` on the statement: the count helper is itself
        // debug-only, so referencing it must vanish from release builds too (a bare
        // `debug_assert!` still *compiles* its arguments in release).
        #[cfg(debug_assertions)]
        debug_assert!(
            forward_pipeline_sampled_texture_count(cube_array_supported)
                <= REQUIRED_SAMPLED_TEXTURES,
            "forward pipeline sampled-texture count ({}) exceeds the requested \
             budget ({}); switch to bindless (TEXTURE_BINDING_ARRAY) rather than \
             raising the limit (16 is Metal's hard ceiling)",
            forward_pipeline_sampled_texture_count(cube_array_supported),
            REQUIRED_SAMPLED_TEXTURES
        );
        // Billboard lighting runs in `vs_main` (per-vertex SH indirect+direct,
        // static-specular, dynamic-diffuse); the group-6 instance storage buffer is
        // VERTEX-read (see §7.4). wgpu charges `max_storage_buffers_per_shader_stage` against
        // the BGL *entry* set per stage — every VERTEX-visible storage entry across the
        // Billboard Pipeline Layout's groups counts, read or not. The downlevel/WebGPU
        // default ceiling (we do not raise it — broad hardware compat for a
        // modder-friendly retro FPS) is 8. Six are genuinely vertex-read; if a shared
        // BGL re-widens an unused storage entry to VERTEX the count hits 9 and pipeline
        // creation fails on real GPUs (headless CI never triggers it). debug-only for
        // the same reason as the texture budget above.
        // Gated as a block: both the helper and the budget const are debug-only,
        // so neither is referenced in release (where the helper does not exist).
        #[cfg(debug_assertions)]
        {
            const MAX_VERTEX_STORAGE_BUFFERS: u32 = 8;
            debug_assert!(
                billboard_pipeline_vertex_storage_buffer_count() <= MAX_VERTEX_STORAGE_BUFFERS,
                "billboard pipeline VERTEX-visible storage-buffer count ({}) exceeds the \
                 downlevel-default max_storage_buffers_per_shader_stage ({}); trim VERTEX \
                 visibility from storage entries vs_main does not read, or consolidate \
                 buffers — do NOT raise the device limit (it breaks modest-spec adapters)",
                billboard_pipeline_vertex_storage_buffer_count(),
                MAX_VERTEX_STORAGE_BUFFERS
            );
        }
        const REQUIRED_STORAGE_TEXTURES: u32 = 4;
        // Stopgap: SH compose's flat delta-probe storage buffer outgrows the
        // WebGPU spec floor (128 MiB) on maps with many animated lights because
        // it bakes a dense AABB grid per light. 512 MiB covers current maps on
        // mainstream desktop adapters (which report 2 GiB+), but it is a
        // load-bearing dependency on above-spec hardware.
        // context/plans/drafts/perf-animated-sh-light-culling/index.md
        // tracks the fix: sparse per-light delta storage that keeps the total
        // binding under the 128 MiB spec floor regardless of light count.
        const REQUIRED_STORAGE_BUFFER_BINDING_SIZE: u64 = 512 * 1024 * 1024;
        // Lightmap atlases bake up to 8192² (see
        // `crates/level-compiler/src/lightmap_bake.rs::MAX_ATLAS_DIMENSION`).
        // The bake is a CLI with no GPU device, so its cap is a fixed constant —
        // the runtime makes that requirement explicit by requesting the limit
        // here and refusing under-spec adapters in the pre-check below. wgpu's
        // default for this field is already 8192; setting it explicitly
        // formalizes the dependency.
        const REQUIRED_MAX_TEXTURE_DIMENSION_2D: u32 = 8192;
        let adapter_limits = adapter.limits();
        let required_limits = wgpu::Limits {
            max_bind_groups: 8,
            max_sampled_textures_per_shader_stage: REQUIRED_SAMPLED_TEXTURES,
            max_storage_textures_per_shader_stage: REQUIRED_STORAGE_TEXTURES,
            max_storage_buffer_binding_size: REQUIRED_STORAGE_BUFFER_BINDING_SIZE,
            max_texture_dimension_2d: REQUIRED_MAX_TEXTURE_DIMENSION_2D,
            ..wgpu::Limits::default()
        };

        // Pre-check so an under-spec adapter fails with a named error here
        // rather than an opaque `request_device` rejection or a deferred
        // pipeline-creation crash.
        if !adapter_features.contains(wgpu::Features::TEXTURE_COMPRESSION_BC) {
            anyhow::bail!(
                "GPU adapter lacks required feature TEXTURE_COMPRESSION_BC \
                 (needed for BC5-compressed normal maps); this engine requires \
                 a desktop GPU with BC texture support"
            );
        }
        if adapter_limits.max_sampled_textures_per_shader_stage < REQUIRED_SAMPLED_TEXTURES {
            anyhow::bail!(
                "GPU adapter supports only {} sampled textures per shader stage; \
                 the forward pass requires {}",
                adapter_limits.max_sampled_textures_per_shader_stage,
                REQUIRED_SAMPLED_TEXTURES
            );
        }
        if adapter_limits.max_storage_textures_per_shader_stage < REQUIRED_STORAGE_TEXTURES {
            anyhow::bail!(
                "GPU adapter supports only {} storage textures per shader stage; \
                 the SH compose pass requires {}",
                adapter_limits.max_storage_textures_per_shader_stage,
                REQUIRED_STORAGE_TEXTURES
            );
        }
        if adapter_limits.max_storage_buffer_binding_size < REQUIRED_STORAGE_BUFFER_BINDING_SIZE {
            anyhow::bail!(
                "GPU adapter supports only {} bytes per storage buffer binding; \
                 the SH compose delta-probe buffer requires {} (stopgap limit — \
                 see context/plans/drafts/perf-animated-sh-light-culling/index.md \
                 for the sparse-storage fix that removes this requirement)",
                adapter_limits.max_storage_buffer_binding_size,
                REQUIRED_STORAGE_BUFFER_BINDING_SIZE
            );
        }
        // The lightmap irradiance + animated atlases (`Rgba16Float`) are sampled
        // with hardware linear filtering (group-4 BGL declares `filterable:true`).
        // Linear filtering of 16-bit-float textures is core WebGPU and mandated
        // on every targeted backend (Vulkan/Metal/DX12), but check anyway so a
        // non-filterable adapter fails here with a named message rather than an
        // opaque `create_bind_group` crash later. See context/lib/rendering_pipeline.md §4.
        if !crate::lighting::lightmap::atlas_format_filterable(&adapter) {
            anyhow::bail!(
                "[Renderer] GPU adapter does not support linear filtering of \
                 Rgba16Float; PostRetro requires it for lightmap irradiance \
                 sampling. All supported backends (Vulkan/Metal/DX12) provide \
                 this — an adapter lacking it is below the supported floor"
            );
        }
        // BC6H is the default irradiance storage at rest — the bake compresses
        // the irradiance atlas to `Bc6hRgbUfloat` and the runtime uploads it
        // through the same `Float { filterable: true }` BGL slot as the
        // uncompressed debug variant. `TEXTURE_COMPRESSION_BC` is already
        // required above; this fail-fast sibling check confirms the adapter
        // also advertises `FILTERABLE` for `Bc6hRgbUfloat` specifically, so a
        // misconfigured adapter fails here with a named message instead of an
        // opaque `create_bind_group` crash later. Matches the
        // `atlas_format_filterable` (`Rgba16Float`) check above.
        if !crate::lighting::lightmap::bc6h_irradiance_filterable(&adapter) {
            anyhow::bail!(
                "[Renderer] GPU adapter does not support linear filtering of \
                 Bc6hRgbUfloat; PostRetro requires it for the compressed \
                 lightmap irradiance atlas. All supported backends \
                 (Vulkan/Metal/DX12) provide this — an adapter lacking it is \
                 below the supported floor"
            );
        }
        // The lightmap bake's `MAX_ATLAS_DIMENSION` (8192) is a fixed CLI-side
        // constant chosen to match guaranteed device support. Mirror that
        // requirement here: a baked atlas can be up to 8192² in either axis, so
        // an adapter that grants less cannot host one. Fail-fast with a named
        // message rather than a deferred texture-creation crash. wgpu's default
        // floor is 8192, so any in-spec desktop adapter satisfies this.
        if adapter_limits.max_texture_dimension_2d < REQUIRED_MAX_TEXTURE_DIMENSION_2D {
            anyhow::bail!(
                "[Renderer] GPU adapter grants max_texture_dimension_2d = {}; \
                 PostRetro requires at least {} to host the lightmap atlas at \
                 its baked ceiling. All supported backends (Vulkan/Metal/DX12) \
                 provide this — an adapter granting less is below the supported floor",
                adapter_limits.max_texture_dimension_2d,
                REQUIRED_MAX_TEXTURE_DIMENSION_2D,
            );
        }

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Postretro Device"),
            required_features,
            required_limits,
            ..Default::default()
        }))
        .context("failed to create GPU device")?;

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: surface_caps.alpha_modes[0],
            desired_maximum_frame_latency: 2,
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);
        log::info!("[Renderer] vsync on");

        let has_geometry =
            geometry.is_some_and(|g| !g.vertices.is_empty() && !g.indices.is_empty());

        let (vertex_data, index_data, index_count) = if let Some(geom) =
            geometry.filter(|g| !g.vertices.is_empty() && !g.indices.is_empty())
        {
            let count = geom.indices.len() as u32;
            (
                cast_world_vertices_to_bytes(geom.vertices),
                bytemuck_cast_slice_u32(geom.indices),
                count,
            )
        } else {
            (
                vec![0u8; crate::geometry::WorldVertex::STRIDE], // one dummy vertex
                vec![0u8; 4],                                    // one dummy index
                0u32,
            )
        };

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("World Vertex Buffer"),
            contents: &vertex_data,
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("World Index Buffer"),
            contents: &index_data,
            usage: wgpu::BufferUsages::INDEX,
        });

        // Build a line-list index buffer from the triangle index buffer for the
        // wireframe overlay. Each triangle contributes its three edges as line
        // pairs. Shared edges are duplicated (cheap, and avoids a hash set).
        let (wireframe_index_data, wireframe_index_count) = if let Some(geom) =
            geometry.filter(|g| !g.vertices.is_empty() && !g.indices.is_empty())
        {
            let line_indices = build_line_indices_from_triangles(geom.indices);
            let count = line_indices.len() as u32;
            (bytemuck_cast_slice_u32(&line_indices), count)
        } else {
            (vec![0u8; 4], 0u32)
        };

        let wireframe_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Wireframe Line Index Buffer"),
            contents: &wireframe_index_data,
            usage: wgpu::BufferUsages::INDEX,
        });

        let view_proj = build_default_view_projection(
            surface_config.width as f32 / surface_config.height as f32,
        );
        let full_lights = geometry.map(|g| g.lights).unwrap_or(&[]);
        let full_influences = geometry.map(|g| g.light_influences).unwrap_or(&[]);
        let (level_lights, dynamic_influences) =
            filter_dynamic_lights(full_lights, full_influences);
        let (shadow_candidate_lights, _) =
            filter_entity_shadow_candidates(full_lights, full_influences);
        let light_count = level_lights.len() as u32;
        let ambient_floor = DEFAULT_AMBIENT_FLOOR;
        let sh_fast_env = std::env::var("POSTRETRO_SH_FAST").ok();
        let probe_occlusion_enabled =
            sh_volume::probe_occlusion_seed_from_fast_env(sh_fast_env.as_deref());
        let uniform_data = build_uniform_data(&FrameUniforms {
            view_proj,
            camera_position: Vec3::ZERO,
            ambient_floor,
            light_count,
            time: 0.0,
            lighting_isolation: LightingIsolation::Normal,
            indirect_scale: DEFAULT_INDIRECT_SCALE,
            // No level loaded yet — per-frame uniform upload in
            // `update_per_frame_uniforms` reflects `has_sdf_atlas()` +
            // `lightmap_mode()` once geometry installs.
            sdf_shadow_flags: 0,
            sdf_shadow_mode: SdfShadowMode::On,
            sdf_force_visibility_one: false,
            dynamic_direct_scale: DEFAULT_DYNAMIC_DIRECT_SCALE,
            dynamic_direct_isolation: DynamicDirectIsolation::Combined,
            // No level loaded yet — `has_direct` reflects the direct SH section
            // once geometry installs (see `update_per_frame_uniforms`).
            has_direct: false,
        });

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Uniform Buffer"),
            contents: &uniform_data,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Uniform Bind Group Layout"),
                entries: &uniform_bind_group_layout_entries(),
            });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Uniform Bind Group"),
            layout: &uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // Group 1: 0=diffuse(sRGB), 2=specular(R8), 3=shininess,
        //          4=normal(Rgba8Unorm, NOT sRGB; n = sample.rgb*2-1),
        //          5=aniso_sampler (linear+anisotropic, Post Retro).
        // Binding 1 is intentionally vacated (former nearest sampler); the
        // aniso sampler stays at 5 — non-contiguous bindings are valid.
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Material Bind Group Layout"),
                entries: &material_bind_group_layout_entries(),
            });

        let lighting_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Lighting Bind Group Layout"),
                entries: &lighting_bind_group_layout_entries(),
            });

        for (idx, light) in level_lights.iter().enumerate() {
            if light.is_dynamic && light.light_type == crate::prl::LightType::Directional {
                log::warn!(
                    "[Renderer] Dynamic directional light (light_sun) at index {} found — not supported. \
                     Will render unshadowed (diffuse + specular only).",
                    idx
                );
            }
        }

        // wgpu rejects zero-size storage buffers — pad to one dummy; light_count stays 0.
        let lights_data = if !level_lights.is_empty() {
            pack_lights(&level_lights)
        } else {
            vec![0u8; GPU_LIGHT_SIZE]
        };
        let lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Direct Lights Storage Buffer"),
            contents: &lights_data,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // BGL owned here so forward pipeline layout and shadow pool bind group share it.
        // The BGL carries bindings 3 (SDF shadow factor) and 4 (scene depth) — both
        // owned outside the pool. Binding 5 (point-light cube-array depth) is present
        // only when `cube_array_supported`; the shared BGL, the forward + fog
        // pipelines, and the shader variants all key off the same flag. The pool
        // itself is built later (after depth_view + sdf_shadow_pass exist) so its
        // bind group can reference those targets directly at construction.
        let spot_shadow_bgl = SpotShadowPool::bind_group_layout(&device, cube_array_supported);

        // Influence volume buffer — same dummy strategy as lights.
        let influence_data = if !dynamic_influences.is_empty() {
            influence::pack_influence(&dynamic_influences)
        } else {
            vec![0u8; 16]
        };
        let influence_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Light Influence Storage Buffer"),
            contents: &influence_data,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Specular-only static lights; 1-record dummy avoids zero-size storage binding.
        let spec_lights_data = {
            let packed = geometry
                .map(|g| pack_spec_lights(g.lights))
                .unwrap_or_default();
            if packed.is_empty() {
                vec![0u8; SPEC_LIGHT_SIZE]
            } else {
                packed
            }
        };
        let spec_lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Spec-Only Lights Storage Buffer"),
            contents: &spec_lights_data,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Absent → fallback payload with has_chunk_grid=0; shader iterates full spec buffer.
        let chunk_grid = match geometry.and_then(|g| g.chunk_light_list) {
            Some(sec) => ChunkGrid::from_section(sec),
            None => ChunkGrid::fallback(),
        };
        if chunk_grid.present {
            log::info!(
                "[Renderer] ChunkLightList active (spec-only path is spatially partitioned)"
            );
        } else {
            log::info!(
                "[Renderer] ChunkLightList absent — specular path iterates the full spec buffer"
            );
        }
        let chunk_grid_info_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Chunk Grid Info Uniform"),
            contents: &chunk_grid.grid_info,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let chunk_grid_offsets_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Chunk Grid Offset Table"),
                contents: &chunk_grid.offset_table,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
        let chunk_grid_indices_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Chunk Grid Index List"),
                contents: &chunk_grid.index_list,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });

        let lighting_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lighting Bind Group"),
            layout: &lighting_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: lights_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: influence_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: spec_lights_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: chunk_grid_info_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: chunk_grid_offsets_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: chunk_grid_indices_buffer.as_entire_binding(),
                },
            ],
        });

        // Sampler pool seeded with the placeholder's mip count of `1`. The
        // pool grows in `install_textures` once `LoadedTexture::mip_count`
        // values arrive from the .prm sidecars. Placeholders always pick up
        // the `1` entry; never miss this lookup.
        let mut mip_count_aniso_samplers: HashMap<u32, wgpu::Sampler> = HashMap::new();
        mip_count_aniso_samplers.insert(1, create_mip_aniso_sampler(&device, 1));

        // Construct an initial placeholder bind group so the world pipeline
        // has a bind group bound even before a level loads. Replaced wholesale
        // by `install_textures` when a `.prl` payload arrives.
        let mut loaded_textures: Vec<LoadedTexture> = Vec::new();
        let mut gpu_textures: Vec<GpuTexture> = Vec::new();
        {
            let placeholder = placeholder_loaded_texture(&device, &queue);
            let aniso_sampler = mip_count_aniso_samplers
                .get(&1)
                .expect("mip_count 1 aniso seeded above");
            let bind_group = build_material_bind_group(
                &device,
                &texture_bind_group_layout,
                &placeholder,
                aniso_sampler,
                Material::Default,
                "Placeholder Material",
            );
            loaded_textures.push(placeholder);
            gpu_textures.push(GpuTexture { bind_group });
        }

        let bvh_leaves: Vec<crate::geometry::BvhLeaf> =
            geometry.map(|g| g.bvh.leaves.clone()).unwrap_or_default();
        let compute_cull = geometry
            .filter(|g| !g.bvh.leaves.is_empty())
            .map(|g| ComputeCullPipeline::new(&device, g.bvh, has_multi_draw_indirect));
        // Sibling shadow cull owner shares the camera cull's read-only BVH
        // node/leaf buffers (uploaded once). Built/rebuilt in lockstep with it.
        let shadow_cull = compute_cull.as_ref().map(|c| {
            crate::shadow_cull::ShadowCullPipeline::new(
                &device,
                c.node_buffer(),
                c.leaf_buffer(),
                c.total_leaves(),
                c.bucket_ranges().to_vec(),
                c.has_multi_draw_indirect(),
            )
        });

        let (_depth_texture, depth_view) =
            create_depth_texture(&device, surface_config.width, surface_config.height);

        // Post-scene compositor seam: `scene_color` offscreen target + identity
        // resolve. Allocated at the sRGB surface format / surface size /
        // single-sample for byte-identical resolve (see `screen_effects.rs`).
        let screen_effects = ScreenEffectsPass::new(
            &device,
            surface_config.width,
            surface_config.height,
            surface_format,
        );

        let sh_volume_resources = ShVolumeResources::new(
            &device,
            &queue,
            geometry.and_then(|g| g.sh_volume),
            geometry.and_then(|g| g.direct_sh_volume),
            level_lights.len(),
            probe_occlusion_enabled,
        );

        let sdf_atlas_resources =
            SdfAtlasResources::new(&device, &queue, geometry.and_then(|g| g.sdf_atlas));
        let lightmap_mode = geometry
            .map(|g| g.lightmap_mode)
            .unwrap_or(crate::prl::LightmapMode::Shadowed);

        let compose_sh_volume = geometry
            .and_then(|g| g.sh_volume)
            .filter(|_| sh_volume_resources.present);
        let compose_delta_sh_volumes = geometry
            .and_then(|g| g.delta_sh_volumes)
            .filter(|_| sh_volume_resources.present);
        let sh_compose = ShComposeResources::new(
            &device,
            &sh_volume_resources,
            compose_sh_volume,
            compose_delta_sh_volumes,
            &uniform_bind_group_layout,
        );

        #[cfg(feature = "dev-tools")]
        let sh_delta_volumes_meta =
            collect_delta_volume_meta(geometry.and_then(|g| g.delta_sh_volumes));

        #[cfg(feature = "dev-tools")]
        let sh_probe_readback = sh_diagnostics::ShProbeReadback::new(
            &device,
            sh_volume_resources.grid_dimensions,
            sh_volume_resources.atlas_dimensions,
            sh_volume_resources.tile_dimension,
            sh_volume_resources.tile_border,
            sh_volume_resources.atlas_tiles_per_row,
        );

        let animated_lm_debug = animated_lightmap::AnimatedLmDebugConfig::from_env();
        // Source the animated atlas size from the same resolver the static
        // lightmap texture uses, so the two atlases are guaranteed to match (the
        // compose pass writes at absolute static-atlas coordinates; the forward
        // pass samples both with one normalized lightmap_uv).
        let lightmap_atlas_dimensions = crate::lighting::lightmap::usable_atlas_dimensions(
            geometry.and_then(|g| g.lightmap),
            device.limits().max_texture_dimension_2d,
        );
        let animated_lightmap = animated_lightmap::AnimatedLightmapResources::new(
            &device,
            geometry.and_then(|g| g.animated_light_weight_maps),
            geometry.and_then(|g| g.animated_light_chunks),
            &bvh_leaves,
            &sh_volume_resources.animation,
            &uniform_bind_group_layout,
            lightmap_atlas_dimensions,
            animated_lm_debug,
        )
        .map_err(|msg| anyhow::anyhow!("[Renderer] animated lightmap init failed: {msg}"))?;

        // Group 4: lightmap atlas. Animated-contribution atlas at binding 3 (real or 1×1 zero dummy).
        let lightmap_bind_group_layout = crate::lighting::lightmap::bind_group_layout(&device);
        let lightmap_resources = LightmapResources::new(
            &device,
            &queue,
            geometry.and_then(|g| g.lightmap),
            &lightmap_bind_group_layout,
            &animated_lightmap.forward_view,
            &animated_lightmap.direction_forward_view,
        );

        // SDF half-res shadow pass (Task 4). Always allocated — dispatch is
        // gated on `sdf_atlas_resources.present`. Owns the half-res factor
        // target and its own group-1 bind group.
        let sdf_shadow_sh_grid = build_sdf_shadow_sh_grid(
            geometry.and_then(|g| g.sh_volume),
            sh_volume_resources.present,
        );
        let sdf_shadow_pass = SdfShadowPass::new(
            &device,
            &sdf_atlas_resources.bind_group_layout,
            &depth_view,
            sh_volume_resources.make_depth_moment_view(),
            sdf_shadow::SdfShadowLightBuffers {
                spec_lights: &spec_lights_buffer,
                chunk_grid_info: &chunk_grid_info_buffer,
                chunk_offsets: &chunk_grid_offsets_buffer,
                chunk_indices: &chunk_grid_indices_buffer,
            },
            sdf_shadow_sh_grid,
            surface_config.width,
            surface_config.height,
        );

        // Cube point-shadow pool — built before the spot pool because the
        // spot-shadow bind group (the shared group-5 BGL) references the cube
        // sampling view at binding 5. Disabled (None) when the adapter lacks
        // CUBE_ARRAY_TEXTURES — in that case binding 5 is omitted from the BGL and
        // NO cube view (not even a dummy) is created, since a `CubeArray` view
        // itself requires the feature. `cube_shadow_pool.is_some()` therefore
        // mirrors `cube_array_supported` exactly.
        let cube_shadow_pool =
            crate::lighting::cube_shadow::CubeShadowPool::new(&device, cube_array_supported);
        let cube_sampling_view = cube_shadow_pool.as_ref().map(|p| &p.sampling_view);

        // Now that the SDF shadow factor target + scene depth view both
        // exist, build the spot-shadow pool — its bind group references
        // both targets at bindings 3/4 and (when present) the cube sampling view
        // at binding 5. See `SpotShadowPool::new` docs.
        let spot_shadow_pool = SpotShadowPool::new(
            &device,
            &spot_shadow_bgl,
            &sdf_shadow_pass.shadow_view,
            &depth_view,
            cube_sampling_view,
        );
        {
            use crate::lighting::spot_shadow::{
                SHADOW_DEPTH_FORMAT, SHADOW_MAP_RESOLUTION, SHADOW_POOL_SIZE,
            };
            // Depth32Float = 4 B/texel; MiB = bytes >> 20. Derived from the consts
            // so the log can't drift from the actual pool size (was a stale literal).
            let vram_mib = (SHADOW_POOL_SIZE as u64
                * SHADOW_MAP_RESOLUTION as u64
                * SHADOW_MAP_RESOLUTION as u64
                * 4)
                >> 20;
            log::info!(
                "[Renderer] Spot shadow pool initialized ({} × {}×{} {:?} = {} MiB VRAM)",
                SHADOW_POOL_SIZE,
                SHADOW_MAP_RESOLUTION,
                SHADOW_MAP_RESOLUTION,
                SHADOW_DEPTH_FORMAT,
                vram_mib,
            );
        }

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Textured Pipeline Layout"),
            bind_group_layouts: &[
                Some(&uniform_bind_group_layout),
                Some(&texture_bind_group_layout),
                Some(&lighting_bind_group_layout),
                Some(&sh_volume_resources.bind_group_layout),
                Some(&lightmap_bind_group_layout),
                Some(&spot_shadow_bgl),
            ],
            immediate_size: 0,
        });

        // On an adapter without CUBE_ARRAY_TEXTURES the shared group-5 BGL omits
        // binding 5, so the forward shader must not declare or sample the
        // `point_shadow_cube` binding. Derive that variant from the one source via
        // `strip_point_shadow_cube` (strips the binding decl, neutralizes
        // `sample_point_shadow` to a no-shadow constant) rather than maintaining a
        // second copy. When supported, the source is used verbatim.
        let forward_source: std::borrow::Cow<str> = if cube_array_supported {
            SHADER_SOURCE.into()
        } else {
            strip_point_shadow_cube(SHADER_SOURCE).into()
        };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Textured Shader"),
            source: wgpu::ShaderSource::Wgsl(forward_source),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Textured Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: crate::geometry::WorldVertex::STRIDE as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        // position: vec3<f32> at offset 0
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        // base_uv: vec2<f32> at offset 12
                        wgpu::VertexAttribute {
                            offset: 12,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        // normal_oct: u16x2 at offset 20
                        wgpu::VertexAttribute {
                            offset: 20,
                            shader_location: 2,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                        // tangent_packed: u16x2 at offset 24
                        wgpu::VertexAttribute {
                            offset: 24,
                            shader_location: 3,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                        // lightmap_uv: u16x2 at offset 28 (quantized 0..1 UV)
                        wgpu::VertexAttribute {
                            offset: 28,
                            shader_location: 4,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                    ],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                // Pre-pass filled the buffer; Equal test → one shade per pixel.
                // Write disabled to skip redundant rewrite of pre-pass values.
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Equal),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        // Wireframe: group 0 = uniforms, group 1 = per-leaf cull status from compute shader.
        let wireframe_cull_status_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Wireframe Cull Status BGL"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let wireframe_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Wireframe Pipeline Layout"),
                bind_group_layouts: &[
                    Some(&uniform_bind_group_layout),
                    Some(&wireframe_cull_status_layout),
                ],
                immediate_size: 0,
            });

        let wireframe_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Wireframe Shader"),
            source: wgpu::ShaderSource::Wgsl(WIREFRAME_SHADER_SOURCE.into()),
        });

        let wireframe_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Wireframe Pipeline"),
            layout: Some(&wireframe_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &wireframe_shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: crate::geometry::WorldVertex::STRIDE as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        wgpu::VertexAttribute {
                            offset: 12,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 20,
                            shader_location: 2,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 24,
                            shader_location: 3,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                    ],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            // Always so wireframe draws on top regardless of depth; write disabled
            // since the forward pass already holds the depth buffer contents.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &wireframe_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        let depth_prepass_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Depth Pre-Pass Pipeline Layout"),
            bind_group_layouts: &[Some(&uniform_bind_group_layout)],
            immediate_size: 0,
        });

        let depth_prepass_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Depth Pre-Pass Shader"),
            source: wgpu::ShaderSource::Wgsl(DEPTH_PREPASS_SHADER_SOURCE.into()),
        });

        let depth_prepass_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Depth Pre-Pass Pipeline"),
                layout: Some(&depth_prepass_layout),
                vertex: wgpu::VertexState {
                    module: &depth_prepass_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: crate::geometry::WorldVertex::STRIDE as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            // Shares the forward vertex buffer — only position used; rest declared to match.
                            wgpu::VertexAttribute {
                                offset: 0,
                                shader_location: 0,
                                format: wgpu::VertexFormat::Float32x3,
                            },
                            wgpu::VertexAttribute {
                                offset: 12,
                                shader_location: 1,
                                format: wgpu::VertexFormat::Float32x2,
                            },
                            wgpu::VertexAttribute {
                                offset: 20,
                                shader_location: 2,
                                format: wgpu::VertexFormat::Uint16x2,
                            },
                            wgpu::VertexAttribute {
                                offset: 24,
                                shader_location: 3,
                                format: wgpu::VertexFormat::Uint16x2,
                            },
                            // Lightmap UV — consumed by the fragment stage and
                            // written to the Rg16Float gbuffer slot below.
                            wgpu::VertexAttribute {
                                offset: 28,
                                shader_location: 4,
                                format: wgpu::VertexFormat::Uint16x2,
                            },
                        ],
                    }],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                // Unchanged from the vertex-only pre-pass: writes depth with a
                // `Less` test. The forward pass still re-tests with `Equal`.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                // Vertex-only depth pre-pass: no color attachment. The
                // lightmap-UV gbuffer MRT was removed with the animated
                // dominant-direction trace (the per-light SDF trace keys on
                // light position, not lightmap UV).
                fragment: None,
                multiview_mask: None,
                cache: None,
            });

        // Spot shadow pipeline: shared across all SHADOW_POOL_SIZE slots via dynamic-offset uniform.
        // Depth bias (constant=2, slope=1.5) suppresses acne without Peter-Panning.
        let shadow_vs_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shadow VS BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: std::num::NonZeroU64::new(64),
                },
                count: None,
            }],
        });
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Spot Shadow Pipeline Layout"),
                bind_group_layouts: &[Some(&shadow_vs_bgl)],
                immediate_size: 0,
            });
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Spot Shadow Shader"),
            source: wgpu::ShaderSource::Wgsl(SPOT_SHADOW_SHADER_SOURCE.into()),
        });
        let shadow_depth_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Spot Shadow Depth Pipeline"),
                layout: Some(&shadow_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shadow_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: crate::geometry::WorldVertex::STRIDE as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x3,
                        }],
                    }],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: crate::lighting::spot_shadow::SHADOW_DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState {
                        constant: 2,
                        slope_scale: 1.5,
                        clamp: 0.0,
                    },
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: None,
                multiview_mask: None,
                cache: None,
            });

        // min_uniform_buffer_offset_alignment required for dynamic-offset bindings.
        let min_ubo_align = device.limits().min_uniform_buffer_offset_alignment.max(64);
        let shadow_vs_stride = min_ubo_align;
        let shadow_vs_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Spot Shadow VS Uniforms"),
            size: (shadow_vs_stride as u64)
                * (crate::lighting::spot_shadow::SHADOW_POOL_SIZE as u64),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_vs_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Spot Shadow VS Bind Group"),
            layout: &shadow_vs_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &shadow_vs_uniform_buffer,
                    offset: 0,
                    size: std::num::NonZeroU64::new(64),
                }),
            }],
        });

        // --- Cube point-shadow VS uniforms -----------------------------------
        // The cube pool itself was built earlier (its sampling view feeds the
        // group-5 BGL). Its per-face light-space matrices ride a dynamic-offset
        // uniform buffer shaped exactly like `shadow_vs_*` (reusing
        // `shadow_vs_bgl`), one slot per `(cube slot, face)` pair. The
        // skinned-depth pipeline binds it at group 0 just like the spot path,
        // proving the cube-ready contract.
        //
        // Total capacity = `shadow_vs_stride × CUBE_COUNT × CUBE_FACES` (every face
        // of every slot gets its own dynamic-offset slot). A render selects a face
        // via dynamic offset = `layer * shadow_vs_stride`, where the layer index is
        // `slot * CUBE_FACES + face` (matching `CubeShadowPool::face_layer`).
        let cube_face_count =
            crate::lighting::cube_shadow::CUBE_COUNT * crate::lighting::cube_shadow::CUBE_FACES;
        let cube_shadow_vs_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Cube Shadow VS Uniforms"),
            size: (shadow_vs_stride as u64) * (cube_face_count as u64),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cube_shadow_vs_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Cube Shadow VS Bind Group"),
            layout: &shadow_vs_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &cube_shadow_vs_uniform_buffer,
                    offset: 0,
                    size: std::num::NonZeroU64::new(64),
                }),
            }],
        });

        let frame_timing = if enable_gpu_timing {
            log::info!("[Renderer] GPU timing enabled (POSTRETRO_GPU_TIMING=1)");
            let mut pass_labels = vec![""; TIMING_PAIR_COUNT];
            pass_labels[TIMING_PAIR_CULL] = "cull";
            pass_labels[TIMING_PAIR_ANIMATED_LM_COMPOSE] = "animated_lm_compose";
            pass_labels[TIMING_PAIR_DEPTH_PREPASS] = "depth_prepass";
            pass_labels[TIMING_PAIR_SDF_SHADOW] = "sdf_shadow";
            pass_labels[TIMING_PAIR_FORWARD] = "forward";
            pass_labels[TIMING_PAIR_SH_COMPOSE] = "sh_compose";
            pass_labels[TIMING_PAIR_SMOKE] = "smoke";
            Some(FrameTiming::new(&device, &queue, pass_labels))
        } else {
            None
        };

        // See: context/lib/rendering_pipeline.md §7.4
        let smoke_pass = SmokePass::new(
            &device,
            surface_format,
            DEPTH_FORMAT,
            &uniform_bind_group_layout,
            &lighting_bind_group_layout,
            &sh_volume_resources.bind_group_layout,
        );

        // Skinned-mesh pass: reuses the camera (group 0) + material (group 1)
        // layouts. `upload_identity_palette` pre-fills the palette at startup so
        // an un-sampled run renders in bind pose. Each frame `plan_and_upload`
        // samples every instance's clip into its palette run before the shadow
        // depth loop; `record_draws` then records the forward draw.
        let mut mesh_pass = mesh_pass::MeshPass::new(
            &device,
            surface_format,
            DEPTH_FORMAT,
            // The depth-only skinned pipeline writes the shadow-map depth format
            // and binds the world spot-shadow `shadow_vs_bgl` at group 0 (the
            // per-render light-space matrix, dynamic-offset per slot).
            crate::lighting::spot_shadow::SHADOW_DEPTH_FORMAT,
            &uniform_bind_group_layout,
            &texture_bind_group_layout,
            &shadow_vs_bgl,
            // Mesh group 4 uses the SUPERSET layout (shared SH entries + the
            // mesh-only dynamic-direct params uniform at binding 16).
            &sh_volume_resources.mesh_bind_group_layout,
            // Cube-array support pins the `Some`-iff-layout invariant: the mesh
            // group-2 BGL carries the b8 cube entry iff this is true, and the
            // no-cube shader strip is applied to the mesh source when it is false.
            cube_array_supported,
        );
        mesh_pass.upload_identity_palette(&queue);
        // Build the mesh group-2 dynamic-direct light bind group over the SAME
        // runtime buffers the forward `lighting_bind_group` binds: the
        // `is_dynamic`-filtered `lights_buffer` (b0), the influence-volume buffer
        // (b1), and forward's scripted-descriptor (b2) / anim-sample (b3) buffers.
        // Rebuilt on level load wherever those buffers reallocate (see
        // `set_geometry`).
        // b5–b8 alias the SAME pool-owned shadow resources forward binds at its
        // group 5: the spot pool's D2-array depth view (b5), its comparison
        // sampler (b6), its light-space-matrices uniform buffer (b7), and the cube
        // pool's `CubeArray` sampling view (b8 — `Some` iff `cube_array_supported`,
        // the `Some`-iff-layout invariant). These pool resources are stable for the
        // renderer's lifetime (the pools are never recreated), so they only ever
        // rebind here alongside the b0–b4 reallocation rebind on level load.
        mesh_pass.rebuild_light_bind_group(
            &device,
            &lights_buffer,
            &influence_buffer,
            &sh_volume_resources.scripted_light_descriptors,
            &sh_volume_resources.animation.anim_samples,
            &spot_shadow_pool.array_view,
            &spot_shadow_pool.compare_sampler,
            &spot_shadow_pool.matrices_buffer,
            cube_shadow_pool.as_ref().map(|p| &p.sampling_view),
        );

        // UI quad / 9-slice + text pass — sibling to fog. Owns all UI GPU state
        // (quad pipeline, glyphon atlas/renderer, white texel). The splash phase
        // and the gameplay path both record through it.
        let ui = ui::UiPass::new(&device, &queue, surface_format);

        let mut fog = FogPass::new(
            &device,
            surface_config.width,
            surface_config.height,
            crate::fx::fog_volume::clamp_fog_pixel_scale(0),
            &depth_view,
            &uniform_bind_group_layout,
            &sh_volume_resources.bind_group_layout,
            &spot_shadow_bgl,
            cube_array_supported,
        );
        // Swapchain may differ from the hardcoded Rgba8UnormSrgb default.
        fog.rebuild_composite_for_format(&device, surface_format);

        if has_geometry {
            log::info!(
                "[Renderer] Textured pipeline ready: {} indices, {} textures, bvh_leaves={}",
                index_count,
                gpu_textures.len(),
                bvh_leaves.len(),
            );
            log::info!(
                "[Renderer] Wireframe overlay pipeline ready: {} line indices",
                wireframe_index_count,
            );
        } else {
            log::info!("[Renderer] Pipeline ready (no geometry loaded)");
        }

        #[cfg(feature = "dev-tools")]
        let debug_lines = debug_lines::DebugLineRenderer::new(
            &device,
            surface_format,
            DEPTH_FORMAT,
            1,
            &uniform_bind_group_layout,
        );

        Ok(Self {
            device,
            queue,
            surface,
            surface_config,
            is_surface_configured: true,
            pipeline,
            depth_prepass_pipeline,
            frame_timing,
            vertex_buffer,
            index_buffer,
            index_count,
            uniform_buffer,
            uniform_bind_group,
            lighting_bind_group,
            light_count,
            mesh_dynamic_time: 0.0,
            ambient_floor,
            indirect_scale: DEFAULT_INDIRECT_SCALE,
            dynamic_direct_scale: DEFAULT_DYNAMIC_DIRECT_SCALE,
            probe_occlusion_enabled,
            sh_volume_resources,
            sdf_atlas_resources,
            sdf_shadow_pass,
            lightmap_mode,
            #[cfg(feature = "dev-tools")]
            sh_delta_volumes_meta,
            #[cfg(feature = "dev-tools")]
            sh_probe_readback,
            #[cfg(feature = "dev-tools")]
            freeze_time: false,
            #[cfg(feature = "dev-tools")]
            frozen_time: 0.0,
            sh_compose,
            lightmap_resources,
            animated_lightmap,
            lights_buffer,
            last_lights_upload: Vec::new(),
            lights_pack_scratch: Vec::new(),
            level_lights,
            shadow_candidate_lights,
            light_effective_brightness: Vec::new(),
            last_camera_position: Vec3::ZERO,
            last_view_proj: Mat4::IDENTITY,
            spot_shadow_pool,
            cube_shadow_pool,
            cube_shadow_vs_uniform_buffer,
            cube_shadow_vs_bind_group,
            shadow_vs_uniform_buffer,
            shadow_vs_bind_group,
            shadow_depth_pipeline,
            shadow_vs_stride,
            depth_view,
            screen_effects,
            gpu_textures,
            bvh_leaves,
            compute_cull,
            shadow_cull,
            wireframe_pipeline,
            wireframe_index_buffer,
            wireframe_index_count,
            wireframe_cull_status_bgl: wireframe_cull_status_layout,
            wireframe_enabled: false,
            #[cfg(feature = "dev-tools")]
            debug_lines,
            #[cfg(feature = "dev-tools")]
            show_navmesh: false,
            lighting_isolation: LightingIsolation::Normal,
            dynamic_direct_isolation: DynamicDirectIsolation::Combined,
            sdf_shadow_mode: SdfShadowMode::On,
            sdf_force_visibility_one: std::env::var("POSTRETRO_SDF_FORCE_VISIBILITY_ONE")
                .ok()
                .as_deref()
                == Some("1"),
            vsync_enabled: true,
            has_geometry,
            debug_frame: 0,
            debug_prev_bitmask: (u32::MAX, u32::MAX),
            debug_prev_vp_hash: u32::MAX,
            debug_prev_visible: ("init", usize::MAX),
            smoke_pass,
            mesh_pass,
            mesh_draws: Vec::new(),
            bone_palette_scratch: Vec::new(),
            mesh_overflow_last_warn: f32::NEG_INFINITY,
            spot_entity_occluders_submitted: 0,
            cube_entity_occluders_submitted: 0,
            ui,
            splash_logo_size: None,
            ui_images: ui::UiImageRegistry::default(),
            ui_snapshot: ui::UiReadSnapshot::default(),
            ui_theme: ui::theme::UiTheme::engine_default(),
            ui_theme_generation: 0,
            fog,
            fog_cell_masks: None,
            active_fog_aabbs: Vec::new(),
            texture_bind_group_layout,
            lighting_bind_group_layout,
            mip_count_aniso_samplers,
            loaded_textures,
            has_multi_draw_indirect,
            stored_texture_materials: Vec::new(),
            uniform_bind_group_layout,
            #[cfg(feature = "dev-tools")]
            debug_ui_gpu: None,
        })
    }

    /// First caller's `spec_intensity` and `lifetime` win — per-collection, not per-emitter.
    pub fn register_smoke_collection(
        &mut self,
        collection: &str,
        frames: &[SpriteFrame],
        spec_intensity: f32,
        lifetime: f32,
    ) {
        self.smoke_pass.register_collection(
            &self.device,
            &self.queue,
            collection,
            frames,
            spec_intensity,
            lifetime,
        );
    }

    /// Release all level-owned GPU resources while keeping the device, queue,
    /// surface, UI, and window-facing state alive for the no-level Frontend.
    pub fn release_level_resources(&mut self) {
        let empty_keys = TextureCacheKeysSection::default();
        let empty_texture_names: Vec<String> = Vec::new();
        let empty_materials: Vec<Material> = Vec::new();
        self.install_textures(
            &empty_texture_names,
            &empty_keys,
            Path::new(""),
            &empty_materials,
        );

        let empty_bvh = BvhTree {
            nodes: Vec::new(),
            leaves: Vec::new(),
            root_node_index: 0,
        };
        let empty_geometry = LevelGeometry {
            vertices: &[],
            indices: &[],
            bvh: &empty_bvh,
            lights: &[],
            light_influences: &[],
            sh_volume: None,
            lightmap: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
            delta_sh_volumes: None,
            direct_sh_volume: None,
            sdf_atlas: None,
            lightmap_mode: crate::prl::LightmapMode::default(),
            texture_materials: &empty_materials,
        };
        self.install_level_geometry(&empty_geometry);

        self.smoke_pass.clear_collections();
        self.mesh_pass.release_level_resources();
        self.mesh_draws.clear();
        self.bone_palette_scratch.clear();
        self.fog_cell_masks = None;
        self.active_fog_aabbs.clear();
        self.upload_fog_volumes(&[], &[], 0);
        self.upload_fog_points(&[]);
        self.set_fog_pixel_scale(0);
    }

    /// Replaces dummy buffers with real geometry; rebuilds lighting, SH, lightmap, and cull pipeline.
    /// See: context/lib/boot_sequence.md §3 (Level Install Order)
    pub fn install_level_geometry(&mut self, geometry: &LevelGeometry<'_>) {
        let has_geometry = !geometry.vertices.is_empty() && !geometry.indices.is_empty();

        // --- Vertex / index buffers ---
        let (vertex_data, index_data, index_count) = if has_geometry {
            let count = geometry.indices.len() as u32;
            (
                cast_world_vertices_to_bytes(geometry.vertices),
                bytemuck_cast_slice_u32(geometry.indices),
                count,
            )
        } else {
            (
                vec![0u8; crate::geometry::WorldVertex::STRIDE],
                vec![0u8; 4],
                0u32,
            )
        };
        self.vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("World Vertex Buffer"),
                contents: &vertex_data,
                usage: wgpu::BufferUsages::VERTEX,
            });
        self.index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("World Index Buffer"),
                contents: &index_data,
                usage: wgpu::BufferUsages::INDEX,
            });
        self.index_count = index_count;

        // --- Wireframe index buffer ---
        let (wireframe_index_data, wireframe_index_count) = if has_geometry {
            let line_indices = build_line_indices_from_triangles(geometry.indices);
            let count = line_indices.len() as u32;
            (bytemuck_cast_slice_u32(&line_indices), count)
        } else {
            (vec![0u8; 4], 0u32)
        };
        self.wireframe_index_buffer =
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Wireframe Line Index Buffer"),
                    contents: &wireframe_index_data,
                    usage: wgpu::BufferUsages::INDEX,
                });
        self.wireframe_index_count = wireframe_index_count;

        // --- Lights + lighting bind group ---
        let (level_lights, dynamic_influences) =
            filter_dynamic_lights(geometry.lights, geometry.light_influences);
        let (shadow_candidate_lights, _) =
            filter_entity_shadow_candidates(geometry.lights, geometry.light_influences);
        self.light_count = level_lights.len() as u32;

        let lights_data = if !level_lights.is_empty() {
            pack_lights(&level_lights)
        } else {
            vec![0u8; GPU_LIGHT_SIZE]
        };
        let lights_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Direct Lights Storage Buffer"),
                contents: &lights_data,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
        self.lights_buffer = lights_buffer;
        self.level_lights = level_lights;
        self.shadow_candidate_lights = shadow_candidate_lights;

        let influence_data = if !dynamic_influences.is_empty() {
            influence::pack_influence(&dynamic_influences)
        } else {
            vec![0u8; 16]
        };
        let influence_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Light Influence Storage Buffer"),
                contents: &influence_data,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });

        let spec_lights_data = {
            let packed = pack_spec_lights(geometry.lights);
            if packed.is_empty() {
                vec![0u8; SPEC_LIGHT_SIZE]
            } else {
                packed
            }
        };
        let spec_lights_buffer =
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Spec-Only Lights Storage Buffer"),
                    contents: &spec_lights_data,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                });

        let chunk_grid = match geometry.chunk_light_list {
            Some(sec) => ChunkGrid::from_section(sec),
            None => ChunkGrid::fallback(),
        };
        let chunk_grid_info_buffer =
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Chunk Grid Info Uniform"),
                    contents: &chunk_grid.grid_info,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                });
        let chunk_grid_offsets_buffer =
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Chunk Grid Offset Table"),
                    contents: &chunk_grid.offset_table,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                });
        let chunk_grid_indices_buffer =
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Chunk Grid Index List"),
                    contents: &chunk_grid.index_list,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                });

        self.lighting_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lighting Bind Group"),
            layout: &self.lighting_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.lights_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: influence_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: spec_lights_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: chunk_grid_info_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: chunk_grid_offsets_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: chunk_grid_indices_buffer.as_entire_binding(),
                },
            ],
        });

        // --- SH volume, sh_compose, lightmap, animated lightmap ---
        self.sh_volume_resources = ShVolumeResources::new(
            &self.device,
            &self.queue,
            geometry.sh_volume,
            geometry.direct_sh_volume,
            self.level_lights.len(),
            self.probe_occlusion_enabled,
        );

        // Rebuild the mesh group-2 dynamic-direct light bind group over the
        // just-reallocated runtime buffers — the `is_dynamic`-filtered
        // `lights_buffer` (b0), the fresh `influence_buffer` (b1), and the new
        // `sh_volume_resources` scripted-descriptor (b2) / anim-sample (b3)
        // buffers. The forward `lighting_bind_group` above is rebuilt for the same
        // reason; this mirrors it for the mesh pass so a level swap does not leave
        // the mesh group-2 bind group dangling at the prior level's buffers.
        // b5–b8 re-reference the SAME pool-owned shadow resources (stable for the
        // renderer's lifetime — the pools are never recreated), supplied here so the
        // shadow bindings rebind alongside the reallocated b0–b4. The cube view is
        // `Some` iff `cube_shadow_pool` is present (the `Some`-iff-layout invariant).
        let cube_sampling_view = self.cube_shadow_pool.as_ref().map(|p| &p.sampling_view);
        self.mesh_pass.rebuild_light_bind_group(
            &self.device,
            &self.lights_buffer,
            &influence_buffer,
            &self.sh_volume_resources.scripted_light_descriptors,
            &self.sh_volume_resources.animation.anim_samples,
            &self.spot_shadow_pool.array_view,
            &self.spot_shadow_pool.compare_sampler,
            &self.spot_shadow_pool.matrices_buffer,
            cube_sampling_view,
        );

        self.sdf_atlas_resources =
            SdfAtlasResources::new(&self.device, &self.queue, geometry.sdf_atlas);
        self.lightmap_mode = geometry.lightmap_mode;
        let compose_sh_volume = geometry
            .sh_volume
            .filter(|_| self.sh_volume_resources.present);
        let compose_delta_sh_volumes = geometry
            .delta_sh_volumes
            .filter(|_| self.sh_volume_resources.present);
        self.sh_compose = ShComposeResources::new(
            &self.device,
            &self.sh_volume_resources,
            compose_sh_volume,
            compose_delta_sh_volumes,
            &self.uniform_bind_group_layout,
        );
        #[cfg(feature = "dev-tools")]
        {
            self.sh_delta_volumes_meta = collect_delta_volume_meta(geometry.delta_sh_volumes);
            // Atlas dims (hence readback buffer size) change per level — rebuild.
            self.sh_probe_readback = sh_diagnostics::ShProbeReadback::new(
                &self.device,
                self.sh_volume_resources.grid_dimensions,
                self.sh_volume_resources.atlas_dimensions,
                self.sh_volume_resources.tile_dimension,
                self.sh_volume_resources.tile_border,
                self.sh_volume_resources.atlas_tiles_per_row,
            );
        }

        let lightmap_bgl = crate::lighting::lightmap::bind_group_layout(&self.device);
        let animated_lm_debug = animated_lightmap::AnimatedLmDebugConfig::from_env();
        let bvh_leaves: Vec<crate::geometry::BvhLeaf> = geometry.bvh.leaves.clone();
        // Match the animated atlas to the static lightmap atlas the same way the
        // constructor does — one resolver, one device limit, guaranteed-equal
        // dimensions (see `usable_atlas_dimensions`).
        let lightmap_atlas_dimensions = crate::lighting::lightmap::usable_atlas_dimensions(
            geometry.lightmap,
            self.device.limits().max_texture_dimension_2d,
        );

        let animated_lightmap_result = animated_lightmap::AnimatedLightmapResources::new(
            &self.device,
            geometry.animated_light_weight_maps,
            geometry.animated_light_chunks,
            &bvh_leaves,
            &self.sh_volume_resources.animation,
            &self.uniform_bind_group_layout,
            lightmap_atlas_dimensions,
            animated_lm_debug,
        );
        match animated_lightmap_result {
            Ok(al) => {
                self.lightmap_resources = LightmapResources::new(
                    &self.device,
                    &self.queue,
                    geometry.lightmap,
                    &lightmap_bgl,
                    &al.forward_view,
                    &al.direction_forward_view,
                );
                self.animated_lightmap = al;
            }
            Err(msg) => {
                log::error!(
                    "[Renderer] animated lightmap install failed: {msg} — level may render without lightmap"
                );
            }
        }

        // SDF half-res shadow pass — rebind to the freshly-loaded SH
        // depth-moment texture + static-light buffers. The pass itself is always
        // allocated; the dispatch is gated on `sdf_atlas_resources.present`,
        // which `install_level_geometry` may have just flipped.
        let sdf_shadow_sh_grid =
            build_sdf_shadow_sh_grid(geometry.sh_volume, self.sh_volume_resources.present);
        self.sdf_shadow_pass.rebuild_for_level(
            &self.device,
            &self.depth_view,
            self.sh_volume_resources.make_depth_moment_view(),
            sdf_shadow::SdfShadowLightBuffers {
                spec_lights: &spec_lights_buffer,
                chunk_grid_info: &chunk_grid_info_buffer,
                chunk_offsets: &chunk_grid_offsets_buffer,
                chunk_indices: &chunk_grid_indices_buffer,
            },
            sdf_shadow_sh_grid,
        );

        // --- BVH + compute cull ---
        self.bvh_leaves = bvh_leaves;
        self.compute_cull = if !self.bvh_leaves.is_empty() {
            Some(ComputeCullPipeline::new(
                &self.device,
                geometry.bvh,
                self.has_multi_draw_indirect,
            ))
        } else {
            None
        };
        // Rebuild the shadow cull owner against the freshly-uploaded BVH
        // buffers — its per-slot bind groups reference the camera cull's
        // node/leaf storage, so a stale reference would point at the old BVH.
        self.shadow_cull = self.compute_cull.as_ref().map(|c| {
            crate::shadow_cull::ShadowCullPipeline::new(
                &self.device,
                c.node_buffer(),
                c.leaf_buffer(),
                c.total_leaves(),
                c.bucket_ranges().to_vec(),
                c.has_multi_draw_indirect(),
            )
        });

        self.has_geometry = has_geometry;
        self.last_lights_upload.clear();
        self.lights_pack_scratch.clear();
        self.light_effective_brightness.clear();
        self.stored_texture_materials = geometry.texture_materials.to_vec();

        if has_geometry {
            log::info!(
                "[Renderer] Geometry installed: {} indices, bvh_leaves={}",
                self.index_count,
                self.bvh_leaves.len(),
            );
        }
    }

    /// Rebuilds all material bind groups from baked `.prm` mip sidecars.
    /// `texture_materials` must be parallel to `texture_names`; entries beyond
    /// its length fall back to `Material::Default`. Caller drives the order:
    /// `install_textures` runs before `install_level_geometry` because the
    /// uploaded diffuse dimensions feed `normalize_world_uvs`.
    /// See: context/lib/boot_sequence.md §3 (Level Install Order) · context/lib/build_pipeline.md
    pub fn install_textures(
        &mut self,
        texture_names: &[String],
        texture_cache_keys: &TextureCacheKeysSection,
        prm_cache_root: &Path,
        texture_materials: &[Material],
    ) {
        // Cache materials so `install_level_geometry` can also recompute the
        // per-leaf material lookup without re-deriving them. (Mirrors the
        // pre-refactor flow where geometry install populated this field.)
        self.stored_texture_materials = texture_materials.to_vec();

        let loaded = load_textures(
            &self.device,
            &self.queue,
            texture_names,
            texture_cache_keys,
            prm_cache_root,
        );

        // Sampler pool grows monotonically: every distinct `mip_count` seen in
        // this batch needs a sampler with matching `lod_max_clamp`. The `1`
        // entry seeded in `Renderer::new` covers placeholders; new mip counts
        // beyond `1` arrive here when real textures load.
        for tex in &loaded {
            self.mip_count_aniso_samplers
                .entry(tex.mip_count)
                .or_insert_with(|| create_mip_aniso_sampler(&self.device, tex.mip_count));
        }

        let mut gpu_textures: Vec<GpuTexture> = Vec::with_capacity(loaded.len());
        for (idx, tex) in loaded.iter().enumerate() {
            let aniso_sampler = self
                .mip_count_aniso_samplers
                .get(&tex.mip_count)
                .expect("aniso mip sampler must have been eagerly populated");
            let material = texture_materials
                .get(idx)
                .copied()
                .unwrap_or(crate::material::Material::Default);
            let bind_group = build_material_bind_group(
                &self.device,
                &self.texture_bind_group_layout,
                tex,
                aniso_sampler,
                material,
                &format!("Material {idx}"),
            );
            gpu_textures.push(GpuTexture { bind_group });
        }

        if gpu_textures.is_empty() {
            // No textures referenced by the level — keep the placeholder slot
            // so the world pipeline still has a bind group bound.
            let placeholder = placeholder_loaded_texture(&self.device, &self.queue);
            let aniso_sampler = self
                .mip_count_aniso_samplers
                .get(&1)
                .expect("mip_count 1 aniso sampler is seeded at Renderer::new");
            let bind_group = build_material_bind_group(
                &self.device,
                &self.texture_bind_group_layout,
                &placeholder,
                aniso_sampler,
                crate::material::Material::Default,
                "Placeholder Material",
            );
            self.loaded_textures = vec![placeholder];
            self.gpu_textures = vec![GpuTexture { bind_group }];
            log::info!("[Renderer] Textures installed: 1 (placeholder fallback)");
            return;
        }

        self.loaded_textures = loaded;
        self.gpu_textures = gpu_textures;
        log::info!("[Renderer] Textures installed: {}", self.gpu_textures.len());
    }

    /// Load one skinned model into the renderer's model cache: parse the glTF,
    /// resolve each submesh's material key (blake3 content-hash of the base-color
    /// PNG, the same recipe the level compiler uses to name `.prm` sidecars) to a
    /// `LoadedTexture`, build one bind group per distinct key, and upload to the
    /// mesh pass.
    ///
    /// Called once per distinct `prop_mesh` model by the level-load model sweep
    /// (after classname dispatch); spawning itself happens earlier in
    /// `prop_mesh::handle`. Returns `Some(tags)` on success (the model's glTF
    /// `extras` tags — currently unused by callers, a residual of the old spawn
    /// seam) or `None` on a load error, which also logs a `warn!` naming the path
    /// and leaves the entry uncached (that model renders nothing).
    ///
    /// The renderer owns the GPU upload + the cached skeleton + first clip
    /// (inside the mesh pass's model cache); the per-frame draw list
    /// (`mesh_draws`) is supplied each frame by the render-frame mesh collector
    /// via [`set_mesh_draws`], not seeded here.
    ///
    /// Open path vs. cache key are deliberately decoupled. The glTF file is
    /// opened from `content_root.join(model_rel)` (every other asset joins the
    /// content root), but the model is cached under the VERBATIM `model_rel`
    /// string — that is the `MeshComponent.model` handle the spawn attaches and
    /// the per-frame planner groups by, so the key must match it exactly (a
    /// joined key would miss the planner's `models.get(&group.model)` lookup and
    /// silently drop every draw). Re-loading the same handle replaces the cache
    /// entry (idempotent upload).
    ///
    /// [`set_mesh_draws`]: Self::set_mesh_draws
    pub fn load_skinned_model(
        &mut self,
        model_rel: &str,
        content_root: &Path,
        prm_cache_root: &Path,
    ) -> Option<Vec<String>> {
        let (model_path, handle) = resolve_model_open_path_and_handle(model_rel, content_root);
        let model = match crate::model::gltf_loader::load_model(&model_path) {
            Ok(m) => m,
            Err(err) => {
                log::warn!(
                    "[Model] skinned model load failed for {} : {err} — mesh pass idle",
                    model_path.display(),
                );
                return None;
            }
        };

        let submesh_materials = self.resolve_skinned_model_material(&model, prm_cache_root);

        let crate::model::gltf_loader::LoadedModel {
            mesh,
            skeleton,
            clips,
            tags,
            ..
        } = model;
        let clip_count = clips.len();
        // Name every parsed clip so a multi-clip asset surfaces its full set in
        // the load log (the cache retains them all; the per-frame palette samples
        // the first). Joined as "name (1.23s)" in glTF order.
        if !clips.is_empty() {
            let clip_summary = clips
                .iter()
                .map(|clip| format!("'{}' ({:.2}s)", clip.name, clip.duration))
                .collect::<Vec<_>>()
                .join(", ");
            log::info!(
                "[Model] skinned model animation: {} clip(s) [{}], {} joints",
                clip_count,
                clip_summary,
                skeleton.joints.len(),
            );
        }

        // `handle` (the verbatim cache key) was derived alongside the open path
        // by `resolve_model_open_path_and_handle` — see this method's doc. The
        // FULL clip set is handed to the cache; clip selection is a sibling plan.
        self.mesh_pass.insert_model(
            &self.device,
            handle,
            &mesh,
            submesh_materials,
            skeleton,
            clips,
        );

        log::info!(
            "[Model] skinned model uploaded: {} clip(s) parsed, {} tag(s)",
            clip_count,
            tags.len(),
        );
        Some(tags)
    }

    /// The clip metadata (name + duration) for a cached skinned model, in glTF
    /// (authored) index order, keyed by the same `model_handle` string
    /// `load_skinned_model` cached it under. Returns an empty `Vec` when the model
    /// is not cached or has no animation — no error, no panic.
    ///
    /// `pub` forwarder over the private `mesh_pass` (same seam as
    /// [`Renderer::skinned_model_clip_by_name`]). Consumed by the level-load model
    /// sweep (`main.rs`) to build the game-side clip tables.
    pub fn skinned_model_clip_metadata(&self, model_handle: &str) -> Vec<mesh_pass::ClipMetadata> {
        self.mesh_pass
            .model_clip_metadata(&crate::model::ModelHandle::from(model_handle))
    }

    /// Replace this frame's skinned-mesh instance list with the inputs emitted by
    /// the render-frame mesh collector (already culled, at interpolated
    /// transforms). Called once per frame in the collection sub-stage, before
    /// `render_frame_indirect`. The renderer plans these into per-model draw
    /// groups + palette runs and records the draws; it needs no world reference
    /// because the cull already happened game-side.
    pub fn set_mesh_draws(&mut self, instances: &[mesh_instances::MeshInstanceInput]) {
        self.mesh_draws.clear();
        self.mesh_draws.extend_from_slice(instances);
    }

    /// Reset per-level transient mesh-pass state at level load. `pub` forwarder
    /// over the private `mesh_pass`; called from the level-load model sweep at the
    /// model-cache install site (where each distinct model uploads). Empties the
    /// `"smooth"`-interrupt snapshot store and the per-entity palette cache —
    /// entity seeds are not stable across levels, so stale state must not survive.
    pub fn clear_mesh_pass_for_level_load(&mut self) {
        self.mesh_pass.clear_for_level_load();
    }

    /// Resolve each submesh's material key (content-hash hex → `.prm`) to a
    /// material bind group, returning one `(bind group, index range)` per
    /// submesh in submesh order for the mesh pass to draw.
    ///
    /// Dedup: one GPU material bind group is built per *distinct* key — a model
    /// reusing a material across primitives builds it once and shares it. Each
    /// submesh range is then paired with its (possibly shared) bind group. The
    /// dedup + range bookkeeping is the GPU-free [`plan_submesh_materials`];
    /// this method is the thin GPU layer that builds the bind groups.
    ///
    /// Degrades to a placeholder per distinct key when its key is absent/garbled
    /// or its `.prm` is missing. Model materials consume only diffuse; specular
    /// and normal always use neutral placeholders in this slice.
    fn resolve_skinned_model_material(
        &mut self,
        model: &crate::model::gltf_loader::LoadedModel,
        prm_cache_root: &Path,
    ) -> Vec<(wgpu::BindGroup, std::ops::Range<u32>)> {
        let plan = plan_submesh_materials(&model.submeshes);

        // Build one material bind group per distinct key (deduped). Indexed
        // parallel to `plan.distinct_keys` so each submesh draw indexes into it.
        let distinct_bind_groups: Vec<wgpu::BindGroup> = plan
            .distinct_keys
            .iter()
            .map(|key_hex| {
                let key = parse_blake3_key(key_hex);
                let tex = load_model_diffuse_texture(
                    &self.device,
                    &self.queue,
                    key_hex,
                    key,
                    prm_cache_root,
                );

                let aniso_sampler = self
                    .mip_count_aniso_samplers
                    .entry(tex.mip_count)
                    .or_insert_with(|| create_mip_aniso_sampler(&self.device, tex.mip_count));
                build_material_bind_group(
                    &self.device,
                    &self.texture_bind_group_layout,
                    &tex,
                    aniso_sampler,
                    Material::Default,
                    &format!("Skinned Model Material {key_hex}"),
                )
            })
            .collect();

        // The resulting Vec is moved into the mesh pass (ownership transfer), so
        // each slot must hold its own handle. Clone the shared handle (cheap Arc
        // clone inside wgpu) for submeshes that reuse a distinct material.
        plan.draws
            .into_iter()
            .map(|draw| (distinct_bind_groups[draw.distinct].clone(), draw.indices))
            .collect()
    }

    /// Normalize texel-space UVs on every BVH-leaf-bound vertex to `[0,1]`
    /// using the diffuse-texture dimensions just installed by
    /// `install_textures`. Runs on the main thread between `install_textures`
    /// and `install_level_geometry`. Reads `texture.width()`/`height()` off
    /// the wgpu textures owned by `self.loaded_textures` so the dimensions
    /// always match the actual upload.
    pub fn normalize_world_uvs(&self, world: &mut crate::prl::LevelWorld) {
        let mut normalized = vec![false; world.vertices.len()];
        for leaf in &world.bvh.leaves {
            let tex_idx = leaf.material_bucket_id as usize;
            let tex = match self.loaded_textures.get(tex_idx) {
                Some(t) => t,
                None => continue,
            };
            let w = tex.diffuse_texture.width();
            let h = tex.diffuse_texture.height();
            if w == 0 || h == 0 {
                continue;
            }
            let start = leaf.index_offset as usize;
            let count = leaf.index_count as usize;
            for i in start..start + count {
                if let Some(&idx) = world.indices.get(i) {
                    let vi = idx as usize;
                    if vi < normalized.len() && !normalized[vi] {
                        if let Some(vert) = world.vertices.get_mut(vi) {
                            vert.base_uv[0] /= w as f32;
                            vert.base_uv[1] /= h as f32;
                            normalized[vi] = true;
                        }
                    }
                }
            }
        }
    }

    /// Install the active splash: upload the logo (reusing the splash texture
    /// upload), build its UI bind group, and install the logo so the JSON-loaded
    /// splash descriptor records through the UI pass in `render_splash_frame`.
    /// May be called more than once (mod-override swap in splash frame 1).
    pub fn install_splash_from_loaded(
        &mut self,
        loaded: &crate::ui_texture::UiTexture,
    ) -> [u32; 2] {
        // Force the splash tree's one-time JSON load + parse now, at install
        // (early in boot), rather than lazily on the first splash frame's render.
        ui::splash::force_splash_tree_init();
        let (texture, dims) = splash::upload_splash_texture(&self.device, &self.queue, loaded);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.ui.make_texture_bind_group(&self.device, &view);
        // Register the logo under the splash's known asset key, so the splash
        // descriptor's `image` node resolves to this bind group through the
        // registry (only known keys are pre-registered).
        self.ui_images
            .register(ui::splash::SPLASH_LOGO_ASSET, texture, bind_group);
        // Shape the logo to the decoded image so it never stretches: its natural
        // reference size flows from the real pixel dims (content-driven via the
        // measure seam), not a hardcoded constant.
        self.splash_logo_size = Some(ui::splash::splash_logo_reference_size(dims));
        dims
    }

    /// The active splash's capture/passthrough mode, for the App to drive the
    /// input-dispatch seam (`UiDispatch::set_mode`). `None` when no splash is
    /// installed. The splash is non-interactive, so this reports `Passthrough`.
    pub fn splash_capture_mode(&self) -> Option<crate::input::UiCaptureMode> {
        self.splash_logo_size
            .map(|_| ui::splash::splash_capture_mode())
    }

    /// Store the once-per-frame read snapshot. The App calls this just before each
    /// render call (splash phase and gameplay path); the UI pass reads it when it
    /// records. Keeps both render signatures stable.
    pub fn set_ui_snapshot(&mut self, snapshot: ui::UiReadSnapshot) {
        self.ui_snapshot = snapshot;
    }

    /// Export the flat hit-test / focus rect list for the TOP gameplay-UI stack
    /// layer against the current surface viewport — the reverse twin of the
    /// app→renderer snapshot. The App reads this after a gameplay render (which
    /// laid out the stack) and feeds it to the focus engine the NEXT frame
    /// (N→N+1 in reverse). Empty when no gameplay layer is active. See: ui.md §4.
    pub fn export_ui_focus_rects(&self) -> ui::tree::FocusRectList {
        let viewport = [self.surface_config.width, self.surface_config.height];
        // Resolve each focusable button's `selected`/`checked` predicate (M13 G2)
        // against the same frame snapshot the draw build used, so the a11y readback
        // matches the author-wired highlight.
        self.ui.export_top_focus_rects(
            viewport,
            &self.ui_snapshot.slot_values,
            &self.ui_snapshot.cell_values,
        )
    }

    /// Install an override UI theme and bump the theme generation. Engine-side
    /// only (no script bridge): a caller hands a fully-merged `UiTheme` (e.g.
    /// `UiTheme::engine_default().with_override(&doc)`), which every subsequent
    /// descriptor build resolves its tokens against. Bumping the generation
    /// invalidates the retained gameplay tree's baked tokens, so the next gameplay
    /// frame rebuilds the tree with the new values even when its descriptor is
    /// unchanged. The splash re-derives its tree each frame, so it picks up the
    /// new theme on its next frame with no extra bookkeeping.
    //
    // The production caller is the G1b mod-init drain (`main.rs`): it merges a
    // mod's `theme` tokens over `engine_default` and installs the result here.
    // `Renderer` needs a GPU device, so this seam is exercised by running the
    // engine, not the CPU test suite; the merge it relies on is covered in
    // `theme.rs`.
    pub fn set_ui_theme(&mut self, theme: ui::theme::UiTheme) {
        self.ui_theme = theme;
        self.ui_theme_generation = self.ui_theme_generation.wrapping_add(1);
    }

    /// Install a runtime UI font face from owned TTF/OTF bytes (the net-new
    /// runtime path behind `UiPass`/glyphon's `FontSystem`; the engine's primary/mono
    /// faces are embedded at compile time). Renderer-owns-GPU: the glyphon
    /// `FontSystem` lives in the renderer, so the mod-init drain in `main.rs` reads
    /// the TTF bytes itself and hands them here. Returns `false` when the bytes
    /// register no face under `family` (a malformed file or a family-name
    /// mismatch), so the caller surfaces a named diagnostic and skips rather than
    /// leaving a `font` token silently resolving to a system fallback.
    pub fn register_ui_font(&mut self, family: &str, ttf_bytes: Vec<u8>) -> bool {
        self.ui.register_font(family, ttf_bytes)
    }

    /// Returns `Err` on swapchain failure; caller exits the event loop on error.
    pub fn render_splash_frame(&mut self) -> Result<()> {
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(tex) => tex,
            wgpu::CurrentSurfaceTexture::Suboptimal(tex) => {
                self.surface.configure(&self.device, &self.surface_config);
                tex
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.surface_config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                anyhow::bail!("surface lost during splash");
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                anyhow::bail!("surface validation error during splash");
            }
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Splash Frame Encoder"),
            });

        let viewport = [self.surface_config.width, self.surface_config.height];
        self.record_splash_ui(&mut encoder, &view, viewport);

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        Ok(())
    }

    /// Record the splash through the UI pass into `view`, clearing to black first.
    /// Calls `build_splash_descriptor` (clones the once-loaded `splash.json` tree,
    /// substitutes the version line) and lays the tree out via `UiPass::layout_tree`.
    /// The background fill is
    /// drawn as a separate first quad outside the tree. `encode` is called
    /// unconditionally with `LoadOp::Clear(BLACK)` — on frame 0 the draw lists are
    /// empty and the pass only applies the clear.
    fn record_splash_ui(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        viewport: [u32; 2],
    ) {
        // Background fill quad (drawn first, behind the panel) stays outside the
        // tree — it is a plain oversized letterbox fill, not part of the panel
        // composition. Projected through the `layout` path like before.
        let bg = ui::splash::SplashDescriptor::background_element(splash::splash_bg_rgba());
        let mut panel_list = ui::layout::project(&[bg], viewport);

        // Lay the splash descriptor tree out (panel/fill quads + logo image batch
        // + version text), rebuilt each frame from the stored logo size and the
        // snapshot's version line. Empty when no splash is installed (frame 0).
        let mut draw = ui::tree::UiDrawData::default();
        if let Some(logo_size) = self.splash_logo_size {
            let desc = ui::splash::build_splash_descriptor(&self.ui_snapshot.version_line);
            // The logo `image` node sizes from the asset's natural reference size
            // via the measure seam — thread it in keyed by the splash logo asset.
            let mut image_sizes = ui::tree::ImageSizes::new();
            image_sizes.insert(ui::splash::SPLASH_LOGO_ASSET.to_string(), logo_size);
            // The splash tree carries no state bindings, so it resolves against
            // an empty slot map — behavior unchanged from before binding landed.
            let empty_slots = std::collections::HashMap::new();
            draw = self.ui.layout_tree(
                desc.tree(),
                viewport,
                &image_sizes,
                &empty_slots,
                &self.ui_theme,
            );
        }

        // The tree's panel quads (border + fill) draw behind the logo/text, in
        // the white-texel batch with the background fill — panels + bg share the
        // 1×1 white texel, so they concatenate into one batch.
        panel_list
            .instances
            .extend_from_slice(&draw.quads.instances);

        let white_bg = self.ui.white_bind_group().clone();
        let mut batches: Vec<ui::UiBatch> = Vec::new();
        if !panel_list.is_empty() {
            batches.push(ui::UiBatch {
                list: &panel_list,
                bind_group: &white_bg,
            });
        }
        // Each image batch (the logo) binds the texture its asset key resolves to
        // through the registry. An unknown key degrades by skipping just that
        // batch. Logged at debug, not warn: this runs every frame with no dedup,
        // so a persistently-missing key would spam the log at warn level (§6.1).
        for (asset, list) in &draw.images {
            if list.is_empty() {
                continue;
            }
            match self.ui_images.resolve(asset) {
                Some(bind_group) => batches.push(ui::UiBatch { list, bind_group }),
                None => log::debug!(
                    "[Renderer] UI image asset key '{asset}' is not registered — skipping its draw"
                ),
            }
        }

        // Wrap the splash's assembled batches + text in a single-layer
        // composition — the same encode unit the gameplay modal stack funnels
        // through, so the splash also satisfies the once-per-composition prepare
        // guard. The splash builds its quads from a standalone `panel_list` plus
        // the tree's panel/logo/text draw data (not a `UiDrawData` stack), so it
        // borrows the assembled batches/text directly via `from_batches`.
        let composition = ui::UiComposition::from_batches(batches, draw.texts.clone());

        // The splash path ALWAYS opens the pass with the black clear, even when
        // the composition is empty (frame 0 before install) — the boot "frame-0
        // black" step depends on this. The gameplay-path empty-tree early-out is
        // separate (see `render_frame_indirect`).
        self.ui.encode(
            &self.device,
            &self.queue,
            encoder,
            view,
            viewport,
            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
            &composition,
        );
    }

    /// Clear the active splash + its logo registration so post-transition frames
    /// record no splash. The UI pass itself survives.
    pub fn clear_splash(&mut self) {
        self.splash_logo_size = None;
        self.ui_images.clear();
    }

    /// `true` when the loaded map carries a baked SH volume. The diagnostic
    /// panel queries this to render either live controls or a disabled-state label.
    #[cfg(feature = "dev-tools")]
    pub fn has_sh_volume(&self) -> bool {
        self.sh_volume_resources.present
    }

    /// `true` when the loaded map carries a baked SDF static-occluder atlas.
    /// The SDF shadow pass gates its dispatch on this; the SDF visibility
    /// applies to the per-light `sdf`-tagged diffuse/specular forward loops,
    /// not to `lm_irr`. Legacy PRLs report `false` and the renderer degrades
    /// cleanly to `main`-equivalent lighting.
    #[allow(dead_code)]
    pub fn has_sdf_atlas(&self) -> bool {
        self.sdf_atlas_resources.present
    }

    /// Borrow the SDF atlas resources. The SDF shadow pass consumes the
    /// bind group + layout here; no other pass should bind these — forward
    /// gets only an upsampled shadow-factor texture in group 5.
    #[allow(dead_code)]
    pub fn sdf_atlas_resources(&self) -> &SdfAtlasResources {
        &self.sdf_atlas_resources
    }

    /// Lightmap bake mode read from the PRL (Shadowed = visibility baked in).
    /// Under the disjoint-direct design, `sdf` lights are excluded from
    /// `lm_irr` at bake time, so the forward pass never multiplies SDF
    /// visibility into the static-lightmap term; this accessor is retained
    /// only for legacy-PRL compatibility.
    #[allow(dead_code)]
    pub fn lightmap_mode(&self) -> crate::prl::LightmapMode {
        self.lightmap_mode
    }

    /// Per-animated-light delta-volume metadata for the SH diagnostic overlay.
    /// Empty when the map has no delta SH volumes.
    #[cfg(feature = "dev-tools")]
    pub fn sh_delta_volumes(&self) -> &[sh_volume::DeltaVolumeMeta] {
        &self.sh_delta_volumes_meta
    }

    /// Emits SH diagnostic line segments into the renderer's per-frame debug-line
    /// buffer. Called from the frame loop between egui UI build and
    /// `render_frame_indirect`. The caller is responsible for clearing the
    /// debug-line buffer before this call (via `clear_debug_lines`) so the
    /// emit path stays purely additive and other debug-line producers can
    /// coexist; this also keeps the buffer bounded across early-return frames
    /// (Timeout/Occluded/Outdated) where `render_frame_indirect` skips its
    /// debug-line render pass.
    ///
    /// `visible_leaf_mask` is the same portal-reachable leaf mask passed to
    /// `render_frame_indirect`; the cells overlay colors each cell by the
    /// frame-visibility of the leaf its center sits in.
    #[cfg(feature = "dev-tools")]
    pub fn emit_sh_diagnostics(
        &mut self,
        state: &sh_diagnostics::ShDiagnosticsState,
        camera_pos: Vec3,
        world: &crate::prl::LevelWorld,
        visible_leaf_mask: &[bool],
    ) {
        // Drive the live atlas readback only while the irradiance overlay is
        // actually drawn — every other frame it costs nothing.
        let want_live_irradiance = state.show_markers
            && state.marker_mode == sh_diagnostics::MarkerMode::Irradiance
            && self.sh_volume_resources.present;
        self.sh_probe_readback.set_wanted(want_live_irradiance);

        sh_diagnostics::emit(
            state,
            &self.sh_volume_resources,
            &self.sh_delta_volumes_meta,
            camera_pos,
            world,
            visible_leaf_mask,
            &mut self.debug_lines,
        );
    }

    /// Emit navmesh diagnostic debug lines (region rectangles + portal edges)
    /// from the runtime nav graph. No-op while the overlay is toggled off.
    /// Must run after `clear_debug_lines` and before the frame's debug-line
    /// pass, mirroring `emit_sh_diagnostics`.
    #[cfg(feature = "dev-tools")]
    pub fn emit_nav_diagnostics(&mut self, graph: &crate::nav::NavGraph) {
        if !self.show_navmesh {
            return;
        }
        nav_diagnostics::emit(graph, &mut self.debug_lines);
    }

    /// Emit agent path/corridor diagnostic debug lines: the corridor from the
    /// agent's `position` through its remaining funnel waypoints (from `cursor`),
    /// plus a per-waypoint cross marker sized to the capsule `radius`. Gated by
    /// the same navmesh overlay toggle (`Alt+Shift+N`) so the path draws
    /// alongside the region/portal overlay. Must run after `clear_debug_lines`
    /// and before the frame's debug-line pass, mirroring `emit_nav_diagnostics`.
    ///
    /// Keeps all wgpu renderer-side (Renderer-owns-GPU): the call site hands in
    /// plain agent geometry, never a debug-line / wgpu handle. The render-private
    /// `nav_diagnostics::emit_agent_path` (it is `pub(super)`) is reached only
    /// through this wrapper.
    #[cfg(feature = "dev-tools")]
    pub fn emit_agent_path_overlay(
        &mut self,
        position: Vec3,
        path: &[Vec3],
        cursor: usize,
        radius: f32,
    ) {
        if !self.show_navmesh {
            return;
        }
        nav_diagnostics::emit_agent_path(position, path, cursor, radius, &mut self.debug_lines);
    }

    /// Flip the navmesh overlay on/off. Bound to `Alt+Shift+N`.
    #[cfg(feature = "dev-tools")]
    pub fn toggle_navmesh_overlay(&mut self) -> bool {
        self.show_navmesh = !self.show_navmesh;
        log::info!(
            "[Renderer] Navmesh overlay: {}",
            if self.show_navmesh { "on" } else { "off" },
        );
        self.show_navmesh
    }

    pub fn toggle_wireframe(&mut self) -> bool {
        self.wireframe_enabled = !self.wireframe_enabled;
        log::info!(
            "[Renderer] Wireframe overlay: {}",
            if self.wireframe_enabled { "on" } else { "off" },
        );
        self.wireframe_enabled
    }

    /// Direct setter used by the debug-panel dropdown. Logs only on actual
    /// transition so spam-clicks on the current mode stay quiet.
    #[cfg(feature = "dev-tools")]
    pub fn set_lighting_isolation(&mut self, mode: LightingIsolation) {
        if self.lighting_isolation != mode {
            self.lighting_isolation = mode;
            log::info!("[Renderer] Lighting isolation: {}", mode.label());
        }
    }

    #[cfg(feature = "dev-tools")]
    pub fn lighting_isolation(&self) -> LightingIsolation {
        self.lighting_isolation
    }

    /// Direct setter for the `SdfShadowMode`; used by the debug-panel dropdown.
    /// Logs only on transition so spam clicks on the current mode stay quiet.
    #[cfg(feature = "dev-tools")]
    pub fn set_sdf_shadow_mode(&mut self, mode: SdfShadowMode) {
        if self.sdf_shadow_mode != mode {
            self.sdf_shadow_mode = mode;
            log::info!("[Renderer] SDF shadow mode: {}", mode.label());
        }
    }

    #[cfg(feature = "dev-tools")]
    pub fn sdf_shadow_mode(&self) -> SdfShadowMode {
        self.sdf_shadow_mode
    }

    /// Dev toggle (panel checkbox): force per-light SDF visibility to 1.0 so
    /// the forward sdf-tag diffuse term lands unshadowed. The no-double-count
    /// A/B: forced-1.0 must reproduce the pre-change render.
    #[cfg(feature = "dev-tools")]
    pub fn set_sdf_force_visibility_one(&mut self, force: bool) {
        if self.sdf_force_visibility_one != force {
            self.sdf_force_visibility_one = force;
            log::info!("[Renderer] SDF force visibility 1.0: {force}");
        }
    }

    #[cfg(feature = "dev-tools")]
    pub fn sdf_force_visibility_one(&self) -> bool {
        self.sdf_force_visibility_one
    }

    #[cfg(feature = "dev-tools")]
    pub fn freeze_time(&self) -> bool {
        self.freeze_time
    }

    /// Pin/unpin `uniforms.time`. Used by the debug panel to freeze all
    /// curve-driven animation while diagnosing time-dependent artifacts.
    #[cfg(feature = "dev-tools")]
    pub fn set_freeze_time(&mut self, freeze: bool) {
        self.freeze_time = freeze;
    }

    /// Most recent averaged GPU-timing window, or `None` when GPU timing is
    /// disabled / no window has elapsed yet. The debug panel reads this each
    /// frame; the underlying snapshot is overwritten every
    /// `AVG_WINDOW_FRAMES` frames.
    #[cfg(feature = "dev-tools")]
    pub fn frame_timing_snapshot(&self) -> Option<&frame_timing::FrameTimingSnapshot> {
        self.frame_timing.as_ref().and_then(|t| t.last_window())
    }

    /// Rebuilds the swapchain via surface.configure (Alt+Shift+V diagnostic chord).
    pub fn toggle_vsync(&mut self) -> bool {
        self.vsync_enabled = !self.vsync_enabled;
        self.surface_config.present_mode = if self.vsync_enabled {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        self.surface.configure(&self.device, &self.surface_config);
        self.vsync_enabled
    }

    pub fn vsync_enabled(&self) -> bool {
        self.vsync_enabled
    }

    /// Camera owns aspect ratio; caller must also call `update_per_frame_uniforms`.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        let (_depth_texture, depth_view) = create_depth_texture(&self.device, width, height);
        self.depth_view = depth_view;
        // `scene_color` is surface-sized; recreate it (and rebuild the resolve
        // bind group) alongside the depth target.
        self.screen_effects.resize(&self.device, width, height);
        self.fog
            .resize(&self.device, width, height, &self.depth_view);
        // SDF shadow target is half-res relative to the surface; the depth view
        // also changed, so the pass bind group has to be rebuilt.
        self.sdf_shadow_pass
            .resize(&self.device, &self.depth_view, width, height);
        // Group-5 bind group references both the SDF shadow factor target
        // and the scene depth — both just got recreated, so rebuild. The cube
        // binding's presence is fixed for the renderer's lifetime: the pool is
        // `Some` iff the adapter supports CUBE_ARRAY_TEXTURES, so rebuild the BGL
        // with the same flag (its presence mirrors the pool's).
        let cube_array_supported = self.cube_shadow_pool.is_some();
        let spot_shadow_bgl = SpotShadowPool::bind_group_layout(&self.device, cube_array_supported);
        // The cube sampling view is surface-size-independent, but the group-5
        // bind group is fully rebuilt here, so re-reference it (`Some` when the
        // pool is present, `None` omits binding 5 to match the BGL).
        let cube_sampling_view = self.cube_shadow_pool.as_ref().map(|p| &p.sampling_view);
        self.spot_shadow_pool.rebuild_bind_group(
            &self.device,
            &spot_shadow_bgl,
            &self.sdf_shadow_pass.shadow_view,
            &self.depth_view,
            cube_sampling_view,
        );
        // The UI pass derives its device scale from `surface_config` at encode
        // time, so the splash needs no per-resize hook — it re-projects against
        // the new backbuffer size on the next `render_splash_frame`.
        self.is_surface_configured = true;
    }

    pub fn update_per_frame_uniforms(
        &mut self,
        view_proj: Mat4,
        camera_position: Vec3,
        script_time: f32,
    ) {
        // Animation clock is the level-relative `script_time` (the same clock
        // the light bridge evaluates animation curves against on the CPU). The
        // GPU scripted-light pulse, SH animation, and animated-lightmap compose
        // all wrap this via `fract(time / period + phase)`. Using wall-clock
        // here instead would desync the GPU-rendered brightness from the CPU
        // `effective_brightness` that gates shadow-pool eligibility, so the pool
        // would shadow lights other than the ones actually lit on screen.
        #[cfg(not(feature = "dev-tools"))]
        let time = script_time;
        // Dev-tools: hold `time` when frozen (debug aid), else track live time so
        // toggling the freeze on holds the current animation phase.
        //
        // Freeze stops BOTH clocks together. While `freeze_time` is set, `App`
        // reads it (`renderer.freeze_time()`) and stops advancing `script_time`
        // (main.rs), so the CPU light bridge's `effective_brightness` (which
        // gates shadow-pool eligibility) and this GPU `time` uniform hold the
        // same phase. The held `frozen_time` here matches that pinned
        // `script_time`, so CPU and GPU stay aligned under freeze — no
        // animation-phase desync for a shadow debugger to chase.
        #[cfg(feature = "dev-tools")]
        let time = if self.freeze_time {
            self.frozen_time
        } else {
            self.frozen_time = script_time;
            self.frozen_time
        };
        // The per-light SDF visibility multiply is enabled whenever a baked SDF
        // atlas is loaded — the half-res target's four channels then hold valid
        // K = 4 per-light slices. With the flag clear (legacy PRL / no atlas)
        // the forward skips the upsample and treats every light fully lit.
        let mut sdf_shadow_flags: u32 = 0;
        if self.sdf_atlas_resources.present {
            sdf_shadow_flags |= SDF_SHADOW_FLAG_ATLAS_PRESENT;
        }
        let data = build_uniform_data(&FrameUniforms {
            view_proj,
            camera_position,
            ambient_floor: self.ambient_floor,
            light_count: self.light_count,
            time,
            lighting_isolation: self.lighting_isolation,
            indirect_scale: self.indirect_scale,
            sdf_shadow_flags,
            sdf_shadow_mode: self.sdf_shadow_mode,
            sdf_force_visibility_one: self.sdf_force_visibility_one,
            dynamic_direct_scale: self.dynamic_direct_scale,
            dynamic_direct_isolation: self.dynamic_direct_isolation,
            has_direct: self.sh_volume_resources.has_direct,
        });
        self.queue.write_buffer(&self.uniform_buffer, 0, &data);
        self.last_camera_position = camera_position;
        self.last_view_proj = view_proj;
        // Cache this frame's `time` so the skinned-mesh group-2 params uniform
        // (`MeshLightParams.time`) is written from the SAME render-clock value —
        // the scripted-light curves the mesh dynamic loop evaluates must share the
        // forward pass's animation phase (and the CPU light bridge's, which gates
        // shadow-pool eligibility). Written from this single source, never
        // recomputed at the mesh draw.
        self.mesh_dynamic_time = time;

        // Mesh dynamic-direct uniform (group 4 binding 16). The mesh path reads
        // a trimmed camera uniform (no group-0 tail), so the scale/isolation/
        // has_direct knobs reach it through this dedicated uniform instead.
        self.sh_volume_resources.write_dynamic_direct_params(
            &self.queue,
            self.dynamic_direct_scale,
            self.dynamic_direct_isolation as u32,
        );

        // Must precede the compose and SH fragment passes (both read the descriptor buffer).
        self.sh_volume_resources
            .animation
            .upload_descriptors_if_dirty(&self.queue);
    }

    /// Flushed to GPU on the next `update_per_frame_uniforms` call.
    #[allow(dead_code)]
    pub fn set_animated_light_active(&mut self, slot: usize, active: bool) {
        self.sh_volume_resources.animation.set_active(slot, active);
    }

    /// Overwrite the entire 48-byte animation descriptor at `slot` in the
    /// animated-compose descriptor buffer. Used by the scripting bridge to
    /// route a `setLightAnimation` curve through the animated-baked compose
    /// path (Task 2c of `sdf-static-occluder-shadows`). Out-of-range slots
    /// log once then no-op (mirrors the dormant `set_active` behavior).
    /// Flushed to GPU on the next `update_per_frame_uniforms` call.
    pub fn write_animated_compose_descriptor(
        &mut self,
        slot: u32,
        bytes: &[u8; sh_volume::ANIMATION_DESCRIPTOR_SIZE],
    ) {
        self.sh_volume_resources
            .animation
            .write_descriptor(slot as usize, bytes);
    }

    /// Must run before `update_dynamic_light_slots` — slot assignment reads
    /// then patches this buffer. If the order is reversed, `update_dynamic_light_slots`
    /// runs first and seeds `last_lights_upload` with static bytes; the subsequent
    /// bridge upload overwrites the mirror with animated base data but skips
    /// re-patching the shadow slot, so the bridge's sentinel slot persists and
    /// the forward shader never samples the shadow map for that frame.
    pub fn upload_bridge_lights(&mut self, lights_bytes: &[u8]) {
        debug_assert_eq!(
            lights_bytes.len(),
            self.level_lights.len() * GPU_LIGHT_SIZE,
            "bridge produced {} bytes; expected {} × {} = {}",
            lights_bytes.len(),
            self.level_lights.len(),
            GPU_LIGHT_SIZE,
            self.level_lights.len() * GPU_LIGHT_SIZE,
        );
        if lights_bytes.is_empty() {
            return;
        }
        self.queue
            .write_buffer(&self.lights_buffer, 0, lights_bytes);
        // Keep the CPU mirror in lock-step with the GPU buffer. The bridge
        // packs animated base data with sentinel shadow slots; the shadow pool
        // (`update_dynamic_light_slots`) then patches the real slot field onto
        // this mirror and re-uploads. Without this sync `last_lights_upload`
        // stays the wrong length or holds stale bytes: `update_dynamic_light_slots`
        // checks `last_lights_upload.len() == expected_len` and takes the fallback
        // full static-repack path when the lengths mismatch, clobbering the
        // animated base data written here with static bytes.
        self.last_lights_upload.clear();
        self.last_lights_upload.extend_from_slice(lights_bytes);
    }

    /// Mismatched length logs a warning and skips upload — fail soft over crashing the frame.
    pub fn upload_bridge_descriptors(&mut self, descriptor_bytes: &[u8]) {
        let expected = self.level_lights.len() * sh_volume::ANIMATION_DESCRIPTOR_SIZE;
        if descriptor_bytes.len() != expected {
            log::warn!(
                "[Renderer] upload_bridge_descriptors: bridge produced {} bytes; \
                 expected {} × {} = {}. Skipping upload.",
                descriptor_bytes.len(),
                self.level_lights.len(),
                sh_volume::ANIMATION_DESCRIPTOR_SIZE,
                expected,
            );
            return;
        }
        if descriptor_bytes.is_empty() {
            return;
        }
        self.queue.write_buffer(
            &self.sh_volume_resources.scripted_light_descriptors,
            0,
            descriptor_bytes,
        );
    }

    /// Writes at scripted-region offset (after FGD samples).
    pub fn upload_bridge_samples(&mut self, samples_bytes: &[u8]) {
        if samples_bytes.is_empty() {
            return;
        }
        let offset = self.sh_volume_resources.scripted_sample_byte_offset as u64;
        self.queue.write_buffer(
            &self.sh_volume_resources.animation.anim_samples,
            offset,
            samples_bytes,
        );
    }

    /// Divide by 4 for float index; pass as `fgd_sample_float_count` to `LightBridge`.
    pub fn scripted_sample_byte_offset(&self) -> usize {
        self.sh_volume_resources.scripted_sample_byte_offset
    }

    pub fn level_lights(&self) -> &[MapLight] {
        &self.level_lights
    }

    /// Collects dynamic spots with a shadow slot this frame.
    /// Unslotted spots excluded — no usable light-space matrix in the shader.
    /// Pre-multiplies color × intensity × brightness; mirrors `FogVolumeBridge::update_points`.
    fn collect_fog_spot_lights(&self) -> Vec<crate::fx::fog_volume::FogSpotLight> {
        const BRIGHTNESS_SUPPRESSION_THRESHOLD: f32 = 0.01;
        let slot_assignment = &self.spot_shadow_pool.slot_assignment;
        if slot_assignment.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (light_idx, &slot) in slot_assignment.iter().enumerate() {
            if slot == crate::lighting::spot_shadow::NO_SHADOW_SLOT {
                continue;
            }
            let Some(light) = self.level_lights.get(light_idx) else {
                continue;
            };
            if !matches!(light.light_type, crate::prl::LightType::Spot) {
                continue;
            }
            let multiplier = self
                .light_effective_brightness
                .get(light_idx)
                .copied()
                .unwrap_or(1.0);
            if multiplier < BRIGHTNESS_SUPPRESSION_THRESHOLD {
                continue;
            }
            // Cull spots whose falloff sphere can't reach any active fog volume;
            // a non-overlapping spot contributes zero scatter in the raymarch.
            let center = Vec3::new(
                light.origin[0] as f32,
                light.origin[1] as f32,
                light.origin[2] as f32,
            );
            if !sphere_intersects_any_fog_aabb(center, light.falloff_range, &self.active_fog_aabbs)
            {
                continue;
            }
            let intensity = light.intensity * multiplier;
            out.push(crate::fx::fog_volume::FogSpotLight {
                position: [
                    light.origin[0] as f32,
                    light.origin[1] as f32,
                    light.origin[2] as f32,
                ],
                slot,
                direction: light.cone_direction,
                cos_outer: light.cone_angle_outer.cos(),
                color: [
                    light.color[0] * intensity,
                    light.color[1] * intensity,
                    light.color[2] * intensity,
                ],
                range: light.falloff_range,
            });
        }
        out
    }

    /// Bytes: tightly packed `[FogVolume]` in PRL order. `live_mask` bit `i` = slot `i` has density > 0.
    /// GPU repack happens in `render_frame_indirect` after the portal-cull mask is known.
    /// Empty input clears the list → `FogPass::active` returns false.
    pub fn upload_fog_volumes(&mut self, bytes: &[u8], planes: &[Vec<[f32; 4]>], live_mask: u32) {
        let stride = std::mem::size_of::<crate::fx::fog_volume::FogVolume>();
        if bytes.is_empty() {
            self.fog.set_canonical_volumes(&[], &[], 0);
            return;
        }
        if bytes.len() % stride != 0 {
            log::warn!(
                "[Renderer] upload_fog_volumes: byte length {} is not a multiple of \
                 FogVolume stride {}; skipping.",
                bytes.len(),
                stride,
            );
            // Zero the canonical list — otherwise stale volumes from the previous frame persist.
            self.fog.set_canonical_volumes(&[], &[], 0);
            return;
        }
        let volumes: &[crate::fx::fog_volume::FogVolume] = bytemuck::cast_slice(bytes);
        self.fog.set_canonical_volumes(volumes, planes, live_mask);
    }

    /// Installs per-cell fog visibility masks for a freshly loaded level and
    /// resets the fog pass's hysteresis timestamps in the same step.
    ///
    /// `None` = legacy PRL without section 31: all canonical slots treated active.
    /// `live_mask` still suppresses density-zero slots.
    ///
    /// Resetting hysteresis is part of the contract: without it, volumes from
    /// the previous level could ride the sticky window into the first frames
    /// of the new level. Because of that coupling, this method is only valid
    /// at level-load boundaries — mid-session fog-volume hot-reloads must use
    /// a different seam that preserves hysteresis state.
    pub fn install_fog_cell_masks_for_level(&mut self, masks: Option<Vec<u32>>) {
        self.fog_cell_masks = masks;
        self.fog.clear_for_level_load();
    }

    /// Must be called after bridge AABB cache is populated and before `collect_fog_spot_lights`.
    /// CPU-side culling data only — can't go through `upload_fog_volumes`.
    /// Empty slice clears the cache so spots aren't kept against a volume that turned off.
    pub fn set_fog_aabbs(&mut self, aabbs: &[(Vec3, Vec3)]) {
        self.active_fog_aabbs.clear();
        self.active_fog_aabbs.extend_from_slice(aabbs);
    }

    /// Bytes: tightly packed `[FogPointLight]`. Empty input zeroes `point_count`.
    pub fn upload_fog_points(&mut self, bytes: &[u8]) {
        let stride = std::mem::size_of::<crate::fx::fog_volume::FogPointLight>();
        if bytes.is_empty() {
            self.fog.point_count = 0;
            return;
        }
        if bytes.len() % stride != 0 {
            log::warn!(
                "[Renderer] upload_fog_points: byte length {} is not a multiple of \
                 FogPointLight stride {}; skipping.",
                bytes.len(),
                stride,
            );
            self.fog.point_count = 0;
            return;
        }
        let points: &[crate::fx::fog_volume::FogPointLight] = bytemuck::cast_slice(bytes);
        self.fog.upload_points(&self.queue, points);
    }

    /// Set the global `fog_pixel_scale` from worldspawn. No-op when unchanged.
    pub fn set_fog_pixel_scale(&mut self, scale: u32) {
        self.fog.set_pixel_scale(
            &self.device,
            scale,
            self.surface_config.width,
            self.surface_config.height,
            &self.depth_view,
        );
    }

    pub fn set_light_effective_brightness(&mut self, effective_brightness: &[f32]) {
        self.light_effective_brightness.clear();
        self.light_effective_brightness
            .extend_from_slice(effective_brightness);
    }

    /// Sub-0.01 lights excluded from slot ranking — animated-dark lights don't waste a shadow slot.
    /// Short/empty `effective_brightness` = all-1.0 (first frame runs before bridge).
    ///
    /// `light_reachable_leaf_mask` is the wider fog/light-reachable leaf set
    /// (includes empty `face_count == 0` portal-reachable leaves), not the
    /// face-visible set — lights in empty reachable leaves stay eligible.
    ///
    /// The **candidate set** is `self.shadow_candidate_lights`
    /// (full level lights filtered by `is_dynamic`), which is the same set as
    /// `self.level_lights` (also `is_dynamic`-filtered) modulo ordering.
    /// `effective_brightness` is keyed on `level_lights` indices though, so
    /// we re-key the per-candidate eligibility into the candidate index
    /// space below.
    pub fn update_dynamic_light_slots(
        &mut self,
        camera_position: Vec3,
        camera_near_clip: f32,
        effective_brightness: &[f32],
        light_reachable_leaf_mask: &[bool],
    ) {
        // Candidate set is `is_dynamic`-filtered; if the map has no dynamic
        // lights the pool stays empty — early-return without disturbing
        // previous slots.
        if self.shadow_candidate_lights.is_empty() {
            return;
        }

        // Empty light_reachable_leaf_mask = DrawAll. ALPHA_LIGHT_LEAF_UNASSIGNED = unassigned → always cull.
        const BRIGHTNESS_SUPPRESSION_THRESHOLD: f32 = 0.01;
        let mut visible_lights = vec![false; self.shadow_candidate_lights.len()];
        for (i, light) in self.shadow_candidate_lights.iter().enumerate() {
            let leaf_visible = if light.leaf_index == ALPHA_LIGHT_LEAF_UNASSIGNED {
                false
            } else if light_reachable_leaf_mask.is_empty() {
                true
            } else {
                let li = light.leaf_index as usize;
                li < light_reachable_leaf_mask.len() && light_reachable_leaf_mask[li]
            };
            if !leaf_visible {
                continue;
            }
            // Brightness suppression is indexed by `level_lights` (the
            // forward / scripted-bridge index space). For candidates not in
            // `level_lights` we have no per-frame brightness — treat as 1.0.
            let b = level_brightness_for_candidate(
                &self.level_lights,
                &self.shadow_candidate_lights[i],
                effective_brightness,
            )
            .unwrap_or(1.0);
            if b < BRIGHTNESS_SUPPRESSION_THRESHOLD {
                continue;
            }
            visible_lights[i] = true;
        }

        let slot_assignment = SpotShadowPool::rank_lights(
            &self.shadow_candidate_lights,
            camera_position,
            camera_near_clip,
            &visible_lights,
            &self.last_view_proj,
        );

        // Rank dynamic POINT lights into the cube pool and upload their per-face
        // matrices. Returns the candidate-indexed cube slot assignment (empty
        // when the pool is disabled), which is patched into the light buffer
        // below alongside the spot slots. Runs before the patch block so both
        // slot fields land in one upload.
        let stride = self.shadow_vs_stride as usize;
        let cube_slot_assignment = self.update_cube_light_slots(
            camera_position,
            camera_near_clip,
            &visible_lights,
            stride,
        );

        // The GPU lights buffer is keyed on `level_lights`. Translate slot
        // assignments from candidate-index space into `level_lights`-index
        // space by identity-matching (origin + light_type). Both sets are
        // `is_dynamic`-filtered snapshots of `world.lights`, so every candidate
        // is in `level_lights` and receives its slot normally. The cube
        // assignment is re-keyed the same way (empty → all-sentinel).
        let level_slots = slot_assignment_for_level_lights(
            &self.level_lights,
            &self.shadow_candidate_lights,
            &slot_assignment,
        );
        let level_cube_slots = if cube_slot_assignment.is_empty() {
            vec![crate::lighting::spot_shadow::NO_SHADOW_SLOT; self.level_lights.len()]
        } else {
            slot_assignment_for_level_lights(
                &self.level_lights,
                &self.shadow_candidate_lights,
                &cube_slot_assignment,
            )
        };

        // Patch the per-light spot AND cube shadow-slot fields onto the CPU
        // mirror of the light buffer, then re-upload only if a slot changed. The
        // mirror holds whatever was last uploaded — the animated bridge's base
        // bytes once it has run, otherwise this fn's static pack. Patching
        // (rather than re-packing static `level_lights`) is what lets the slots
        // and the bridge's animated base data coexist: the two writers share one
        // buffer, so a full re-pack here would clobber the animation, and the
        // bridge's sentinel slot would clobber the shadow. The spot slot rides
        // `cone_angles_and_pad.z` and the cube slot rides `.w` — disjoint bytes,
        // so the two patches compose. See `upload_bridge_lights`.
        let expected_len = self.level_lights.len() * crate::lighting::GPU_LIGHT_SIZE;
        if self.last_lights_upload.len() == expected_len {
            let spot_changed =
                crate::lighting::patch_shadow_slots(&mut self.last_lights_upload, &level_slots);
            let cube_changed =
                crate::lighting::patch_cube_slots(&mut self.last_lights_upload, &level_cube_slots);
            if spot_changed || cube_changed {
                self.queue
                    .write_buffer(&self.lights_buffer, 0, &self.last_lights_upload);
            }
        } else {
            // Mirror not yet sized to the current light set (before the first
            // bridge upload, or the light count changed): full static pack so
            // frame-zero still uploads valid lights + slots and seeds the mirror.
            let mut scratch = std::mem::take(&mut self.lights_pack_scratch);
            pack_lights_with_slots_into(&mut scratch, &self.level_lights, &level_slots);
            crate::lighting::patch_cube_slots(&mut scratch, &level_cube_slots);
            if scratch != self.last_lights_upload {
                self.queue.write_buffer(&self.lights_buffer, 0, &scratch);
                self.last_lights_upload.clear();
                self.last_lights_upload.extend_from_slice(&scratch);
            }
            self.lights_pack_scratch = scratch;
        }

        // Upload slot matrices to both fragment-side storage (group 5 binding 2)
        // and vertex-side dynamic-offset uniform buffer. Matrices come from
        // the candidate list — that's the index space `slot_assignment` is
        // keyed on.
        const MAT_BYTES: usize = 64;
        let mut fragment_matrices =
            vec![0u8; MAT_BYTES * crate::lighting::spot_shadow::SHADOW_POOL_SIZE];
        let mut vertex_uniforms =
            vec![0u8; stride * crate::lighting::spot_shadow::SHADOW_POOL_SIZE];
        // Reset the per-slot cone-matrix stash; reoccupied slots overwrite, the
        // rest stay `None` so the GPU cone cull skips them this frame. The
        // entity-occluder gate resets to `false` in lockstep.
        self.spot_shadow_pool.slot_cone_matrices =
            [None; crate::lighting::spot_shadow::SHADOW_POOL_SIZE];
        self.spot_shadow_pool.slot_entity_eligible =
            [false; crate::lighting::spot_shadow::SHADOW_POOL_SIZE];
        for (light_idx, &slot) in slot_assignment.iter().enumerate() {
            if slot == crate::lighting::spot_shadow::NO_SHADOW_SLOT {
                continue;
            }
            let candidate = &self.shadow_candidate_lights[light_idx];
            let m = crate::lighting::spot_shadow::light_space_matrix(candidate);
            // Stash the SAME light-space matrix uploaded to bind-group-5 below —
            // the shadow-depth render loop reads it to build this slot's cone
            // cull frustum planes (one source of truth, no recomputation).
            self.spot_shadow_pool.slot_cone_matrices[slot as usize] = Some(m);
            // Record whether this slot's occupant renders entity occluders. The
            // shadow-depth loop draws skinned occluders into the slot only when
            // this is set; an ineligible (e.g. toggle-off dynamic) slot keeps its
            // world shadow but draws none.
            self.spot_shadow_pool.slot_entity_eligible[slot as usize] =
                crate::lighting::entity_occluder_eligible(candidate);
            let cols = m.to_cols_array();
            let mut bytes = [0u8; MAT_BYTES];
            for (i, v) in cols.iter().enumerate() {
                bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_ne_bytes());
            }
            let slot_usize = slot as usize;
            fragment_matrices[slot_usize * MAT_BYTES..(slot_usize + 1) * MAT_BYTES]
                .copy_from_slice(&bytes);
            vertex_uniforms[slot_usize * stride..slot_usize * stride + MAT_BYTES]
                .copy_from_slice(&bytes);
        }
        self.queue.write_buffer(
            &self.spot_shadow_pool.matrices_buffer,
            0,
            &fragment_matrices,
        );
        self.queue
            .write_buffer(&self.shadow_vs_uniform_buffer, 0, &vertex_uniforms);

        self.spot_shadow_pool.slot_assignment = slot_assignment;
    }

    /// Rank dynamic POINT lights into the cube pool and write each occupied
    /// slot's 6 per-face light-space matrices into the cube VS uniform buffer.
    /// Returns the candidate-indexed cube slot assignment so the caller can
    /// patch each point light's cube slot into the forward light buffer
    /// (`cone_angles_and_pad.w`). An EMPTY return means the pool is disabled
    /// (adapter lacks `CUBE_ARRAY_TEXTURES`) — every point light then keeps the
    /// sentinel and does unshadowed attenuation.
    ///
    /// Shares the spot path's per-light eligibility (`visible_lights`) and the
    /// SHARED scoring/drop ranking core, so cube and spot slot assignment cannot
    /// drift. Cube faces are ENTITY-ONLY in v1 — `slot_entity_eligible` decides
    /// whether the depth loop draws anything into a slot at all.
    ///
    /// The RETURNED (shader-facing) assignment masks any light that owns a ranked
    /// slot but is not `entity_occluder_eligible` back to the sentinel: its cube
    /// faces are never cleared/rendered (the depth loop skips `None` matrices), so
    /// the shader must not sample that slot. See `cube_shadow::shader_facing_cube_slot`.
    /// The pool's internal `slot_assignment` keeps the raw rank for diagnostics.
    fn update_cube_light_slots(
        &mut self,
        camera_position: Vec3,
        camera_near_clip: f32,
        visible_lights: &[bool],
        stride: usize,
    ) -> Vec<u32> {
        use crate::lighting::cube_shadow;

        let Some(pool) = self.cube_shadow_pool.as_mut() else {
            return Vec::new();
        };

        let slot_assignment = cube_shadow::rank_point_lights(
            &self.shadow_candidate_lights,
            camera_position,
            camera_near_clip,
            visible_lights,
        );

        // Shader-facing slot assignment, returned to the caller and patched into
        // each point light's `cone_angles_and_pad.w`. It DIVERGES from the
        // internal `slot_assignment` for ineligible lights: see the per-light
        // masking below. Starts as a copy of the rank and is downgraded to the
        // sentinel for any light whose cube faces will not be rendered.
        let mut shader_slot_assignment = slot_assignment.clone();

        // Reset per-face matrices + per-slot entity gate; reoccupied faces
        // overwrite, the rest stay `None`/`false` so the render loop skips them.
        let face_count = cube_shadow::CUBE_COUNT * cube_shadow::CUBE_FACES;
        for m in pool.face_matrices.iter_mut() {
            *m = None;
        }
        for e in pool.slot_entity_eligible.iter_mut() {
            *e = false;
        }

        let mut vertex_uniforms = vec![0u8; stride * face_count];
        for (light_idx, &slot) in slot_assignment.iter().enumerate() {
            if slot == crate::lighting::spot_shadow::NO_SHADOW_SLOT {
                continue;
            }
            let candidate = &self.shadow_candidate_lights[light_idx];
            // Cube faces are entity-only: an ineligible point light draws
            // nothing, so it needs no per-face matrices either.
            let eligible = crate::lighting::entity_occluder_eligible(candidate);
            pool.slot_entity_eligible[slot as usize] = eligible;
            // CRITICAL: a cube slot's faces are only CLEARED + rendered when the
            // light is entity-eligible (the depth loop skips `None` face matrices).
            // An ineligible slot's faces hold stale/uninitialized depth, so the
            // shader must NOT sample them — `shader_facing_cube_slot` downgrades
            // those to the sentinel (unshadowed). Unlike the spot path, where every
            // occupied slot always renders a Clear(1.0)+world-depth baseline, a cube
            // face carries no world geometry and no clear, so sampling an
            // occluder-free face would read garbage (often fully shadowed) and ZERO
            // the light when its origin is on-screen (slots are only assigned to
            // visible lights — hence the view-dependence of the original bug).
            shader_slot_assignment[light_idx] =
                cube_shadow::shader_facing_cube_slot(slot, eligible);
            if !eligible {
                continue;
            }
            let face_mats = cube_shadow::cube_face_matrices(candidate);
            for (face, m) in face_mats.iter().enumerate() {
                let layer = cube_shadow::CubeShadowPool::face_layer(slot, face);
                pool.face_matrices[layer] = Some(*m);
                let cols = m.to_cols_array();
                let off = layer * stride;
                for (i, v) in cols.iter().enumerate() {
                    vertex_uniforms[off + i * 4..off + i * 4 + 4].copy_from_slice(&v.to_ne_bytes());
                }
            }
        }
        self.queue
            .write_buffer(&self.cube_shadow_vs_uniform_buffer, 0, &vertex_uniforms);

        pool.slot_assignment = slot_assignment;
        // Return the SHADER-facing assignment (ineligible lights masked to the
        // sentinel), not the raw rank — the caller patches this into the light
        // buffer, and only slots with rendered occluders may be sampled.
        shader_slot_assignment
    }

    /// Count of skinned entity occluder instances submitted into spot shadow
    /// slots last frame (summed across slots). The CPU-side verification for the
    /// out-of-cone acceptance criterion — an instance the per-light cone cull
    /// rejects is never tallied here. No GPU readback.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn spot_entity_occluders_submitted(&self) -> u32 {
        self.spot_entity_occluders_submitted
    }

    /// Count of skinned entity occluder instances submitted into CUBE point-light
    /// shadow faces last frame (summed across occupied slots × 6 faces). The
    /// CPU-side verification that entity occluders render only for eligible point
    /// lights and only inside a face frustum. No GPU readback.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn cube_entity_occluders_submitted(&self) -> u32 {
        self.cube_entity_occluders_submitted
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn ambient_floor(&self) -> f32 {
        self.ambient_floor
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_ambient_floor(&mut self, value: f32) {
        self.ambient_floor = value.clamp(0.0, 1.0);
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn indirect_scale(&self) -> f32 {
        self.indirect_scale
    }

    /// Takes effect on the next `update_per_frame_uniforms` upload.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_indirect_scale(&mut self, value: f32) {
        self.indirect_scale = value.clamp(0.0, 1.0);
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn dynamic_direct_scale(&self) -> f32 {
        self.dynamic_direct_scale
    }

    /// Takes effect on the next `update_per_frame_uniforms` upload.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_dynamic_direct_scale(&mut self, value: f32) {
        self.dynamic_direct_scale = value.clamp(0.0, 1.0);
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn dynamic_direct_isolation(&self) -> DynamicDirectIsolation {
        self.dynamic_direct_isolation
    }

    /// Takes effect on the next `update_per_frame_uniforms` upload.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_dynamic_direct_isolation(&mut self, mode: DynamicDirectIsolation) {
        self.dynamic_direct_isolation = mode;
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn probe_occlusion_enabled(&self) -> bool {
        self.probe_occlusion_enabled
    }

    /// Takes effect immediately for the SH grid uniform and persists across
    /// level reloads because `install_level_geometry` seeds rebuilt resources
    /// from this renderer state.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_probe_occlusion_enabled(&mut self, enabled: bool) {
        if self.probe_occlusion_enabled != enabled {
            self.probe_occlusion_enabled = enabled;
            self.sh_volume_resources
                .set_probe_occlusion_enabled(&self.queue, enabled);
            log::info!("[Renderer] Probe Occlusion: {enabled}");
        }
    }

    // --- Task 7: SDF / Fog quality-slider seams ---
    //
    // The SDF knobs live on `SdfShadowPass.tuning` — pure uniform scalars
    // packed each frame in `pack_params_bytes` (no resource rebuild). The fog
    // knobs split: `step_size` is a per-frame uniform repacked in
    // `upload_params`; `fog_pixel_scale` is a resource-rebuild knob already
    // owned by `set_fog_pixel_scale` above.

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn sdf_max_march_steps(&self) -> u32 {
        self.sdf_shadow_pass.tuning().max_march_steps
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_sdf_max_march_steps(&mut self, steps: u32) {
        self.sdf_shadow_pass.set_max_march_steps(steps);
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn sdf_open_space_skip_threshold(&self) -> f32 {
        self.sdf_shadow_pass.tuning().open_space_skip_threshold
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_sdf_open_space_skip_threshold(&mut self, threshold: f32) {
        self.sdf_shadow_pass
            .set_open_space_skip_threshold(threshold);
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn sdf_penumbra_k(&self) -> f32 {
        self.sdf_shadow_pass.tuning().penumbra_k
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_sdf_penumbra_k(&mut self, k: f32) {
        self.sdf_shadow_pass.set_penumbra_k(k);
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn sdf_surface_bias(&self) -> f32 {
        self.sdf_shadow_pass.tuning().surface_bias
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_sdf_surface_bias(&mut self, bias: f32) {
        self.sdf_shadow_pass.set_surface_bias(bias);
    }

    /// Current per-frame fog raymarch step size (world units). Read by the
    /// debug-UI slider on first draw so it shows the live value rather than
    /// the construction default.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn fog_step_size(&self) -> f32 {
        self.fog.step_size
    }

    /// Update the fog raymarch step size in place. `FogPass.step_size` is
    /// read by `upload_params` on the next frame, so this is a pure uniform
    /// write — no resource rebuild. Clamped to a positive minimum to guard
    /// against a runaway slider stalling the raymarch.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_fog_step_size(&mut self, step_size: f32) {
        self.fog.step_size = step_size.max(0.01);
    }

    /// Current `fog_pixel_scale` — read by the debug-UI slider on first draw.
    /// The setter (`set_fog_pixel_scale` above) drives a scatter-target
    /// rebuild rather than a per-frame uniform write.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn fog_pixel_scale(&self) -> u32 {
        self.fog.pixel_scale
    }

    pub fn is_ready(&self) -> bool {
        self.is_surface_configured
    }

    #[allow(dead_code)]
    pub fn has_compute_cull(&self) -> bool {
        self.compute_cull.is_some()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_frame_indirect(
        &mut self,
        visible: &VisibleCells,
        light_reachable_leaf_mask: &[bool],
        fog_reachable: &[u32],
        camera_leaf: Option<u32>,
        view_proj: Mat4,
        particle_collections: &[(&str, &[u8])],
        now_seconds: f64,
        clear_color: ClearColor,
        render_world: bool,
    ) -> Result<Option<wgpu::SurfaceTexture>> {
        self.debug_frame = self.debug_frame.wrapping_add(1);
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(tex) => tex,
            wgpu::CurrentSurfaceTexture::Suboptimal(tex) => {
                self.surface.configure(&self.device, &self.surface_config);
                tex
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.surface_config);
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                anyhow::bail!("surface lost");
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                anyhow::bail!("surface validation error");
            }
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Frame Encoder"),
            });

        // Same submission as render passes — no readback or GPU sync between cull and draw.
        if render_world {
            if let Some(cull) = &mut self.compute_cull {
                let cull_ts = self
                    .frame_timing
                    .as_ref()
                    .map(|t| t.compute_pass_writes(TIMING_PAIR_CULL));
                cull.dispatch(
                    &self.device,
                    &self.queue,
                    &mut encoder,
                    visible,
                    &view_proj,
                    cull_ts,
                );

                if log::log_enabled!(log::Level::Debug) {
                    let f = self.debug_frame;

                    let bm = cull.debug_bitmask_fingerprint();
                    if bm != self.debug_prev_bitmask {
                        log::debug!(
                            "[cull f={f}] visible-cell bitmask changed: pop={} hash={:#010x} (was pop={} hash={:#010x})",
                            bm.0,
                            bm.1,
                            self.debug_prev_bitmask.0,
                            self.debug_prev_bitmask.1,
                        );
                        self.debug_prev_bitmask = bm;
                    }

                    let mut vp_hash = 0u32;
                    for i in 0..4 {
                        let col = view_proj.col(i);
                        vp_hash ^= col.x.to_bits();
                        vp_hash ^= col.y.to_bits().rotate_left(7);
                        vp_hash ^= col.z.to_bits().rotate_left(13);
                        vp_hash ^= col.w.to_bits().rotate_left(19);
                    }
                    if vp_hash != self.debug_prev_vp_hash {
                        log::debug!("[cull f={f}] view_proj changed: hash={:#010x}", vp_hash);
                        self.debug_prev_vp_hash = vp_hash;
                    }

                    let cur_vis = match visible {
                        VisibleCells::Culled(cells) => ("Culled", cells.len()),
                        VisibleCells::DrawAll => ("DrawAll", 0),
                    };
                    if cur_vis != self.debug_prev_visible {
                        log::debug!(
                            "[cull f={f}] VisibleCells changed: {}(n={}) (was {}(n={}))",
                            cur_vis.0,
                            cur_vis.1,
                            self.debug_prev_visible.0,
                            self.debug_prev_visible.1,
                        );
                        self.debug_prev_visible = cur_vis;
                    }
                }
            }
        }

        // Before depth pre-pass: storage→sampled barrier must resolve before forward sampling.
        if render_world && self.animated_lightmap.is_active() {
            let animated_ts = self
                .frame_timing
                .as_ref()
                .map(|t| t.compute_pass_writes(TIMING_PAIR_ANIMATED_LM_COMPOSE));
            self.animated_lightmap.dispatch(
                &self.queue,
                &mut encoder,
                &self.uniform_bind_group,
                visible,
                animated_ts,
            );
        }

        // Before depth pre-pass: storage-write → sampled-read barrier for SH.
        if render_world {
            let sh_compose_ts = self
                .frame_timing
                .as_ref()
                .map(|t| t.compute_pass_writes(TIMING_PAIR_SH_COMPOSE));
            self.sh_compose
                .dispatch(&mut encoder, &self.uniform_bind_group, sh_compose_ts);
        }

        // The readback copy is deliberately not encoded here. A
        // `copy_texture_to_buffer` in the same command buffer as the compose
        // dispatch reads the `total` atlas texture before its storage writes
        // are visible, flickering garbage into the markers. It runs after a
        // blocking `poll(Wait)` below, once the compose submit has retired.

        // mem::take avoids a simultaneous borrow of self; returned after call to reuse the allocation.
        if render_world {
            let eff_brightness = std::mem::take(&mut self.light_effective_brightness);
            self.update_dynamic_light_slots(
                self.last_camera_position,
                crate::lighting::spot_shadow::SHADOW_NEAR_CLIP,
                &eff_brightness,
                light_reachable_leaf_mask,
            );
            self.light_effective_brightness = eff_brightness;
        }

        // --- Skinned-mesh pose/upload HOIST ----------------------------------
        // Plan + sample + upload the skinned-mesh palette/instance buffers HERE —
        // after `update_dynamic_light_slots`, BEFORE the spot-shadow depth loop —
        // so the skinned-depth shadow occluder pass and the forward mesh draw both
        // read the SAME already-posed buffers. Nothing rewrites `palette_buffer`/
        // `instance_buffer` between this point and the forward `record_draws`, so
        // an entity and its shadow are sampled at the identical pose (no one-frame
        // lag). The plan is held in `mesh_frame_plan` and consumed by both passes.
        let mesh_frame_plan: Option<mesh_instances::MeshFramePlan> =
            if render_world && self.mesh_pass.has_model() && !self.mesh_draws.is_empty() {
                // Plan: group instances by model, assign each a contiguous palette
                // run, drop any overflow past the fixed budget. GPU-free.
                let plan = mesh_instances::plan_mesh_frame(&self.mesh_draws, &self.mesh_pass);

                // Overflow drops excess instances rather than corrupting the
                // palette or panicking — rate-limited warning. Covers BOTH the
                // palette-slot cap and the instance-count cap (the latter is what
                // fires for rigid / zero-joint props, which consume no slots).
                if plan.dropped > 0 {
                    let now = now_seconds as f32;
                    if now - self.mesh_overflow_last_warn >= 1.0 {
                        log::warn!(
                            "[Renderer] skinned-mesh budget exceeded: dropped {} instance(s) \
                             (budget {} palette slots / {} instances); excess not drawn",
                            plan.dropped,
                            mesh_instances::MAX_PALETTE_ENTRIES,
                            mesh_instances::MAX_INSTANCES,
                        );
                        self.mesh_overflow_last_warn = now;
                    }
                }

                // Sample every instance's clip into its palette run + write the
                // per-instance SSBO. The ONLY per-frame write to these buffers —
                // both the shadow loop and the forward draw read them unchanged.
                self.mesh_pass
                    .plan_and_upload(&self.queue, &plan, &mut self.bone_palette_scratch);
                (!plan.groups.is_empty()).then_some(plan)
            } else {
                None
            };

        if render_world && self.has_geometry && self.index_count > 0 {
            let stride = self.shadow_vs_stride;
            let slot_assignment = self.spot_shadow_pool.slot_assignment.clone();
            let mut used_slots: Vec<u32> = slot_assignment
                .iter()
                .copied()
                .filter(|&s| s != crate::lighting::spot_shadow::NO_SHADOW_SLOT)
                .collect();
            used_slots.sort_unstable();
            used_slots.dedup();

            // Reset the per-frame entity-occluder counter; the per-slot cull
            // tallies into it below. Mirrors `shadow-cone-cull`'s submitted
            // counter — pure CPU, no GPU readback.
            self.spot_entity_occluders_submitted = 0;

            // Per-slot GPU cone cull: one compute pass loops the occupied slots,
            // dispatching BVH traversal into each slot's indirect sub-region
            // gated by that slot's cone frustum planes. Runs after the camera
            // BVH cull and before the per-slot depth render passes below, so the
            // sub-regions are populated when each slot draws indirect.
            if let Some(shadow_cull) = &self.shadow_cull {
                shadow_cull.dispatch_occupied_slots(
                    &self.queue,
                    &mut encoder,
                    &self.spot_shadow_pool.slot_cone_matrices,
                );
            }

            for slot in used_slots {
                let view = &self.spot_shadow_pool.views[slot as usize];
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Spot Shadow Depth Pass"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    ..Default::default()
                });
                pass.set_pipeline(&self.shadow_depth_pipeline);
                pass.set_bind_group(0, &self.shadow_vs_bind_group, &[slot * stride]);
                pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                // Indirect cone-culled draw from this slot's sub-region. The
                // depth-only shadow pipeline has no group-1 material slot, so
                // `None` skips the texture bind (matching the depth pre-pass).
                // Fall back to the full unconditional draw if the shadow cull
                // owner is absent (no BVH).
                if let Some(shadow_cull) = &self.shadow_cull {
                    shadow_cull.draw_slot_indirect(&mut pass, slot, None);
                } else {
                    pass.draw_indexed(0..self.index_count, 0, 0..1);
                }

                // Skinned ENTITY occluders into the SAME slot, through the
                // parameterized depth-only path: target view = this slot's depth
                // attachment (the `pass` above), light-space matrix = the
                // per-slot `shadow_vs_bind_group` + dynamic offset. This proves
                // the cube-ready contract — the pipeline takes the view + matrix
                // as per-render parameters, with no slot-count or 2D-target
                // assumption baked in. Reads the already-posed buffers from the
                // hoist (no rewrite since), so the occluder pose matches the
                // forward draw with no one-frame lag.
                //
                // TWO gates (kept separate from pool-slot eligibility):
                //   1. `slot_entity_eligible[slot]` — the slot's light passes
                //      `entity_occluder_eligible` (dynamic + toggle on). An
                //      ineligible slot keeps its world shadow (already drawn
                //      above) but draws ZERO entity occluders.
                //   2. per-instance cone cull inside `record_skinned_depth` —
                //      only instances whose transformed bound intersects this
                //      slot's cone are submitted.
                if let Some(plan) = &mesh_frame_plan {
                    if self.spot_shadow_pool.slot_entity_eligible[slot as usize] {
                        if let Some(cone_matrix) =
                            self.spot_shadow_pool.slot_cone_matrices[slot as usize]
                        {
                            let cone_planes =
                                crate::lighting::cone_frustum::cone_frustum_planes(&cone_matrix);
                            self.spot_entity_occluders_submitted +=
                                self.mesh_pass.record_skinned_depth(
                                    &mut pass,
                                    plan,
                                    &self.shadow_vs_bind_group,
                                    slot * stride,
                                    &cone_planes,
                                );
                        }
                    }
                }
            }
        }

        // --- Cube point-light shadow depth loop (entity-only) ----------------
        // For each occupied cube slot whose light is `entity_occluder_eligible`,
        // CLEAR all 6 faces to the far plane (1.0) and render entity occluders
        // into them. Cube faces carry NO world geometry in v1, so this loop is
        // independent of `has_geometry`; an ineligible point light (which has no
        // per-face matrices) is skipped entirely. Per face: a depth render pass
        // into the `slot*6 + face` D2Array view, projecting by that face's
        // light-space matrix (group 0, dynamic offset into the cube VS uniform
        // buffer), with the per-instance cone cull inside `record_skinned_depth`
        // testing each bound against the face's 90° frustum planes. Reuses the
        // SAME cube-ready depth pipeline as the spot path.
        //
        // CRITICAL: the per-face Clear(1.0) baseline must run for EVERY occupied
        // eligible face regardless of whether any skinned-mesh occluders exist
        // this frame. Gating the whole loop on `mesh_frame_plan` being `Some`
        // (the prior bug) meant that when no mesh entity was in the PVS — e.g. a
        // combat arena whose meshes are all off-screen — the occupied faces were
        // NEVER cleared and held stale/uninitialized depth (~0.0). An on-screen
        // eligible point light then sampled that garbage and read fully shadowed
        // (CompareFunction::Less: reference >= 0 is never < 0), zeroing its world
        // illumination. Off-screen lights own no slot (sentinel), so they stayed
        // lit — the view-dependent symptom. The clear is now unconditional and
        // the occluder draw is the only mesh-plan-gated step, mirroring the spot
        // path's "every occupied slot gets a Clear(1.0) baseline" invariant.
        self.cube_entity_occluders_submitted = 0;
        if render_world {
            if let Some(pool) = &self.cube_shadow_pool {
                let stride = self.shadow_vs_stride;
                for layer in 0..pool.face_matrices.len() {
                    let face_matrix_opt = pool.face_matrices[layer];
                    // Only occupied faces are touched; an occupied face ALWAYS gets
                    // its Clear(1.0) far-plane baseline this frame, mesh plan or not
                    // (the occluder draw below is the only mesh-plan-gated step). See
                    // `cube_shadow::cube_face_needs_clear` for why the clear must not
                    // be gated on the plan.
                    if !crate::lighting::cube_shadow::cube_face_needs_clear(
                        face_matrix_opt.is_some(),
                    ) {
                        continue;
                    }
                    let face_matrix = face_matrix_opt.expect("face_needs_clear implies occupied");
                    let view = &pool.face_views[layer];
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Cube Shadow Depth Pass"),
                        color_attachments: &[],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(1.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        ..Default::default()
                    });
                    // Occluders are entity-only: submit skinned meshes ONLY when a
                    // mesh frame plan exists. With no plan the face still receives its
                    // Clear(1.0) far-plane baseline above, so an occluder-free eligible
                    // cube reads as fully lit (shadow factor 1.0) — matching the spot
                    // path and the off-camera (no-slot) path.
                    if let Some(plan) = &mesh_frame_plan {
                        // Face frustum planes from the same matrix uploaded to the cube
                        // VS uniform buffer — one source of truth for cull + projection.
                        let face_planes =
                            crate::lighting::cone_frustum::cone_frustum_planes(&face_matrix);
                        self.cube_entity_occluders_submitted +=
                            self.mesh_pass.record_skinned_depth(
                                &mut pass,
                                plan,
                                &self.cube_shadow_vs_bind_group,
                                layer as u32 * stride,
                                &face_planes,
                            );
                    }
                }
            }
        }

        if render_world {
            let depth_ts = self
                .frame_timing
                .as_ref()
                .map(|t| t.render_pass_writes(TIMING_PAIR_DEPTH_PREPASS));
            let mut depth_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Depth Pre-Pass"),
                // Vertex-only: depth attachment only. The lightmap-UV gbuffer
                // MRT was removed with the animated dominant-direction trace.
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: depth_ts,
                ..Default::default()
            });

            if self.has_geometry && self.index_count > 0 {
                depth_pass.set_pipeline(&self.depth_prepass_pipeline);
                depth_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                depth_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                depth_pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

                if let Some(cull) = &self.compute_cull {
                    cull.draw_indirect(&mut depth_pass, None); // None = no texture bind (group 0 only)
                }
            }
        }

        // SDF half-res shadow pass — Task 4. Runs after the depth pre-pass
        // (consumes its texture) and before the forward pass (which will
        // bilateral-upsample the factor in Task 5). Skipped when no SDF
        // atlas is loaded; Task 6 will also gate on the mode selector. When
        // skipped, the half-res target retains its prior contents — Task 5's
        // forward multiply is responsible for gating on the same mode.
        if render_world && self.sdf_atlas_resources.present {
            let sdf_ts = self
                .frame_timing
                .as_ref()
                .map(|t| t.compute_pass_writes(TIMING_PAIR_SDF_SHADOW));
            let inv_view_proj = view_proj.inverse();
            // TEMP DEBUG: SDF shadow path visualization. When a debug-viz mode is
            // selected, the pass writes a debug RGB code into slot 0 instead of
            // per-light visibility floats. The mode value (3 = debug paths,
            // 4 = normals) is threaded so the shader picks the right encoding;
            // 0 means "not a debug mode" (production path).
            let sdf_debug_mode = match self.sdf_shadow_mode {
                SdfShadowMode::VisualizeDebugPaths => SdfShadowMode::VisualizeDebugPaths as u32,
                SdfShadowMode::VisualizeNormals => SdfShadowMode::VisualizeNormals as u32,
                _ => 0,
            };
            self.sdf_shadow_pass.dispatch(
                &self.queue,
                &mut encoder,
                &self.sdf_atlas_resources,
                SdfShadowFrameInputs {
                    inv_view_proj,
                    camera_position: self.last_camera_position.into(),
                },
                sdf_ts,
                sdf_debug_mode,
            );
        }

        // Post-scene compositor seam: every gameplay scene + UI pass renders into
        // `scene_color` (the offscreen target) instead of the swapchain `view`.
        // The resolve pass below is the sole swapchain writer for the gameplay
        // path. Borrowing the field-method here keeps `scene_color` a disjoint
        // borrow from the `&mut self.ui` / `&mut self.debug_lines` passes that
        // also run in this region. The splash path is unaffected — it writes the
        // swapchain directly and never touches this target.
        let scene_color = self.screen_effects.scene_color_view();

        {
            let forward_ts = self
                .frame_timing
                .as_ref()
                .map(|t| t.render_pass_writes(TIMING_PAIR_FORWARD));
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Textured Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: scene_color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color.into()),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    // Forward pass uses `depth_compare: Equal` with depth
                    // writes disabled — the depth buffer is read-only here.
                    // Task 5 of sdf-static-occluder-shadows samples this
                    // same depth texture via group 5 binding 4 (the
                    // bilateral upsample's depth-aware weights); wgpu
                    // requires `depth_ops: None` so the attachment doesn't
                    // alias a writable resource with a sampled-texture
                    // binding. The depth contents the pre-pass wrote
                    // persist for the wireframe pass that follows.
                    depth_ops: None,
                    stencil_ops: None,
                }),
                timestamp_writes: forward_ts,
                ..Default::default()
            });

            if render_world && self.has_geometry && self.index_count > 0 {
                render_pass.set_pipeline(&self.pipeline);
                render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                render_pass.set_bind_group(2, &self.lighting_bind_group, &[]);
                render_pass.set_bind_group(3, &self.sh_volume_resources.bind_group, &[]);
                render_pass.set_bind_group(4, &self.lightmap_resources.bind_group, &[]);
                render_pass.set_bind_group(5, &self.spot_shadow_pool.bind_group, &[]);
                render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

                if let Some(cull) = &self.compute_cull {
                    let gpu_textures = &self.gpu_textures;
                    cull.draw_indirect(
                        &mut render_pass,
                        Some(&|pass, bucket| {
                            let bind_group = if (bucket as usize) < gpu_textures.len() {
                                &gpu_textures[bucket as usize].bind_group
                            } else {
                                &gpu_textures[0].bind_group
                            };
                            pass.set_bind_group(1, bind_group, &[]);
                        }),
                    );
                }
            }
        }

        // Skinned-mesh forward pass — after the opaque world forward, before
        // billboards. Its own render pass so it can WRITE depth (the forward pass
        // holds the depth attachment read-only). Loads the existing color + depth
        // so the mesh composites over the world and depth-tests (`Less`).
        //
        // Reads the `mesh_frame_plan` PLANNED + UPLOADED earlier in this frame
        // (the pose/upload hoist, before the shadow loop). NO re-plan, NO
        // re-upload here — `record_draws` only records draws against the buffers
        // the hoist populated, the SAME buffers the skinned-depth shadow pass
        // read, so an entity and its shadow share one pose (no one-frame lag).
        if render_world {
            if let Some(plan) = &mesh_frame_plan {
                // Mesh group-2 params uniform (binding 4): the dynamic-light count, the
                // frame's render-clock time (the SAME value written to forward
                // `Uniforms.time` this frame — cached in `update_per_frame_uniforms` —
                // so the scripted-light curves the mesh loop evaluates stay
                // phase-coherent), and the SAME `lighting_isolation` value written to
                // forward `Uniforms.lighting_isolation` this frame, so the mesh
                // dynamic-direct term participates in the lighting-isolation debug
                // modes exactly as the world dynamic term does (the shader derives
                // `use_dynamic` from it, mirroring forward.wgsl).
                self.mesh_pass.write_light_params(
                    &self.queue,
                    self.light_count,
                    self.mesh_dynamic_time,
                    self.lighting_isolation as u32,
                    self.ambient_floor,
                );
                let mut mesh_enc = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Skinned Mesh Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: scene_color,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    ..Default::default()
                });
                mesh_enc.set_bind_group(0, &self.uniform_bind_group, &[]);
                // Group 4 = SH irradiance volume (baked indirect) + the mesh-only
                // dynamic-direct params uniform (binding 16). The mesh SUPERSET bind
                // group: shared SH entries the forward/billboard/fog passes hold PLUS
                // the dynamic-direct knobs (group 3 = instance data; group 2
                // unallocated).
                mesh_enc.set_bind_group(4, &self.sh_volume_resources.mesh_bind_group, &[]);
                self.mesh_pass.record_draws(&mut mesh_enc, plan);
            }
        }

        // After opaque forward, before wireframe. Alpha additive; depth test on, write off.
        if render_world && self.smoke_pass.has_any_sheet() && !particle_collections.is_empty() {
            let smoke_ts = self
                .frame_timing
                .as_ref()
                .map(|t| t.render_pass_writes(TIMING_PAIR_SMOKE));
            let mut smoke_pass_enc = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Billboard Sprite Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: scene_color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: smoke_ts,
                ..Default::default()
            });
            smoke_pass_enc.set_bind_group(0, &self.uniform_bind_group, &[]);
            smoke_pass_enc.set_bind_group(2, &self.lighting_bind_group, &[]);
            smoke_pass_enc.set_bind_group(3, &self.sh_volume_resources.bind_group, &[]);
            // One shared instance buffer, drawn per collection from its own
            // 256-byte-aligned dynamic offset.
            self.smoke_pass.record_draws(
                &self.device,
                &self.queue,
                &mut smoke_pass_enc,
                particle_collections,
            );
        }

        // Volumetric fog: low-res compute raymarch + additive composite.
        // Skipped when no active volumes — scatter target need not be cleared.
        // See: context/lib/rendering_pipeline.md §7.5
        if render_world {
            let cell_mask = compute_fog_cell_mask(
                fog_reachable,
                self.fog_cell_masks.as_deref(),
                self.fog.canonical_volume_count(),
                camera_leaf,
            );
            self.fog.repack_active(&self.queue, cell_mask, now_seconds);
        }
        if render_world && self.fog.active() {
            // Spots before params so FogParams.spot_count reflects this frame's count.
            let fog_spots = self.collect_fog_spot_lights();
            self.fog.upload_spots(&self.queue, &fog_spots);

            let inv_view_proj = view_proj.inverse();
            self.fog.upload_params(
                &self.queue,
                inv_view_proj,
                self.last_camera_position,
                crate::camera::NEAR,
                crate::camera::FAR,
            );

            let (scatter_w, scatter_h) = self.fog.scatter_dims();
            // 8×8 matches @workgroup_size(8,8); div_ceil covers edge pixels.
            let groups_x = scatter_w.div_ceil(8);
            let groups_y = scatter_h.div_ceil(8);
            {
                let mut raymarch = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Fog Raymarch Pass"),
                    timestamp_writes: None,
                });
                raymarch.set_pipeline(&self.fog.raymarch_pipeline);
                raymarch.set_bind_group(0, &self.uniform_bind_group, &[]);
                raymarch.set_bind_group(3, &self.sh_volume_resources.bind_group, &[]);
                raymarch.set_bind_group(5, &self.spot_shadow_pool.bind_group, &[]);
                raymarch.set_bind_group(6, &self.fog.bind_group, &[]);
                raymarch.dispatch_workgroups(groups_x, groups_y, 1);
            }

            let mut composite = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Fog Composite Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: scene_color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            composite.set_pipeline(&self.fog.composite_pipeline);
            composite.set_bind_group(0, &self.fog.composite_bind_group, &[]);
            composite.draw(0..3, 0..1); // fullscreen triangle from vertex_index — no vertex buffer
        }

        if render_world
            && self.wireframe_enabled
            && self.has_geometry
            && self.wireframe_index_count > 0
            && !self.bvh_leaves.is_empty()
        {
            if let Some(cull) = &self.compute_cull {
                let cull_status_bind_group =
                    self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("Wireframe Cull Status BG"),
                        layout: &self.wireframe_cull_status_bgl,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: cull.cull_status_buffer().as_entire_binding(),
                        }],
                    });

                let mut overlay_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Wireframe Overlay Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: scene_color,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });

                overlay_pass.set_pipeline(&self.wireframe_pipeline);
                overlay_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                overlay_pass.set_bind_group(1, &cull_status_bind_group, &[]);
                overlay_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                overlay_pass.set_index_buffer(
                    self.wireframe_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );

                // instance_index = leaf index so shader looks up per-leaf cull status.
                for (leaf_idx, leaf) in self.bvh_leaves.iter().enumerate() {
                    let wire_offset = leaf.index_offset * 2;
                    let wire_count = leaf.index_count * 2;
                    let li = leaf_idx as u32;
                    overlay_pass.draw_indexed(wire_offset..wire_offset + wire_count, 0, li..li + 1);
                }
            }
        }

        #[cfg(feature = "dev-tools")]
        if render_world {
            self.debug_lines.render(
                &self.queue,
                &mut encoder,
                scene_color,
                &self.depth_view,
                &self.uniform_bind_group,
            );
            // Buffer is cleared by the frame loop (via `clear_debug_lines`)
            // before the next frame's emit call — that single owner handles
            // surface Timeout/Occluded/Outdated early-returns above without
            // leaking segments across frames.
        }

        // UI pass: records into `scene_color` (offscreen) with `LoadOp::Load`
        // after the world/fog/wireframe/debug-line passes, before the timing
        // resolve and submit — beneath the egui overlay (which draws in the
        // caller's separate submission).
        //
        // The gameplay path lays out the snapshot's descriptor tree (renderer
        // owns layout) and records its draw data. EMPTY-TREE EARLY-OUT: when the
        // snapshot carries no tree, or the tree lays out empty, the pass is
        // skipped entirely — no `begin_render_pass`. This is the gameplay-path-
        // only early-out (A follow-up #3); the splash path opens the pass
        // unconditionally for its frame-0 black clear (see `record_splash_ui`).
        let ui_viewport = [self.surface_config.width, self.surface_config.height];
        // Modal stack: lay out and record each layer bottom→top (`trees[0]` is the
        // bottom HUD, the last entry the top/active modal). Each layer keeps its
        // own retained tree + dirty gate, so a frozen lower layer recomputes
        // nothing while the top animates. Painter's order is the stack order: a
        // later layer's quads composite over the earlier ones into the same view
        // (LoadOp::Load). Empty/empty-laying-out layers early-out individually.
        let stack: Vec<ui::descriptor::AnchoredTree> = self
            .ui_snapshot
            .trees
            .iter()
            .map(|entry| entry.descriptor.clone())
            .collect();

        // Lay out EVERY layer first into owned draw data, THEN compose all layers
        // into a SINGLE `encode` call. The glyphon text half (`UiTextRenderer`) is
        // shared across layers and holds ONE vertex buffer it overwrites at offset
        // 0 on each `prepare`; `queue.write_buffer` resolves on the queue timeline
        // (last write wins) regardless of recording order, so issuing a separate
        // `encode` per layer makes EVERY layer's text draw read the LAST layer's
        // shaped glyphs — the readout-aliasing bug (a lower layer's text rendered
        // the top layer's glyphs). This mirrors the multi-batch quad-buffer clobber
        // already documented in `UiPass::encode`: one `prepare`/`render` per frame,
        // with all layers' glyphs concatenated in painter order, sidesteps it.
        let mut layer_draws: Vec<ui::tree::UiDrawData> = Vec::with_capacity(stack.len());
        for (layer, tree) in stack.iter().enumerate() {
            // Image sizes are optional for gameplay layers — an `image` node with
            // no size entry measures to zero. The splash supplies its logo size
            // separately in `record_splash_ui`.
            // Bound text/panel nodes resolve against the snapshot's slot values
            // (disjoint field borrow from `&mut self.ui`). The cloned `stack`
            // above already released the snapshot, so this borrow is clean.
            let mut draw = self.ui.layout_gameplay_tree(
                layer,
                tree,
                ui_viewport,
                &ui::tree::ImageSizes::new(),
                &self.ui_snapshot.slot_values,
                &self.ui_snapshot.cell_values,
                &self.ui_theme,
                self.ui_theme_generation,
                self.ui_snapshot.time_seconds,
            );
            // Focus ring (M13 Goal F, Task 3): only the TOP layer takes focus, so
            // draw the engine ring around the focused node's rect on it. The
            // focused id rode in on the snapshot (resolved app-side last frame, so
            // it may trail a focus change by one frame). The ring is a `focus.ring`
            // bordered frame inset by the `xs` spacing token; appended to this
            // layer's quad list so it composites over the layer's own quads.
            let is_top = layer + 1 == stack.len();
            if is_top {
                if let Some(focused) = self.ui_snapshot.focused_id.as_deref() {
                    let focus_rects = self.ui.export_top_focus_rects(
                        ui_viewport,
                        &self.ui_snapshot.slot_values,
                        &self.ui_snapshot.cell_values,
                    );
                    if let Some(fr) = focus_rects.rects.iter().find(|r| r.id == focused) {
                        let inset = self.ui_theme.spacing("xs").unwrap_or(0.0)
                            * ui::layout::device_scale(ui_viewport);
                        let ring_color = self
                            .ui_theme
                            .color("focus.ring")
                            .unwrap_or([1.0, 0.0, 1.0, 1.0]);
                        ui::push_focus_ring(&mut draw.quads, fr.rect, inset, ring_color);
                    }
                }
            }
            layer_draws.push(draw);
        }

        // Fold every laid-out layer into ONE whole-frame composition (bottom→top
        // painter order) and record a SINGLE UI pass. The composition is the unit
        // of encoding — `encode` takes the whole composition, never one layer — so
        // the cross-layer glyphon clobber (every layer's text reading the last
        // layer's shaped glyphs) is unrepresentable. The white bind group is cloned
        // out first so the `&self.ui_images` borrow the fold takes can coexist with
        // the `&mut self.ui` encode call below.
        let white_bg = self.ui.white_bind_group().clone();
        let composition =
            ui::UiComposition::from_layer_draws(&layer_draws, &white_bg, &self.ui_images);
        if !composition.is_empty() {
            self.ui.encode(
                &self.device,
                &self.queue,
                &mut encoder,
                scene_color,
                ui_viewport,
                wgpu::LoadOp::Load,
                &composition,
            );
        }
        // Drop retained state for any layers popped since last frame (stack
        // shrank), so freed modal trees release their layout cache.
        self.ui.truncate_gameplay_stack(stack.len());

        // Post-scene compositor resolve: blit `scene_color` into the swapchain
        // `view`, composing flash/vignette/shake from the frame's UI slot
        // snapshot on top. Encoded AFTER the UI pass and BEFORE the timing
        // resolve — the sole swapchain writer for the gameplay path, run every
        // frame (never skipped at rest). At-rest slot values pack to the identity
        // uniform, so the output stays byte-identical to the pre-SE blit.
        self.screen_effects.encode_resolve(
            &self.queue,
            &mut encoder,
            &view,
            &self.ui_snapshot.slot_values,
        );

        if let Some(timing) = &self.frame_timing {
            timing.encode_resolve(&mut encoder);
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        // Capture the just-composed SH atlas for the live irradiance overlay.
        // Separate submission so the boundary orders this copy after the compose
        // storage writes (see the note at the compose dispatch above). Skipped
        // unless the overlay is active.
        #[cfg(feature = "dev-tools")]
        if self.sh_probe_readback.wants_copy() {
            // Block until the compose submit above has fully retired before the
            // copy reads `total`. A submission boundary alone does not hard-sync
            // the compute storage writes against the copy on the Metal backend:
            // when the in-room compose runs longer (active delta lights), the
            // copy catches the last-written (high-z) texels mid-flight and reads
            // foreign/zero garbage. Only reached while the overlay is active, so
            // the per-readback stall is confined to debug sessions.
            let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
            let mut readback_encoder =
                self.device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("SH Readback Encoder"),
                    });
            self.sh_probe_readback.encode_copy(
                &mut readback_encoder,
                &self.sh_volume_resources.total_atlas_texture,
            );
            self.queue
                .submit(std::iter::once(readback_encoder.finish()));
        }

        if let Some(timing) = self.frame_timing.as_mut() {
            timing.post_submit(&self.device);
        }

        // Drive the SH readback map and, when a frame's data has landed, swap it
        // into the probe-marker source so the next overlay frame shows live
        // (base + animated-delta) irradiance instead of the static bake.
        #[cfg(feature = "dev-tools")]
        if let Some(live_irradiance) = self.sh_probe_readback.post_submit(&self.device) {
            self.sh_volume_resources.probe_irradiance = live_irradiance;
        }

        // Caller (`App`) presents after optionally appending the egui overlay
        // pass via `render_debug_ui`.
        Ok(Some(output))
    }

    #[cfg(feature = "dev-tools")]
    pub fn clear_debug_lines(&mut self) {
        self.debug_lines.clear();
    }
}

fn build_default_view_projection(aspect: f32) -> Mat4 {
    let eye = glam::Vec3::new(0.0, 200.0, 500.0);
    let center = glam::Vec3::ZERO;
    let up = glam::Vec3::Y;

    let view = Mat4::look_at_rh(eye, center, up);
    let projection = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, aspect, 0.1, 4096.0);

    projection * view
}

fn cast_world_vertices_to_bytes(data: &[crate::geometry::WorldVertex]) -> Vec<u8> {
    let byte_len = data.len() * crate::geometry::WorldVertex::STRIDE;
    let mut bytes = Vec::with_capacity(byte_len);
    for vertex in data {
        for &c in &vertex.position {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.base_uv {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.normal_oct {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.tangent_packed {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.lightmap_uv {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
    }
    bytes
}

// Each triangle [a, b, c] → three line pairs [a,b, b,c, c,a].
// Shared edges are emitted multiple times; fine for a debug overlay.
fn build_line_indices_from_triangles(tri_indices: &[u32]) -> Vec<u32> {
    let tri_count = tri_indices.len() / 3;
    let mut lines = Vec::with_capacity(tri_count * 6);
    for tri in tri_indices.chunks_exact(3) {
        let (a, b, c) = (tri[0], tri[1], tri[2]);
        lines.push(a);
        lines.push(b);
        lines.push(b);
        lines.push(c);
        lines.push(c);
        lines.push(a);
    }
    lines
}

fn bytemuck_cast_slice_u32(data: &[u32]) -> Vec<u8> {
    let byte_len = std::mem::size_of_val(data);
    let mut bytes = Vec::with_capacity(byte_len);
    for &val in data {
        bytes.extend_from_slice(&val.to_ne_bytes());
    }
    bytes
}

/// Pack the SH grid metadata the SDF shadow pass needs for its open-space
/// skip uniform. Mirrors what the forward pass reads from `ShGridInfo` (group
/// 3) — replicating it here lets the shadow pass keep group 3 off its
/// pipeline layout. Returns the "empty SH" defaults when the section is
/// absent or marked not-present, matching the dummy 1×1×1 path in
/// `ShVolumeResources`.
fn build_sdf_shadow_sh_grid(
    sh_volume: Option<&postretro_level_format::sh_volume::OctahedralShVolumeSection>,
    present: bool,
) -> SdfShadowShGrid {
    if !present {
        return SdfShadowShGrid::default();
    }
    let Some(sec) = sh_volume else {
        return SdfShadowShGrid::default();
    };
    SdfShadowShGrid {
        origin: sec.grid_origin,
        cell_size: sec.cell_size,
        dimensions: sec.grid_dimensions,
        has_volume: true,
    }
}

/// Per-light delta AABB overlays no longer have a source: the sparse CSR delta
/// format (v2) is keyed by affinity cell, not per-light AABB grids, so there are
/// no per-light origin/dims to draw. Returns empty; the diagnostics consumer
/// skips the delta-AABB loop. A future affinity-cell overlay could repopulate
/// this from `affinity_dims` + the base grid origin/cell-size.
#[cfg(feature = "dev-tools")]
fn collect_delta_volume_meta(
    _section: Option<&postretro_level_format::delta_sh_volumes::DeltaShVolumesSection>,
) -> Vec<sh_volume::DeltaVolumeMeta> {
    Vec::new()
}

// Static lights are baked — including them would double-apply their contribution.
// Short influence list → zero-radius placeholder.
fn filter_dynamic_lights(
    lights: &[MapLight],
    influences: &[LightInfluence],
) -> (Vec<MapLight>, Vec<LightInfluence>) {
    lights
        .iter()
        // enumerate before filter so i preserves the original index into influences
        .enumerate()
        .filter(|(_, l)| l.is_dynamic)
        .map(|(i, l)| {
            let inf = influences.get(i).cloned().unwrap_or(LightInfluence {
                center: Vec3::ZERO,
                radius: 0.0,
            });
            (l.clone(), inf)
        })
        .unzip()
}

/// Pull the spot-shadow pool's candidate set from the **full** level light
/// list: every dynamic-tier light (`is_dynamic`). A baked light's world shadow
/// is frozen in the lightmap, so it never needs a pool slot; only dynamic-tier
/// lights qualify.
///
/// Dynamic-tier spotlights cast world shadows through the shadow depth pass
/// (which renders static world geometry), so a pooled dynamic spot shadows
/// pillars and other occluders. The per-light `casts_entity_shadows` toggle
/// (FGD `_cast_entity_shadows`) is orthogonal to slot allocation — it gates
/// whether moving-ENTITY occluders are drawn into the already-allocated slot
/// (`entity_occluder_eligible`), not whether the slot exists.
///
/// Ranking is layered on top of the existing `eligible_lights`
/// visibility/brightness slice in `rank_lights`.
fn filter_entity_shadow_candidates(
    lights: &[MapLight],
    influences: &[LightInfluence],
) -> (Vec<MapLight>, Vec<LightInfluence>) {
    lights
        .iter()
        .enumerate()
        .filter(|(_, l)| l.is_dynamic)
        .map(|(i, l)| {
            let inf = influences.get(i).cloned().unwrap_or(LightInfluence {
                center: Vec3::ZERO,
                radius: 0.0,
            });
            (l.clone(), inf)
        })
        .unzip()
}

/// Identity-match a shadow candidate against the `level_lights` slice
/// (origin + light_type) and return that level-light's per-frame
/// effective brightness. Returns `None` when the candidate isn't in
/// `level_lights`. Both sets are `is_dynamic`-filtered snapshots of the same
/// `world.lights` source, so today every candidate is present and this returns
/// `Some`; the `None` arm is the defensive path for once light-movement
/// re-keying lands.
fn level_brightness_for_candidate(
    level_lights: &[MapLight],
    candidate: &MapLight,
    effective_brightness: &[f32],
) -> Option<f32> {
    // Re-keys by float-exact `origin` equality. Both `level_lights` and
    // `shadow_candidate_lights` are immutable load-time snapshots filtered from
    // the same `world.lights` source, so origins match exactly today. The match
    // breaks only once runtime light-movement lands and mutates one side's
    // origins live (the candidate snapshot would keep a stale origin and
    // silently lose the forward shadow slot). That feature doesn't exist —
    // `is_dynamic` is a dormant seam with no authoring surface and
    // `self.level_lights` is never mutated post-load — so keying on a stable id
    // now would be scaffolding for an unlanded feature. When movement lands, key
    // both sites on the `world.lights` source index (the natural shared id;
    // currently discarded by `filter_dynamic_lights` /
    // `filter_entity_shadow_candidates`) instead of origin equality.
    level_lights
        .iter()
        .enumerate()
        .find(|(_, l)| l.origin == candidate.origin && l.light_type == candidate.light_type)
        .and_then(|(i, _)| effective_brightness.get(i).copied())
}

/// Translate a slot assignment from candidate-index space into
/// `level_lights`-index space. Returns a Vec the size of `level_lights`,
/// each entry either a slot or `NO_SHADOW_SLOT`. Used to pack the GPU
/// lights buffer (`pack_lights_with_slots_into`), which is keyed on
/// `level_lights`. Candidates not in `level_lights` have no forward-side
/// slot today — that bridge is post-1b work.
fn slot_assignment_for_level_lights(
    level_lights: &[MapLight],
    candidates: &[MapLight],
    candidate_slot_assignment: &[u32],
) -> Vec<u32> {
    use crate::lighting::spot_shadow::NO_SHADOW_SLOT;
    let mut out = vec![NO_SHADOW_SLOT; level_lights.len()];
    for (cand_idx, &slot) in candidate_slot_assignment.iter().enumerate() {
        if slot == NO_SHADOW_SLOT {
            continue;
        }
        let cand = &candidates[cand_idx];
        // Re-keys by float-exact `origin` equality — same constraint as
        // `level_brightness_for_candidate`: exact today because both collections
        // are immutable load-time snapshots of the same `world.lights` source.
        // A moving spot (unlanded; see that fn) would carry a stale candidate
        // origin and silently drop its slot. Key both sites on the
        // `world.lights` source index when light-movement lands.
        if let Some((level_idx, _)) = level_lights
            .iter()
            .enumerate()
            .find(|(_, l)| l.origin == cand.origin && l.light_type == cand.light_type)
        {
            out[level_idx] = slot;
        }
    }
    out
}

/// See: context/lib/boot_sequence.md §3 (Level Install Order)
pub fn level_world_to_geometry<'a>(
    world: &'a crate::prl::LevelWorld,
    texture_materials: &'a [Material],
) -> LevelGeometry<'a> {
    LevelGeometry {
        vertices: &world.vertices,
        indices: &world.indices,
        bvh: &world.bvh,
        lights: &world.lights,
        light_influences: &world.light_influences,
        sh_volume: world.sh_volume.as_ref(),
        lightmap: world.lightmap.as_ref(),
        chunk_light_list: world.chunk_light_list.as_ref(),
        animated_light_chunks: world.animated_light_chunks.as_ref(),
        animated_light_weight_maps: world.animated_light_weight_maps.as_ref(),
        delta_sh_volumes: world.delta_sh_volumes.as_ref(),
        direct_sh_volume: world.direct_sh_volume.as_ref(),
        sdf_atlas: world.sdf_atlas.as_ref(),
        lightmap_mode: world.lightmap_mode,
        texture_materials,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression guard for the exact bug this fix closes: the renderer must thread
    // `self.ambient_floor` into the mesh `write_light_params` call so the
    // diagnostics ambient-floor slider reaches skinned meshes (it was silently
    // dropped, leaving shadowed mesh faces black). A behavioral assertion needs a
    // GPU, so this pins the call-site source: if the `self.ambient_floor` argument
    // is removed or renamed, the contract fails here before it reaches a frame.
    #[test]
    fn renderer_threads_ambient_floor_into_mesh_write_light_params() {
        let src = include_str!("mod.rs");
        let call = src
            .split("self.mesh_pass.write_light_params(")
            .nth(1)
            .expect("mesh_pass.write_light_params call site must exist");
        let args = call
            .split_once(");")
            .expect("call must terminate with );")
            .0;
        assert!(
            args.contains("self.ambient_floor"),
            "mesh write_light_params call must pass self.ambient_floor (the \
             ambient-floor slider must reach skinned meshes)",
        );
    }

    // Regression: the forward "Textured Pipeline Layout" grew a fragment-stage
    // sampled-texture binding (the SH direct atlas, group-3 binding 15) but the
    // hand-maintained device-limit constant was not bumped, so
    // create_pipeline_layout panicked at launch — uncatchable in CI, which has no
    // GPU. This re-derives the requested limit from the same GPU-free BGL builders
    // the pipeline layout is composed from, asserting the actual binding count
    // stays within the 16-texture design budget (the Metal/WebGPU spec floor).
    // Mirrors `sh_volume::group3_shader_bindings_are_represented_by_rust_layout`.
    #[cfg(debug_assertions)]
    #[test]
    fn forward_pipeline_sampled_texture_request_matches_bgl_definitions() {
        // The forward pipeline layout (see `create_pipeline_layout`) composes
        // exactly these six BGLs in this group order. Counting fragment-visible
        // texture entries across them is how wgpu charges
        // `max_sampled_textures_per_shader_stage`. Group 5's count is
        // feature-conditional, so check both variants from the same builders.
        //
        // `cube_array_supported = true`: Group 5 carries 4 sampled textures — spot
        // depth array (b0), SDF shadow factor (b3), SDF scene depth (b4), and the
        // dynamic point-light cube depth (b5). Total forward sampled textures: 14.
        //
        // `cube_array_supported = false`: binding 5 is omitted, so Group 5 carries
        // 3 and the total is 13. The forward + fog pipelines then build from a
        // group-5 BGL WITHOUT the cube entry (the no-cube shader variants drop the
        // matching declaration), so point shadows disable cleanly with no panic.
        let per_group = |cube_array_supported: bool| {
            [
                fragment_sampled_textures(&uniform_bind_group_layout_entries()), // group 0
                fragment_sampled_textures(&material_bind_group_layout_entries()), // group 1
                fragment_sampled_textures(&lighting_bind_group_layout_entries()), // group 2
                fragment_sampled_textures(&sh_volume::sh_bind_group_layout_entries()), // group 3
                fragment_sampled_textures(&crate::lighting::lightmap::bind_group_layout_entries()), // group 4
                fragment_sampled_textures(&SpotShadowPool::bind_group_layout_entries(
                    cube_array_supported,
                )), // group 5
            ]
        };

        // Supported: Group 5 = 4, total = 14.
        let supported = per_group(true);
        assert_eq!(
            supported,
            [0, 3, 0, 3, 4, 4],
            "forward BGL texture inventory changed (CUBE_ARRAY supported)"
        );
        let derived_supported: u32 = supported.iter().sum();
        assert_eq!(derived_supported, 14);
        assert_eq!(
            forward_pipeline_sampled_texture_count(true),
            derived_supported
        );

        // Unsupported: Group 5 = 3 (no cube entry), total = 13. The group-5 BGL
        // builder must omit binding 5 — pin both the count and the absence.
        let unsupported = per_group(false);
        assert_eq!(
            unsupported,
            [0, 3, 0, 3, 4, 3],
            "forward BGL texture inventory changed (CUBE_ARRAY absent)"
        );
        let derived_unsupported: u32 = unsupported.iter().sum();
        assert_eq!(derived_unsupported, 13);
        assert_eq!(
            forward_pipeline_sampled_texture_count(false),
            derived_unsupported
        );
        let no_cube_entries = SpotShadowPool::bind_group_layout_entries(false);
        assert!(
            no_cube_entries.iter().all(|e| e.binding != 5),
            "no-CUBE_ARRAY group-5 BGL must omit binding 5 (the CubeArray cube depth)"
        );
        assert_eq!(
            no_cube_entries.len(),
            5,
            "no-CUBE_ARRAY group-5 BGL must carry exactly 5 entries (bindings 0..=4)"
        );
        // And the supported variant DOES carry binding 5.
        assert!(
            SpotShadowPool::bind_group_layout_entries(true)
                .iter()
                .any(|e| e.binding == 5),
            "CUBE_ARRAY group-5 BGL must include binding 5 (the CubeArray cube depth)"
        );

        // 16 is the design budget: the WebGPU spec floor and Metal's hard ceiling.
        // If the derived count exceeds 16, switch to bindless (TEXTURE_BINDING_ARRAY)
        // rather than raising REQUIRED_SAMPLED_TEXTURES in the device limit request.
        assert!(
            derived_supported <= 16,
            "forward pipeline sampled-texture count ({derived_supported}) exceeds the Metal/WebGPU spec floor of 16; \
             use bindless (TEXTURE_BINDING_ARRAY) rather than raising this limit"
        );
    }

    // Regression: billboard lighting runs in `vs_main` (per-vertex SH indirect+direct,
    // static-specular, dynamic-diffuse) and the group-6 instance storage buffer is
    // VERTEX-read. wgpu charges `max_storage_buffers_per_shader_stage`
    // against the BGL *entry* set per stage — every VERTEX-visible storage entry in
    // the Billboard Pipeline Layout counts, whether or not vs_main reads it. The hoist
    // initially left the three group-3 anim/scripted-light storage buffers marked
    // VERTEX-visible, pushing the count to 9 > the downlevel-default 8 and crashing
    // `create_pipeline_layout` on real GPUs ("Too many bindings of type StorageBuffers
    // in Stage VERTEX") — uncatchable in CI, which has no GPU. This re-derives the
    // count from the same GPU-free BGL builders the layout is composed from and pins
    // it at <= 8. Mirrors `forward_pipeline_sampled_texture_request_matches_bgl_definitions`.
    #[cfg(debug_assertions)]
    #[test]
    fn billboard_pipeline_vertex_storage_request_matches_bgl_definitions() {
        // The Billboard Pipeline Layout (see `smoke::SmokePass::new`) composes
        // exactly these BGLs in this group order: 0 camera, 1 sheet, 2 lighting,
        // 3 SH volume, 6 instance (groups 4 and 5 are empty `None` slots). Counting
        // VERTEX-visible storage entries across them is how wgpu charges
        // `max_storage_buffers_per_shader_stage`.
        let per_group = [
            vertex_storage_buffers(&uniform_bind_group_layout_entries()), // group 0
            vertex_storage_buffers(&smoke::sprite_sheet_bind_group_layout_entries()), // group 1
            vertex_storage_buffers(&lighting_bind_group_layout_entries()), // group 2
            vertex_storage_buffers(&sh_volume::sh_bind_group_layout_entries()), // group 3
            vertex_storage_buffers(&smoke::sprite_instance_bind_group_layout_entries()), // group 6
        ];
        // Per-group expectations document the inventory; if a BGL drifts, the failing
        // index points straight at the group. Group 2 contributes its five storage
        // light/chunk buffers (lights, light_influence, spec_lights, chunk_offsets,
        // chunk_indices); group 6 contributes the one sprite instance buffer. Group 3
        // (SH volume) contributes ZERO — its three anim/scripted-light storage buffers
        // are FRAGMENT | COMPUTE only, NOT VERTEX, because vs_main never reads them.
        // If a group-3 storage entry regains VERTEX visibility this index flips to a
        // nonzero count and the budget assert below fails before a real GPU would.
        assert_eq!(
            per_group,
            [0, 0, 5, 0, 1],
            "billboard BGL vertex storage-buffer inventory changed"
        );

        let derived: u32 = per_group.iter().sum();
        // The aggregation helper must agree with the hand-summed inventory above.
        assert_eq!(billboard_pipeline_vertex_storage_buffer_count(), derived);
        // 8 is the downlevel/WebGPU-default ceiling. If the derived count exceeds 8,
        // trim VERTEX visibility from storage entries vs_main does not read, or
        // consolidate buffers — do NOT raise max_storage_buffers_per_shader_stage in
        // the device limit request (it breaks modest-spec adapters the engine targets).
        assert!(
            derived <= 8,
            "billboard pipeline VERTEX-visible storage-buffer count ({derived}) exceeds the \
             downlevel-default max_storage_buffers_per_shader_stage of 8; trim VERTEX \
             visibility or consolidate rather than raising the limit"
        );
    }

    #[test]
    fn compute_fog_cell_mask_culled_unions_visible_leaf_masks() {
        let masks = vec![0b001u32, 0b010, 0b101, 0b000]; // 4 leaves, 3 fog volumes
        let fog_reachable = [1u32, 2];
        let active = compute_fog_cell_mask(&fog_reachable, Some(&masks), 3, Some(1));
        // leaf1→0b010, leaf2→0b101 → OR 0b111; camera-leaf union (camera_leaf=1,
        // already in reachable set) is idempotent here — see
        // compute_fog_cell_mask_camera_leaf_union_is_idempotent_when_already_reachable
        assert_eq!(active, 0b111);
    }

    #[test]
    fn compute_fog_cell_mask_drawall_returns_all_canonical_slots() {
        let masks = vec![0u32; 4]; // present but ignored on DrawAll path
        // Empty fog_reachable signals DrawAll-equivalent (portal isolation N/A).
        assert_eq!(compute_fog_cell_mask(&[], Some(&masks), 3, Some(0)), 0b111);
        assert_eq!(compute_fog_cell_mask(&[], None, 3, Some(0)), 0b111);
    }

    #[test]
    fn compute_fog_cell_mask_culled_without_baked_masks_falls_back_to_all_slots() {
        let fog_reachable = [0u32, 1, 2];
        assert_eq!(
            compute_fog_cell_mask(&fog_reachable, None, 4, Some(0)),
            0b1111
        );
    }

    #[test]
    fn compute_fog_cell_mask_zero_canonical_volumes_returns_zero() {
        assert_eq!(compute_fog_cell_mask(&[], None, 0, Some(0)), 0);
        assert_eq!(
            compute_fog_cell_mask(&[0u32], Some(&[0xFFu32]), 0, Some(0)),
            0
        );
    }

    #[test]
    fn compute_fog_cell_mask_unions_camera_leaf_when_absent_from_fog_reachable() {
        // Camera in leaf 3 (not in fog_reachable). Its 0b100 bit must still appear.
        // Regression: portal traversal can transiently omit the camera leaf,
        // causing fog the camera is inside to flicker off.
        let masks = vec![0b001u32, 0b010, 0b000, 0b100];
        let fog_reachable = [0u32, 1];
        let active = compute_fog_cell_mask(&fog_reachable, Some(&masks), 3, Some(3));
        // 0b001 | 0b010 (union) | 0b100 (camera leaf) = 0b111
        assert_eq!(active, 0b111);
    }

    #[test]
    fn compute_fog_cell_mask_camera_leaf_union_is_idempotent_when_already_reachable() {
        let masks = vec![0b001u32, 0b010, 0b100];
        let fog_reachable = [0u32, 2];
        let with_cam = compute_fog_cell_mask(&fog_reachable, Some(&masks), 3, Some(2));
        let without_cam = compute_fog_cell_mask(&fog_reachable, Some(&masks), 3, None);
        assert_eq!(with_cam, without_cam);
        assert_eq!(with_cam, 0b101);
    }

    #[test]
    fn sphere_intersects_any_fog_aabb_inside_passes() {
        let aabbs = vec![(Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0))];
        assert!(sphere_intersects_any_fog_aabb(
            Vec3::new(0.0, 0.0, 0.0),
            0.1,
            &aabbs,
        ));
    }

    #[test]
    fn sphere_intersects_any_fog_aabb_outside_all_drops() {
        let aabbs = vec![
            (Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0)),
            (Vec3::new(50.0, 50.0, 50.0), Vec3::new(52.0, 52.0, 52.0)),
        ];
        assert!(!sphere_intersects_any_fog_aabb(
            Vec3::new(100.0, 100.0, 100.0),
            5.0,
            &aabbs,
        ));
    }

    #[test]
    fn sphere_intersects_any_fog_aabb_empty_list_passes_everything() {
        assert!(sphere_intersects_any_fog_aabb(
            Vec3::new(0.0, 0.0, 0.0),
            1.0,
            &[],
        ));
    }

    #[test]
    fn sphere_intersects_any_fog_aabb_grazing_edge_passes() {
        // distance == radius counts as intersecting (matches sphere_intersects_any_aabb).
        let aabbs = vec![(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0))];
        assert!(sphere_intersects_any_fog_aabb(
            Vec3::new(2.0, 0.5, 0.5),
            1.0,
            &aabbs,
        ));
    }

    #[test]
    fn default_view_projection_is_finite() {
        let vp = build_default_view_projection(16.0 / 9.0);
        let cols = vp.to_cols_array();
        for (i, val) in cols.iter().enumerate() {
            assert!(val.is_finite(), "view_proj[{i}] is not finite: {val}");
        }
    }

    #[test]
    fn mip_lod_max_clamp_derivation() {
        // The aniso sampler pool uses this clamp so no sampler reads past the uploaded mip chain.
        assert_eq!(mip_lod_max_clamp(1), 0.0);
        assert_eq!(mip_lod_max_clamp(8), 7.0);
        // mip_count 0 is degenerate; saturating_sub keeps it at the base level.
        assert_eq!(mip_lod_max_clamp(0), 0.0);
    }

    #[test]
    fn cast_world_vertices_roundtrips() {
        let input = vec![
            crate::geometry::WorldVertex {
                position: [1.0, 2.0, 3.0],
                base_uv: [0.5, 0.75],
                normal_oct: [32768, 32768],
                tangent_packed: [65535, 32768],
                lightmap_uv: [100, 200],
            },
            crate::geometry::WorldVertex {
                position: [4.0, 5.0, 6.0],
                base_uv: [0.25, 0.125],
                normal_oct: [0, 32768],
                tangent_packed: [32768, 0],
                lightmap_uv: [0, 0],
            },
        ];
        let bytes = cast_world_vertices_to_bytes(&input);
        // 2 vertices * 32 bytes = 64 bytes
        assert_eq!(bytes.len(), 64);

        let pos_x = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        let pos_y = f32::from_ne_bytes(bytes[4..8].try_into().unwrap());
        let pos_z = f32::from_ne_bytes(bytes[8..12].try_into().unwrap());
        let uv_u = f32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        let uv_v = f32::from_ne_bytes(bytes[16..20].try_into().unwrap());
        let n_u = u16::from_ne_bytes(bytes[20..22].try_into().unwrap());
        let n_v = u16::from_ne_bytes(bytes[22..24].try_into().unwrap());
        let t_u = u16::from_ne_bytes(bytes[24..26].try_into().unwrap());
        let t_v = u16::from_ne_bytes(bytes[26..28].try_into().unwrap());
        let lm_u = u16::from_ne_bytes(bytes[28..30].try_into().unwrap());
        let lm_v = u16::from_ne_bytes(bytes[30..32].try_into().unwrap());

        assert_eq!([pos_x, pos_y, pos_z], [1.0, 2.0, 3.0]);
        assert_eq!([uv_u, uv_v], [0.5, 0.75]);
        assert_eq!([n_u, n_v], [32768, 32768]);
        assert_eq!([t_u, t_v], [65535, 32768]);
        assert_eq!([lm_u, lm_v], [100, 200]);
    }

    #[test]
    fn byte_cast_u32_roundtrips() {
        let input = vec![100u32, 200, 300];
        let bytes = bytemuck_cast_slice_u32(&input);
        assert_eq!(bytes.len(), 12);

        let mut output = Vec::new();
        for chunk in bytes.chunks_exact(4) {
            output.push(u32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        assert_eq!(output, vec![100, 200, 300]);
    }

    #[test]
    fn uniform_data_has_correct_size() {
        let data = build_uniform_data(&FrameUniforms {
            view_proj: Mat4::IDENTITY,
            camera_position: Vec3::ZERO,
            ambient_floor: 0.05,
            light_count: 0,
            time: 0.0,
            lighting_isolation: LightingIsolation::Normal,
            indirect_scale: 1.0,
            sdf_shadow_flags: 0,
            sdf_shadow_mode: SdfShadowMode::On,
            sdf_force_visibility_one: false,
            dynamic_direct_scale: DEFAULT_DYNAMIC_DIRECT_SCALE,
            dynamic_direct_isolation: DynamicDirectIsolation::Combined,
            has_direct: false,
        });
        assert_eq!(data.len(), UNIFORM_SIZE);
    }

    /// `sdf_shadow_flags` packs to bytes 96..100 — confirm the bitset round-trips.
    #[test]
    fn uniform_data_encodes_sdf_shadow_flags_at_correct_offset() {
        let data = build_uniform_data(&FrameUniforms {
            view_proj: Mat4::IDENTITY,
            camera_position: Vec3::ZERO,
            ambient_floor: 0.0,
            light_count: 0,
            time: 0.0,
            lighting_isolation: LightingIsolation::Normal,
            indirect_scale: 1.0,
            sdf_shadow_flags: SDF_SHADOW_FLAG_ATLAS_PRESENT,
            sdf_shadow_mode: SdfShadowMode::On,
            sdf_force_visibility_one: false,
            dynamic_direct_scale: 0.0,
            dynamic_direct_isolation: DynamicDirectIsolation::Combined,
            has_direct: false,
        });
        let flags = u32::from_ne_bytes(data[96..100].try_into().unwrap());
        assert_eq!(flags, SDF_SHADOW_FLAG_ATLAS_PRESENT);
        // `sdf_shadow_mode` at 100..104 — `On` encodes to 0;
        // `sdf_force_visibility_one` at 104..108 (false ⇒ 0). The dynamic-direct
        // tail (108..120) is zero here (scale 0, Combined=0, has_direct=false),
        // and the trailing pad 120..128 stays zero.
        assert_eq!(
            u32::from_ne_bytes(data[100..104].try_into().unwrap()),
            SdfShadowMode::On as u32,
        );
        assert!(data[104..128].iter().all(|&b| b == 0));
    }

    /// sdf-per-light-shadows Task 3: the dev "force visibility 1.0" toggle
    /// packs as a u32 at offset 104..108 (non-zero ⇒ forced) and leaves the
    /// trailing pad 108..112 zero. Guards the CPU↔WGSL uniform layout drift
    /// for the new field.
    #[test]
    fn uniform_data_encodes_sdf_force_visibility_one_at_correct_offset() {
        for (force, expected) in [(false, 0u32), (true, 1u32)] {
            let data = build_uniform_data(&FrameUniforms {
                view_proj: Mat4::IDENTITY,
                camera_position: Vec3::ZERO,
                ambient_floor: 0.0,
                light_count: 0,
                time: 0.0,
                lighting_isolation: LightingIsolation::Normal,
                indirect_scale: 1.0,
                sdf_shadow_flags: 0,
                sdf_shadow_mode: SdfShadowMode::On,
                sdf_force_visibility_one: force,
                dynamic_direct_scale: 0.0,
                dynamic_direct_isolation: DynamicDirectIsolation::Combined,
                has_direct: false,
            });
            assert_eq!(
                u32::from_ne_bytes(data[104..108].try_into().unwrap()),
                expected,
                "sdf_force_visibility_one={force} should encode to {expected} at 104..108",
            );
            assert!(
                data[120..128].iter().all(|&b| b == 0),
                "tail pad 120..128 must stay zero for force={force}",
            );
        }
    }

    /// Task 6 of `sdf-static-occluder-shadows`: the `SdfShadowMode` selector
    /// must round-trip through the `FrameUniforms` byte packer — every
    /// variant encodes to its `u32` repr at offset 100..104 with the
    /// trailing pad bytes zeroed. Mirrors
    /// `uniform_data_encodes_sdf_shadow_flags_at_correct_offset`.
    #[test]
    fn sdf_shadow_mode_round_trips_through_uniform() {
        for mode in SdfShadowMode::ALL_VARIANTS {
            let data = build_uniform_data(&FrameUniforms {
                view_proj: Mat4::IDENTITY,
                camera_position: Vec3::ZERO,
                ambient_floor: 0.0,
                light_count: 0,
                time: 0.0,
                lighting_isolation: LightingIsolation::Normal,
                indirect_scale: 1.0,
                sdf_shadow_flags: 0,
                sdf_shadow_mode: mode,
                sdf_force_visibility_one: false,
                dynamic_direct_scale: 0.0,
                dynamic_direct_isolation: DynamicDirectIsolation::Combined,
                has_direct: false,
            });
            let decoded = u32::from_ne_bytes(data[100..104].try_into().unwrap());
            assert_eq!(
                decoded, mode as u32,
                "SdfShadowMode::{:?} should encode to {} at offset 100..104",
                mode, mode as u32,
            );
            // Trailing pad 120..128 stays zero regardless of mode.
            assert!(
                data[120..128].iter().all(|&b| b == 0),
                "trailing pad bytes 120..128 must stay zero for {:?}",
                mode,
            );
        }
    }

    /// baked-static-direct-sh Task 6: the dynamic-direct tail of the shared
    /// group-0 `Uniforms` must round-trip through the byte packer. `direct_scale`
    /// repurposes the former `_sdf_pad1` slot (108..112); isolation + has_direct
    /// land in the fresh 16-byte row (112..120), with 120..128 padding.
    #[test]
    fn uniform_data_encodes_dynamic_direct_tail_at_correct_offsets() {
        let data = build_uniform_data(&FrameUniforms {
            view_proj: Mat4::IDENTITY,
            camera_position: Vec3::ZERO,
            ambient_floor: 0.0,
            light_count: 0,
            time: 0.0,
            lighting_isolation: LightingIsolation::Normal,
            indirect_scale: 1.0,
            sdf_shadow_flags: 0,
            sdf_shadow_mode: SdfShadowMode::On,
            sdf_force_visibility_one: false,
            dynamic_direct_scale: 0.25,
            dynamic_direct_isolation: DynamicDirectIsolation::IndirectOnly,
            has_direct: true,
        });
        let scale = f32::from_ne_bytes(data[108..112].try_into().unwrap());
        assert!((scale - 0.25).abs() < 1e-6, "direct_scale at 108..112");
        assert_eq!(
            u32::from_ne_bytes(data[112..116].try_into().unwrap()),
            DynamicDirectIsolation::IndirectOnly as u32,
            "dynamic_direct_isolation at 112..116",
        );
        assert_eq!(
            u32::from_ne_bytes(data[116..120].try_into().unwrap()),
            1,
            "has_direct at 116..120",
        );
        assert!(
            data[120..128].iter().all(|&b| b == 0),
            "trailing pad 120..128 must stay zero",
        );
    }

    #[test]
    fn line_indices_from_single_triangle_produces_three_edges() {
        let tri = vec![0u32, 1, 2];
        let lines = build_line_indices_from_triangles(&tri);
        assert_eq!(lines, vec![0, 1, 1, 2, 2, 0]);
    }

    #[test]
    fn line_indices_from_two_triangles_produces_twelve_indices() {
        let tris = vec![0u32, 1, 2, 3, 4, 5];
        let lines = build_line_indices_from_triangles(&tris);
        assert_eq!(lines.len(), 12);
        assert_eq!(lines, vec![0, 1, 1, 2, 2, 0, 3, 4, 4, 5, 5, 3]);
    }

    #[test]
    fn line_indices_from_empty_input_is_empty() {
        let lines = build_line_indices_from_triangles(&[]);
        assert!(lines.is_empty());
    }

    #[test]
    fn line_indices_ignores_incomplete_trailing_triangle() {
        // 4 indices = 1 full triangle + 1 dangling index.
        let tris = vec![0u32, 1, 2, 3];
        let lines = build_line_indices_from_triangles(&tris);
        assert_eq!(lines, vec![0, 1, 1, 2, 2, 0]);
    }

    fn scripted_light_intensity_scalar_reference(
        premultiplied_color: [f32; 3],
        base_color: [f32; 3],
    ) -> f32 {
        let (premultiplied_channel, color_channel) =
            if base_color[0] >= base_color[1] && base_color[0] >= base_color[2] {
                (premultiplied_color[0], base_color[0])
            } else if base_color[1] >= base_color[2] {
                (premultiplied_color[1], base_color[1])
            } else {
                (premultiplied_color[2], base_color[2])
            };
        if color_channel <= 1.0e-6 {
            return 0.0;
        }
        premultiplied_channel / color_channel
    }

    fn scripted_color_curve_effective_color(
        premultiplied_color: [f32; 3],
        base_color: [f32; 3],
        color_sample: [f32; 3],
        brightness: f32,
    ) -> [f32; 3] {
        let intensity = scripted_light_intensity_scalar_reference(premultiplied_color, base_color);
        [
            color_sample[0].max(0.0) * intensity * brightness.max(0.0),
            color_sample[1].max(0.0) * intensity * brightness.max(0.0),
            color_sample[2].max(0.0) * intensity * brightness.max(0.0),
        ]
    }

    fn assert_vec3_near(actual: [f32; 3], expected: [f32; 3]) {
        for i in 0..3 {
            assert!(
                (actual[i] - expected[i]).abs() < 1.0e-6,
                "channel {i}: expected {}, got {}",
                expected[i],
                actual[i],
            );
        }
    }

    #[test]
    fn forward_shader_color_curve_branch_reapplies_static_intensity() {
        let src = include_str!("../shaders/forward.wgsl");
        let color_branch_start = src
            .find("if scripted_desc.color_count > 0u")
            .expect("forward shader should have a scripted color-curve branch");
        let brightness_branch_start = src[color_branch_start..]
            .find("} else if scripted_desc.brightness_count > 0u")
            .map(|offset| color_branch_start + offset)
            .expect("forward shader should keep a brightness-only branch");
        let color_branch = &src[color_branch_start..brightness_branch_start];

        assert!(
            color_branch.contains("let unit_sample = max("),
            "color branch should bind the clamped unit-RGB sample before applying intensity",
        );
        assert!(
            color_branch.contains("light_eval_scripted_intensity_scalar("),
            "color branch should recover the static intensity scalar",
        );
        assert!(
            color_branch.contains("effective_color = unit_sample * intensity * brightness;"),
            "color branch should apply unit sample, static intensity, and optional brightness multiplicatively",
        );
        assert!(
            !color_branch.contains("effective_color = max("),
            "color branch must not assign the raw clamped unit-RGB sample as final effective_color",
        );
    }

    #[test]
    fn scripted_color_curve_white_sample_keeps_static_intensity() {
        let actual = scripted_color_curve_effective_color(
            [10.0, 10.0, 10.0],
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
            1.0,
        );
        assert_vec3_near(actual, [10.0, 10.0, 10.0]);
    }

    #[test]
    fn scripted_color_curve_hue_sample_uses_static_intensity_as_magnitude() {
        let actual = scripted_color_curve_effective_color(
            [10.0, 10.0, 10.0],
            [1.0, 1.0, 1.0],
            [0.5, 0.0, 0.0],
            1.0,
        );
        assert_vec3_near(actual, [5.0, 0.0, 0.0]);
    }

    #[test]
    fn scripted_color_curve_multiplies_optional_brightness_curve() {
        let actual = scripted_color_curve_effective_color(
            [10.0, 10.0, 10.0],
            [1.0, 1.0, 1.0],
            [1.0, 0.0, 0.0],
            0.5,
        );
        assert_vec3_near(actual, [5.0, 0.0, 0.0]);
    }

    /// Regression: both the CPU-side `build_uniform_data` packer and the
    /// CPU-side `pack_light` packer must match the WGSL struct layouts
    /// that the fragment shader compiles against. Parsing the live
    /// shader source with naga catches drift before it reaches a GPU
    /// round-trip (see the similar test in `compute_cull.rs`).
    #[test]
    fn forward_wgsl_struct_strides_match_cpu_layout() {
        let module = naga::front::wgsl::parse_str(SHADER_SOURCE)
            .expect("forward shader should parse as WGSL");

        let mut seen = std::collections::HashMap::new();
        for (_handle, ty) in module.types.iter() {
            if let naga::TypeInner::Struct { span, .. } = &ty.inner
                && let Some(name) = &ty.name
            {
                seen.insert(name.clone(), *span);
            }
        }

        let uniforms_span = seen
            .get("Uniforms")
            .copied()
            .expect("forward shader should declare struct Uniforms");
        assert_eq!(
            uniforms_span as usize, UNIFORM_SIZE,
            "forward.wgsl Uniforms stride ({uniforms_span}) must match UNIFORM_SIZE ({UNIFORM_SIZE})",
        );

        let light_span = seen
            .get("GpuLight")
            .copied()
            .expect("forward shader should declare struct GpuLight");
        assert_eq!(
            light_span as usize,
            crate::lighting::GPU_LIGHT_SIZE,
            "forward.wgsl GpuLight stride ({light_span}) must match GPU_LIGHT_SIZE ({})",
            crate::lighting::GPU_LIGHT_SIZE,
        );
    }

    /// Task 5 (sdf-static-occluder-shadows): the forward shader must parse
    /// cleanly with the new SDF shadow-factor bindings (`sdf_shadow_factor` and
    /// `sdf_shadow_depth` on group 5 bindings 3 and 4) and must declare the
    /// inline bilateral upsample helper. Mirrors the parse-and-binding shape of
    /// Task 2b's `compose_shader_parses_and_declares_debug_binding`.
    #[test]
    fn forward_shader_parses_and_declares_sdf_shadow_upsample() {
        let src = SHADER_SOURCE;
        let module = naga::front::wgsl::parse_str(src)
            .expect("forward.wgsl should parse as WGSL after Task 5 plumbing");

        // The upsample function is the public surface of the bilateral filter.
        let has_upsample = module
            .functions
            .iter()
            .any(|(_h, f)| f.name.as_deref() == Some("upsample_shadow_factor"));
        assert!(
            has_upsample,
            "forward.wgsl must declare `upsample_shadow_factor` (Task 5 bilateral upsample)",
        );

        // The bilateral filter is depth-aware — both the factor target and
        // the scene depth texture must be declared.
        assert!(
            src.contains("sdf_shadow_factor"),
            "forward.wgsl must bind the half-res SDF shadow factor target",
        );
        assert!(
            src.contains("sdf_shadow_depth"),
            "forward.wgsl must bind the scene depth texture for the depth-aware bilateral",
        );

        // The fragment entry point must reference the upsample helper — else
        // the wiring is dead and the multiply never lands.
        let fs = src
            .find("fn fs_main(")
            .expect("forward.wgsl must declare fs_main");
        let fs_tail = &src[fs..];
        assert!(
            fs_tail.contains("upsample_shadow_factor("),
            "fs_main must call upsample_shadow_factor (otherwise the multiply is dead)",
        );

        // The gating bitset must be wired into the Uniforms struct.
        assert!(
            src.contains("sdf_shadow_flags"),
            "forward.wgsl Uniforms must include the `sdf_shadow_flags` gate field",
        );
    }

    /// Guards that the forward shader composes `sdf_light_select.wgsl` and
    /// validates end-to-end: `select_sdf_lights` (K-selection parity seam with
    /// the visibility pass) and `slice_for_visibility` (per-light diffuse
    /// multiply via R/B/A slices) must be declared and called from `fs_main`.
    /// Also confirms the bilateral upsample wiring is intact. Full naga
    /// validation — not just parse — catches type/binding errors.
    #[test]
    fn forward_shader_composes_sdf_light_selection_and_reads_slices() {
        let src = SHADER_SOURCE;
        let module = naga::front::wgsl::parse_str(src)
            .expect("forward + sdf_light_select must parse as one composed WGSL module");
        // Full validation catches type/binding errors a bare parse misses.
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("forward + sdf_light_select composed source should validate");

        // The shared selection helper must be present as a function — proving
        // the helper source was concatenated, not reimplemented inline.
        let has_select = module
            .functions
            .iter()
            .any(|(_h, f)| f.name.as_deref() == Some("select_sdf_lights"));
        assert!(
            has_select,
            "forward must compose the shared `select_sdf_lights` helper (K-selection parity seam)",
        );

        // The slice→channel mapper must exist — it is how the forward reads a
        // selection slot's visibility (slot 0→R, 1→B, 2→A).
        let has_slice_map = module
            .functions
            .iter()
            .any(|(_h, f)| f.name.as_deref() == Some("slice_for_visibility"));
        assert!(
            has_slice_map,
            "forward must declare `slice_for_visibility` to read per-light slices from R/B/A",
        );

        // fs_main must actually drive the per-light path: select the lights and
        // read each one's slice — else the diffuse term attaches to nothing.
        let fs = src
            .find("fn fs_main(")
            .expect("forward.wgsl must declare fs_main");
        let fs_tail = &src[fs..];
        assert!(
            fs_tail.contains("select_sdf_lights("),
            "fs_main must call select_sdf_lights (parity with the visibility pass)",
        );
        assert!(
            fs_tail.contains("slice_for_visibility("),
            "fs_main must read per-light visibility via slice_for_visibility (else slices are dead)",
        );

        // The dev force-visibility-1.0 toggle must be wired into the Uniforms
        // struct (drives the no-double-count A/B).
        assert!(
            src.contains("sdf_force_visibility_one"),
            "forward.wgsl Uniforms must include the `sdf_force_visibility_one` dev toggle",
        );
    }

    /// Pins Task 5's headline contract (invariant 9): an `sdf`-typed light's
    /// SPECULAR term reads the SAME per-light visibility slice as its diffuse.
    /// The specular loop walks the chunk list in chunk order, so it resolves the
    /// slice through `sdf_visibility_for_light`, which finds the light's slot in
    /// the shared `sdf_sel` selection and maps it via `slice_for_visibility` —
    /// the same selection and slot→channel mapping the diffuse loop uses, so the
    /// two terms read the same slice by construction. Full naga validation plus
    /// structural assertions that the resolver exists, is composed, and is
    /// actually applied to the specular contribution in `fs_main`.
    #[test]
    fn forward_shader_specular_reads_sdf_visibility_slice() {
        let src = SHADER_SOURCE;
        let module = naga::front::wgsl::parse_str(src)
            .expect("forward + sdf_light_select must parse as one composed WGSL module");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("forward + sdf_light_select composed source should validate");

        // The specular slice resolver must exist as a function.
        let has_resolver = module
            .functions
            .iter()
            .any(|(_h, f)| f.name.as_deref() == Some("sdf_visibility_for_light"));
        assert!(
            has_resolver,
            "forward must declare `sdf_visibility_for_light` (specular reads the per-light slice)",
        );

        let fs = src
            .find("fn fs_main(")
            .expect("forward.wgsl must declare fs_main");
        let fs_tail = &src[fs..];

        // The specular loop must drive the resolver — else specular is unshadowed
        // for sdf lights and Task 5's headline contract is unmet.
        assert!(
            fs_tail.contains("sdf_visibility_for_light("),
            "fs_main must call sdf_visibility_for_light so sdf specular reads its visibility slice",
        );

        // Diffuse and specular must read off the SAME selection: one shared
        // `sdf_sel` (single `select_sdf_lights` call), not two. A second call
        // could drift the slot ordering and break diffuse/specular parity.
        // Count against forward.wgsl ALONE — `SHADER_SOURCE` appends the helper
        // file, whose `fn select_sdf_lights(` definition would otherwise count.
        let forward_only = include_str!("../shaders/forward.wgsl");
        assert_eq!(
            forward_only.matches("select_sdf_lights(").count(),
            1,
            "forward.wgsl must call select_sdf_lights exactly once (diffuse + specular share one selection)",
        );
        assert!(
            fs_tail.contains("sdf_visibility_for_light(sdf_sel,"),
            "specular must resolve visibility through the shared `sdf_sel` selection",
        );

        // The specular contribution must actually be multiplied by the resolved
        // visibility (gated through the sdf tag), proving the slice reaches the
        // blinn-phong term and is not dead.
        assert!(
            fs_tail.contains("sdf_select_is_sdf("),
            "specular must gate visibility on the sdf tag via sdf_select_is_sdf",
        );
    }

    /// Regression: the SH volume's `ShGridInfo` uniform struct must have
    /// matching byte stride on both sides of the bind group — CPU packer
    /// (`sh_volume::build_grid_info_bytes`) and the fragment shader's
    /// declaration in `forward.wgsl`.
    #[test]
    fn forward_wgsl_sh_grid_info_matches_cpu_layout() {
        let module = naga::front::wgsl::parse_str(SHADER_SOURCE)
            .expect("forward shader should parse as WGSL");

        let mut seen = std::collections::HashMap::new();
        for (_handle, ty) in module.types.iter() {
            if let naga::TypeInner::Struct { span, .. } = &ty.inner
                && let Some(name) = &ty.name
            {
                seen.insert(name.clone(), *span);
            }
        }

        let span = seen
            .get("ShGridInfo")
            .copied()
            .expect("forward shader should declare struct ShGridInfo");
        assert_eq!(
            span as usize,
            sh_volume::SH_GRID_INFO_SIZE,
            "forward.wgsl ShGridInfo stride ({span}) must match SH_GRID_INFO_SIZE ({})",
            sh_volume::SH_GRID_INFO_SIZE,
        );

        let desc_span = seen
            .get("AnimationDescriptor")
            .copied()
            .expect("forward shader should declare struct AnimationDescriptor");
        assert_eq!(
            desc_span as usize,
            sh_volume::ANIMATION_DESCRIPTOR_SIZE,
            "forward.wgsl AnimationDescriptor stride ({desc_span}) must match ANIMATION_DESCRIPTOR_SIZE ({})",
            sh_volume::ANIMATION_DESCRIPTOR_SIZE,
        );
    }

    /// Regression: every storage/uniform buffer binding in `forward.wgsl` must
    /// receive a payload large enough to satisfy wgpu's minimum-binding-size
    /// validation. The original bug was `anim_descriptors` bound with 16 B while
    /// `array<AnimationDescriptor>` requires ≥ 48 B (one full element stride).
    ///
    /// Strategy: parse the live shader with naga, derive the minimum required
    /// size for every buffer binding from the WGSL type information, then check
    /// that the Rust-side dummy payloads (empty-map / no-SH-section case) are
    /// at least that large. Catches mismatches at `cargo test` time, not at
    /// draw time on real hardware.
    #[test]
    fn forward_wgsl_dummy_buffers_meet_shader_min_binding_size() {
        use std::collections::HashMap;

        let module = naga::front::wgsl::parse_str(SHADER_SOURCE)
            .expect("forward shader should parse as WGSL");

        // Build (group, binding) → minimum byte count required by the shader.
        // Only storage and uniform address spaces produce buffer bindings.
        let mut min_sizes: HashMap<(u32, u32), u64> = HashMap::new();
        for (_handle, var) in module.global_variables.iter() {
            let is_buffer = matches!(
                var.space,
                naga::AddressSpace::Storage { .. } | naga::AddressSpace::Uniform
            );
            if !is_buffer {
                continue;
            }
            let Some(rb) = &var.binding else { continue };
            let ty = &module.types[var.ty];
            let min: u64 = match &ty.inner {
                // Unbounded array<T> — shader needs at least one element.
                naga::TypeInner::Array {
                    stride,
                    size: naga::ArraySize::Dynamic,
                    ..
                } => *stride as u64,
                // Bounded array<T, N> — shader needs all N elements.
                naga::TypeInner::Array {
                    stride,
                    size: naga::ArraySize::Constant(n),
                    ..
                } => n.get() as u64 * *stride as u64,
                // Struct — shader needs the full declared span.
                naga::TypeInner::Struct { span, .. } => *span as u64,
                // Scalars / vectors / matrices: trivially satisfied; skip.
                _ => continue,
            };
            min_sizes.insert((rb.group, rb.binding), min);
        }

        // Verify that the empty-map dummy animation buffers (no SH section)
        // satisfy the shader's per-binding size requirements.
        //
        // binding 11: array<AnimationDescriptor> — stride = ANIMATION_DESCRIPTOR_SIZE
        // binding 12: array<f32>                 — stride = 4
        let (anim_desc, anim_samples, _count) = sh_volume::build_animation_buffers(None);

        for (label, binding, buf) in [
            (
                "anim_descriptors",
                sh_volume::BIND_ANIM_DESCRIPTORS,
                anim_desc.as_slice(),
            ),
            (
                "anim_samples",
                sh_volume::BIND_ANIM_SAMPLES,
                anim_samples.as_slice(),
            ),
        ] {
            if let Some(&min) = min_sizes.get(&(3, binding)) {
                assert!(
                    buf.len() as u64 >= min,
                    "dummy {label} buffer (group=3, binding={binding}): Rust side \
                     produces {} B but forward.wgsl min binding size is {min} B \
                     (array element stride — at least one element required)",
                    buf.len(),
                );
            } else {
                panic!(
                    "forward.wgsl has no buffer at group=3 binding={binding}; \
                        check BIND_* constants match shader @binding decorators"
                );
            }
        }

        // Verify the ShGridInfo uniform payload size.
        let sh_grid_binding = sh_volume::BIND_SH_GRID_INFO;
        let grid_info = sh_volume::build_grid_info_bytes(sh_volume::ShGridInfoParams {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: [1, 1, 1],
            atlas_dimensions: [1, 1],
            tile_dimension: 1,
            tile_border: 0,
            atlas_tiles_per_row: 1,
            present: false,
            probe_occlusion_enabled: true,
        });
        if let Some(&min) = min_sizes.get(&(3, sh_grid_binding)) {
            assert!(
                grid_info.len() as u64 >= min,
                "sh_grid uniform (group=3, binding={sh_grid_binding}): Rust side \
                 produces {} B but forward.wgsl struct span is {min} B",
                grid_info.len(),
            );
        } else {
            panic!(
                "forward.wgsl has no uniform at group=3 binding={sh_grid_binding}; \
                    check BIND_SH_GRID_INFO matches shader @binding decorators"
            );
        }
    }

    /// Validates that `forward.wgsl` passes naga's full uniformity analysis.
    /// Implicit derivatives (`dpdx`/`dpdy`) and `textureSample` must stay in
    /// uniform control flow; the anisotropic filtering branches must use only
    /// `textureSampleGrad` (which is safe under non-uniform flow).  Naga's
    /// `Validator` enforces this property — `parse_str` alone does not.
    /// A future edit that moves a derivative call under a non-uniform branch
    /// would silently pass `parse_str` but will be caught here at `cargo test`
    /// time, before reaching GPU pipeline creation.
    #[test]
    fn forward_wgsl_passes_naga_validation() {
        let module = naga::front::wgsl::parse_str(SHADER_SOURCE).expect("forward.wgsl must parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("forward.wgsl must pass naga validation (control-flow uniformity)");
    }

    /// The no-`CUBE_ARRAY_TEXTURES` variant of the forward shader, derived from the
    /// single source via `strip_point_shadow_cube`, must (1) drop the
    /// `point_shadow_cube` binding entirely so it matches a group-5 BGL that omits
    /// binding 5, and (2) still parse + pass naga validation. This is what ships on
    /// an adapter without the feature — point shadows cleanly off, no panic.
    #[test]
    fn forward_wgsl_no_cube_variant_strips_binding_and_validates() {
        let stripped = strip_point_shadow_cube(SHADER_SOURCE);
        // The `point_shadow_cube` binding DECLARATION is gone (comments mentioning
        // the name in prose are harmless; naga validation below proves there is no
        // dangling code reference).
        assert!(
            !stripped.contains("var point_shadow_cube:"),
            "no-cube forward variant must not declare the point_shadow_cube binding"
        );
        // The body markers (and everything between them, including the cube
        // sample) are gone, replaced by the no-shadow constant. naga validation
        // below is the real guarantee that no code references the absent binding.
        assert!(
            !stripped.contains("CUBE_SHADOW_BODY_BEGIN")
                && !stripped.contains("CUBE_SHADOW_BODY_END"),
            "no-cube forward variant must consume the sample_point_shadow body markers"
        );
        // The supported variant keeps the declaration (sanity that the transform
        // actually removed something).
        assert!(SHADER_SOURCE.contains("var point_shadow_cube:"));

        let module =
            naga::front::wgsl::parse_str(&stripped).expect("no-cube forward variant must parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("no-cube forward variant must pass naga validation");
    }

    /// The depth pre-pass shader must parse as valid WGSL and declare
    /// the same `Uniforms` struct binding as `forward.wgsl` (only the
    /// leading `view_proj` field is referenced, but the shader still
    /// needs to compile cleanly).
    #[test]
    fn depth_prepass_wgsl_parses() {
        let module = naga::front::wgsl::parse_str(DEPTH_PREPASS_SHADER_SOURCE)
            .expect("depth_prepass.wgsl should parse as WGSL");
        // Sanity: the vertex entry point must be named `vs_main` so the
        // pipeline's `entry_point: Some("vs_main")` resolves.
        let has_vs_main = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "vs_main" && ep.stage == naga::ShaderStage::Vertex);
        assert!(
            has_vs_main,
            "depth_prepass.wgsl must export @vertex vs_main"
        );
        // Vertex-only: the lightmap-UV gbuffer MRT was removed with the animated
        // dominant-direction trace, so there must be NO fragment stage.
        let has_fs = module
            .entry_points
            .iter()
            .any(|ep| ep.stage == naga::ShaderStage::Fragment);
        assert!(
            !has_fs,
            "depth_prepass.wgsl must be vertex-only — the gbuffer MRT was removed"
        );
    }

    /// The depth pre-pass attachment is recreated at the surface size on resize.
    /// Actual texture creation needs a GPU device (unavailable in `cargo test`);
    /// the size decision is factored into `prepass_attachment_extent`, asserted
    /// here. Zero-size transients clamp to 1 so texture creation stays valid.
    #[test]
    fn prepass_attachment_extent_matches_surface_size() {
        let e = prepass_attachment_extent(1920, 1080);
        assert_eq!(
            (e.width, e.height, e.depth_or_array_layers),
            (1920, 1080, 1)
        );
        // Zero-size transients clamp to 1 so texture creation stays valid.
        assert_eq!(
            prepass_attachment_extent(0, 0),
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Ensure the wireframe shader's `Uniforms` struct stays in sync with
    /// the forward shader's — they share a single uniform buffer binding.
    #[test]
    fn wireframe_wgsl_uniforms_match_forward_layout() {
        let module = naga::front::wgsl::parse_str(WIREFRAME_SHADER_SOURCE)
            .expect("wireframe shader should parse as WGSL");

        let mut seen = std::collections::HashMap::new();
        for (_handle, ty) in module.types.iter() {
            if let naga::TypeInner::Struct { span, .. } = &ty.inner
                && let Some(name) = &ty.name
            {
                seen.insert(name.clone(), *span);
            }
        }

        let uniforms_span = seen
            .get("Uniforms")
            .copied()
            .expect("wireframe shader should declare struct Uniforms");
        assert_eq!(
            uniforms_span as usize, UNIFORM_SIZE,
            "wireframe.wgsl Uniforms stride ({uniforms_span}) must match UNIFORM_SIZE ({UNIFORM_SIZE})",
        );
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
            lighting_isolation: LightingIsolation::Normal,
            indirect_scale,
            sdf_shadow_flags: 0,
            sdf_shadow_mode: SdfShadowMode::On,
            sdf_force_visibility_one: false,
            dynamic_direct_scale: DEFAULT_DYNAMIC_DIRECT_SCALE,
            dynamic_direct_isolation: DynamicDirectIsolation::Combined,
            has_direct: false,
        });

        // view_proj: first 64 bytes = 16 f32 identity columns.
        let mut floats = Vec::new();
        for chunk in data.chunks_exact(4).take(16) {
            floats.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        let identity = Mat4::IDENTITY.to_cols_array();
        for i in 0..16 {
            let epsilon = 1e-6;
            assert!(
                (floats[i] - identity[i]).abs() < epsilon,
                "view_proj[{i}] mismatch: expected {}, got {}",
                identity[i],
                floats[i],
            );
        }

        // camera_position at bytes 64..76.
        let cx = f32::from_ne_bytes(data[64..68].try_into().unwrap());
        let cy = f32::from_ne_bytes(data[68..72].try_into().unwrap());
        let cz = f32::from_ne_bytes(data[72..76].try_into().unwrap());
        assert_eq!(cx, 10.0);
        assert_eq!(cy, 20.0);
        assert_eq!(cz, 30.0);

        // ambient_floor at bytes 76..80.
        let af = f32::from_ne_bytes(data[76..80].try_into().unwrap());
        assert!((af - ambient_floor).abs() < 1e-6);

        // light_count at bytes 80..84.
        let lc = u32::from_ne_bytes(data[80..84].try_into().unwrap());
        assert_eq!(lc, light_count);

        // time at bytes 84..88 (passed 0.0 in this test).
        let t = f32::from_ne_bytes(data[84..88].try_into().unwrap());
        assert_eq!(t, 0.0);

        // lighting_isolation at bytes 88..92 (passed Normal = 0).
        let iso = u32::from_ne_bytes(data[88..92].try_into().unwrap());
        assert_eq!(iso, 0);

        // indirect_scale at bytes 92..96.
        let scale = f32::from_ne_bytes(data[92..96].try_into().unwrap());
        assert!((scale - indirect_scale).abs() < 1e-6);
    }

    // Regression: spot-shadow clock skew — GPU `time` uniform must equal
    // `script_time` so shadow-pool eligibility (CPU) and GPU animation phase
    // stay in sync. Using wall-clock here instead would desync them.
    #[test]
    fn uniform_data_encodes_script_time_as_gpu_time_field() {
        let script_time = 3.75_f32;
        let data = build_uniform_data(&FrameUniforms {
            view_proj: Mat4::IDENTITY,
            camera_position: Vec3::ZERO,
            ambient_floor: 0.0,
            light_count: 0,
            time: script_time,
            lighting_isolation: LightingIsolation::Normal,
            indirect_scale: 1.0,
            sdf_shadow_flags: 0,
            sdf_shadow_mode: SdfShadowMode::On,
            sdf_force_visibility_one: false,
            dynamic_direct_scale: DEFAULT_DYNAMIC_DIRECT_SCALE,
            dynamic_direct_isolation: DynamicDirectIsolation::Combined,
            has_direct: false,
        });
        // time at bytes 84..88.
        let t = f32::from_ne_bytes(data[84..88].try_into().unwrap());
        assert!(
            (t - script_time).abs() < 1e-6,
            "GPU time ({t}) must equal script_time ({script_time})",
        );
    }

    /// Static lights are baked into the lightmap; including them in the
    /// runtime direct-light loop would double-apply their contribution on
    /// top of the bake. The filter at renderer init time must drop them
    /// while keeping influences index-aligned with the surviving lights.
    #[test]
    fn dynamic_light_filter_excludes_static_lights() {
        fn mk_light(intensity: f32, is_dynamic: bool) -> MapLight {
            MapLight {
                origin: [0.0, 0.0, 0.0],
                light_type: crate::prl::LightType::Point,
                // intensity doubles as an identity tag so the test can verify
                // ordering after the filter without inspecting other fields.
                intensity,
                color: [1.0, 1.0, 1.0],
                falloff_model: crate::prl::FalloffModel::InverseSquared,
                falloff_range: 10.0,
                cone_angle_inner: 0.0,
                cone_angle_outer: 0.0,
                cone_direction: [0.0, 0.0, -1.0],
                is_dynamic,
                casts_entity_shadows: false,
                animated_slot: None,
                tags: vec![],
                leaf_index: 0,
                shadow_type: crate::prl::ShadowType::StaticLightMap,
            }
        }

        // Mixed input: dyn, static, dyn, static, dyn — three should survive.
        let lights = vec![
            mk_light(1.0, true),
            mk_light(2.0, false),
            mk_light(3.0, true),
            mk_light(4.0, false),
            mk_light(5.0, true),
        ];
        // Each influence's `radius` doubles as an identity tag so the test
        // can verify alignment between surviving lights and their influence.
        let influences = vec![
            LightInfluence {
                center: Vec3::new(1.0, 0.0, 0.0),
                radius: 1.0,
            },
            LightInfluence {
                center: Vec3::new(2.0, 0.0, 0.0),
                radius: 2.0,
            },
            LightInfluence {
                center: Vec3::new(3.0, 0.0, 0.0),
                radius: 3.0,
            },
            LightInfluence {
                center: Vec3::new(4.0, 0.0, 0.0),
                radius: 4.0,
            },
            LightInfluence {
                center: Vec3::new(5.0, 0.0, 0.0),
                radius: 5.0,
            },
        ];

        let (out_lights, out_influences) = filter_dynamic_lights(&lights, &influences);

        assert_eq!(out_lights.len(), 3, "expected 3 dynamic lights");
        assert_eq!(out_influences.len(), 3, "influences must match lights len");

        // Surviving lights are the dynamic ones (intensity 1, 3, 5) in order.
        assert_eq!(out_lights[0].intensity, 1.0);
        assert_eq!(out_lights[1].intensity, 3.0);
        assert_eq!(out_lights[2].intensity, 5.0);
        assert!(out_lights.iter().all(|l| l.is_dynamic));

        // Influences are aligned with the original light's index — radius
        // 1.0 stays paired with the light tagged 1.0, not shifted.
        assert_eq!(out_influences[0].radius, 1.0);
        assert_eq!(out_influences[1].radius, 3.0);
        assert_eq!(out_influences[2].radius, 5.0);
        assert_eq!(out_influences[0].center, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(out_influences[1].center, Vec3::new(3.0, 0.0, 0.0));
        assert_eq!(out_influences[2].center, Vec3::new(5.0, 0.0, 0.0));
    }

    /// Valid 64-char hex string round-trips to the expected 32 bytes.
    #[test]
    fn parse_blake3_key_parses_valid_hex_to_expected_bytes() {
        // 32 bytes: 00 01 02 … 1e 1f
        let hex = (0u8..32).map(|b| format!("{b:02x}")).collect::<String>();
        let result = parse_blake3_key(&hex);
        let expected: [u8; 32] = std::array::from_fn(|i| i as u8);
        assert_eq!(result, expected);
    }

    /// A hex string that is too short yields the zero sentinel key.
    #[test]
    fn parse_blake3_key_wrong_length_returns_zero_sentinel() {
        // 63 chars — one short of the required 64.
        let short = "a".repeat(63);
        assert_eq!(parse_blake3_key(&short), [0u8; 32]);
    }

    /// A non-hex character anywhere in the string yields the zero sentinel key.
    #[test]
    fn parse_blake3_key_non_hex_chars_return_zero_sentinel() {
        // 64 chars but contains 'zz' at the start — not valid hex.
        let bad = format!("zz{}", "00".repeat(31));
        assert_eq!(parse_blake3_key(&bad), [0u8; 32]);
    }

    // Regression: a 64-byte non-ASCII key panicked on a UTF-8 boundary slice.
    #[test]
    fn parse_blake3_key_non_ascii_input_does_not_panic_and_returns_zero_sentinel() {
        let non_ascii = "é".repeat(32);
        assert_eq!(non_ascii.len(), 64);

        let result = std::panic::catch_unwind(|| parse_blake3_key(&non_ascii));

        assert!(result.is_ok());
        assert_eq!(result.expect("parser must not panic"), [0u8; 32]);
    }

    /// The all-zero 64-char sentinel string maps to the zero key. This is the
    /// same string `zero_material_key()` in the loader produces ("0".repeat(64)),
    /// pinning the cross-module contract without importing that function here.
    #[test]
    fn parse_blake3_key_maps_zero_sentinel_to_zero_key() {
        assert_eq!(parse_blake3_key(&"0".repeat(64)), [0u8; 32]);
    }

    // --- Model open-path vs. cache-key split (finding: content_root join) ---
    //
    // `load_skinned_model` needs a live `wgpu::Device`, so the path/key
    // derivation is factored into the pure `resolve_model_open_path_and_handle`.
    // These pin the contract: the glTF opens content-root-JOINED while the cache
    // key stays the VERBATIM handle, so it equals what `mesh_render.rs` produces
    // from `mesh.model` (`ModelHandle::from(mesh.model.clone())`) and the
    // planner's `models.get(&group.model)` lookup hits.

    #[test]
    fn model_cache_key_is_the_verbatim_handle_while_open_path_is_joined() {
        let content_root = Path::new("/content/root");
        let model_rel = "models/x/scene.gltf";
        let (open_path, handle) = resolve_model_open_path_and_handle(model_rel, content_root);

        // Open path is joined under the content root.
        assert_eq!(open_path, content_root.join(model_rel));
        // Cache key is the raw handle, NOT the joined path — must match the
        // per-frame collector's `ModelHandle::from(mesh.model.clone())`.
        assert_eq!(handle, crate::model::ModelHandle::from(model_rel));
        assert_eq!(handle.as_str(), model_rel);
        // And the key is explicitly not the joined string.
        assert_ne!(handle.as_str(), open_path.to_string_lossy());
    }

    // --- Submesh material plan (GPU-free dedup + draw bookkeeping) ---------
    //
    // `resolve_skinned_model_material` needs a live `wgpu::Device` to build bind
    // groups, so the dedup + range bookkeeping is factored into the pure
    // `plan_submesh_materials`. These tests pin the contract the GPU layer
    // builds on: one distinct key per distinct material (deduped), one draw per
    // submesh covering its range, in submesh order.

    use crate::model::gltf_loader::Submesh;

    fn submesh(key: &str, start: u32, end: u32) -> Submesh {
        Submesh {
            material_key: key.to_string(),
            indices: start..end,
        }
    }

    #[test]
    fn plan_records_one_draw_per_submesh_covering_every_range() {
        // Three distinct materials → three submeshes; every range must be
        // recorded (not just the first), in submesh order, each pointing at its
        // own distinct key.
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let c = "c".repeat(64);
        let submeshes = vec![submesh(&a, 0, 6), submesh(&b, 6, 12), submesh(&c, 12, 15)];

        let plan = plan_submesh_materials(&submeshes);

        // Three distinct keys, in first-seen order.
        assert_eq!(plan.distinct_keys, vec![a, b, c]);
        // One draw per submesh, ranges preserved in submesh order, each to its
        // own distinct material (0, 1, 2) — every range covered, not just #0.
        assert_eq!(plan.draws.len(), 3, "one draw entry per submesh");
        assert_eq!(plan.draws[0].indices, 0..6);
        assert_eq!(plan.draws[1].indices, 6..12);
        assert_eq!(plan.draws[2].indices, 12..15);
        assert_eq!(
            plan.draws.iter().map(|d| d.distinct).collect::<Vec<_>>(),
            vec![0, 1, 2],
            "distinct materials map to distinct plan entries",
        );
    }

    #[test]
    fn plan_dedups_repeated_material_key_to_one_build() {
        // A model reusing one material across three primitives must build that
        // material ONCE (one distinct key) while still recording three draws —
        // each submesh range paired with the shared (deduped) material.
        let shared = "f".repeat(64);
        let submeshes = vec![
            submesh(&shared, 0, 3),
            submesh(&shared, 3, 6),
            submesh(&shared, 6, 9),
        ];

        let plan = plan_submesh_materials(&submeshes);

        assert_eq!(
            plan.distinct_keys.len(),
            1,
            "reused material key dedups to a single bind-group build",
        );
        assert_eq!(plan.distinct_keys[0], shared);
        assert_eq!(plan.draws.len(), 3, "still one draw per submesh");
        assert!(
            plan.draws.iter().all(|d| d.distinct == 0),
            "every submesh shares the one distinct material",
        );
        // Ranges still cover each submesh independently.
        assert_eq!(
            plan.draws
                .iter()
                .map(|d| d.indices.clone())
                .collect::<Vec<_>>(),
            vec![0..3, 3..6, 6..9],
        );
    }

    #[test]
    fn plan_mixes_shared_and_distinct_keys_with_first_seen_order() {
        // Interleaved reuse: keys [x, y, x, z]. Distinct keys are first-seen
        // [x, y, z] (3 builds, not 4), and the third submesh reuses x's entry.
        let x = "1".repeat(64);
        let y = "2".repeat(64);
        let z = "3".repeat(64);
        let submeshes = vec![
            submesh(&x, 0, 3),
            submesh(&y, 3, 6),
            submesh(&x, 6, 9),
            submesh(&z, 9, 12),
        ];

        let plan = plan_submesh_materials(&submeshes);

        assert_eq!(
            plan.distinct_keys,
            vec![x, y, z],
            "distinct keys in first-seen order, deduped",
        );
        assert_eq!(
            plan.draws.iter().map(|d| d.distinct).collect::<Vec<_>>(),
            vec![0, 1, 0, 2],
            "third submesh reuses the first distinct material",
        );
        assert_eq!(plan.draws.len(), 4, "one draw per submesh, none dropped");
    }
}
