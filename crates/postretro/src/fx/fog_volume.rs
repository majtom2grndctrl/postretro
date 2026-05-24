// CPU-side GPU types and packing for the volumetric fog pass.
// See: context/lib/rendering_pipeline.md §7.5

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

// Authoritative definitions live in `postretro_level_format::fog_volumes`;
// re-exported here so the renderer can import from a single crate-local path.
pub use postretro_level_format::fog_volumes::{MAX_FOG_VOLUMES, MAX_PLANES_PER_VOLUME};

/// Capacity (in `vec4<f32>` planes) of the global `fog_planes` storage buffer.
/// Worst case: every fog volume slot active with the maximum plane budget.
pub const FOG_PLANES_BUFFER_CAPACITY: usize = MAX_FOG_VOLUMES * MAX_PLANES_PER_VOLUME;

/// Maximum number of fog point lights iterated per ray step. The point-light
/// loop in `fog_volume.wgsl` runs over the buffer prefix gated by
/// `FogParams.point_count`, so the CPU may upload anywhere from 0 to this many
/// records each frame.
pub const MAX_FOG_POINT_LIGHTS: usize = 32;

/// Default raymarch step size in world units. Smaller values increase quality
/// and GPU cost; larger values are faster but produce visible banding.
pub const DEFAULT_FOG_STEP_SIZE: f32 = 0.5;

/// Packed AABB + scattering parameters for a single fog volume. 112 bytes,
/// matches the `FogVolume` struct in fog_volume.wgsl.
///
/// `max_v` (rather than `max`) avoids the WGSL `max` builtin shadowing a
/// member field name.
///
/// `center` and `half_diag` are baked at compile time by the level compiler and
/// actively consumed by the raymarch shader. `inv_half_ext` stores the
/// reciprocal per-axis half-extent (`1 / ((max - min) * 0.5)`) and is live on
/// the ellipsoid path: the shader reads it when `shape_mode > 0.5` to scale
/// `point - center` into the unit-sphere domain. It is ignored on the legacy
/// radial path (`shape_mode == 0.0`). `shape_mode` is a discriminant flag
/// (0.0 = legacy radial sphere/capsule fade against `half_diag`, 1.0 =
/// ellipsoid using `inv_half_ext`).
///
/// Field order pairs each `vec3<f32>` with a trailing scalar so WGSL's 16-byte
/// vec3 alignment slots fill naturally without internal padding holes. Seven
/// 16-byte blocks: (1) `min`, `density`; (2) `max_v`, `edge_softness`;
/// (3) `center`, `half_diag`; (4) `inv_half_ext`, `shape_mode`;
/// (5) `tint`, `saturation`; (6) `radial_falloff`, `glow`, `plane_offset`,
/// `plane_count`; (7) `min_brightness`, `light_range`, `anisotropy`,
/// `ambient_scatter`.
///
/// Any PRL file compiled before all fields were present must be recompiled.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct FogVolume {
    pub min: [f32; 3],
    pub density: f32,
    pub max_v: [f32; 3],
    /// World-unit fade band for primitive (plane-bounded) volumes. The shader
    /// scales density by `saturate(min_signed_dist / edge_softness)`. `<= 0`
    /// produces a hard cutoff. Semantic entities (zero planes) ignore this
    /// field and use `radial_falloff` instead.
    pub edge_softness: f32,
    pub center: [f32; 3],
    pub half_diag: f32,
    /// Reciprocal per-axis half-extent (`1 / ((max - min) * 0.5)`). Live on
    /// the ellipsoid path (`shape_mode == 1.0`); ignored on the legacy radial
    /// path (`shape_mode == 0.0`).
    pub inv_half_ext: [f32; 3],
    /// Shape discriminant: `0.0` = legacy radial (sphere/capsule fade against
    /// `half_diag`), `1.0` = ellipsoid (uses `inv_half_ext`). The shader
    /// compares with `> 0.5` to avoid float precision issues.
    pub shape_mode: f32,
    /// Scatter tint multiplier. `[1, 1, 1]` = no tint (default). Applied to
    /// the scatter result after saturation.
    pub tint: [f32; 3],
    /// Scatter saturation: 0 = greyscale, 1 = natural (default), >1 = boosted.
    pub saturation: f32,
    pub radial_falloff: f32,
    pub glow: f32,
    /// Index of this volume's first plane in the global `fog_planes` storage
    /// buffer. Cursor rebuilt each frame as the active set is packed; source
    /// PRL index is irrelevant.
    pub plane_offset: u32,
    /// Number of planes that bound this volume. Zero means the volume is a
    /// semantic entity (AABB-only membership + radial fade).
    pub plane_count: u32,
    pub min_brightness: f32,
    pub light_range: f32,
    /// Henyey-Greenstein anisotropy `g`, translated by the compiler from the
    /// authored `scatter_bias` KVP. `0.0` selects the isotropic world-up SH read.
    pub anisotropy: f32,
    /// Static SH ambient scatter scale. `1.0` preserves existing maps.
    pub ambient_scatter: f32,
}

/// Per-frame spot-light record consumed by the fog raymarch. 48 bytes;
/// layout mirrors `FogSpotLight` in fog_volume.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct FogSpotLight {
    pub position: [f32; 3],
    /// Spot shadow map slot index. `u32::MAX` marks the entry as unused.
    pub slot: u32,
    /// Unit aim direction (light → target).
    pub direction: [f32; 3],
    pub cos_outer: f32,
    /// Pre-multiplied color × intensity.
    pub color: [f32; 3],
    pub range: f32,
}

/// Per-frame point-light record marched by the fog shader. Uploaded by
/// `FogVolumeBridge::update_points`; pre-culled against fog volume AABBs
/// before upload so only nearby lights reach the GPU.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct FogPointLight {
    pub position: [f32; 3],
    pub range: f32,
    /// Pre-multiplied by intensity.
    pub color: [f32; 3],
    pub _pad: f32,
}

/// Per-frame fog uniform. Layout matches `FogParams` in fog_volume.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct FogParams {
    pub inv_view_proj: [[f32; 4]; 4],
    pub camera_position: [f32; 3],
    pub step_size: f32,
    /// Number of dense-packed `FogVolume` records the shader iterates this
    /// frame — the result of OR-ing portal-visible cells' per-cell volume
    /// bitmasks and counting set bits. Renamed from `volume_count` when
    /// portal-based fog culling repurposed the field for the active-set
    /// count rather than the static loaded count.
    pub active_count: u32,
    pub near_clip: f32,
    pub far_clip: f32,
    /// Number of valid `FogPointLight` records uploaded this frame. The shader
    /// loops over this prefix rather than `arrayLength(&fog_points)` so stale
    /// records from a previous frame don't ghost into the current pass when
    /// the point-light count drops to zero.
    pub point_count: u32,
    /// Number of valid `FogSpotLight` records uploaded this frame. Same
    /// reasoning as `point_count`: the spots buffer is sized for
    /// `SHADOW_POOL_SIZE` capacity and never shrinks, so the shader must
    /// bound its loop on a CPU-tracked count instead of
    /// `arrayLength(&fog_spots)`.
    pub spot_count: u32,
    /// Monotonic frame counter (wrapping). Drives the per-frame temporal offset
    /// of the raymarch ray-start jitter so the stratification pattern animates
    /// instead of freezing into a static noise texture. Repurposed from one of
    /// the former `_pad2` slots — size is unchanged.
    pub frame_index: u32,
    /// Explicit padding to keep the `frame_index` 16-byte block full before the
    /// trailing `mat4x4`. The prefix through `frame_index` is 100 bytes; with a
    /// `mat4x4<f32>` (16-aligned) appended, WGSL rounds the prefix to 112 and
    /// then `prev_view_proj` occupies bytes 112..176. The CPU struct must match.
    pub _pad2: [u32; 2],
    /// Previous frame's view-projection matrix. Used by the temporal-resolve
    /// pass to reproject the current world position into the prior frame's
    /// screen UV and sample the accumulation history there. Appended at the END
    /// so the composite's prefix-only `FogParams` (ending at `far_clip`) is
    /// unaffected. See rendering_pipeline.md §7.5.
    pub prev_view_proj: [[f32; 4]; 4],
}

pub const FOG_VOLUME_SIZE: usize = std::mem::size_of::<FogVolume>();
pub const FOG_SPOT_LIGHT_SIZE: usize = std::mem::size_of::<FogSpotLight>();
pub const FOG_POINT_LIGHT_SIZE: usize = std::mem::size_of::<FogPointLight>();
pub const FOG_PARAMS_SIZE: usize = std::mem::size_of::<FogParams>();

// Compile-time guards against accidental layout drift.
const _: () = assert!(FOG_VOLUME_SIZE == 112);
const _: () = assert!(FOG_SPOT_LIGHT_SIZE == 48);
const _: () = assert!(FOG_POINT_LIGHT_SIZE == 32);
const _: () = assert!(FOG_PARAMS_SIZE == 176);

pub fn pack_fog_volumes(volumes: &[FogVolume]) -> &[u8] {
    bytemuck::cast_slice(volumes)
}

pub fn pack_fog_spot_lights(lights: &[FogSpotLight]) -> &[u8] {
    bytemuck::cast_slice(lights)
}

pub fn pack_fog_point_lights(lights: &[FogPointLight]) -> &[u8] {
    bytemuck::cast_slice(lights)
}

/// Inputs to [`pack_fog_params`]. Decouples callers from the GPU struct
/// layout (`FogParams`): this struct is the stable call-shape contract, while
/// `FogParams` — the GPU-side layout with explicit padding — can evolve
/// independently.
pub struct FogParamsInput {
    pub inv_view_proj: Mat4,
    pub camera_position: Vec3,
    pub step_size: f32,
    pub active_count: u32,
    pub near_clip: f32,
    pub far_clip: f32,
    pub point_count: u32,
    pub spot_count: u32,
    pub frame_index: u32,
    pub prev_view_proj: Mat4,
}

/// Build the per-frame fog uniform. Takes a [`FogParamsInput`] rather than a
/// `FogParams` directly so callers don't depend on the GPU struct layout —
/// `FogParamsInput` is the stable call-shape contract; `FogParams` carries the
/// GPU-aligned layout (with explicit padding) and can drift independently.
/// Callers cast the returned struct to bytes via `bytemuck::bytes_of` at the
/// upload site, avoiding a per-frame `Vec<u8>` allocation.
pub fn pack_fog_params(input: FogParamsInput) -> FogParams {
    FogParams {
        inv_view_proj: input.inv_view_proj.to_cols_array_2d(),
        camera_position: input.camera_position.to_array(),
        step_size: input.step_size,
        active_count: input.active_count,
        near_clip: input.near_clip,
        far_clip: input.far_clip,
        point_count: input.point_count,
        spot_count: input.spot_count,
        frame_index: input.frame_index,
        _pad2: [0; 2],
        prev_view_proj: input.prev_view_proj.to_cols_array_2d(),
    }
}

/// Clamp the worldspawn `fog_pixel_scale` to a supported range. `0` is the
/// "unset" sentinel and falls back to the default of 4× downscaling.
pub fn clamp_fog_pixel_scale(scale: u32) -> u32 {
    match scale {
        0 => 4,
        n => n.clamp(1, 8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_fog_pixel_scale_zero_returns_default() {
        assert_eq!(clamp_fog_pixel_scale(0), 4);
    }

    #[test]
    fn clamp_fog_pixel_scale_one_passes_through() {
        assert_eq!(clamp_fog_pixel_scale(1), 1);
    }

    #[test]
    fn clamp_fog_pixel_scale_max_passes_through() {
        assert_eq!(clamp_fog_pixel_scale(8), 8);
    }

    #[test]
    fn clamp_fog_pixel_scale_above_max_clamps_to_eight() {
        assert_eq!(clamp_fog_pixel_scale(9), 8);
    }

    #[test]
    fn pack_fog_volumes_round_trips_all_baked_fields() {
        // Byte offsets: min(0) density(12) max_v(16) edge_softness(28)
        // center(32) half_diag(44) inv_half_ext(48) shape_mode(60)
        // tint(64) saturation(76) radial_falloff(80) glow(84)
        // plane_offset(88) plane_count(92) min_brightness(96)
        // light_range(100) anisotropy(104) ambient_scatter(108). Total: 112 bytes.
        // Spot-checking key baked fields catches silent layout drift between Rust and WGSL.
        let v = FogVolume {
            min: [1.0, 2.0, 3.0],
            density: 0.75,
            max_v: [4.0, 5.0, 6.0],
            edge_softness: 0.25,
            center: [2.5, 3.5, 4.5],
            half_diag: 2.598_076,
            inv_half_ext: [0.666_666_7, 0.666_666_7, 0.666_666_7],
            shape_mode: 0.0,
            tint: [1.0, 0.5, 0.25],
            saturation: 1.5,
            radial_falloff: 0.0,
            glow: 0.5,
            plane_offset: 0,
            plane_count: 0,
            min_brightness: 0.125,
            light_range: 2.0,
            anisotropy: 0.45,
            ambient_scatter: 0.25,
        };
        let volumes = [v];
        let bytes = pack_fog_volumes(&volumes);
        assert_eq!(bytes.len(), FOG_VOLUME_SIZE);
        assert_eq!(FOG_VOLUME_SIZE, 112);

        let density = f32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let edge_softness = f32::from_le_bytes(bytes[28..32].try_into().unwrap());
        let center_x = f32::from_le_bytes(bytes[32..36].try_into().unwrap());
        let half_diag = f32::from_le_bytes(bytes[44..48].try_into().unwrap());
        let inv_hx = f32::from_le_bytes(bytes[48..52].try_into().unwrap());
        let shape_mode = f32::from_le_bytes(bytes[60..64].try_into().unwrap());
        let tint_r = f32::from_le_bytes(bytes[64..68].try_into().unwrap());
        let tint_g = f32::from_le_bytes(bytes[68..72].try_into().unwrap());
        let tint_b = f32::from_le_bytes(bytes[72..76].try_into().unwrap());
        let saturation = f32::from_le_bytes(bytes[76..80].try_into().unwrap());
        let radial_falloff = f32::from_le_bytes(bytes[80..84].try_into().unwrap());
        let glow = f32::from_le_bytes(bytes[84..88].try_into().unwrap());
        let plane_offset = u32::from_le_bytes(bytes[88..92].try_into().unwrap());
        let plane_count = u32::from_le_bytes(bytes[92..96].try_into().unwrap());
        let min_brightness = f32::from_le_bytes(bytes[96..100].try_into().unwrap());
        let light_range = f32::from_le_bytes(bytes[100..104].try_into().unwrap());
        let anisotropy = f32::from_le_bytes(bytes[104..108].try_into().unwrap());
        let ambient_scatter = f32::from_le_bytes(bytes[108..112].try_into().unwrap());
        assert_eq!(density, 0.75);
        assert_eq!(edge_softness, 0.25);
        assert_eq!(center_x, 2.5);
        assert!((half_diag - 2.598_076).abs() < 1e-5);
        assert!((inv_hx - 0.666_666_7).abs() < 1e-5);
        assert_eq!(shape_mode, 0.0);
        assert_eq!(tint_r, 1.0);
        assert_eq!(tint_g, 0.5);
        assert_eq!(tint_b, 0.25);
        assert_eq!(saturation, 1.5);
        assert_eq!(radial_falloff, 0.0);
        assert_eq!(glow, 0.5);
        assert_eq!(plane_offset, 0);
        assert_eq!(plane_count, 0);
        assert_eq!(min_brightness, 0.125);
        assert_eq!(light_range, 2.0);
        assert_eq!(anisotropy, 0.45);
        assert_eq!(ambient_scatter, 0.25);
    }
}
