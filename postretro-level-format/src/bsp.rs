// BSP tree sections: flat node and leaf arrays for the PRL format.
// See: context/lib/build_pipeline.md §PRL

use crate::FormatError;

/// A single BSP interior node record.
#[derive(Debug, Clone, PartialEq)]
pub struct BspNodeRecord {
    pub plane_normal: [f32; 3],
    pub plane_distance: f32,
    /// Positive = node index; negative = `(-1 - leaf_index)` (sentinel encoding).
    pub front: i32,
    /// Same encoding as `front`.
    pub back: i32,
}

/// BSP nodes section: flat array of interior nodes.
///
/// On-disk layout (all little-endian):
///   u32            node_count
///   Per node (24 bytes each):
///     f32 * 3      plane_normal (x, y, z)
///     f32          plane_distance
///     i32          front (positive = node index, negative = -1 - leaf_index)
///     i32          back  (same encoding)
#[derive(Debug, Clone, PartialEq)]
pub struct BspNodesSection {
    pub nodes: Vec<BspNodeRecord>,
}

const NODE_RECORD_SIZE: usize = 24;

impl BspNodesSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let node_count = self.nodes.len() as u32;
        let size = 4 + self.nodes.len() * NODE_RECORD_SIZE;
        let mut buf = Vec::with_capacity(size);

        buf.extend_from_slice(&node_count.to_le_bytes());

        for n in &self.nodes {
            buf.extend_from_slice(&n.plane_normal[0].to_le_bytes());
            buf.extend_from_slice(&n.plane_normal[1].to_le_bytes());
            buf.extend_from_slice(&n.plane_normal[2].to_le_bytes());
            buf.extend_from_slice(&n.plane_distance.to_le_bytes());
            buf.extend_from_slice(&n.front.to_le_bytes());
            buf.extend_from_slice(&n.back.to_le_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 4 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "BSP nodes section too short for header",
            )));
        }

        let node_count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;

        let expected_size = 4 + node_count * NODE_RECORD_SIZE;
        if data.len() < expected_size {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "BSP nodes section too short: need {expected_size} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let mut nodes = Vec::with_capacity(node_count);
        for i in 0..node_count {
            let base = 4 + i * NODE_RECORD_SIZE;
            let f = |off: usize| -> f32 {
                f32::from_le_bytes([
                    data[base + off],
                    data[base + off + 1],
                    data[base + off + 2],
                    data[base + off + 3],
                ])
            };

            let front = i32::from_le_bytes([
                data[base + 16],
                data[base + 17],
                data[base + 18],
                data[base + 19],
            ]);
            let back = i32::from_le_bytes([
                data[base + 20],
                data[base + 21],
                data[base + 22],
                data[base + 23],
            ]);

            nodes.push(BspNodeRecord {
                plane_normal: [f(0), f(4), f(8)],
                plane_distance: f(12),
                front,
                back,
            });
        }

        Ok(Self { nodes })
    }
}

/// A single BSP leaf record.
#[derive(Debug, Clone, PartialEq)]
pub struct BspLeafRecord {
    /// Index into the geometry section's face list.
    pub face_start: u32,
    /// Number of faces in this leaf.
    pub face_count: u32,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    /// Byte offset into the LeafPvs section blob.
    pub pvs_offset: u32,
    /// Byte length of this leaf's RLE-compressed PVS.
    pub pvs_size: u32,
    /// 1 if solid, 0 if empty.
    pub is_solid: u8,
}

/// BSP leaves section: flat array of leaf records.
///
/// On-disk layout (all little-endian):
///   u32              leaf_count
///   Per leaf (41 bytes each):
///     u32            face_start
///     u32            face_count
///     f32 * 3        bounds_min (x, y, z)
///     f32 * 3        bounds_max (x, y, z)
///     u32            pvs_offset
///     u32            pvs_size
///     u8             is_solid (1 = solid, 0 = empty)
#[derive(Debug, Clone, PartialEq)]
pub struct BspLeavesSection {
    pub leaves: Vec<BspLeafRecord>,
}

const LEAF_RECORD_SIZE: usize = 41;

impl BspLeavesSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let leaf_count = self.leaves.len() as u32;
        let size = 4 + self.leaves.len() * LEAF_RECORD_SIZE;
        let mut buf = Vec::with_capacity(size);

        buf.extend_from_slice(&leaf_count.to_le_bytes());

        for l in &self.leaves {
            buf.extend_from_slice(&l.face_start.to_le_bytes());
            buf.extend_from_slice(&l.face_count.to_le_bytes());
            buf.extend_from_slice(&l.bounds_min[0].to_le_bytes());
            buf.extend_from_slice(&l.bounds_min[1].to_le_bytes());
            buf.extend_from_slice(&l.bounds_min[2].to_le_bytes());
            buf.extend_from_slice(&l.bounds_max[0].to_le_bytes());
            buf.extend_from_slice(&l.bounds_max[1].to_le_bytes());
            buf.extend_from_slice(&l.bounds_max[2].to_le_bytes());
            buf.extend_from_slice(&l.pvs_offset.to_le_bytes());
            buf.extend_from_slice(&l.pvs_size.to_le_bytes());
            buf.push(l.is_solid);
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 4 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "BSP leaves section too short for header",
            )));
        }

        let leaf_count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;

        let expected_size = 4 + leaf_count * LEAF_RECORD_SIZE;
        if data.len() < expected_size {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "BSP leaves section too short: need {expected_size} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let mut leaves = Vec::with_capacity(leaf_count);
        for i in 0..leaf_count {
            let base = 4 + i * LEAF_RECORD_SIZE;
            let u = |off: usize| -> u32 {
                u32::from_le_bytes([
                    data[base + off],
                    data[base + off + 1],
                    data[base + off + 2],
                    data[base + off + 3],
                ])
            };
            let f = |off: usize| -> f32 {
                f32::from_le_bytes([
                    data[base + off],
                    data[base + off + 1],
                    data[base + off + 2],
                    data[base + off + 3],
                ])
            };

            leaves.push(BspLeafRecord {
                face_start: u(0),
                face_count: u(4),
                bounds_min: [f(8), f(12), f(16)],
                bounds_max: [f(20), f(24), f(28)],
                pvs_offset: u(32),
                pvs_size: u(36),
                is_solid: data[base + 40],
            });
        }

        Ok(Self { leaves })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- BspNodesSection tests --

    #[test]
    fn nodes_round_trip() {
        let section = BspNodesSection {
            nodes: vec![
                BspNodeRecord {
                    plane_normal: [1.0, 0.0, 0.0],
                    plane_distance: 32.0,
                    front: 1, // node index 1
                    back: -1, // leaf index 0
                },
                BspNodeRecord {
                    plane_normal: [0.0, 1.0, 0.0],
                    plane_distance: -16.5,
                    front: -1 - 1, // leaf index 1
                    back: -1 - 2,  // leaf index 2
                },
            ],
        };

        let bytes = section.to_bytes();
        let restored = BspNodesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn nodes_empty_round_trip() {
        let section = BspNodesSection { nodes: vec![] };
        let bytes = section.to_bytes();
        let restored = BspNodesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn nodes_sentinel_encoding_correct() {
        // Verify the sentinel encoding convention: negative = (-1 - leaf_index)
        let section = BspNodesSection {
            nodes: vec![BspNodeRecord {
                plane_normal: [0.0, 0.0, 1.0],
                plane_distance: 0.0,
                front: -1 - 5,  // leaf index 5 => -6
                back: -1 - 100, // leaf index 100 => -101
            }],
        };

        let bytes = section.to_bytes();
        let restored = BspNodesSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored.nodes[0].front, -6);
        assert_eq!(restored.nodes[0].back, -101);
        // Decode back to leaf index
        assert_eq!(-1 - restored.nodes[0].front, 5);
        assert_eq!(-1 - restored.nodes[0].back, 100);
    }

    #[test]
    fn nodes_rejects_truncated_header() {
        let result = BspNodesSection::from_bytes(&[0; 2]);
        assert!(result.is_err());
    }

    #[test]
    fn nodes_rejects_truncated_body() {
        // Header says 1 node but body is too short
        let mut data = vec![0u8; 8];
        data[0] = 1; // node_count = 1
        let result = BspNodesSection::from_bytes(&data);
        assert!(result.is_err());
    }

    // -- BspLeavesSection tests --

    #[test]
    fn leaves_round_trip() {
        let section = BspLeavesSection {
            leaves: vec![
                BspLeafRecord {
                    face_start: 0,
                    face_count: 10,
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [64.0, 64.0, 64.0],
                    pvs_offset: 0,
                    pvs_size: 5,
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 10,
                    face_count: 5,
                    bounds_min: [64.0, 0.0, 0.0],
                    bounds_max: [128.0, 64.0, 64.0],
                    pvs_offset: 5,
                    pvs_size: 3,
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 0,
                    face_count: 0,
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [0.0, 0.0, 0.0],
                    pvs_offset: 0,
                    pvs_size: 0,
                    is_solid: 1,
                },
            ],
        };

        let bytes = section.to_bytes();
        let restored = BspLeavesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn leaves_empty_round_trip() {
        let section = BspLeavesSection { leaves: vec![] };
        let bytes = section.to_bytes();
        let restored = BspLeavesSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn leaves_solid_flag_preserved() {
        let section = BspLeavesSection {
            leaves: vec![
                BspLeafRecord {
                    face_start: 0,
                    face_count: 3,
                    bounds_min: [1.0, 2.0, 3.0],
                    bounds_max: [4.0, 5.0, 6.0],
                    pvs_offset: 0,
                    pvs_size: 2,
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 0,
                    face_count: 0,
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [0.0, 0.0, 0.0],
                    pvs_offset: 0,
                    pvs_size: 0,
                    is_solid: 1,
                },
            ],
        };

        let bytes = section.to_bytes();
        let restored = BspLeavesSection::from_bytes(&bytes).unwrap();
        assert_eq!(restored.leaves[0].is_solid, 0);
        assert_eq!(restored.leaves[1].is_solid, 1);
    }

    #[test]
    fn leaves_rejects_truncated_header() {
        let result = BspLeavesSection::from_bytes(&[0; 2]);
        assert!(result.is_err());
    }

    #[test]
    fn leaves_rejects_truncated_body() {
        // Header says 1 leaf but body is too short
        let mut data = vec![0u8; 10];
        data[0] = 1; // leaf_count = 1
        let result = BspLeavesSection::from_bytes(&data);
        assert!(result.is_err());
    }
}
