// ChunkLightList PRL section (ID 23): world-space uniform chunk grid with a
// per-chunk list of static-light indices. Each chunk stores a conservative,
// visibility-filtered set of lights that can contribute to fragments inside
// it. Consumed by the runtime forward pass for bounded per-fragment specular
// iteration.
//
// See: context/plans/in-progress/lighting-chunk-lists/index.md

use crate::FormatError;

/// Current section version.
pub const CHUNK_LIGHT_LIST_VERSION: u32 = 1;

/// Maximum light indices per chunk (default cap; matches the baker's clamp).
pub const DEFAULT_PER_CHUNK_CAP: u32 = 64;

/// One entry in the offset table: byte-free `(offset, count)` into the flat
/// `light_indices` array, one per chunk in linearized `z*ny*nx + y*nx + x` order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChunkEntry {
    pub offset: u32,
    pub count: u32,
}

/// ChunkLightList section (ID 23).
///
/// On-disk layout (little-endian):
///
/// ```text
///   Header (48 bytes):
///     u32      version            (= 1)
///     u32      has_grid           (0 = placeholder / no grid, 1 = populated)
///     f32 × 3  grid_origin        (world-space min corner, meters)
///     f32      cell_size          (meters per cell, uniform cubic)
///     u32 × 3  grid_dimensions    (chunk count along x, y, z)
///     u32      per_chunk_cap      (clamp used during bake; informational)
///     u32      index_count        (length of the flat light-indices array)
///     u32      reserved           (= 0)
///
///   Offset table (8 bytes × chunk_count):
///     per chunk: u32 offset, u32 count
///
///   Flat indices:
///     u32 × index_count, packed per chunk in offset-table order
/// ```
///
/// The placeholder shape (`has_grid == 0`, empty offset table and indices) lets
/// the runtime fall back to full-buffer iteration when no grid was baked.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkLightListSection {
    /// World-space min corner of the grid, in meters.
    pub grid_origin: [f32; 3],
    /// Uniform cubic chunk edge length, meters.
    pub cell_size: f32,
    /// Chunk count along x, y, z.
    pub grid_dimensions: [u32; 3],
    /// 0 = no grid baked (runtime falls back to full-buffer iteration).
    /// 1 = populated grid.
    pub has_grid: u32,
    /// Clamp used during bake. Informational — the real invariant is that no
    /// `ChunkEntry.count` exceeds this.
    pub per_chunk_cap: u32,
    /// Offset table, length = `grid_dimensions.x * y * z` when `has_grid == 1`.
    pub offsets: Vec<ChunkEntry>,
    /// Flat per-chunk light index arrays concatenated in offset-table order.
    pub light_indices: Vec<u32>,
}

const HEADER_SIZE: usize = 48;
const OFFSET_ENTRY_SIZE: usize = 8;

impl ChunkLightListSection {
    /// Empty placeholder — used when no grid is baked (no static lights, or
    /// the map has no geometry). Runtime treats `has_grid == 0` as the
    /// full-buffer-iteration fallback signal.
    pub fn placeholder() -> Self {
        Self {
            grid_origin: [0.0; 3],
            cell_size: 0.0,
            grid_dimensions: [0, 0, 0],
            has_grid: 0,
            per_chunk_cap: DEFAULT_PER_CHUNK_CAP,
            offsets: Vec::new(),
            light_indices: Vec::new(),
        }
    }

    /// Linearized chunk count. `0` when placeholder.
    pub fn chunk_count(&self) -> usize {
        (self.grid_dimensions[0] as usize)
            * (self.grid_dimensions[1] as usize)
            * (self.grid_dimensions[2] as usize)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(
            HEADER_SIZE + self.offsets.len() * OFFSET_ENTRY_SIZE + self.light_indices.len() * 4,
        );

        buf.extend_from_slice(&CHUNK_LIGHT_LIST_VERSION.to_le_bytes());
        buf.extend_from_slice(&self.has_grid.to_le_bytes());
        buf.extend_from_slice(&self.grid_origin[0].to_le_bytes());
        buf.extend_from_slice(&self.grid_origin[1].to_le_bytes());
        buf.extend_from_slice(&self.grid_origin[2].to_le_bytes());
        buf.extend_from_slice(&self.cell_size.to_le_bytes());
        buf.extend_from_slice(&self.grid_dimensions[0].to_le_bytes());
        buf.extend_from_slice(&self.grid_dimensions[1].to_le_bytes());
        buf.extend_from_slice(&self.grid_dimensions[2].to_le_bytes());
        buf.extend_from_slice(&self.per_chunk_cap.to_le_bytes());
        buf.extend_from_slice(&(self.light_indices.len() as u32).to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());

        for entry in &self.offsets {
            buf.extend_from_slice(&entry.offset.to_le_bytes());
            buf.extend_from_slice(&entry.count.to_le_bytes());
        }
        for idx in &self.light_indices {
            buf.extend_from_slice(&idx.to_le_bytes());
        }
        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "chunk light list section too short for header",
            )));
        }

        let version = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        if version != CHUNK_LIGHT_LIST_VERSION {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "chunk light list section version {version}, expected {CHUNK_LIGHT_LIST_VERSION}"
                ),
            )));
        }

        let has_grid = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        let ox = f32::from_le_bytes([data[8], data[9], data[10], data[11]]);
        let oy = f32::from_le_bytes([data[12], data[13], data[14], data[15]]);
        let oz = f32::from_le_bytes([data[16], data[17], data[18], data[19]]);
        let cell = f32::from_le_bytes([data[20], data[21], data[22], data[23]]);
        let dx = u32::from_le_bytes([data[24], data[25], data[26], data[27]]);
        let dy = u32::from_le_bytes([data[28], data[29], data[30], data[31]]);
        let dz = u32::from_le_bytes([data[32], data[33], data[34], data[35]]);
        let cap = u32::from_le_bytes([data[36], data[37], data[38], data[39]]);
        let index_count = u32::from_le_bytes([data[40], data[41], data[42], data[43]]) as usize;

        let chunk_count = (dx as usize) * (dy as usize) * (dz as usize);
        let needed = HEADER_SIZE + chunk_count * OFFSET_ENTRY_SIZE + index_count * 4;
        if data.len() < needed {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "chunk light list section truncated: need {needed} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let mut offsets = Vec::with_capacity(chunk_count);
        let mut cursor = HEADER_SIZE;
        for _ in 0..chunk_count {
            let off = u32::from_le_bytes([
                data[cursor],
                data[cursor + 1],
                data[cursor + 2],
                data[cursor + 3],
            ]);
            let cnt = u32::from_le_bytes([
                data[cursor + 4],
                data[cursor + 5],
                data[cursor + 6],
                data[cursor + 7],
            ]);
            offsets.push(ChunkEntry {
                offset: off,
                count: cnt,
            });
            cursor += OFFSET_ENTRY_SIZE;
        }

        let mut light_indices = Vec::with_capacity(index_count);
        for _ in 0..index_count {
            let v = u32::from_le_bytes([
                data[cursor],
                data[cursor + 1],
                data[cursor + 2],
                data[cursor + 3],
            ]);
            light_indices.push(v);
            cursor += 4;
        }

        Ok(Self {
            grid_origin: [ox, oy, oz],
            cell_size: cell,
            grid_dimensions: [dx, dy, dz],
            has_grid,
            per_chunk_cap: cap,
            offsets,
            light_indices,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_round_trip() {
        let section = ChunkLightListSection::placeholder();
        let bytes = section.to_bytes();
        let restored = ChunkLightListSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored, section);
        assert_eq!(restored.has_grid, 0);
        assert_eq!(restored.chunk_count(), 0);
    }

    #[test]
    fn populated_round_trip_preserves_offsets_and_indices() {
        let section = ChunkLightListSection {
            grid_origin: [-8.0, 0.0, -8.0],
            cell_size: 8.0,
            grid_dimensions: [2, 1, 2],
            has_grid: 1,
            per_chunk_cap: DEFAULT_PER_CHUNK_CAP,
            offsets: vec![
                ChunkEntry {
                    offset: 0,
                    count: 2,
                },
                ChunkEntry {
                    offset: 2,
                    count: 1,
                },
                ChunkEntry {
                    offset: 3,
                    count: 0,
                },
                ChunkEntry {
                    offset: 3,
                    count: 3,
                },
            ],
            light_indices: vec![0, 4, 2, 0, 1, 2],
        };
        let bytes = section.to_bytes();
        let restored = ChunkLightListSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored, section);
    }

    #[test]
    fn rejects_truncated_header() {
        let err = ChunkLightListSection::from_bytes(&[0u8; 12]).unwrap_err();
        assert!(err.to_string().contains("too short"));
    }

    #[test]
    fn rejects_truncated_body() {
        let mut section = ChunkLightListSection::placeholder();
        section.has_grid = 1;
        section.grid_dimensions = [2, 1, 1];
        section.offsets = vec![
            ChunkEntry {
                offset: 0,
                count: 1,
            },
            ChunkEntry {
                offset: 1,
                count: 0,
            },
        ];
        section.light_indices = vec![7];
        let mut bytes = section.to_bytes();
        bytes.truncate(bytes.len() - 1);
        let err = ChunkLightListSection::from_bytes(&bytes).unwrap_err();
        assert!(err.to_string().contains("truncated"));
    }
}
