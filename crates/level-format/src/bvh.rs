// BVH section: flat node + leaf arrays for runtime GPU and bake-time traversal.
// See: context/lib/build_pipeline.md and context/lib/rendering_pipeline.md

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
///
/// The serialized form (40 bytes, little-endian) lays out the AABB corners as
/// six scalar f32s rather than a pair of `vec3<f32>`s: the matching WGSL
/// storage-buffer struct must use scalar fields too, because WGSL's
/// `AlignOf(vec3<f32>) = 16` would round the struct stride up to 48 and
/// desync the GPU layout from this on-disk one. The on-disk layout here is
/// mirrored by four downstream definitions that must stay byte-identical:
///   - `postretro/src/shaders/bvh_cull.wgsl` (GPU-side `struct BvhNode` /
///     `struct BvhLeaf`)
///   - `postretro/src/geometry.rs` (engine-side `BvhNode` / `BvhLeaf`)
///   - `postretro/src/prl.rs` (format → engine converter + test fixture)
///   - `postretro/src/compute_cull.rs` (`serialize_bvh_nodes` /
///     `serialize_bvh_leaves` + naga-based stride regression test)
///
/// Any layout change must update all four sites and the `LEAF_STRIDE` /
/// `NODE_STRIDE` constants below in the same pass.
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
    /// First chunk in the `AnimatedLightChunksSection.chunks` array owned by
    /// this leaf. `chunk_range_count == 0` on leaves whose faces have no
    /// animated-light overlap (or on maps without animated lights).
    pub chunk_range_start: u32,
    pub chunk_range_count: u32,
}

/// BVH section: flat node + leaf arrays plus a fixed header.
///
/// On-disk layout (all little-endian):
///   u32 node_count
///   u32 leaf_count
///   u32 root_node_index
///   u32 padding
///   BvhNode * node_count   (40 bytes each; see `NODE_STRIDE`)
///   BvhLeaf * leaf_count   (48 bytes each; see `LEAF_STRIDE`)
///
/// Nodes are DFS-ordered (root at `root_node_index`). Leaves are sorted by
/// `material_bucket_id` so each bucket owns a contiguous slot range in the
/// indirect draw buffer; the runtime derives the per-bucket `(first, count)`
/// table at load time with an O(leaf_count) scan.
#[derive(Debug, Clone, PartialEq)]
pub struct BvhSection {
    pub nodes: Vec<BvhNode>,
    pub leaves: Vec<BvhLeaf>,
    /// Root node index within `nodes`. Must be 0 for all current compiler-emitted
    /// sections; the loader rejects nonzero values.
    pub root_node_index: u32,
}

pub const NODE_STRIDE: usize = 40;
pub const LEAF_STRIDE: usize = 48;
pub const HEADER_SIZE: usize = 16;

/// Contiguous leaf-index range owned by a single material bucket in a
/// bucket-sorted leaf array. Produced by [`derive_bucket_ranges`]; consumed by
/// both the runtime (to issue one `multi_draw_indexed_indirect` call per
/// bucket) and the compiler (for stats/log output).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BucketRange {
    pub material_bucket_id: u32,
    pub first_leaf: u32,
    pub leaf_count: u32,
}

/// Scan a leaf array sorted by `material_bucket_id` and emit one [`BucketRange`]
/// per distinct bucket, in the order they appear. This is a pure helper over
/// the shared on-disk leaf layout, so both the engine and the compiler use it
/// as a single source of truth.
pub fn derive_bucket_ranges(leaves: &[BvhLeaf]) -> Vec<BucketRange> {
    let mut ranges: Vec<BucketRange> = Vec::new();
    for (i, leaf) in leaves.iter().enumerate() {
        match ranges.last_mut() {
            Some(last) if last.material_bucket_id == leaf.material_bucket_id => {
                last.leaf_count += 1;
            }
            _ => {
                ranges.push(BucketRange {
                    material_bucket_id: leaf.material_bucket_id,
                    first_leaf: i as u32,
                    leaf_count: 1,
                });
            }
        }
    }
    ranges
}

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
            buf.extend_from_slice(&leaf.chunk_range_start.to_le_bytes());
            buf.extend_from_slice(&leaf.chunk_range_count.to_le_bytes());
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

        let node_bytes = node_count.checked_mul(NODE_STRIDE).ok_or_else(|| {
            FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bvh node count overflows section size",
            ))
        })?;
        let leaf_bytes = leaf_count.checked_mul(LEAF_STRIDE).ok_or_else(|| {
            FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bvh leaf count overflows section size",
            ))
        })?;
        let expected_size = HEADER_SIZE
            .checked_add(node_bytes)
            .and_then(|size| size.checked_add(leaf_bytes))
            .ok_or_else(|| {
                FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "bvh declared counts overflow section size",
                ))
            })?;
        if data.len() < expected_size {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "bvh section too short: need {expected_size} bytes, got {}",
                    data.len()
                ),
            )));
        }
        if data.len() > expected_size {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "bvh section has trailing bytes: expected {expected_size}, got {}",
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
        for node_idx in 0..node_count {
            let aabb_min = read_vec3(data, offset);
            let skip_index = read_u32(data, offset + 12);
            let aabb_max = read_vec3(data, offset + 16);
            let left_child_or_leaf_index = read_u32(data, offset + 28);
            let flags = read_u32(data, offset + 32);
            let padding = read_u32(data, offset + 36);
            validate_aabb("node", node_idx, aabb_min, aabb_max)?;
            if padding != 0 {
                return Err(FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("bvh node {node_idx} has nonzero reserved padding {padding}"),
                )));
            }
            nodes.push(BvhNode {
                aabb_min,
                skip_index,
                aabb_max,
                left_child_or_leaf_index,
                flags,
                _padding: padding,
            });
            offset += NODE_STRIDE;
        }

        let mut leaves = Vec::with_capacity(leaf_count);
        for leaf_idx in 0..leaf_count {
            let aabb_min = read_vec3(data, offset);
            let material_bucket_id = read_u32(data, offset + 12);
            let aabb_max = read_vec3(data, offset + 16);
            let index_offset = read_u32(data, offset + 28);
            let index_count = read_u32(data, offset + 32);
            let cell_id = read_u32(data, offset + 36);
            let chunk_range_start = read_u32(data, offset + 40);
            let chunk_range_count = read_u32(data, offset + 44);
            validate_aabb("leaf", leaf_idx, aabb_min, aabb_max)?;
            leaves.push(BvhLeaf {
                aabb_min,
                material_bucket_id,
                aabb_max,
                index_offset,
                index_count,
                cell_id,
                chunk_range_start,
                chunk_range_count,
            });
            offset += LEAF_STRIDE;
        }

        validate_flat_traversal(&nodes, &leaves, root_node_index)?;

        Ok(Self {
            nodes,
            leaves,
            root_node_index,
        })
    }
}

fn validate_aabb(
    record_kind: &str,
    record_idx: usize,
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
) -> crate::Result<()> {
    for axis in 0..3 {
        let min = aabb_min[axis];
        let max = aabb_max[axis];
        if !min.is_finite() || !max.is_finite() {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "bvh {record_kind} {record_idx} has non-finite AABB axis {axis}: min={min}, max={max}"
                ),
            )));
        }
        if min > max {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "bvh {record_kind} {record_idx} has inverted AABB axis {axis}: min={min}, max={max}"
                ),
            )));
        }
    }

    Ok(())
}

fn validate_flat_traversal(
    nodes: &[BvhNode],
    leaves: &[BvhLeaf],
    root_node_index: u32,
) -> crate::Result<()> {
    if nodes.is_empty() {
        if root_node_index != 0 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("empty bvh has nonzero root_node_index {root_node_index}"),
            )));
        }
        if !leaves.is_empty() {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("bvh has {} leaves but no traversal nodes", leaves.len()),
            )));
        }
        return Ok(());
    }

    if root_node_index != 0 {
        return Err(FormatError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("bvh root_node_index {root_node_index} must be 0"),
        )));
    }

    let node_count = nodes.len();
    let mut leaf_reference_counts = vec![0u32; leaves.len()];
    for (node_idx, node) in nodes.iter().enumerate() {
        let skip_index = node.skip_index as usize;
        if skip_index <= node_idx || skip_index > node_count {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "bvh node {node_idx} skip_index {} must be in {}..={node_count}",
                    node.skip_index,
                    node_idx + 1
                ),
            )));
        }

        if node.flags & BVH_NODE_FLAG_LEAF != 0 {
            let leaf_index = node.left_child_or_leaf_index as usize;
            let Some(reference_count) = leaf_reference_counts.get_mut(leaf_index) else {
                return Err(FormatError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "bvh leaf node {node_idx} references leaf {leaf_index} out of range for {} leaves",
                        leaves.len()
                    ),
                )));
            };
            *reference_count += 1;
            continue;
        }

        let left_child = node_idx + 1;
        if left_child >= node_count || left_child >= skip_index {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "bvh internal node {node_idx} has no valid implicit left child at {left_child} before skip_index {}",
                    node.skip_index
                ),
            )));
        }
    }

    for (leaf_idx, reference_count) in leaf_reference_counts.iter().enumerate() {
        if *reference_count != 1 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "bvh leaf {leaf_idx} is referenced {reference_count} times by traversal nodes; expected exactly once"
                ),
            )));
        }
    }

    Ok(())
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
                    chunk_range_start: 0,
                    chunk_range_count: 2,
                },
                BvhLeaf {
                    aabb_min: [-1.0, -1.0, 0.5],
                    material_bucket_id: 1,
                    aabb_max: [1.0, 1.0, 0.5],
                    index_offset: 6,
                    index_count: 3,
                    cell_id: 4,
                    chunk_range_start: 2,
                    chunk_range_count: 0,
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
        assert_eq!(LEAF_STRIDE, 48);
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
    fn rejects_trailing_bytes() {
        let mut bytes = sample_section().to_bytes();
        bytes.extend_from_slice(&[0xab, 0xcd]);
        let err = BvhSection::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)), "got {err:?}");
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
    fn rejects_nonzero_root_for_nonempty_tree() {
        let mut bytes = sample_section().to_bytes();
        bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
        let err = BvhSection::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)), "got {err:?}");
    }

    #[test]
    fn rejects_leaves_without_nodes() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; LEAF_STRIDE]);

        let err = BvhSection::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)), "got {err:?}");
    }

    #[test]
    fn rejects_skip_index_at_or_before_current_node() {
        let mut bytes = sample_section().to_bytes();
        let node_1_skip_offset = HEADER_SIZE + NODE_STRIDE + 12;
        bytes[node_1_skip_offset..node_1_skip_offset + 4].copy_from_slice(&1u32.to_le_bytes());

        let err = BvhSection::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)), "got {err:?}");
    }

    #[test]
    fn rejects_skip_index_past_node_count() {
        let mut bytes = sample_section().to_bytes();
        let node_1_skip_offset = HEADER_SIZE + NODE_STRIDE + 12;
        bytes[node_1_skip_offset..node_1_skip_offset + 4].copy_from_slice(&4u32.to_le_bytes());

        let err = BvhSection::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)), "got {err:?}");
    }

    #[test]
    fn rejects_internal_node_without_implicit_left_child() {
        let mut bytes = sample_section().to_bytes();
        let node_2_flags_offset = HEADER_SIZE + (2 * NODE_STRIDE) + 32;
        bytes[node_2_flags_offset..node_2_flags_offset + 4].copy_from_slice(&0u32.to_le_bytes());

        let err = BvhSection::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)), "got {err:?}");
    }

    #[test]
    fn rejects_leaf_node_reference_out_of_range() {
        let mut section = sample_section();
        section.nodes[1].left_child_or_leaf_index = 99;

        let err = BvhSection::from_bytes(&section.to_bytes()).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)), "got {err:?}");
    }

    #[test]
    fn rejects_duplicate_leaf_node_reference() {
        // Regression: duplicate traversal references could omit a serialized leaf.
        let mut section = sample_section();
        section.nodes[2].left_child_or_leaf_index = 0;

        let err = BvhSection::from_bytes(&section.to_bytes()).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)), "got {err:?}");
    }

    #[test]
    fn rejects_node_non_finite_aabb() {
        let mut section = sample_section();
        section.nodes[0].aabb_min[0] = f32::NAN;

        let err = BvhSection::from_bytes(&section.to_bytes()).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)), "got {err:?}");
    }

    #[test]
    fn rejects_leaf_inverted_aabb() {
        let mut section = sample_section();
        section.leaves[0].aabb_min[1] = 3.0;

        let err = BvhSection::from_bytes(&section.to_bytes()).unwrap_err();
        assert!(matches!(err, FormatError::Io(_)), "got {err:?}");
    }

    #[test]
    fn rejects_nonzero_node_padding() {
        let mut bytes = sample_section().to_bytes();
        let node_0_padding_offset = HEADER_SIZE + 36;
        bytes[node_0_padding_offset..node_0_padding_offset + 4]
            .copy_from_slice(&7u32.to_le_bytes());

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
