// BVH section: flat node + leaf arrays for runtime GPU and bake-time traversal.
// See: context/plans/in-progress/bvh-foundation/1-compile-bvh.md

use crate::FormatError;

/// Flag bit 0 on `BvhNode.flags`: set iff the node is a leaf.
pub const BVH_NODE_FLAG_LEAF: u32 = 1 << 0;

/// One entry in the flat BVH node array.
///
/// Nodes are written in DFS order. For internal nodes, the left child is always
/// at `current_index + 1`; `skip_index` points to the next sibling subtree root
/// (the value to jump to on AABB reject). For leaf nodes,
/// `left_child_or_leaf_index` indexes into the `leaves` array and `skip_index`
/// still points past this node for DFS continuation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BvhNode {
    pub aabb_min: [f32; 3],
    /// Index of the next sibling subtree root — the node to visit after this
    /// subtree on AABB reject or after descending this subtree's leaves.
    pub skip_index: u32,
    pub aabb_max: [f32; 3],
    /// For leaves, index into `BvhSection::leaves`. Unused (zero) for internal
    /// nodes — the left child is always `current_index + 1`.
    pub left_child_or_leaf_index: u32,
    /// Bit 0 (`BVH_NODE_FLAG_LEAF`) set iff this node is a leaf.
    pub flags: u32,
    /// Reserved, always zero.
    pub _padding: u32,
}

/// One entry in the flat BVH leaf array.
///
/// Each leaf owns a contiguous range of the level's shared index buffer plus
/// the cell + material metadata needed to classify and dispatch its draw call.
/// The leaf's position in the flat leaf array is also its permanent slot in
/// the runtime indirect draw buffer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BvhLeaf {
    pub aabb_min: [f32; 3],
    pub material_bucket_id: u32,
    pub aabb_max: [f32; 3],
    pub index_offset: u32,
    pub index_count: u32,
    pub cell_id: u32,
}

/// BVH section: flat node + leaf arrays plus a fixed header.
///
/// On-disk layout (all little-endian):
///   u32 node_count
///   u32 leaf_count
///   u32 root_node_index
///   u32 padding
///   BvhNode * node_count   (40 bytes each; see `NODE_STRIDE`)
///   BvhLeaf * leaf_count   (40 bytes each; see `LEAF_STRIDE`)
///
/// Nodes are DFS-ordered (root at `root_node_index`). Leaves are sorted by
/// `material_bucket_id` so each bucket owns a contiguous slot range in the
/// indirect draw buffer; the runtime derives the per-bucket `(first, count)`
/// table at load time with an O(leaf_count) scan.
#[derive(Debug, Clone, PartialEq)]
pub struct BvhSection {
    pub nodes: Vec<BvhNode>,
    pub leaves: Vec<BvhLeaf>,
    /// Index of the root node in `nodes`. Always 0 for non-empty trees built
    /// by the compiler, but carried explicitly for forward compatibility.
    pub root_node_index: u32,
}

pub const NODE_STRIDE: usize = 40;
pub const LEAF_STRIDE: usize = 40;
pub const HEADER_SIZE: usize = 16;

impl BvhSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let node_count = self.nodes.len() as u32;
        let leaf_count = self.leaves.len() as u32;

        let size =
            HEADER_SIZE + (self.nodes.len() * NODE_STRIDE) + (self.leaves.len() * LEAF_STRIDE);
        let mut buf = Vec::with_capacity(size);

        buf.extend_from_slice(&node_count.to_le_bytes());
        buf.extend_from_slice(&leaf_count.to_le_bytes());
        buf.extend_from_slice(&self.root_node_index.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes()); // header padding

        for node in &self.nodes {
            buf.extend_from_slice(&node.aabb_min[0].to_le_bytes());
            buf.extend_from_slice(&node.aabb_min[1].to_le_bytes());
            buf.extend_from_slice(&node.aabb_min[2].to_le_bytes());
            buf.extend_from_slice(&node.skip_index.to_le_bytes());
            buf.extend_from_slice(&node.aabb_max[0].to_le_bytes());
            buf.extend_from_slice(&node.aabb_max[1].to_le_bytes());
            buf.extend_from_slice(&node.aabb_max[2].to_le_bytes());
            buf.extend_from_slice(&node.left_child_or_leaf_index.to_le_bytes());
            buf.extend_from_slice(&node.flags.to_le_bytes());
            buf.extend_from_slice(&0u32.to_le_bytes()); // node padding
        }

        for leaf in &self.leaves {
            buf.extend_from_slice(&leaf.aabb_min[0].to_le_bytes());
            buf.extend_from_slice(&leaf.aabb_min[1].to_le_bytes());
            buf.extend_from_slice(&leaf.aabb_min[2].to_le_bytes());
            buf.extend_from_slice(&leaf.material_bucket_id.to_le_bytes());
            buf.extend_from_slice(&leaf.aabb_max[0].to_le_bytes());
            buf.extend_from_slice(&leaf.aabb_max[1].to_le_bytes());
            buf.extend_from_slice(&leaf.aabb_max[2].to_le_bytes());
            buf.extend_from_slice(&leaf.index_offset.to_le_bytes());
            buf.extend_from_slice(&leaf.index_count.to_le_bytes());
            buf.extend_from_slice(&leaf.cell_id.to_le_bytes());
        }

        debug_assert_eq!(buf.len(), size);
        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "bvh section too short for header",
            )));
        }

        let node_count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let leaf_count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let root_node_index = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
        // bytes 12..16 are header padding (ignored)

        let expected_size = HEADER_SIZE + (node_count * NODE_STRIDE) + (leaf_count * LEAF_STRIDE);
        if data.len() < expected_size {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "bvh section too short: need {expected_size} bytes, got {}",
                    data.len()
                ),
            )));
        }

        if node_count > 0 && root_node_index as usize >= node_count {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "bvh root_node_index {root_node_index} out of range (node_count {node_count})"
                ),
            )));
        }

        let mut offset = HEADER_SIZE;

        let mut nodes = Vec::with_capacity(node_count);
        for _ in 0..node_count {
            let aabb_min = read_vec3(data, offset);
            let skip_index = read_u32(data, offset + 12);
            let aabb_max = read_vec3(data, offset + 16);
            let left_child_or_leaf_index = read_u32(data, offset + 28);
            let flags = read_u32(data, offset + 32);
            // bytes 36..40 are node padding (ignored)
            nodes.push(BvhNode {
                aabb_min,
                skip_index,
                aabb_max,
                left_child_or_leaf_index,
                flags,
                _padding: 0,
            });
            offset += NODE_STRIDE;
        }

        let mut leaves = Vec::with_capacity(leaf_count);
        for _ in 0..leaf_count {
            let aabb_min = read_vec3(data, offset);
            let material_bucket_id = read_u32(data, offset + 12);
            let aabb_max = read_vec3(data, offset + 16);
            let index_offset = read_u32(data, offset + 28);
            let index_count = read_u32(data, offset + 32);
            let cell_id = read_u32(data, offset + 36);
            leaves.push(BvhLeaf {
                aabb_min,
                material_bucket_id,
                aabb_max,
                index_offset,
                index_count,
                cell_id,
            });
            offset += LEAF_STRIDE;
        }

        Ok(Self {
            nodes,
            leaves,
            root_node_index,
        })
    }
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_vec3(data: &[u8], at: usize) -> [f32; 3] {
    let x = f32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
    let y = f32::from_le_bytes([data[at + 4], data[at + 5], data[at + 6], data[at + 7]]);
    let z = f32::from_le_bytes([data[at + 8], data[at + 9], data[at + 10], data[at + 11]]);
    [x, y, z]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_section() -> BvhSection {
        BvhSection {
            nodes: vec![
                // Internal root: covers whole scene; left child at 1, skip to end
                BvhNode {
                    aabb_min: [-1.0, -1.0, -1.0],
                    skip_index: 3,
                    aabb_max: [1.0, 1.0, 1.0],
                    left_child_or_leaf_index: 0,
                    flags: 0,
                    _padding: 0,
                },
                // Leaf 0: refers to leaves[0]
                BvhNode {
                    aabb_min: [-1.0, -1.0, 0.0],
                    skip_index: 2,
                    aabb_max: [1.0, 1.0, 0.0],
                    left_child_or_leaf_index: 0,
                    flags: BVH_NODE_FLAG_LEAF,
                    _padding: 0,
                },
                // Leaf 1: refers to leaves[1]
                BvhNode {
                    aabb_min: [-1.0, -1.0, 0.5],
                    skip_index: 3,
                    aabb_max: [1.0, 1.0, 0.5],
                    left_child_or_leaf_index: 1,
                    flags: BVH_NODE_FLAG_LEAF,
                    _padding: 0,
                },
            ],
            leaves: vec![
                BvhLeaf {
                    aabb_min: [-1.0, -1.0, 0.0],
                    material_bucket_id: 0,
                    aabb_max: [1.0, 1.0, 0.0],
                    index_offset: 0,
                    index_count: 6,
                    cell_id: 3,
                },
                BvhLeaf {
                    aabb_min: [-1.0, -1.0, 0.5],
                    material_bucket_id: 1,
                    aabb_max: [1.0, 1.0, 0.5],
                    index_offset: 6,
                    index_count: 3,
                    cell_id: 4,
                },
            ],
            root_node_index: 0,
        }
    }

    #[test]
    fn round_trip_byte_identical() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let restored = BvhSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);

        // Rewrite and compare byte-for-byte — this is what the PRL round-trip relies on.
        let rebytes = restored.to_bytes();
        assert_eq!(bytes, rebytes);
    }

    #[test]
    fn byte_layout_strides_match_spec() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let expected_len = HEADER_SIZE
            + (section.nodes.len() * NODE_STRIDE)
            + (section.leaves.len() * LEAF_STRIDE);
        assert_eq!(bytes.len(), expected_len);
        assert_eq!(NODE_STRIDE, 40);
        assert_eq!(LEAF_STRIDE, 40);
    }

    #[test]
    fn empty_section_round_trips() {
        let section = BvhSection {
            nodes: Vec::new(),
            leaves: Vec::new(),
            root_node_index: 0,
        };
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), HEADER_SIZE);
        let restored = BvhSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn rejects_truncated_header() {
        let err = BvhSection::from_bytes(&[0u8; 8]).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }

    #[test]
    fn rejects_truncated_body() {
        let section = sample_section();
        let bytes = section.to_bytes();
        // Drop the last leaf's worth of bytes and observe the decoder reject it.
        let truncated = &bytes[..bytes.len() - LEAF_STRIDE];
        let err = BvhSection::from_bytes(truncated).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)));
    }

    #[test]
    fn rejects_malformed_root_index() {
        let mut bytes = sample_section().to_bytes();
        // node_count is the first u32. Set root_node_index past the end.
        bytes[8..12].copy_from_slice(&999u32.to_le_bytes());
        let err = BvhSection::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)), "got {err:?}");
    }

    #[test]
    fn leaves_sorted_by_material_bucket() {
        // Exercising the contract that BvhSection callers are expected to honor.
        let section = sample_section();
        for w in section.leaves.windows(2) {
            assert!(w[0].material_bucket_id <= w[1].material_bucket_id);
        }
    }
}
