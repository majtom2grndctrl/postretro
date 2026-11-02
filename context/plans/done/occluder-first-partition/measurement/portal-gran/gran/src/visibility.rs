// Per-frame visibility determination: portal walk and frustum-culled fallbacks.
// See: context/lib/rendering_pipeline.md §2

mod fallback;
mod frustum;

use glam::{Mat4, Vec3};

use crate::cpu_stages::{self, VisibilityStage};
use crate::region_walk::{
    self, MAX_PORTAL_WALK_STEPS, PortalWalkScratch, WalkTrace, WalkView, WalkWorld,
};
use fallback::{fog_reachable_frustum_fallback, visible_cells_frustum_all};
use postretro_level_loader::LevelWorld;
use postretro_stage_timing::{StageFrame, TimingGate};

pub(crate) use frustum::is_aabb_outside_frustum;
pub(crate) use frustum::{
    FAR_PLANE_INDEX, NEAR_PLANE_INDEX, frustum_plane_array, is_aabb_outside_planes, slide_plane_to,
};
pub use frustum::{Frustum, FrustumPlane, extract_frustum_planes};

/// Result of per-frame visibility determination for the GPU-driven indirect
/// draw path. The portal walk determines the visible cell set; the BVH
/// traversal compute shader consumes it directly via the visible-cell bitmask.
#[derive(Debug)]
pub enum VisibleCells {
    /// Specific cells are visible, ascending; pass to the compute cull
    /// shader. Hand the `Vec` back with `PortalWalkScratch::recycle_cells`
    /// once consumed, so the next frame reuses its allocation.
    Culled(Vec<u32>),
    /// All cells are visible because there are no cells to cull.
    DrawAll,
}

/// Borrowed renderer input describing this frame's camera visibility, threaded
/// into `render_frame_indirect`/`record_pre_scene_compute` so the renderer can
/// choose its camera-cull strategy without reaching into game state. Carries
/// only the camera visible-cell set and the path provenance that produced it;
/// candidate-cull eligibility is derived solely from this struct plus the
/// loaded `CellDrawIndex`.
///
/// `path` is `Copy`; `cells` is borrowed for the duration of the render call.
#[derive(Debug, Clone, Copy)]
pub struct CameraCullVisibility<'a> {
    pub cells: &'a VisibleCells,
    pub path: VisibilityPath,
}

/// Per-frame visibility pipeline statistics for diagnostics.
#[derive(Debug, Clone)]
pub struct VisibilityStats {
    /// Runtime cell the camera currently occupies.
    pub camera_cell: u32,
    /// Total faces in the level.
    pub total_faces: u32,
    /// Faces submitted to the renderer this frame, after every narrowing
    /// and culling stage the path applied. Same meaning on all paths.
    pub drawn_faces: u32,
    /// Which visibility determination path produced these stats. Path-
    /// specific diagnostics (e.g., portal walk reach) live on the variant.
    pub path: VisibilityPath,
    /// CPU stage values for this frame: walk time and traversal counters on a
    /// walk frame, a fallback marker otherwise. Empty when timing is off.
    pub cpu: StageFrame<VisibilityStage>,
}

/// Identifies the code path that produced a given `VisibilityStats`, and
/// carries any metrics that are only meaningful on that path.
///
/// Readers that only care about the cross-path totals can ignore this
/// field; readers that want to inspect portal-specific diagnostics,
/// can `match` on it.
#[derive(Debug, Clone, Copy)]
pub enum VisibilityPath {
    /// Primary PRL rendering path using the per-frame bounded-region portal
    /// walk. Every region the walk clips to lies inside the camera frustum
    /// with its near plane slid to the eye and its side planes pushed out by
    /// the walk's clip pad (half a snap step), so no separate AABB cull runs
    /// on this path; a cell lying wholly in that sliver beside a screen edge
    /// draws nothing on screen. One exception: a portal within the bypass
    /// distance of the eye hands its neighbour the source cell's rect
    /// unclipped, so that neighbour can lie behind or beside the eye. That
    /// is why the step-limit fallback's fog set also keeps every cell within
    /// the bypass distance. The set is a conservative
    /// superset of the cells a point camera can see through the portals
    /// (`rendering_pipeline.md` §2); never read a cell in it as proof of line
    /// of sight.
    /// `walk_reach` counts only drawable cells (`face_count > 0`); the
    /// wider `fog_reachable` set (which includes empty cells) is tracked
    /// separately on `VisibilityResult` and is not reflected here.
    PrlPortal { walk_reach: u32 },
    /// Fallback: world has no cells to cull against. DrawAll with every
    /// face in the level submitted.
    EmptyWorldFallback,
    /// Fallback: camera position lies inside solid geometry (clipped
    /// into a wall). All drawable cells are drawn,
    /// subject to AABB frustum culling.
    SolidCellFallback,
    /// Fallback: camera is in an exterior cell. The
    /// camera has left the playable interior — spectator, noclip, debug
    /// fly. All drawable cells are drawn, subject to
    /// AABB frustum culling.
    ExteriorCellFallback,
    /// Fallback: portal data missing from the level file. All drawable cells
    /// are submitted, subject to AABB frustum culling.
    NoPortalsFallback,
    /// Fallback: the portal walk hit its per-frame step cap. Drawable cells
    /// are submitted through bounded AABB frustum culling against the camera
    /// frustum; `fog_reachable` uses a wider predicate (see
    /// `fog_reachable_frustum_fallback`) that holds every cell the walk could
    /// reach. The drawing set is a superset of the walk's too, except for
    /// cells lying entirely in the near slab, beside the eye, or in the clip
    /// pad's sliver just outside a screen edge, which the walk may keep but
    /// the plain camera cull drops — harmless, since the camera frustum
    /// clips their geometry regardless. Unlike the other
    /// fallbacks, this path never hands consumers the empty "no portal
    /// isolation" sentinel.
    PortalStepLimitFallback { considered: u32, accepted: u32 },
}

impl VisibilityStats {
    /// On the PRL portal-traversal path, the count of cells the portal
    /// walk can reach from the camera cell. `None` on every other path.
    pub fn walk_reach(&self) -> Option<u32> {
        match self.path {
            VisibilityPath::PrlPortal { walk_reach } => Some(walk_reach),
            _ => None,
        }
    }
}

/// Per-frame visibility output. Carries two cell sets:
///
/// - `visible_cells`: drawable set — faces submitted to the renderer
///   (portal-reachable drawable cells).
/// - `fog_reachable`: wider set used for portal-isolated effects (fog
///   culling, dynamic-light cell gating). Same portal-reachability and
///   `!is_solid` predicates as `visible_cells`, but without the
///   `face_count > 0` filter — empty cells still bound light/fog
///   influence even though they have no geometry to draw. Do not add the
///   `face_count > 0` predicate to this set; empty cells must remain
///   eligible for fog and dynamic-light gating (that predicate's removal
///   is the point of this field). Populated on the portal path and, as a
///   frustum-culled superset, on the portal step-limit fallback; empty on
///   every other fallback (where the downstream consumer treats empty as
///   "no portal isolation").
///
/// Both sets are ascending and, on the portal path, conservative supersets
/// per `rendering_pipeline.md` §2.
#[derive(Debug)]
pub struct VisibilityResult {
    pub visible_cells: VisibleCells,
    pub fog_reachable: Vec<u32>,
    pub stats: VisibilityStats,
}

// --- Shared cell-level visibility determination ---

/// Internal classification of which visibility path was selected.
#[derive(Debug, Clone, Copy)]
enum CellVisPath {
    EmptyWorld,
    SolidCell,
    ExteriorCell,
    Portal,
    PortalStepLimit { considered: u32, accepted: u32 },
    NoPortals,
}

/// Internal result of shared cell-level visibility determination. The draw
/// set itself goes into the caller's buffer.
struct CellVisResult {
    /// Wider, fog/light-reachable cell set. Same predicates as the draw set
    /// minus the `face_count > 0` filter. Populated on the portal path and
    /// the portal step-limit fallback; empty on every other variant.
    fog_reachable: Vec<u32>,
    camera_cell: u32,
    total_faces: u32,
    path: CellVisPath,
    /// The camera frustum extracted for this frame.
    frustum: Frustum,
}

impl CellVisResult {
    fn new(camera_cell: usize, total_faces: u32, path: CellVisPath, frustum: Frustum) -> Self {
        Self {
            fog_reachable: Vec::new(),
            camera_cell: camera_cell as u32,
            total_faces,
            path,
            frustum,
        }
    }
}

/// Shared cell-level visibility determination: picks the path and writes the
/// ascending draw set into `cells`. `portal_step_limit` is the walk's
/// per-frame cap; tests inject a small one to force the step-limit fallback.
#[allow(clippy::too_many_arguments)]
fn determine_visible_cell_set(
    camera_position: Vec3,
    view_proj: Mat4,
    world: &LevelWorld,
    blocked_portals: &[bool],
    capture_portal_walk: bool,
    portal_step_limit: u32,
    scratch: &mut PortalWalkScratch,
    cells: &mut Vec<u32>,
    cpu: &StageFrame<VisibilityStage>,
) -> CellVisResult {
    let total_faces = world.total_face_count();
    let frustum = extract_frustum_planes(view_proj);
    cells.clear();

    if world.cells.is_empty() {
        return CellVisResult::new(0, total_faces, CellVisPath::EmptyWorld, frustum);
    }

    let camera_cell = world.locate_cell(camera_position);
    let located = world.cells.get(camera_cell);

    if located.is_some_and(|cell| cell.is_solid) {
        log::warn!(
            "[Visibility] path=SolidCellFallback camera in solid cell {}",
            camera_cell,
        );
        visible_cells_frustum_all(&world.cells, &frustum, cells);
        return CellVisResult::new(camera_cell, total_faces, CellVisPath::SolidCell, frustum);
    }

    // Exterior camera fallback: camera is in a compiler-marked exterior cell.
    // Frustum-cull every drawable cell.
    if located.is_some_and(|cell| cell.is_exterior) {
        visible_cells_frustum_all(&world.cells, &frustum, cells);
        return CellVisResult::new(camera_cell, total_faces, CellVisPath::ExteriorCell, frustum);
    }

    if !world.has_portals {
        visible_cells_frustum_all(&world.cells, &frustum, cells);
        return CellVisResult::new(camera_cell, total_faces, CellVisPath::NoPortals, frustum);
    }

    // A locator answer past the cell list has nothing to walk from: empty
    // sets on the portal path, and no walk recorded.
    if located.is_none() {
        if capture_portal_walk {
            region_walk::trace_out_of_range(camera_position, camera_cell, world.cell_count());
        }
        return CellVisResult::new(camera_cell, total_faces, CellVisPath::Portal, frustum);
    }

    let view = WalkView::camera(camera_position, view_proj);
    let mut trace =
        capture_portal_walk.then(|| WalkTrace::begin(camera_position, camera_cell, world));
    let input = WalkWorld {
        world,
        blocked_portals,
    };
    // Build the portal planes once, outside the walk's timer, so the first
    // frame's walk time does not carry the one-time build.
    world.portal_planes();
    let walk_start = cpu.gate().is_enabled().then(std::time::Instant::now);
    let stats = region_walk::walk_from_camera_traced(
        input,
        &view,
        camera_cell,
        portal_step_limit,
        scratch,
        trace.as_mut(),
    );
    // A step-limit trip is still a walk frame: its walk is the cost the cap
    // exists to bound.
    if let Some(start) = walk_start {
        let walk_nanos = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        cpu_stages::record_walk(cpu, &stats, walk_nanos);
    }
    if let Some(trace) = trace {
        trace.emit(&stats, scratch.reached().len());
    }

    if stats.step_limit_hit {
        log::debug!(
            "[Visibility] path=PortalStepLimitFallback cell={}, considered={}, accepted={}; \
             using per-cell AABB culling for this frame",
            camera_cell,
            stats.considered,
            stats.accepted,
        );
        // Stay bounded: fog, shadow-light eligibility, and the SH compose
        // gate all read `fog_reachable`, and an empty set would make each
        // of them fall back to "every cell" on exactly the frames the walk
        // was already too expensive. The partial walk is discarded.
        visible_cells_frustum_all(&world.cells, &frustum, cells);
        // Fog reach tests against the walk's own outermost clip region,
        // built in the scratch's plane buffer.
        let clip_planes = region_walk::screen_clip_planes(&view, scratch);
        let fog_reachable =
            fog_reachable_frustum_fallback(&world.cells, clip_planes, camera_position, camera_cell);
        let path = CellVisPath::PortalStepLimit {
            considered: stats.considered,
            accepted: stats.accepted,
        };
        return CellVisResult {
            fog_reachable,
            ..CellVisResult::new(camera_cell, total_faces, path, frustum)
        };
    }

    // Both sets come from the walk's final state (every reached cell is
    // non-solid; solid neighbours are rejected), so the draw set is exactly
    // the drawable part of fog reach.
    let mut fog_reachable = scratch.reached().to_vec();
    fog_reachable.sort_unstable();
    cells.extend(
        fog_reachable
            .iter()
            .copied()
            .filter(|&cell| world.cells[cell as usize].is_drawable),
    );
    CellVisResult {
        fog_reachable,
        ..CellVisResult::new(camera_cell, total_faces, CellVisPath::Portal, frustum)
    }
}

/// Count faces across the given visible cells, and convert the internal path
/// tag to the public `VisibilityPath`.
fn build_visibility_stats(
    result: &CellVisResult,
    visible_cells: &[u32],
    world: &LevelWorld,
    cpu: StageFrame<VisibilityStage>,
) -> VisibilityStats {
    let drawn_faces = visible_cells
        .iter()
        .map(|&cell| world.cells[cell as usize].face_count)
        .sum();

    let path = match result.path {
        CellVisPath::SolidCell => VisibilityPath::SolidCellFallback,
        CellVisPath::ExteriorCell => VisibilityPath::ExteriorCellFallback,
        CellVisPath::Portal => VisibilityPath::PrlPortal {
            walk_reach: visible_cells.len() as u32,
        },
        CellVisPath::PortalStepLimit {
            considered,
            accepted,
        } => VisibilityPath::PortalStepLimitFallback {
            considered,
            accepted,
        },
        CellVisPath::NoPortals => VisibilityPath::NoPortalsFallback,
        CellVisPath::EmptyWorld => VisibilityPath::EmptyWorldFallback,
    };

    match result.path {
        CellVisPath::Portal => {
            log::trace!(
                "[Visibility] path=PrlPortal cell={}, walk_reach={}, drawn_faces={}, total_faces={}",
                result.camera_cell,
                visible_cells.len(),
                drawn_faces,
                result.total_faces,
            );
        }
        CellVisPath::PortalStepLimit {
            considered,
            accepted,
        } => {
            log::trace!(
                "[Visibility] path=PortalStepLimitFallback cell={}, considered={}, accepted={}, drawn_faces={}, total_faces={}",
                result.camera_cell,
                considered,
                accepted,
                drawn_faces,
                result.total_faces,
            );
        }
        _ => {}
    }

    VisibilityStats {
        camera_cell: result.camera_cell,
        total_faces: result.total_faces,
        drawn_faces,
        path,
        cpu,
    }
}

// --- Public visibility API ---

/// GPU-driven visibility path: run the portal walk with a frustum-cull
/// fallback and produce the set of visible cell IDs consumed by the BVH
/// traversal compute shader (via the visible-cell bitmask).
///
/// Pipeline: cell locator to find the camera cell, then the bounded-region
/// portal walk from it (`rendering_pipeline.md` §2). Fallbacks (solid cell,
/// exterior camera, empty world, no portal data, step-cap trip) frustum-cull
/// cell AABBs instead and feed the same downstream bitmask path.
///
/// `scratch` is the caller's walk state, reused across frames so a walk
/// allocates nothing once it has grown to the world. It also lends the
/// `VisibleCells::Culled` buffer; return it with
/// [`PortalWalkScratch::recycle_cells`] after the frame consumes it. One
/// scratch per thread.
pub fn determine_visible_cells(
    camera_position: Vec3,
    view_proj: Mat4,
    world: &LevelWorld,
    blocked_portals: &[bool],
    capture_portal_walk: bool,
    scratch: &mut PortalWalkScratch,
    timing: TimingGate,
) -> (VisibilityResult, Frustum) {
    determine_visible_cells_with_step_limit(
        camera_position,
        view_proj,
        world,
        blocked_portals,
        capture_portal_walk,
        scratch,
        MAX_PORTAL_WALK_STEPS,
        timing,
    )
}

#[allow(clippy::too_many_arguments)]
fn determine_visible_cells_with_step_limit(
    camera_position: Vec3,
    view_proj: Mat4,
    world: &LevelWorld,
    blocked_portals: &[bool],
    capture_portal_walk: bool,
    scratch: &mut PortalWalkScratch,
    portal_step_limit: u32,
    timing: TimingGate,
) -> (VisibilityResult, Frustum) {
    let cpu = StageFrame::new(timing);
    let mut cells = scratch.take_cells();
    let result = determine_visible_cell_set(
        camera_position,
        view_proj,
        world,
        blocked_portals,
        capture_portal_walk,
        portal_step_limit,
        scratch,
        &mut cells,
        &cpu,
    );
    if !matches!(
        result.path,
        CellVisPath::Portal | CellVisPath::PortalStepLimit { .. }
    ) {
        cpu.mark(VisibilityStage::PortalFallback);
    }

    if matches!(result.path, CellVisPath::EmptyWorld) {
        scratch.recycle_cells(cells);
        let stats = VisibilityStats {
            camera_cell: result.camera_cell,
            total_faces: result.total_faces,
            drawn_faces: result.total_faces,
            path: VisibilityPath::EmptyWorldFallback,
            cpu,
        };
        return (
            VisibilityResult {
                visible_cells: VisibleCells::DrawAll,
                fog_reachable: result.fog_reachable,
                stats,
            },
            result.frustum,
        );
    }

    let stats = build_visibility_stats(&result, &cells, world, cpu);
    (
        VisibilityResult {
            visible_cells: VisibleCells::Culled(cells),
            fog_reachable: result.fog_reachable,
            stats,
        },
        result.frustum,
    )
}
