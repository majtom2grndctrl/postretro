// Geometry section data types: vertex/index buffers, per-face metadata.
// See: context/plans/ready/prl-phase-1-minimum-viable-compiler/

use crate::FormatError;

/// Per-face metadata referencing into the index buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceMeta {
    /// Byte offset into the index buffer is not stored; this is the index
    /// into the index array where this face's triangle indices begin.
    pub index_offset: u32,
    /// Number of indices (always a multiple of 3).
    pub index_count: u32,
    /// Which cluster this face belongs to.
    pub cluster_index: u32,
}

/// Geometry section: vertex positions, triangle indices, and per-face metadata.
///
/// Faces are ordered by cluster (all cluster-0 faces, then cluster-1, etc.)
/// so that contiguous ranges in the face metadata array correspond to
/// per-cluster draw calls.
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
//   FaceMeta * face_count      (12 bytes each: offset, count, cluster)

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
            buf.extend_from_slice(&face.cluster_index.to_le_bytes());
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
            let cluster_index = u32::from_le_bytes([
                data[offset + 8],
                data[offset + 9],
                data[offset + 10],
                data[offset + 11],
            ]);
            faces.push(FaceMeta {
                index_offset,
                index_count,
                cluster_index,
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
                    cluster_index: 0,
                },
                FaceMeta {
                    index_offset: 3,
                    index_count: 3,
                    cluster_index: 1,
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
}
