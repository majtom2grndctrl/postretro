// Cell tree: the leaf → cell chokepoint of a two-tier BSP. Every reader of
// runtime cell ids reads this view; readers that need exact sub-leaf
// solidity keep the full tree.
// See: context/plans/in-progress/occluder-first-partition/plan.md §Approach notes

use std::collections::HashSet;

use super::cell_hull::{CellHull, HullPiece};
use super::types::{Aabb, BspChild, BspLeaf, BspNode, BspTree};
use super::{OccluderTier, TierLeaf};

/// The connected interior pockets of each occluder-tier leaf: per full-tree
/// leaf, the pocket it belongs to, named by the pocket's lowest full-tree
/// leaf index; a solid or exterior leaf names itself. Only interior leaves'
/// entries are read, and a pocket never spans two occluder-tier leaves.
#[derive(Debug, Clone)]
pub struct InteriorPockets {
    pocket_of: Vec<usize>,
}

impl InteriorPockets {
    /// `pocket_of[leaf]` names `leaf`'s pocket by its lowest member, so it is
    /// never greater than `leaf`.
    pub fn new(pocket_of: Vec<usize>) -> Self {
        debug_assert!(
            pocket_of
                .iter()
                .enumerate()
                .all(|(leaf, &pocket)| pocket <= leaf && pocket_of[pocket] == pocket),
            "a pocket is named by its lowest leaf"
        );
        Self { pocket_of }
    }

    /// One pocket per occluder-tier leaf, whatever solid parts its interior:
    /// the grouping before cells followed connectivity.
    #[cfg(test)]
    pub fn one_per_tier_leaf(
        tree: &BspTree,
        tier: &OccluderTier,
        exterior_leaves: &HashSet<usize>,
    ) -> Self {
        let mut pocket_of: Vec<usize> = (0..tree.leaves.len()).collect();
        for tier_leaf in &tier.leaves {
            let mut interior = tier_leaf
                .leaf_range
                .clone()
                .filter(|&leaf| !tree.leaves[leaf].is_solid && !exterior_leaves.contains(&leaf));
            if let Some(first) = interior.next() {
                for leaf in interior {
                    pocket_of[leaf] = first;
                }
            }
        }
        Self::new(pocket_of)
    }

    pub fn pocket_of(&self, leaf: usize) -> usize {
        self.pocket_of[leaf]
    }
}

/// A `BspTree`-shaped view of a two-tier tree whose leaves are runtime cells.
/// Nodes and planes are the full tree's, so a point descends to the same
/// place; each leaf child names the cell that full-tree leaf belongs to,
/// many leaves to one. Each connected interior pocket of an occluder-tier
/// leaf is one interior cell, holding the pocket's non-solid, non-exterior
/// sub-leaves; each solid and each exterior sub-leaf stays its own cell, so
/// a point's solidity and exterior class read the same on the view as on the
/// full tree.
#[derive(Debug)]
pub struct CellTree {
    /// The view. `leaves[c]` is cell `c`: an interior cell's planes are its
    /// occluder-tier leaf's, a convex region holding the cell, its bounds
    /// enclose every member leaf, and its faces are the members' in
    /// full-tree leaf order; a solid or exterior cell is a copy of its one
    /// leaf. A cell's exact region is `hulls[c]`, the only region readers may
    /// treat as the cell: an interior cell's `defining_planes` are its tier
    /// leaf's, which for a pocket also bound sibling pockets and solid.
    pub tree: BspTree,
    /// Per cell, its hull: the convex regions of its member leaves.
    pub hulls: Vec<CellHull>,
    /// Exterior cells: the full tree's exterior leaves, in cell ids.
    pub exterior_cells: HashSet<usize>,
    /// Per full-tree leaf, its cell.
    pub leaf_cell: Vec<usize>,
    /// Per occluder-tier leaf, its interior cells ascending, one per pocket.
    pub tier_leaf_cells: Vec<Vec<usize>>,
}

impl CellTree {
    /// Group `tree`'s leaves into cells by `tier` and `pockets`. Cell ids
    /// follow full-tree leaf order, an interior cell taking the id where its
    /// pocket's first member appears, so a tier whose leaves are each one
    /// full-tree leaf (every brush an occluder) gives every leaf its own id.
    pub fn build(
        tree: &BspTree,
        tier: &OccluderTier,
        exterior_leaves: &HashSet<usize>,
        pockets: &InteriorPockets,
    ) -> Self {
        let mut leaves: Vec<BspLeaf> = Vec::new();
        let mut hulls: Vec<CellHull> = Vec::new();
        let mut exterior_cells = HashSet::new();
        let mut leaf_cell = Vec::with_capacity(tree.leaves.len());
        let mut tier_leaf_cells: Vec<Vec<usize>> = vec![Vec::new(); tier.leaves.len()];

        for (index, leaf) in tree.leaves.iter().enumerate() {
            let exterior = exterior_leaves.contains(&index);
            if leaf.is_solid || exterior {
                if exterior {
                    exterior_cells.insert(leaves.len());
                }
                leaf_cell.push(leaves.len());
                leaves.push(leaf.clone());
                hulls.push(CellHull {
                    pieces: vec![HullPiece::of_leaf(leaf)],
                });
                continue;
            }
            let tier_leaf = tier.leaf_tier_leaf[index];
            let pocket = pockets.pocket_of(index);
            let cell = if pocket == index {
                leaves.push(interior_cell(&tier.leaves[tier_leaf]));
                hulls.push(CellHull::default());
                tier_leaf_cells[tier_leaf].push(leaves.len() - 1);
                leaves.len() - 1
            } else {
                debug_assert_eq!(
                    tier.leaf_tier_leaf[pocket], tier_leaf,
                    "a pocket lies in one occluder-tier leaf"
                );
                leaf_cell[pocket]
            };
            let entry = &mut leaves[cell];
            entry.bounds.expand_aabb(&leaf.bounds);
            entry.face_indices.extend_from_slice(&leaf.face_indices);
            hulls[cell].pieces.push(HullPiece::of_leaf(leaf));
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
            hulls,
            exterior_cells,
            leaf_cell,
            tier_leaf_cells,
        }
    }

    /// Interior cells: neither solid nor exterior.
    pub fn interior_cell_count(&self) -> usize {
        self.tier_leaf_cells.iter().map(Vec::len).sum()
    }

    /// Occluder-tier leaves whose interior falls into more than one pocket.
    pub fn split_tier_leaf_count(&self) -> usize {
        self.tier_leaf_cells
            .iter()
            .filter(|cells| cells.len() > 1)
            .count()
    }
}

/// An interior cell before its members join: the tier leaf's planes, empty
/// bounds and no faces. The planes are the tier leaf's, not the pocket's; the
/// cell's region is its `CellHull` pieces.
fn interior_cell(tier_leaf: &TierLeaf) -> BspLeaf {
    BspLeaf {
        face_indices: Vec::new(),
        bounds: Aabb::empty(),
        is_solid: false,
        defining_planes: tier_leaf.defining_planes.clone(),
    }
}
