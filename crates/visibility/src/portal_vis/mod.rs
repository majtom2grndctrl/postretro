// Runtime portal traversal entry: camera walk, traversal stats and the step budget.
// See: context/lib/rendering_pipeline.md §2

mod chain_walk;
mod clip;
#[cfg(test)]
mod tests;

use std::fmt::Write as _;

use glam::Vec3;

use crate::visibility::Frustum;
use postretro_level_loader::LevelWorld;
use postretro_stage_timing::TimingGate;

use chain_walk::{DfsState, flood};
pub use clip::narrow_frustum;
#[cfg(test)]
pub(crate) use clip::{CLIP_EPSILON, clip_polygon_to_frustum, clip_polygon_to_planes};

// Dense detail-brush partitions can create highly cyclic portal graphs where
// the number of distinct portal chains is far larger than the number of cells.
// Keep the per-frame CPU walk bounded; the visibility layer falls back to
// per-cell AABB frustum culling when this trips.
pub(crate) const MAX_PORTAL_WALK_STEPS: u32 = 20_000;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PortalTraversalStats {
    pub considered: u32,
    pub accepted: u32,
    pub rejected_blocked_portal: u32,
    pub rejected_solid: u32,
    pub rejected_clipped: u32,
    pub rejected_narrow: u32,
    pub rejected_invalid: u32,
    pub rejected_path_cycle: u32,
    pub rejected_depth_limit: u32,
    pub step_limit_hit: bool,
    /// Time inside `flood`, when CPU timing is on and the flood ran.
    pub walk_nanos: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct PortalTraversalResult {
    pub visible: Vec<bool>,
    pub stats: PortalTraversalStats,
}

/// Cycle prevention keys on *portals crossed in the current chain*, not on
/// cells reached globally — keying on cells would silently drop every chain
/// after the first to arrive at a cell, losing whichever carried the widest
/// sub-frustum. The visible set is the union across all chains.
///
/// By induction, every narrowed frustum is a strict subset of the camera
/// frustum, so a per-cell AABB cull is redundant and omitted.
///
/// `capture: true` emits per-portal events to the `postretro::portal_trace`
/// target as a single batched log message. Triggered by `Alt+Shift+1`; see
/// `context/lib/input.md` §7.
pub fn portal_traverse(
    camera_position: Vec3,
    camera_cell: usize,
    frustum: &Frustum,
    world: &LevelWorld,
    capture: bool,
) -> Vec<bool> {
    portal_traverse_detailed(camera_position, camera_cell, frustum, world, &[], capture).visible
}

pub(crate) fn portal_traverse_detailed(
    camera_position: Vec3,
    camera_cell: usize,
    frustum: &Frustum,
    world: &LevelWorld,
    blocked_portals: &[bool],
    capture: bool,
) -> PortalTraversalResult {
    portal_traverse_with_step_limit(
        camera_position,
        camera_cell,
        frustum,
        world,
        blocked_portals,
        capture,
        MAX_PORTAL_WALK_STEPS,
        TimingGate::OFF,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn portal_traverse_with_step_limit(
    camera_position: Vec3,
    camera_cell: usize,
    frustum: &Frustum,
    world: &LevelWorld,
    blocked_portals: &[bool],
    capture: bool,
    step_limit: u32,
    timing: TimingGate,
) -> PortalTraversalResult {
    let (visible, trace) = portal_traverse_inner(
        camera_position,
        camera_cell,
        frustum,
        world,
        blocked_portals,
        capture,
        step_limit,
        timing,
    );
    // One `log::info!` call: one timestamp/target prefix per traced frame
    // instead of one per event.
    if let Some(buf) = trace {
        log::info!(target: "postretro::portal_trace", "[portal_trace]\n{}", buf);
    }
    visible
}

// Split from `portal_traverse` so tests can inspect the formatted trace string
// directly without wiring a test logger.
#[allow(clippy::too_many_arguments)]
fn portal_traverse_inner(
    camera_position: Vec3,
    camera_cell: usize,
    frustum: &Frustum,
    world: &LevelWorld,
    blocked_portals: &[bool],
    capture: bool,
    step_limit: u32,
    timing: TimingGate,
) -> (PortalTraversalResult, Option<String>) {
    let cell_count = world.cell_count();
    let visible = vec![false; cell_count];
    let stats = PortalTraversalStats::default();

    let mut trace = if capture {
        Some(String::with_capacity(512))
    } else {
        None
    };

    // Out-of-range camera cell: emit a single `cell_oor` line into the buffer
    // and bail before any header write that reads the cell.
    if camera_cell >= cell_count {
        if let Some(buf) = trace.as_mut() {
            let _ = writeln!(
                buf,
                "abort cell_oor cam=({:.2},{:.2},{:.2}) cell={} cells={}",
                camera_position.x, camera_position.y, camera_position.z, camera_cell, cell_count,
            );
        }
        return (PortalTraversalResult { visible, stats }, trace);
    }

    // `solid` is omitted from the header — solid cells short-circuit in
    // `determine_visible_cells` before reaching `portal_traverse`.
    if let Some(buf) = trace.as_mut() {
        let (bounds_min, bounds_max) = world
            .cell_bounds(camera_cell)
            .unwrap_or((Vec3::ZERO, Vec3::ZERO));
        let _ = writeln!(
            buf,
            "cam=({:.2},{:.2},{:.2}) cell={} faces={} bnds=({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2}) cells={}",
            camera_position.x,
            camera_position.y,
            camera_position.z,
            camera_cell,
            world.cell_face_count(camera_cell),
            bounds_min.x,
            bounds_min.y,
            bounds_min.z,
            bounds_max.x,
            bounds_max.y,
            bounds_max.z,
            cell_count,
        );
    }

    let mut state = DfsState {
        world,
        blocked_portals,
        camera_position,
        trace,
        visible,
        cell_count,
        stats,
        step_limit,
        depth_limit_warned: false,
        camera_cell,
    };

    // The render pipeline uses a 0.1-unit near clip for depth-buffer
    // precision, but visibility has no such need. Slide the near plane up
    // to the camera apex so portals the player is pressed against aren't
    // clipped to empty at the Near step. See
    // `Frustum::slide_near_plane_to` for the full rationale and
    // `portal_traverse_reaches_neighbor_when_camera_is_close_to_portal_wall`
    // for the regression probe. Only applied to the top-level camera
    // frustum — narrowed sub-frustums produced by `narrow_frustum` already
    // build all edge planes through the camera apex, so the relaxation is
    // redundant (and would be incorrect) inside the DFS.
    let mut visibility_frustum = frustum.clone();
    visibility_frustum.slide_near_plane_to(camera_position);

    let mut path: Vec<usize> = Vec::new();
    let mut clip_scratch_a: Vec<Vec3> = Vec::new();
    let mut clip_scratch_b: Vec<Vec3> = Vec::new();
    let walk_start = timing.is_enabled().then(std::time::Instant::now);
    flood(
        &mut state,
        camera_cell,
        &visibility_frustum,
        &mut path,
        &mut clip_scratch_a,
        &mut clip_scratch_b,
    );
    state.stats.walk_nanos =
        walk_start.map(|start| u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX));

    // Summary: reach count + the considered/accepted totals, plus a compact
    // rej[...] bracket that elides zero counters. An all-clean frame still
    // prints `rej[]` so the shape of every summary is visually identical.
    if let Some(buf) = state.trace.as_mut() {
        let reach_count = state.visible.iter().filter(|&&v| v).count();
        let _ = write!(
            buf,
            "  = reach={} cons={} acc={} rej[",
            reach_count, state.stats.considered, state.stats.accepted,
        );
        let mut first = true;
        let mut emit = |buf: &mut String, name: &str, count: u32| {
            if count == 0 {
                return;
            }
            if !first {
                buf.push(' ');
            }
            let _ = write!(buf, "{}={}", name, count);
            first = false;
        };
        // Same order as the event-site reason codes: blocked, clip, narrow,
        // solid, cycle, depth, invalid.
        emit(buf, "blocked", state.stats.rejected_blocked_portal);
        emit(buf, "clip", state.stats.rejected_clipped);
        emit(buf, "narrow", state.stats.rejected_narrow);
        emit(buf, "solid", state.stats.rejected_solid);
        emit(buf, "cycle", state.stats.rejected_path_cycle);
        emit(buf, "depth", state.stats.rejected_depth_limit);
        emit(buf, "limit", u32::from(state.stats.step_limit_hit));
        emit(buf, "invalid", state.stats.rejected_invalid);
        let _ = writeln!(buf, "]");
    }

    (
        PortalTraversalResult {
            visible: state.visible,
            stats: state.stats,
        },
        state.trace,
    )
}
