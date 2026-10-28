// Occluder tier of the brush-volume BSP: splitter choice among occluder
// planes, and the record of which nodes and leaves the tier produced.
// See: context/plans/in-progress/occluder-first-partition/index.md §Decisions

use std::ops::Range;

use anyhow::Result;
use glam::DVec3;

use super::{
    IMBALANCE_PENALTY, REGION_CLASSIFICATION_EPSILON, SPLIT_PENALTY, SplitTier, build_recursive,
    build_tree, count_partition, leaf_bounds, planes_equivalent, split_region,
};
use crate::map_data::BrushVolume;
use crate::partition::region_polytope::RegionPolytope;
use crate::partition::types::{Aabb, BspChild, BspTree};

/// Score credit for a plane whose slice through the current region is the
/// largest among the qualifying occluder planes, scaled down linearly with
/// slice area. A wall's top must beat its back face while the region still
/// holds the open space beyond the wall, though the top also cuts that
/// space's far walls. Worth eight spanning occluders: on occlusion-test two
/// is too few (the back face wins), four just carries the top, and an
/// area-dominant credit (sixteen or more) slices every room at every occluder
/// height, doubling the tier's leaves and portals.
const CROSS_SECTION_WEIGHT: f64 = 8.0 * SPLIT_PENALTY as f64;

/// One leaf of the occluder tier: a convex region that no occluder plane
/// spans. Exempt brushes split it further into the full tree's leaves.
#[derive(Debug, Clone)]
pub struct TierLeaf {
    /// Root of the region's sub-tree in the full tree: a node below the
    /// tier, or a single full-tree leaf when nothing split the region.
    pub subtree: BspChild,
    /// The full-tree leaves inside this region. Contiguous, because the
    /// builder emits each tier leaf's sub-tree depth-first in one pass.
    pub leaf_range: Range<usize>,
    /// Inward planes bounding the region, as `BspLeaf::defining_planes`.
    pub defining_planes: Vec<(DVec3, f64)>,
    pub bounds: Aabb,
}

/// Which parts of a full BSP tree the occluder tier built. The full tree's
/// nodes are unchanged; a cell-tree view keeps them all and maps each leaf to
/// a cell through `leaf_tier_leaf`, many leaves to one.
#[derive(Debug, Clone)]
pub struct OccluderTier {
    /// Per full-tree node: true when its plane was chosen inside the tier.
    /// Tier nodes are exactly the ancestors of the tier leaves.
    pub node_is_tier: Vec<bool>,
    pub leaves: Vec<TierLeaf>,
    /// Per full-tree leaf: index into `leaves` of the tier leaf holding it.
    pub leaf_tier_leaf: Vec<usize>,
}

impl OccluderTier {
    /// Tier leaf owning a child the tier descent stopped at: a tier leaf's
    /// sub-tree root. Every leaf under that root maps to the same tier leaf.
    pub fn tier_leaf_of(&self, tree: &BspTree, child: &BspChild) -> usize {
        let mut child = child;
        loop {
            match child {
                BspChild::Leaf(leaf) => return self.leaf_tier_leaf[*leaf],
                BspChild::Node(node) => child = &tree.nodes[*node].front,
            }
        }
    }
}

/// Build a two-tier BSP tree. `occluders[i]` marks brush `i` an occluder:
/// occluder planes split first, each region no occluder plane spans becomes
/// an occluder-tier leaf, and every brush's planes split beneath it with
/// `build_bsp_from_brushes`' scoring. Solidity, leaves and face ownership read
/// the returned tree exactly as they read a one-tier tree.
pub fn build_bsp_with_occluder_tier(
    brushes: &[BrushVolume],
    occluders: &[bool],
) -> Result<(BspTree, OccluderTier)> {
    assert_eq!(
        occluders.len(),
        brushes.len(),
        "one occluder flag per brush"
    );
    let mut recorder = TierRecorder::default();
    let tree = build_tree(brushes, SplitTier::Occluder(occluders), &mut recorder)?;
    let tier = recorder.finish(&tree);
    Ok((tree, tier))
}

/// Build a region inside the occluder tier: split on the best occluder plane,
/// or, when none spans the region, close it as a tier leaf and build its
/// sub-tree on every brush's planes.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_tier_region(
    tree: &mut BspTree,
    brushes: &[BrushVolume],
    region: &Aabb,
    region_polytope: &RegionPolytope,
    candidates: &[usize],
    inside: &[usize],
    ancestor_planes: &[(DVec3, f64)],
    leaf_planes: &[(DVec3, f64)],
    occluders: &[bool],
    recorder: &mut TierRecorder,
    parent: Option<usize>,
    depth: usize,
) -> Result<BspChild> {
    // An empty or solid region is a tier leaf as it stands; otherwise the
    // tier continues while an occluder plane spans the region.
    let splitter = if candidates.is_empty() || !inside.is_empty() {
        None
    } else {
        select_occluder_splitter(
            brushes,
            candidates,
            occluders,
            region_polytope,
            ancestor_planes,
        )
    };
    let Some((normal, distance)) = splitter else {
        let first_leaf = tree.leaves.len();
        let subtree = build_recursive(
            tree,
            brushes,
            region,
            region_polytope,
            candidates,
            inside,
            ancestor_planes,
            leaf_planes,
            SplitTier::Full,
            recorder,
            parent,
            depth,
        )?;
        recorder.leaves.push(TierLeaf {
            subtree: subtree.clone(),
            leaf_range: first_leaf..tree.leaves.len(),
            defining_planes: leaf_planes.to_vec(),
            bounds: leaf_bounds(region, region_polytope),
        });
        return Ok(subtree);
    };
    split_region(
        tree,
        brushes,
        region,
        region_polytope,
        candidates,
        ancestor_planes,
        leaf_planes,
        (normal, distance),
        SplitTier::Occluder(occluders),
        recorder,
        parent,
        depth,
    )
}

/// Builder-side record of the tier as recursion produces it.
#[derive(Debug, Default)]
pub(super) struct TierRecorder {
    pub(super) tier_nodes: Vec<usize>,
    pub(super) leaves: Vec<TierLeaf>,
}

impl TierRecorder {
    pub(super) fn finish(self, tree: &BspTree) -> OccluderTier {
        let mut node_is_tier = vec![false; tree.nodes.len()];
        for node in self.tier_nodes {
            node_is_tier[node] = true;
        }
        let mut leaf_tier_leaf = vec![usize::MAX; tree.leaves.len()];
        for (tier_leaf, leaf) in self.leaves.iter().enumerate() {
            for full_leaf in leaf.leaf_range.clone() {
                leaf_tier_leaf[full_leaf] = tier_leaf;
            }
        }
        debug_assert!(
            leaf_tier_leaf
                .iter()
                .all(|&tier_leaf| tier_leaf != usize::MAX),
            "every full-tree leaf lies under exactly one occluder-tier leaf"
        );
        OccluderTier {
            node_is_tier,
            leaves: self.leaves,
            leaf_tier_leaf,
        }
    }
}

/// Pick the occluder-tier splitter: among the planes of candidate occluder
/// brushes only, qualified as `select_splitter` qualifies planes, scored by
/// today's spanning and balance terms over the occluder candidates, less a
/// credit for the plane's slice through the region. `None` makes the region
/// an occluder-tier leaf.
pub(super) fn select_occluder_splitter(
    brushes: &[BrushVolume],
    candidates: &[usize],
    occluders: &[bool],
    region_polytope: &RegionPolytope,
    ancestor_planes: &[(DVec3, f64)],
) -> Option<(DVec3, f64)> {
    let occluder_candidates: Vec<usize> = candidates
        .iter()
        .copied()
        .filter(|&idx| occluders[idx])
        .collect();

    // (plane, today's score, cross-section area), gathered first so the
    // area credit can be scaled to the region's largest slice.
    let mut qualifying: Vec<((DVec3, f64), i32, f64)> = Vec::new();
    for &idx in &occluder_candidates {
        for plane in &brushes[idx].planes {
            let key = (plane.normal, plane.distance);
            if ancestor_planes
                .iter()
                .any(|&anc| planes_equivalent(anc, key))
            {
                continue;
            }
            if !region_polytope.plane_spans(
                plane.normal,
                plane.distance,
                REGION_CLASSIFICATION_EPSILON,
            ) {
                continue;
            }
            let (front, back, spanning) =
                count_partition(brushes, &occluder_candidates, plane.normal, plane.distance);
            let score = spanning * SPLIT_PENALTY + (front - back).abs() * IMBALANCE_PENALTY;
            let area = region_polytope.cross_section_area(plane.normal, plane.distance);
            qualifying.push((key, score, area));
        }
    }

    let max_area = qualifying
        .iter()
        .map(|&(_, _, area)| area)
        .fold(0.0_f64, f64::max);
    let mut best: Option<((DVec3, f64), f64)> = None;
    for (key, score, area) in qualifying {
        let credit = if max_area > 0.0 {
            CROSS_SECTION_WEIGHT * area / max_area
        } else {
            0.0
        };
        let total = f64::from(score) - credit;
        match best {
            Some((_, best_total)) if total >= best_total => {}
            _ => best = Some((key, total)),
        }
    }
    best.map(|(plane, _)| plane)
}
