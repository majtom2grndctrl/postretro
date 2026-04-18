// SDF atlas section (ID 22): brick-indexed sparse signed distance field for
// soft-shadow sphere tracing (sub-plan 8).
//
// See: context/plans/in-progress/lighting-foundation/8-sdf-shadows.md

use crate::FormatError;

/// Sentinel brick-slot value: no geometry within reach of this brick.
/// The runtime sampler returns a large positive distance for this slot.
pub const BRICK_SLOT_EMPTY: u32 = u32::MAX;
/// Sentinel brick-slot value: this brick is fully inside solid geometry.
/// The runtime sampler returns a large negative distance for this slot.
pub const BRICK_SLOT_INTERIOR: u32 = u32::MAX - 1;

/// The first slot index usable for real surface bricks.
pub const BRICK_SLOT_FIRST_SURFACE: u32 = 0;

/// Fallback coarse-distance magnitude (in meters) when the baker finds no
/// triangles within its max BVH expansion. Arbitrary "large" value chosen to
/// match the shader's `SDF_LARGE_POS` sentinel so the sphere tracer treats
/// these bricks as far-from-geometry.
///
/// The shader cannot import this constant directly (WGSL has no cross-module
/// imports), so `forward.wgsl::SDF_LARGE_POS` must be updated in lockstep if
/// this value ever changes.
pub const SDF_FALLBACK_DISTANCE_M: f32 = 1.0e6;

/// SDF atlas section (ID 22).
///
/// On-disk layout (all little-endian):
///
/// ```text
///   Header (48 bytes):
///     f32 × 3  world_min             (world-space AABB min corner, meters)
///     f32 × 3  world_max             (world-space AABB max corner, meters)
///     f32      voxel_size_m          (meters per voxel)
///     u32      brick_size_voxels     (voxels per brick edge; brick volume = this³)
///     u32      grid_x                (top-level brick grid width)
///     u32      grid_y                (top-level brick grid height)
///     u32      grid_z                (top-level brick grid depth)
///     u32      surface_brick_count   (number of stored surface bricks)
///
///   Top-level index (grid_x * grid_y * grid_z * 4 bytes):
///     u32 × (grid_x * grid_y * grid_z)  brick_slot per cell
///       BRICK_SLOT_EMPTY    → no surface
///       BRICK_SLOT_INTERIOR → inside solid
///       other               → index into the atlas
///
///   Atlas data (surface_brick_count * brick_volume * 2 bytes):
///     i16 × (surface_brick_count * brick_volume)  signed distances
///       quantized: 1 unit = voxel_size_m / 256.0 meters (~0.31 mm at 0.08 m voxels)
///
///   Coarse distances (grid_x * grid_y * grid_z * 4 bytes):
///     f32 × (grid_x * grid_y * grid_z)  signed distance from brick *center*
///       to nearest surface triangle, in meters. Sampled trilinearly by the
///       runtime to provide valid SDF coverage in EMPTY/INTERIOR regions of
///       the grid, where the sparse per-voxel atlas has no data. Without this
///       the sphere tracer would step the full light distance in one jump when
///       starting in a non-SURFACE brick and miss all thin-occluder shadows.
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct SdfAtlasSection {
    /// World-space AABB minimum (meters, engine coordinates).
    pub world_min: [f32; 3],
    /// World-space AABB maximum (meters, engine coordinates).
    pub world_max: [f32; 3],
    /// Voxel edge length in meters.
    pub voxel_size_m: f32,
    /// Number of voxels per brick edge. Brick volume = brick_size_voxels³.
    pub brick_size_voxels: u32,
    /// Brick grid dimensions (x, y, z).
    pub grid_dims: [u32; 3],
    /// Top-level index: one slot per brick cell.
    /// Length = grid_dims[0] * grid_dims[1] * grid_dims[2].
    pub top_level: Vec<u32>,
    /// Atlas body: surface_brick_count * brick_volume * i16 values.
    /// Signed distance in quantized units (voxel_size_m / 256.0 per unit).
    pub atlas: Vec<i16>,
    /// Coarse SDF: one f32 per brick, signed distance from brick center to
    /// nearest surface triangle (meters). Length = grid_dims.x*y*z, same as
    /// `top_level`. Uploaded as a trilinearly-sampled 3D texture so the
    /// sphere tracer has a valid distance field everywhere in the grid — not
    /// just inside SURFACE bricks.
    pub coarse_distances: Vec<f32>,
}

/// Byte size of the fixed header.
/// 3×f32 world_min + 3×f32 world_max + f32 voxel_size_m + u32 brick_size_voxels
/// + 3×u32 grid_dims + u32 surface_brick_count = 12 × 4 = 48 bytes.
const HEADER_SIZE: usize = 48;

impl SdfAtlasSection {
    /// Serialize to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        // Invariant: coarse_distances has exactly one entry per top-level cell.
        // Enforced in debug builds so a mis-constructed section is caught by
        // tests rather than silently producing a truncated on-disk section.
        debug_assert_eq!(
            self.coarse_distances.len(),
            self.top_level.len(),
            "SdfAtlasSection invariant: coarse_distances.len() must match top_level.len() \
             (got coarse={}, top={})",
            self.coarse_distances.len(),
            self.top_level.len(),
        );
        let top_len = self.top_level.len();
        let atlas_len = self.atlas.len();
        let coarse_len = self.coarse_distances.len();
        let size = HEADER_SIZE + top_len * 4 + atlas_len * 2 + coarse_len * 4;
        let mut out = Vec::with_capacity(size);

        // Header
        for &v in &self.world_min {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for &v in &self.world_max {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&self.voxel_size_m.to_le_bytes());
        out.extend_from_slice(&self.brick_size_voxels.to_le_bytes());
        out.extend_from_slice(&self.grid_dims[0].to_le_bytes());
        out.extend_from_slice(&self.grid_dims[1].to_le_bytes());
        out.extend_from_slice(&self.grid_dims[2].to_le_bytes());

        let surface_count = self.surface_brick_count();
        out.extend_from_slice(&surface_count.to_le_bytes());

        // Top-level index
        for &slot in &self.top_level {
            out.extend_from_slice(&slot.to_le_bytes());
        }

        // Atlas
        for &v in &self.atlas {
            out.extend_from_slice(&v.to_le_bytes());
        }

        // Coarse distances
        for &v in &self.coarse_distances {
            out.extend_from_slice(&v.to_le_bytes());
        }

        out
    }

    /// Deserialize from bytes.
    pub fn from_bytes(data: &[u8]) -> Result<Self, FormatError> {
        if data.len() < HEADER_SIZE {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "SdfAtlas header truncated: need {HEADER_SIZE} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let mut off = 0usize;

        let read_f32 = |data: &[u8], o: &mut usize| -> f32 {
            let v = f32::from_le_bytes([data[*o], data[*o + 1], data[*o + 2], data[*o + 3]]);
            *o += 4;
            v
        };
        let read_u32 = |data: &[u8], o: &mut usize| -> u32 {
            let v = u32::from_le_bytes([data[*o], data[*o + 1], data[*o + 2], data[*o + 3]]);
            *o += 4;
            v
        };

        let world_min = [
            read_f32(data, &mut off),
            read_f32(data, &mut off),
            read_f32(data, &mut off),
        ];
        let world_max = [
            read_f32(data, &mut off),
            read_f32(data, &mut off),
            read_f32(data, &mut off),
        ];
        let voxel_size_m = read_f32(data, &mut off);
        let brick_size_voxels = read_u32(data, &mut off);
        let grid_x = read_u32(data, &mut off);
        let grid_y = read_u32(data, &mut off);
        let grid_z = read_u32(data, &mut off);
        let surface_brick_count = read_u32(data, &mut off);

        let top_count = (grid_x as usize) * (grid_y as usize) * (grid_z as usize);
        let needed_top = top_count * 4;
        let brick_vol = (brick_size_voxels as usize).saturating_pow(3);
        let needed_atlas = (surface_brick_count as usize) * brick_vol * 2;
        let needed_coarse = top_count * 4;
        let needed_total = HEADER_SIZE + needed_top + needed_atlas + needed_coarse;

        if data.len() < needed_total {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "SdfAtlas data truncated: need {needed_total} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let mut top_level = Vec::with_capacity(top_count);
        for _ in 0..top_count {
            top_level.push(read_u32(data, &mut off));
        }

        let atlas_count = (surface_brick_count as usize) * brick_vol;
        let mut atlas = Vec::with_capacity(atlas_count);
        for _ in 0..atlas_count {
            let v = i16::from_le_bytes([data[off], data[off + 1]]);
            off += 2;
            atlas.push(v);
        }

        let mut coarse_distances = Vec::with_capacity(top_count);
        for _ in 0..top_count {
            coarse_distances.push(read_f32(data, &mut off));
        }

        Ok(SdfAtlasSection {
            world_min,
            world_max,
            voxel_size_m,
            brick_size_voxels,
            grid_dims: [grid_x, grid_y, grid_z],
            top_level,
            atlas,
            coarse_distances,
        })
    }

    /// Number of surface bricks stored in the atlas.
    pub fn surface_brick_count(&self) -> u32 {
        let brick_vol = (self.brick_size_voxels as usize).saturating_pow(3);
        if brick_vol == 0 {
            return 0;
        }
        (self.atlas.len() / brick_vol) as u32
    }

    /// Brick volume in voxels (brick_size_voxels³).
    pub fn brick_volume(&self) -> usize {
        (self.brick_size_voxels as usize).saturating_pow(3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty_atlas() {
        let sec = SdfAtlasSection {
            world_min: [-1.0, -1.0, -1.0],
            world_max: [1.0, 1.0, 1.0],
            voxel_size_m: 0.08,
            brick_size_voxels: 8,
            grid_dims: [2, 2, 2],
            top_level: vec![BRICK_SLOT_EMPTY; 8],
            atlas: Vec::new(),
            coarse_distances: vec![1.0; 8],
        };
        let bytes = sec.to_bytes();
        let restored = SdfAtlasSection::from_bytes(&bytes).unwrap();
        assert_eq!(sec, restored);
    }

    #[test]
    fn round_trip_with_surface_brick() {
        let brick_vol = 8_usize.pow(3); // 512
        let atlas: Vec<i16> = (0..brick_vol as i16).collect();
        let sec = SdfAtlasSection {
            world_min: [0.0, 0.0, 0.0],
            world_max: [1.0, 1.0, 1.0],
            voxel_size_m: 0.08,
            brick_size_voxels: 8,
            grid_dims: [1, 1, 1],
            top_level: vec![0], // slot 0 is the single surface brick
            atlas,
            coarse_distances: vec![0.0],
        };
        let bytes = sec.to_bytes();
        let restored = SdfAtlasSection::from_bytes(&bytes).unwrap();
        assert_eq!(sec, restored);
        assert_eq!(restored.surface_brick_count(), 1);
    }

    #[test]
    fn sentinel_slots_round_trip() {
        let sec = SdfAtlasSection {
            world_min: [0.0; 3],
            world_max: [1.0; 3],
            voxel_size_m: 0.08,
            brick_size_voxels: 8,
            grid_dims: [1, 1, 2],
            top_level: vec![BRICK_SLOT_EMPTY, BRICK_SLOT_INTERIOR],
            atlas: Vec::new(),
            coarse_distances: vec![5.0, -5.0],
        };
        let bytes = sec.to_bytes();
        let restored = SdfAtlasSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored.top_level[0], BRICK_SLOT_EMPTY);
        assert_eq!(restored.top_level[1], BRICK_SLOT_INTERIOR);
        assert_eq!(restored.coarse_distances, vec![5.0, -5.0]);
    }

    #[test]
    #[should_panic(expected = "coarse_distances.len() must match top_level.len()")]
    #[cfg(debug_assertions)]
    fn to_bytes_panics_on_coarse_length_mismatch() {
        let sec = SdfAtlasSection {
            world_min: [0.0; 3],
            world_max: [1.0; 3],
            voxel_size_m: 0.08,
            brick_size_voxels: 8,
            grid_dims: [1, 1, 2],
            top_level: vec![BRICK_SLOT_EMPTY, BRICK_SLOT_INTERIOR],
            atlas: Vec::new(),
            // Wrong length: 1 instead of 2.
            coarse_distances: vec![0.0],
        };
        let _ = sec.to_bytes();
    }

    #[test]
    fn from_bytes_truncated_header_returns_error() {
        let data = vec![0u8; HEADER_SIZE - 1];
        assert!(SdfAtlasSection::from_bytes(&data).is_err());
    }
}
