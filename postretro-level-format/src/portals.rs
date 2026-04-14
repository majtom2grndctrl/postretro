// Portal graph section: portal polygon array for runtime traversal.
// See: context/lib/build_pipeline.md §PRL Compilation, context/lib/rendering_pipeline.md §5

use crate::FormatError;

/// A single portal record referencing into the packed vertex array.
#[derive(Debug, Clone, PartialEq)]
pub struct PortalRecord {
    /// Index into the packed vertex array where this portal's vertices start.
    pub vertex_start: u32,
    /// Number of vertices in this portal polygon.
    pub vertex_count: u32,
    /// Leaf index on the front side of this portal.
    pub front_leaf: u32,
    /// Leaf index on the back side of this portal.
    pub back_leaf: u32,
}

/// Portals section: packed vertex array + portal records.
///
/// On-disk layout (all little-endian):
///   u32              portal_count
///   u32              vertex_count  (total vertices across all portals)
///   [f32; 3] * vertex_count        packed portal polygon vertices
///   PortalRecord * portal_count    (16 bytes each: vertex_start, vertex_count, front_leaf, back_leaf)
#[derive(Debug, Clone, PartialEq)]
pub struct PortalsSection {
    pub vertices: Vec<[f32; 3]>,
    pub portals: Vec<PortalRecord>,
}

const PORTAL_RECORD_SIZE: usize = 16;

impl PortalsSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let portal_count = self.portals.len() as u32;
        let vertex_count = self.vertices.len() as u32;

        let size = 8 + (self.vertices.len() * 12) + (self.portals.len() * PORTAL_RECORD_SIZE);
        let mut buf = Vec::with_capacity(size);

        buf.extend_from_slice(&portal_count.to_le_bytes());
        buf.extend_from_slice(&vertex_count.to_le_bytes());

        for v in &self.vertices {
            buf.extend_from_slice(&v[0].to_le_bytes());
            buf.extend_from_slice(&v[1].to_le_bytes());
            buf.extend_from_slice(&v[2].to_le_bytes());
        }

        for p in &self.portals {
            buf.extend_from_slice(&p.vertex_start.to_le_bytes());
            buf.extend_from_slice(&p.vertex_count.to_le_bytes());
            buf.extend_from_slice(&p.front_leaf.to_le_bytes());
            buf.extend_from_slice(&p.back_leaf.to_le_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 8 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "portals section too short for header",
            )));
        }

        let portal_count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let vertex_count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;

        let expected_size = 8 + (vertex_count * 12) + (portal_count * PORTAL_RECORD_SIZE);
        if data.len() < expected_size {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "portals section too short: need {expected_size} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let mut offset = 8;

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

        let mut portals = Vec::with_capacity(portal_count);
        for _ in 0..portal_count {
            let vertex_start = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            let vertex_count = u32::from_le_bytes([
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]);
            let front_leaf = u32::from_le_bytes([
                data[offset + 8],
                data[offset + 9],
                data[offset + 10],
                data[offset + 11],
            ]);
            let back_leaf = u32::from_le_bytes([
                data[offset + 12],
                data[offset + 13],
                data[offset + 14],
                data[offset + 15],
            ]);
            portals.push(PortalRecord {
                vertex_start,
                vertex_count,
                front_leaf,
                back_leaf,
            });
            offset += PORTAL_RECORD_SIZE;
        }

        Ok(Self { vertices, portals })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_section() -> PortalsSection {
        PortalsSection {
            vertices: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [5.0, 0.0, 0.0],
                [6.0, 0.0, 0.0],
                [6.0, 1.0, 0.0],
            ],
            portals: vec![
                PortalRecord {
                    vertex_start: 0,
                    vertex_count: 4,
                    front_leaf: 0,
                    back_leaf: 1,
                },
                PortalRecord {
                    vertex_start: 4,
                    vertex_count: 3,
                    front_leaf: 1,
                    back_leaf: 2,
                },
            ],
        }
    }

    #[test]
    fn round_trip() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let restored = PortalsSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn empty_round_trip() {
        let section = PortalsSection {
            vertices: vec![],
            portals: vec![],
        };
        let bytes = section.to_bytes();
        let restored = PortalsSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn byte_layout_header() {
        let section = sample_section();
        let bytes = section.to_bytes();

        let portal_count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let vertex_count = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);

        assert_eq!(portal_count, 2);
        assert_eq!(vertex_count, 7);
    }

    #[test]
    fn vertex_data_preserved() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let restored = PortalsSection::from_bytes(&bytes).unwrap();

        assert_eq!(restored.vertices.len(), 7);
        assert_eq!(restored.vertices[0], [0.0, 0.0, 0.0]);
        assert_eq!(restored.vertices[4], [5.0, 0.0, 0.0]);
    }

    #[test]
    fn leaf_indices_preserved() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let restored = PortalsSection::from_bytes(&bytes).unwrap();

        assert_eq!(restored.portals[0].front_leaf, 0);
        assert_eq!(restored.portals[0].back_leaf, 1);
        assert_eq!(restored.portals[1].front_leaf, 1);
        assert_eq!(restored.portals[1].back_leaf, 2);
    }

    #[test]
    fn rejects_truncated_header() {
        let result = PortalsSection::from_bytes(&[0; 4]);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_truncated_body() {
        // Header says 1 portal, 3 vertices but body is too short
        let mut data = vec![0u8; 12];
        data[0..4].copy_from_slice(&1u32.to_le_bytes()); // portal_count
        data[4..8].copy_from_slice(&3u32.to_le_bytes()); // vertex_count
        let result = PortalsSection::from_bytes(&data);
        assert!(result.is_err());
    }
}
