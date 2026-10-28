// Cell tree: the leaf → cell chokepoint of a two-tier BSP. Every reader of
// runtime cell ids reads this view; readers that need exact sub-leaf
// solidity keep the full tree.
// See: context/plans/in-progress/occluder-first-partition/plan.md §Approach notes

use std::collections::HashSet;

use super::types::{Aabb, BspChild, BspLeaf, BspNode, BspTree};
use super::{OccluderTier, TierLeaf};

/// A `BspTree`-shaped view of a two-tier tree whose leaves are runtime cells.
/// Nodes and planes are the full tree's, so a point descends to the same
/// place; each leaf child names the cell that full-tree leaf belongs to,
/// many leaves to one. An occluder-tier leaf gives at most one interior cell,
/// holding all its non-solid, non-exterior sub-leaves; each solid and each
/// exterior sub-leaf stays its own cell, so a point's solidity and exterior
/// class read the same on the view as on the full tree, and every cell's
/// hull stays convex.
#[derive(Debug)]
pub struct CellTree {
    /// The view. `leaves[c]` is cell `c`: an interior cell's planes are its
    /// occluder-tier leaf's, its bounds enclose every member leaf, and its
    /// faces are the members' in full-tree leaf order; a solid or exterior
    /// cell is a copy of its one leaf.
    pub tree: BspTree,
    /// Exterior cells: the full tree's exterior leaves, in cell ids.
    pub exterior_cells: HashSet<usize>,
    /// Per full-tree leaf, its cell.
    pub leaf_cell: Vec<usize>,
    /// Per occluder-tier leaf, its interior cell, if any of its sub-leaves
    /// is interior.
    pub tier_leaf_cell: Vec<Option<usize>>,
}

impl CellTree {
    /// Group `tree`'s leaves into cells by `tier`. Cell ids follow full-tree
    /// leaf order, an interior cell taking the id where its first member
    /// appears, so a tier whose leaves are each one full-tree leaf (every
    /// brush an occluder) gives every leaf its own id.
    pub fn build(tree: &BspTree, tier: &OccluderTier, exterior_leaves: &HashSet<usize>) -> Self {
        let mut leaves: Vec<BspLeaf> = Vec::new();
        let mut exterior_cells = HashSet::new();
        let mut leaf_cell = Vec::with_capacity(tree.leaves.len());
        let mut tier_leaf_cell: Vec<Option<usize>> = vec![None; tier.leaves.len()];

        for (index, leaf) in tree.leaves.iter().enumerate() {
            let exterior = exterior_leaves.contains(&index);
            if leaf.is_solid || exterior {
                if exterior {
                    exterior_cells.insert(leaves.len());
                }
                leaf_cell.push(leaves.len());
                leaves.push(leaf.clone());
                continue;
            }
            let tier_leaf = tier.leaf_tier_leaf[index];
            let cell = *tier_leaf_cell[tier_leaf].get_or_insert_with(|| {
                leaves.push(interior_cell(&tier.leaves[tier_leaf]));
                leaves.len() - 1
            });
            let entry = &mut leaves[cell];
            entry.bounds.expand_aabb(&leaf.bounds);
            entry.face_indices.extend_from_slice(&leaf.face_indices);
            leaf_cell.push(cell);
        }

        let child = |child: &BspChild| match child {
            BspChild::Node(node) => BspChild::Node(*node),
            BspChild::Leaf(leaf) => BspChild::Leaf(leaf_cell[*leaf]),
        };
        let nodes = tree
            .nodes
            .iter()
            .map(|node| BspNode {
                plane_normal: node.plane_normal,
                plane_distance: node.plane_distance,
                front: child(&node.front),
                back: child(&node.back),
                parent: node.parent,
            })
            .collect();

        Self {
            tree: BspTree { nodes, leaves },
            exterior_cells,
            leaf_cell,
            tier_leaf_cell,
        }
    }

    /// Interior cells: neither solid nor exterior.
    pub fn interior_cell_count(&self) -> usize {
        self.tier_leaf_cell.iter().flatten().count()
    }
}

/// An interior cell before its members join: the tier leaf's hull, empty
/// bounds and no faces.
fn interior_cell(tier_leaf: &TierLeaf) -> BspLeaf {
    BspLeaf {
        face_indices: Vec::new(),
        bounds: Aabb::empty(),
        is_solid: false,
        defining_planes: tier_leaf.defining_planes.clone(),
    }
}
