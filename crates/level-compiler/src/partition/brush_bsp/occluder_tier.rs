// Occluder tier of the brush-volume BSP: splitter choice among occluder
// planes, and the record of which nodes and leaves the tier produced.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::ops::Range;

use anyhow::Result;
use glam::DVec3;

use super::cell_cap::CellSizeCap;
use super::shadow_prism::ShadowPrism;
use super::{
    IMBALANCE_PENALTY, REGION_CLASSIFICATION_EPSILON, SPLIT_PENALTY, SplitTier, build_recursive,
    build_tree, count_partition, leaf_bounds, planes_equivalent, split_region,
};
use crate::map_data::BrushVolume;
use crate::partition::door_recognition::DoorSplitter;
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

/// How a brush's planes split inside the occluder tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TierRole {
    /// Splits only beneath the tier.
    Exempt,
    /// Every plane splits at occluder rank.
    Occluder,
    /// A member of an occluding cluster section, one flag per plane: a
    /// flagged plane bounds the section's box on its bounding face or one of
    /// its edge faces, one plane to a face, and splits at occluder rank,
    /// bounding the section as a wall's face and top bound the wall. The rest
    /// split only beneath the tier, so each opening's air joins the
    /// occluder-tier leaf behind the bounding face and, unless another tier
    /// plane crosses the slab, crosses into the leaf in front through one
    /// portal.
    SectionMember(Vec<bool>),
}

impl TierRole {
    /// True when plane `plane` of the brush splits at occluder rank.
    fn occluder_rank(&self, plane: usize) -> bool {
        match self {
            TierRole::Exempt => false,
            TierRole::Occluder => true,
            TierRole::SectionMember(outline) => outline[plane],
        }
    }
}

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

/// What splits inside the occluder tier, in rank order: each brush's role,
/// the shadow prisms, the doors, and the cell-size cap.
#[derive(Clone, Copy)]
pub(super) struct TierPlanes<'a> {
    roles: &'a [TierRole],
    prisms: &'a [ShadowPrism],
    doors: &'a [DoorSplitter],
    cap: Option<&'a CellSizeCap<'a>>,
}

/// Build a two-tier BSP tree. `roles[i]` says how brush `i`'s planes split
/// inside the tier: occluder-rank planes split first, then the free end
/// planes of each prism in `prisms` inside the regions it overlaps, then the
/// planes of each door in `doors` inside the region holding it, then, with
/// `cap`, the cell-size cap's grid lines in any region still wider than the
/// cap; each region none of them spans becomes an occluder-tier leaf, and
/// every brush's planes split beneath it with `build_bsp_from_brushes`'
/// scoring. Prism, door and cap planes are never solid: solidity, leaves and
/// face ownership read the returned tree exactly as they read a one-tier
/// tree.
pub fn build_bsp_with_occluder_tier(
    brushes: &[BrushVolume],
    roles: &[TierRole],
    prisms: &[ShadowPrism],
    doors: &[DoorSplitter],
    cap: Option<&CellSizeCap<'_>>,
) -> Result<(BspTree, OccluderTier)> {
    assert_eq!(roles.len(), brushes.len(), "one tier role per brush");
    let mut recorder = TierRecorder::default();
    let planes = TierPlanes {
        roles,
        prisms,
        doors,
        cap,
    };
    let tree = build_tree(brushes, SplitTier::Occluder(planes), &mut recorder)?;
    let tier = recorder.finish(&tree);
    Ok((tree, tier))
}

/// Build a region inside the occluder tier: split on the best occluder plane,
/// or prism plane once no occluder plane qualifies, or door plane once
/// neither does, or cap line once none of those does, or, when none spans
/// the region, close it as a tier leaf and build its sub-tree on every
/// brush's planes.
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
    planes: TierPlanes<'_>,
    recorder: &mut TierRecorder,
    parent: Option<usize>,
    depth: usize,
) -> Result<BspChild> {
    // A solid region is a tier leaf as it stands, so no prism, door or cap
    // plane splits solid; otherwise the tier continues while an occluder,
    // prism, door or cap plane spans the region. A region no brush reaches
    // has no occluder plane, but it may lie in a prism, a door may stand in
    // it, and it may still be wider than the cap.
    let splitter = if !inside.is_empty() {
        None
    } else {
        select_occluder_splitter(
            brushes,
            candidates,
            planes,
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
        SplitTier::Occluder(planes),
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

/// Pick the occluder-tier splitter: among the occluder-rank planes of the
/// candidates (occluders' planes, and section members' flagged planes)
/// scored by the full tree's `SPLIT_PENALTY` and `IMBALANCE_PENALTY` terms
/// over the occluder candidates; failing those, among the free end planes of
/// the shadow prisms the region overlaps, scored over every candidate;
/// failing those, among the planes of the doors the region holds, broad-face
/// planes before edge planes, scored over every candidate. Each of those ranks takes a credit
/// for the plane's slice through the region. Failing all three, the cap's
/// grid line, chosen by its own rule and never scored, since the slice
/// credit would fight its nearest-the-middle choice. `None` makes the region
/// an occluder-tier leaf.
///
/// Prism planes rank after occluder planes, so an occluder's own planes
/// still split first wherever they reach (a wall's top carries across the
/// hall before anything else cuts it), and the prisms then bound whatever
/// region the occluder planes leave behind a wall face, whichever of them
/// split first: a wall's back face can split before its end planes and leave
/// the region behind it without the wall, and the back face's prism still
/// carries its free end planes into it.
///
/// Door planes rank after occluder and prism planes, so a door splits only
/// what would otherwise be the occluder-tier leaf holding its doorway, and
/// adds no boundary to any region it does not reach. Cap lines rank
/// last, so they split only what would otherwise be an occluder-tier leaf,
/// and a cap line through a doorway cuts the door's portals into pieces the
/// closed door still covers.
fn select_occluder_splitter(
    brushes: &[BrushVolume],
    candidates: &[usize],
    TierPlanes {
        roles,
        prisms,
        doors,
        cap,
    }: TierPlanes<'_>,
    region_polytope: &RegionPolytope,
    ancestor_planes: &[(DVec3, f64)],
) -> Option<(DVec3, f64)> {
    let occluders: Vec<usize> = candidates
        .iter()
        .copied()
        .filter(|&idx| roles[idx] == TierRole::Occluder)
        .collect();
    let occluder_rank: Vec<(DVec3, f64)> = candidates
        .iter()
        .flat_map(|&idx| {
            brushes[idx]
                .planes
                .iter()
                .enumerate()
                .filter(move |&(k, _)| roles[idx].occluder_rank(k))
                .map(|(_, plane)| (plane.normal, plane.distance))
        })
        .collect();

    best_plane(
        brushes,
        occluder_rank,
        &occluders,
        region_polytope,
        ancestor_planes,
    )
    .or_else(|| {
        select_prism_splitter(
            brushes,
            candidates,
            prisms,
            region_polytope,
            ancestor_planes,
        )
    })
    .or_else(|| select_door_splitter(brushes, candidates, doors, region_polytope, ancestor_planes))
    .or_else(|| cap.and_then(|cap| cap.splitter(region_polytope, ancestor_planes)))
}

/// The best prism plane for a region no occluder plane spans: a free end
/// plane of a prism the region overlaps.
fn select_prism_splitter(
    brushes: &[BrushVolume],
    candidates: &[usize],
    prisms: &[ShadowPrism],
    region_polytope: &RegionPolytope,
    ancestor_planes: &[(DVec3, f64)],
) -> Option<(DVec3, f64)> {
    let planes: Vec<(DVec3, f64)> = prisms
        .iter()
        .filter(|prism| prism.overlaps(region_polytope))
        .flat_map(|prism| prism.splitters.iter().copied())
        .collect();
    if planes.is_empty() {
        return None;
    }
    best_plane(
        brushes,
        planes,
        candidates,
        region_polytope,
        ancestor_planes,
    )
}

/// The best door plane for a region no occluder or prism plane spans: a
/// broad-face plane of a door the region overlaps, or failing those, one of
/// its edge planes, which then cut only the slab between its broad faces.
fn select_door_splitter(
    brushes: &[BrushVolume],
    candidates: &[usize],
    doors: &[DoorSplitter],
    region_polytope: &RegionPolytope,
    ancestor_planes: &[(DVec3, f64)],
) -> Option<(DVec3, f64)> {
    let held: Vec<&DoorSplitter> = doors
        .iter()
        .filter(|door| door.overlaps(region_polytope))
        .collect();
    if held.is_empty() {
        return None;
    }
    let planes_of = |pick: fn(&DoorSplitter) -> &[(DVec3, f64)]| -> Vec<(DVec3, f64)> {
        held.iter()
            .flat_map(|door| pick(door).iter().copied())
            .collect()
    };
    best_plane(
        brushes,
        planes_of(|door| &door.broad_planes),
        candidates,
        region_polytope,
        ancestor_planes,
    )
    .or_else(|| {
        best_plane(
            brushes,
            planes_of(|door| &door.edge_planes),
            candidates,
            region_polytope,
            ancestor_planes,
        )
    })
}

/// The best of `planes` that qualifies as `select_splitter` qualifies
/// planes, scored over `counted`, less a credit for its slice through the
/// region scaled to the largest qualifying slice.
fn best_plane(
    brushes: &[BrushVolume],
    mut planes: Vec<(DVec3, f64)>,
    counted: &[usize],
    region_polytope: &RegionPolytope,
    ancestor_planes: &[(DVec3, f64)],
) -> Option<(DVec3, f64)> {
    // Coplanar members share planes; score each plane once. The first of a
    // run of equivalent planes in exact order stands for it.
    planes.sort_by(|a, b| {
        a.0.x
            .total_cmp(&b.0.x)
            .then(a.0.y.total_cmp(&b.0.y))
            .then(a.0.z.total_cmp(&b.0.z))
            .then(a.1.total_cmp(&b.1))
    });
    planes.dedup_by(|later, earlier| planes_equivalent(*later, *earlier));

    // (plane, spanning and balance score, cross-section area), gathered
    // first so the area credit can be scaled to the region's largest slice.
    let mut qualifying: Vec<((DVec3, f64), i32, f64)> = Vec::new();
    for key in planes {
        if ancestor_planes
            .iter()
            .any(|&anc| planes_equivalent(anc, key))
        {
            continue;
        }
        if !region_polytope.plane_spans(key.0, key.1, REGION_CLASSIFICATION_EPSILON) {
            continue;
        }
        let (front, back, spanning) = count_partition(brushes, counted, key.0, key.1);
        let score = spanning * SPLIT_PENALTY + (front - back).abs() * IMBALANCE_PENALTY;
        let area = region_polytope.cross_section_area(key.0, key.1);
        qualifying.push((key, score, area));
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
