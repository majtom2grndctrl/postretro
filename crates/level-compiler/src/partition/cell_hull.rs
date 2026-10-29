// Cell hulls: a runtime cell's region as the convex regions of its member BSP
// leaves. Readers that test a volume against a cell (fog masks, teleport
// routes) and the keys that hash those tests read these, never a single
// plane set, because a connected pocket need not be convex.
// See: context/plans/in-progress/occluder-first-partition/index.md §Decisions (Two tiers in one BSP)

use glam::DVec3;

use super::types::{Aabb, BspLeaf, BspTree};

/// One convex piece of a cell's hull: a member leaf's region.
#[derive(Debug, Clone)]
pub struct HullPiece {
    pub bounds: Aabb,
    /// Inward planes bounding the piece, as `BspLeaf::defining_planes`.
    pub defining_planes: Vec<(DVec3, f64)>,
}

impl HullPiece {
    pub fn of_leaf(leaf: &BspLeaf) -> Self {
        Self {
            bounds: leaf.bounds.clone(),
            defining_planes: leaf.defining_planes.clone(),
        }
    }
}

/// A runtime cell's hull: the union of its pieces. A volume meets the cell
/// when it meets any piece. A solid or exterior cell, and every cell of a
/// tree whose leaves are its cells, has one piece; an interior cell of a
/// two-tier tree has one per interior leaf of its pocket.
#[derive(Debug, Clone, Default)]
pub struct CellHull {
    pub pieces: Vec<HullPiece>,
}

impl CellHull {
    /// One hull per leaf of `tree`, for a tree whose leaves are its cells.
    pub fn per_leaf(tree: &BspTree) -> Vec<Self> {
        tree.leaves
            .iter()
            .map(|leaf| Self {
                pieces: vec![HullPiece::of_leaf(leaf)],
            })
            .collect()
    }
}
