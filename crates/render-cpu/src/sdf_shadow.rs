// SDF shadow tuning, light packing, and pass parameter bytes.
// See: context/lib/rendering_pipeline.md §4

use glam::Mat4;

pub const DEFAULT_MAX_MARCH_STEPS: u32 = 64;
pub const DEFAULT_OPEN_SPACE_SKIP_THRESHOLD: f32 = 8.0;

/// Penumbra sharpness (larger = harder shadow). The trace's soft term models
/// an area light of radius `distance/k`, CAPPED at one SDF voxel
/// (`sdf_shadow.wgsl`, `cone_scale`): uncapped, the virtual disk grows with
/// receiver distance until a light mounted ~a voxel under a ceiling darkens
/// every distant receiver in its room. Within `k·voxel` meters of the light
/// the cap is inactive and `k` tunes contact penumbras exactly.
pub const DEFAULT_PENUMBRA_K: f32 = 8.0;

/// Self-shadow surface bias, in MULTIPLES of the SDF voxel size (0.5 m default,
/// so the seed below ≈ 0.75 m). The shadow-ray ORIGIN is pushed off the shading
/// surface ALONG THE GEOMETRIC NORMAL by `surface_bias × voxel` before tracing.
/// This is the distance-field self-intersection fix (cf. UE mesh/global DF
/// shadows): the caster is baked into the field, so a ray launched on a lit
/// surface grazes that surface's own ≈0 field near the origin and the penumbra
/// estimate reads it as occlusion — soft round dark blobs on faces that point at
/// the light. A *normal* offset (not an along-ray start) is what fixes the
/// grazing case: when the light is off to the side, an along-ray start skims
/// tangent to the surface and the penumbra estimate collapses to ~0, falsely
/// darkening walls and the bridge top. Lifting along the normal clears the
/// surface field regardless of light direction.
///
/// Seeded at 1.5 voxels (≈ 0.75 m) — a deliberately CONSERVATIVE normal offset.
/// Reasoning: a normal offset risks peter-panning / light-leak (detached
/// shadows) far more than an along-ray start did, because it physically moves
/// the origin away from the contact point, so a large value would let a
/// nearby occluder's shadow detach from the surface. 1.5 voxels is enough to
/// clear the caster's own quantization band (one fine voxel) PLUS the half-res
/// depth-reconstruction error (sub-voxel at this resolution), while staying
/// well under the ~1 m gap at which contact shadows visibly detach. The
/// contact shadow at a block's base survives regardless — it comes from the
/// trace HITTING the block (`d < voxel*0.5`), which fires from the first
/// step, not from the penumbra term.
pub const DEFAULT_SURFACE_BIAS_VOXELS: f32 = 1.5;

/// Size in bytes of the `ShadowPassParams` uniform. Mirrors the WGSL struct
/// in `shaders/sdf_shadow.wgsl`. std140-aligned: vec3<f32>/u32 pairs share
/// 16-byte slots, mat4x4 takes 64.
///
/// Layout:
///   0..64    inv_view_proj             (mat4x4<f32>)
///   64..76   camera_position           (vec3<f32>)
///   76..80   half_res_size_x           (u32)
///   80..84   half_res_size_y           (u32)
///   84..88   max_march_steps           (u32)
///   88..92   open_space_skip_threshold (f32)
///   92..96   penumbra_k                (f32)
///   96..108  sh_grid_origin            (vec3<f32>)
///   108..112 sh_has_volume             (u32)
///   112..124 sh_cell_size              (vec3<f32>)
///   124..128 surface_bias              (f32)
///   128..140 sh_grid_dimensions        (vec3<u32>)
///   140..144 debug_mode                (u32)
pub const SHADOW_PASS_PARAMS_SIZE: usize = 144;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SdfShadowTuning {
    pub max_march_steps: u32,
    pub open_space_skip_threshold: f32,
    pub penumbra_k: f32,
    pub surface_bias: f32,
}

impl Default for SdfShadowTuning {
    fn default() -> Self {
        Self {
            max_march_steps: DEFAULT_MAX_MARCH_STEPS,
            open_space_skip_threshold: DEFAULT_OPEN_SPACE_SKIP_THRESHOLD,
            penumbra_k: DEFAULT_PENUMBRA_K,
            surface_bias: DEFAULT_SURFACE_BIAS_VOXELS,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SdfShadowFrameInputs {
    pub inv_view_proj: Mat4,
    pub camera_position: [f32; 3],
}

#[derive(Debug, Clone, Copy)]
pub struct SdfShadowShGrid {
    pub origin: [f32; 3],
    pub cell_size: [f32; 3],
    pub dimensions: [u32; 3],
    pub has_volume: bool,
}

impl Default for SdfShadowShGrid {
    fn default() -> Self {
        Self {
            origin: [0.0; 3],
            cell_size: [1.0; 3],
            dimensions: [1, 1, 1],
            has_volume: false,
        }
    }
}

/// Pack the `ShadowPassParams` uniform. Mirrors the WGSL struct in
/// `sdf_shadow.wgsl` (see `SHADOW_PASS_PARAMS_SIZE` for the layout table).
/// Kept as a free function so it can be unit-tested without a wgpu device.
pub fn pack_params_bytes(
    frame: SdfShadowFrameInputs,
    half_res: (u32, u32),
    tuning: SdfShadowTuning,
    sh_grid: SdfShadowShGrid,
    // SDF shadow path visualization: 0 = production; 3 = trace-outcome
    // paths; 4 = the reconstructed geometric normal (RGB = normal*0.5+0.5).
    debug_mode: u32,
) -> [u8; SHADOW_PASS_PARAMS_SIZE] {
    let mut bytes = [0u8; SHADOW_PASS_PARAMS_SIZE];
    // 0..64: inv_view_proj (column-major, same convention as the rest of the
    // renderer's mat4 uploads — see `build_uniform_data` in frame_uniforms.rs).
    let cols = frame.inv_view_proj.to_cols_array();
    for (i, val) in cols.iter().enumerate() {
        let off = i * 4;
        bytes[off..off + 4].copy_from_slice(&val.to_ne_bytes());
    }
    // 64..76: camera_position; 76..80: half_res_size_x.
    bytes[64..68].copy_from_slice(&frame.camera_position[0].to_ne_bytes());
    bytes[68..72].copy_from_slice(&frame.camera_position[1].to_ne_bytes());
    bytes[72..76].copy_from_slice(&frame.camera_position[2].to_ne_bytes());
    bytes[76..80].copy_from_slice(&half_res.0.to_ne_bytes());
    // 80..84: half_res_size_y; 84..88: max_march_steps.
    bytes[80..84].copy_from_slice(&half_res.1.to_ne_bytes());
    bytes[84..88].copy_from_slice(&tuning.max_march_steps.to_ne_bytes());
    // 88..92: open_space_skip_threshold; 92..96: penumbra_k.
    bytes[88..92].copy_from_slice(&tuning.open_space_skip_threshold.to_ne_bytes());
    bytes[92..96].copy_from_slice(&tuning.penumbra_k.to_ne_bytes());
    // 96..108: sh_grid_origin; 108..112: sh_has_volume.
    bytes[96..100].copy_from_slice(&sh_grid.origin[0].to_ne_bytes());
    bytes[100..104].copy_from_slice(&sh_grid.origin[1].to_ne_bytes());
    bytes[104..108].copy_from_slice(&sh_grid.origin[2].to_ne_bytes());
    bytes[108..112].copy_from_slice(&(sh_grid.has_volume as u32).to_ne_bytes());
    // 112..124: sh_cell_size; 124..128: surface_bias.
    bytes[112..116].copy_from_slice(&sh_grid.cell_size[0].to_ne_bytes());
    bytes[116..120].copy_from_slice(&sh_grid.cell_size[1].to_ne_bytes());
    bytes[120..124].copy_from_slice(&sh_grid.cell_size[2].to_ne_bytes());
    bytes[124..128].copy_from_slice(&tuning.surface_bias.to_ne_bytes());
    // 128..140: sh_grid_dimensions.
    bytes[128..132].copy_from_slice(&sh_grid.dimensions[0].to_ne_bytes());
    bytes[132..136].copy_from_slice(&sh_grid.dimensions[1].to_ne_bytes());
    bytes[136..140].copy_from_slice(&sh_grid.dimensions[2].to_ne_bytes());
    // 140..144: debug_mode, carried verbatim so the shader branches on the
    // actual mode.
    bytes[140..144].copy_from_slice(&debug_mode.to_ne_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_params_bytes_encodes_camera_half_res_and_tuning() {
        let frame = SdfShadowFrameInputs {
            inv_view_proj: Mat4::IDENTITY,
            camera_position: [1.0, 2.0, 3.0],
        };
        let bytes = pack_params_bytes(
            frame,
            (320, 180),
            SdfShadowTuning {
                max_march_steps: 24,
                open_space_skip_threshold: 4.5,
                penumbra_k: 6.0,
                surface_bias: 1.25,
            },
            SdfShadowShGrid {
                origin: [-4.0, -2.0, -6.0],
                cell_size: [2.0, 3.0, 4.0],
                dimensions: [5, 6, 7],
                has_volume: true,
            },
            9,
        );

        assert_eq!(bytes.len(), SHADOW_PASS_PARAMS_SIZE);
        assert_eq!(f32::from_ne_bytes(bytes[64..68].try_into().unwrap()), 1.0);
        assert_eq!(f32::from_ne_bytes(bytes[68..72].try_into().unwrap()), 2.0);
        assert_eq!(f32::from_ne_bytes(bytes[72..76].try_into().unwrap()), 3.0);
        assert_eq!(u32::from_ne_bytes(bytes[76..80].try_into().unwrap()), 320);
        assert_eq!(u32::from_ne_bytes(bytes[80..84].try_into().unwrap()), 180);
        assert_eq!(u32::from_ne_bytes(bytes[84..88].try_into().unwrap()), 24);
        assert_eq!(f32::from_ne_bytes(bytes[88..92].try_into().unwrap()), 4.5);
        assert_eq!(f32::from_ne_bytes(bytes[92..96].try_into().unwrap()), 6.0);
        assert_eq!(f32::from_ne_bytes(bytes[96..100].try_into().unwrap()), -4.0);
        assert_eq!(u32::from_ne_bytes(bytes[108..112].try_into().unwrap()), 1);
        assert_eq!(
            f32::from_ne_bytes(bytes[124..128].try_into().unwrap()),
            1.25
        );
        assert_eq!(u32::from_ne_bytes(bytes[128..132].try_into().unwrap()), 5);
        assert_eq!(u32::from_ne_bytes(bytes[132..136].try_into().unwrap()), 6);
        assert_eq!(u32::from_ne_bytes(bytes[136..140].try_into().unwrap()), 7);
        assert_eq!(u32::from_ne_bytes(bytes[140..144].try_into().unwrap()), 9);
    }

    #[test]
    fn shadow_pass_params_size_matches_layout_doc() {
        assert_eq!(SHADOW_PASS_PARAMS_SIZE, 144);
        assert_eq!(SHADOW_PASS_PARAMS_SIZE % 16, 0);
    }
}
