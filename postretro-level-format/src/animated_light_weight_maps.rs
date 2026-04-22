// AnimatedLightWeightMaps PRL section (ID 25): per-chunk atlas rectangles with
// per-texel (offset, count) pairs into a flat pool of (light_index, weight)
// tuples. Baked by the animator at compile time; composed at runtime into an
// animated lightmap contribution atlas.
//
// See: context/plans/in-progress/animated-light-weight-maps/index.md

use crate::FormatError;

/// Current section version.
pub const ANIMATED_LIGHT_WEIGHT_MAPS_VERSION: u32 = 1;

/// Atlas rectangle for one chunk: position and dimensions within the lightmap
/// atlas, plus an offset into the per-texel offset-count table.
///
/// Texture coordinates for atlas sampling are computed as:
///   atlas_uv = (chunk_atlas_xy + texel_uv) / atlas_size
///
/// where `texel_uv` is in [0, width) x [0, height) and `atlas_size` is the
/// atlas resolution (e.g., 1024²).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChunkAtlasRect {
    pub atlas_x: u32,
    pub atlas_y: u32,
    pub width: u32,
    pub height: u32,
    pub texel_offset: u32, // index into the per-texel offset_counts array
}

/// One per-texel entry: (offset, count) into the flat `texel_lights` pool.
///
/// For a texel at position (tx, ty) within a chunk rect, the per-texel record
/// is at index `chunk_rect.texel_offset + ty * chunk_rect.width + tx`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TexelLightEntry {
    pub offset: u32,
    pub count: u32,
}

/// One light-weight entry: (light_index, weight).
///
/// `light_index`: direct slot into the GPU `AnimationDescriptor` buffer —
/// the same namespace as `AnimatedLightChunks.chunks[i].light_indices`,
/// filtered by `!is_dynamic && animation.is_some()`. No remap is needed at
/// bake time because the chunk-list builder and the descriptor buffer use
/// the same filter and iteration order. `weight`: per-texel contribution
/// magnitude (0.0..1.0, normalized after bake).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TexelLight {
    pub light_index: u32,
    pub weight: f32,
}

/// AnimatedLightWeightMaps section (ID 25).
///
/// On-disk layout (little-endian):
///
/// ```text
///   Header (16 bytes):
///     u32      version            (= 1)
///     u32      chunk_count
///     u32      offset_counts_len  (length of per-texel offset_counts array)
///     u32      texel_lights_len   (length of flat light weights pool)
///
///   Chunk rects (20 bytes × chunk_count):
///     u32      atlas_x
///     u32      atlas_y
///     u32      width
///     u32      height
///     u32      texel_offset       (index into offset_counts)
///
///   Offset table (8 bytes × offset_counts_len):
///     u32      offset             (into texel_lights)
///     u32      count
///
///   Light weights (8 bytes × texel_lights_len):
///     u32      light_index
///     f32      weight
/// ```
///
/// Invariants verified at load time:
///   - chunk_count matches AnimatedLightChunksSection.chunks.len()
///   - offset_counts_len == Σ (chunk_rect.width × chunk_rect.height) for all chunks
///   - chunk_rect[i].texel_offset == Σ_{j<i} (chunk_rect[j].width × chunk_rect[j].height)
///   - All indices in texel_lights are within the animated-light descriptor array bounds.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimatedLightWeightMapsSection {
    pub chunk_rects: Vec<ChunkAtlasRect>,
    pub offset_counts: Vec<TexelLightEntry>,
    pub texel_lights: Vec<TexelLight>,
}

const HEADER_SIZE: usize = 16;
const CHUNK_RECT_SIZE: usize = 20;
const OFFSET_ENTRY_SIZE: usize = 8;
const TEXEL_LIGHT_SIZE: usize = 8;

impl AnimatedLightWeightMapsSection {
    /// Empty section — used when a map has no animated lights or no weight maps.
    pub fn empty() -> Self {
        Self {
            chunk_rects: Vec::new(),
            offset_counts: Vec::new(),
            texel_lights: Vec::new(),
        }
    }

    /// Verify internal consistency: offset_counts length matches chunk area sum.
    pub fn is_consistent(&self) -> bool {
        let expected_offset_counts_len: u32 =
            self.chunk_rects.iter().map(|r| r.width * r.height).sum();

        if self.offset_counts.len() as u32 != expected_offset_counts_len {
            return false;
        }

        // Verify chunk texel offsets form a valid partition.
        let mut expected_offset = 0;
        for chunk in &self.chunk_rects {
            if chunk.texel_offset != expected_offset {
                return false;
            }
            expected_offset += chunk.width * chunk.height;
        }

        true
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(
            HEADER_SIZE
                + self.chunk_rects.len() * CHUNK_RECT_SIZE
                + self.offset_counts.len() * OFFSET_ENTRY_SIZE
                + self.texel_lights.len() * TEXEL_LIGHT_SIZE,
        );

        buf.extend_from_slice(&ANIMATED_LIGHT_WEIGHT_MAPS_VERSION.to_le_bytes());
        buf.extend_from_slice(&(self.chunk_rects.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(self.offset_counts.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(self.texel_lights.len() as u32).to_le_bytes());

        for rect in &self.chunk_rects {
            buf.extend_from_slice(&rect.atlas_x.to_le_bytes());
            buf.extend_from_slice(&rect.atlas_y.to_le_bytes());
            buf.extend_from_slice(&rect.width.to_le_bytes());
            buf.extend_from_slice(&rect.height.to_le_bytes());
            buf.extend_from_slice(&rect.texel_offset.to_le_bytes());
        }

        for entry in &self.offset_counts {
            buf.extend_from_slice(&entry.offset.to_le_bytes());
            buf.extend_from_slice(&entry.count.to_le_bytes());
        }

        for light in &self.texel_lights {
            buf.extend_from_slice(&light.light_index.to_le_bytes());
            buf.extend_from_slice(&light.weight.to_le_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "animated light weight maps section too short for header",
            )));
        }

        let version = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        if version != ANIMATED_LIGHT_WEIGHT_MAPS_VERSION {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "animated light weight maps section version {version}, expected {ANIMATED_LIGHT_WEIGHT_MAPS_VERSION}"
                ),
            )));
        }

        let chunk_count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let offset_counts_len = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
        let texel_lights_len =
            u32::from_le_bytes([data[12], data[13], data[14], data[15]]) as usize;

        let needed = HEADER_SIZE
            + chunk_count * CHUNK_RECT_SIZE
            + offset_counts_len * OFFSET_ENTRY_SIZE
            + texel_lights_len * TEXEL_LIGHT_SIZE;

        if data.len() < needed {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "animated light weight maps section truncated: need {needed} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let mut chunk_rects = Vec::with_capacity(chunk_count);
        let mut cursor = HEADER_SIZE;
        for _ in 0..chunk_count {
            let atlas_x = read_u32(data, cursor);
            let atlas_y = read_u32(data, cursor + 4);
            let width = read_u32(data, cursor + 8);
            let height = read_u32(data, cursor + 12);
            let texel_offset = read_u32(data, cursor + 16);

            chunk_rects.push(ChunkAtlasRect {
                atlas_x,
                atlas_y,
                width,
                height,
                texel_offset,
            });
            cursor += CHUNK_RECT_SIZE;
        }

        let mut offset_counts = Vec::with_capacity(offset_counts_len);
        for _ in 0..offset_counts_len {
            let offset = read_u32(data, cursor);
            let count = read_u32(data, cursor + 4);
            offset_counts.push(TexelLightEntry { offset, count });
            cursor += OFFSET_ENTRY_SIZE;
        }

        let mut texel_lights = Vec::with_capacity(texel_lights_len);
        for _ in 0..texel_lights_len {
            let light_index = read_u32(data, cursor);
            let weight = read_f32(data, cursor + 4);
            texel_lights.push(TexelLight {
                light_index,
                weight,
            });
            cursor += TEXEL_LIGHT_SIZE;
        }

        Ok(Self {
            chunk_rects,
            offset_counts,
            texel_lights,
        })
    }
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_f32(data: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_section() -> AnimatedLightWeightMapsSection {
        // Two chunks: 2x2 and 3x1, with 4 and 3 texels respectively.
        AnimatedLightWeightMapsSection {
            chunk_rects: vec![
                ChunkAtlasRect {
                    atlas_x: 0,
                    atlas_y: 0,
                    width: 2,
                    height: 2,
                    texel_offset: 0,
                },
                ChunkAtlasRect {
                    atlas_x: 2,
                    atlas_y: 0,
                    width: 3,
                    height: 1,
                    texel_offset: 4,
                },
            ],
            offset_counts: vec![
                TexelLightEntry {
                    offset: 0,
                    count: 2,
                },
                TexelLightEntry {
                    offset: 2,
                    count: 1,
                },
                TexelLightEntry {
                    offset: 3,
                    count: 0,
                },
                TexelLightEntry {
                    offset: 3,
                    count: 1,
                },
                TexelLightEntry {
                    offset: 4,
                    count: 2,
                },
                TexelLightEntry {
                    offset: 6,
                    count: 1,
                },
                TexelLightEntry {
                    offset: 7,
                    count: 1,
                },
            ],
            texel_lights: vec![
                TexelLight {
                    light_index: 0,
                    weight: 0.8,
                },
                TexelLight {
                    light_index: 1,
                    weight: 0.2,
                },
                TexelLight {
                    light_index: 2,
                    weight: 1.0,
                },
                TexelLight {
                    light_index: 3,
                    weight: 0.5,
                },
                TexelLight {
                    light_index: 4,
                    weight: 0.6,
                },
                TexelLight {
                    light_index: 5,
                    weight: 0.3,
                },
                TexelLight {
                    light_index: 6,
                    weight: 0.9,
                },
                TexelLight {
                    light_index: 7,
                    weight: 0.4,
                },
            ],
        }
    }

    #[test]
    fn round_trip_byte_identical() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let restored = AnimatedLightWeightMapsSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
        let rebytes = restored.to_bytes();
        assert_eq!(bytes, rebytes);
    }

    #[test]
    fn to_bytes_is_deterministic() {
        // Two calls on the same input must yield byte-identical output.
        // Guards against hash-map iteration order or other nondeterministic
        // packing entering the encoder.
        let section = sample_section();
        let a = section.to_bytes();
        let b = section.to_bytes();
        assert_eq!(a, b);
    }

    #[test]
    fn invariant_offset_counts_length_and_prefix_sum() {
        // For a valid fixture: offset_counts.len() == Σ (width × height)
        // and chunk_rects[i].texel_offset == Σ_{j<i} (width_j × height_j).
        let section = sample_section();
        let mut running = 0u32;
        for chunk in &section.chunk_rects {
            assert_eq!(chunk.texel_offset, running);
            running += chunk.width * chunk.height;
        }
        assert_eq!(section.offset_counts.len() as u32, running);
    }

    #[test]
    fn empty_section_round_trips() {
        let section = AnimatedLightWeightMapsSection::empty();
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), HEADER_SIZE);
        let restored = AnimatedLightWeightMapsSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn byte_layout_matches_sizes() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let expected_len = HEADER_SIZE
            + section.chunk_rects.len() * CHUNK_RECT_SIZE
            + section.offset_counts.len() * OFFSET_ENTRY_SIZE
            + section.texel_lights.len() * TEXEL_LIGHT_SIZE;
        assert_eq!(bytes.len(), expected_len);
    }

    #[test]
    fn consistency_check_valid() {
        let section = sample_section();
        assert!(section.is_consistent());
    }

    #[test]
    fn consistency_check_fails_on_wrong_offset_counts_length() {
        let mut section = sample_section();
        section.offset_counts.pop();
        assert!(!section.is_consistent());
    }

    #[test]
    fn consistency_check_fails_on_wrong_chunk_offset() {
        let mut section = sample_section();
        // Break the second chunk's texel_offset (should be 4, not 5).
        section.chunk_rects[1].texel_offset = 5;
        assert!(!section.is_consistent());
    }

    #[test]
    fn rejects_truncated_header() {
        let err = AnimatedLightWeightMapsSection::from_bytes(&[0u8; 8]).unwrap_err();
        assert!(err.to_string().contains("too short"));
    }

    #[test]
    fn rejects_truncated_body() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let truncated = &bytes[..bytes.len() - 1];
        let err = AnimatedLightWeightMapsSection::from_bytes(truncated).unwrap_err();
        assert!(err.to_string().contains("truncated"));
    }

    #[test]
    fn rejects_bad_version() {
        let mut bytes = sample_section().to_bytes();
        bytes[0..4].copy_from_slice(&999u32.to_le_bytes());
        let err = AnimatedLightWeightMapsSection::from_bytes(&bytes).unwrap_err();
        assert!(err.to_string().contains("version"));
    }

    #[test]
    fn offset_counts_length_matches_chunk_area_sum() {
        let section = sample_section();
        let expected_len: u32 = section.chunk_rects.iter().map(|r| r.width * r.height).sum();
        assert_eq!(section.offset_counts.len() as u32, expected_len);
    }

    #[test]
    fn chunk_texels_form_contiguous_partition() {
        let section = sample_section();
        let mut expected = 0u32;
        for chunk in &section.chunk_rects {
            assert_eq!(chunk.texel_offset, expected);
            expected += chunk.width * chunk.height;
        }
        assert_eq!(expected as usize, section.offset_counts.len());
    }
}
