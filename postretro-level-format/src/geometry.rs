// Geometry section data types: vertex/index buffers, per-face metadata.
// See: context/lib/build_pipeline.md §PRL

use crate::FormatError;

/// Per-face metadata referencing into the index buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceMeta {
    /// Index into the index array where this face's triangle indices begin.
    pub index_offset: u32,
    /// Number of indices (always a multiple of 3).
    pub index_count: u32,
    /// Sequential empty-leaf index this face belongs to.
    pub leaf_index: u32,
}

/// Geometry section: vertex positions, triangle indices, and per-face metadata.
///
/// **Leaf ordering invariant:** faces are ordered contiguously by BSP leaf.
/// All faces for leaf 0 come first, then all faces for leaf 1, etc. Only empty
/// leaves contribute faces; solid leaves are skipped. The `face_start` /
/// `face_count` fields in `BspLeavesSection` index into this ordering, enabling
/// per-leaf draw calls without an index lookup.
#[derive(Debug, Clone, PartialEq)]
pub struct GeometrySection {
    pub vertices: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub faces: Vec<FaceMeta>,
}

// On-disk layout (all little-endian):
//   u32  vertex_count
//   u32  index_count
//   u32  face_count
//   [f32; 3] * vertex_count   (vertex positions)
//   u32 * index_count          (triangle indices)
//   FaceMeta * face_count      (12 bytes each: offset, count, leaf_index)

impl GeometrySection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let vertex_count = self.vertices.len() as u32;
        let index_count = self.indices.len() as u32;
        let face_count = self.faces.len() as u32;

        let size =
            12 + (self.vertices.len() * 12) + (self.indices.len() * 4) + (self.faces.len() * 12);
        let mut buf = Vec::with_capacity(size);

        buf.extend_from_slice(&vertex_count.to_le_bytes());
        buf.extend_from_slice(&index_count.to_le_bytes());
        buf.extend_from_slice(&face_count.to_le_bytes());

        for v in &self.vertices {
            buf.extend_from_slice(&v[0].to_le_bytes());
            buf.extend_from_slice(&v[1].to_le_bytes());
            buf.extend_from_slice(&v[2].to_le_bytes());
        }

        for &idx in &self.indices {
            buf.extend_from_slice(&idx.to_le_bytes());
        }

        for face in &self.faces {
            buf.extend_from_slice(&face.index_offset.to_le_bytes());
            buf.extend_from_slice(&face.index_count.to_le_bytes());
            buf.extend_from_slice(&face.leaf_index.to_le_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 12 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "geometry section too short for header",
            )));
        }

        let vertex_count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let index_count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let face_count = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;

        let expected_size = 12 + (vertex_count * 12) + (index_count * 4) + (face_count * 12);
        if data.len() < expected_size {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "geometry section too short: need {expected_size} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let mut offset = 12;

        let mut vertices = Vec::with_capacity(vertex_count);
        for _ in 0..vertex_count {
            let x = f32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            let y = f32::from_le_bytes([
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]);
            let z = f32::from_le_bytes([
                data[offset + 8],
                data[offset + 9],
                data[offset + 10],
                data[offset + 11],
            ]);
            vertices.push([x, y, z]);
            offset += 12;
        }

        let mut indices = Vec::with_capacity(index_count);
        for _ in 0..index_count {
            let idx = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            indices.push(idx);
            offset += 4;
        }

        let mut faces = Vec::with_capacity(face_count);
        for _ in 0..face_count {
            let index_offset = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            let index_count = u32::from_le_bytes([
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]);
            let leaf_index = u32::from_le_bytes([
                data[offset + 8],
                data[offset + 9],
                data[offset + 10],
                data[offset + 11],
            ]);
            faces.push(FaceMeta {
                index_offset,
                index_count,
                leaf_index,
            });
            offset += 12;
        }

        Ok(Self {
            vertices,
            indices,
            faces,
        })
    }
}

/// Sentinel value for `FaceMetaV2.texture_index`: face has no texture (checkerboard fallback).
pub const NO_TEXTURE: u32 = u32::MAX;

/// Per-face metadata for GeometryV2, with texture index.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceMetaV2 {
    /// Index into the index array where this face's triangle indices begin.
    pub index_offset: u32,
    /// Number of indices (always a multiple of 3).
    pub index_count: u32,
    /// Sequential empty-leaf index this face belongs to.
    pub leaf_index: u32,
    /// Index into the TextureNames section. `NO_TEXTURE` (u32::MAX) = no texture.
    pub texture_index: u32,
}

/// Extended geometry section: 5-float vertices (position + UV) and FaceMetaV2.
///
/// **Leaf ordering invariant:** same as GeometrySection -- faces are ordered
/// contiguously by BSP leaf.
///
/// On-disk layout (all little-endian):
///   u32  vertex_count
///   u32  index_count
///   u32  face_count
///   [f32; 5] * vertex_count   (x, y, z, u, v)
///   u32 * index_count          (triangle indices)
///   FaceMetaV2 * face_count   (16 bytes each: offset, count, leaf_index, texture_index)
#[derive(Debug, Clone, PartialEq)]
pub struct GeometrySectionV2 {
    pub vertices: Vec<[f32; 5]>,
    pub indices: Vec<u32>,
    pub faces: Vec<FaceMetaV2>,
}

impl GeometrySectionV2 {
    pub fn to_bytes(&self) -> Vec<u8> {
        let vertex_count = self.vertices.len() as u32;
        let index_count = self.indices.len() as u32;
        let face_count = self.faces.len() as u32;

        let size =
            12 + (self.vertices.len() * 20) + (self.indices.len() * 4) + (self.faces.len() * 16);
        let mut buf = Vec::with_capacity(size);

        buf.extend_from_slice(&vertex_count.to_le_bytes());
        buf.extend_from_slice(&index_count.to_le_bytes());
        buf.extend_from_slice(&face_count.to_le_bytes());

        for v in &self.vertices {
            buf.extend_from_slice(&v[0].to_le_bytes());
            buf.extend_from_slice(&v[1].to_le_bytes());
            buf.extend_from_slice(&v[2].to_le_bytes());
            buf.extend_from_slice(&v[3].to_le_bytes());
            buf.extend_from_slice(&v[4].to_le_bytes());
        }

        for &idx in &self.indices {
            buf.extend_from_slice(&idx.to_le_bytes());
        }

        for face in &self.faces {
            buf.extend_from_slice(&face.index_offset.to_le_bytes());
            buf.extend_from_slice(&face.index_count.to_le_bytes());
            buf.extend_from_slice(&face.leaf_index.to_le_bytes());
            buf.extend_from_slice(&face.texture_index.to_le_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 12 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "geometry v2 section too short for header",
            )));
        }

        let vertex_count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let index_count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let face_count = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;

        let expected_size = 12 + (vertex_count * 20) + (index_count * 4) + (face_count * 16);
        if data.len() < expected_size {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "geometry v2 section too short: need {expected_size} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let mut offset = 12;

        let mut vertices = Vec::with_capacity(vertex_count);
        for _ in 0..vertex_count {
            let x = f32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            let y = f32::from_le_bytes([
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]);
            let z = f32::from_le_bytes([
                data[offset + 8],
                data[offset + 9],
                data[offset + 10],
                data[offset + 11],
            ]);
            let u = f32::from_le_bytes([
                data[offset + 12],
                data[offset + 13],
                data[offset + 14],
                data[offset + 15],
            ]);
            let v = f32::from_le_bytes([
                data[offset + 16],
                data[offset + 17],
                data[offset + 18],
                data[offset + 19],
            ]);
            vertices.push([x, y, z, u, v]);
            offset += 20;
        }

        let mut indices = Vec::with_capacity(index_count);
        for _ in 0..index_count {
            let idx = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            indices.push(idx);
            offset += 4;
        }

        let mut faces = Vec::with_capacity(face_count);
        for _ in 0..face_count {
            let index_offset = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            let index_count = u32::from_le_bytes([
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]);
            let leaf_index = u32::from_le_bytes([
                data[offset + 8],
                data[offset + 9],
                data[offset + 10],
                data[offset + 11],
            ]);
            let texture_index = u32::from_le_bytes([
                data[offset + 12],
                data[offset + 13],
                data[offset + 14],
                data[offset + 15],
            ]);
            faces.push(FaceMetaV2 {
                index_offset,
                index_count,
                leaf_index,
                texture_index,
            });
            offset += 16;
        }

        Ok(Self {
            vertices,
            indices,
            faces,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_section() -> GeometrySection {
        GeometrySection {
            vertices: vec![
                [1.0, 2.0, 3.0],
                [4.0, 5.0, 6.0],
                [7.0, 8.0, 9.0],
                [10.0, 11.0, 12.0],
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            faces: vec![
                FaceMeta {
                    index_offset: 0,
                    index_count: 3,
                    leaf_index: 0,
                },
                FaceMeta {
                    index_offset: 3,
                    index_count: 3,
                    leaf_index: 1,
                },
            ],
        }
    }

    #[test]
    fn round_trip() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let restored = GeometrySection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn byte_layout_header() {
        let section = sample_section();
        let bytes = section.to_bytes();

        let vertex_count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let index_count = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let face_count = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);

        assert_eq!(vertex_count, 4);
        assert_eq!(index_count, 6);
        assert_eq!(face_count, 2);
    }

    #[test]
    fn rejects_truncated_data() {
        let result = GeometrySection::from_bytes(&[0; 8]);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_short_body() {
        // Header claims 1 vertex, 0 indices, 0 faces but body is missing
        let mut data = vec![0u8; 12];
        data[0] = 1; // vertex_count = 1
        let result = GeometrySection::from_bytes(&data);
        assert!(result.is_err());
    }

    #[test]
    fn empty_section_round_trips() {
        let section = GeometrySection {
            vertices: vec![],
            indices: vec![],
            faces: vec![],
        };
        let bytes = section.to_bytes();
        let restored = GeometrySection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    // -- GeometrySectionV2 tests --

    fn sample_v2_section() -> GeometrySectionV2 {
        GeometrySectionV2 {
            vertices: vec![
                [1.0, 2.0, 3.0, 0.25, 0.75],
                [4.0, 5.0, 6.0, 0.5, 0.0],
                [7.0, 8.0, 9.0, 1.0, 1.0],
                [10.0, 11.0, 12.0, 0.0, 0.5],
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            faces: vec![
                FaceMetaV2 {
                    index_offset: 0,
                    index_count: 3,
                    leaf_index: 0,
                    texture_index: 0,
                },
                FaceMetaV2 {
                    index_offset: 3,
                    index_count: 3,
                    leaf_index: 1,
                    texture_index: 2,
                },
            ],
        }
    }

    #[test]
    fn v2_round_trip() {
        let section = sample_v2_section();
        let bytes = section.to_bytes();
        let restored = GeometrySectionV2::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn v2_byte_layout_header() {
        let section = sample_v2_section();
        let bytes = section.to_bytes();

        let vertex_count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let index_count = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let face_count = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);

        assert_eq!(vertex_count, 4);
        assert_eq!(index_count, 6);
        assert_eq!(face_count, 2);
    }

    #[test]
    fn v2_uv_coordinates_preserved() {
        let section = sample_v2_section();
        let bytes = section.to_bytes();
        let restored = GeometrySectionV2::from_bytes(&bytes).unwrap();

        // Verify UV coordinates survived the round-trip
        assert_eq!(restored.vertices[0][3], 0.25);
        assert_eq!(restored.vertices[0][4], 0.75);
        assert_eq!(restored.vertices[2][3], 1.0);
        assert_eq!(restored.vertices[2][4], 1.0);
    }

    #[test]
    fn v2_texture_index_preserved() {
        let section = sample_v2_section();
        let bytes = section.to_bytes();
        let restored = GeometrySectionV2::from_bytes(&bytes).unwrap();

        assert_eq!(restored.faces[0].texture_index, 0);
        assert_eq!(restored.faces[1].texture_index, 2);
    }

    #[test]
    fn v2_no_texture_sentinel_round_trips() {
        let section = GeometrySectionV2 {
            vertices: vec![[0.0, 0.0, 0.0, 0.0, 0.0]],
            indices: vec![],
            faces: vec![FaceMetaV2 {
                index_offset: 0,
                index_count: 0,
                leaf_index: 0,
                texture_index: NO_TEXTURE,
            }],
        };
        let bytes = section.to_bytes();
        let restored = GeometrySectionV2::from_bytes(&bytes).unwrap();
        assert_eq!(restored.faces[0].texture_index, u32::MAX);
    }

    #[test]
    fn v2_empty_section_round_trips() {
        let section = GeometrySectionV2 {
            vertices: vec![],
            indices: vec![],
            faces: vec![],
        };
        let bytes = section.to_bytes();
        let restored = GeometrySectionV2::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn v2_rejects_truncated_data() {
        let result = GeometrySectionV2::from_bytes(&[0; 8]);
        assert!(result.is_err());
    }

    #[test]
    fn v2_rejects_short_body() {
        // Header claims 1 vertex, 0 indices, 0 faces but body is missing
        let mut data = vec![0u8; 12];
        data[0] = 1; // vertex_count = 1
        let result = GeometrySectionV2::from_bytes(&data);
        assert!(result.is_err());
    }
}
