// PVS-based visibility culling with frustum culling: frustum plane extraction,
// AABB-frustum test, PRL leaf-based visibility determination.
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
    /// Fallback: no PVS data in the level file. All non-solid leaves are
    /// submitted, subject to AABB frustum culling.
    NoPvsFallback,
    /// Fallback: world has no leaves to cull against. DrawAll with every
    /// face in the level submitted.
    EmptyWorldFallback,
    /// Fallback: camera position lies inside solid geometry (clipped
    /// into a wall). All non-solid leaves are drawn, subject to AABB
    /// frustum culling.
    SolidLeafFallback,
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
    let total_faces = world.face_meta.len() as u32;

    if world.leaves.is_empty() {
        let stats = VisibilityStats {
            camera_leaf: 0,
            total_faces,
            pvs_reach: total_faces,
            drawn_faces: total_faces,
            path: VisibilityPath::EmptyWorldFallback,
        };
        return (VisibleFaces::DrawAll, stats);
    }

    let camera_leaf_idx = world.find_leaf(camera_position);
    let frustum = extract_frustum_planes(view_proj);

    // Solid leaf fallback: draw all leaves.
    let in_solid = world
        .leaves
        .get(camera_leaf_idx)
        .is_some_and(|l| l.is_solid);

    if in_solid {
        log::warn!(
            "[Visibility] path=SolidLeafFallback camera in solid leaf {} — drawing all leaves",
            camera_leaf_idx,
        );
        scratch.clear();
        let mut drawn_faces = 0u32;

        for leaf in &world.leaves {
            if leaf.is_solid || leaf.face_count == 0 {
                continue;
            }
            if is_aabb_outside_frustum(leaf.bounds_min, leaf.bounds_max, &frustum) {
                continue;
            }
            let start = leaf.face_start as usize;
            let count = leaf.face_count as usize;
            for face in world.face_meta.iter().skip(start).take(count) {
                if face.index_count > 0 {
                    scratch.push(DrawRange {
                        index_offset: face.index_offset,
                        index_count: face.index_count,
                    });
                    drawn_faces += 1;
                }
            }
        }

        // Solid leaf has no meaningful PVS (camera is clipped into geometry),
        // so report total_faces as the PVS baseline to match the
        // "draw everything" fallback semantics of this branch.
        let stats = VisibilityStats {
            camera_leaf: camera_leaf_idx as u32,
            total_faces,
            pvs_reach: total_faces,
            drawn_faces,
            path: VisibilityPath::SolidLeafFallback,
        };
        return (VisibleFaces::Culled(std::mem::take(scratch)), stats);
    }

    let pvs_reach = raw_pvs_face_count(world, camera_leaf_idx);

    if world.has_portals {
        // Runtime portal traversal. Polygon-vs-frustum clipping at each hop
        // keeps every narrowed frustum a strict subset of the camera frustum,
        // so the reachability bitset is also the final visibility set — no
        // per-leaf AABB cull needed on this path. See
        // `context/lib/build_pipeline.md` §Runtime visibility.
        let portal_visible = portal_vis::portal_traverse(
            camera_position,
            camera_leaf_idx,
            &frustum,
            world,
            capture_portal_walk,
        );

        scratch.clear();
        let mut walk_reach = 0u32;

        for (leaf_idx, leaf) in world.leaves.iter().enumerate() {
            if leaf.is_solid || leaf.face_count == 0 {
                continue;
            }

            let is_visible = portal_visible.get(leaf_idx).copied().unwrap_or(false);
            if !is_visible {
                continue;
            }

            // Single pass: count non-zero faces and push their draw ranges.
            // Portal traversal has no separate AABB cull (narrowed frustums
            // already clip at each hop), so there is no rollback path.
            let start = leaf.face_start as usize;
            let count = leaf.face_count as usize;
            for face in world.face_meta.iter().skip(start).take(count) {
                if face.index_count > 0 {
                    walk_reach += 1;
                    scratch.push(DrawRange {
                        index_offset: face.index_offset,
                        index_count: face.index_count,
                    });
                }
            }
        }

        // Portal traversal already clips against a narrowed frustum at each
        // hop, so every face reached by the portal walk is also frustum-
        // visible — no separate AABB cull runs on this path. `drawn_faces`
        // equals `walk_reach` by construction.
        let drawn_faces = walk_reach;

        log::trace!(
            "[Visibility] path=PrlPortal leaf={}, pvs_reach={}, walk_reach={}, drawn_faces={}, total_faces={}",
            camera_leaf_idx,
            pvs_reach,
            walk_reach,
            drawn_faces,
            total_faces,
        );

        let stats = VisibilityStats {
            camera_leaf: camera_leaf_idx as u32,
            total_faces,
            pvs_reach,
            drawn_faces,
            path: VisibilityPath::PrlPortal { walk_reach },
        };
        return (VisibleFaces::Culled(std::mem::take(scratch)), stats);
    }

    if !world.has_pvs {
        // No PVS data: draw all non-solid leaves, applying frustum culling only.
        scratch.clear();
        let mut drawn_faces = 0u32;

        for leaf in &world.leaves {
            if leaf.is_solid || leaf.face_count == 0 {
                continue;
            }
            if is_aabb_outside_frustum(leaf.bounds_min, leaf.bounds_max, &frustum) {
                continue;
            }

            let start = leaf.face_start as usize;
            let count = leaf.face_count as usize;
            for face in world.face_meta.iter().skip(start).take(count) {
                if face.index_count > 0 {
                    scratch.push(DrawRange {
                        index_offset: face.index_offset,
                        index_count: face.index_count,
                    });
                    drawn_faces += 1;
                }
            }
        }

        // PVS wasn't consulted on this branch, so report total_faces as the
        // baseline — pvs_reach's contract is "what the PVS admits," and
        // "no PVS" admits everything.
        let stats = VisibilityStats {
            camera_leaf: camera_leaf_idx as u32,
            total_faces,
            pvs_reach: total_faces,
            drawn_faces,
            path: VisibilityPath::NoPvsFallback,
        };
        return (VisibleFaces::Culled(std::mem::take(scratch)), stats);
    }

    // PVS available: determine visible leaves.
    let pvs = &world.leaves[camera_leaf_idx].pvs;

    scratch.clear();
    let mut drawn_faces = 0u32;

    for (leaf_idx, leaf) in world.leaves.iter().enumerate() {
        if leaf.is_solid || leaf.face_count == 0 {
            continue;
        }

        let is_camera_leaf = leaf_idx == camera_leaf_idx;
        let is_pvs_visible = pvs.get(leaf_idx).copied().unwrap_or(false);

        if !is_pvs_visible && !is_camera_leaf {
            continue;
        }

        // Run the AABB-frustum cull *before* touching face_meta so culled
        // leaves pay nothing for face iteration. The pre-cull count comes
        // for free from `pvs_reach` (computed above via
        // `raw_pvs_face_count`): both walk PVS-visible leaves counting
        // non-empty faces. The counter accumulated inside this loop is the
        // *post-cull* count, exposed as `drawn_faces`.
        if is_aabb_outside_frustum(leaf.bounds_min, leaf.bounds_max, &frustum) {
            continue;
        }

        // Single pass: count surviving faces and push their draw ranges.
        let start = leaf.face_start as usize;
        let count = leaf.face_count as usize;
        for face in world.face_meta.iter().skip(start).take(count) {
            if face.index_count > 0 {
                drawn_faces += 1;
                scratch.push(DrawRange {
                    index_offset: face.index_offset,
                    index_count: face.index_count,
                });
            }
        }
    }

    log::trace!(
        "[Visibility] path=PrlPvs leaf={}, pvs_reach={}, drawn_faces={}, total_faces={}",
        camera_leaf_idx,
        pvs_reach,
        drawn_faces,
        total_faces,
    );

    let stats = VisibilityStats {
        camera_leaf: camera_leaf_idx as u32,
        total_faces,
        pvs_reach,
        drawn_faces,
        path: VisibilityPath::PrlPvs,
    };
    (VisibleFaces::Culled(std::mem::take(scratch)), stats)
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

    // -- Frustum plane extraction tests --

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

    use crate::geometry::TexturedVertex;
    use crate::material::Material;
    use crate::prl::{BspChild, FaceMeta as PrlFaceMeta, LeafData, LevelWorld, NodeData};

    fn zero_vertex() -> TexturedVertex {
        TexturedVertex {
            position: [0.0; 3],
            base_uv: [0.0; 2],
            vertex_color: [1.0, 1.0, 1.0, 1.0],
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
            texture_sub_ranges: Vec::new(),
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
}
