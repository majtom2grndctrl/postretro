// Per-chunk light list: grid metadata + offset table + flat index list
// uploaded to GPU storage buffers. Parsed from the `ChunkLightList` PRL
// section; missing-section fallback is a zeroed grid with
// `has_chunk_grid = 0` so the shader iterates the full spec buffer.
//
// See: context/plans/ready/lighting-chunk-lists/index.md Task B step 2

use postretro_level_format::chunk_light_list::ChunkLightListSection;

/// WGSL uniform size for the grid metadata. Two vec4<f32>-aligned slots:
///
///   0..12   grid_origin     (f32x3)
///   12..16  cell_size       (f32)
///   16..28  dims            (u32x3)
///   28..32  has_chunk_grid  (u32) — 0 = fall back to full spec buffer
pub const CHUNK_GRID_UNIFORM_SIZE: usize = 32;

/// CPU-side metadata and payload, ready for GPU upload.
pub struct ChunkGrid {
    pub grid_info: [u8; CHUNK_GRID_UNIFORM_SIZE],
    /// Offset table: `[offset:u32, count:u32]` per chunk, linearised by
    /// `z * dims.x * dims.y + y * dims.x + x`. Minimum one element so the
    /// storage binding is never empty.
    pub offset_table: Vec<u8>,
    /// Flat u32 index list (into the spec buffer). Minimum one element so
    /// the storage binding is never empty.
    pub index_list: Vec<u8>,
    /// True when the PRL section was present and used. False when the
    /// fallback dummy payload is in effect — the shader reads
    /// `has_chunk_grid == 0` and iterates the full spec buffer.
    pub present: bool,
}

impl ChunkGrid {
    /// Fallback grid used when no `ChunkLightList` section is present.
    /// The shader guards on `has_chunk_grid == 0` and never reads the
    /// dummy buffers; they exist solely to satisfy wgpu's nonzero
    /// storage-binding requirement.
    pub fn fallback() -> Self {
        let mut grid_info = [0u8; CHUNK_GRID_UNIFORM_SIZE];
        // All zeros already encode has_chunk_grid = 0.
        write_f32(&mut grid_info, 12, 1.0); // non-zero cell_size defends against divide-by-zero should the shader ever forget the guard
        Self {
            grid_info,
            offset_table: vec![0u8; 8], // one dummy (offset=0, count=0)
            index_list: vec![0u8; 4],   // one dummy index
            present: false,
        }
    }

    pub fn from_section(sec: &ChunkLightListSection) -> Self {
        let mut grid_info = [0u8; CHUNK_GRID_UNIFORM_SIZE];
        write_f32(&mut grid_info, 0, sec.grid_origin[0]);
        write_f32(&mut grid_info, 4, sec.grid_origin[1]);
        write_f32(&mut grid_info, 8, sec.grid_origin[2]);
        write_f32(&mut grid_info, 12, sec.cell_size.max(1.0e-6));
        write_u32(&mut grid_info, 16, sec.grid_dimensions[0]);
        write_u32(&mut grid_info, 20, sec.grid_dimensions[1]);
        write_u32(&mut grid_info, 24, sec.grid_dimensions[2]);
        write_u32(&mut grid_info, 28, sec.has_grid);

        let mut offset_table = Vec::with_capacity(sec.offsets.len() * 8);
        for entry in &sec.offsets {
            offset_table.extend_from_slice(&entry.offset.to_ne_bytes());
            offset_table.extend_from_slice(&entry.count.to_ne_bytes());
        }
        if offset_table.is_empty() {
            offset_table.extend_from_slice(&[0u8; 8]);
        }

        let mut index_list = Vec::with_capacity(sec.light_indices.len() * 4);
        for &idx in &sec.light_indices {
            index_list.extend_from_slice(&idx.to_ne_bytes());
        }
        if index_list.is_empty() {
            index_list.extend_from_slice(&[0u8; 4]);
        }

        Self {
            grid_info,
            offset_table,
            index_list,
            present: sec.has_grid != 0,
        }
    }
}

#[inline]
fn write_f32(dst: &mut [u8], off: usize, v: f32) {
    dst[off..off + 4].copy_from_slice(&v.to_ne_bytes());
}

#[inline]
fn write_u32(dst: &mut [u8], off: usize, v: u32) {
    dst[off..off + 4].copy_from_slice(&v.to_ne_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::chunk_light_list::ChunkEntry;

    #[test]
    fn fallback_has_chunk_grid_zero() {
        let g = ChunkGrid::fallback();
        assert!(!g.present);
        let has = u32::from_ne_bytes(g.grid_info[28..32].try_into().unwrap());
        assert_eq!(has, 0);
        // Non-empty dummy payloads so the storage bindings are never zero-sized.
        assert!(!g.offset_table.is_empty());
        assert!(!g.index_list.is_empty());
    }

    #[test]
    fn from_section_encodes_metadata_and_flag() {
        let sec = ChunkLightListSection {
            grid_origin: [-1.0, 0.5, 2.0],
            cell_size: 8.0,
            grid_dimensions: [2, 1, 1],
            has_grid: 1,
            per_chunk_cap: 64,
            offsets: vec![
                ChunkEntry {
                    offset: 0,
                    count: 2,
                },
                ChunkEntry {
                    offset: 2,
                    count: 1,
                },
            ],
            light_indices: vec![3, 5, 7],
        };
        let g = ChunkGrid::from_section(&sec);
        assert!(g.present);

        let read_f32 =
            |off: usize| f32::from_ne_bytes(g.grid_info[off..off + 4].try_into().unwrap());
        let read_u32 =
            |off: usize| u32::from_ne_bytes(g.grid_info[off..off + 4].try_into().unwrap());
        assert_eq!(read_f32(0), -1.0);
        assert_eq!(read_f32(4), 0.5);
        assert_eq!(read_f32(8), 2.0);
        assert_eq!(read_f32(12), 8.0);
        assert_eq!(read_u32(16), 2);
        assert_eq!(read_u32(20), 1);
        assert_eq!(read_u32(24), 1);
        assert_eq!(read_u32(28), 1);

        assert_eq!(g.offset_table.len(), 2 * 8);
        assert_eq!(g.index_list.len(), 3 * 4);
    }
}
