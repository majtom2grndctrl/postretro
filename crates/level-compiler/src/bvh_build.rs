// SAH BVH construction and flattening for the level compiler.
// See: context/plans/in-progress/bvh-foundation/1-compile-bvh.md

use bvh::aabb::{Aabb, Bounded};
use bvh::bounding_hierarchy::BHShape;
use bvh::bvh::{Bvh, BvhNode};
use nalgebra::Point3;
use postretro_level_format::bvh::{
    BVH_NODE_FLAG_LEAF, BvhLeaf, BvhNode as FlatNode, BvhSection, derive_bucket_ranges,
};
use postretro_level_format::geometry::GeometrySection;

use crate::geometry::GeometryResult;

/// Runtime visible-cell bitmask capacity. The bitmask is a fixed 4096-bit
/// structure; any leaf with a higher cell id would be silently dropped by the
/// traversal shader, so BVH construction rejects it at compile time.
const MAX_CELL_ID_EXCLUSIVE: u32 = 4096;

#[derive(Debug, thiserror::Error)]
pub enum BvhBuildError {
    #[error(
        "cell_id {cell_id} exceeds the runtime visible-cell bitmask capacity ({max}); \
         the map has more than {max} BSP leaves contributing geometry and cannot be rendered"
    )]
    CellIdOutOfRange { cell_id: u32, max: u32 },
}

/// One primitive fed to the BVH builder: a contiguous index-buffer slice for a
/// single `(face, material_bucket)` pair. `material_bucket_id` is currently
/// the face's texture index; the struct shape is stable if bucket criteria expand.
#[derive(Debug, Clone)]
pub struct BvhPrimitive {
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
    pub cell_id: u32,
    pub material_bucket_id: u32,
    pub index_offset: u32,
    pub index_count: u32,
    /// Stable sort key for deterministic builder input regardless of face interleaving order.
    pub sort_key: u64,
    /// Written by `Bvh::build`; not set by this module.
    node_index: usize,
}

impl Bounded<f32, 3> for BvhPrimitive {
    fn aabb(&self) -> Aabb<f32, 3> {
        Aabb::with_bounds(
            Point3::new(self.aabb_min[0], self.aabb_min[1], self.aabb_min[2]),
            Point3::new(self.aabb_max[0], self.aabb_max[1], self.aabb_max[2]),
        )
    }
}

impl BHShape<f32, 3> for BvhPrimitive {
    fn set_bh_node_index(&mut self, index: usize) {
        self.node_index = index;
    }

    fn bh_node_index(&self) -> usize {
        self.node_index
    }
}

/// Collect one `BvhPrimitive` per face and sort by stable key before returning.
/// Sorting here guarantees deterministic SAH builder input regardless of geometry
/// extractor iteration order.
pub fn collect_primitives(geo: &GeometryResult) -> Vec<BvhPrimitive> {
    let section = &geo.geometry;
    let mut primitives: Vec<BvhPrimitive> = Vec::with_capacity(section.faces.len());

    for (face_idx, face) in section.faces.iter().enumerate() {
        let range = geo.face_index_ranges[face_idx];
        if range.index_count == 0 {
            continue;
        }

        let (aabb_min, aabb_max) = face_aabb(section, range.index_offset, range.index_count);

        primitives.push(BvhPrimitive {
            aabb_min,
            aabb_max,
            cell_id: face.leaf_index,
            material_bucket_id: face.texture_index,
            index_offset: range.index_offset,
            index_count: range.index_count,
            sort_key: primitive_sort_key(face.texture_index, face.leaf_index, range.index_offset),
            node_index: 0,
        });
    }

    primitives.sort_by_key(|p| p.sort_key);
    primitives
}

fn primitive_sort_key(material_bucket_id: u32, cell_id: u32, index_offset: u32) -> u64 {
    // Material in high bits clusters same-bucket primitives after sort.
    // index_offset breaks ties. Texture indices and cell ids are well under
    // 2^20 for realistic maps so 64 bits is sufficient.
    ((material_bucket_id as u64) << 40)
        | ((cell_id as u64 & 0xF_FFFF) << 20)
        | (index_offset as u64 & 0xF_FFFF)
}

fn face_aabb(
    section: &GeometrySection,
    index_offset: u32,
    index_count: u32,
) -> ([f32; 3], [f32; 3]) {
    let start = index_offset as usize;
    let end = start + index_count as usize;
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for &vertex_idx in &section.indices[start..end] {
        let pos = &section.vertices[vertex_idx as usize].position;
        for i in 0..3 {
            min[i] = min[i].min(pos[i]);
            max[i] = max[i].max(pos[i]);
        }
    }
    (min, max)
}

/// Build a SAH BVH and flatten it into `BvhSection`.
///
/// Returns the live `bvh::Bvh` and primitive vector alongside the flattened
/// section so callers (e.g. SH baker) can traverse the tree on the CPU without
/// rebuilding from the PRL file. Leaves in `BvhSection` are sorted so each
/// material bucket owns a contiguous range.
pub fn build_bvh(
    geo: &GeometryResult,
) -> Result<(Bvh<f32, 3>, Vec<BvhPrimitive>, BvhSection), BvhBuildError> {
    let mut primitives = collect_primitives(geo);

    for prim in &primitives {
        if prim.cell_id >= MAX_CELL_ID_EXCLUSIVE {
            return Err(BvhBuildError::CellIdOutOfRange {
                cell_id: prim.cell_id,
                max: MAX_CELL_ID_EXCLUSIVE,
            });
        }
    }

    if primitives.is_empty() {
        return Ok((
            Bvh { nodes: Vec::new() },
            primitives,
            BvhSection {
                nodes: Vec::new(),
                leaves: Vec::new(),
                root_node_index: 0,
            },
        ));
    }

    let bvh = Bvh::build(&mut primitives);
    let section = flatten(&bvh, &primitives);
    Ok((bvh, primitives, section))
}

/// Flatten `bvh::Bvh` into the dense `BvhSection` layout.
///
/// Emits one flat node per BVH node in DFS order. Leaf nodes also get a
/// `BvhLeaf` entry. Leaves are stable-sorted by `material_bucket_id` so each
/// bucket owns a contiguous range; `left_child_or_leaf_index` on each leaf
/// node is patched to point at the post-sort slot.
///
/// `skip_index` is the "skip to next sibling" pointer used by the stack-free
/// WGSL traversal shader: the flat slot at which DFS resumes after finishing
/// the current subtree.
fn flatten(bvh: &Bvh<f32, 3>, primitives: &[BvhPrimitive]) -> BvhSection {
    let src_nodes = &bvh.nodes;

    // Iterative DFS to avoid stack overflow on deep trees.
    let mut flat_index_of: Vec<u32> = vec![u32::MAX; src_nodes.len()];
    let mut dfs_order: Vec<usize> = Vec::with_capacity(src_nodes.len());
    let mut stack: Vec<usize> = Vec::with_capacity(64);
    stack.push(0);
    while let Some(src_idx) = stack.pop() {
        flat_index_of[src_idx] = dfs_order.len() as u32;
        dfs_order.push(src_idx);
        if let BvhNode::Node {
            child_l_index,
            child_r_index,
            ..
        } = src_nodes[src_idx]
        {
            // Right pushed first so left is popped first.
            stack.push(child_r_index);
            stack.push(child_l_index);
        }
    }

    // Leaf nodes point at unsorted-leaf slots; patched after the sort below.
    let mut flat_nodes: Vec<FlatNode> = Vec::with_capacity(src_nodes.len());
    let mut unsorted_leaves: Vec<BvhLeaf> = Vec::new();
    let mut leaf_slot_for_flat: Vec<Option<u32>> = Vec::with_capacity(src_nodes.len());

    for (flat_idx, &src_idx) in dfs_order.iter().enumerate() {
        match src_nodes[src_idx] {
            BvhNode::Leaf { shape_index, .. } => {
                let prim = &primitives[shape_index];
                let unsorted_slot = unsorted_leaves.len() as u32;
                unsorted_leaves.push(BvhLeaf {
                    aabb_min: prim.aabb_min,
                    material_bucket_id: prim.material_bucket_id,
                    aabb_max: prim.aabb_max,
                    index_offset: prim.index_offset,
                    index_count: prim.index_count,
                    cell_id: prim.cell_id,
                    // Stamped at serialization time by the animated-light-chunks
                    // builder; zero for maps without animated lights.
                    chunk_range_start: 0,
                    chunk_range_count: 0,
                });
                flat_nodes.push(FlatNode {
                    aabb_min: prim.aabb_min,
                    skip_index: 0, // patched in subtree_end pass below
                    aabb_max: prim.aabb_max,
                    left_child_or_leaf_index: unsorted_slot,
                    flags: BVH_NODE_FLAG_LEAF,
                    _padding: 0,
                });
                leaf_slot_for_flat.push(Some(unsorted_slot));
            }
            BvhNode::Node {
                child_l_index,
                child_l_aabb,
                child_r_aabb,
                ..
            } => {
                let min_x = child_l_aabb.min.x.min(child_r_aabb.min.x);
                let min_y = child_l_aabb.min.y.min(child_r_aabb.min.y);
                let min_z = child_l_aabb.min.z.min(child_r_aabb.min.z);
                let max_x = child_l_aabb.max.x.max(child_r_aabb.max.x);
                let max_y = child_l_aabb.max.y.max(child_r_aabb.max.y);
                let max_z = child_l_aabb.max.z.max(child_r_aabb.max.z);
                flat_nodes.push(FlatNode {
                    aabb_min: [min_x, min_y, min_z],
                    skip_index: 0, // patched in subtree_end pass below
                    aabb_max: [max_x, max_y, max_z],
                    left_child_or_leaf_index: 0, // unused for internal nodes
                    flags: 0,
                    _padding: 0,
                });
                leaf_slot_for_flat.push(None);

                // DFS invariant: left child immediately follows parent in flat order.
                debug_assert_eq!(
                    flat_index_of[child_l_index],
                    flat_idx as u32 + 1,
                    "DFS invariant violated: left child is not current + 1"
                );
            }
        }
    }

    // Compute subtree_end[f] = one past the last flat slot in f's subtree.
    // Leaf: flat_idx + 1. Internal: subtree_end of right child.
    // Reverse pass works because children always appear after their parent in
    // flat order, so a child's subtree_end is already set when we reach the parent.
    let mut subtree_end = vec![0u32; flat_nodes.len()];
    for flat_idx in (0..flat_nodes.len()).rev() {
        let src_idx = dfs_order[flat_idx];
        match src_nodes[src_idx] {
            BvhNode::Leaf { .. } => {
                subtree_end[flat_idx] = flat_idx as u32 + 1;
            }
            BvhNode::Node { child_r_index, .. } => {
                let right_flat = flat_index_of[child_r_index] as usize;
                subtree_end[flat_idx] = subtree_end[right_flat];
            }
        }
    }

    for flat_idx in 0..flat_nodes.len() {
        flat_nodes[flat_idx].skip_index = subtree_end[flat_idx];
    }

    // Stable sort by (bucket, cell, index_offset) so identical inputs produce
    // identical output and each bucket owns a contiguous leaf range.
    let mut sorted_order: Vec<u32> = (0..unsorted_leaves.len() as u32).collect();
    sorted_order.sort_by_key(|&i| {
        let leaf = &unsorted_leaves[i as usize];
        (leaf.material_bucket_id, leaf.cell_id, leaf.index_offset)
    });
    let mut new_slot_of = vec![0u32; unsorted_leaves.len()];
    for (new_slot, &old_slot) in sorted_order.iter().enumerate() {
        new_slot_of[old_slot as usize] = new_slot as u32;
    }
    let leaves: Vec<BvhLeaf> = sorted_order
        .iter()
        .map(|&old| unsorted_leaves[old as usize])
        .collect();

    // Patch leaf nodes to point at their post-sort slot.
    for (flat_idx, maybe_slot) in leaf_slot_for_flat.iter().enumerate() {
        if let Some(old_slot) = maybe_slot {
            flat_nodes[flat_idx].left_child_or_leaf_index = new_slot_of[*old_slot as usize];
        }
    }

    BvhSection {
        nodes: flat_nodes,
        leaves,
        root_node_index: 0,
    }
}

pub fn log_stats(section: &BvhSection) {
    let internal = section
        .nodes
        .iter()
        .filter(|n| n.flags & BVH_NODE_FLAG_LEAF == 0)
        .count();
    let leaves = section.nodes.len() - internal;
    log::info!(
        "Bvh: {} nodes ({} internal, {} leaf), {} leaf entries",
        section.nodes.len(),
        internal,
        leaves,
        section.leaves.len()
    );
    if !section.leaves.is_empty() {
        let bucket_count = derive_bucket_ranges(&section.leaves).len();
        log::info!("  Material buckets: {bucket_count}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::bvh::BVH_NODE_FLAG_LEAF;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;

    use crate::geometry::FaceIndexRange;

    fn make_geometry(
        positions: &[[f32; 3]],
        faces: &[(u32, u32, u32, u32)], // (index_offset, index_count, leaf_index, texture_index)
        indices: &[u32],
    ) -> GeometryResult {
        let vertices: Vec<Vertex> = positions
            .iter()
            .map(|&pos| {
                Vertex::new(
                    pos,
                    [0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [1.0, 0.0, 0.0],
                    true,
                    [0.0, 0.0],
                )
            })
            .collect();
        let face_metas: Vec<FaceMeta> = faces
            .iter()
            .map(|&(_, _, leaf_index, texture_index)| FaceMeta {
                leaf_index,
                texture_index,
            })
            .collect();
        let face_index_ranges: Vec<FaceIndexRange> = faces
            .iter()
            .map(|&(index_offset, index_count, _, _)| FaceIndexRange {
                index_offset,
                index_count,
            })
            .collect();
        GeometryResult {
            geometry: GeometrySection {
                vertices,
                indices: indices.to_vec(),
                faces: face_metas,
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges,
        }
    }

    #[test]
    fn single_face_single_leaf() {
        let geo = make_geometry(
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            &[(0, 3, 0, 0)],
            &[0, 1, 2],
        );
        let (_bvh, _prims, section) = build_bvh(&geo).unwrap();
        assert_eq!(section.leaves.len(), 1);
        assert_eq!(section.nodes.len(), 1);
        assert_eq!(
            section.nodes[0].flags & BVH_NODE_FLAG_LEAF,
            BVH_NODE_FLAG_LEAF
        );
        assert_eq!(section.leaves[0].cell_id, 0);
        assert_eq!(section.leaves[0].material_bucket_id, 0);
        assert_eq!(section.leaves[0].index_offset, 0);
        assert_eq!(section.leaves[0].index_count, 3);
    }

    #[test]
    fn deterministic_build() {
        let geo = multi_face_geometry();
        let (_, _, a) = build_bvh(&geo).unwrap();
        let (_, _, b) = build_bvh(&geo).unwrap();
        assert_eq!(a.to_bytes(), b.to_bytes(), "BVH build is not deterministic");
    }

    #[test]
    fn deterministic_across_input_reorder() {
        // Same primitives in reverse order must produce identical bytes.
        // The reversed faces/face_index_ranges are consistent because each face
        // still points at its original index slice.
        let geo_forward = multi_face_geometry();
        let mut geo_reverse = multi_face_geometry();
        geo_reverse.geometry.faces.reverse();
        geo_reverse.face_index_ranges.reverse();
        let (_, _, a) = build_bvh(&geo_forward).unwrap();
        let (_, _, b) = build_bvh(&geo_reverse).unwrap();
        assert_eq!(
            a.to_bytes(),
            b.to_bytes(),
            "BVH output depends on primitive feed order"
        );
    }

    #[test]
    fn leaves_sorted_by_material_bucket() {
        let geo = multi_face_geometry();
        let (_, _, section) = build_bvh(&geo).unwrap();
        for w in section.leaves.windows(2) {
            assert!(
                w[0].material_bucket_id <= w[1].material_bucket_id,
                "leaves not sorted by material bucket"
            );
        }
    }

    #[test]
    fn every_triangle_appears_in_exactly_one_leaf() {
        let geo = multi_face_geometry();
        let total_indices: u32 = geo.face_index_ranges.iter().map(|r| r.index_count).sum();

        let (_, _, section) = build_bvh(&geo).unwrap();

        let leaf_indices: u32 = section.leaves.iter().map(|l| l.index_count).sum();
        assert_eq!(leaf_indices, total_indices, "leaf coverage mismatch");

        let mut seen = vec![0u8; geo.geometry.indices.len()];
        for leaf in &section.leaves {
            for slot in leaf.index_offset..leaf.index_offset + leaf.index_count {
                seen[slot as usize] += 1;
            }
        }
        for (slot, count) in seen.iter().enumerate() {
            assert_eq!(*count, 1, "index slot {slot} covered {count} times");
        }
    }

    #[test]
    fn leaf_aabbs_tightly_bound_geometry() {
        let geo = multi_face_geometry();
        let (_, _, section) = build_bvh(&geo).unwrap();

        for leaf in &section.leaves {
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            for slot in leaf.index_offset..leaf.index_offset + leaf.index_count {
                let vertex_idx = geo.geometry.indices[slot as usize] as usize;
                let pos = geo.geometry.vertices[vertex_idx].position;
                for i in 0..3 {
                    min[i] = min[i].min(pos[i]);
                    max[i] = max[i].max(pos[i]);
                }
            }
            assert_eq!(leaf.aabb_min, min);
            assert_eq!(leaf.aabb_max, max);
        }
    }

    #[test]
    fn round_trip_through_format_crate() {
        let geo = multi_face_geometry();
        let (_, _, section) = build_bvh(&geo).unwrap();
        let bytes = section.to_bytes();
        let restored = BvhSection::from_bytes(&bytes).expect("round-trip should succeed");
        assert_eq!(section, restored);
    }

    #[test]
    fn build_bvh_rejects_cell_id_at_or_above_limit() {
        let geo = make_geometry(
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            &[(0, 3, MAX_CELL_ID_EXCLUSIVE, 0)],
            &[0, 1, 2],
        );
        match build_bvh(&geo) {
            Err(BvhBuildError::CellIdOutOfRange { cell_id, max }) => {
                assert_eq!(cell_id, MAX_CELL_ID_EXCLUSIVE);
                assert_eq!(max, MAX_CELL_ID_EXCLUSIVE);
            }
            Ok(_) => panic!("expected CellIdOutOfRange error"),
        }
    }

    #[test]
    fn build_bvh_accepts_cell_id_just_below_limit() {
        let geo = make_geometry(
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            &[(0, 3, MAX_CELL_ID_EXCLUSIVE - 1, 0)],
            &[0, 1, 2],
        );
        let (_, _, section) = build_bvh(&geo).expect("cell_id = limit - 1 must be accepted");
        assert_eq!(section.leaves[0].cell_id, MAX_CELL_ID_EXCLUSIVE - 1);
    }

    #[test]
    fn empty_geometry_produces_empty_section() {
        let geo = GeometryResult {
            geometry: GeometrySection {
                vertices: Vec::new(),
                indices: Vec::new(),
                faces: Vec::new(),
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: Vec::new(),
        };
        let (_, prims, section) = build_bvh(&geo).unwrap();
        assert!(prims.is_empty());
        assert!(section.nodes.is_empty());
        assert!(section.leaves.is_empty());
    }

    #[test]
    fn skip_index_past_subtree() {
        let geo = multi_face_geometry();
        let (_, _, section) = build_bvh(&geo).unwrap();

        let total = section.nodes.len() as u32;
        for (idx, node) in section.nodes.iter().enumerate() {
            assert!(
                node.skip_index > idx as u32,
                "node {idx} has skip_index {} <= current",
                node.skip_index
            );
            assert!(node.skip_index <= total, "node {idx} skip_index past end");
        }
    }

    fn multi_face_geometry() -> GeometryResult {
        // 4 faces · 3 material buckets · 2 cells; vertices spread for SAH.
        let positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [10.0, 10.0, 10.0],
            [11.0, 10.0, 10.0],
            [10.0, 11.0, 10.0],
            [20.0, 0.0, 5.0],
            [21.0, 0.0, 5.0],
            [20.0, 1.0, 5.0],
            [0.0, 20.0, -5.0],
            [1.0, 20.0, -5.0],
            [0.0, 21.0, -5.0],
        ];
        let faces = vec![
            (0u32, 3u32, 0u32, 2u32), // cell 0, bucket 2
            (3u32, 3u32, 0u32, 0u32), // cell 0, bucket 0
            (6u32, 3u32, 1u32, 1u32), // cell 1, bucket 1
            (9u32, 3u32, 1u32, 0u32), // cell 1, bucket 0
        ];
        let indices: Vec<u32> = (0u32..12u32).collect();
        make_geometry(&positions, &faces, &indices)
    }
}
