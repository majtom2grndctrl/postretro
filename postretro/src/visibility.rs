// Per-frame visibility determination: portal traversal, PVS, and frustum-culled fallbacks.
// See: context/lib/rendering_pipeline.md

use glam::{Mat4, Vec3, Vec4};

use crate::portal_vis;
use crate::prl::LevelWorld;

/// A draw range referencing a contiguous run of indices in the shared index buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawRange {
    pub index_offset: u32,
    pub index_count: u32,
}

/// Result of per-frame visibility determination.
#[derive(Debug)]
pub enum VisibleFaces {
    /// PVS data is available; draw only these face ranges.
    Culled(Vec<DrawRange>),
    /// No PVS data; draw everything.
    DrawAll,
}

/// Result of per-frame visibility determination for the GPU-driven indirect
/// draw path. Carries visible cell IDs that feed the compute culling pass
/// instead of per-face draw ranges.
#[derive(Debug)]
pub enum VisibleCells {
    /// Specific cells are visible; pass to the compute cull shader.
    Culled(Vec<u32>),
    /// All cells are visible (empty world or missing visibility data).
    DrawAll,
}

/// Per-frame visibility pipeline statistics for diagnostics.
///
/// `pvs_reach` and `drawn_faces` have uniform meaning across every path,
/// so a reader that just wants a "how much is culling doing?" ratio can
/// use those two fields without inspecting `path`. Path-specific
/// diagnostics live on the `VisibilityPath` variants.
#[derive(Debug, Clone)]
pub struct VisibilityStats {
    /// BSP leaf the camera currently occupies.
    pub camera_leaf: u32,
    /// Total faces in the level.
    pub total_faces: u32,
    /// Angle-independent PVS baseline: the face count the camera leaf's
    /// raw PVS admits, ignoring every view-direction-dependent narrowing
    /// stage. Same meaning on all paths. On fallback paths that bypass PVS
    /// entirely this is `total_faces` (nothing is excluded by the PVS
    /// because the PVS was not consulted).
    pub pvs_reach: u32,
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
/// field; readers that want to distinguish between primary and fallback
/// paths, or inspect portal-specific diagnostics, can `match` on it.
#[derive(Debug, Clone, Copy)]
pub enum VisibilityPath {
    /// Primary PRL rendering path using precomputed PVS bitsets plus
    /// AABB frustum cull.
    PrlPvs,
    /// Primary PRL rendering path using per-frame portal traversal.
    /// Portal traversal narrows the frustum at every hop, so the reach
    /// of the portal walk is also the final visibility set — no separate
    /// AABB cull runs on this path and `drawn_faces == walk_reach`.
    ///
    /// `walk_reach` is exposed on the variant so a reader comparing
    /// `pvs_reach` against `walk_reach` can see how much the portal walk
    /// discarded beyond what PVS alone would have admitted.
    PrlPortal { walk_reach: u32 },
    /// Fallback: no PVS data in the level file. All non-solid non-zero-face
    /// leaves are submitted, subject to AABB frustum culling.
    NoPvsFallback,
    /// Fallback: world has no leaves to cull against. DrawAll with every
    /// face in the level submitted.
    EmptyWorldFallback,
    /// Fallback: camera position lies inside solid geometry (clipped
    /// into a wall). All non-solid non-zero-face leaves are drawn,
    /// subject to AABB frustum culling.
    SolidLeafFallback,
    /// Fallback: camera is in an exterior leaf (empty, no faces). The
    /// camera has left the playable interior — spectator, noclip, debug
    /// fly. All non-solid non-zero-face leaves are drawn, subject to
    /// AABB frustum culling.
    ExteriorCameraFallback,
}

impl VisibilityStats {
    /// On the PRL portal-traversal path, the count of faces the portal
    /// walk can reach from the camera leaf — a subset of `pvs_reach` that
    /// reflects both PVS and portal-chain reachability. `None` on every
    /// other path.
    pub fn walk_reach(&self) -> Option<u32> {
        match self.path {
            VisibilityPath::PrlPortal { walk_reach } => Some(walk_reach),
            _ => None,
        }
    }
}

// --- Frustum culling ---

/// A plane in Hessian normal form: dot(normal, point) + dist >= 0 for points on the inside.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FrustumPlane {
    pub normal: Vec3,
    pub dist: f32,
}

/// The planes of a view frustum, extracted from a view-projection matrix.
///
/// The initial camera frustum always contains exactly 6 planes in
/// left/right/bottom/top/near/far order. After portal traversal narrows the
/// frustum, it may contain more planes (one per portal edge plus near/far).
#[derive(Debug, Clone)]
pub(crate) struct Frustum {
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
pub(crate) fn extract_frustum_planes(view_proj: Mat4) -> Frustum {
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

/// Count drawable faces in the camera leaf's raw PVS, ignoring frustum and
/// portal narrowing. The camera leaf itself is always included even if its
/// own bit is unset, matching the iteration pattern of the PVS path. Used as
/// the angle-independent baseline in `VisibilityStats::pvs_reach` so the
/// portal-traversal path's `walk_reach` can be compared against "what PVS
/// allows."
fn raw_pvs_face_count(world: &LevelWorld, camera_leaf_idx: usize) -> u32 {
    let pvs = match world.leaves.get(camera_leaf_idx) {
        Some(leaf) => &leaf.pvs,
        None => return world.face_meta.len() as u32,
    };

    let mut count = 0u32;
    for (leaf_idx, leaf) in world.leaves.iter().enumerate() {
        if leaf.is_solid || leaf.face_count == 0 {
            continue;
        }
        let is_camera_leaf = leaf_idx == camera_leaf_idx;
        let is_pvs_visible = pvs.get(leaf_idx).copied().unwrap_or(false);
        if !is_pvs_visible && !is_camera_leaf {
            continue;
        }
        let start = leaf.face_start as usize;
        let n = leaf.face_count as usize;
        count += world
            .face_meta
            .iter()
            .skip(start)
            .take(n)
            .filter(|f| f.index_count > 0)
            .count() as u32;
    }
    count
}

// --- Shared leaf-level visibility determination ---

/// Internal classification of which visibility path was selected.
#[derive(Debug, Clone, Copy)]
enum LeafVisPath {
    EmptyWorld,
    SolidLeaf,
    ExteriorCamera,
    Portal,
    NoPvs,
    Pvs,
}

/// Internal result of shared leaf-level visibility determination.
struct LeafVisResult {
    /// Visible leaf indices, or `None` for the empty-world DrawAll case.
    leaves: Option<Vec<usize>>,
    camera_leaf: u32,
    total_faces: u32,
    pvs_reach: u32,
    path: LeafVisPath,
}

/// Collect all non-solid non-empty leaves that pass AABB frustum culling.
fn visible_leaves_frustum_all(
    leaves: &[crate::prl::LeafData],
    frustum: &Frustum,
) -> Vec<usize> {
    leaves
        .iter()
        .enumerate()
        .filter(|(_, leaf)| {
            !leaf.is_solid
                && leaf.face_count > 0
                && !is_aabb_outside_frustum(leaf.bounds_min, leaf.bounds_max, frustum)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Shared leaf-level visibility determination. Identifies which leaves are
/// visible and through which path, without collecting output-specific data
/// (draw ranges or cell IDs). Both `determine_prl_visibility` and
/// `determine_visible_cells` delegate here.
fn determine_visible_leaf_set(
    camera_position: Vec3,
    view_proj: Mat4,
    world: &LevelWorld,
    capture_portal_walk: bool,
) -> LeafVisResult {
    let total_faces = world.face_meta.len() as u32;

    if world.leaves.is_empty() {
        return LeafVisResult {
            leaves: None,
            camera_leaf: 0,
            total_faces,
            pvs_reach: total_faces,
            path: LeafVisPath::EmptyWorld,
        };
    }

    let camera_leaf_idx = world.find_leaf(camera_position);
    let frustum = extract_frustum_planes(view_proj);

    // Solid leaf fallback.
    let in_solid = world
        .leaves
        .get(camera_leaf_idx)
        .is_some_and(|l| l.is_solid);

    if in_solid {
        log::warn!(
            "[Visibility] path=SolidLeafFallback camera in solid leaf {}",
            camera_leaf_idx,
        );
        return LeafVisResult {
            leaves: Some(visible_leaves_frustum_all(&world.leaves, &frustum)),
            camera_leaf: camera_leaf_idx as u32,
            total_faces,
            pvs_reach: total_faces,
            path: LeafVisPath::SolidLeaf,
        };
    }

    // Exterior camera fallback: camera is in an empty leaf with no faces
    // (the structural signature of exterior leaves after the compiler strips
    // their face data). Frustum-cull every non-solid non-zero-face leaf.
    let in_exterior = world
        .leaves
        .get(camera_leaf_idx)
        .is_some_and(|l| !l.is_solid && l.face_count == 0);

    if in_exterior {
        return LeafVisResult {
            leaves: Some(visible_leaves_frustum_all(&world.leaves, &frustum)),
            camera_leaf: camera_leaf_idx as u32,
            total_faces,
            pvs_reach: total_faces,
            path: LeafVisPath::ExteriorCamera,
        };
    }

    let pvs_reach = raw_pvs_face_count(world, camera_leaf_idx);

    if world.has_portals {
        // Runtime portal traversal. Polygon-vs-frustum clipping at each hop
        // keeps every narrowed frustum a strict subset of the camera frustum,
        // so the reachability bitset is also the final visibility set — no
        // per-leaf AABB cull needed on this path.
        let portal_visible = portal_vis::portal_traverse(
            camera_position,
            camera_leaf_idx,
            &frustum,
            world,
            capture_portal_walk,
        );

        let leaves: Vec<usize> = world
            .leaves
            .iter()
            .enumerate()
            .filter(|(leaf_idx, leaf)| {
                !leaf.is_solid
                    && leaf.face_count > 0
                    && portal_visible.get(*leaf_idx).copied().unwrap_or(false)
            })
            .map(|(i, _)| i)
            .collect();

        return LeafVisResult {
            leaves: Some(leaves),
            camera_leaf: camera_leaf_idx as u32,
            total_faces,
            pvs_reach,
            path: LeafVisPath::Portal,
        };
    }

    if !world.has_pvs {
        return LeafVisResult {
            leaves: Some(visible_leaves_frustum_all(&world.leaves, &frustum)),
            camera_leaf: camera_leaf_idx as u32,
            total_faces,
            pvs_reach: total_faces,
            path: LeafVisPath::NoPvs,
        };
    }

    // PVS available.
    let pvs = &world.leaves[camera_leaf_idx].pvs;
    let leaves: Vec<usize> = world
        .leaves
        .iter()
        .enumerate()
        .filter(|(leaf_idx, leaf)| {
            if leaf.is_solid || leaf.face_count == 0 {
                return false;
            }
            let is_camera_leaf = *leaf_idx == camera_leaf_idx;
            let is_pvs_visible = pvs.get(*leaf_idx).copied().unwrap_or(false);
            if !is_pvs_visible && !is_camera_leaf {
                return false;
            }
            !is_aabb_outside_frustum(leaf.bounds_min, leaf.bounds_max, &frustum)
        })
        .map(|(i, _)| i)
        .collect();

    LeafVisResult {
        leaves: Some(leaves),
        camera_leaf: camera_leaf_idx as u32,
        total_faces,
        pvs_reach,
        path: LeafVisPath::Pvs,
    }
}

/// Count non-zero-index-count faces across the given visible leaves, and
/// convert the internal path tag to the public `VisibilityPath`. Shared by
/// both adapter functions to avoid duplicating stats logic.
fn build_visibility_stats(
    result: &LeafVisResult,
    visible_leaves: &[usize],
    world: &LevelWorld,
) -> VisibilityStats {
    let mut drawn_faces = 0u32;
    for &leaf_idx in visible_leaves {
        let leaf = &world.leaves[leaf_idx];
        let start = leaf.face_start as usize;
        let count = leaf.face_count as usize;
        for face in world.face_meta.iter().skip(start).take(count) {
            if face.index_count > 0 {
                drawn_faces += 1;
            }
        }
    }

    let path = match result.path {
        LeafVisPath::SolidLeaf => VisibilityPath::SolidLeafFallback,
        LeafVisPath::ExteriorCamera => VisibilityPath::ExteriorCameraFallback,
        // Portal: drawn_faces == walk_reach by construction (no separate
        // AABB cull after portal narrowing).
        LeafVisPath::Portal => VisibilityPath::PrlPortal {
            walk_reach: drawn_faces,
        },
        LeafVisPath::NoPvs => VisibilityPath::NoPvsFallback,
        LeafVisPath::Pvs => VisibilityPath::PrlPvs,
        LeafVisPath::EmptyWorld => VisibilityPath::EmptyWorldFallback,
    };

    match result.path {
        LeafVisPath::Portal => {
            log::trace!(
                "[Visibility] path=PrlPortal leaf={}, pvs_reach={}, walk_reach={}, drawn_faces={}, total_faces={}",
                result.camera_leaf,
                result.pvs_reach,
                drawn_faces,
                drawn_faces,
                result.total_faces,
            );
        }
        LeafVisPath::Pvs => {
            log::trace!(
                "[Visibility] path=PrlPvs leaf={}, pvs_reach={}, drawn_faces={}, total_faces={}",
                result.camera_leaf,
                result.pvs_reach,
                drawn_faces,
                result.total_faces,
            );
        }
        _ => {}
    }

    VisibilityStats {
        camera_leaf: result.camera_leaf,
        total_faces: result.total_faces,
        pvs_reach: result.pvs_reach,
        drawn_faces,
        path,
    }
}

// --- Public visibility APIs ---

/// Perform full visibility determination for a PRL level.
///
/// Pipeline: BSP tree descent to find camera leaf, PVS lookup for visible
/// leaves, then frustum culling discards leaves whose bounding box falls
/// entirely outside the view frustum.
///
/// Solid leaf fallback: if the camera lands in a solid leaf (clipped into
/// geometry), all leaves are drawn. This avoids complexity of finding the
/// "nearest empty leaf" for a rare edge case.
///
/// `scratch` is cleared on entry by every branch that returns
/// `VisibleFaces::Culled`, and populated in place. The `DrawAll` early-return
/// branch (empty world) intentionally does not touch `scratch` — reclaim is a
/// no-op in that case and `App::scratch_ranges` retains its capacity for the
/// next `Culled` frame. The steady-state zero-allocation contract depends on
/// main.rs reclaiming the allocation from `VisibleFaces::Culled` after
/// `render_frame` consumes it; see the `App::scratch_ranges` field for the
/// reclaim side of the handshake.
pub fn determine_prl_visibility(
    camera_position: Vec3,
    view_proj: Mat4,
    world: &LevelWorld,
    capture_portal_walk: bool,
    scratch: &mut Vec<DrawRange>,
) -> (VisibleFaces, VisibilityStats) {
    let result = determine_visible_leaf_set(camera_position, view_proj, world, capture_portal_walk);

    let visible_leaves = match result.leaves {
        None => {
            let stats = VisibilityStats {
                camera_leaf: result.camera_leaf,
                total_faces: result.total_faces,
                pvs_reach: result.pvs_reach,
                drawn_faces: result.total_faces,
                path: VisibilityPath::EmptyWorldFallback,
            };
            return (VisibleFaces::DrawAll, stats);
        }
        Some(ref leaves) => leaves,
    };

    scratch.clear();
    for &leaf_idx in visible_leaves {
        let leaf = &world.leaves[leaf_idx];
        let start = leaf.face_start as usize;
        let count = leaf.face_count as usize;
        for face in world.face_meta.iter().skip(start).take(count) {
            if face.index_count > 0 {
                scratch.push(DrawRange {
                    index_offset: face.index_offset,
                    index_count: face.index_count,
                });
            }
        }
    }

    let stats = build_visibility_stats(&result, visible_leaves, world);
    (VisibleFaces::Culled(std::mem::take(scratch)), stats)
}

/// GPU-driven visibility path: produces visible cell IDs for the compute
/// culling pass. Same logic as `determine_prl_visibility` (portal traversal,
/// PVS, fallbacks), but collects cell IDs instead of per-face draw ranges.
///
/// Cell IDs equal BSP leaf indices in the current compiler. The compute
/// shader performs per-chunk AABB frustum culling, so this function only
/// needs to identify potentially-visible cells, not per-face ranges.
///
/// `scratch` is cleared and reused to avoid per-frame allocation. The caller
/// reclaims it from `VisibleCells::Culled` after the compute pass consumes it.
pub fn determine_visible_cells(
    camera_position: Vec3,
    view_proj: Mat4,
    world: &LevelWorld,
    capture_portal_walk: bool,
    scratch: &mut Vec<u32>,
) -> (VisibleCells, VisibilityStats) {
    let result = determine_visible_leaf_set(camera_position, view_proj, world, capture_portal_walk);

    let visible_leaves = match result.leaves {
        None => {
            let stats = VisibilityStats {
                camera_leaf: result.camera_leaf,
                total_faces: result.total_faces,
                pvs_reach: result.pvs_reach,
                drawn_faces: result.total_faces,
                path: VisibilityPath::EmptyWorldFallback,
            };
            return (VisibleCells::DrawAll, stats);
        }
        Some(ref leaves) => leaves,
    };

    scratch.clear();
    for &leaf_idx in visible_leaves {
        scratch.push(leaf_idx as u32);
    }

    let stats = build_visibility_stats(&result, visible_leaves, world);
    (VisibleCells::Culled(std::mem::take(scratch)), stats)
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

    // -- PRL leaf-based visibility tests --

    use crate::geometry::WorldVertex;
    use crate::material::Material;
    use crate::prl::{BspChild, FaceMeta as PrlFaceMeta, LeafData, LevelWorld, NodeData};

    fn zero_vertex() -> WorldVertex {
        WorldVertex {
            position: [0.0; 3],
            base_uv: [0.0; 2],
            normal_oct: [32768, 32768], // +Z
            tangent_packed: [65535, 32768 | 0x8000], // +X, positive bitangent
        }
    }

    fn prl_face_meta(index_offset: u32, index_count: u32) -> PrlFaceMeta {
        PrlFaceMeta {
            index_offset,
            index_count,
            leaf_index: 0,
            texture_index: None,
            texture_dimensions: (64, 64),
            texture_name: String::new(),
            material: Material::Default,
        }
    }

    fn prl_leaf(
        bounds_min: Vec3,
        bounds_max: Vec3,
        face_start: u32,
        face_count: u32,
        pvs: Vec<bool>,
        is_solid: bool,
    ) -> LeafData {
        LeafData {
            bounds_min,
            bounds_max,
            face_start,
            face_count,
            pvs,
            is_solid,
        }
    }

    /// Build a two-leaf PRL world: one BSP node splits space at X=0.
    /// Front (X >= 0) -> leaf 0, back (X < 0) -> leaf 1.
    fn two_leaf_prl_world() -> LevelWorld {
        LevelWorld {
            vertices: vec![zero_vertex(); 6],
            indices: vec![0, 1, 2, 3, 4, 5],
            face_meta: vec![prl_face_meta(0, 3), prl_face_meta(3, 3)],
            nodes: vec![NodeData {
                plane_normal: Vec3::X,
                plane_distance: 0.0,
                front: BspChild::Leaf(0),
                back: BspChild::Leaf(1),
            }],
            leaves: vec![
                prl_leaf(
                    Vec3::new(0.0, -100.0, -100.0),
                    Vec3::new(100.0, 100.0, 100.0),
                    0,
                    1,
                    vec![true, true],
                    false,
                ),
                prl_leaf(
                    Vec3::new(-100.0, -100.0, -100.0),
                    Vec3::new(0.0, 100.0, 100.0),
                    1,
                    1,
                    vec![true, true],
                    false,
                ),
            ],
            root: BspChild::Node(0),
            has_pvs: true,
            portals: vec![],
            leaf_portals: vec![vec![], vec![]],
            has_portals: false,
            texture_names: vec![],
            cell_chunk_table: None,
        }
    }

    #[test]
    fn prl_visibility_with_pvs() {
        let world = two_leaf_prl_world();
        let vp = wide_view_proj(Vec3::new(50.0, 0.0, 0.0));
        let mut scratch = Vec::new();
        let (result, stats) =
            determine_prl_visibility(Vec3::new(50.0, 0.0, 0.0), vp, &world, false, &mut scratch);
        match result {
            VisibleFaces::Culled(ranges) => {
                assert!(!ranges.is_empty(), "should have draw ranges");
            }
            VisibleFaces::DrawAll => panic!("expected Culled, got DrawAll"),
        }
        assert_eq!(stats.total_faces, 2);
    }

    #[test]
    fn prl_visibility_without_pvs_draws_all_with_frustum() {
        let mut world = two_leaf_prl_world();
        world.has_pvs = false;
        let vp = wide_view_proj(Vec3::new(50.0, 0.0, 0.0));
        let mut scratch = Vec::new();
        let (result, stats) =
            determine_prl_visibility(Vec3::new(50.0, 0.0, 0.0), vp, &world, false, &mut scratch);
        match result {
            VisibleFaces::Culled(_) => {
                // Frustum culling still applies, but PVS is skipped. The
                // no-PVS fallback reports total_faces as the PVS reach,
                // matching its "PVS admits everything" contract.
                assert!(matches!(stats.path, VisibilityPath::NoPvsFallback));
                assert_eq!(stats.pvs_reach, stats.total_faces);
            }
            VisibleFaces::DrawAll => panic!("expected Culled with frustum culling"),
        }
    }

    #[test]
    fn prl_visibility_empty_world_draws_all() {
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![],
            leaves: vec![],
            root: BspChild::Leaf(0),
            has_pvs: false,
            portals: vec![],
            leaf_portals: vec![],
            has_portals: false,
            texture_names: vec![],
            cell_chunk_table: None,
        };
        let vp = wide_view_proj(Vec3::ZERO);
        let mut scratch = Vec::new();
        let (result, stats) = determine_prl_visibility(Vec3::ZERO, vp, &world, false, &mut scratch);
        assert!(matches!(result, VisibleFaces::DrawAll));
        assert_eq!(stats.total_faces, 0);
    }

    #[test]
    fn prl_frustum_culling_reduces_draw_count() {
        let world = two_leaf_prl_world();
        let position = Vec3::new(50.0, 0.0, 0.0);
        // Looking straight down +X (away from leaf 1 at negative X).
        let view = Mat4::look_at_rh(position, position + Vec3::X, Vec3::Y);
        let proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, 1.0, 0.1, 4096.0);
        let vp = proj * view;

        let mut scratch = Vec::new();
        let (result, stats) = determine_prl_visibility(position, vp, &world, false, &mut scratch);
        match result {
            VisibleFaces::Culled(ranges) => {
                // Leaf 1 (negative X) should be frustum-culled.
                assert_eq!(
                    ranges.len(),
                    1,
                    "should only draw camera leaf's face when looking away"
                );
            }
            VisibleFaces::DrawAll => panic!("expected Culled"),
        }
        // `pvs_reach` is the pre-cull PVS lookup count (2 faces, both leaves
        // admitted by PVS). `drawn_faces` is the post-cull count. The delta
        // between them reflects what the AABB-frustum cull discarded.
        assert!(matches!(stats.path, VisibilityPath::PrlPvs));
        assert_eq!(stats.pvs_reach, 2);
        assert_eq!(stats.drawn_faces, 1);
    }

    #[test]
    fn prl_camera_leaf_always_drawn() {
        // Even if PVS says nothing visible, camera leaf is always included.
        let mut world = two_leaf_prl_world();
        // Set leaf 0's PVS to see nothing.
        world.leaves[0].pvs = vec![false, false];
        let vp = wide_view_proj(Vec3::new(50.0, 0.0, 0.0));
        let mut scratch = Vec::new();
        let (result, _stats) =
            determine_prl_visibility(Vec3::new(50.0, 0.0, 0.0), vp, &world, false, &mut scratch);
        match result {
            VisibleFaces::Culled(ranges) => {
                assert_eq!(ranges.len(), 1, "camera leaf should always be drawn");
                assert_eq!(ranges[0].index_offset, 0);
                assert_eq!(ranges[0].index_count, 3);
            }
            VisibleFaces::DrawAll => panic!("expected Culled"),
        }
    }

    #[test]
    fn prl_solid_leaf_fallback_draws_all() {
        // Camera in a solid leaf should draw all non-solid leaves.
        let world = LevelWorld {
            vertices: vec![zero_vertex(); 6],
            indices: vec![0, 1, 2, 3, 4, 5],
            face_meta: vec![prl_face_meta(0, 3), prl_face_meta(3, 3)],
            nodes: vec![NodeData {
                plane_normal: Vec3::X,
                plane_distance: 0.0,
                front: BspChild::Leaf(0), // solid
                back: BspChild::Leaf(1),  // empty
            }],
            leaves: vec![
                prl_leaf(
                    Vec3::new(0.0, -100.0, -100.0),
                    Vec3::new(100.0, 100.0, 100.0),
                    0,
                    1,
                    vec![false, false],
                    true,
                ),
                prl_leaf(
                    Vec3::new(-100.0, -100.0, -100.0),
                    Vec3::new(0.0, 100.0, 100.0),
                    1,
                    1,
                    vec![true, true],
                    false,
                ),
            ],
            root: BspChild::Node(0),
            has_pvs: true,
            portals: vec![],
            leaf_portals: vec![vec![], vec![]],
            has_portals: false,
            texture_names: vec![],
            cell_chunk_table: None,
        };

        // Camera at X=50 lands in solid leaf 0.
        let vp = wide_view_proj(Vec3::new(50.0, 0.0, 0.0));
        let mut scratch = Vec::new();
        let (result, stats) =
            determine_prl_visibility(Vec3::new(50.0, 0.0, 0.0), vp, &world, false, &mut scratch);
        match result {
            VisibleFaces::Culled(ranges) => {
                // Should draw all non-solid leaf faces (leaf 1's face).
                assert!(!ranges.is_empty(), "should draw non-solid leaf faces");
            }
            VisibleFaces::DrawAll => panic!("expected Culled with solid-leaf fallback"),
        }
        // Solid-leaf fallback has no meaningful PVS, so pvs_reach is
        // reported as total_faces (draw-all semantics).
        assert!(matches!(stats.path, VisibilityPath::SolidLeafFallback));
        assert_eq!(stats.pvs_reach, stats.total_faces);
    }

    // -- Exterior camera fallback tests --

    /// Build a world with one exterior leaf (no faces, not solid) and one
    /// interior leaf (has faces, not solid). BSP node splits at X=0:
    /// front (X >= 0) -> leaf 0 (exterior), back (X < 0) -> leaf 1 (interior).
    fn exterior_interior_world() -> LevelWorld {
        LevelWorld {
            vertices: vec![zero_vertex(); 3],
            indices: vec![0, 1, 2],
            face_meta: vec![prl_face_meta(0, 3)],
            nodes: vec![NodeData {
                plane_normal: Vec3::X,
                plane_distance: 0.0,
                front: BspChild::Leaf(0), // exterior (no faces)
                back: BspChild::Leaf(1),  // interior (has faces)
            }],
            leaves: vec![
                // Leaf 0: exterior — empty, no faces, not solid
                prl_leaf(
                    Vec3::new(0.0, -100.0, -100.0),
                    Vec3::new(100.0, 100.0, 100.0),
                    0,
                    0, // face_count == 0: the exterior signature
                    vec![false, false],
                    false,
                ),
                // Leaf 1: interior — has faces
                prl_leaf(
                    Vec3::new(-100.0, -100.0, -100.0),
                    Vec3::new(0.0, 100.0, 100.0),
                    0,
                    1,
                    vec![false, true],
                    false,
                ),
            ],
            root: BspChild::Node(0),
            has_pvs: false,
            portals: vec![],
            leaf_portals: vec![vec![], vec![]],
            has_portals: true,
            texture_names: vec![],
            cell_chunk_table: None,
        }
    }

    #[test]
    fn exterior_camera_fallback_detects_exterior_leaf() {
        let world = exterior_interior_world();
        // Camera at X=50 lands in exterior leaf 0.
        let position = Vec3::new(50.0, 0.0, 0.0);
        let vp = wide_view_proj(position);
        let mut scratch = Vec::new();
        let (result, stats) = determine_prl_visibility(position, vp, &world, false, &mut scratch);
        match result {
            VisibleFaces::Culled(ranges) => {
                assert!(!ranges.is_empty(), "should draw interior leaf faces");
            }
            VisibleFaces::DrawAll => panic!("expected Culled with exterior fallback"),
        }
        assert!(
            matches!(stats.path, VisibilityPath::ExteriorCameraFallback),
            "expected ExteriorCameraFallback, got {:?}",
            stats.path,
        );
        assert!(stats.drawn_faces > 0, "should draw at least one face");
        // Exterior fallback bypasses PVS, so pvs_reach == total_faces.
        assert_eq!(stats.pvs_reach, stats.total_faces);
    }

    #[test]
    fn exterior_camera_fallback_frustum_culls() {
        // Add a second interior leaf placed outside the view frustum.
        // BSP: node 0 splits at X=0 (front -> leaf 0 exterior, back -> node 1).
        //      node 1 splits at Z=0 (front -> leaf 1 in-frustum, back -> leaf 2 out-of-frustum).
        let world = LevelWorld {
            vertices: vec![zero_vertex(); 6],
            indices: vec![0, 1, 2, 3, 4, 5],
            face_meta: vec![prl_face_meta(0, 3), prl_face_meta(3, 3)],
            nodes: vec![
                NodeData {
                    plane_normal: Vec3::X,
                    plane_distance: 0.0,
                    front: BspChild::Leaf(0), // exterior
                    back: BspChild::Node(1),
                },
                NodeData {
                    plane_normal: Vec3::Z,
                    plane_distance: 0.0,
                    front: BspChild::Leaf(2), // interior, behind camera (+Z)
                    back: BspChild::Leaf(1),  // interior, in front of camera (-Z)
                },
            ],
            leaves: vec![
                // Leaf 0: exterior (no faces)
                prl_leaf(
                    Vec3::new(0.0, -100.0, -100.0),
                    Vec3::new(100.0, 100.0, 100.0),
                    0,
                    0,
                    vec![],
                    false,
                ),
                // Leaf 1: interior, in front of camera (negative Z, inside frustum)
                prl_leaf(
                    Vec3::new(-100.0, -100.0, -100.0),
                    Vec3::new(0.0, 100.0, 0.0),
                    0,
                    1,
                    vec![],
                    false,
                ),
                // Leaf 2: interior, behind camera (positive Z, outside frustum)
                prl_leaf(
                    Vec3::new(-100.0, -100.0, 0.0),
                    Vec3::new(0.0, 100.0, 100.0),
                    1,
                    1,
                    vec![],
                    false,
                ),
            ],
            root: BspChild::Node(0),
            has_pvs: false,
            portals: vec![],
            leaf_portals: vec![vec![], vec![], vec![]],
            has_portals: true,
            texture_names: vec![],
            cell_chunk_table: None,
        };

        // Camera at X=50, looking down -Z. Leaf 1 (-Z) is in frustum,
        // leaf 2 (+Z) is behind the camera and should be culled.
        let position = Vec3::new(50.0, 0.0, 0.0);
        let vp = wide_view_proj(position);
        let mut scratch = Vec::new();
        let (result, stats) = determine_prl_visibility(position, vp, &world, false, &mut scratch);
        match result {
            VisibleFaces::Culled(ranges) => {
                // Only leaf 1's face (index_offset=0, index_count=3) should survive.
                assert_eq!(
                    ranges.len(),
                    1,
                    "only the in-frustum leaf's face should be drawn, got {}",
                    ranges.len(),
                );
                assert_eq!(ranges[0].index_offset, 0);
                assert_eq!(ranges[0].index_count, 3);
            }
            VisibleFaces::DrawAll => panic!("expected Culled"),
        }
        assert!(matches!(stats.path, VisibilityPath::ExteriorCameraFallback));
        assert_eq!(stats.drawn_faces, 1);
    }

    #[test]
    fn interior_camera_uses_portal_path_not_exterior_fallback() {
        let world = exterior_interior_world();
        // Camera at X=-50 lands in interior leaf 1 (the leaf with faces).
        let position = Vec3::new(-50.0, 0.0, 0.0);
        let vp = wide_view_proj(position);
        let mut scratch = Vec::new();
        let (_result, stats) = determine_prl_visibility(position, vp, &world, false, &mut scratch);
        assert!(
            matches!(stats.path, VisibilityPath::PrlPortal { .. }),
            "interior camera should use PrlPortal path, got {:?}",
            stats.path,
        );
    }

    // -- Cell-based visibility tests (determine_visible_cells) --
    // These exercise the GPU-driven indirect draw path's visibility input,
    // ensuring all fallback paths produce correct cell ID lists.

    #[test]
    fn cells_empty_world_returns_draw_all() {
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![],
            leaves: vec![],
            root: BspChild::Leaf(0),
            has_pvs: false,
            portals: vec![],
            leaf_portals: vec![],
            has_portals: false,
            texture_names: vec![],
            cell_chunk_table: None,
        };
        let vp = wide_view_proj(Vec3::ZERO);
        let mut scratch = Vec::new();
        let (result, stats) = determine_visible_cells(Vec3::ZERO, vp, &world, false, &mut scratch);
        assert!(matches!(result, VisibleCells::DrawAll));
        assert!(matches!(stats.path, VisibilityPath::EmptyWorldFallback));
        assert_eq!(stats.total_faces, 0);
    }

    #[test]
    fn cells_with_pvs_returns_visible_cell_ids() {
        let world = two_leaf_prl_world();
        let vp = wide_view_proj(Vec3::new(50.0, 0.0, 0.0));
        let mut scratch = Vec::new();
        let (result, stats) =
            determine_visible_cells(Vec3::new(50.0, 0.0, 0.0), vp, &world, false, &mut scratch);
        match result {
            VisibleCells::Culled(cell_ids) => {
                assert!(!cell_ids.is_empty(), "should have visible cell IDs");
                // Both leaves are PVS-visible from each other in this test world.
                // Camera is in leaf 0 looking down -Z; leaf 1 is at negative X.
                // With a wide 90-degree FOV from X=50, leaf 1 should be visible.
                assert!(
                    cell_ids.contains(&0),
                    "camera leaf should be in visible set"
                );
            }
            VisibleCells::DrawAll => panic!("expected Culled, got DrawAll"),
        }
        assert!(matches!(stats.path, VisibilityPath::PrlPvs));
        assert_eq!(stats.total_faces, 2);
    }

    #[test]
    fn cells_solid_leaf_fallback_includes_all_non_solid_cells() {
        let world = LevelWorld {
            vertices: vec![zero_vertex(); 6],
            indices: vec![0, 1, 2, 3, 4, 5],
            face_meta: vec![prl_face_meta(0, 3), prl_face_meta(3, 3)],
            nodes: vec![NodeData {
                plane_normal: Vec3::X,
                plane_distance: 0.0,
                front: BspChild::Leaf(0), // solid
                back: BspChild::Leaf(1),  // empty
            }],
            leaves: vec![
                prl_leaf(
                    Vec3::new(0.0, -100.0, -100.0),
                    Vec3::new(100.0, 100.0, 100.0),
                    0,
                    1,
                    vec![false, false],
                    true,
                ),
                prl_leaf(
                    Vec3::new(-100.0, -100.0, -100.0),
                    Vec3::new(0.0, 100.0, 100.0),
                    1,
                    1,
                    vec![true, true],
                    false,
                ),
            ],
            root: BspChild::Node(0),
            has_pvs: true,
            portals: vec![],
            leaf_portals: vec![vec![], vec![]],
            has_portals: false,
            texture_names: vec![],
            cell_chunk_table: None,
        };

        // Camera at X=50 lands in solid leaf 0.
        let vp = wide_view_proj(Vec3::new(50.0, 0.0, 0.0));
        let mut scratch = Vec::new();
        let (result, stats) =
            determine_visible_cells(Vec3::new(50.0, 0.0, 0.0), vp, &world, false, &mut scratch);
        match result {
            VisibleCells::Culled(cell_ids) => {
                // Should include leaf 1 (the only non-solid leaf with faces).
                assert!(
                    cell_ids.contains(&1),
                    "non-solid leaf should be in visible set"
                );
                // Should NOT include leaf 0 (solid).
                assert!(
                    !cell_ids.contains(&0),
                    "solid leaf should not be in visible set"
                );
            }
            VisibleCells::DrawAll => panic!("expected Culled with solid-leaf fallback"),
        }
        assert!(matches!(stats.path, VisibilityPath::SolidLeafFallback));
        assert_eq!(stats.pvs_reach, stats.total_faces);
    }

    #[test]
    fn cells_exterior_camera_fallback_frustum_culls() {
        let world = LevelWorld {
            vertices: vec![zero_vertex(); 6],
            indices: vec![0, 1, 2, 3, 4, 5],
            face_meta: vec![prl_face_meta(0, 3), prl_face_meta(3, 3)],
            nodes: vec![
                NodeData {
                    plane_normal: Vec3::X,
                    plane_distance: 0.0,
                    front: BspChild::Leaf(0), // exterior
                    back: BspChild::Node(1),
                },
                NodeData {
                    plane_normal: Vec3::Z,
                    plane_distance: 0.0,
                    front: BspChild::Leaf(2), // interior, behind camera (+Z)
                    back: BspChild::Leaf(1),  // interior, in front of camera (-Z)
                },
            ],
            leaves: vec![
                // Leaf 0: exterior (no faces)
                prl_leaf(
                    Vec3::new(0.0, -100.0, -100.0),
                    Vec3::new(100.0, 100.0, 100.0),
                    0,
                    0,
                    vec![],
                    false,
                ),
                // Leaf 1: interior, in front of camera (-Z, inside frustum)
                prl_leaf(
                    Vec3::new(-100.0, -100.0, -100.0),
                    Vec3::new(0.0, 100.0, 0.0),
                    0,
                    1,
                    vec![],
                    false,
                ),
                // Leaf 2: interior, behind camera (+Z, outside frustum)
                prl_leaf(
                    Vec3::new(-100.0, -100.0, 0.0),
                    Vec3::new(0.0, 100.0, 100.0),
                    1,
                    1,
                    vec![],
                    false,
                ),
            ],
            root: BspChild::Node(0),
            has_pvs: false,
            portals: vec![],
            leaf_portals: vec![vec![], vec![], vec![]],
            has_portals: true,
            texture_names: vec![],
            cell_chunk_table: None,
        };

        // Camera at X=50, looking down -Z. Leaf 1 in front, leaf 2 behind.
        let position = Vec3::new(50.0, 0.0, 0.0);
        let vp = wide_view_proj(position);
        let mut scratch = Vec::new();
        let (result, stats) =
            determine_visible_cells(position, vp, &world, false, &mut scratch);
        match result {
            VisibleCells::Culled(cell_ids) => {
                // Only leaf 1 should survive frustum culling.
                assert!(
                    cell_ids.contains(&1),
                    "in-frustum leaf should be visible"
                );
                assert!(
                    !cell_ids.contains(&2),
                    "behind-camera leaf should be frustum-culled"
                );
                assert!(
                    !cell_ids.contains(&0),
                    "exterior leaf should not be in visible set"
                );
            }
            VisibleCells::DrawAll => panic!("expected Culled"),
        }
        assert!(matches!(stats.path, VisibilityPath::ExteriorCameraFallback));
    }

    #[test]
    fn cells_no_pvs_fallback_uses_frustum_culling() {
        let mut world = two_leaf_prl_world();
        world.has_pvs = false;
        let vp = wide_view_proj(Vec3::new(50.0, 0.0, 0.0));
        let mut scratch = Vec::new();
        let (result, stats) =
            determine_visible_cells(Vec3::new(50.0, 0.0, 0.0), vp, &world, false, &mut scratch);
        match result {
            VisibleCells::Culled(cell_ids) => {
                assert!(
                    !cell_ids.is_empty(),
                    "should have visible cells even without PVS"
                );
            }
            VisibleCells::DrawAll => panic!("expected Culled with frustum culling"),
        }
        assert!(matches!(stats.path, VisibilityPath::NoPvsFallback));
        assert_eq!(stats.pvs_reach, stats.total_faces);
    }

    #[test]
    fn cells_frustum_culling_reduces_visible_set() {
        let world = two_leaf_prl_world();
        let position = Vec3::new(50.0, 0.0, 0.0);
        // Looking straight down +X (away from leaf 1 at negative X).
        let view = Mat4::look_at_rh(position, position + Vec3::X, Vec3::Y);
        let proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, 1.0, 0.1, 4096.0);
        let vp = proj * view;

        let mut scratch = Vec::new();
        let (result, stats) =
            determine_visible_cells(position, vp, &world, false, &mut scratch);
        match result {
            VisibleCells::Culled(cell_ids) => {
                // Leaf 1 (negative X) should be frustum-culled.
                assert_eq!(
                    cell_ids.len(),
                    1,
                    "only camera leaf should be visible when looking away from other leaf"
                );
                assert_eq!(cell_ids[0], 0, "only leaf 0 (camera leaf) should survive");
            }
            VisibleCells::DrawAll => panic!("expected Culled"),
        }
        assert!(matches!(stats.path, VisibilityPath::PrlPvs));
        assert_eq!(stats.pvs_reach, 2);
        assert_eq!(stats.drawn_faces, 1);
    }

    #[test]
    fn cells_portal_path_produces_cell_ids() {
        let world = exterior_interior_world();
        // Camera at X=-50 lands in interior leaf 1.
        let position = Vec3::new(-50.0, 0.0, 0.0);
        let vp = wide_view_proj(position);
        let mut scratch = Vec::new();
        let (result, stats) =
            determine_visible_cells(position, vp, &world, false, &mut scratch);
        match result {
            VisibleCells::Culled(cell_ids) => {
                // Interior leaf 1 has faces, should be visible.
                assert!(
                    cell_ids.contains(&1),
                    "interior leaf should be in visible cell set"
                );
            }
            VisibleCells::DrawAll => panic!("expected Culled"),
        }
        assert!(
            matches!(stats.path, VisibilityPath::PrlPortal { .. }),
            "expected PrlPortal, got {:?}",
            stats.path,
        );
    }
}
