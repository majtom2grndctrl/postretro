// CPU mirror oracle for the candidate-cull equivalence proof (test-only).
//
// Models a synthetic world (BVH leaves + per-cell drawability + a camera
// frustum) and computes, for BOTH camera-cull paths, the byte-for-byte runtime
// observables the GPU would produce: submitted leaf indices, the renderer
// `bucket_ranges` over the GLOBAL indirect slots, and the per-leaf indirect
// commands. The two paths must agree on submitted leaves; the global
// `bucket_ranges` is identical by construction (it depends only on the sorted
// leaf array, never on which leaves submit).
//
// Tree-walk oracle  : every drawable leaf whose `cell_id` is in
//                     `VisibleCells::Culled`, after the frustum predicate —
//                     mirrors `bvh_cull.wgsl::cull_main`.
// Candidate oracle  : `gather_candidate_leaves` CSR expansion, after the SAME
//                     frustum predicate — mirrors `candidate_cull.wgsl`.
//
// This is the GPU-free half of development_guide.md §4.1: it asserts the data
// contract the thin dispatch layer relies on, with no wgpu in sight.

#![cfg(test)]

use glam::Mat4;

use std::collections::HashSet;

use crate::candidate_cull::{GatherStatus, gather_candidate_leaves};
use crate::visibility::VisibleCells;
use postretro_level_loader::CellDrawIndex;
use postretro_render_data::cone_frustum::extract_frustum_planes_for_gpu;
use postretro_render_data::geometry::{BucketRange, BvhLeaf, BvhTree};

use postretro_level_format::cell_draw_index::{CellDrawIndexSection, Span};

/// Per-leaf cull status, matching the WGSL `cull_status` encoding:
/// `0` = non-candidate / visible-cell reject (left cleared),
/// `1` = frustum reject, `2` = submitted.
pub(crate) const STATUS_NON_CANDIDATE: u32 = 0;
pub(crate) const STATUS_FRUSTUM_REJECT: u32 = 1;
pub(crate) const STATUS_SUBMITTED: u32 = 2;

/// A `DrawIndexedIndirect` command as the cull shaders write it, in the SAME
/// field order as `bvh_cull.wgsl::DrawIndexedIndirect`. Mirrors the 5-field GPU
/// struct so a per-leaf slot can be compared field-by-field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct IndirectCommand {
    pub index_count: u32,
    pub instance_count: u32,
    pub first_index: u32,
    pub base_vertex: i32,
    pub first_instance: u32,
}

/// One global indirect/status slot after a cull pass. `cull_status` carries the
/// overlay status (`0`/`1`/`2`); `command` is only meaningful (compared in full)
/// when `cull_status == STATUS_SUBMITTED`. For rejected and non-candidate slots
/// the normalized contract is `command.index_count == 0` plus the expected
/// status — other command fields are stale and must be ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LeafSlot {
    pub command: IndirectCommand,
    pub cull_status: u32,
}

/// The runtime observables a single camera-cull pass produces, for one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CullMirror {
    /// Global leaf indices that submitted (`cull_status == 2`), in leaf-index
    /// order. The two paths must produce identical sets *and* order.
    pub submitted: Vec<u32>,
    /// One slot per global leaf, indexed by leaf position. Cleared slots
    /// (`index_count == 0`, `cull_status == 0`) model the CPU `clear_buffer`.
    pub slots: Vec<LeafSlot>,
    /// Renderer `bucket_ranges` over the GLOBAL indirect slots. Identical for
    /// both paths — it never depends on which leaves submit.
    pub bucket_ranges: Vec<BucketRange>,
}

impl CullMirror {
    /// Assert this mirror matches `other` under the normalized comparison
    /// semantics from the AC. The draw path consumes only submitted slots, so
    /// equivalence is defined on those plus the zeroed `index_count` of the
    /// rest:
    ///   * submitted leaf sets/order are identical,
    ///   * `bucket_ranges` are identical,
    ///   * for submitted leaves, ALL five indirect fields AND `cull_status == 2`
    ///     match,
    ///   * for every other slot, both paths have `index_count == 0`. Their
    ///     `cull_status` (`0` vs `1`) is NOT cross-compared: a leaf in a hidden
    ///     cell that is also frustum-rejected legitimately reads status `0` on
    ///     the candidate path (never gathered) and status `1` on the tree walk
    ///     (visited, frustum-tested first). That is an overlay-only cosmetic
    ///     difference — neither submits, so the drawn output is byte-identical.
    ///     The per-path status contract (`1` for a candidate frustum reject,
    ///     `0` for a non-candidate) is asserted directly on each mirror by the
    ///     representative-case tests, not cross-path here.
    pub fn assert_matches(&self, other: &CullMirror) {
        assert_eq!(
            self.submitted, other.submitted,
            "submitted leaf sets differ between paths"
        );
        assert_eq!(
            self.bucket_ranges, other.bucket_ranges,
            "global bucket_ranges differ between paths"
        );
        assert_eq!(
            self.slots.len(),
            other.slots.len(),
            "slot counts differ between paths"
        );
        for (leaf_idx, (a, b)) in self.slots.iter().zip(&other.slots).enumerate() {
            let a_submitted = a.cull_status == STATUS_SUBMITTED;
            let b_submitted = b.cull_status == STATUS_SUBMITTED;
            assert_eq!(
                a_submitted, b_submitted,
                "leaf {leaf_idx}: submitted disagreement (self status {}, other status {})",
                a.cull_status, b.cull_status
            );
            if a_submitted {
                // Submitted: every indirect field is load-bearing, status == 2.
                assert_eq!(
                    a.command, b.command,
                    "leaf {leaf_idx}: submitted indirect command differs"
                );
            } else {
                // Non-submitted: only the zeroed index_count is contractual.
                assert_eq!(
                    a.command.index_count, 0,
                    "leaf {leaf_idx}: non-submitted slot must have index_count == 0 (self)"
                );
                assert_eq!(
                    b.command.index_count, 0,
                    "leaf {leaf_idx}: non-submitted slot must have index_count == 0 (other)"
                );
            }
        }
    }
}

/// Compact synthetic world for the equivalence proof. `leaves` is the global
/// BVH leaf array (sorted by `material_bucket_id`, as the compiler emits it);
/// `cell_drawable[c]` is true iff cell `c` is `!is_solid && face_count > 0`.
/// `index` is the baked CSR (built to match `cell_drawable` and the leaves).
pub(crate) struct SyntheticWorld {
    pub leaves: Vec<BvhLeaf>,
    pub cell_drawable: Vec<bool>,
    pub index: CellDrawIndex,
}

impl SyntheticWorld {
    /// Build a mirror world from a loaded `LevelWorld` and its (already
    /// cross-validated) `CellDrawIndex`. Reuses the LOADED CSR — not a rebuild —
    /// so the heavy stress-map probe compares the candidate path against the
    /// exact baked index the runtime would consume. Per-cell drawability comes
    /// from the explicit runtime cell records, the same predicate the gather
    /// and the tree walk apply.
    #[cfg(test)]
    pub(crate) fn from_level_world(
        world: &postretro_level_loader::LevelWorld,
        index: CellDrawIndex,
    ) -> Self {
        let cell_drawable: Vec<bool> = world.cells.iter().map(|cell| cell.is_drawable).collect();
        SyntheticWorld {
            leaves: world.bvh.leaves.clone(),
            cell_drawable,
            index,
        }
    }

    fn bucket_ranges(&self) -> Vec<BucketRange> {
        BvhTree {
            nodes: Vec::new(),
            leaves: self.leaves.clone(),
            root_node_index: 0,
        }
        .derive_bucket_ranges()
    }

    /// A leaf draws iff `index_count > 0` and its cell is drawable — the same
    /// predicate the compiler bakes the CSR against.
    fn leaf_drawable(&self, leaf_idx: usize) -> bool {
        let leaf = &self.leaves[leaf_idx];
        leaf.index_count > 0
            && self
                .cell_drawable
                .get(leaf.cell_id as usize)
                .copied()
                .unwrap_or(false)
    }
}

/// The submitted indirect command for a leaf that passed the frustum predicate,
/// matching the five fields both shaders write on the submit branch.
fn submit_command(leaf: &BvhLeaf) -> IndirectCommand {
    IndirectCommand {
        index_count: leaf.index_count,
        instance_count: 1,
        first_index: leaf.index_offset,
        base_vertex: 0,
        first_instance: 0,
    }
}

/// Whether a leaf's AABB survives the frustum, mirroring
/// `is_aabb_outside_frustum` in both shaders (p-vertex test, inside-sign
/// `dot(n, p) + d >= 0`). `planes` come from `extract_frustum_planes_for_gpu`,
/// the exact CPU source the GPU uniform is serialized from.
fn passes_frustum(leaf: &BvhLeaf, planes: &[[f32; 4]; 6]) -> bool {
    for plane in planes {
        let n = glam::Vec3::new(plane[0], plane[1], plane[2]);
        let d = plane[3];
        let p = glam::Vec3::new(
            if n.x >= 0.0 {
                leaf.aabb_max[0]
            } else {
                leaf.aabb_min[0]
            },
            if n.y >= 0.0 {
                leaf.aabb_max[1]
            } else {
                leaf.aabb_min[1]
            },
            if n.z >= 0.0 {
                leaf.aabb_max[2]
            } else {
                leaf.aabb_min[2]
            },
        );
        if n.dot(p) + d < 0.0 {
            return false;
        }
    }
    true
}

/// Tree-walk oracle: mirrors `bvh_cull.wgsl::cull_main` over the global leaf
/// array. Every leaf is visited (whole-BVH walk). A leaf submits iff it passes
/// the frustum AND its cell is in `VisibleCells::Culled` AND it is drawable.
/// Per-leaf status: frustum reject → 1, visible-cell reject → 0, submit → 2.
/// Non-drawable leaves are never enumerated into the candidate CSR, so to keep
/// the two paths comparable the tree-walk oracle leaves their slots cleared too
/// (a non-drawable leaf has `index_count == 0`, so it can never submit).
/// The oracle intentionally deviates from the real GPU shader for non-drawable
/// leaves: it leaves those slots cleared (status 0) rather than possibly writing
/// status 1 as the GPU might for non-drawable BVH nodes it visits. This is safe
/// because both paths agree `index_count == 0` for those slots, and
/// `assert_matches` cross-compares only `index_count` — not the 0-vs-1 status —
/// for non-submitted slots.
pub(crate) fn tree_walk_mirror(
    world: &SyntheticWorld,
    visible: &VisibleCells,
    view_proj: &Mat4,
) -> CullMirror {
    let planes = extract_frustum_planes_for_gpu(view_proj);
    let mut slots = vec![
        LeafSlot {
            command: IndirectCommand::default(),
            cull_status: STATUS_NON_CANDIDATE,
        };
        world.leaves.len()
    ];
    let mut submitted = Vec::new();

    let cell_visible = |cell_id: u32| -> bool {
        match visible {
            VisibleCells::DrawAll => true,
            VisibleCells::Culled(cells) => cells.contains(&cell_id),
        }
    };

    for (leaf_idx, leaf) in world.leaves.iter().enumerate() {
        if !world.leaf_drawable(leaf_idx) {
            // Cleared slot: a non-drawable leaf is never submitted by either
            // path. Leave status 0 / index_count 0.
            continue;
        }
        if !passes_frustum(leaf, &planes) {
            slots[leaf_idx].cull_status = STATUS_FRUSTUM_REJECT;
        } else if !cell_visible(leaf.cell_id) {
            slots[leaf_idx].cull_status = STATUS_NON_CANDIDATE;
        } else {
            slots[leaf_idx].command = submit_command(leaf);
            slots[leaf_idx].cull_status = STATUS_SUBMITTED;
            submitted.push(leaf_idx as u32);
        }
    }

    CullMirror {
        submitted,
        slots,
        bucket_ranges: world.bucket_ranges(),
    }
}

/// Candidate-path oracle: mirrors `candidate_cull.wgsl` driven by
/// `gather_candidate_leaves`. Buffers start cleared; only gathered candidate
/// leaves are visited. A candidate submits iff it passes the frustum (its cell
/// is visible by gather construction); a frustum-rejected candidate writes
/// status 1. Non-candidate leaves stay cleared (status 0, index_count 0).
///
/// Returns `None` when the gather signals `OutOfRange` — the renderer routes
/// that frame to the tree walk, so there is no candidate mirror to compare.
pub(crate) fn candidate_mirror(
    world: &SyntheticWorld,
    visible: &VisibleCells,
    view_proj: &Mat4,
) -> Option<CullMirror> {
    let cells = match visible {
        VisibleCells::Culled(cells) => cells,
        // DrawAll routes to the tree walk; the candidate path never runs.
        VisibleCells::DrawAll => return None,
    };
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    match gather_candidate_leaves(&world.index, cells, &mut candidates, &mut seen) {
        GatherStatus::Ok => {}
        GatherStatus::OutOfRange { .. } => return None,
    };

    let planes = extract_frustum_planes_for_gpu(view_proj);
    let mut slots = vec![
        LeafSlot {
            command: IndirectCommand::default(),
            cull_status: STATUS_NON_CANDIDATE,
        };
        world.leaves.len()
    ];
    let mut submitted = Vec::new();

    for &leaf_idx in &candidates {
        let leaf = &world.leaves[leaf_idx as usize];
        if !passes_frustum(leaf, &planes) {
            slots[leaf_idx as usize].cull_status = STATUS_FRUSTUM_REJECT;
        } else {
            slots[leaf_idx as usize].command = submit_command(leaf);
            slots[leaf_idx as usize].cull_status = STATUS_SUBMITTED;
        }
    }

    // Submitted set in leaf-index order, to match the tree walk's ordering.
    for (leaf_idx, slot) in slots.iter().enumerate() {
        if slot.cull_status == STATUS_SUBMITTED {
            submitted.push(leaf_idx as u32);
        }
    }

    Some(CullMirror {
        submitted,
        slots,
        bucket_ranges: world.bucket_ranges(),
    })
}

// --- Synthetic-world builders ---

/// A drawable BVH leaf with an explicit AABB, bucket, and cell.
pub(crate) fn leaf(
    cell_id: u32,
    material_bucket_id: u32,
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
    index_offset: u32,
    index_count: u32,
) -> BvhLeaf {
    BvhLeaf {
        aabb_min,
        material_bucket_id,
        aabb_max,
        index_offset,
        index_count,
        cell_id,
        chunk_range_start: 0,
        chunk_range_count: 0,
    }
}

/// Build the CSR `CellDrawIndex` from the synthetic world's drawable leaves,
/// exactly as the compiler would: for each cell in ascending id, collect its
/// drawable leaves' contiguous per-bucket spans in ascending `leaf_start`. This
/// is the bake the runtime validates, so deriving it here keeps the oracle's
/// CSR honest rather than hand-listing spans.
///
/// Precondition: `leaves` must be sorted by `(material_bucket_id, cell_id,
/// index_offset)` — matching BVH flatten output — because the bucket-boundary
/// span-splitting depends on that order; unsorted input produces valid-looking
/// but wrong CSR.
pub(crate) fn build_index(
    leaves: &[BvhLeaf],
    cell_count: u32,
    cell_drawable: &[bool],
) -> CellDrawIndex {
    let mut cell_span_offset = Vec::with_capacity(cell_count as usize + 1);
    let mut spans: Vec<Span> = Vec::new();
    cell_span_offset.push(0u32);

    for cell in 0..cell_count {
        if cell_drawable.get(cell as usize).copied().unwrap_or(false) {
            // Drawable leaves for this cell, in global leaf-index order.
            let mut cur: Option<Span> = None;
            for (leaf_idx, leaf) in leaves.iter().enumerate() {
                let drawable = leaf.index_count > 0 && leaf.cell_id == cell;
                if !drawable {
                    continue;
                }
                let leaf_idx = leaf_idx as u32;
                match cur.as_mut() {
                    // Extend a span only when contiguous AND same bucket.
                    Some(s)
                        if s.leaf_start + s.leaf_count == leaf_idx
                            && leaves[(s.leaf_start) as usize].material_bucket_id
                                == leaf.material_bucket_id =>
                    {
                        s.leaf_count += 1;
                    }
                    _ => {
                        if let Some(s) = cur.take() {
                            spans.push(s);
                        }
                        cur = Some(Span {
                            leaf_start: leaf_idx,
                            leaf_count: 1,
                        });
                    }
                }
            }
            if let Some(s) = cur.take() {
                spans.push(s);
            }
        }
        cell_span_offset.push(spans.len() as u32);
    }

    CellDrawIndexSection {
        cell_count,
        span_count: spans.len() as u32,
        cell_span_offset,
        spans,
    }
}

/// Assemble a synthetic world: build the CSR from the leaves and drawability.
pub(crate) fn synthetic_world(
    leaves: Vec<BvhLeaf>,
    cell_count: u32,
    cell_drawable: Vec<bool>,
) -> SyntheticWorld {
    let index = build_index(&leaves, cell_count, &cell_drawable);
    SyntheticWorld {
        leaves,
        cell_drawable,
        index,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    /// Camera at origin looking down -Z, wide FOV. A box in front of the camera
    /// passes the frustum; a box behind it (+Z) is frustum-rejected.
    fn forward_view_proj() -> Mat4 {
        let view = Mat4::look_at_rh(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y);
        let proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 16.0 / 9.0, 0.1, 4096.0);
        proj * view
    }

    fn in_front(min_z: f32, max_z: f32) -> ([f32; 3], [f32; 3]) {
        ([-10.0, -10.0, min_z], [10.0, 10.0, max_z])
    }

    /// A 3-cell world. cell 0 (drawable) owns leaf 0 in front of the camera;
    /// cell 1 (drawable) owns leaf 1 behind the camera; cell 2 (drawable) owns
    /// leaf 2 in front. All in one material bucket so the global bucket range is
    /// a single span.
    fn three_cell_world() -> SyntheticWorld {
        let leaves = vec![
            {
                let (mn, mx) = in_front(-60.0, -40.0);
                leaf(0, 0, mn, mx, 0, 6)
            },
            {
                // Behind the camera (+Z): frustum-rejected.
                leaf(1, 0, [-10.0, -10.0, 40.0], [10.0, 10.0, 60.0], 6, 6)
            },
            {
                let (mn, mx) = in_front(-120.0, -80.0);
                leaf(2, 0, mn, mx, 12, 9)
            },
        ];
        synthetic_world(leaves, 3, vec![true, true, true])
    }

    /// Frustum-visible candidate: submitted with all five indirect fields, and
    /// the candidate path matches the tree walk for the submitted leaf.
    #[test]
    fn frustum_visible_candidate_submits_with_full_command() {
        let world = three_cell_world();
        let vp = forward_view_proj();
        // Only cell 0 visible: its single in-front leaf must submit.
        let visible = VisibleCells::Culled(vec![0]);

        let tree = tree_walk_mirror(&world, &visible, &vp);
        let cand = candidate_mirror(&world, &visible, &vp).expect("candidate path runs");

        assert_eq!(
            tree.submitted,
            vec![0],
            "leaf 0 should submit on the tree walk"
        );
        cand.assert_matches(&tree);

        // The submitted slot carries the full command.
        assert_eq!(cand.slots[0].cull_status, STATUS_SUBMITTED);
        assert_eq!(
            cand.slots[0].command,
            IndirectCommand {
                index_count: 6,
                instance_count: 1,
                first_index: 0,
                base_vertex: 0,
                first_instance: 0,
            }
        );
    }

    /// Frustum-rejected candidate: a visible cell whose leaf is behind the
    /// camera → index_count == 0, cull_status == 1, and the paths agree.
    #[test]
    fn frustum_rejected_candidate_clears_slot_with_status_one() {
        let world = three_cell_world();
        let vp = forward_view_proj();
        // Cell 1 visible, but its leaf is behind the camera.
        let visible = VisibleCells::Culled(vec![1]);

        let tree = tree_walk_mirror(&world, &visible, &vp);
        let cand = candidate_mirror(&world, &visible, &vp).expect("candidate path runs");

        assert!(
            tree.submitted.is_empty(),
            "frustum-rejected leaf must not submit"
        );
        cand.assert_matches(&tree);

        assert_eq!(cand.slots[1].cull_status, STATUS_FRUSTUM_REJECT);
        assert_eq!(cand.slots[1].command.index_count, 0);
    }

    /// Visible-cell-rejected: a drawable leaf whose cell is NOT in `Culled` →
    /// index_count == 0, cull_status == 0 on both paths. Cell 2 is hidden here;
    /// the tree walk visits leaf 2 and marks it status 0, the candidate path
    /// never gathers it (also status 0 / cleared) — normalized semantics agree.
    #[test]
    fn visible_cell_rejected_leaf_stays_cleared_with_status_zero() {
        let world = three_cell_world();
        let vp = forward_view_proj();
        // Only cell 0 visible; cell 2 (also in front, drawable) is hidden.
        let visible = VisibleCells::Culled(vec![0]);

        let tree = tree_walk_mirror(&world, &visible, &vp);
        let cand = candidate_mirror(&world, &visible, &vp).expect("candidate path runs");

        cand.assert_matches(&tree);

        // Leaf 2's cell is not visible: cleared, status 0, on both paths.
        assert_eq!(tree.slots[2].cull_status, STATUS_NON_CANDIDATE);
        assert_eq!(tree.slots[2].command.index_count, 0);
        assert_eq!(cand.slots[2].cull_status, STATUS_NON_CANDIDATE);
        assert_eq!(cand.slots[2].command.index_count, 0);
    }

    /// Multi-cell visible set across buckets: submitted set, global bucket
    /// ranges, and per-leaf commands all match between the two paths. Proves
    /// the candidate path never fragments the GLOBAL bucket ranges.
    #[test]
    fn multi_cell_visible_set_matches_tree_walk() {
        // Two buckets: leaves 0,1 in bucket 0; leaves 2,3 in bucket 1.
        let (mn, mx) = in_front(-60.0, -40.0);
        let leaves = vec![
            leaf(0, 0, mn, mx, 0, 6),
            leaf(1, 0, mn, mx, 6, 6),
            leaf(2, 1, mn, mx, 12, 9),
            leaf(3, 1, mn, mx, 21, 9),
        ];
        let world = synthetic_world(leaves, 4, vec![true, true, true, true]);
        let vp = forward_view_proj();
        // Cells 0, 2, 3 visible; cell 1 hidden.
        let visible = VisibleCells::Culled(vec![0, 2, 3]);

        let tree = tree_walk_mirror(&world, &visible, &vp);
        let cand = candidate_mirror(&world, &visible, &vp).expect("candidate path runs");

        cand.assert_matches(&tree);
        assert_eq!(cand.submitted, vec![0, 2, 3]);
        // Global bucket ranges span ALL leaves, including the hidden one — the
        // draw path is unchanged and never compacts.
        assert_eq!(cand.bucket_ranges.len(), 2);
        assert_eq!(cand.bucket_ranges[0].leaf_count, 2);
        assert_eq!(cand.bucket_ranges[1].leaf_count, 2);
    }

    /// `DrawAll` routes to the tree walk; the candidate path declines (returns
    /// `None`) so the fallback output is preserved.
    #[test]
    fn draw_all_routes_to_tree_walk_only() {
        let world = three_cell_world();
        let vp = forward_view_proj();
        let visible = VisibleCells::DrawAll;

        let tree = tree_walk_mirror(&world, &visible, &vp);
        assert!(
            candidate_mirror(&world, &visible, &vp).is_none(),
            "DrawAll must not run the candidate path"
        );
        // DrawAll submits every drawable in-front leaf (0 and 2); leaf 1 behind.
        assert_eq!(tree.submitted, vec![0, 2]);
    }

    /// Duplicate visible cell id: gather dedupes before expansion, so the
    /// candidate path writes each slot exactly once and still matches the tree
    /// walk. A double-write would corrupt the submitted set or the command.
    #[test]
    fn duplicate_visible_cell_dedupes_before_expansion() {
        let world = three_cell_world();
        let vp = forward_view_proj();
        // Cell 0 repeated three times.
        let visible = VisibleCells::Culled(vec![0, 0, 0]);

        // The gather output has no duplicate leaf indices.
        let mut leaves = Vec::new();
        let mut seen = HashSet::new();
        let status = gather_candidate_leaves(&world.index, &[0, 0, 0], &mut leaves, &mut seen);
        assert_eq!(status, GatherStatus::Ok);
        assert_eq!(leaves, vec![0], "cell 0 owns exactly leaf 0, written once");

        let tree = tree_walk_mirror(&world, &visible, &vp);
        let cand = candidate_mirror(&world, &visible, &vp).expect("candidate path runs");
        cand.assert_matches(&tree);
        assert_eq!(cand.submitted, vec![0]);
    }

    /// Out-of-range visible cell id routes the frame to the tree walk (the
    /// candidate mirror returns `None`), so no partial/corrupt set is compared.
    #[test]
    fn out_of_range_cell_declines_candidate_path() {
        let world = three_cell_world();
        let vp = forward_view_proj();
        // cell_count == 3, so id 9 is out of range.
        let visible = VisibleCells::Culled(vec![0, 9]);
        assert!(
            candidate_mirror(&world, &visible, &vp).is_none(),
            "out-of-range cell id must decline the candidate path"
        );
    }
}
