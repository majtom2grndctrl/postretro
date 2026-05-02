// CPU-side GPU types and packing for the volumetric fog pass.
// See: context/lib/rendering_pipeline.md §7.5

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

/// Maximum number of fog volumes the raymarch shader iterates per frame.
// Mirrors `postretro_level_format::fog_volumes::MAX_FOG_VOLUMES` — keep in sync.
pub const MAX_FOG_VOLUMES: usize = 16;

/// Maximum number of fog point lights (currently unused by the shader, but
/// reserved so the CPU side can stage point-light beams alongside spots).
pub const MAX_FOG_POINT_LIGHTS: usize = 32;

/// Default raymarch step size in world units. Smaller values increase quality
/// and GPU cost; larger values are faster but produce visible banding.
pub const DEFAULT_FOG_STEP_SIZE: f32 = 0.5;

/// Packed AABB + scattering parameters for a single fog volume. 64 bytes,
/// matches the `FogVolume` struct in fog_volume.wgsl.
///
/// `max_v` (rather than `max`) avoids the WGSL `max` builtin shadowing a
/// member field name.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct FogVolume {
    pub min: [f32; 3],
    pub density: f32,
    pub max_v: [f32; 3],
    pub falloff: f32,
    pub color: [f32; 3],
    pub scatter: f32,
    pub height_gradient: f32,
    pub radial_falloff: f32,
    pub _pad0: f32,
    pub _pad1: f32,
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
    pub volume_count: u32,
    pub near_clip: f32,
    pub far_clip: f32,
    pub _pad: u32,
}

pub const FOG_VOLUME_SIZE: usize = std::mem::size_of::<FogVolume>();
pub const FOG_SPOT_LIGHT_SIZE: usize = std::mem::size_of::<FogSpotLight>();
pub const FOG_POINT_LIGHT_SIZE: usize = std::mem::size_of::<FogPointLight>();
pub const FOG_PARAMS_SIZE: usize = std::mem::size_of::<FogParams>();

// Compile-time guards against accidental layout drift.
const _: () = assert!(FOG_VOLUME_SIZE == 64);
const _: () = assert!(FOG_SPOT_LIGHT_SIZE == 48);
const _: () = assert!(FOG_POINT_LIGHT_SIZE == 32);
const _: () = assert!(FOG_PARAMS_SIZE == 96);

pub fn pack_fog_volumes(volumes: &[FogVolume]) -> Vec<u8> {
    bytemuck::cast_slice(volumes).to_vec()
}

pub fn pack_fog_spot_lights(lights: &[FogSpotLight]) -> Vec<u8> {
    bytemuck::cast_slice(lights).to_vec()
}

pub fn pack_fog_point_lights(lights: &[FogPointLight]) -> Vec<u8> {
    bytemuck::cast_slice(lights).to_vec()
}

/// Pack the per-frame fog uniform from its constituent values. Kept as
/// individual arguments so the renderer doesn't have to maintain a separate
/// struct just to call this — the GPU layout is what's stable, not the call
/// shape.
pub fn pack_fog_params(
    inv_view_proj: Mat4,
    camera_position: Vec3,
    step_size: f32,
    volume_count: u32,
    near_clip: f32,
    far_clip: f32,
) -> Vec<u8> {
    let params = FogParams {
        inv_view_proj: inv_view_proj.to_cols_array_2d(),
        camera_position: camera_position.to_array(),
        step_size,
        volume_count,
        near_clip,
        far_clip,
        _pad: 0,
    };
    bytemuck::bytes_of(&params).to_vec()
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
    fn pack_fog_volumes_round_trips_density_and_falloff() {
        // density at byte offset 12, falloff at byte offset 28 — these are
        // the float32 fields most likely to silently drift if a layout
        // change forgets the WGSL counterpart.
        let v = FogVolume {
            min: [1.0, 2.0, 3.0],
            density: 0.75,
            max_v: [4.0, 5.0, 6.0],
            falloff: 0.25,
            color: [0.1, 0.2, 0.3],
            scatter: 0.5,
            height_gradient: 0.0,
            radial_falloff: 0.0,
            _pad0: 0.0,
            _pad1: 0.0,
        };
        let bytes = pack_fog_volumes(&[v]);
        assert_eq!(bytes.len(), FOG_VOLUME_SIZE);

        let density = f32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let falloff = f32::from_le_bytes(bytes[28..32].try_into().unwrap());
        assert_eq!(density, 0.75);
        assert_eq!(falloff, 0.25);
    }
}
