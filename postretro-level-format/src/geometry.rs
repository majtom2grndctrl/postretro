// Geometry section data types: vertex/index buffers, per-face metadata.
// See: context/lib/build_pipeline.md §PRL, rendering_pipeline.md §6

use crate::FormatError;
use crate::octahedral;

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

/// 28-byte vertex for GeometryV3: position (f32x3) + UV (f32x2) + octahedral
/// normal (u16x2) + octahedral tangent with bitangent sign (u16x2).
///
/// The bitangent sign is packed into the MSB of `tangent_packed[1]`: the lower
/// 15 bits hold the octahedral v-component, and bit 15 is 1 for positive
/// bitangent sign, 0 for negative. The tangent's v-component is encoded at
/// 15-bit precision to make room for the sign bit.
#[derive(Debug, Clone, PartialEq)]
pub struct VertexV3 {
    /// World-space position (Y-up, engine meters).
    pub position: [f32; 3],
    /// Texture-space UV, normalized by texture dimensions.
    pub uv: [f32; 2],
    /// Octahedral-encoded unit normal, quantized to u16x2.
    pub normal_oct: [u16; 2],
    /// Packed tangent: [0] is full u16 octahedral u-component, [1] has
    /// bitangent sign in bit 15 and 15-bit octahedral v-component in bits 0..14.
    pub tangent_packed: [u16; 2],
}

impl VertexV3 {
    /// Create a vertex from floating-point normal and tangent vectors.
    /// Encodes normal and tangent to octahedral representation, packing the
    /// bitangent sign into the tangent's v-component MSB.
    ///
    /// Note: the tangent v-component is encoded at 15-bit precision (not the
    /// 16 bits used for the normal) because the high bit of `tangent_packed[1]`
    /// carries the bitangent sign. This asymmetry is flagged as a Phase 4 risk
    /// — see "Octahedral precision insufficient for Phase 4 specular" in
    /// `context/plans/drafts/phase-4-baked-lighting/index.md`.
    pub fn new(
        position: [f32; 3],
        uv: [f32; 2],
        normal: [f32; 3],
        tangent: [f32; 3],
        bitangent_sign: bool,
    ) -> Self {
        let normal_oct = octahedral::encode(normal[0], normal[1], normal[2]);
        let tangent_oct = octahedral::encode(tangent[0], tangent[1], tangent[2]);
        // Remap tangent v from [0, 65535] to [0, 32767] and pack sign in MSB
        let tangent_v_15bit = (tangent_oct[1] as u32 * 32767 / 65535) as u16;
        let sign_bit: u16 = if bitangent_sign { 0x8000 } else { 0 };
        let tangent_packed = [tangent_oct[0], tangent_v_15bit | sign_bit];
        Self {
            position,
            uv,
            normal_oct,
            tangent_packed,
        }
    }

    /// Decode the octahedral normal back to a unit vector.
    pub fn decode_normal(&self) -> [f32; 3] {
        octahedral::decode(self.normal_oct)
    }

    /// Decode the octahedral tangent back to a unit vector.
    pub fn decode_tangent(&self) -> [f32; 3] {
        // Unpack: strip the sign bit, remap 15-bit v back to 16-bit range
        let v_15bit = self.tangent_packed[1] & 0x7FFF;
        let v_16bit = (v_15bit as u32 * 65535 / 32767) as u16;
        octahedral::decode([self.tangent_packed[0], v_16bit])
    }

    /// Extract the bitangent sign. True = positive, false = negative.
    pub fn bitangent_sign(&self) -> bool {
        (self.tangent_packed[1] & 0x8000) != 0
    }
}

/// Per-face metadata for GeometryV3 — same as V2 (texture index per face).
pub type FaceMetaV3 = FaceMetaV2;

/// Extended geometry section V3: 28-byte vertices with packed normals/tangents.
///
/// On-disk layout (all little-endian):
///   u32  vertex_count
///   u32  index_count
///   u32  face_count
///   VertexV3 * vertex_count  (28 bytes each, see below)
///   u32 * index_count         (triangle indices)
///   FaceMetaV3 * face_count  (16 bytes each: offset, count, leaf_index, texture_index)
///
/// Per-vertex on-disk (28 bytes):
///   f32 x, f32 y, f32 z     (position, 12 bytes)
///   f32 u, f32 v             (UV, 8 bytes)
///   u16 normal_u, u16 normal_v  (octahedral normal, 4 bytes)
///   u16 tangent_u, u16 tangent_v_with_sign  (octahedral tangent + bitangent sign in MSB of v, 4 bytes total = 28 bytes/vertex)
#[derive(Debug, Clone, PartialEq)]
pub struct GeometrySectionV3 {
    pub vertices: Vec<VertexV3>,
    pub indices: Vec<u32>,
    pub faces: Vec<FaceMetaV3>,
}

const V3_VERTEX_SIZE: usize = 28;
const V3_FACE_SIZE: usize = 16;

impl GeometrySectionV3 {
    pub fn to_bytes(&self) -> Vec<u8> {
        let vertex_count = self.vertices.len() as u32;
        let index_count = self.indices.len() as u32;
        let face_count = self.faces.len() as u32;

        let size = 12
            + (self.vertices.len() * V3_VERTEX_SIZE)
            + (self.indices.len() * 4)
            + (self.faces.len() * V3_FACE_SIZE);
        let mut buf = Vec::with_capacity(size);

        buf.extend_from_slice(&vertex_count.to_le_bytes());
        buf.extend_from_slice(&index_count.to_le_bytes());
        buf.extend_from_slice(&face_count.to_le_bytes());

        for v in &self.vertices {
            buf.extend_from_slice(&v.position[0].to_le_bytes());
            buf.extend_from_slice(&v.position[1].to_le_bytes());
            buf.extend_from_slice(&v.position[2].to_le_bytes());
            buf.extend_from_slice(&v.uv[0].to_le_bytes());
            buf.extend_from_slice(&v.uv[1].to_le_bytes());
            buf.extend_from_slice(&v.normal_oct[0].to_le_bytes());
            buf.extend_from_slice(&v.normal_oct[1].to_le_bytes());
            buf.extend_from_slice(&v.tangent_packed[0].to_le_bytes());
            buf.extend_from_slice(&v.tangent_packed[1].to_le_bytes());
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
                "geometry v3 section too short for header",
            )));
        }

        let vertex_count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let index_count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let face_count = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;

        let expected_size =
            12 + (vertex_count * V3_VERTEX_SIZE) + (index_count * 4) + (face_count * V3_FACE_SIZE);
        if data.len() < expected_size {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "geometry v3 section too short: need {expected_size} bytes, got {}",
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
            let normal_u =
                u16::from_le_bytes([data[offset + 20], data[offset + 21]]);
            let normal_v =
                u16::from_le_bytes([data[offset + 22], data[offset + 23]]);
            let tangent_u =
                u16::from_le_bytes([data[offset + 24], data[offset + 25]]);
            let tangent_v_with_sign =
                u16::from_le_bytes([data[offset + 26], data[offset + 27]]);

            vertices.push(VertexV3 {
                position: [x, y, z],
                uv: [u, v],
                normal_oct: [normal_u, normal_v],
                tangent_packed: [tangent_u, tangent_v_with_sign],
            });
            offset += V3_VERTEX_SIZE;
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
            faces.push(FaceMetaV3 {
                index_offset,
                index_count,
                leaf_index,
                texture_index,
            });
            offset += V3_FACE_SIZE;
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

    // -- GeometrySectionV3 tests --

    fn sample_v3_section() -> GeometrySectionV3 {
        GeometrySectionV3 {
            vertices: vec![
                VertexV3::new(
                    [1.0, 2.0, 3.0],
                    [0.25, 0.75],
                    [0.0, 1.0, 0.0],
                    [1.0, 0.0, 0.0],
                    true,
                ),
                VertexV3::new(
                    [4.0, 5.0, 6.0],
                    [0.5, 0.0],
                    [0.0, 0.0, 1.0],
                    [1.0, 0.0, 0.0],
                    false,
                ),
                VertexV3::new(
                    [7.0, 8.0, 9.0],
                    [1.0, 1.0],
                    [0.0, 1.0, 0.0],
                    [0.0, 0.0, 1.0],
                    true,
                ),
            ],
            indices: vec![0, 1, 2],
            faces: vec![FaceMetaV3 {
                index_offset: 0,
                index_count: 3,
                leaf_index: 0,
                texture_index: 5,
            }],
        }
    }

    #[test]
    fn v3_round_trip_preserves_all_fields() {
        let section = sample_v3_section();
        let bytes = section.to_bytes();
        let restored = GeometrySectionV3::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn v3_vertex_is_28_bytes() {
        // 3 vertices should produce header(12) + verts(3*28) + indices(3*4) + faces(1*16)
        let section = sample_v3_section();
        let bytes = section.to_bytes();
        let expected = 12 + (3 * 28) + (3 * 4) + (1 * 16);
        assert_eq!(bytes.len(), expected);
    }

    #[test]
    fn v3_position_and_uv_preserved_exactly() {
        let section = sample_v3_section();
        let bytes = section.to_bytes();
        let restored = GeometrySectionV3::from_bytes(&bytes).unwrap();

        assert_eq!(restored.vertices[0].position, [1.0, 2.0, 3.0]);
        assert_eq!(restored.vertices[0].uv, [0.25, 0.75]);
        assert_eq!(restored.vertices[1].position, [4.0, 5.0, 6.0]);
        assert_eq!(restored.vertices[1].uv, [0.5, 0.0]);
    }

    #[test]
    fn v3_bitangent_sign_round_trips() {
        let section = sample_v3_section();
        let bytes = section.to_bytes();
        let restored = GeometrySectionV3::from_bytes(&bytes).unwrap();

        assert!(restored.vertices[0].bitangent_sign());  // true
        assert!(!restored.vertices[1].bitangent_sign()); // false
        assert!(restored.vertices[2].bitangent_sign());  // true
    }

    #[test]
    fn v3_normal_tangent_decode_close_to_input() {
        let section = sample_v3_section();
        let bytes = section.to_bytes();
        let restored = GeometrySectionV3::from_bytes(&bytes).unwrap();

        // +Y normal
        let n = restored.vertices[0].decode_normal();
        assert!((n[1] - 1.0).abs() < 0.001, "expected ~+Y normal, got {:?}", n);

        // +X tangent
        let t = restored.vertices[0].decode_tangent();
        assert!((t[0] - 1.0).abs() < 0.001, "expected ~+X tangent, got {:?}", t);
    }

    #[test]
    fn v3_face_meta_preserved() {
        let section = sample_v3_section();
        let bytes = section.to_bytes();
        let restored = GeometrySectionV3::from_bytes(&bytes).unwrap();

        assert_eq!(restored.faces[0].index_offset, 0);
        assert_eq!(restored.faces[0].index_count, 3);
        assert_eq!(restored.faces[0].leaf_index, 0);
        assert_eq!(restored.faces[0].texture_index, 5);
    }

    #[test]
    fn v3_empty_section_round_trips() {
        let section = GeometrySectionV3 {
            vertices: vec![],
            indices: vec![],
            faces: vec![],
        };
        let bytes = section.to_bytes();
        let restored = GeometrySectionV3::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn v3_rejects_truncated_data() {
        let result = GeometrySectionV3::from_bytes(&[0; 8]);
        assert!(result.is_err());
    }

    #[test]
    fn v3_rejects_short_body() {
        let mut data = vec![0u8; 12];
        data[0] = 1; // vertex_count = 1, but body missing
        let result = GeometrySectionV3::from_bytes(&data);
        assert!(result.is_err());
    }

    #[test]
    fn v3_no_texture_sentinel_round_trips() {
        let section = GeometrySectionV3 {
            vertices: vec![VertexV3::new(
                [0.0, 0.0, 0.0],
                [0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
            )],
            indices: vec![],
            faces: vec![FaceMetaV3 {
                index_offset: 0,
                index_count: 0,
                leaf_index: 0,
                texture_index: NO_TEXTURE,
            }],
        };
        let bytes = section.to_bytes();
        let restored = GeometrySectionV3::from_bytes(&bytes).unwrap();
        assert_eq!(restored.faces[0].texture_index, u32::MAX);
    }
}
