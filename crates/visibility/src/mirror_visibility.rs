// Planar water mirror visibility: the seeded walk's draw and reach sets.
// See: context/lib/rendering_pipeline.md §2, §7.3a

#[cfg(test)]
mod tests;

use std::time::Instant;

use glam::{Mat4, Vec3};
use postretro_level_loader::LevelWorld;
use postretro_stage_timing::StageFrame;

use crate::cpu_stages::VisibilityStage;
use crate::region_walk::{
    MAX_PORTAL_WALK_STEPS, PortalWalkScratch, SeededWalk, seed_is_usable, seeded_view_planes,
    walk_seeded,
};
use crate::visibility::{FrustumPlane, VisibleCells, is_aabb_outside_planes};

/// The mirror view the seeded walk and the mirror cull take, as given.
#[derive(Debug, Clone, Copy)]
pub struct MirrorView {
    /// The reflected eye. It may lie in a solid cell: nothing locates it.
    pub eye: Vec3,
    /// The mirror's cull view-projection. With D set, its far plane is D.
    pub view_proj: Mat4,
    /// The water plane, unit normal facing the reflected content. It
    /// replaces the view-projection's near plane.
    pub near_plane: FrustumPlane,
}

/// How a frame's mirror visibility was formed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MirrorVisibilityPath {
    /// The walk drained under the shared cap.
    Walked,
    /// The walk hit the shared cap after `considered` portal tests. Both sets
    /// are the camera-bounded trip set.
    Tripped { considered: u32 },
    /// No seed was usable (out of range, solid, exterior, or wholly outside
    /// the mirror frustum). Both sets are empty.
    #[default]
    NoSeeds,
    /// The world has no portal data. Both sets are the camera-bounded trip
    /// set; no walk runs.
    NoPortals,
}

/// One frame's mirror visibility, in caller-owned buffers the entry clears
/// and refills, so a steady state allocates nothing.
#[derive(Debug, Default)]
pub struct MirrorVisibility {
    /// Drawable cells the mirror draws, ascending.
    pub visible_cells: Vec<u32>,
    /// Non-solid cells the mirror reaches, ascending, empty cells included:
    /// what entity collection, lighting and fog read. Holds `visible_cells`.
    pub reach_cells: Vec<u32>,
    pub path: MirrorVisibilityPath,
    /// Portal tests and acceptances of this frame's walk; zero when none ran.
    pub considered: u32,
    pub accepted: u32,
}

/// Mirror visibility for one frame (`rendering_pipeline.md` §2, §7.3a): walk
/// from `seeds` through the seeded entry under the shared step cap, and
/// filter the result into `out`.
///
/// A completed walk draws its drawable reached cells that pass the mirror
/// frustum and reaches every cell it reached. A trip, or a world with no
/// portals, draws `camera_visible`'s cells that pass the mirror frustum, all
/// of which the main view already draws, and never widens to every cell in
/// the frustum. `DrawAll` reads as every drawable cell.
///
/// `seeds` come from the caller (cells in the camera's reach that straddle
/// the water); unusable ones are skipped, and output never depends on their
/// order. `scratch` is the mirror's own, never the camera's. MirrorWalk time
/// goes into `cpu` on every frame a usable seed walks.
#[allow(clippy::too_many_arguments)]
pub fn determine_mirror_visible_cells(
    view: &MirrorView,
    seeds: &[usize],
    camera_visible: &VisibleCells,
    world: &LevelWorld,
    blocked_portals: &[bool],
    scratch: &mut PortalWalkScratch,
    cpu: &StageFrame<VisibilityStage>,
    out: &mut MirrorVisibility,
) {
    determine_with_step_limit(
        view,
        seeds,
        camera_visible,
        world,
        blocked_portals,
        MAX_PORTAL_WALK_STEPS,
        scratch,
        cpu,
        out,
    );
}

/// [`determine_mirror_visible_cells`] under `step_cap` in place of the shared
/// cap, so another crate's tests can force a trip. Never in a normal build.
#[cfg(any(test, feature = "test-seams"))]
#[allow(clippy::too_many_arguments)]
pub fn determine_mirror_visible_cells_capped(
    view: &MirrorView,
    seeds: &[usize],
    camera_visible: &VisibleCells,
    world: &LevelWorld,
    blocked_portals: &[bool],
    step_cap: u32,
    scratch: &mut PortalWalkScratch,
    cpu: &StageFrame<VisibilityStage>,
    out: &mut MirrorVisibility,
) {
    determine_with_step_limit(
        view,
        seeds,
        camera_visible,
        world,
        blocked_portals,
        step_cap,
        scratch,
        cpu,
        out,
    );
}

#[allow(clippy::too_many_arguments)]
fn determine_with_step_limit(
    view: &MirrorView,
    seeds: &[usize],
    camera_visible: &VisibleCells,
    world: &LevelWorld,
    blocked_portals: &[bool],
    step_limit: u32,
    scratch: &mut PortalWalkScratch,
    cpu: &StageFrame<VisibilityStage>,
    out: &mut MirrorVisibility,
) {
    out.visible_cells.clear();
    out.reach_cells.clear();
    out.considered = 0;
    out.accepted = 0;
    let planes = seeded_view_planes(view.view_proj, view.near_plane);

    if !world.has_portals {
        out.path = if seeds
            .iter()
            .any(|&seed| seed_is_usable(world, &planes, seed))
        {
            camera_visible_in_mirror(camera_visible, world, &planes, out);
            MirrorVisibilityPath::NoPortals
        } else {
            MirrorVisibilityPath::NoSeeds
        };
        return;
    }

    // Build the portal planes once, outside the walk's timer, as the camera
    // walk does.
    world.portal_planes();
    let walk_start = cpu.gate().is_enabled().then(Instant::now);
    let stats = walk_seeded(
        view.eye,
        view.view_proj,
        view.near_plane,
        seeds,
        world,
        blocked_portals,
        step_limit,
        scratch,
    );
    let walk_nanos =
        walk_start.map(|start| u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX));
    out.considered = stats.considered;
    out.accepted = stats.accepted;

    out.path = match SeededWalk::from_stats(&stats, scratch) {
        // A usable seed is always reached, so an empty reach means none was.
        SeededWalk::Completed { reached: [] } => MirrorVisibilityPath::NoSeeds,
        SeededWalk::Completed { reached } => {
            // Every reached cell is non-solid: the walk skips solid seeds and
            // rejects solid neighbours.
            out.reach_cells.extend_from_slice(reached);
            out.reach_cells.sort_unstable();
            out.visible_cells.extend(
                out.reach_cells
                    .iter()
                    .copied()
                    .filter(|&cell| drawable_in_mirror(world, &planes, cell as usize)),
            );
            MirrorVisibilityPath::Walked
        }
        SeededWalk::Tripped { considered } => {
            log::debug!(
                "[Visibility] path=MirrorStepLimit considered={considered}; drawing the \
                 camera's visible cells inside the mirror frustum this frame",
            );
            camera_visible_in_mirror(camera_visible, world, &planes, out);
            MirrorVisibilityPath::Tripped { considered }
        }
    };
    if let Some(nanos) = walk_nanos
        && out.path != MirrorVisibilityPath::NoSeeds
    {
        cpu.add_nanos(VisibilityStage::MirrorWalk, nanos);
    }
}

fn drawable_in_mirror(world: &LevelWorld, planes: &[FrustumPlane], cell: usize) -> bool {
    world.cells.get(cell).is_some_and(|data| {
        data.is_drawable && !is_aabb_outside_planes(data.bounds_min, data.bounds_max, planes)
    })
}

/// The trip set: the camera's visible cells that pass the mirror frustum.
/// Drawing it reads nothing the main view does not, so it is also the reach.
fn camera_visible_in_mirror(
    camera_visible: &VisibleCells,
    world: &LevelWorld,
    planes: &[FrustumPlane],
    out: &mut MirrorVisibility,
) {
    match camera_visible {
        VisibleCells::Culled(cells) => out.visible_cells.extend(
            cells
                .iter()
                .copied()
                .filter(|&cell| drawable_in_mirror(world, planes, cell as usize)),
        ),
        VisibleCells::DrawAll => out.visible_cells.extend(
            (0..world.cell_count())
                .filter(|&cell| drawable_in_mirror(world, planes, cell))
                .map(|cell| cell as u32),
        ),
    }
    out.reach_cells.extend_from_slice(&out.visible_cells);
}
