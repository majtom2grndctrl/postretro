// SDF atlas metadata packing and brick geometry helpers.
// See: context/lib/rendering_pipeline.md §4

/// Width (in bytes) of one `SdfAtlasMeta` uniform record on the GPU.
/// Two vec4 slots for world_min+voxel_size and world_max+brick_size, plus
/// two uvec4 slots for grid_dims+surface_brick_count and
/// atlas_bricks_per_axis+present_flag. 64 bytes total — std140-aligned.
///
/// WGSL layout (must match the consumer shader once Task 4 lands):
///   0..12   world_min            (vec3<f32>)
///   12..16  voxel_size_m         (f32)
///   16..28  world_max            (vec3<f32>)
///   28..32  brick_size_voxels    (u32, reinterpreted as f32 if convenient)
///   32..44  grid_dims            (vec3<u32>)
///   44..48  surface_brick_count  (u32)
///   48..60  atlas_bricks_per_axis (vec3<u32>)
///   60..64  present              (u32, 0 = no atlas, 1 = present)
pub const SDF_ATLAS_META_SIZE: usize = 64;

pub const SDF_I16_QUANT_STEPS_PER_VOXEL: f32 = 256.0;

/// Pack the SDF meta uniform. See the `SDF_ATLAS_META_SIZE` doc for the
/// 64-byte field layout. Kept as a free function so it can be unit-tested
/// without a wgpu device.
#[allow(clippy::too_many_arguments)]
pub fn build_meta_bytes(
    world_min: [f32; 3],
    world_max: [f32; 3],
    voxel_size_m: f32,
    brick_size_voxels: u32,
    grid_dims: [u32; 3],
    atlas_bricks_per_axis: [u32; 3],
    surface_brick_count: u32,
    present: bool,
) -> [u8; SDF_ATLAS_META_SIZE] {
    let mut bytes = [0u8; SDF_ATLAS_META_SIZE];
    bytes[0..4].copy_from_slice(&world_min[0].to_le_bytes());
    bytes[4..8].copy_from_slice(&world_min[1].to_le_bytes());
    bytes[8..12].copy_from_slice(&world_min[2].to_le_bytes());
    bytes[12..16].copy_from_slice(&voxel_size_m.to_le_bytes());
    bytes[16..20].copy_from_slice(&world_max[0].to_le_bytes());
    bytes[20..24].copy_from_slice(&world_max[1].to_le_bytes());
    bytes[24..28].copy_from_slice(&world_max[2].to_le_bytes());
    bytes[28..32].copy_from_slice(&brick_size_voxels.to_le_bytes());
    bytes[32..36].copy_from_slice(&grid_dims[0].to_le_bytes());
    bytes[36..40].copy_from_slice(&grid_dims[1].to_le_bytes());
    bytes[40..44].copy_from_slice(&grid_dims[2].to_le_bytes());
    bytes[44..48].copy_from_slice(&surface_brick_count.to_le_bytes());
    bytes[48..52].copy_from_slice(&atlas_bricks_per_axis[0].to_le_bytes());
    bytes[52..56].copy_from_slice(&atlas_bricks_per_axis[1].to_le_bytes());
    bytes[56..60].copy_from_slice(&atlas_bricks_per_axis[2].to_le_bytes());
    let present_flag: u32 = if present { 1 } else { 0 };
    bytes[60..64].copy_from_slice(&present_flag.to_le_bytes());
    bytes
}

pub fn scatter_bricks_to_atlas(
    atlas_i16: &[i16],
    surface_brick_count: u32,
    brick_size: u32,
    atlas_bricks_per_axis: [u32; 3],
) -> Vec<u16> {
    let edge = (brick_size + 2) as usize;
    let voxels_per_brick = edge * edge * edge;
    let apx = atlas_bricks_per_axis[0].max(1) as usize;
    let apy = atlas_bricks_per_axis[1].max(1) as usize;
    let apz = atlas_bricks_per_axis[2].max(1) as usize;

    let atlas_w = apx * edge;
    let atlas_h = apy * edge;
    let atlas_d = apz * edge;
    let mut out = vec![0u16; atlas_w * atlas_h * atlas_d];

    let slots = (surface_brick_count as usize).min(atlas_i16.len() / voxels_per_brick.max(1));
    for slot in 0..slots {
        let bx = slot % apx;
        let by = (slot / apx) % apy;
        let bz = slot / (apx * apy);
        if bz >= apz {
            break;
        }
        let base_x = bx * edge;
        let base_y = by * edge;
        let base_z = bz * edge;

        let brick = &atlas_i16[slot * voxels_per_brick..(slot + 1) * voxels_per_brick];
        for sz in 0..edge {
            for sy in 0..edge {
                for sx in 0..edge {
                    let src = sz * edge * edge + sy * edge + sx;
                    let dst =
                        (base_z + sz) * atlas_w * atlas_h + (base_y + sy) * atlas_w + (base_x + sx);
                    out[dst] = crate::sh_volume::f32_to_f16_bits(brick[src] as f32);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scatter_places_each_brick_as_contiguous_subcube() {
        let brick_size = 1u32;
        let edge = (brick_size + 2) as usize;
        let voxels_per_brick = edge * edge * edge;
        let apx = 2usize;
        let mut atlas_i16 = vec![0i16; 2 * voxels_per_brick];
        for s in 0..voxels_per_brick {
            atlas_i16[s] = s as i16;
            atlas_i16[voxels_per_brick + s] = (s + 100) as i16;
        }

        let out = scatter_bricks_to_atlas(&atlas_i16, 2, brick_size, [apx as u32, 1, 1]);
        let atlas_w = apx * edge;
        let atlas_h = edge;

        for s in 0..voxels_per_brick {
            let sx = s % edge;
            let sy = (s / edge) % edge;
            let sz = s / (edge * edge);

            let dst0 = sz * atlas_w * atlas_h + sy * atlas_w + sx;
            assert_eq!(out[dst0], crate::sh_volume::f32_to_f16_bits(s as f32));

            let dst1 = sz * atlas_w * atlas_h + sy * atlas_w + (edge + sx);
            assert_eq!(
                out[dst1],
                crate::sh_volume::f32_to_f16_bits((s + 100) as f32)
            );
        }
    }

    #[test]
    fn meta_bytes_encode_world_bounds_and_present_flag() {
        let bytes = build_meta_bytes(
            [-8.0, -2.0, -8.0],
            [8.0, 6.0, 8.0],
            0.0625,
            4,
            [2, 3, 4],
            [2, 1, 1],
            5,
            true,
        );

        assert_eq!(bytes.len(), SDF_ATLAS_META_SIZE);
        assert_eq!(f32::from_le_bytes(bytes[0..4].try_into().unwrap()), -8.0);
        assert_eq!(
            f32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            0.0625
        );
        assert_eq!(f32::from_le_bytes(bytes[20..24].try_into().unwrap()), 6.0);
        assert_eq!(u32::from_le_bytes(bytes[28..32].try_into().unwrap()), 4);
        assert_eq!(u32::from_le_bytes(bytes[36..40].try_into().unwrap()), 3);
        assert_eq!(u32::from_le_bytes(bytes[44..48].try_into().unwrap()), 5);
        assert_eq!(u32::from_le_bytes(bytes[48..52].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(bytes[60..64].try_into().unwrap()), 1);
    }

    #[test]
    fn meta_bytes_present_flag_zero_when_absent() {
        let bytes = build_meta_bytes([0.0; 3], [0.0; 3], 0.0, 0, [0; 3], [0; 3], 0, false);
        assert_eq!(u32::from_le_bytes(bytes[60..64].try_into().unwrap()), 0);
    }
}
