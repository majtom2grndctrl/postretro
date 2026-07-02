// CPU-side skinned-mesh visibility and clip metadata.
// See: context/lib/rendering_pipeline.md §9

use postretro_level_loader::LevelWorld;
use postretro_visibility::VisibleCells;

#[derive(Debug, Clone, PartialEq)]
pub struct ClipMetadata {
    pub name: String,
    pub duration: f32,
}

/// Pure cull decision for one skinned-mesh instance — GPU-free, unit-testable.
///
/// An instance draws iff the visible set is `DrawAll`, or the cell its
/// position lands in is a member of the visible cell set. Mirrors the world
/// path's membership test (`cells.contains(&(locate_cell(pos) as u32))`).
///
/// The render-frame mesh collector (`scripting/systems/mesh_render.rs`) calls
/// this (it holds the `LevelWorld` + the frame's `VisibleCells`) before pushing
/// an instance into the draw list, so the renderer's GPU pass never needs a
/// world reference. The cull tests the entity's CURRENT-TICK transform (stable
/// per-tick visibility), not the sub-tick interpolated position. The locator
/// lookup and the membership decision are split so the decision is unit-testable
/// without constructing a full `LevelWorld` (see [`mesh_visible_in_cell`]).
pub fn mesh_visible(world: &LevelWorld, visible: &VisibleCells, pos: glam::Vec3) -> bool {
    // `DrawAll` short-circuits before the cell lookup: every instance draws, so
    // the locator descent is pure waste on that path.
    let VisibleCells::Culled(_) = visible else {
        return true;
    };
    let cell = world.locate_cell(pos) as u32;
    mesh_visible_in_cell(visible, cell)
}

/// Membership half of the cull decision: does `cell_id` draw given `visible`?
/// `DrawAll` always draws; otherwise the cell must be in the visible cell set.
/// Pure data logic — no world, no GPU. Consumed by `mesh_visible` (the
/// collector path) and the cull unit tests.
pub(crate) fn mesh_visible_in_cell(visible: &VisibleCells, cell_id: u32) -> bool {
    match visible {
        VisibleCells::DrawAll => true,
        VisibleCells::Culled(cells) => cells.contains(&cell_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_cull_excludes_instance_in_nonvisible_cell() {
        let visible = VisibleCells::Culled(vec![0]);
        assert!(!mesh_visible_in_cell(&visible, 1));
    }

    #[test]
    fn mesh_cull_includes_instance_in_visible_cell() {
        let visible = VisibleCells::Culled(vec![0, 1]);
        assert!(mesh_visible_in_cell(&visible, 1));
    }

    #[test]
    fn mesh_cull_includes_instance_on_draw_all() {
        assert!(mesh_visible_in_cell(&VisibleCells::DrawAll, 1));
        assert!(mesh_visible_in_cell(&VisibleCells::DrawAll, 999));
    }
}
