// AnimatedLightWeightMaps PRL section (ID 25): per-chunk atlas rectangles with
// per-texel (offset, count) pairs into a flat pool of (light_index, weight)
// tuples. Baked by the animator at compile time; composed at runtime into an
// animated lightmap contribution atlas.
//
// See: context/plans/in-progress/animated-lightmap-array-atlas/index.md

use crate::FormatError;

/// Current section version. Version 3 adds static-atlas layer information so
/// the runtime can compose animated lightmaps into an array atlas.
pub const ANIMATED_LIGHT_WEIGHT_MAPS_VERSION: u32 = 3;

const ANIMATED_LIGHT_WEIGHT_MAPS_V2_VERSION: u32 = 2;

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
    // Appended in v3. Do not move before the v2 fields above.
    pub layer: u32,
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

/// One light-weight entry: (light_index, weight, direction_oct).
///
/// `light_index`: direct slot into the GPU `AnimationDescriptor` buffer —
/// the same namespace as `AnimatedLightChunks.chunks[i].light_indices`,
/// filtered by `!is_dynamic && animation.is_some()`. No remap is needed at
/// bake time because the chunk-list builder and the descriptor buffer use
/// the same filter and iteration order. `weight`: per-texel contribution
/// magnitude (0.0..1.0, normalized after bake).
///
/// `direction_oct`: octahedral-encoded unit vector from the texel toward
/// the light (`[u16; 2]`, same encoding as `crate::octahedral::encode`).
/// Baked because the light's geometry is static — its per-texel incoming
/// direction never changes. The compose pass weights it by the light's
/// per-frame radiance to fuse a runtime dominant-direction atlas the SDF
/// shadow pass traces toward (Task 2b of sdf-static-occluder-shadows).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TexelLight {
    pub light_index: u32,
    pub weight: f32,
    pub direction_oct: [u16; 2],
}

/// AnimatedLightWeightMaps section (ID 25).
///
/// On-disk layout (little-endian):
///
/// ```text
///   Header (20 bytes):
///     u32      version            (= 3)
///     u32      chunk_count
///     u32      offset_counts_len  (length of per-texel offset_counts array)
///     u32      texel_lights_len   (length of flat light weights pool)
///     u32      slot_count          (length of slot_to_static_layer)
///
///   Chunk rects (24 bytes × chunk_count):
///     u32      atlas_x
///     u32      atlas_y
///     u32      width
///     u32      height
///     u32      texel_offset       (index into offset_counts)
///     u32      layer              (static lightmap atlas layer)
///
///   Offset table (8 bytes × offset_counts_len):
///     u32      offset             (into texel_lights)
///     u32      count
///
///   Light weights (12 bytes × texel_lights_len):
///     u32      light_index
///     f32      weight
///     u16      direction_oct[0]
///     u16      direction_oct[1]
///
///   Slot table (4 bytes × slot_count):
///     u32      static_layer       (slot index maps to this static atlas layer)
/// ```
///
/// Invariants verified at load time:
///   - chunk_count matches AnimatedLightChunksSection.chunks.len()
///   - offset_counts_len == Σ (chunk_rect.width × chunk_rect.height) for all chunks
///   - chunk_rect[i].texel_offset == Σ_{j<i} (chunk_rect[j].width × chunk_rect[j].height)
///   - All indices in texel_lights are within the animated-light descriptor array bounds.
///   - slot_to_static_layer is sorted, duplicate-free, and maps exactly the
///     static layers in chunk_rects.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimatedLightWeightMapsSection {
    pub chunk_rects: Vec<ChunkAtlasRect>,
    pub offset_counts: Vec<TexelLightEntry>,
    pub texel_lights: Vec<TexelLight>,
    /// Ascending static-atlas layers indexed by animated atlas slot.
    pub slot_to_static_layer: Vec<u32>,
}

const V2_HEADER_SIZE: usize = 16;
const HEADER_SIZE: usize = 20;
const V2_CHUNK_RECT_SIZE: usize = 20;
const CHUNK_RECT_SIZE: usize = 24;
const OFFSET_ENTRY_SIZE: usize = 8;
const TEXEL_LIGHT_SIZE: usize = 12;
const SLOT_STATIC_LAYER_SIZE: usize = 4;

impl AnimatedLightWeightMapsSection {
    /// Empty section — used when a map has no animated lights or no weight maps.
    pub fn empty() -> Self {
        Self {
            chunk_rects: Vec::new(),
            offset_counts: Vec::new(),
            texel_lights: Vec::new(),
            slot_to_static_layer: Vec::new(),
        }
    }

    /// Verify internal consistency: slots map bijectively to chunk layers,
    /// offset_counts length matches chunk area sum, chunk texel offsets form a
    /// valid partition, and every (offset, count) pair falls within
    /// `texel_lights`.
    pub fn is_consistent(&self) -> bool {
        if !self
            .slot_to_static_layer
            .windows(2)
            .all(|layers| layers[0] < layers[1])
        {
            return false;
        }

        if self.chunk_rects.iter().any(|chunk| {
            self.slot_to_static_layer
                .binary_search(&chunk.layer)
                .is_err()
        }) {
            return false;
        }

        if self
            .slot_to_static_layer
            .iter()
            .any(|layer| !self.chunk_rects.iter().any(|chunk| chunk.layer == *layer))
        {
            return false;
        }

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

        // Verify every (offset, count) pair is in bounds within texel_lights.
        // Use checked_add to avoid overflow before casting to usize.
        let texel_lights_len = self.texel_lights.len();
        for entry in &self.offset_counts {
            let end = match entry.offset.checked_add(entry.count) {
                Some(v) => v as usize,
                None => return false,
            };
            if end > texel_lights_len {
                return false;
            }
        }

        true
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(
            HEADER_SIZE
                + self.chunk_rects.len() * CHUNK_RECT_SIZE
                + self.offset_counts.len() * OFFSET_ENTRY_SIZE
                + self.texel_lights.len() * TEXEL_LIGHT_SIZE
                + self.slot_to_static_layer.len() * SLOT_STATIC_LAYER_SIZE,
        );

        buf.extend_from_slice(&ANIMATED_LIGHT_WEIGHT_MAPS_VERSION.to_le_bytes());
        buf.extend_from_slice(&(self.chunk_rects.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(self.offset_counts.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(self.texel_lights.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(self.slot_to_static_layer.len() as u32).to_le_bytes());

        for rect in &self.chunk_rects {
            buf.extend_from_slice(&rect.atlas_x.to_le_bytes());
            buf.extend_from_slice(&rect.atlas_y.to_le_bytes());
            buf.extend_from_slice(&rect.width.to_le_bytes());
            buf.extend_from_slice(&rect.height.to_le_bytes());
            buf.extend_from_slice(&rect.texel_offset.to_le_bytes());
            buf.extend_from_slice(&rect.layer.to_le_bytes());
        }

        for entry in &self.offset_counts {
            buf.extend_from_slice(&entry.offset.to_le_bytes());
            buf.extend_from_slice(&entry.count.to_le_bytes());
        }

        for light in &self.texel_lights {
            buf.extend_from_slice(&light.light_index.to_le_bytes());
            buf.extend_from_slice(&light.weight.to_le_bytes());
            buf.extend_from_slice(&light.direction_oct[0].to_le_bytes());
            buf.extend_from_slice(&light.direction_oct[1].to_le_bytes());
        }

        for static_layer in &self.slot_to_static_layer {
            buf.extend_from_slice(&static_layer.to_le_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < V2_HEADER_SIZE {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "animated light weight maps section too short for header",
            )));
        }

        let version = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let (header_size, chunk_rect_size, has_slot_table) = match version {
            ANIMATED_LIGHT_WEIGHT_MAPS_V2_VERSION => (V2_HEADER_SIZE, V2_CHUNK_RECT_SIZE, false),
            ANIMATED_LIGHT_WEIGHT_MAPS_VERSION => (HEADER_SIZE, CHUNK_RECT_SIZE, true),
            _ => {
                return Err(FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("animated light weight maps section unsupported version {version}"),
                )));
            }
        };

        if data.len() < header_size {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "animated light weight maps section too short for header",
            )));
        }

        let chunk_count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let offset_counts_len = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
        let texel_lights_len =
            u32::from_le_bytes([data[12], data[13], data[14], data[15]]) as usize;
        let slot_count = if has_slot_table {
            u32::from_le_bytes([data[16], data[17], data[18], data[19]]) as usize
        } else {
            0
        };

        let needed = header_size
            .checked_add(chunk_count.checked_mul(chunk_rect_size).ok_or_else(|| {
                FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "animated light weight maps section size overflows",
                ))
            })?)
            .and_then(|size| size.checked_add(offset_counts_len.checked_mul(OFFSET_ENTRY_SIZE)?))
            .and_then(|size| size.checked_add(texel_lights_len.checked_mul(TEXEL_LIGHT_SIZE)?))
            .and_then(|size| size.checked_add(slot_count.checked_mul(SLOT_STATIC_LAYER_SIZE)?))
            .ok_or_else(|| {
                FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "animated light weight maps section size overflows",
                ))
            })?;

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
        let mut cursor = header_size;
        for _ in 0..chunk_count {
            let atlas_x = read_u32(data, cursor);
            let atlas_y = read_u32(data, cursor + 4);
            let width = read_u32(data, cursor + 8);
            let height = read_u32(data, cursor + 12);
            let texel_offset = read_u32(data, cursor + 16);
            let layer = if has_slot_table {
                read_u32(data, cursor + 20)
            } else {
                0
            };

            chunk_rects.push(ChunkAtlasRect {
                atlas_x,
                atlas_y,
                width,
                height,
                texel_offset,
                layer,
            });
            cursor += chunk_rect_size;
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
            let dx = read_u16(data, cursor + 8);
            let dy = read_u16(data, cursor + 10);
            texel_lights.push(TexelLight {
                light_index,
                weight,
                direction_oct: [dx, dy],
            });
            cursor += TEXEL_LIGHT_SIZE;
        }

        let slot_to_static_layer = if has_slot_table {
            let mut slots = Vec::with_capacity(slot_count);
            for _ in 0..slot_count {
                slots.push(read_u32(data, cursor));
                cursor += SLOT_STATIC_LAYER_SIZE;
            }
            slots
        } else if chunk_rects.is_empty() {
            Vec::new()
        } else {
            vec![0]
        };

        Ok(Self {
            chunk_rects,
            offset_counts,
            texel_lights,
            slot_to_static_layer,
        })
    }
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_f32(data: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_u16(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
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
                    layer: 3,
                },
                ChunkAtlasRect {
                    atlas_x: 2,
                    atlas_y: 0,
                    width: 3,
                    height: 1,
                    texel_offset: 4,
                    layer: 11,
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
                    direction_oct: [32768, 65535],
                },
                TexelLight {
                    light_index: 1,
                    weight: 0.2,
                    direction_oct: [65535, 32768],
                },
                TexelLight {
                    light_index: 2,
                    weight: 1.0,
                    direction_oct: [0, 32768],
                },
                TexelLight {
                    light_index: 3,
                    weight: 0.5,
                    direction_oct: [32768, 0],
                },
                TexelLight {
                    light_index: 4,
                    weight: 0.6,
                    direction_oct: [32768, 32768],
                },
                TexelLight {
                    light_index: 5,
                    weight: 0.3,
                    direction_oct: [16384, 49152],
                },
                TexelLight {
                    light_index: 6,
                    weight: 0.9,
                    direction_oct: [49152, 16384],
                },
                TexelLight {
                    light_index: 7,
                    weight: 0.4,
                    direction_oct: [12345, 54321],
                },
            ],
            slot_to_static_layer: vec![3, 11],
        }
    }

    fn v2_bytes(section: &AnimatedLightWeightMapsSection) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&ANIMATED_LIGHT_WEIGHT_MAPS_V2_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(section.chunk_rects.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(section.offset_counts.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(section.texel_lights.len() as u32).to_le_bytes());

        for rect in &section.chunk_rects {
            bytes.extend_from_slice(&rect.atlas_x.to_le_bytes());
            bytes.extend_from_slice(&rect.atlas_y.to_le_bytes());
            bytes.extend_from_slice(&rect.width.to_le_bytes());
            bytes.extend_from_slice(&rect.height.to_le_bytes());
            bytes.extend_from_slice(&rect.texel_offset.to_le_bytes());
        }

        for entry in &section.offset_counts {
            bytes.extend_from_slice(&entry.offset.to_le_bytes());
            bytes.extend_from_slice(&entry.count.to_le_bytes());
        }

        for light in &section.texel_lights {
            bytes.extend_from_slice(&light.light_index.to_le_bytes());
            bytes.extend_from_slice(&light.weight.to_le_bytes());
            bytes.extend_from_slice(&light.direction_oct[0].to_le_bytes());
            bytes.extend_from_slice(&light.direction_oct[1].to_le_bytes());
        }

        bytes
    }

    #[test]
    fn v3_multi_slot_round_trip_is_byte_identical() {
        let section = sample_section();
        let bytes = section.to_bytes();
        assert_eq!(
            &bytes[0..4],
            &ANIMATED_LIGHT_WEIGHT_MAPS_VERSION.to_le_bytes()
        );
        assert_eq!(&bytes[16..20], &2_u32.to_le_bytes());
        let restored = AnimatedLightWeightMapsSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
        let rebytes = restored.to_bytes();
        assert_eq!(bytes, rebytes);
    }

    #[test]
    fn v3_appends_chunk_layer_and_slot_table() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let first_layer_offset = HEADER_SIZE + 20;
        let slot_table_offset = HEADER_SIZE
            + section.chunk_rects.len() * CHUNK_RECT_SIZE
            + section.offset_counts.len() * OFFSET_ENTRY_SIZE
            + section.texel_lights.len() * TEXEL_LIGHT_SIZE;

        assert_eq!(
            &bytes[first_layer_offset..first_layer_offset + 4],
            &3_u32.to_le_bytes()
        );
        assert_eq!(
            &bytes[slot_table_offset..],
            &[3_u32.to_le_bytes(), 11_u32.to_le_bytes()].concat()
        );
    }

    #[test]
    fn v3_single_slot_round_trip_preserves_layer() {
        let mut section = sample_section();
        section.chunk_rects[1].layer = 3;
        section.slot_to_static_layer = vec![3];

        let restored = AnimatedLightWeightMapsSection::from_bytes(&section.to_bytes()).unwrap();
        assert_eq!(restored, section);
    }

    #[test]
    fn v2_nonempty_decode_defaults_chunks_to_layer_zero_and_one_slot() {
        let section = sample_section();
        let mut expected = section.clone();
        expected
            .chunk_rects
            .iter_mut()
            .for_each(|chunk| chunk.layer = 0);
        expected.slot_to_static_layer = vec![0];

        assert_eq!(
            AnimatedLightWeightMapsSection::from_bytes(&v2_bytes(&section)).unwrap(),
            expected
        );
    }

    #[test]
    fn v2_empty_decode_has_no_slots() {
        let bytes = v2_bytes(&AnimatedLightWeightMapsSection::empty());
        assert_eq!(bytes.len(), V2_HEADER_SIZE);
        assert_eq!(
            AnimatedLightWeightMapsSection::from_bytes(&bytes).unwrap(),
            AnimatedLightWeightMapsSection::empty()
        );
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
            + section.texel_lights.len() * TEXEL_LIGHT_SIZE
            + section.slot_to_static_layer.len() * SLOT_STATIC_LAYER_SIZE;
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
    fn consistency_check_fails_when_chunk_layer_has_no_slot() {
        let mut section = sample_section();
        section.chunk_rects[1].layer = 9;
        assert!(!section.is_consistent());
    }

    #[test]
    fn consistency_check_fails_when_slot_table_is_unsorted() {
        let mut section = sample_section();
        section.slot_to_static_layer = vec![11, 3];
        assert!(!section.is_consistent());
    }

    #[test]
    fn consistency_check_fails_when_slot_table_has_duplicates() {
        let mut section = sample_section();
        section.slot_to_static_layer = vec![3, 3, 11];
        assert!(!section.is_consistent());
    }

    #[test]
    fn consistency_check_fails_when_slot_table_has_unoccupied_layer() {
        let mut section = sample_section();
        section.slot_to_static_layer = vec![3, 7, 11];
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
        let message = err.to_string();
        assert!(message.contains("unsupported version"));
        assert!(message.contains("999"));
        match err {
            FormatError::Io(error) => assert_eq!(error.kind(), std::io::ErrorKind::InvalidData),
            _ => unreachable!("only I/O format errors are returned by this decoder"),
        }
    }

    #[test]
    fn offset_counts_length_matches_chunk_area_sum() {
        let section = sample_section();
        let expected_len: u32 = section.chunk_rects.iter().map(|r| r.width * r.height).sum();
        assert_eq!(section.offset_counts.len() as u32, expected_len);
    }

    #[test]
    fn direction_oct_round_trips_per_texel_light() {
        // Each per-texel light entry must carry its octahedral-encoded direction
        // through serialize → deserialize. Task 2b of sdf-static-occluder-shadows
        // bakes this; the compose pass reads it to fuse a per-frame dominant
        // direction.
        let section = sample_section();
        let bytes = section.to_bytes();
        let restored = AnimatedLightWeightMapsSection::from_bytes(&bytes).unwrap();
        assert_eq!(section.texel_lights.len(), restored.texel_lights.len());
        for (a, b) in section
            .texel_lights
            .iter()
            .zip(restored.texel_lights.iter())
        {
            assert_eq!(a.direction_oct, b.direction_oct);
        }
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
