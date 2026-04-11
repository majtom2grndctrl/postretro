// PVS-based visibility culling with frustum culling: point-in-leaf, PVS decompression,
// frustum plane extraction, AABB-frustum test, visible face collection.
// Supports both BSP (per-leaf) and PRL (per-cluster) visibility paths.
// See: context/lib/rendering_pipeline.md

use glam::{Mat4, Vec3, Vec4};

use crate::bsp::BspWorld;
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
#[derive(Debug, Clone)]
pub struct VisibilityStats {
    /// BSP leaf the camera currently occupies.
    pub camera_leaf: u32,
    /// Total faces in the level.
    pub total_faces: u32,
    /// Faces in the camera leaf's raw PVS, ignoring frustum and portal
    /// narrowing. Constant with respect to view direction. Lets you compare
    /// "what PVS allows" against the post-narrowing counts below to isolate
    /// which culling stage is dropping a given surface.
    pub raw_pvs_faces: u32,
    /// Faces remaining after the visibility path's primary narrowing stage,
    /// before any frustum-based leaf culling. Pre-cull across all paths:
    ///
    /// - **BSP path:** PVS lookup count, counted before the AABB frustum test.
    /// - **PRL PVS path:** PVS lookup count — equal to `raw_pvs_faces` by
    ///   construction, since both walk PVS-visible leaves and count non-empty
    ///   faces.
    /// - **PRL portal path:** portal-walk reach. Differs from `raw_pvs_faces`
    ///   because the portal walk discards leaves the PVS would have admitted
    ///   but the portal chain cannot reach. Equals `frustum_faces` by
    ///   construction because portal traversal already clips against a
    ///   narrowed frustum at every hop — there is no separate AABB stage.
    ///
    /// Compare against `raw_pvs_faces` to see how much the primary narrowing
    /// stage discarded, and against `frustum_faces` to see how much the AABB
    /// frustum cull discarded.
    pub pvs_faces: u32,
    /// Faces remaining after frustum (AABB) culling on top of `pvs_faces`.
    /// Post-cull across all paths. On the PRL portal-traversal path this
    /// equals `pvs_faces` by construction because portal traversal already
    /// clips against a narrowed frustum at every hop.
    pub frustum_faces: u32,
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

/// Extract the six frustum planes from a combined view-projection matrix.
///
/// Uses the Griess-Hartmann method for a right-handed projection:
/// each plane is a combination of rows from the 4x4 matrix. The resulting
/// planes point inward (a point satisfying all six is inside the frustum).
fn extract_frustum_planes(view_proj: Mat4) -> Frustum {
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

/// Walk the BSP node tree to find which leaf contains the given point.
///
/// Returns the leaf index into `bsp_world.leaves`. At each internal node, tests the
/// point against the split plane and descends into the appropriate child.
pub fn find_camera_leaf(position: Vec3, world: &BspWorld) -> u32 {
    let mut node_idx = world.root_node;
    let mut is_leaf = false;

    loop {
        if is_leaf {
            return node_idx;
        }

        let node = &world.nodes[node_idx as usize];

        // Plane test: dot(normal, point) - dist.
        // The normal has been transformed to engine Y-up via quake_to_engine (an orthonormal
        // rotation), and the camera position is already in engine coordinates. Since the
        // transform preserves distances, the original dist value is still valid.
        let side = node.plane_normal.dot(position) - node.plane_dist;

        if side >= 0.0 {
            node_idx = node.front;
            is_leaf = node.front_is_leaf;
        } else {
            node_idx = node.back;
            is_leaf = node.back_is_leaf;
        }
    }
}

/// Decompress the PVS bitfield for a leaf using Quake's standard RLE format.
///
/// Returns a `Vec<bool>` indexed by leaf index, where `true` means the leaf is potentially
/// visible. Returns `None` if the leaf has no PVS data (visdata_offset is negative or
/// visdata is empty).
///
/// The RLE format: read bytes from `visdata[offset..]`.
/// - Non-zero byte: 8 raw visibility bits (LSB = lowest leaf index in this group).
/// - Zero byte: the next byte is the count of zero bytes to expand (run-length encoding
///   of groups of 8 invisible leaves).
///
/// Leaf 0 is always the "invalid" / out-of-bounds leaf in Quake BSP, so the bitfield
/// starts counting at leaf 1.
pub fn decompress_pvs(leaf_index: u32, world: &BspWorld) -> Option<Vec<bool>> {
    let leaf = world.leaves.get(leaf_index as usize)?;

    if leaf.visdata_offset < 0 || world.visdata.is_empty() {
        return None;
    }

    let offset = leaf.visdata_offset as usize;
    if offset >= world.visdata.len() {
        return None;
    }

    let num_leaves = world.leaves.len();
    // The bitfield covers leaves 1..num_leaves. We need ceil(num_leaves / 8) bytes
    // of decompressed data, but we index by leaf_index directly, so allocate num_leaves.
    let mut visible = vec![false; num_leaves];

    // Leaf 0 is the out-of-bounds sentinel; PVS bits start at leaf 1.
    let mut leaf_bit = 1usize;
    let data = &world.visdata[offset..];
    let mut pos = 0;

    while leaf_bit < num_leaves && pos < data.len() {
        let byte = data[pos];
        pos += 1;

        if byte == 0 {
            if pos >= data.len() {
                break;
            }
            let count = data[pos] as usize;
            pos += 1;
            leaf_bit += 8 * count;
        } else {
            for bit in 0..8 {
                if leaf_bit >= num_leaves {
                    break;
                }
                if byte & (1 << bit) != 0 {
                    visible[leaf_bit] = true;
                }
                leaf_bit += 1;
            }
        }
    }

    Some(visible)
}

/// Result of collecting visible faces, including separate PVS and frustum counts.
///
/// `ranges` are pushed into the caller-provided scratch buffer (see
/// `collect_visible_faces`); this struct only reports counts. The caller reads
/// the populated ranges from the scratch buffer it supplied.
pub(crate) struct CollectedFaces {
    /// Number of `DrawRange`s pushed into the scratch buffer this call. Equal
    /// to `frustum_faces` — ranges that survived AABB culling.
    pub frustum_face_count: u32,
    /// Number of faces visible after PVS alone (before frustum culling).
    pub pvs_face_count: u32,
}

/// Collect draw ranges for all faces belonging to visible leaves into `scratch`.
///
/// Given a visibility bitfield (from `decompress_pvs`), iterates visible leaves and
/// gathers their face draw ranges. The camera's own leaf is always included.
///
/// When a frustum is provided, each PVS-visible leaf is further tested against the
/// frustum planes. Leaves whose AABB falls entirely outside the frustum are skipped.
/// The returned `pvs_face_count` reflects the count before frustum culling is applied.
///
/// `scratch` is cleared on entry and populated in place — the caller owns the
/// backing storage so no per-frame allocation occurs in steady state. A single
/// pass over each visible leaf's `face_indices` accumulates the pre-cull
/// `pvs_face_count` and speculatively appends `DrawRange`s to `scratch`; if the
/// leaf subsequently fails the frustum test, those speculative ranges are
/// rolled back with `scratch.truncate`. The count is still accumulated for
/// culled leaves because `pvs_face_count` reflects PVS reach, not frustum
/// reach.
///
/// The steady-state zero-allocation contract is completed by the caller:
/// `App::scratch_ranges` holds the persistent backing storage, and main.rs
/// reclaims the allocation from `VisibleFaces::Culled` after `render_frame`
/// consumes it. A future change to the shape of `VisibleFaces::Culled` (e.g.,
/// reshaping to a borrowed slice) must preserve that round-trip, or introduce
/// a different mechanism that keeps the scratch capacity alive across frames.
pub fn collect_visible_faces(
    visible_leaves: &[bool],
    camera_leaf: u32,
    world: &BspWorld,
    frustum: Option<&Frustum>,
    scratch: &mut Vec<DrawRange>,
) -> CollectedFaces {
    scratch.clear();
    let mut pvs_face_count: u32 = 0;

    for (leaf_idx, leaf) in world.leaves.iter().enumerate() {
        let is_visible = visible_leaves.get(leaf_idx).copied().unwrap_or(false);
        let is_camera_leaf = leaf_idx as u32 == camera_leaf;

        if !is_visible && !is_camera_leaf {
            continue;
        }

        // Single pass: count non-zero faces for the pre-cull PVS stat and
        // speculatively push their draw ranges into `scratch`. If the leaf
        // fails the frustum cull below, rollback the speculative pushes with
        // `truncate(commit_len)` — the count still stands because the BSP path
        // reports `pvs_face_count` as the pre-cull count by design.
        let commit_len = scratch.len();
        let mut leaf_face_count = 0u32;
        for &face_idx in &leaf.face_indices {
            if let Some(face) = world.face_meta.get(face_idx as usize) {
                if face.index_count > 0 {
                    leaf_face_count += 1;
                    scratch.push(DrawRange {
                        index_offset: face.index_offset,
                        index_count: face.index_count,
                    });
                }
            }
        }
        pvs_face_count += leaf_face_count;

        if let Some(frustum) = frustum {
            if is_aabb_outside_frustum(leaf.mins, leaf.maxs, frustum) {
                scratch.truncate(commit_len);
                continue;
            }
        }
    }

    CollectedFaces {
        frustum_face_count: scratch.len() as u32,
        pvs_face_count,
    }
}

/// Perform full visibility determination for a single frame.
///
/// Pipeline: PVS narrows the visible leaf set, then frustum culling discards
/// leaves whose bounding box falls entirely outside the view frustum.
///
/// Returns `VisibleFaces::Culled` with draw ranges when PVS data is available,
/// or `VisibleFaces::DrawAll` when it is not. Always returns `VisibilityStats`
/// with per-frame diagnostic counters.
pub fn determine_visibility(
    camera_position: Vec3,
    view_proj: Mat4,
    world: &BspWorld,
    scratch: &mut Vec<DrawRange>,
) -> (VisibleFaces, VisibilityStats) {
    let total_faces = world.face_meta.len() as u32;

    if world.nodes.is_empty() || world.leaves.is_empty() {
        let stats = VisibilityStats {
            camera_leaf: 0,
            total_faces,
            raw_pvs_faces: total_faces,
            pvs_faces: total_faces,
            frustum_faces: total_faces,
        };
        return (VisibleFaces::DrawAll, stats);
    }

    let camera_leaf = find_camera_leaf(camera_position, world);
    let frustum = extract_frustum_planes(view_proj);

    match decompress_pvs(camera_leaf, world) {
        Some(visible_leaves) => {
            let collected =
                collect_visible_faces(&visible_leaves, camera_leaf, world, Some(&frustum), scratch);
            let frustum_faces = collected.frustum_face_count;
            let pvs_faces = collected.pvs_face_count;

            log::trace!(
                "[Visibility] leaf={}, pvs_faces={}, frustum_faces={}, total_faces={}",
                camera_leaf,
                pvs_faces,
                frustum_faces,
                total_faces,
            );

            let stats = VisibilityStats {
                camera_leaf,
                total_faces,
                raw_pvs_faces: pvs_faces,
                pvs_faces,
                frustum_faces,
            };
            (VisibleFaces::Culled(std::mem::take(scratch)), stats)
        }
        None => {
            log::trace!(
                "[Visibility] leaf={}, no PVS data — drawing all faces",
                camera_leaf,
            );
            let stats = VisibilityStats {
                camera_leaf,
                total_faces,
                raw_pvs_faces: total_faces,
                pvs_faces: total_faces,
                frustum_faces: total_faces,
            };
            (VisibleFaces::DrawAll, stats)
        }
    }
}

/// Count drawable faces in the camera leaf's raw PVS, ignoring frustum and
/// portal narrowing. The camera leaf itself is always included even if its
/// own bit is unset, matching the iteration pattern of the PVS path. Used as
/// the angle-independent baseline in `VisibilityStats::raw_pvs_faces` so the
/// portal-traversal path can be compared against "what PVS allows."
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
            raw_pvs_faces: total_faces,
            pvs_faces: total_faces,
            frustum_faces: total_faces,
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
            "[Visibility] Camera in solid leaf {} — drawing all leaves",
            camera_leaf_idx,
        );
        scratch.clear();
        let mut frustum_faces = 0u32;

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
                    frustum_faces += 1;
                }
            }
        }

        // Solid leaf has no meaningful PVS (camera is clipped into geometry),
        // so report total_faces as the raw PVS baseline to match the
        // "draw everything" fallback semantics of this branch.
        let stats = VisibilityStats {
            camera_leaf: camera_leaf_idx as u32,
            total_faces,
            raw_pvs_faces: total_faces,
            pvs_faces: total_faces,
            frustum_faces,
        };
        return (VisibleFaces::Culled(std::mem::take(scratch)), stats);
    }

    let raw_pvs_faces = raw_pvs_face_count(world, camera_leaf_idx);

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
        let mut pvs_faces = 0u32;

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
                    pvs_faces += 1;
                    scratch.push(DrawRange {
                        index_offset: face.index_offset,
                        index_count: face.index_count,
                    });
                }
            }
        }

        // Portal traversal already clips against a narrowed frustum at each hop,
        // so every face reached by the portal walk is also frustum-visible — no
        // separate AABB cull runs on this path, making frustum_faces equal to
        // pvs_faces by construction.
        let frustum_faces = pvs_faces;

        log::trace!(
            "[Visibility] leaf={}, raw_pvs_faces={}, portal_vis_faces={}, frustum_faces={}, total_faces={}",
            camera_leaf_idx,
            raw_pvs_faces,
            pvs_faces,
            frustum_faces,
            total_faces,
        );

        let stats = VisibilityStats {
            camera_leaf: camera_leaf_idx as u32,
            total_faces,
            raw_pvs_faces,
            pvs_faces,
            frustum_faces,
        };
        return (VisibleFaces::Culled(std::mem::take(scratch)), stats);
    }

    if !world.has_pvs {
        // No PVS data: draw all non-solid leaves, applying frustum culling only.
        scratch.clear();
        let mut frustum_faces = 0u32;

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
                    frustum_faces += 1;
                }
            }
        }

        let stats = VisibilityStats {
            camera_leaf: camera_leaf_idx as u32,
            total_faces,
            raw_pvs_faces,
            pvs_faces: total_faces,
            frustum_faces,
        };
        return (VisibleFaces::Culled(std::mem::take(scratch)), stats);
    }

    // PVS available: determine visible leaves.
    let pvs = &world.leaves[camera_leaf_idx].pvs;

    scratch.clear();
    let mut frustum_faces = 0u32;

    for (leaf_idx, leaf) in world.leaves.iter().enumerate() {
        if leaf.is_solid || leaf.face_count == 0 {
            continue;
        }

        let is_camera_leaf = leaf_idx == camera_leaf_idx;
        let is_pvs_visible = pvs.get(leaf_idx).copied().unwrap_or(false);

        if !is_pvs_visible && !is_camera_leaf {
            continue;
        }

        // Reorder: run the AABB-frustum cull *before* touching face_meta so
        // culled leaves pay nothing for face iteration. Unlike the BSP path
        // (`collect_visible_faces`), which uses a commit/rollback pattern to
        // preserve the pre-cull `pvs_face_count` semantic while still counting
        // in a single pass, this path gets the pre-cull count for free from
        // `raw_pvs_faces` (computed above via `raw_pvs_face_count`). The two
        // are definitionally equal on this path: both walk PVS-visible leaves
        // counting non-empty faces. The counter accumulated inside this loop
        // is the *post-cull* count, exposed as `frustum_faces`.
        if is_aabb_outside_frustum(leaf.bounds_min, leaf.bounds_max, &frustum) {
            continue;
        }

        // Single pass: count surviving faces and push their draw ranges.
        let start = leaf.face_start as usize;
        let count = leaf.face_count as usize;
        for face in world.face_meta.iter().skip(start).take(count) {
            if face.index_count > 0 {
                frustum_faces += 1;
                scratch.push(DrawRange {
                    index_offset: face.index_offset,
                    index_count: face.index_count,
                });
            }
        }
    }

    // `pvs_faces` on this path is the pre-cull PVS lookup count, which is
    // definitionally `raw_pvs_faces`. Reuse it rather than recomputing in a
    // second walk — the AABB cull reorder above means the loop only counts
    // post-cull faces, but we still want a pre-cull figure in the stats.
    let pvs_faces = raw_pvs_faces;

    log::trace!(
        "[Visibility] leaf={}, raw_pvs_faces={}, pvs_faces={}, frustum_faces={}, total_faces={}",
        camera_leaf_idx,
        raw_pvs_faces,
        pvs_faces,
        frustum_faces,
        total_faces,
    );

    let stats = VisibilityStats {
        camera_leaf: camera_leaf_idx as u32,
        total_faces,
        raw_pvs_faces,
        pvs_faces,
        frustum_faces,
    };
    (VisibleFaces::Culled(std::mem::take(scratch)), stats)
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bsp::{BspLeafData, BspNodeData, BspWorld, FaceMeta};

    // -- Helper: build a minimal BspWorld --

    fn empty_world() -> BspWorld {
        BspWorld {
            vertices: Vec::new(),
            indices: Vec::new(),
            face_meta: Vec::new(),
            nodes: Vec::new(),
            leaves: Vec::new(),
            visdata: Vec::new(),
            root_node: 0,
        }
    }

    /// Build a simple two-leaf BSP: one node splits space at X=0.
    /// Front (X >= 0) goes to leaf 1, back (X < 0) goes to leaf 2.
    /// Leaf 0 is the out-of-bounds sentinel.
    fn two_leaf_world() -> BspWorld {
        let nodes = vec![BspNodeData {
            plane_normal: Vec3::X,
            plane_dist: 0.0,
            front: 1, // leaf 1
            front_is_leaf: true,
            back: 2, // leaf 2
            back_is_leaf: true,
        }];

        let leaves = vec![
            // Leaf 0: out-of-bounds sentinel
            BspLeafData {
                mins: Vec3::ZERO,
                maxs: Vec3::ZERO,
                face_indices: Vec::new(),
                visdata_offset: -1,
                texture_sub_ranges: Vec::new(),
            },
            // Leaf 1: front half, contains face 0
            BspLeafData {
                mins: Vec3::new(0.0, -100.0, -100.0),
                maxs: Vec3::new(100.0, 100.0, 100.0),
                face_indices: vec![0],
                visdata_offset: 0,
                texture_sub_ranges: Vec::new(),
            },
            // Leaf 2: back half, contains face 1
            BspLeafData {
                mins: Vec3::new(-100.0, -100.0, -100.0),
                maxs: Vec3::new(0.0, 100.0, 100.0),
                face_indices: vec![1],
                visdata_offset: 1,
                texture_sub_ranges: Vec::new(),
            },
        ];

        let face_meta = vec![
            FaceMeta {
                index_offset: 0,
                index_count: 3,
                leaf_index: 1,
                texture_index: None,
                texture_dimensions: (64, 64),
                texture_name: String::new(),
                material: crate::material::Material::Default,
            },
            FaceMeta {
                index_offset: 3,
                index_count: 6,
                leaf_index: 2,
                texture_index: None,
                texture_dimensions: (64, 64),
                texture_name: String::new(),
                material: crate::material::Material::Default,
            },
        ];

        // Visdata: leaf 1 can see leaf 2 and vice versa.
        // Bit layout for 3 leaves (leaf 0 excluded from bitfield):
        // Byte at offset 0 (leaf 1's PVS): bit 0 = leaf 1, bit 1 = leaf 2 -> 0b11 = 3
        // Byte at offset 1 (leaf 2's PVS): bit 0 = leaf 1, bit 1 = leaf 2 -> 0b11 = 3
        let visdata = vec![0b0000_0011, 0b0000_0011];

        let dummy_vert = crate::bsp::TexturedVertex {
            position: [0.0; 3],
            base_uv: [0.0; 2],
            vertex_color: [1.0, 1.0, 1.0, 1.0],
        };
        BspWorld {
            vertices: vec![dummy_vert; 6],
            indices: vec![0, 1, 2, 3, 4, 5, 3, 5, 6],
            face_meta,
            nodes,
            leaves,
            visdata,
            root_node: 0,
        }
    }

    // -- Point-in-leaf tests --

    #[test]
    fn point_in_leaf_front_side() {
        let world = two_leaf_world();
        let leaf = find_camera_leaf(Vec3::new(10.0, 0.0, 0.0), &world);
        assert_eq!(leaf, 1, "point on positive X side should be in leaf 1");
    }

    #[test]
    fn point_in_leaf_back_side() {
        let world = two_leaf_world();
        let leaf = find_camera_leaf(Vec3::new(-10.0, 0.0, 0.0), &world);
        assert_eq!(leaf, 2, "point on negative X side should be in leaf 2");
    }

    #[test]
    fn point_in_leaf_on_plane_goes_front() {
        let world = two_leaf_world();
        // Exactly on the plane (dot = 0.0 >= 0.0) should go to front.
        let leaf = find_camera_leaf(Vec3::ZERO, &world);
        assert_eq!(leaf, 1, "point on plane should go to front child (leaf 1)");
    }

    #[test]
    fn point_in_leaf_deep_tree() {
        // Build a 3-level tree: root splits on X=0, front splits on Y=0.
        let nodes = vec![
            // Node 0: split on X=0
            BspNodeData {
                plane_normal: Vec3::X,
                plane_dist: 0.0,
                front: 1, // node 1
                front_is_leaf: false,
                back: 1, // leaf 1
                back_is_leaf: true,
            },
            // Node 1: split on Y=0
            BspNodeData {
                plane_normal: Vec3::Y,
                plane_dist: 0.0,
                front: 2, // leaf 2
                front_is_leaf: true,
                back: 3, // leaf 3
                back_is_leaf: true,
            },
        ];

        let leaves = vec![
            BspLeafData {
                mins: Vec3::ZERO,
                maxs: Vec3::ZERO,
                face_indices: Vec::new(),
                visdata_offset: -1,
                texture_sub_ranges: Vec::new(),
            },
            BspLeafData {
                mins: Vec3::splat(-100.0),
                maxs: Vec3::new(0.0, 100.0, 100.0),
                face_indices: Vec::new(),
                visdata_offset: -1,
                texture_sub_ranges: Vec::new(),
            },
            BspLeafData {
                mins: Vec3::new(0.0, 0.0, -100.0),
                maxs: Vec3::splat(100.0),
                face_indices: Vec::new(),
                visdata_offset: -1,
                texture_sub_ranges: Vec::new(),
            },
            BspLeafData {
                mins: Vec3::new(0.0, -100.0, -100.0),
                maxs: Vec3::new(100.0, 0.0, 100.0),
                face_indices: Vec::new(),
                visdata_offset: -1,
                texture_sub_ranges: Vec::new(),
            },
        ];

        let world = BspWorld {
            vertices: Vec::new(),
            indices: Vec::new(),
            face_meta: Vec::new(),
            nodes,
            leaves,
            visdata: Vec::new(),
            root_node: 0,
        };

        // X < 0 -> leaf 1
        assert_eq!(find_camera_leaf(Vec3::new(-5.0, 0.0, 0.0), &world), 1);
        // X > 0, Y > 0 -> leaf 2
        assert_eq!(find_camera_leaf(Vec3::new(5.0, 5.0, 0.0), &world), 2);
        // X > 0, Y < 0 -> leaf 3
        assert_eq!(find_camera_leaf(Vec3::new(5.0, -5.0, 0.0), &world), 3);
    }

    // -- PVS decompression tests --

    #[test]
    fn decompress_pvs_simple_raw_byte() {
        // 3 leaves total. Visdata byte 0b11 means leaves 1 and 2 are visible.
        let world = two_leaf_world();
        let visible = decompress_pvs(1, &world).expect("should have PVS");
        assert_eq!(visible.len(), 3);
        assert!(!visible[0], "leaf 0 (sentinel) should never be visible");
        assert!(visible[1], "leaf 1 should be visible");
        assert!(visible[2], "leaf 2 should be visible");
    }

    #[test]
    fn decompress_pvs_rle_zeros() {
        // Test RLE: 0x00, count=2 skips 16 leaves, then 0b0000_0001 marks the next leaf.
        // Leaves: 0 (sentinel), 1..16 (skipped by RLE), 17 (visible), 18..24 (not visible).
        let visdata = vec![0x00, 0x02, 0b0000_0001];

        let mut world = empty_world();
        world.leaves = (0..25)
            .map(|i| BspLeafData {
                mins: Vec3::ZERO,
                maxs: Vec3::ZERO,
                face_indices: Vec::new(),
                visdata_offset: if i == 1 { 0 } else { -1 },
                texture_sub_ranges: Vec::new(),
            })
            .collect();
        world.visdata = visdata;

        let visible = decompress_pvs(1, &world).expect("should have PVS");
        assert_eq!(visible.len(), 25);

        // Leaves 1..16 should be invisible (RLE skip).
        for i in 1..=16 {
            assert!(!visible[i], "leaf {i} should be invisible (RLE skip)");
        }
        // Leaf 17 should be visible.
        assert!(visible[17], "leaf 17 should be visible");
        // Leaves 18..24 should be invisible.
        for i in 18..25 {
            assert!(!visible[i], "leaf {i} should be invisible");
        }
    }

    #[test]
    fn decompress_pvs_negative_offset_returns_none() {
        let mut world = empty_world();
        world.leaves.push(BspLeafData {
            mins: Vec3::ZERO,
            maxs: Vec3::ZERO,
            face_indices: Vec::new(),
            visdata_offset: -1,
            texture_sub_ranges: Vec::new(),
        });
        assert!(decompress_pvs(0, &world).is_none());
    }

    #[test]
    fn decompress_pvs_empty_visdata_returns_none() {
        let mut world = empty_world();
        world.leaves.push(BspLeafData {
            mins: Vec3::ZERO,
            maxs: Vec3::ZERO,
            face_indices: Vec::new(),
            visdata_offset: 0,
            texture_sub_ranges: Vec::new(),
        });
        // visdata is empty
        assert!(decompress_pvs(0, &world).is_none());
    }

    #[test]
    fn decompress_pvs_out_of_bounds_leaf_returns_none() {
        let world = empty_world();
        assert!(decompress_pvs(999, &world).is_none());
    }

    #[test]
    fn decompress_pvs_matches_qbsp_reference() {
        // Use the same test data as qbsp's own test suite.
        // TEST_VISDATA: [0b1010_0111, 0, 5, 0b0000_0001, 0b0001_0000, 0, 12, 0b1000_0000]
        // Expected visible leaves: 1, 2, 3, 6, 8, 49, 61, 168
        let visdata = vec![
            0b1010_0111,
            0,
            5,
            0b0000_0001,
            0b0001_0000,
            0,
            12,
            0b1000_0000,
        ];

        let mut world = empty_world();
        world.leaves = (0..256)
            .map(|i| BspLeafData {
                mins: Vec3::ZERO,
                maxs: Vec3::ZERO,
                face_indices: Vec::new(),
                visdata_offset: if i == 1 { 0 } else { -1 },
                texture_sub_ranges: Vec::new(),
            })
            .collect();
        world.visdata = visdata;

        let visible = decompress_pvs(1, &world).expect("should have PVS");

        let visible_indices: Vec<usize> = visible
            .iter()
            .enumerate()
            .filter(|(_, v)| **v)
            .map(|(i, _)| i)
            .collect();

        assert_eq!(
            visible_indices,
            vec![1, 2, 3, 6, 8, 49, 61, 168],
            "should match qbsp reference test data"
        );
    }

    // -- Visible face collection tests --

    #[test]
    fn collect_faces_from_visible_leaves() {
        let world = two_leaf_world();
        // Both leaves visible.
        let visible = vec![false, true, true];
        let mut scratch = Vec::new();
        let collected = collect_visible_faces(&visible, 1, &world, None, &mut scratch);

        assert_eq!(scratch.len(), 2);
        assert_eq!(
            scratch[0],
            DrawRange {
                index_offset: 0,
                index_count: 3
            }
        );
        assert_eq!(
            scratch[1],
            DrawRange {
                index_offset: 3,
                index_count: 6
            }
        );
        assert_eq!(collected.pvs_face_count, 2);
        assert_eq!(collected.frustum_face_count, 2);
    }

    #[test]
    fn collect_faces_only_camera_leaf_when_others_invisible() {
        let world = two_leaf_world();
        // Only leaf 1 is visible (PVS says nothing else visible).
        let visible = vec![false, true, false];
        let mut scratch = Vec::new();
        let collected = collect_visible_faces(&visible, 1, &world, None, &mut scratch);

        assert_eq!(scratch.len(), 1);
        assert_eq!(
            scratch[0],
            DrawRange {
                index_offset: 0,
                index_count: 3
            }
        );
        assert_eq!(collected.pvs_face_count, 1);
    }

    #[test]
    fn collect_faces_camera_leaf_always_included() {
        let world = two_leaf_world();
        // PVS says nothing visible at all — but camera leaf is always included.
        let visible = vec![false, false, false];
        let mut scratch = Vec::new();
        let collected = collect_visible_faces(&visible, 1, &world, None, &mut scratch);

        assert_eq!(scratch.len(), 1, "camera leaf should always be included");
        assert_eq!(
            scratch[0],
            DrawRange {
                index_offset: 0,
                index_count: 3
            }
        );
        assert_eq!(collected.pvs_face_count, 1);
    }

    #[test]
    fn collect_faces_empty_world() {
        let world = empty_world();
        let visible: Vec<bool> = Vec::new();
        let mut scratch = Vec::new();
        let collected = collect_visible_faces(&visible, 0, &world, None, &mut scratch);
        assert!(scratch.is_empty());
        assert_eq!(collected.pvs_face_count, 0);
    }

    #[test]
    fn collect_faces_clears_scratch_on_entry() {
        // Pre-populate scratch with stale data to verify it is cleared.
        let world = two_leaf_world();
        let visible = vec![false, true, false];
        let mut scratch = vec![
            DrawRange {
                index_offset: 999,
                index_count: 999,
            };
            8
        ];
        let collected = collect_visible_faces(&visible, 1, &world, None, &mut scratch);
        assert_eq!(scratch.len(), 1);
        assert_eq!(
            scratch[0],
            DrawRange {
                index_offset: 0,
                index_count: 3
            }
        );
        assert_eq!(collected.pvs_face_count, 1);
    }

    #[test]
    fn collect_faces_rollback_preserves_pvs_count_for_culled_leaves() {
        // A leaf that PVS says is visible but is outside the frustum: the
        // BSP path must still report its face count in `pvs_face_count`
        // (pre-cull semantics) while its ranges are rolled back from scratch.
        let world = two_leaf_world();
        let visible = vec![false, true, true];
        // Camera in leaf 1, looking +X (away from leaf 2 which is at -X).
        let position = Vec3::new(50.0, 0.0, 0.0);
        let view = Mat4::look_at_rh(position, position + Vec3::X, Vec3::Y);
        let proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, 1.0, 0.1, 4096.0);
        let frustum = extract_frustum_planes(proj * view);

        let mut scratch = Vec::new();
        let collected = collect_visible_faces(&visible, 1, &world, Some(&frustum), &mut scratch);

        // Leaf 2 is frustum-culled: its range is rolled back, but its face
        // still counts toward pvs_face_count (pre-cull semantics).
        assert_eq!(scratch.len(), 1, "only leaf 1's range survives");
        assert_eq!(collected.pvs_face_count, 2);
        assert_eq!(collected.frustum_face_count, 1);
    }

    // -- determine_visibility integration tests --

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
    fn determine_visibility_with_pvs() {
        let world = two_leaf_world();
        let vp = wide_view_proj(Vec3::new(10.0, 0.0, 0.0));
        let mut scratch = Vec::new();
        let (result, stats) =
            determine_visibility(Vec3::new(10.0, 0.0, 0.0), vp, &world, &mut scratch);
        match result {
            VisibleFaces::Culled(ranges) => {
                assert!(!ranges.is_empty(), "should have draw ranges");
            }
            VisibleFaces::DrawAll => panic!("expected Culled, got DrawAll"),
        }
        assert_eq!(stats.total_faces, 2);
        assert_eq!(stats.camera_leaf, 1);
        assert!(stats.pvs_faces > 0);
        assert!(stats.frustum_faces <= stats.pvs_faces);
    }

    #[test]
    fn determine_visibility_without_pvs_draws_all() {
        let mut world = two_leaf_world();
        world.visdata.clear();
        let vp = wide_view_proj(Vec3::new(10.0, 0.0, 0.0));
        let mut scratch = Vec::new();
        let (result, stats) =
            determine_visibility(Vec3::new(10.0, 0.0, 0.0), vp, &world, &mut scratch);
        assert!(
            matches!(result, VisibleFaces::DrawAll),
            "should draw all when visdata is empty"
        );
        // Without PVS, stats report total for both pvs and frustum.
        assert_eq!(stats.total_faces, 2);
        assert_eq!(stats.pvs_faces, 2);
        assert_eq!(stats.frustum_faces, 2);
    }

    #[test]
    fn determine_visibility_empty_world_draws_all() {
        let world = empty_world();
        let vp = wide_view_proj(Vec3::ZERO);
        let mut scratch = Vec::new();
        let (result, stats) = determine_visibility(Vec3::ZERO, vp, &world, &mut scratch);
        assert!(matches!(result, VisibleFaces::DrawAll));
        assert_eq!(stats.total_faces, 0);
        assert_eq!(stats.camera_leaf, 0);
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

    // -- Frustum culling + PVS integration tests --

    #[test]
    fn frustum_culling_reduces_draw_count_for_behind_leaves() {
        let world = two_leaf_world();
        // Camera in leaf 1 (positive X), looking down -Z.
        // Leaf 2 is at negative X — its AABB spans (-100,-100,-100) to (0,100,100).
        // The camera at (50,0,0) looking -Z: leaf 2's box extends from x=-100 to x=0,
        // which is to the left. With a 90-degree FOV the half-angle is ~45 degrees,
        // so some of leaf 2 may be in view. Let's use a narrow FOV camera pointed
        // away from leaf 2 to guarantee culling.
        let position = Vec3::new(50.0, 0.0, 0.0);
        // Looking straight down +X (away from leaf 2).
        let view = Mat4::look_at_rh(position, position + Vec3::X, Vec3::Y);
        let proj = Mat4::perspective_rh(
            std::f32::consts::FRAC_PI_4, // 45-degree narrow FOV
            1.0,
            0.1,
            4096.0,
        );
        let vp = proj * view;

        let mut scratch = Vec::new();
        let (result, stats) = determine_visibility(position, vp, &world, &mut scratch);
        match result {
            VisibleFaces::Culled(ranges) => {
                // Leaf 2 should be culled (it's behind/to the side of the camera).
                // Only leaf 1's face should remain.
                assert_eq!(
                    ranges.len(),
                    1,
                    "should only draw camera leaf's face when looking away from leaf 2"
                );
                assert_eq!(
                    ranges[0],
                    DrawRange {
                        index_offset: 0,
                        index_count: 3
                    }
                );
            }
            VisibleFaces::DrawAll => panic!("expected Culled, got DrawAll"),
        }
        // PVS sees both leaves (2 faces), frustum culls leaf 2 (1 face remains).
        assert_eq!(stats.pvs_faces, 2);
        assert_eq!(stats.frustum_faces, 1);
    }

    #[test]
    fn frustum_culling_keeps_leaves_in_view() {
        let world = two_leaf_world();
        // Camera in leaf 1 (positive X), looking toward leaf 2 (negative X).
        let position = Vec3::new(50.0, 0.0, 0.0);
        let view = Mat4::look_at_rh(position, position + Vec3::NEG_X, Vec3::Y);
        let proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 16.0 / 9.0, 0.1, 4096.0);
        let vp = proj * view;

        let mut scratch = Vec::new();
        let (result, stats) = determine_visibility(position, vp, &world, &mut scratch);
        match result {
            VisibleFaces::Culled(ranges) => {
                // Both leaves should be visible — leaf 2 is in front of the camera.
                assert_eq!(
                    ranges.len(),
                    2,
                    "should draw both leaves when looking toward leaf 2"
                );
            }
            VisibleFaces::DrawAll => panic!("expected Culled, got DrawAll"),
        }
        // Both PVS and frustum should see all 2 faces.
        assert_eq!(stats.pvs_faces, 2);
        assert_eq!(stats.frustum_faces, 2);
    }

    // -- VisibilityStats tests --

    #[test]
    fn stats_reflect_pvs_vs_frustum_difference() {
        // When frustum culls some PVS-visible faces, pvs_faces > frustum_faces.
        let world = two_leaf_world();
        let position = Vec3::new(50.0, 0.0, 0.0);
        // Narrow FOV looking away from leaf 2 to force frustum culling.
        let view = Mat4::look_at_rh(position, position + Vec3::X, Vec3::Y);
        let proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, 1.0, 0.1, 4096.0);
        let vp = proj * view;

        let mut scratch = Vec::new();
        let (_, stats) = determine_visibility(position, vp, &world, &mut scratch);
        assert_eq!(stats.total_faces, 2);
        assert_eq!(stats.camera_leaf, 1);
        assert!(
            stats.pvs_faces > stats.frustum_faces,
            "frustum should cull some PVS-visible faces: pvs={} frustum={}",
            stats.pvs_faces,
            stats.frustum_faces,
        );
    }

    // -- PRL leaf-based visibility tests --

    use crate::bsp::TexturedVertex;
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
                // Frustum culling still applies, but PVS is skipped.
                assert_eq!(stats.pvs_faces, stats.total_faces);
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
        // `pvs_faces` is the pre-cull PVS lookup count (== raw_pvs_faces on
        // this path). `frustum_faces` is the post-cull count. The delta
        // between them reflects what the AABB-frustum cull discarded.
        assert_eq!(stats.raw_pvs_faces, 2);
        assert_eq!(stats.pvs_faces, 2);
        assert_eq!(stats.frustum_faces, 1);
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
        // PVS stats should show all faces as pvs_faces (solid fallback = draw all).
        assert_eq!(stats.pvs_faces, stats.total_faces);
    }
}
