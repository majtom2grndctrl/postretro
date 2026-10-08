// Per-frame visibility determination: portal traversal and frustum-culled fallbacks.
// See: context/lib/rendering_pipeline.md

use glam::{Mat4, Vec3, Vec4};

use crate::cpu_stages::{self, VisibilityStage};
use crate::portal_vis;
use postretro_level_loader::{CellData, LevelWorld};
use postretro_stage_timing::{StageFrame, TimingGate};

/// Result of per-frame visibility determination for the GPU-driven indirect
/// draw path. Portal DFS still determines the visible cell set; the BVH
/// traversal compute shader consumes it directly via the visible-cell bitmask.
#[derive(Debug)]
pub enum VisibleCells {
    /// Specific cells are visible; pass to the compute cull shader.
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
    /// Primary PRL rendering path using per-frame portal traversal.
    /// Portal traversal narrows the frustum at every hop, so no separate
    /// AABB cull runs on this path.
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
    /// Fallback: portal traversal hit its per-frame CPU step budget. Drawable
    /// cells are submitted through bounded AABB frustum culling against the
    /// camera frustum; `fog_reachable` uses a wider predicate against a
    /// near-plane-slid copy of that frustum (see
    /// `fog_reachable_frustum_fallback`). Fog reach is a true superset of the
    /// exact portal walk's reachable set; the drawing set is a superset too,
    /// except for geometry lying entirely in the near slab, which the exact
    /// walk keeps but the unslid drawing cull drops — harmless, since such
    /// geometry is near-clipped regardless. Unlike the other fallbacks, this
    /// path never hands consumers the empty "no portal isolation" sentinel.
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
#[derive(Debug)]
pub struct VisibilityResult {
    pub visible_cells: VisibleCells,
    pub fog_reachable: Vec<u32>,
    pub stats: VisibilityStats,
}

// --- Frustum culling ---

/// A plane in Hessian normal form: dot(normal, point) + dist >= 0 for points on the inside.
#[derive(Debug, Clone, Copy)]
pub struct FrustumPlane {
    pub normal: Vec3,
    pub dist: f32,
}

/// The planes of a view frustum, extracted from a view-projection matrix.
///
/// The initial camera frustum always contains exactly 6 planes in
/// left/right/bottom/top/near/far order. After portal traversal narrows the
/// frustum, it may contain more planes (one per portal edge plus near/far).
#[derive(Debug, Clone)]
pub struct Frustum {
    pub planes: Vec<FrustumPlane>,
}

/// Canonical plane indices for a 6-plane frustum produced by
/// [`extract_frustum_planes`]. Kept next to the extraction so any future
/// reordering has exactly one place to update.
pub(crate) const NEAR_PLANE_INDEX: usize = 4;

impl Frustum {
    /// Slide the near plane so it passes exactly through `apex`, keeping
    /// the inward normal unchanged. Intended for the initial camera frustum
    /// before any portal narrowing.
    ///
    /// Fixes the tight-corridor blank-frame bug: the render pipeline's
    /// 0.1-unit near clip is depth-precision only, and when the camera sits
    /// closer than that to a portal plane, every portal vertex lies between
    /// camera and near plane — Sutherland-Hodgman clips the polygon to
    /// empty, the neighbor is rejected, and the frame flashes the clear
    /// color. See the regression probe in `portal_vis::tests`.
    ///
    /// Assumes the canonical 6-plane layout from [`extract_frustum_planes`]
    /// (Left, Right, Bottom, Top, Near, Far). **Do not call on a narrowed
    /// sub-frustum**: those replace the near plane with the portal plane,
    /// and sliding it to the camera apex would defeat the narrowing.
    pub(crate) fn slide_near_plane_to(&mut self, apex: Vec3) {
        debug_assert_eq!(
            self.planes.len(),
            6,
            "slide_near_plane_to expects the canonical 6-plane extraction; \
             narrowed sub-frustums must not call this"
        );
        if let Some(near) = self.planes.get_mut(NEAR_PLANE_INDEX) {
            near.dist = -near.normal.dot(apex);
        }
    }
}

/// Extract the six frustum planes from a combined view-projection matrix.
///
/// Uses the Griess-Hartmann method for a right-handed projection:
/// each plane is a combination of rows from the 4x4 matrix. The resulting
/// planes point inward (a point satisfying all six is inside the frustum).
pub fn extract_frustum_planes(view_proj: Mat4) -> Frustum {
    // glam stores matrices column-major. To get row N, we read element N from each column.
    let row = |n: usize| -> Vec4 {
        Vec4::new(
            view_proj.col(0)[n],
            view_proj.col(1)[n],
            view_proj.col(2)[n],
            view_proj.col(3)[n],
        )
    };

    let r0 = row(0);
    let r1 = row(1);
    let r2 = row(2);
    let r3 = row(3);

    let raw_planes = [
        r3 + r0, // Left
        r3 - r0, // Right
        r3 + r1, // Bottom
        r3 - r1, // Top
        r3 + r2, // Near
        r3 - r2, // Far
    ];

    let mut planes = Vec::with_capacity(6);

    for raw in &raw_planes {
        let normal = Vec3::new(raw.x, raw.y, raw.z);
        let length = normal.length();
        if length > 0.0 {
            let inv_len = 1.0 / length;
            planes.push(FrustumPlane {
                normal: normal * inv_len,
                dist: raw.w * inv_len,
            });
        } else {
            planes.push(FrustumPlane {
                normal: Vec3::ZERO,
                dist: 0.0,
            });
        }
    }

    Frustum { planes }
}

/// Test whether an axis-aligned bounding box is completely outside the frustum.
///
/// Uses the "positive vertex" (p-vertex) test: for each frustum plane, find the AABB
/// corner most in the direction of the plane normal. If that corner is behind the plane,
/// the entire AABB is outside. This is conservative — partially-outside boxes pass.
pub(crate) fn is_aabb_outside_frustum(mins: Vec3, maxs: Vec3, frustum: &Frustum) -> bool {
    for plane in &frustum.planes {
        // Select the AABB vertex farthest along the plane normal (positive vertex).
        let p_vertex = Vec3::new(
            if plane.normal.x >= 0.0 {
                maxs.x
            } else {
                mins.x
            },
            if plane.normal.y >= 0.0 {
                maxs.y
            } else {
                mins.y
            },
            if plane.normal.z >= 0.0 {
                maxs.z
            } else {
                mins.z
            },
        );

        // If the positive vertex is behind the plane, the AABB is fully outside.
        if plane.normal.dot(p_vertex) + plane.dist < 0.0 {
            return true;
        }
    }

    false
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

/// Internal result of shared cell-level visibility determination.
struct CellVisResult {
    /// Visible cell indices, or `None` for the empty-world DrawAll case.
    cells: Option<Vec<usize>>,
    /// Wider, fog/light-reachable cell set. Same predicates as `cells`
    /// minus the `face_count > 0` filter. Populated on the portal path and
    /// the portal step-limit fallback; empty on every other variant.
    fog_reachable: Vec<u32>,
    camera_cell: u32,
    total_faces: u32,
    path: CellVisPath,
    /// The camera frustum extracted for this frame.
    frustum: Frustum,
}

/// Collect all drawable cells that pass AABB frustum culling.
fn visible_cells_frustum_all(cells: &[CellData], frustum: &Frustum) -> Vec<usize> {
    cells
        .iter()
        .enumerate()
        .filter(|(_, cell)| {
            cell.is_drawable && !is_aabb_outside_frustum(cell.bounds_min, cell.bounds_max, frustum)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Fog/light reach for the portal step-limit fallback: every non-solid,
/// non-exterior cell whose AABB passes `frustum`, plus the camera cell.
/// Callers must pass the frustum with its near plane already slid to the
/// camera position (`Frustum::slide_near_plane_to`) — the same starting
/// frustum `portal_traverse_inner` narrows from. The unslid camera frustum
/// has a near cutoff around 0.05 units, so a reachable cell beside or just
/// behind the camera plane would fail the AABB test and drop out of this
/// set; sliding the near plane first keeps this a true superset of the
/// exact walk. This is the drawing fallback's predicate minus
/// `face_count > 0`, mirroring how the portal path's `fog_reachable` relates
/// to its drawable set. Every narrowed portal sub-frustum is a subset of the
/// walk's slid starting frustum, so this bounds the exact portal set from
/// above. The camera cell is forced in so the set is never empty (empty
/// means "no portal isolation" downstream) even when its AABB sits wholly
/// behind the near plane. Ascending order.
fn fog_reachable_frustum_fallback(
    cells: &[CellData],
    frustum: &Frustum,
    camera_cell: usize,
) -> Vec<u32> {
    cells
        .iter()
        .enumerate()
        .filter(|(i, cell)| {
            !cell.is_solid
                && !cell.is_exterior
                && (*i == camera_cell
                    || !is_aabb_outside_frustum(cell.bounds_min, cell.bounds_max, frustum))
        })
        .map(|(i, _)| i as u32)
        .collect()
}

/// Shared cell-level visibility determination. Identifies which cells are
/// visible and through which path. `determine_visible_cells` delegates here.
/// `portal_step_limit` is the portal walk's per-frame budget; tests inject a
/// small one to force the step-limit fallback.
fn determine_visible_cell_set(
    camera_position: Vec3,
    view_proj: Mat4,
    world: &LevelWorld,
    blocked_portals: &[bool],
    capture_portal_walk: bool,
    portal_step_limit: u32,
    cpu: &StageFrame<VisibilityStage>,
) -> CellVisResult {
    let total_faces = world.total_face_count();
    let mut frustum = extract_frustum_planes(view_proj);

    if world.cells.is_empty() {
        return CellVisResult {
            cells: None,
            fog_reachable: Vec::new(),
            camera_cell: 0,
            total_faces,
            path: CellVisPath::EmptyWorld,
            frustum,
        };
    }

    let camera_cell_idx = world.locate_cell(camera_position);

    // Solid cell fallback.
    let in_solid = world
        .cells
        .get(camera_cell_idx)
        .is_some_and(|cell| cell.is_solid);

    if in_solid {
        log::warn!(
            "[Visibility] path=SolidCellFallback camera in solid cell {}",
            camera_cell_idx,
        );
        let visible = visible_cells_frustum_all(&world.cells, &frustum);
        return CellVisResult {
            cells: Some(visible),
            fog_reachable: Vec::new(),
            camera_cell: camera_cell_idx as u32,
            total_faces,
            path: CellVisPath::SolidCell,
            frustum,
        };
    }

    // Exterior camera fallback: camera is in a compiler-marked exterior cell.
    // Frustum-cull every drawable cell.
    let in_exterior = world
        .cells
        .get(camera_cell_idx)
        .is_some_and(|cell| cell.is_exterior);

    if in_exterior {
        let visible = visible_cells_frustum_all(&world.cells, &frustum);
        return CellVisResult {
            cells: Some(visible),
            fog_reachable: Vec::new(),
            camera_cell: camera_cell_idx as u32,
            total_faces,
            path: CellVisPath::ExteriorCell,
            frustum,
        };
    }

    if world.has_portals {
        // Runtime portal traversal. Polygon-vs-frustum clipping at each hop
        // keeps every narrowed frustum a strict subset of the camera frustum,
        // so the reachability bitset is also the final visibility set — no
        // per-cell AABB cull needed on this path.
        let portal_result = portal_vis::portal_traverse_with_step_limit(
            camera_position,
            camera_cell_idx,
            &frustum,
            world,
            blocked_portals,
            capture_portal_walk,
            portal_step_limit,
            cpu.gate(),
        );
        // A step-limit trip is still a walk frame: its walk is the cost the
        // budget exists to bound. An out-of-range camera cell never floods, so
        // it records no walk.
        if let Some(walk_nanos) = portal_result.stats.walk_nanos {
            cpu_stages::record_walk(cpu, &portal_result.stats, walk_nanos);
        }

        if portal_result.stats.step_limit_hit {
            log::debug!(
                "[Visibility] path=PortalStepLimitFallback cell={}, considered={}, accepted={}; \
                 using per-cell AABB culling for this frame",
                camera_cell_idx,
                portal_result.stats.considered,
                portal_result.stats.accepted,
            );
            // Stay bounded: fog, shadow-light eligibility, and the SH compose
            // gate all read `fog_reachable`, and an empty set would make each
            // of them fall back to "every cell" on exactly the frames the walk
            // was already too expensive.
            let visible = visible_cells_frustum_all(&world.cells, &frustum);
            // Fog reach needs the near plane slid to the camera, matching the
            // exact walk's starting frustum — see `fog_reachable_frustum_fallback`.
            // Slide `frustum`'s own near plane in place and restore it after,
            // rather than heap-allocating a cloned `Frustum` every step-limit
            // frame.
            let original_near = frustum.planes[NEAR_PLANE_INDEX];
            frustum.slide_near_plane_to(camera_position);
            let fog_reachable =
                fog_reachable_frustum_fallback(&world.cells, &frustum, camera_cell_idx);
            frustum.planes[NEAR_PLANE_INDEX] = original_near;
            return CellVisResult {
                cells: Some(visible),
                fog_reachable,
                camera_cell: camera_cell_idx as u32,
                total_faces,
                path: CellVisPath::PortalStepLimit {
                    considered: portal_result.stats.considered,
                    accepted: portal_result.stats.accepted,
                },
                frustum,
            };
        }

        let portal_visible = portal_result.visible;

        let cells: Vec<usize> = world
            .cells
            .iter()
            .enumerate()
            .filter(|(cell_idx, cell)| {
                cell.is_drawable && portal_visible.get(*cell_idx).copied().unwrap_or(false)
            })
            .map(|(i, _)| i)
            .collect();

        // Wider set for fog and dynamic-light cell gating: drop the
        // `face_count > 0` predicate so empty cells on the portal walk
        // still bound light/fog influence. `!is_solid` and the portal
        // reachability bit stay.
        let fog_reachable: Vec<u32> = world
            .cells
            .iter()
            .enumerate()
            .filter(|(cell_idx, cell)| {
                !cell.is_solid && portal_visible.get(*cell_idx).copied().unwrap_or(false)
            })
            .map(|(i, _)| i as u32)
            .collect();

        return CellVisResult {
            cells: Some(cells),
            fog_reachable,
            camera_cell: camera_cell_idx as u32,
            total_faces,
            path: CellVisPath::Portal,
            frustum,
        };
    }

    // No portals: frustum-cull all drawable cells.
    let visible = visible_cells_frustum_all(&world.cells, &frustum);
    CellVisResult {
        cells: Some(visible),
        fog_reachable: Vec::new(),
        camera_cell: camera_cell_idx as u32,
        total_faces,
        path: CellVisPath::NoPortals,
        frustum,
    }
}

/// Count non-zero-index-count faces across the given visible cells, and
/// convert the internal path tag to the public `VisibilityPath`. Shared by
/// both adapter functions to avoid duplicating stats logic.
fn build_visibility_stats(
    result: &CellVisResult,
    visible_cells: &[usize],
    world: &LevelWorld,
    cpu: StageFrame<VisibilityStage>,
) -> VisibilityStats {
    let mut drawn_faces = 0u32;
    for &cell_idx in visible_cells {
        let cell = &world.cells[cell_idx];
        drawn_faces += cell.face_count;
    }

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

/// GPU-driven visibility path: run portal traversal with a frustum-cull
/// fallback and produce the set of visible cell IDs consumed by the BVH
/// traversal compute shader (via the visible-cell bitmask).
///
/// Pipeline: cell locator to find the camera cell, portal traversal for
/// visible cells, frustum culling discards cells whose AABB falls entirely
/// outside the view frustum. Fallbacks (solid cell, exterior camera, empty
/// world, no portal data) all feed the same downstream bitmask path.
///
/// `scratch` is cleared and reused to avoid per-frame allocation. The caller
/// reclaims it from `VisibleCells::Culled` after the compute pass consumes it.
pub fn determine_visible_cells(
    camera_position: Vec3,
    view_proj: Mat4,
    world: &LevelWorld,
    blocked_portals: &[bool],
    capture_portal_walk: bool,
    scratch: &mut Vec<u32>,
    timing: TimingGate,
) -> (VisibilityResult, Frustum) {
    determine_visible_cells_with_step_limit(
        camera_position,
        view_proj,
        world,
        blocked_portals,
        capture_portal_walk,
        scratch,
        portal_vis::MAX_PORTAL_WALK_STEPS,
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
    scratch: &mut Vec<u32>,
    portal_step_limit: u32,
    timing: TimingGate,
) -> (VisibilityResult, Frustum) {
    let cpu = StageFrame::new(timing);
    let result = determine_visible_cell_set(
        camera_position,
        view_proj,
        world,
        blocked_portals,
        capture_portal_walk,
        portal_step_limit,
        &cpu,
    );
    if !matches!(
        result.path,
        CellVisPath::Portal | CellVisPath::PortalStepLimit { .. }
    ) {
        cpu.mark(VisibilityStage::PortalFallback);
    }

    let visible_cells = match result.cells {
        None => {
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
        Some(ref cells) => cells,
    };

    scratch.clear();
    for &cell_idx in visible_cells {
        scratch.push(cell_idx as u32);
    }

    let stats = build_visibility_stats(&result, visible_cells, world, cpu);
    let fog_reachable = result.fog_reachable;
    (
        VisibilityResult {
            visible_cells: VisibleCells::Culled(std::mem::take(scratch)),
            fog_reachable,
            stats,
        },
        result.frustum,
    )
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    // -- Frustum plane extraction tests --

    /// Build a view-projection matrix that sees everything in front along -Z.
    /// Camera at the given position, looking down -Z, with a wide FOV.
    fn wide_view_proj(position: Vec3) -> Mat4 {
        let view = glam::camera::rh::view::look_at_mat4(position, position + Vec3::NEG_Z, Vec3::Y);
        let proj = glam::camera::rh::proj::directx::perspective(
            std::f32::consts::FRAC_PI_2, // 90-degree vertical FOV
            16.0 / 9.0,
            0.1,
            4096.0,
        );
        proj * view
    }

    #[test]
    fn frustum_planes_are_normalized() {
        let vp = wide_view_proj(Vec3::ZERO);
        let frustum = extract_frustum_planes(vp);

        for (i, plane) in frustum.planes.iter().enumerate() {
            let len = plane.normal.length();
            assert!(
                (len - 1.0).abs() < 1e-5,
                "plane {i} normal not normalized: length = {len}"
            );
        }
    }

    #[test]
    fn frustum_planes_count() {
        let vp = wide_view_proj(Vec3::ZERO);
        let frustum = extract_frustum_planes(vp);
        assert_eq!(frustum.planes.len(), 6, "should have exactly 6 planes");
    }

    #[test]
    fn frustum_origin_is_inside() {
        // Camera at origin looking down -Z. The origin should be inside the frustum.
        let vp = wide_view_proj(Vec3::ZERO);
        let frustum = extract_frustum_planes(vp);

        // A point just in front of the camera (past the near plane) should be inside.
        let test_point = Vec3::new(0.0, 0.0, -1.0);
        let mut inside = true;
        for plane in &frustum.planes {
            if plane.normal.dot(test_point) + plane.dist < 0.0 {
                inside = false;
                break;
            }
        }
        assert!(
            inside,
            "point just in front of camera should be inside frustum"
        );
    }

    #[test]
    fn point_behind_camera_is_outside() {
        // Camera at origin looking down -Z. A point behind (+Z) should be outside.
        let vp = wide_view_proj(Vec3::ZERO);
        let frustum = extract_frustum_planes(vp);

        let test_point = Vec3::new(0.0, 0.0, 10.0);
        let mut outside = false;
        for plane in &frustum.planes {
            if plane.normal.dot(test_point) + plane.dist < 0.0 {
                outside = true;
                break;
            }
        }
        assert!(outside, "point behind camera should be outside frustum");
    }

    // -- AABB-frustum tests --

    #[test]
    fn aabb_fully_inside_frustum_is_not_culled() {
        // Camera at origin looking down -Z. Box centered at (0, 0, -50).
        let vp = wide_view_proj(Vec3::ZERO);
        let frustum = extract_frustum_planes(vp);

        let mins = Vec3::new(-10.0, -10.0, -60.0);
        let maxs = Vec3::new(10.0, 10.0, -40.0);
        assert!(
            !is_aabb_outside_frustum(mins, maxs, &frustum),
            "box directly in front should not be culled"
        );
    }

    #[test]
    fn aabb_fully_behind_camera_is_culled() {
        // Camera at origin looking down -Z. Box behind at (0, 0, +50).
        let vp = wide_view_proj(Vec3::ZERO);
        let frustum = extract_frustum_planes(vp);

        let mins = Vec3::new(-10.0, -10.0, 40.0);
        let maxs = Vec3::new(10.0, 10.0, 60.0);
        assert!(
            is_aabb_outside_frustum(mins, maxs, &frustum),
            "box behind camera should be culled"
        );
    }

    #[test]
    fn aabb_far_left_is_culled() {
        // Camera at origin looking down -Z. Box far to the left.
        let vp = wide_view_proj(Vec3::ZERO);
        let frustum = extract_frustum_planes(vp);

        let mins = Vec3::new(-500.0, -10.0, -60.0);
        let maxs = Vec3::new(-490.0, 10.0, -40.0);
        assert!(
            is_aabb_outside_frustum(mins, maxs, &frustum),
            "box far to the left should be culled"
        );
    }

    #[test]
    fn aabb_far_right_is_culled() {
        // Camera at origin looking down -Z. Box far to the right.
        let vp = wide_view_proj(Vec3::ZERO);
        let frustum = extract_frustum_planes(vp);

        let mins = Vec3::new(490.0, -10.0, -60.0);
        let maxs = Vec3::new(500.0, 10.0, -40.0);
        assert!(
            is_aabb_outside_frustum(mins, maxs, &frustum),
            "box far to the right should be culled"
        );
    }

    #[test]
    fn aabb_far_above_is_culled() {
        // Camera at origin looking down -Z. Box far above.
        let vp = wide_view_proj(Vec3::ZERO);
        let frustum = extract_frustum_planes(vp);

        let mins = Vec3::new(-10.0, 490.0, -60.0);
        let maxs = Vec3::new(10.0, 500.0, -40.0);
        assert!(
            is_aabb_outside_frustum(mins, maxs, &frustum),
            "box far above should be culled"
        );
    }

    #[test]
    fn aabb_far_below_is_culled() {
        // Camera at origin looking down -Z. Box far below.
        let vp = wide_view_proj(Vec3::ZERO);
        let frustum = extract_frustum_planes(vp);

        let mins = Vec3::new(-10.0, -500.0, -60.0);
        let maxs = Vec3::new(10.0, -490.0, -40.0);
        assert!(
            is_aabb_outside_frustum(mins, maxs, &frustum),
            "box far below should be culled"
        );
    }

    #[test]
    fn aabb_beyond_far_plane_is_culled() {
        // Camera at origin looking down -Z. Box beyond the far plane (4096).
        let vp = wide_view_proj(Vec3::ZERO);
        let frustum = extract_frustum_planes(vp);

        let mins = Vec3::new(-10.0, -10.0, -5000.0);
        let maxs = Vec3::new(10.0, 10.0, -4500.0);
        assert!(
            is_aabb_outside_frustum(mins, maxs, &frustum),
            "box beyond far plane should be culled"
        );
    }

    #[test]
    fn aabb_straddling_frustum_edge_is_not_culled() {
        // Camera at origin looking down -Z. Large box that straddles the left edge.
        let vp = wide_view_proj(Vec3::ZERO);
        let frustum = extract_frustum_planes(vp);

        // This box extends from inside to outside the left plane — conservative test keeps it.
        let mins = Vec3::new(-100.0, -10.0, -60.0);
        let maxs = Vec3::new(0.0, 10.0, -40.0);
        assert!(
            !is_aabb_outside_frustum(mins, maxs, &frustum),
            "box straddling frustum edge should not be culled (conservative)"
        );
    }

    #[test]
    fn aabb_enclosing_camera_is_not_culled() {
        // Camera at origin, box encloses the camera entirely.
        let vp = wide_view_proj(Vec3::ZERO);
        let frustum = extract_frustum_planes(vp);

        let mins = Vec3::splat(-1000.0);
        let maxs = Vec3::splat(1000.0);
        assert!(
            !is_aabb_outside_frustum(mins, maxs, &frustum),
            "box enclosing the camera should not be culled"
        );
    }

    // -- PRL cell-based visibility tests --

    use postretro_level_loader::LevelWorld;

    fn prl_cell(
        bounds_min: Vec3,
        bounds_max: Vec3,
        face_start: u32,
        face_count: u32,
        is_solid: bool,
        is_exterior: bool,
    ) -> CellData {
        CellData {
            bounds_min,
            bounds_max,
            face_start,
            face_count,
            portal_ref_start: 0,
            portal_ref_count: 0,
            is_solid,
            is_exterior,
            is_drawable: !is_solid && !is_exterior && face_count > 0,
        }
    }

    /// Two-cell PRL world. The locator returns cell 0 for these tests.
    fn two_cell_prl_world() -> LevelWorld {
        LevelWorld::new_visibility_only(
            vec![
                prl_cell(
                    Vec3::new(0.0, -100.0, -100.0),
                    Vec3::new(100.0, 100.0, 100.0),
                    0,
                    1,
                    false,
                    false,
                ),
                prl_cell(
                    Vec3::new(-100.0, -100.0, -100.0),
                    Vec3::new(0.0, 100.0, 100.0),
                    1,
                    1,
                    false,
                    false,
                ),
            ],
            vec![],
            postretro_level_loader::CellLocatorChild::Cell(0),
            vec![],
            vec![],
            false,
        )
        .expect("valid visibility-only test world")
    }

    #[test]
    fn visible_cells_returns_camera_cell() {
        let world = two_cell_prl_world();
        let vp = wide_view_proj(Vec3::new(50.0, 0.0, 0.0));
        let mut scratch = Vec::new();
        let (result, _frustum) = determine_visible_cells(
            Vec3::new(50.0, 0.0, 0.0),
            vp,
            &world,
            &[],
            false,
            &mut scratch,
            TimingGate::OFF,
        );
        match result.visible_cells {
            VisibleCells::Culled(cells) => {
                assert!(cells.contains(&0), "camera cell 0 should be visible");
            }
            VisibleCells::DrawAll => panic!("expected Culled"),
        }
        assert_eq!(result.stats.total_faces, 2);
        assert!(matches!(
            result.stats.path,
            VisibilityPath::NoPortalsFallback
        ));
        // No-portals fallback does not populate the wider fog/light set.
        assert!(result.fog_reachable.is_empty());
    }

    #[test]
    fn visible_cells_empty_world_draws_all() {
        let world = LevelWorld::new_visibility_only(
            vec![],
            vec![],
            postretro_level_loader::CellLocatorChild::Cell(0),
            vec![],
            vec![],
            false,
        )
        .expect("valid empty visibility-only test world");
        let vp = wide_view_proj(Vec3::ZERO);
        let mut scratch = Vec::new();
        let (result, _frustum) = determine_visible_cells(
            Vec3::ZERO,
            vp,
            &world,
            &[],
            false,
            &mut scratch,
            TimingGate::OFF,
        );
        assert!(matches!(result.visible_cells, VisibleCells::DrawAll));
        assert_eq!(result.stats.total_faces, 0);
        assert!(matches!(
            result.stats.path,
            VisibilityPath::EmptyWorldFallback
        ));
        assert!(result.fog_reachable.is_empty());
    }

    #[test]
    fn visible_cells_frustum_culling_removes_offscreen_cell() {
        let world = two_cell_prl_world();
        let position = Vec3::new(50.0, 0.0, 0.0);
        // Looking down +X, away from cell 1.
        let view = glam::camera::rh::view::look_at_mat4(position, position + Vec3::X, Vec3::Y);
        let proj = glam::camera::rh::proj::directx::perspective(
            std::f32::consts::FRAC_PI_4,
            1.0,
            0.1,
            4096.0,
        );
        let vp = proj * view;

        let mut scratch = Vec::new();
        let (result, _frustum) = determine_visible_cells(
            position,
            vp,
            &world,
            &[],
            false,
            &mut scratch,
            TimingGate::OFF,
        );
        match result.visible_cells {
            VisibleCells::Culled(cells) => {
                assert_eq!(cells.len(), 1, "should cull cell behind camera");
                assert_eq!(cells[0], 0);
            }
            VisibleCells::DrawAll => panic!("expected Culled"),
        }
        assert!(matches!(
            result.stats.path,
            VisibilityPath::NoPortalsFallback
        ));
    }

    #[test]
    fn portal_stats_walk_reach_counts_drawable_cells_not_faces() {
        let mut world = two_cell_prl_world();
        world.cells[0].face_count = 2;
        world.cells[1].face_count = 3;
        let result = CellVisResult {
            cells: Some(vec![0, 1]),
            fog_reachable: Vec::new(),
            camera_cell: 0,
            total_faces: 5,
            path: CellVisPath::Portal,
            frustum: extract_frustum_planes(wide_view_proj(Vec3::ZERO)),
        };

        let stats = build_visibility_stats(&result, &[0, 1], &world, StageFrame::default());

        assert_eq!(stats.drawn_faces, 5);
        assert_eq!(stats.walk_reach(), Some(2));
    }

    #[test]
    fn visible_cells_solid_cell_fallback() {
        let mut world = two_cell_prl_world();
        // Mark cell 0 as solid so the camera at X=50 lands in it.
        world.cells[0].is_solid = true;
        world.cells[0].is_drawable = false;
        let vp = wide_view_proj(Vec3::new(50.0, 0.0, 0.0));
        let mut scratch = Vec::new();
        let (result, _frustum) = determine_visible_cells(
            Vec3::new(50.0, 0.0, 0.0),
            vp,
            &world,
            &[],
            false,
            &mut scratch,
            TimingGate::OFF,
        );
        // Solid fallback draws all drawable cells.
        match result.visible_cells {
            VisibleCells::Culled(cells) => {
                assert!(cells.contains(&1), "non-solid cell 1 should be visible");
                assert!(!cells.contains(&0), "solid cell 0 should not appear");
            }
            VisibleCells::DrawAll => panic!("expected Culled"),
        }
        assert!(matches!(
            result.stats.path,
            VisibilityPath::SolidCellFallback
        ));
        assert!(result.fog_reachable.is_empty());
    }

    #[test]
    fn visible_cells_exterior_camera_fallback() {
        let mut world = two_cell_prl_world();
        // Make cell 0 exterior.
        world.cells[0].face_count = 0;
        world.cells[0].is_drawable = false;
        world.cells[0].is_exterior = true;
        let vp = wide_view_proj(Vec3::new(50.0, 0.0, 0.0));
        let mut scratch = Vec::new();
        let (result, _frustum) = determine_visible_cells(
            Vec3::new(50.0, 0.0, 0.0),
            vp,
            &world,
            &[],
            false,
            &mut scratch,
            TimingGate::OFF,
        );
        match result.visible_cells {
            VisibleCells::Culled(cells) => {
                // Cell 1 still has faces; cell 0 is excluded (exterior).
                assert!(cells.contains(&1));
            }
            VisibleCells::DrawAll => panic!("expected Culled"),
        }
        assert!(matches!(
            result.stats.path,
            VisibilityPath::ExteriorCellFallback
        ));
        assert!(result.fog_reachable.is_empty());
    }

    // -- Portal step-limit fallback --

    /// Camera at the origin looking down -Z through a portal chain, with
    /// in-frustum cells the walk cannot reach:
    ///
    /// - 0: camera cell, drawable, portal 0 at z=-10 to cell 1.
    /// - 1: drawable, portal 1 at z=-30 to cell 2.
    /// - 2: empty (`face_count == 0`), portal-reachable.
    /// - 3: drawable, in frustum, no portals (occluded from the walk).
    /// - 4: solid, in frustum.
    /// - 5: exterior, in frustum.
    /// - 6: drawable, behind the camera.
    fn portal_chain_world() -> LevelWorld {
        let square_at_z = |z: f32| {
            vec![
                Vec3::new(-10.0, -10.0, z),
                Vec3::new(10.0, -10.0, z),
                Vec3::new(10.0, 10.0, z),
                Vec3::new(-10.0, 10.0, z),
            ]
        };
        let mut cells = vec![
            prl_cell(
                Vec3::new(-10.0, -10.0, -10.0),
                Vec3::new(10.0, 10.0, 10.0),
                0,
                1,
                false,
                false,
            ),
            prl_cell(
                Vec3::new(-10.0, -10.0, -30.0),
                Vec3::new(10.0, 10.0, -10.0),
                1,
                1,
                false,
                false,
            ),
            prl_cell(
                Vec3::new(-10.0, -10.0, -50.0),
                Vec3::new(10.0, 10.0, -30.0),
                2,
                0,
                false,
                false,
            ),
            prl_cell(
                Vec3::new(20.0, -10.0, -50.0),
                Vec3::new(40.0, 10.0, -30.0),
                2,
                1,
                false,
                false,
            ),
            prl_cell(
                Vec3::new(-40.0, -10.0, -50.0),
                Vec3::new(-20.0, 10.0, -30.0),
                3,
                0,
                true,
                false,
            ),
            prl_cell(
                Vec3::new(-40.0, -10.0, -30.0),
                Vec3::new(-20.0, 10.0, -10.0),
                3,
                0,
                false,
                true,
            ),
            prl_cell(
                Vec3::new(-10.0, -10.0, 20.0),
                Vec3::new(10.0, 10.0, 40.0),
                3,
                1,
                false,
                false,
            ),
        ];
        // Portal refs: cell 0 -> [0], cell 1 -> [0, 1], cell 2 -> [1].
        cells[0].portal_ref_start = 0;
        cells[0].portal_ref_count = 1;
        cells[1].portal_ref_start = 1;
        cells[1].portal_ref_count = 2;
        cells[2].portal_ref_start = 3;
        cells[2].portal_ref_count = 1;
        LevelWorld::new_visibility_only(
            cells,
            vec![0, 0, 1, 1],
            postretro_level_loader::CellLocatorChild::Cell(0),
            vec![],
            vec![
                postretro_level_loader::PortalData {
                    polygon: square_at_z(-10.0),
                    front_cell: 0,
                    back_cell: 1,
                },
                postretro_level_loader::PortalData {
                    polygon: square_at_z(-30.0),
                    front_cell: 1,
                    back_cell: 2,
                },
            ],
            true,
        )
        .expect("valid portal-chain test world")
    }

    fn culled(cells: &VisibleCells) -> &[u32] {
        match cells {
            VisibleCells::Culled(cells) => cells,
            VisibleCells::DrawAll => panic!("expected Culled"),
        }
    }

    // Regression: stress-warren-hallway-inspection tripped the portal step
    // limit and returned an empty fog_reachable, which fog, shadow-light
    // eligibility, and the SH compose gate all read as "every cell".
    #[test]
    fn step_limit_fallback_fills_fog_reachable_with_frustum_culled_non_solid_cells() {
        let world = portal_chain_world();
        let eye = Vec3::ZERO;
        let mut scratch = Vec::new();
        let (result, _frustum) = determine_visible_cells_with_step_limit(
            eye,
            wide_view_proj(eye),
            &world,
            &[],
            false,
            &mut scratch,
            0,
            TimingGate::OFF,
        );

        assert!(matches!(
            result.stats.path,
            VisibilityPath::PortalStepLimitFallback { .. }
        ));
        // Drawing keeps the existing frustum-culled drawable set, occluded
        // cell 3 included.
        assert_eq!(culled(&result.visible_cells), &[0, 1, 3]);
        // Fog reach is the same set without the face-count filter: empty cell
        // 2 joins; solid 4, exterior 5, and behind-camera 6 stay out.
        assert_eq!(result.fog_reachable, vec![0, 1, 2, 3]);
    }

    #[test]
    fn step_limit_fallback_fog_reachable_keeps_camera_cell_when_frustum_set_is_empty() {
        let world = portal_chain_world();
        // The locator always answers cell 0. Looking down +X from well outside
        // cell 0's AABB, no cell passes the frustum.
        let eye = Vec3::new(100.0, 0.0, 0.0);
        let view = glam::camera::rh::view::look_at_mat4(eye, eye + Vec3::X, Vec3::Y);
        let proj = glam::camera::rh::proj::directx::perspective(
            std::f32::consts::FRAC_PI_4,
            1.0,
            0.1,
            4096.0,
        );
        let mut scratch = Vec::new();
        let (result, _frustum) = determine_visible_cells_with_step_limit(
            eye,
            proj * view,
            &world,
            &[],
            false,
            &mut scratch,
            0,
            TimingGate::OFF,
        );

        assert!(matches!(
            result.stats.path,
            VisibilityPath::PortalStepLimitFallback { .. }
        ));
        assert!(culled(&result.visible_cells).is_empty());
        // Never the empty "no portal isolation" sentinel.
        assert_eq!(result.fog_reachable, vec![0]);
    }

    #[test]
    fn portal_walk_within_step_limit_keeps_exact_visible_and_fog_sets() {
        let world = portal_chain_world();
        let eye = Vec3::ZERO;
        let mut scratch = Vec::new();
        let (result, _frustum) = determine_visible_cells(
            eye,
            wide_view_proj(eye),
            &world,
            &[],
            false,
            &mut scratch,
            TimingGate::OFF,
        );

        assert!(matches!(
            result.stats.path,
            VisibilityPath::PrlPortal { walk_reach: 2 }
        ));
        // Exact portal sets: occluded cell 3 is absent from both.
        assert_eq!(culled(&result.visible_cells), &[0, 1]);
        assert_eq!(result.fog_reachable, vec![0, 1, 2]);
    }

    #[test]
    fn step_limit_fallback_sets_are_supersets_of_exact_portal_sets() {
        let world = portal_chain_world();
        let eye = Vec3::ZERO;
        let vp = wide_view_proj(eye);
        let mut scratch = Vec::new();
        let (exact, _) =
            determine_visible_cells(eye, vp, &world, &[], false, &mut scratch, TimingGate::OFF);
        let exact_visible = culled(&exact.visible_cells).to_vec();
        let (fallback, _) = determine_visible_cells_with_step_limit(
            eye,
            vp,
            &world,
            &[],
            false,
            &mut scratch,
            0,
            TimingGate::OFF,
        );

        let fallback_visible = culled(&fallback.visible_cells);
        for cell in &exact_visible {
            assert!(
                fallback_visible.contains(cell),
                "fallback draw set dropped visible cell {cell}"
            );
        }
        for cell in &exact.fog_reachable {
            assert!(
                fallback.fog_reachable.contains(cell),
                "fallback fog reach dropped reachable cell {cell}"
            );
        }
    }

    // Regression: the fallback fog-reachable set used the unslid camera
    // frustum, whose near cutoff excludes an AABB that lies entirely between
    // the camera and the near plane. The exact portal walk slides its near
    // plane to the camera (`Frustum::slide_near_plane_to`) and can reach such
    // a cell, so the fallback silently dropped it — breaking the "superset
    // of the exact portal set" contract for a cell beside or just behind the
    // camera plane.
    #[test]
    fn step_limit_fallback_fog_reachable_includes_camera_adjacent_near_slab_cell() {
        let camera_cell = prl_cell(
            Vec3::new(-10.0, -10.0, -10.0),
            Vec3::new(10.0, 10.0, 10.0),
            0,
            1,
            false,
            false,
        );
        // Lies entirely in the near slab: z is between the camera (z=0) and
        // the unslid frustum's near plane. `extract_frustum_planes` combines
        // rows for a -1..1 depth convention, but `wide_view_proj` projects
        // with wgpu's 0..1 depth, so the extracted near plane lands at
        // z ≈ -0.05 rather than the configured 0.1-unit near clip. The
        // unslid frustum classifies this AABB as fully outside; sliding the
        // near plane to the camera apex (z=0) brings it inside. `face_count`
        // 1 makes the cell drawable, so it also stands in for the drawing
        // set's "unslid frustum" half of the contract below.
        let mut near_slab_cell = prl_cell(
            Vec3::new(-0.01, -0.01, -0.04),
            Vec3::new(0.01, 0.01, -0.02),
            1,
            1,
            false,
            false,
        );
        near_slab_cell.portal_ref_start = 1;
        near_slab_cell.portal_ref_count = 1;

        let mut cells = vec![camera_cell, near_slab_cell];
        cells[0].portal_ref_start = 0;
        cells[0].portal_ref_count = 1;

        let world = LevelWorld::new_visibility_only(
            cells,
            vec![0, 0],
            postretro_level_loader::CellLocatorChild::Cell(0),
            vec![],
            vec![postretro_level_loader::PortalData {
                polygon: vec![
                    Vec3::new(-0.01, -0.01, -0.02),
                    Vec3::new(0.01, -0.01, -0.02),
                    Vec3::new(0.01, 0.01, -0.02),
                    Vec3::new(-0.01, 0.01, -0.02),
                ],
                front_cell: 0,
                back_cell: 1,
            }],
            true,
        )
        .expect("valid near-slab test world");

        let eye = Vec3::ZERO;
        let vp = wide_view_proj(eye);
        let mut scratch = Vec::new();

        // Ground truth: the exact walk with a large step limit.
        let (exact, _) =
            determine_visible_cells(eye, vp, &world, &[], false, &mut scratch, TimingGate::OFF);
        assert!(
            matches!(exact.stats.path, VisibilityPath::PrlPortal { .. }),
            "expected the exact portal walk to run without hitting the step limit"
        );
        assert!(
            exact.fog_reachable.contains(&1),
            "exact portal walk should reach the near-slab cell beside the camera"
        );

        // Force the step-limit fallback and check its fog-reachable set still
        // covers what the exact walk reached.
        let (fallback, _) = determine_visible_cells_with_step_limit(
            eye,
            vp,
            &world,
            &[],
            false,
            &mut scratch,
            0,
            TimingGate::OFF,
        );
        assert!(matches!(
            fallback.stats.path,
            VisibilityPath::PortalStepLimitFallback { .. }
        ));
        for cell in &exact.fog_reachable {
            assert!(
                fallback.fog_reachable.contains(cell),
                "fallback fog reach dropped near-slab reachable cell {cell}"
            );
        }

        // Pins the other half of the contract: the drawing set keeps the
        // unslid frustum, so the near-slab cell — reachable for fog — must
        // not be drawn. Only the slid fog frustum reaches into the near slab.
        assert!(
            !culled(&fallback.visible_cells).contains(&1),
            "fallback drawing set should not draw the near-slab cell; \
             only fog reach slides its near plane to the camera"
        );
    }

    // --- CPU stage timing ---

    fn cpu(result: &VisibilityResult, stage: VisibilityStage) -> Option<u64> {
        result.stats.cpu.value(stage)
    }

    #[test]
    fn timing_on_and_off_produce_identical_visibility() {
        // Behavior neutrality (P-gate): the gate reaches visibility as a value.
        let world = portal_chain_world();
        for (eye, limit) in [
            (Vec3::ZERO, portal_vis::MAX_PORTAL_WALK_STEPS),
            (Vec3::ZERO, 0),
            (
                Vec3::new(100.0, 0.0, 0.0),
                portal_vis::MAX_PORTAL_WALK_STEPS,
            ),
        ] {
            let vp = wide_view_proj(eye);
            let run = |timing| {
                let mut scratch = Vec::new();
                let (result, _) = determine_visible_cells_with_step_limit(
                    eye,
                    vp,
                    &world,
                    &[],
                    false,
                    &mut scratch,
                    limit,
                    timing,
                );
                (
                    format!("{:?}", result.visible_cells),
                    result.fog_reachable,
                    format!("{:?}", result.stats.path),
                )
            };
            assert_eq!(
                run(TimingGate::ON),
                run(TimingGate::OFF),
                "eye {eye}, limit {limit}"
            );
        }
    }

    #[test]
    fn portal_frame_records_walk_time_and_every_counter() {
        let world = portal_chain_world();
        let eye = Vec3::ZERO;
        let mut scratch = Vec::new();
        let (result, _) = determine_visible_cells(
            eye,
            wide_view_proj(eye),
            &world,
            &[],
            false,
            &mut scratch,
            TimingGate::ON,
        );
        assert!(matches!(
            result.stats.path,
            VisibilityPath::PrlPortal { .. }
        ));
        assert!(cpu(&result, VisibilityStage::PortalWalk).is_some());
        assert!(cpu(&result, VisibilityStage::Considered).unwrap() >= 2);
        assert!(cpu(&result, VisibilityStage::Accepted).unwrap() >= 1);
        for stage in [
            VisibilityStage::RejectedBlocked,
            VisibilityStage::RejectedSolid,
            VisibilityStage::RejectedClipped,
            VisibilityStage::RejectedNarrow,
            VisibilityStage::RejectedInvalid,
            VisibilityStage::RejectedPathCycle,
            VisibilityStage::RejectedDepthLimit,
        ] {
            assert!(cpu(&result, stage).is_some(), "{stage:?} present");
        }
        assert_eq!(cpu(&result, VisibilityStage::StepLimit), None);
        assert_eq!(cpu(&result, VisibilityStage::PortalFallback), None);
    }

    #[test]
    fn step_limit_frame_is_a_walk_frame_not_a_fallback_frame() {
        // P-steplimit
        let world = portal_chain_world();
        let eye = Vec3::ZERO;
        let mut scratch = Vec::new();
        let (result, _) = determine_visible_cells_with_step_limit(
            eye,
            wide_view_proj(eye),
            &world,
            &[],
            false,
            &mut scratch,
            0,
            TimingGate::ON,
        );
        assert!(matches!(
            result.stats.path,
            VisibilityPath::PortalStepLimitFallback { .. }
        ));
        assert!(cpu(&result, VisibilityStage::PortalWalk).is_some());
        assert!(cpu(&result, VisibilityStage::Considered).is_some());
        assert_eq!(cpu(&result, VisibilityStage::StepLimit), Some(1));
        assert_eq!(cpu(&result, VisibilityStage::PortalFallback), None);
    }

    #[test]
    fn walk_that_considers_no_portal_is_present_with_zero_counters() {
        // P-zero-walk: a portal-path world whose camera cell has no portals.
        let mut world = portal_chain_world();
        world.cells[0].portal_ref_count = 0;
        let eye = Vec3::ZERO;
        let mut scratch = Vec::new();
        let (result, _) = determine_visible_cells(
            eye,
            wide_view_proj(eye),
            &world,
            &[],
            false,
            &mut scratch,
            TimingGate::ON,
        );
        assert!(matches!(
            result.stats.path,
            VisibilityPath::PrlPortal { .. }
        ));
        assert!(cpu(&result, VisibilityStage::PortalWalk).is_some());
        assert_eq!(cpu(&result, VisibilityStage::Considered), Some(0));
        assert_eq!(cpu(&result, VisibilityStage::Accepted), Some(0));
    }

    #[test]
    fn fallback_frame_records_only_the_fallback_marker() {
        let world = two_cell_prl_world();
        let eye = Vec3::new(50.0, 0.0, 0.0);
        let mut scratch = Vec::new();
        let (result, _) = determine_visible_cells(
            eye,
            wide_view_proj(eye),
            &world,
            &[],
            false,
            &mut scratch,
            TimingGate::ON,
        );
        assert!(matches!(
            result.stats.path,
            VisibilityPath::NoPortalsFallback
        ));
        assert_eq!(cpu(&result, VisibilityStage::PortalFallback), Some(1));
        assert_eq!(cpu(&result, VisibilityStage::PortalWalk), None);
        assert_eq!(cpu(&result, VisibilityStage::Considered), None);
    }

    #[test]
    fn timing_off_records_no_stage() {
        let world = portal_chain_world();
        let eye = Vec3::ZERO;
        let mut scratch = Vec::new();
        let (result, _) = determine_visible_cells(
            eye,
            wide_view_proj(eye),
            &world,
            &[],
            false,
            &mut scratch,
            TimingGate::OFF,
        );
        use postretro_stage_timing::StageSet;
        assert!(
            VisibilityStage::ALL
                .iter()
                .all(|&stage| cpu(&result, stage).is_none())
        );
    }
}
