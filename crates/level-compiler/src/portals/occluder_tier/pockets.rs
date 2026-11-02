// Interior pockets: the connected open space inside each occluder-tier leaf,
// the grouping each runtime interior cell is made of.
// See: context/plans/done/occluder-first-partition/plan.md §Block (Task 9), resolved

use std::collections::HashSet;

use glam::DVec3;

use super::super::{PORTAL_EPSILON, PlaneEntry, distribute_winding, polygon_area};
use super::FRAGMENT_DUST_AREA_M2;
use crate::geometry_utils::{clip_winding_to_half_spaces, make_base_winding};
use crate::partition::{BspChild, BspTree, InteriorPockets, OccluderTier};

/// Group each occluder-tier leaf's interior sub-leaves (neither solid nor in
/// `exterior_leaves`) into pockets. Two interior sub-leaves join when they
/// share a face fragment of at least `FRAGMENT_DUST_AREA_M2`, found by
/// pushing every below-tier node's winding through both of its sub-trees,
/// and a pocket is a connected set under that relation. Every below-tier
/// node lies inside one tier leaf, so no pocket crosses a tier boundary.
///
/// The threshold is clipping dust, not the portal-area cull: no runtime
/// portal ever joins two pockets of one tier leaf, so a crack the cull
/// would drop still joins its pockets into one cell, as before cells
/// followed connectivity, and the walk never loses a sightline the
/// unchanged cull lets through. Only solid (or space touching only at
/// clipping dust) parts two pockets.
pub fn interior_pockets(
    tree: &BspTree,
    tier: &OccluderTier,
    exterior_leaves: &HashSet<usize>,
) -> InteriorPockets {
    let mut parent: Vec<usize> = (0..tree.leaves.len()).collect();
    if !tree.nodes.is_empty() {
        join_recursive(tree, tier, exterior_leaves, 0, &[], &mut parent);
    }
    let pocket_of = (0..parent.len())
        .map(|leaf| root(&mut parent, leaf))
        .collect();
    InteriorPockets::new(pocket_of)
}

fn join_recursive(
    tree: &BspTree,
    tier: &OccluderTier,
    exterior_leaves: &HashSet<usize>,
    node_idx: usize,
    ancestor_planes: &[PlaneEntry],
    parent: &mut [usize],
) {
    let node = &tree.nodes[node_idx];
    let (normal, distance) = (node.plane_normal, node.plane_distance);

    if !tier.node_is_tier[node_idx]
        && let Some(winding) = dust_winding(normal, distance, ancestor_planes)
    {
        let interior =
            |leaf: usize| !tree.leaves[leaf].is_solid && !exterior_leaves.contains(&leaf);
        distribute_winding(
            tree,
            &winding,
            &node.front,
            &node.back,
            &|_| true,
            FRAGMENT_DUST_AREA_M2,
            &mut |_, front, back| {
                if let (BspChild::Leaf(f), BspChild::Leaf(b)) = (front, back)
                    && interior(*f)
                    && interior(*b)
                {
                    let (rf, rb) = (root(parent, *f), root(parent, *b));
                    parent[rf.max(rb)] = rf.min(rb);
                }
            },
        );
    }

    for (child, plane) in [
        (&node.front, (normal, distance)),
        (&node.back, (-normal, -distance)),
    ] {
        if let BspChild::Node(child_idx) = child {
            let mut child_ancestors = ancestor_planes.to_vec();
            child_ancestors.push(plane);
            join_recursive(
                tree,
                tier,
                exterior_leaves,
                *child_idx,
                &child_ancestors,
                parent,
            );
        }
    }
}

/// A node's winding clipped to its region, kept down to clipping dust rather
/// than the portal-area cull, so a crack still joins what it opens onto.
fn dust_winding(
    normal: DVec3,
    distance: f64,
    ancestor_planes: &[PlaneEntry],
) -> Option<Vec<DVec3>> {
    let winding = make_base_winding(normal, distance);
    let winding = clip_winding_to_half_spaces(winding, ancestor_planes, PORTAL_EPSILON)?;
    (winding.len() >= 3 && polygon_area(&winding) >= FRAGMENT_DUST_AREA_M2).then_some(winding)
}

/// Union-find root, compressing the path; roots are each set's lowest leaf.
fn root(parent: &mut [usize], mut leaf: usize) -> usize {
    while parent[leaf] != leaf {
        parent[leaf] = parent[parent[leaf]];
        leaf = parent[leaf];
    }
    leaf
}
