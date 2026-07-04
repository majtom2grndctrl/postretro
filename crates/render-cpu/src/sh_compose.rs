// Animated SH and direct SH delta compose sizing and parameter packing.
// See: context/lib/rendering_pipeline.md §4

use postretro_level_format::delta_sh_volumes::{
    AFFINITY_FACTOR, DeltaShVolumesSection, delta_probe_f16_stride,
};
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;

const COMPOSE_GRID_DIMS_SIZE: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComposeStorageFootprint {
    pub delta_subblocks_bytes: usize,
    pub affinity_offsets_bytes: usize,
    pub affinity_lights_bytes: usize,
    pub animation_descriptor_indices_bytes: usize,
}

impl ComposeStorageFootprint {
    pub fn total_bytes(&self) -> usize {
        self.delta_subblocks_bytes
            + self.affinity_offsets_bytes
            + self.affinity_lights_bytes
            + self.animation_descriptor_indices_bytes
    }

    pub fn log(&self) {
        let mib = |b: usize| b as f64 / (1024.0 * 1024.0);
        log::info!(
            "[Renderer] SH compose @group(1) storage footprint: \
             delta_subblocks {:.2} MiB ({} B), affinity_offsets {:.2} MiB ({} B), \
             affinity_lights {:.2} MiB ({} B), animation_descriptor_indices {:.2} MiB ({} B) \
             - total {:.2} MiB ({} B)",
            mib(self.delta_subblocks_bytes),
            self.delta_subblocks_bytes,
            mib(self.affinity_offsets_bytes),
            self.affinity_offsets_bytes,
            mib(self.affinity_lights_bytes),
            self.affinity_lights_bytes,
            mib(self.animation_descriptor_indices_bytes),
            self.animation_descriptor_indices_bytes,
            mib(self.total_bytes()),
            self.total_bytes(),
        );
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeltaComposeBuffers {
    pub animated_light_count: u32,
    pub delta_subblocks: Vec<u16>,
    pub affinity_offsets: Vec<u32>,
    pub affinity_lights: Vec<u32>,
    pub animation_descriptor_indices: Vec<u32>,
    pub affinity_dims: [u32; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct DirectDeltaComposeBuffers {
    pub delta_subblocks: Vec<u16>,
    pub affinity_offsets: Vec<u32>,
    pub affinity_lights: Vec<u32>,
    pub affinity_dims: [u32; 3],
}

pub fn build_delta_buffers(
    delta: Option<&DeltaShVolumesSection>,
    grid_dimensions: [u32; 3],
) -> DeltaComposeBuffers {
    let Some(delta) = delta else {
        let affinity_dims = affinity_dims_for_grid(grid_dimensions);
        return DeltaComposeBuffers {
            animated_light_count: 0,
            delta_subblocks: Vec::new(),
            affinity_offsets: vec![0; affinity_cell_count(affinity_dims) + 1],
            affinity_lights: Vec::new(),
            animation_descriptor_indices: Vec::new(),
            affinity_dims,
        };
    };
    DeltaComposeBuffers {
        animated_light_count: delta.animation_descriptor_indices.len() as u32,
        delta_subblocks: delta.delta_subblocks.clone(),
        affinity_offsets: delta.affinity_offsets.clone(),
        affinity_lights: delta.affinity_lights.clone(),
        animation_descriptor_indices: delta.animation_descriptor_indices.clone(),
        affinity_dims: delta.affinity_dims,
    }
}

pub fn build_direct_delta_buffers(
    delta: Option<&DirectShDeltaVolumesSection>,
    grid_dimensions: [u32; 3],
) -> DirectDeltaComposeBuffers {
    let Some(delta) = delta else {
        let affinity_dims = affinity_dims_for_grid(grid_dimensions);
        return DirectDeltaComposeBuffers {
            delta_subblocks: Vec::new(),
            affinity_offsets: vec![0; affinity_cell_count(affinity_dims) + 1],
            affinity_lights: Vec::new(),
            affinity_dims,
        };
    };
    DirectDeltaComposeBuffers {
        delta_subblocks: delta.delta_subblocks.clone(),
        affinity_offsets: delta.affinity_offsets.clone(),
        affinity_lights: delta.affinity_lights.clone(),
        affinity_dims: delta.affinity_dims,
    }
}

fn affinity_dims_for_grid(grid_dimensions: [u32; 3]) -> [u32; 3] {
    let factor = AFFINITY_FACTOR as u32;
    [
        grid_dimensions[0].div_ceil(factor).max(1),
        grid_dimensions[1].div_ceil(factor).max(1),
        grid_dimensions[2].div_ceil(factor).max(1),
    ]
}

fn affinity_cell_count(dims: [u32; 3]) -> usize {
    dims[0] as usize * dims[1] as usize * dims[2] as usize
}

pub fn build_compose_grid_bytes(
    grid_dimensions: [u32; 3],
    atlas_dimensions: [u32; 2],
    tile_dimension: u32,
    tile_border: u32,
    atlas_tiles_per_row: u32,
    affinity_dims: [u32; 3],
) -> [u8; COMPOSE_GRID_DIMS_SIZE] {
    let mut bytes = [0u8; COMPOSE_GRID_DIMS_SIZE];
    bytes[0..4].copy_from_slice(&grid_dimensions[0].to_ne_bytes());
    bytes[4..8].copy_from_slice(&grid_dimensions[1].to_ne_bytes());
    bytes[8..12].copy_from_slice(&grid_dimensions[2].to_ne_bytes());
    bytes[12..16].copy_from_slice(&tile_dimension.to_ne_bytes());
    bytes[16..20].copy_from_slice(&atlas_dimensions[0].to_ne_bytes());
    bytes[20..24].copy_from_slice(&atlas_dimensions[1].to_ne_bytes());
    bytes[24..28].copy_from_slice(&tile_border.to_ne_bytes());
    bytes[28..32].copy_from_slice(&(delta_probe_f16_stride(tile_dimension) as u32).to_ne_bytes());
    bytes[32..36].copy_from_slice(&affinity_dims[0].to_ne_bytes());
    bytes[36..40].copy_from_slice(&affinity_dims[1].to_ne_bytes());
    bytes[40..44].copy_from_slice(&affinity_dims[2].to_ne_bytes());
    bytes[44..48].copy_from_slice(&atlas_tiles_per_row.to_ne_bytes());
    bytes
}

pub fn u16_slice_to_bytes(data: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 2);
    for &v in data {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

pub fn u32_slice_to_bytes(data: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 4);
    for &v in data {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

pub fn pad_storage_bytes(mut bytes: Vec<u8>, min_bytes: usize) -> Vec<u8> {
    if bytes.is_empty() {
        bytes.resize(min_bytes, 0);
    }
    bytes
}

pub fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign = ((bits >> 15) & 0x1) as u32;
    let exp = ((bits >> 10) & 0x1f) as u32;
    let mant = (bits & 0x3ff) as u32;

    let f32_bits: u32 = if exp == 0 {
        if mant == 0 {
            sign << 31
        } else {
            let mut m = mant;
            let mut e: i32 = -14;
            while (m & 0x400) == 0 {
                m <<= 1;
                e -= 1;
            }
            let m = m & 0x3ff;
            let e_f32 = (e + 127) as u32;
            (sign << 31) | (e_f32 << 23) | (m << 13)
        }
    } else if exp == 0x1f {
        (sign << 31) | (0xff << 23) | (mant << 13)
    } else {
        let e_f32 = exp + (127 - 15);
        (sign << 31) | (e_f32 << 23) | (mant << 13)
    };

    f32::from_bits(f32_bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sh_volume::f32_to_f16_bits;
    use postretro_level_format::delta_sh_volumes::{
        DEFAULT_DELTA_PROBE_F16_STRIDE, PROBES_PER_CELL,
    };
    use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
    use postretro_level_format::octahedral::{
        DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
    };

    fn sample_subblock(seed: u16) -> Vec<u16> {
        (0..PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE)
            .map(|i| seed.wrapping_add(i as u16))
            .collect()
    }

    #[test]
    fn f16_bits_round_trip_for_simple_values() {
        for v in [0.0f32, 1.0, -1.0, 0.5, 2.0, -0.25, 100.0] {
            let bits = f32_to_f16_bits(v);
            let back = f16_bits_to_f32(bits);
            assert!((back - v).abs() < 1e-3);
        }
    }

    #[test]
    fn build_delta_buffers_no_section_returns_empty_payload_with_full_empty_offsets() {
        let b = build_delta_buffers(None, [5, 2, 1]);
        assert_eq!(b.animated_light_count, 0);
        assert!(b.delta_subblocks.is_empty());
        assert_eq!(b.affinity_dims, [2, 1, 1]);
        assert_eq!(b.affinity_offsets, vec![0, 0, 0]);
    }

    #[test]
    fn build_delta_buffers_maps_section_fields_keeping_f16() {
        let mut subblocks = sample_subblock(10);
        subblocks.extend(sample_subblock(200));
        let section = DeltaShVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [3, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            animation_descriptor_indices: vec![4, u32::MAX],
            affinity_offsets: vec![0, 1, 1, 2],
            affinity_lights: vec![0, 1],
            delta_subblocks: subblocks.clone(),
        };

        let b = build_delta_buffers(Some(&section), [12, 1, 1]);
        assert_eq!(b.animated_light_count, 2);
        assert_eq!(b.affinity_dims, [3, 1, 1]);
        assert_eq!(b.affinity_offsets, vec![0, 1, 1, 2]);
        assert_eq!(b.affinity_lights, vec![0, 1]);
        assert_eq!(b.animation_descriptor_indices, vec![4, u32::MAX]);
        assert_eq!(b.delta_subblocks, subblocks);
    }

    #[test]
    fn build_direct_delta_buffers_no_section_returns_empty_payload_with_full_empty_offsets() {
        let b = build_direct_delta_buffers(None, [5, 2, 1]);
        assert!(b.delta_subblocks.is_empty());
        assert_eq!(b.affinity_dims, [2, 1, 1]);
        assert_eq!(b.affinity_offsets, vec![0, 0, 0]);
        assert!(b.affinity_lights.is_empty());
    }

    #[test]
    fn build_direct_delta_buffers_maps_section_fields_keeping_f16() {
        let mut subblocks = sample_subblock(10);
        subblocks.extend(sample_subblock(200));
        let section = DirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [3, 1, 1],
            tile_dimension: DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: DEFAULT_IRRADIANCE_TILE_BORDER,
            affinity_offsets: vec![0, 1, 1, 2],
            affinity_lights: vec![0, 1],
            delta_subblocks: subblocks.clone(),
        };

        let b = build_direct_delta_buffers(Some(&section), [12, 1, 1]);
        assert_eq!(b.affinity_dims, [3, 1, 1]);
        assert_eq!(b.affinity_offsets, vec![0, 1, 1, 2]);
        assert_eq!(b.affinity_lights, vec![0, 1]);
        assert_eq!(b.delta_subblocks, subblocks);
    }
}
