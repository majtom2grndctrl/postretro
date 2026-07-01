// Per-frame visibility determination: portal traversal and frustum-culled fallbacks.
// See: context/lib/rendering_pipeline.md

use glam::{Mat4, Vec3, Vec4};

use crate::portal_vis;
use postretro_level_loader::{CellData, LevelWorld};

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
///   is the point of this field). Populated only on the portal path;
///   empty on every fallback (where the downstream consumer treats empty
///   as "no portal isolation").
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
    NoPortals,
}

/// Internal result of shared cell-level visibility determination.
struct CellVisResult {
    /// Visible cell indices, or `None` for the empty-world DrawAll case.
    cells: Option<Vec<usize>>,
    /// Wider, fog/light-reachable cell set. Same predicates as `cells`
    /// minus the `face_count > 0` filter. Populated only on the portal
    /// path; empty on every other variant.
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

/// Shared cell-level visibility determination. Identifies which cells are
/// visible and through which path. `determine_visible_cells` delegates here.
fn determine_visible_cell_set(
    camera_position: Vec3,
    view_proj: Mat4,
    world: &LevelWorld,
    capture_portal_walk: bool,
) -> CellVisResult {
    let total_faces = world.face_meta.len() as u32;
    let frustum = extract_frustum_planes(view_proj);

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
        let portal_visible = portal_vis::portal_traverse(
            camera_position,
            camera_cell_idx,
            &frustum,
            world,
            capture_portal_walk,
        );

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
        CellVisPath::NoPortals => VisibilityPath::NoPortalsFallback,
        CellVisPath::EmptyWorld => VisibilityPath::EmptyWorldFallback,
    };

    if matches!(result.path, CellVisPath::Portal) {
        log::trace!(
            "[Visibility] path=PrlPortal cell={}, walk_reach={}, drawn_faces={}, total_faces={}",
            result.camera_cell,
            visible_cells.len(),
            drawn_faces,
            result.total_faces,
        );
    }

    VisibilityStats {
        camera_cell: result.camera_cell,
        total_faces: result.total_faces,
        drawn_faces,
        path,
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
    capture_portal_walk: bool,
    scratch: &mut Vec<u32>,
) -> (VisibilityResult, Frustum) {
    let result = determine_visible_cell_set(camera_position, view_proj, world, capture_portal_walk);

    let visible_cells = match result.cells {
        None => {
            let stats = VisibilityStats {
                camera_cell: result.camera_cell,
                total_faces: result.total_faces,
                drawn_faces: result.total_faces,
                path: VisibilityPath::EmptyWorldFallback,
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

    let stats = build_visibility_stats(&result, visible_cells, world);
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
        let view = Mat4::look_at_rh(position, position + Vec3::NEG_Z, Vec3::Y);
        let proj = Mat4::perspective_rh(
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

    use postretro_level_loader::{FaceMeta as PrlFaceMeta, LevelWorld};
    use postretro_render_data::geometry::{BvhTree, WorldVertex};
    use postretro_render_data::material::Material;

    fn zero_vertex() -> WorldVertex {
        WorldVertex {
            position: [0.0; 3],
            base_uv: [0.0; 2],
            normal_oct: [32768, 32768],
            tangent_packed: [65535, 0x8000],
            lightmap_uv: [0, 0],
            lightmap_layer: 0,
        }
    }

    fn prl_face_meta() -> PrlFaceMeta {
        PrlFaceMeta {
            leaf_index: 0,
            texture_index: None,
            texture_dimensions: (64, 64),
            texture_name: String::new(),
            material: Material::Default,
        }
    }

    fn empty_bvh() -> BvhTree {
        BvhTree {
            nodes: vec![],
            leaves: vec![],
            root_node_index: 0,
        }
    }

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
        LevelWorld {
            vertices: vec![zero_vertex(); 6],
            indices: vec![0, 1, 2, 3, 4, 5],
            face_meta: vec![prl_face_meta(), prl_face_meta()],
            cells: vec![
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
            cell_portal_refs: vec![],
            cell_locator_root: postretro_level_loader::CellLocatorChild::Cell(0),
            cell_locator_nodes: vec![],
            portals: vec![],
            has_portals: false,
            texture_names: vec![],
            texture_cache_keys:
                postretro_level_format::texture_cache_keys::TextureCacheKeysSection { keys: vec![] },
            bvh: empty_bvh(),
            lights: vec![],
            light_influences: vec![],
            sh_volume: None,
            lightmap: None,
            lightmap_mode: postretro_level_loader::LightmapMode::Shadowed,
            sdf_atlas: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
            delta_sh_volumes: None,
            direct_sh_volume: None,
            data_script: None,
            map_entities: Vec::new(),
            fog_volumes: Vec::new(),
            fog_pixel_scale: 4,
            initial_gravity: -9.81,
            fog_cell_masks: None,
            navmesh: None,
            cell_draw_index: None,
        }
    }

    #[test]
    fn visible_cells_returns_camera_cell() {
        let world = two_cell_prl_world();
        let vp = wide_view_proj(Vec3::new(50.0, 0.0, 0.0));
        let mut scratch = Vec::new();
        let (result, _frustum) =
            determine_visible_cells(Vec3::new(50.0, 0.0, 0.0), vp, &world, false, &mut scratch);
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
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            cells: vec![],
            cell_portal_refs: vec![],
            cell_locator_root: postretro_level_loader::CellLocatorChild::Cell(0),
            cell_locator_nodes: vec![],
            portals: vec![],
            has_portals: false,
            texture_names: vec![],
            texture_cache_keys:
                postretro_level_format::texture_cache_keys::TextureCacheKeysSection { keys: vec![] },
            bvh: empty_bvh(),
            lights: vec![],
            light_influences: vec![],
            sh_volume: None,
            lightmap: None,
            lightmap_mode: postretro_level_loader::LightmapMode::Shadowed,
            sdf_atlas: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
            delta_sh_volumes: None,
            direct_sh_volume: None,
            data_script: None,
            map_entities: Vec::new(),
            fog_volumes: Vec::new(),
            fog_pixel_scale: 4,
            initial_gravity: -9.81,
            fog_cell_masks: None,
            navmesh: None,
            cell_draw_index: None,
        };
        let vp = wide_view_proj(Vec3::ZERO);
        let mut scratch = Vec::new();
        let (result, _frustum) =
            determine_visible_cells(Vec3::ZERO, vp, &world, false, &mut scratch);
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
        let view = Mat4::look_at_rh(position, position + Vec3::X, Vec3::Y);
        let proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, 1.0, 0.1, 4096.0);
        let vp = proj * view;

        let mut scratch = Vec::new();
        let (result, _frustum) = determine_visible_cells(position, vp, &world, false, &mut scratch);
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

        let stats = build_visibility_stats(&result, &[0, 1], &world);

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
        let (result, _frustum) =
            determine_visible_cells(Vec3::new(50.0, 0.0, 0.0), vp, &world, false, &mut scratch);
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
        let (result, _frustum) =
            determine_visible_cells(Vec3::new(50.0, 0.0, 0.0), vp, &world, false, &mut scratch);
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
}
