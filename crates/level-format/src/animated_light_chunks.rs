// AnimatedLightChunks PRL section (ID 24): per-face spatial partition where
// every chunk carries a bounded list of animated-light indices influencing it.
// Prerequisite for the future per-light weight-map animated-lightmap pipeline.
// Uses the offset-table + flat-pool *pattern* from `ChunkLightListSection`
// (ID 23); structurally distinct (leaf-range-indexed per-face records, not a
// world-space uniform grid).
//
// See: context/plans/in-progress/animated-light-chunks/index.md

use crate::FormatError;

/// Current section version.
pub const ANIMATED_LIGHT_CHUNKS_VERSION: u32 = 1;

/// Maximum animated-light indices a single chunk may carry before the
/// subdivision cap forces the builder to split (or bottom out at the
/// min-extent floor). Shared format-crate constant; the builder, tests, and
/// any downstream consumer must agree on this value.
pub const MAX_ANIMATED_LIGHTS_PER_CHUNK: usize = 4;

/// One chunk record. Fixed 56-byte stride. Build-time produced by `prl-build`; runtime treats the section as read-only.
///
/// Chunk UV is **face-local in world-meter units** along the parent face's
/// (u,v) basis — the same units as `Chart.uv_min` / `Chart.uv_extent` in the
/// lightmap baker. Not 0..1 normalized. Storing face-local UV keeps the chunk
/// record independent of atlas packing changes.
///
/// `(index_offset, index_count)` points into the section's flat
/// `light_indices` pool. Light indices are into the **filtered** light list
/// (same namespace as `AlphaLightsSection` / `LightInfluenceSection`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimatedLightChunk {
    pub aabb_min: [f32; 3],
    pub face_index: u32,
    pub aabb_max: [f32; 3],
    pub index_offset: u32,
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub index_count: u32,
    pub _padding: u32,
}

/// AnimatedLightChunks section (ID 24).
///
/// On-disk layout (little-endian):
///
/// ```text
///   Header (16 bytes):
///     u32      version         (= 1)
///     u32      chunk_count
///     u32      index_count     (length of flat light_indices array)
///     u32      reserved        (= 0)
///
///   Chunk records (CHUNK_STRIDE bytes × chunk_count):
///     f32 × 3  aabb_min                (offset  0)
///     u32      face_index              (offset 12)
///     f32 × 3  aabb_max                (offset 16)
///     u32      index_offset            (offset 28, into light_indices)
///     f32 × 2  uv_min                  (offset 32, face-local, world meters)
///     f32 × 2  uv_max                  (offset 40)
///     u32      index_count             (offset 48)
///     u32      _padding  (= 0)         (offset 52)
///
///   Flat light indices:
///     u32 × index_count
/// ```
///
/// Chunks are emitted in flat-leaf-array order (the BVH leaves' builder-stable
/// order), so each `BvhLeaf` owns a contiguous range expressible via
/// `chunk_range_start` / `chunk_range_count`.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimatedLightChunksSection {
    pub chunks: Vec<AnimatedLightChunk>,
    pub light_indices: Vec<u32>,
}

pub const HEADER_SIZE: usize = 16;
pub const CHUNK_STRIDE: usize = 56;

impl AnimatedLightChunksSection {
    /// Empty section — used when a map has no animated lights (or no overlap).
    pub fn empty() -> Self {
        Self {
            chunks: Vec::new(),
            light_indices: Vec::new(),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(
            HEADER_SIZE + self.chunks.len() * CHUNK_STRIDE + self.light_indices.len() * 4,
        );

        buf.extend_from_slice(&ANIMATED_LIGHT_CHUNKS_VERSION.to_le_bytes());
        buf.extend_from_slice(&(self.chunks.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(self.light_indices.len() as u32).to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes()); // reserved

        for chunk in &self.chunks {
            buf.extend_from_slice(&chunk.aabb_min[0].to_le_bytes());
            buf.extend_from_slice(&chunk.aabb_min[1].to_le_bytes());
            buf.extend_from_slice(&chunk.aabb_min[2].to_le_bytes());
            buf.extend_from_slice(&chunk.face_index.to_le_bytes());
            buf.extend_from_slice(&chunk.aabb_max[0].to_le_bytes());
            buf.extend_from_slice(&chunk.aabb_max[1].to_le_bytes());
            buf.extend_from_slice(&chunk.aabb_max[2].to_le_bytes());
            buf.extend_from_slice(&chunk.index_offset.to_le_bytes());
            buf.extend_from_slice(&chunk.uv_min[0].to_le_bytes());
            buf.extend_from_slice(&chunk.uv_min[1].to_le_bytes());
            buf.extend_from_slice(&chunk.uv_max[0].to_le_bytes());
            buf.extend_from_slice(&chunk.uv_max[1].to_le_bytes());
            buf.extend_from_slice(&chunk.index_count.to_le_bytes());
            buf.extend_from_slice(&0u32.to_le_bytes()); // _padding
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
                "animated light chunks section too short for header",
            )));
        }

        let version = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        if version != ANIMATED_LIGHT_CHUNKS_VERSION {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "animated light chunks section version {version}, expected {ANIMATED_LIGHT_CHUNKS_VERSION}"
                ),
            )));
        }

        let chunk_count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let index_count = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
        // bytes 12..16 are header reserved (ignored)

        let needed = HEADER_SIZE + chunk_count * CHUNK_STRIDE + index_count * 4;
        if data.len() < needed {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "animated light chunks section truncated: need {needed} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let mut chunks = Vec::with_capacity(chunk_count);
        let mut cursor = HEADER_SIZE;
        for _ in 0..chunk_count {
            let aabb_min = read_vec3(data, cursor);
            let face_index = read_u32(data, cursor + 12);
            let aabb_max = read_vec3(data, cursor + 16);
            let index_offset = read_u32(data, cursor + 28);
            let uv_min = [read_f32(data, cursor + 32), read_f32(data, cursor + 36)];
            let uv_max = [read_f32(data, cursor + 40), read_f32(data, cursor + 44)];
            let index_count_field = read_u32(data, cursor + 48);
            // bytes 52..56 are chunk _padding (ignored)

            chunks.push(AnimatedLightChunk {
                aabb_min,
                face_index,
                aabb_max,
                index_offset,
                uv_min,
                uv_max,
                index_count: index_count_field,
                _padding: 0,
            });
            cursor += CHUNK_STRIDE;
        }

        let mut light_indices = Vec::with_capacity(index_count);
        for _ in 0..index_count {
            light_indices.push(read_u32(data, cursor));
            cursor += 4;
        }

        Ok(Self {
            chunks,
            light_indices,
        })
    }
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_f32(data: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_vec3(data: &[u8], at: usize) -> [f32; 3] {
    [
        read_f32(data, at),
        read_f32(data, at + 4),
        read_f32(data, at + 8),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_section() -> AnimatedLightChunksSection {
        AnimatedLightChunksSection {
            chunks: vec![
                AnimatedLightChunk {
                    aabb_min: [0.0, 0.0, 0.0],
                    face_index: 7,
                    aabb_max: [1.0, 2.0, 3.0],
                    index_offset: 0,
                    uv_min: [0.0, 0.0],
                    uv_max: [1.0, 1.0],
                    index_count: 2,
                    _padding: 0,
                },
                AnimatedLightChunk {
                    aabb_min: [4.0, 5.0, 6.0],
                    face_index: 9,
                    aabb_max: [5.0, 6.0, 7.0],
                    index_offset: 2,
                    uv_min: [0.5, 0.5],
                    uv_max: [2.5, 1.5],
                    index_count: 3,
                    _padding: 0,
                },
            ],
            light_indices: vec![10, 11, 12, 13, 14],
        }
    }

    #[test]
    fn round_trip_byte_identical() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let restored = AnimatedLightChunksSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
        let rebytes = restored.to_bytes();
        assert_eq!(bytes, rebytes);
    }

    #[test]
    fn empty_section_round_trips() {
        let section = AnimatedLightChunksSection::empty();
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), HEADER_SIZE);
        let restored = AnimatedLightChunksSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn byte_layout_matches_stride() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let expected_len =
            HEADER_SIZE + section.chunks.len() * CHUNK_STRIDE + section.light_indices.len() * 4;
        assert_eq!(bytes.len(), expected_len);
        assert_eq!(CHUNK_STRIDE, 56);
    }

    #[test]
    fn rejects_truncated_header() {
        let err = AnimatedLightChunksSection::from_bytes(&[0u8; 8]).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }

    #[test]
    fn rejects_truncated_body() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let truncated = &bytes[..bytes.len() - 1];
        let err = AnimatedLightChunksSection::from_bytes(truncated).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }

    #[test]
    fn rejects_bad_version() {
        let mut bytes = sample_section().to_bytes();
        bytes[0..4].copy_from_slice(&999u32.to_le_bytes());
        let err = AnimatedLightChunksSection::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }
}
