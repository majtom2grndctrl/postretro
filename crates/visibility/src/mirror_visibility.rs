// Seeded mirror visibility, independent of the mirror apex's locator cell.
// See: context/plans/in-progress/planar-water-spike-contract.md

use glam::{Mat4, Vec3, Vec4};
use postretro_level_loader::LevelWorld;
use postretro_stage_timing::{StageFrame, TimingGate};

use crate::visibility::{
    Frustum, FrustumPlane, NEAR_PLANE_INDEX, VisibleCells, extract_frustum_planes,
    is_aabb_outside_frustum,
};
use crate::{cpu_stages::VisibilityStage, portal_vis};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorVisibilityPath {
    EmptySeeds,
    Seeded,
    StepLimitFallback,
    NoPortalsFallback,
}

#[derive(Debug)]
pub struct MirrorVisibilityResult {
    pub visible_cells: VisibleCells,
    /// Includes empty cells for lighting residency and entity collection.
    pub reachable_cells: Vec<u32>,
    pub path: MirrorVisibilityPath,
    pub considered: u32,
    pub accepted: u32,
    pub cpu: StageFrame<VisibilityStage>,
}

/// Flood from main-visible water cells. Never calls `locate_cell`.
/// `clip_plane` is a world half-space, inward toward reflected content.
/// `cull_vp` carries D as its far plane; the water plane replaces its near.
/// Reclaim `VisibleCells::Culled` into `scratch` after rendering.
#[allow(clippy::too_many_arguments)]
pub fn determine_mirror_visible_cells(
    mirror_eye: Vec3,
    cull_vp: Mat4,
    clip_plane: Vec4,
    seeds: &[u32],
    world: &LevelWorld,
    blocked_portals: &[bool],
    scratch: &mut Vec<u32>,
    timing: TimingGate,
) -> (MirrorVisibilityResult, Frustum) {
    determine_with_step_limit(
        mirror_eye,
        cull_vp,
        clip_plane,
        seeds,
        world,
        blocked_portals,
        scratch,
        timing,
        portal_vis::MAX_PORTAL_WALK_STEPS,
    )
}

#[allow(clippy::too_many_arguments)]
fn determine_with_step_limit(
    mirror_eye: Vec3,
    cull_vp: Mat4,
    clip_plane: Vec4,
    seeds: &[u32],
    world: &LevelWorld,
    blocked_portals: &[bool],
    scratch: &mut Vec<u32>,
    timing: TimingGate,
    step_limit: u32,
) -> (MirrorVisibilityResult, Frustum) {
    let mut frustum = extract_frustum_planes(cull_vp);
    let length = clip_plane.truncate().length();
    if length.is_finite() && length > 0.0 {
        frustum.planes[NEAR_PLANE_INDEX] = FrustumPlane {
            normal: clip_plane.truncate() / length,
            dist: clip_plane.w / length,
        };
    }
    scratch.clear();
    let cpu = StageFrame::new(timing);
    let valid_seed = |&seed: &u32| {
        world.cells.get(seed as usize).is_some_and(|cell| {
            !cell.is_solid
                && !cell.is_exterior
                && !is_aabb_outside_frustum(cell.bounds_min, cell.bounds_max, &frustum)
        })
    };
    if !seeds.iter().any(valid_seed) {
        return (
            MirrorVisibilityResult {
                visible_cells: VisibleCells::Culled(std::mem::take(scratch)),
                reachable_cells: Vec::new(),
                path: MirrorVisibilityPath::EmptySeeds,
                considered: 0,
                accepted: 0,
                cpu,
            },
            frustum,
        );
    }
    let walk = portal_vis::portal_traverse_seeded(
        mirror_eye,
        seeds,
        &frustum,
        world,
        blocked_portals,
        step_limit,
        timing,
    );
    if let Some(nanos) = walk.stats.walk_nanos {
        cpu.add_nanos(VisibilityStage::MirrorWalk, nanos);
    }
    let path = if !world.has_portals {
        MirrorVisibilityPath::NoPortalsFallback
    } else if walk.stats.step_limit_hit {
        MirrorVisibilityPath::StepLimitFallback
    } else {
        MirrorVisibilityPath::Seeded
    };
    let fallback = path != MirrorVisibilityPath::Seeded;
    let mut reachable_cells = Vec::new();
    for (id, cell) in world.cells.iter().enumerate() {
        if cell.is_solid
            || cell.is_exterior
            || (!fallback && !walk.visible[id])
            || is_aabb_outside_frustum(cell.bounds_min, cell.bounds_max, &frustum)
        {
            continue;
        }
        reachable_cells.push(id as u32);
        if cell.is_drawable {
            scratch.push(id as u32);
        }
    }
    (
        MirrorVisibilityResult {
            visible_cells: VisibleCells::Culled(std::mem::take(scratch)),
            reachable_cells,
            path,
            considered: walk.stats.considered,
            accepted: walk.stats.accepted,
            cpu,
        },
        frustum,
    )
}

#[cfg(test)]
#[path = "mirror_visibility_tests.rs"]
mod tests;
