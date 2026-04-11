// Point-to-leaf descent for compiled BSP trees. Tree construction lives in
// `brush_bsp.rs` and face emission in `face_extract.rs`; this module owns
// only the runtime point lookup that tests and diagnostics share.
// See: context/lib/build_pipeline.md §PRL Compilation

use glam::DVec3;

use super::types::*;

/// Walk the BSP tree to find which leaf contains a given point.
///
/// Sign convention: `dot(point, node.plane_normal) - node.plane_distance >= 0`
/// descends to the front child; negative descends to the back child. Zero
/// goes to the front, matching the brush-side projection rule that a polygon
/// lying on a node's plane routes to the front child only.
/// Returns leaf index 0 for an empty tree.
pub fn find_leaf_for_point(tree: &BspTree, point: DVec3) -> usize {
    if tree.nodes.is_empty() {
        return 0;
    }
    let mut child = BspChild::Node(0);
    loop {
        match child {
            BspChild::Leaf(idx) => return idx,
            BspChild::Node(idx) => {
                let node = &tree.nodes[idx];
                let dist = point.dot(node.plane_normal) - node.plane_distance;
                child = if dist >= 0.0 {
                    node.front.clone()
                } else {
                    node.back.clone()
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_leaf_for_point_empty_tree_returns_zero() {
        let tree = BspTree {
            nodes: Vec::new(),
            leaves: Vec::new(),
        };
        // Empty tree degenerate case: no nodes, no leaves — function returns 0.
        // Callers are expected to handle empty trees before descending.
        assert_eq!(find_leaf_for_point(&tree, DVec3::ZERO), 0);
    }

    #[test]
    fn find_leaf_for_point_walks_single_plane_tree() {
        // Trivial tree: one splitting plane on X=0 with two leaf children.
        // Back (negative X) is leaf 0, front (positive X or zero) is leaf 1.
        let tree = BspTree {
            nodes: vec![BspNode {
                plane_normal: DVec3::X,
                plane_distance: 0.0,
                front: BspChild::Leaf(1),
                back: BspChild::Leaf(0),
                parent: None,
            }],
            leaves: vec![
                BspLeaf {
                    face_indices: vec![],
                    bounds: Aabb::empty(),
                    is_solid: false,
                },
                BspLeaf {
                    face_indices: vec![],
                    bounds: Aabb::empty(),
                    is_solid: false,
                },
            ],
        };

        // Point strictly in the back half-space.
        assert_eq!(find_leaf_for_point(&tree, DVec3::new(-5.0, 0.0, 0.0)), 0);
        // Point strictly in the front half-space.
        assert_eq!(find_leaf_for_point(&tree, DVec3::new(5.0, 0.0, 0.0)), 1);
        // Point exactly on the plane descends to the front (plan sign convention).
        assert_eq!(find_leaf_for_point(&tree, DVec3::ZERO), 1);
    }
}
