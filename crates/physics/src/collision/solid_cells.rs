// Point-in-solid lookup over the compiler's runtime cell locator. Triangle
// queries see only surfaces, so a shape wholly inside a brush touches nothing;
// this answers the enclosed case the way Quake's point-contents test does.
// See: context/lib/movement.md §6 (Teleport) · context/lib/build_pipeline.md §PRL (Cells, id 38)

use glam::Vec3;
use postretro_level_loader::{CellLocatorChild, LevelWorld};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Child {
    Node(u32),
    Cell(u32),
}

#[derive(Debug, Clone, Copy)]
struct Node {
    normal: Vec3,
    distance: f32,
    front: Child,
    back: Child,
}

/// The level's cell locator reduced to what a containment query needs: split
/// planes and one blocked flag per cell. Solid and exterior cells both block.
/// Exterior cells are the void outside the sealed hull, which the compiler
/// strips of geometry; qbsp's outside fill treats that space as solid for the
/// same reason. Built once at install; queries allocate nothing.
#[derive(Debug, Clone, Default)]
pub(crate) struct SolidCells {
    root: Option<Child>,
    nodes: Vec<Node>,
    blocked: Vec<bool>,
}

impl SolidCells {
    /// A level with no cells has no locator, and nothing reads as solid.
    pub(crate) fn from_level(world: &LevelWorld) -> Self {
        if world.cells.is_empty() {
            return Self::default();
        }
        let child = |child: CellLocatorChild| match child {
            CellLocatorChild::Node(index) => Child::Node(index as u32),
            CellLocatorChild::Cell(index) => Child::Cell(index as u32),
        };
        Self {
            root: Some(child(world.cell_locator_root)),
            nodes: world
                .cell_locator_nodes
                .iter()
                .map(|node| Node {
                    normal: node.plane_normal,
                    distance: node.plane_distance,
                    front: child(node.front),
                    back: child(node.back),
                })
                .collect(),
            blocked: world
                .cells
                .iter()
                .map(|cell| cell.is_solid || cell.is_exterior)
                .collect(),
        }
    }

    /// Whether `point` locates to a solid or exterior cell. Descends exactly as
    /// `LevelWorld::locate_cell` does, so a point on a split plane goes front.
    /// Malformed links and non-finite points read as open: the loader already
    /// validated the locator, and the triangle test still guards surfaces.
    pub(crate) fn contains(&self, point: Vec3) -> bool {
        let Some(mut current) = self.root else {
            return false;
        };
        if !point.is_finite() {
            return false;
        }
        // A valid locator is a tree, so a descent visits each node at most once.
        for _ in 0..=self.nodes.len() {
            match current {
                Child::Cell(cell) => {
                    return self.blocked.get(cell as usize).copied().unwrap_or(false);
                }
                Child::Node(node) => {
                    let Some(node) = self.nodes.get(node as usize) else {
                        return false;
                    };
                    let side = node.normal.dot(point) - node.distance;
                    current = if side >= 0.0 { node.front } else { node.back };
                }
            }
        }
        false
    }
}
