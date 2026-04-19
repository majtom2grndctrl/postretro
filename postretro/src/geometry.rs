// Format-agnostic vertex and BVH runtime types shared by the PRL loader and renderer.
// See: context/lib/rendering_pipeline.md §5, §6
// See: context/plans/in-progress/bvh-foundation/2-runtime-bvh.md

/// World-geometry vertex: position + base UV + octahedral normal + octahedral
/// tangent + lightmap UV. Matches the `Geometry` on-disk layout. Normal and
/// tangent decode in the vertex shader; lightmap UV is passed through to the
/// fragment shader for atlas sampling.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldVertex {
    pub position: [f32; 3],
    pub base_uv: [f32; 2],
    /// Octahedral-encoded unit normal (u16 x 2).
    pub normal_oct: [u16; 2],
    /// Packed tangent: u16 octahedral u-component, u16 v-component with
    /// bitangent sign in bit 15.
    pub tangent_packed: [u16; 2],
    /// Lightmap atlas UV, quantized 0..65535 → 0..1. Zero on vertices that
    /// did not receive a lightmap chart (runtime renders against the
    /// placeholder atlas in that case).
    pub lightmap_uv: [u16; 2],
}

impl WorldVertex {
    /// Stride in bytes: 12 (pos) + 8 (base uv) + 4 (normal) + 4 (tangent) + 4
    /// (lightmap uv) = 32 bytes.
    pub const STRIDE: usize = 32;
}

/// One flat BVH node, matching the WGSL `BvhNode` struct byte-for-byte.
/// See `context/plans/in-progress/bvh-foundation/1-compile-bvh.md` for the layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BvhNode {
    pub aabb_min: [f32; 3],
    /// Index of the next sibling subtree root — the array slot to jump to
    /// when the current subtree is rejected.
    pub skip_index: u32,
    pub aabb_max: [f32; 3],
    /// For leaves, the index into `BvhTree::leaves`. For internal nodes,
    /// ignored (left child is always `current_index + 1`).
    pub left_child_or_leaf_index: u32,
    /// Bit 0 (`BVH_NODE_FLAG_LEAF`) is set iff this node is a leaf.
    pub flags: u32,
}

/// Flag bit 0 on `BvhNode.flags`: set iff the node is a leaf.
pub const BVH_NODE_FLAG_LEAF: u32 = 1;

/// One flat BVH leaf, matching the WGSL `BvhLeaf` struct byte-for-byte.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BvhLeaf {
    pub aabb_min: [f32; 3],
    pub material_bucket_id: u32,
    pub aabb_max: [f32; 3],
    pub index_offset: u32,
    pub index_count: u32,
    /// BSP leaf id this leaf's primitives live in — the value checked against
    /// the visible-cell bitmask in the compute shader.
    pub cell_id: u32,
}

/// CPU-side view of the BVH section, held alongside the level geometry and
/// used to size GPU buffers, derive per-bucket draw ranges, and drive the
/// wireframe overlay. The data is uploaded verbatim to GPU storage buffers.
#[derive(Debug, Clone)]
pub struct BvhTree {
    pub nodes: Vec<BvhNode>,
    pub leaves: Vec<BvhLeaf>,
    pub root_node_index: u32,
}

/// Contiguous leaf-index range owned by a single material bucket in the
/// sorted leaf array. Derived at level load from the sorted leaf array by
/// scanning `material_bucket_id` transitions (O(leaf_count)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BucketRange {
    pub material_bucket_id: u32,
    pub first_leaf: u32,
    pub leaf_count: u32,
}

impl BvhTree {
    /// Scan the sorted leaf array once and produce one `BucketRange` per
    /// distinct `material_bucket_id`. Leaves are assumed to be sorted by
    /// `material_bucket_id` (sub-plan 1 guarantees this). Buckets are
    /// emitted in the order they appear, which matches the sort order.
    pub fn derive_bucket_ranges(&self) -> Vec<BucketRange> {
        let mut ranges: Vec<BucketRange> = Vec::new();
        for (i, leaf) in self.leaves.iter().enumerate() {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(material_bucket_id: u32, cell_id: u32) -> BvhLeaf {
        BvhLeaf {
            aabb_min: [0.0; 3],
            material_bucket_id,
            aabb_max: [1.0; 3],
            index_offset: 0,
            index_count: 3,
            cell_id,
        }
    }

    #[test]
    fn derive_bucket_ranges_single_bucket() {
        let tree = BvhTree {
            nodes: vec![],
            leaves: vec![leaf(0, 0), leaf(0, 1), leaf(0, 2)],
            root_node_index: 0,
        };
        let ranges = tree.derive_bucket_ranges();
        assert_eq!(ranges.len(), 1);
        assert_eq!(
            ranges[0],
            BucketRange {
                material_bucket_id: 0,
                first_leaf: 0,
                leaf_count: 3,
            }
        );
    }

    #[test]
    fn derive_bucket_ranges_multiple_contiguous_buckets() {
        let tree = BvhTree {
            nodes: vec![],
            leaves: vec![leaf(0, 0), leaf(0, 1), leaf(1, 2), leaf(2, 3), leaf(2, 4)],
            root_node_index: 0,
        };
        let ranges = tree.derive_bucket_ranges();
        assert_eq!(ranges.len(), 3);
        assert_eq!(ranges[0].material_bucket_id, 0);
        assert_eq!(ranges[0].first_leaf, 0);
        assert_eq!(ranges[0].leaf_count, 2);
        assert_eq!(ranges[1].material_bucket_id, 1);
        assert_eq!(ranges[1].first_leaf, 2);
        assert_eq!(ranges[1].leaf_count, 1);
        assert_eq!(ranges[2].material_bucket_id, 2);
        assert_eq!(ranges[2].first_leaf, 3);
        assert_eq!(ranges[2].leaf_count, 2);
    }

    #[test]
    fn derive_bucket_ranges_empty_leaves() {
        let tree = BvhTree {
            nodes: vec![],
            leaves: vec![],
            root_node_index: 0,
        };
        assert!(tree.derive_bucket_ranges().is_empty());
    }

    #[test]
    fn derive_bucket_ranges_single_leaf() {
        let tree = BvhTree {
            nodes: vec![],
            leaves: vec![leaf(7, 42)],
            root_node_index: 0,
        };
        let ranges = tree.derive_bucket_ranges();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].material_bucket_id, 7);
        assert_eq!(ranges[0].first_leaf, 0);
        assert_eq!(ranges[0].leaf_count, 1);
    }
}
