// Public walk entries outside the frame loop: the id-51 bake and the mirror.
// See: context/lib/rendering_pipeline.md §2

use glam::{Mat4, Vec3};

use super::{
    MAX_PORTAL_WALK_STEPS, PortalWalkScratch, WalkView, WalkWorld, walk_from_camera,
    walk_from_seeds,
};
use crate::visibility::FrustumPlane;
use postretro_level_loader::LevelWorld;

/// One camera walk's reach, borrowed from the scratch until its next walk.
#[derive(Debug, Clone, Copy)]
pub struct PortalTraversal<'a> {
    /// Cells the walk reached, in reach order, the camera cell first. On a
    /// trip, only cells the uncapped walk also reaches: the walk is
    /// deterministic, so a capped walk is a prefix of the uncapped one.
    pub reached: &'a [u32],
    pub step_limit_hit: bool,
}

/// The camera entry with the runtime cap and no blocked portals, for callers
/// that keep a cap trip's partial walk instead of the frame's frustum
/// fallback (the id-51 bake). `camera_cell` is taken as given, never
/// located; a cell past the world's cell list walks nothing.
pub fn portal_traverse<'a>(
    eye: Vec3,
    view_proj: Mat4,
    camera_cell: usize,
    world: &LevelWorld,
    scratch: &'a mut PortalWalkScratch,
) -> PortalTraversal<'a> {
    portal_traverse_capped(
        eye,
        view_proj,
        camera_cell,
        world,
        MAX_PORTAL_WALK_STEPS,
        scratch,
    )
}

pub(crate) fn portal_traverse_capped<'a>(
    eye: Vec3,
    view_proj: Mat4,
    camera_cell: usize,
    world: &LevelWorld,
    step_limit: u32,
    scratch: &'a mut PortalWalkScratch,
) -> PortalTraversal<'a> {
    let step_limit_hit = if camera_cell < world.cell_count() {
        let input = WalkWorld {
            world,
            blocked_portals: &[],
        };
        let view = WalkView::camera(eye, view_proj);
        walk_from_camera(input, &view, camera_cell, step_limit, scratch).step_limit_hit
    } else {
        scratch.begin(world.cell_count());
        false
    };
    PortalTraversal {
        reached: scratch.reached(),
        step_limit_hit,
    }
}

/// How a seeded walk ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeededWalk<'a> {
    /// The walk drained under the cap. `reached` is every cell it reached,
    /// in reach order, borrowed from the scratch until its next walk; empty
    /// when no seed was usable.
    Completed { reached: &'a [u32] },
    /// The walk hit the cap after `considered` portal tests. No fallback set
    /// comes with it: the caller picks its own.
    Tripped { considered: u32 },
}

/// The seeded entry (`rendering_pipeline.md` §2), for the planar water
/// mirror: walk from caller-given seed cells under the caller's eye,
/// view-projection and near plane, taken as given. It never calls the
/// locator and never slides the near plane; a portal wholly behind
/// `near_plane` is rejected before the camera-on-plane bypass is tested, so
/// the bypass never crosses it. Every usable seed starts with the full
/// screen and is queued before any cell expands, sharing one region state
/// and one cap, so seed order and repeats change nothing. Seeds out of
/// range, solid, exterior or wholly off screen are skipped. The result is
/// the raw reach, a conservative superset per §2; filtering it into draw
/// and fog sets is the caller's.
pub fn portal_walk_seeded<'a>(
    eye: Vec3,
    view_proj: Mat4,
    near_plane: FrustumPlane,
    seeds: &[usize],
    world: &LevelWorld,
    blocked_portals: &[bool],
    scratch: &'a mut PortalWalkScratch,
) -> SeededWalk<'a> {
    let input = WalkWorld {
        world,
        blocked_portals,
    };
    let view = WalkView::seeded(eye, view_proj, near_plane);
    let stats = walk_from_seeds(input, &view, seeds, MAX_PORTAL_WALK_STEPS, scratch);
    if stats.step_limit_hit {
        SeededWalk::Tripped {
            considered: stats.considered,
        }
    } else {
        SeededWalk::Completed {
            reached: scratch.reached(),
        }
    }
}
