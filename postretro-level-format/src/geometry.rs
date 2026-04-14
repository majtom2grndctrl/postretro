// Geometry section data types: vertex/index buffers, per-face metadata.
// See: context/lib/build_pipeline.md §PRL, rendering_pipeline.md §6

use crate::FormatError;
use crate::octahedral;

/// Sentinel value for `FaceMeta.texture_index`: face has no texture (checkerboard fallback).
pub const NO_TEXTURE: u32 = u32::MAX;

/// 28-byte vertex: position (f32x3) + UV (f32x2) + octahedral normal (u16x2)
/// + octahedral tangent with bitangent sign (u16x2).
///
/// The bitangent sign is packed into the MSB of `tangent_packed[1]`: the lower
/// 15 bits hold the octahedral v-component, and bit 15 is 1 for positive
/// bitangent sign, 0 for negative. The tangent's v-component is encoded at
/// 15-bit precision to make room for the sign bit.
#[derive(Debug, Clone, PartialEq)]
pub struct Vertex {
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

impl Vertex {
    /// Create a vertex from floating-point normal and tangent vectors.
    /// Encodes normal and tangent to octahedral representation, packing the
    /// bitangent sign into the tangent's v-component MSB.
    ///
    /// Note: the tangent v-component is encoded at 15-bit precision (not the
    /// 16 bits used for the normal) because the high bit of `tangent_packed[1]`
    /// carries the bitangent sign.
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

/// Per-face metadata. Face → index-range mapping is owned by the `Bvh` section;
/// `FaceMeta` carries only the per-face attributes the renderer needs to resolve
/// cells and textures.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceMeta {
    /// Raw BSP leaf index this face belongs to (= runtime cell id).
    pub leaf_index: u32,
    /// Index into the TextureNames section. `NO_TEXTURE` (u32::MAX) = no texture.
    pub texture_index: u32,
}

/// Geometry section: packed vertices, triangle indices, and per-face metadata.
///
/// On-disk layout (all little-endian):
///   u32  vertex_count
///   u32  index_count
///   u32  face_count
///   Vertex   * vertex_count    (28 bytes each)
///   u32      * index_count      (triangle indices)
///   FaceMeta * face_count       (8 bytes each: leaf_index, texture_index)
///
/// Per-vertex on-disk (28 bytes):
///   f32 x, f32 y, f32 z                     (position, 12 bytes)
///   f32 u, f32 v                             (UV, 8 bytes)
///   u16 normal_u, u16 normal_v               (octahedral normal, 4 bytes)
///   u16 tangent_u, u16 tangent_v_with_sign   (octahedral tangent + sign, 4 bytes)
#[derive(Debug, Clone, PartialEq)]
pub struct GeometrySection {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub faces: Vec<FaceMeta>,
}

const VERTEX_SIZE: usize = 28;
const FACE_SIZE: usize = 8;
const HEADER_SIZE: usize = 12;

impl GeometrySection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let vertex_count = self.vertices.len() as u32;
        let index_count = self.indices.len() as u32;
        let face_count = self.faces.len() as u32;

        let size = HEADER_SIZE
            + (self.vertices.len() * VERTEX_SIZE)
            + (self.indices.len() * 4)
            + (self.faces.len() * FACE_SIZE);
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
            buf.extend_from_slice(&face.leaf_index.to_le_bytes());
            buf.extend_from_slice(&face.texture_index.to_le_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "geometry section too short for header",
            )));
        }

        let vertex_count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let index_count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let face_count = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;

        let expected_size = HEADER_SIZE
            + (vertex_count * VERTEX_SIZE)
            + (index_count * 4)
            + (face_count * FACE_SIZE);
        if data.len() < expected_size {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "geometry section too short: need {expected_size} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let mut offset = HEADER_SIZE;

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
            let normal_u = u16::from_le_bytes([data[offset + 20], data[offset + 21]]);
            let normal_v = u16::from_le_bytes([data[offset + 22], data[offset + 23]]);
            let tangent_u = u16::from_le_bytes([data[offset + 24], data[offset + 25]]);
            let tangent_v_with_sign = u16::from_le_bytes([data[offset + 26], data[offset + 27]]);

            vertices.push(Vertex {
                position: [x, y, z],
                uv: [u, v],
                normal_oct: [normal_u, normal_v],
                tangent_packed: [tangent_u, tangent_v_with_sign],
            });
            offset += VERTEX_SIZE;
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
            let leaf_index = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            let texture_index = u32::from_le_bytes([
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]);
            faces.push(FaceMeta {
                leaf_index,
                texture_index,
            });
            offset += FACE_SIZE;
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
                Vertex::new(
                    [1.0, 2.0, 3.0],
                    [0.25, 0.75],
                    [0.0, 1.0, 0.0],
                    [1.0, 0.0, 0.0],
                    true,
                ),
                Vertex::new(
                    [4.0, 5.0, 6.0],
                    [0.5, 0.0],
                    [0.0, 0.0, 1.0],
                    [1.0, 0.0, 0.0],
                    false,
                ),
                Vertex::new(
                    [7.0, 8.0, 9.0],
                    [1.0, 1.0],
                    [0.0, 1.0, 0.0],
                    [0.0, 0.0, 1.0],
                    true,
                ),
            ],
            indices: vec![0, 1, 2],
            faces: vec![FaceMeta {
                leaf_index: 0,
                texture_index: 5,
            }],
        }
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let restored = GeometrySection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn vertex_is_28_bytes_face_is_8_bytes() {
        // 3 vertices produce header(12) + verts(3*28) + indices(3*4) + faces(1*8)
        let section = sample_section();
        let bytes = section.to_bytes();
        let expected = 12 + (3 * 28) + (3 * 4) + (1 * 8);
        assert_eq!(bytes.len(), expected);
    }

    #[test]
    fn position_and_uv_preserved_exactly() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let restored = GeometrySection::from_bytes(&bytes).unwrap();

        assert_eq!(restored.vertices[0].position, [1.0, 2.0, 3.0]);
        assert_eq!(restored.vertices[0].uv, [0.25, 0.75]);
        assert_eq!(restored.vertices[1].position, [4.0, 5.0, 6.0]);
        assert_eq!(restored.vertices[1].uv, [0.5, 0.0]);
    }

    #[test]
    fn bitangent_sign_round_trips() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let restored = GeometrySection::from_bytes(&bytes).unwrap();

        assert!(restored.vertices[0].bitangent_sign()); // true
        assert!(!restored.vertices[1].bitangent_sign()); // false
        assert!(restored.vertices[2].bitangent_sign()); // true
    }

    #[test]
    fn normal_tangent_decode_close_to_input() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let restored = GeometrySection::from_bytes(&bytes).unwrap();

        // +Y normal
        let n = restored.vertices[0].decode_normal();
        assert!(
            (n[1] - 1.0).abs() < 0.001,
            "expected ~+Y normal, got {:?}",
            n
        );

        // +X tangent
        let t = restored.vertices[0].decode_tangent();
        assert!(
            (t[0] - 1.0).abs() < 0.001,
            "expected ~+X tangent, got {:?}",
            t
        );
    }

    #[test]
    fn face_meta_preserved() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let restored = GeometrySection::from_bytes(&bytes).unwrap();

        assert_eq!(restored.faces[0].leaf_index, 0);
        assert_eq!(restored.faces[0].texture_index, 5);
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

    #[test]
    fn rejects_truncated_header() {
        let result = GeometrySection::from_bytes(&[0; 8]);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_short_body() {
        let mut data = vec![0u8; 12];
        data[0] = 1; // vertex_count = 1, but body missing
        let result = GeometrySection::from_bytes(&data);
        assert!(result.is_err());
    }

    #[test]
    fn no_texture_sentinel_round_trips() {
        let section = GeometrySection {
            vertices: vec![Vertex::new(
                [0.0, 0.0, 0.0],
                [0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
            )],
            indices: vec![],
            faces: vec![FaceMeta {
                leaf_index: 0,
                texture_index: NO_TEXTURE,
            }],
        };
        let bytes = section.to_bytes();
        let restored = GeometrySection::from_bytes(&bytes).unwrap();
        assert_eq!(restored.faces[0].texture_index, u32::MAX);
    }
}
