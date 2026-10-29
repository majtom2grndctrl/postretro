// Brush-volume BSP builder. Leaf solidity assigned during construction via
// the inside-set rule; no post-pass classifier needed.
// See: context/lib/build_pipeline.md §PRL Compilation

mod cell_cap;
mod occluder_tier;
mod shadow_prism;

use anyhow::{Result, bail};
use glam::DVec3;

use super::region_polytope::RegionPolytope;
use super::types::*;
use crate::map_data::BrushVolume;
pub use cell_cap::{
    CAP_MIN_OPEN_SLICE_FRACTION, CAP_MIN_PIECE_FRACTION, CELL_SIZE_CAP_M, CellSizeCap,
};
pub use occluder_tier::{OccluderTier, TierLeaf, TierRole, build_bsp_with_occluder_tier};
use occluder_tier::{TierPlanes, TierRecorder, build_tier_region};
pub use shadow_prism::{SHADOW_PRISM_LENGTH_FACTOR, ShadowPrism, shadow_prisms};

/// Classification tolerance for AABB corners against brush-derived planes.
#[cfg(test)]
const PLANE_EPSILON: f64 = 0.1;

/// Candidate-routing tolerance. A brush routes to one side when its AABB has
/// meaningful extent on that side, including plane contact from that side.
/// Near-plane-only and genuinely straddling AABBs stay on both children so
/// exact child regions keep their owning brush.
const CANDIDATE_ROUTING_EPSILON: f64 = 1e-6;

/// Classification tolerance for exact region-polytope queries. A region
/// within it of a brush's planes is inside the brush, so brush-side
/// projection treats brushes this close as touching and cuts both facing
/// faces; reflection-probe capture exclusion derives its contact tests from it.
/// A change here changes capture output, so it needs a bump of the
/// reflection_probes `STAGE_VERSION`.
pub(crate) const REGION_CLASSIFICATION_EPSILON: f64 = 1e-3;

/// Slack added on every axis when deriving the world AABB from brush bounds.
/// Keeps the splitter from producing degenerate sub-regions at the world boundary
/// without making the tree unnecessarily deep.
const WORLD_AABB_SLACK: f64 = 1.0;

/// Splitter scoring weights. Spanning brushes are penalized heavily because
/// they increase candidate set size on both sides without reducing depth.
const SPLIT_PENALTY: i32 = 8;
const IMBALANCE_PENALTY: i32 = 1;

/// Recursion depth cap. Guards against pathological input that would otherwise
/// exhaust the stack; returns an error instead of overflowing.
const MAX_RECURSION_DEPTH: usize = 256;

/// Build a BSP tree from brush volumes. Leaf solidity is assigned during
/// construction; `face_extract::extract_faces` populates `face_indices` afterward.
pub fn build_bsp_from_brushes(brushes: &[BrushVolume]) -> Result<BspTree> {
    let mut recorder = TierRecorder::default();
    build_tree(brushes, SplitTier::Full, &mut recorder)
}

/// How `build_recursive` picks a region's splitter.
#[derive(Clone, Copy)]
enum SplitTier<'a> {
    /// Occluder-tier planes only, ranked by each brush's role, then shadow
    /// prism planes, then door splitters, then the cell-size cap's grid
    /// lines; a region none spans becomes an occluder-tier leaf and
    /// continues as `Full`.
    Occluder(TierPlanes<'a>),
    /// Every candidate brush's planes, scored by `select_splitter`.
    Full,
}

fn build_tree(
    brushes: &[BrushVolume],
    tier: SplitTier<'_>,
    recorder: &mut TierRecorder,
) -> Result<BspTree> {
    let mut tree = BspTree {
        nodes: Vec::new(),
        leaves: Vec::new(),
    };

    if brushes.is_empty() {
        return Ok(tree);
    }

    let world_bounds = world_aabb_from_brushes(brushes);
    let world_polytope = RegionPolytope::from_aabb(&world_bounds);
    let candidates: Vec<usize> = (0..brushes.len()).collect();
    let inside = compute_inside_set(brushes, &candidates, &world_bounds, &world_polytope);
    let ancestor_planes: Vec<(DVec3, f64)> = Vec::new();
    let leaf_planes: Vec<(DVec3, f64)> = Vec::new();

    build_recursive(
        &mut tree,
        brushes,
        &world_bounds,
        &world_polytope,
        &candidates,
        &inside,
        &ancestor_planes,
        &leaf_planes,
        tier,
        recorder,
        None,
        0,
    )?;

    Ok(tree)
}

fn world_aabb_from_brushes(brushes: &[BrushVolume]) -> Aabb {
    let mut bounds = Aabb::empty();
    for brush in brushes {
        bounds.expand_aabb(&brush.aabb);
    }
    Aabb {
        min: bounds.min - DVec3::splat(WORLD_AABB_SLACK),
        max: bounds.max + DVec3::splat(WORLD_AABB_SLACK),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AabbSide {
    /// Every corner satisfies `dot(p, normal) - distance >= -epsilon`.
    Front,
    Back,
    Spanning,
}

fn aabb_plane_distance_range(bounds: &Aabb, normal: DVec3, distance: f64) -> (f64, f64) {
    // max_support: corner farthest in +normal. min_support: corner farthest in -normal.
    let max_support = DVec3::new(
        if normal.x >= 0.0 {
            bounds.max.x
        } else {
            bounds.min.x
        },
        if normal.y >= 0.0 {
            bounds.max.y
        } else {
            bounds.min.y
        },
        if normal.z >= 0.0 {
            bounds.max.z
        } else {
            bounds.min.z
        },
    );
    let min_support = DVec3::new(
        if normal.x >= 0.0 {
            bounds.min.x
        } else {
            bounds.max.x
        },
        if normal.y >= 0.0 {
            bounds.min.y
        } else {
            bounds.max.y
        },
        if normal.z >= 0.0 {
            bounds.min.z
        } else {
            bounds.max.z
        },
    );

    let max_dist = max_support.dot(normal) - distance;
    let min_dist = min_support.dot(normal) - distance;

    (min_dist, max_dist)
}

/// Classify an AABB against a plane via support-point tests — cheaper than
/// iterating all 8 corners.
#[cfg(test)]
fn classify_aabb(bounds: &Aabb, normal: DVec3, distance: f64) -> AabbSide {
    let (min_dist, max_dist) = aabb_plane_distance_range(bounds, normal, distance);

    if min_dist >= -PLANE_EPSILON {
        AabbSide::Front
    } else if max_dist <= PLANE_EPSILON {
        AabbSide::Back
    } else {
        AabbSide::Spanning
    }
}

fn classify_aabb_for_candidate_routing(bounds: &Aabb, normal: DVec3, distance: f64) -> AabbSide {
    let (min_dist, max_dist) = aabb_plane_distance_range(bounds, normal, distance);

    if min_dist >= -CANDIDATE_ROUTING_EPSILON && max_dist > CANDIDATE_ROUTING_EPSILON {
        AabbSide::Front
    } else if max_dist <= CANDIDATE_ROUTING_EPSILON && min_dist < -CANDIDATE_ROUTING_EPSILON {
        AabbSide::Back
    } else {
        AabbSide::Spanning
    }
}

/// True when every brush half-space has the region fully on its back side,
/// meaning the brush's volume contains the entire region.
fn brush_contains_region(
    brush: &BrushVolume,
    _region: &Aabb,
    region_polytope: &RegionPolytope,
) -> bool {
    for plane in &brush.planes {
        if !region_polytope.all_vertices_behind(
            plane.normal,
            plane.distance,
            REGION_CLASSIFICATION_EPSILON,
        ) {
            return false;
        }
    }
    true
}

fn compute_inside_set(
    brushes: &[BrushVolume],
    candidates: &[usize],
    region: &Aabb,
    region_polytope: &RegionPolytope,
) -> Vec<usize> {
    candidates
        .iter()
        .copied()
        .filter(|&idx| brush_contains_region(&brushes[idx], region, region_polytope))
        .collect()
}

/// Split candidates across a plane. Routing is AABB-based and conservative:
/// near-plane and genuinely straddling AABBs stay on both sides. Exact
/// solidity and splitter qualification use `RegionPolytope`.
fn partition_candidates(
    brushes: &[BrushVolume],
    candidates: &[usize],
    plane_normal: DVec3,
    plane_distance: f64,
) -> (Vec<usize>, Vec<usize>) {
    let mut front = Vec::new();
    let mut back = Vec::new();
    for &idx in candidates {
        let side =
            classify_aabb_for_candidate_routing(&brushes[idx].aabb, plane_normal, plane_distance);
        match side {
            AabbSide::Front => front.push(idx),
            AabbSide::Back => back.push(idx),
            AabbSide::Spanning => {
                front.push(idx);
                back.push(idx);
            }
        }
    }
    (front, back)
}

fn count_partition(
    brushes: &[BrushVolume],
    candidates: &[usize],
    plane_normal: DVec3,
    plane_distance: f64,
) -> (i32, i32, i32) {
    let mut front = 0;
    let mut back = 0;
    let mut spanning = 0;
    for &idx in candidates {
        let side =
            classify_aabb_for_candidate_routing(&brushes[idx].aabb, plane_normal, plane_distance);
        match side {
            AabbSide::Front => front += 1,
            AabbSide::Back => back += 1,
            AabbSide::Spanning => {
                front += 1;
                back += 1;
                spanning += 1;
            }
        }
    }
    (front, back, spanning)
}

/// Tight tolerances for ancestor-plane deduplication. Ancestor entries come
/// from the same plane data they're compared against, so any mismatch is a
/// different plane — not float drift.
const ANCESTOR_PLANE_NORMAL_EPSILON_SQ: f64 = 1e-6;
const ANCESTOR_PLANE_DISTANCE_EPSILON: f64 = 1e-4;

/// True when two (normal, distance) pairs describe the same oriented plane.
/// Used to prevent the recursive descent from selecting the same splitter twice
/// on a root-to-leaf path, which would prevent termination.
fn planes_equivalent(a: (DVec3, f64), b: (DVec3, f64)) -> bool {
    (a.0 - b.0).length_squared() < ANCESTOR_PLANE_NORMAL_EPSILON_SQ
        && (a.1 - b.1).abs() < ANCESTOR_PLANE_DISTANCE_EPSILON
}

/// Pick the best splitting plane from the candidate brushes' bounding planes.
///
/// Considers every plane bounding a candidate brush — no visible-face filter.
/// This is intentional: brush planes that carry no world geometry still
/// separate air gaps and solid regions that a face-driven splitter would miss.
/// (Mirrors id Tech 4 dmap's `SelectSplitPlaneNum`.)
///
/// A plane qualifies iff:
/// 1. Not already on the ancestor path — dedup is what guarantees termination.
/// 2. Spans the current region — ensures both child regions are strictly smaller.
///
/// Returns `None` when no qualifying plane exists.
fn select_splitter(
    brushes: &[BrushVolume],
    candidates: &[usize],
    _region: &Aabb,
    region_polytope: &RegionPolytope,
    ancestor_planes: &[(DVec3, f64)],
) -> Option<(DVec3, f64)> {
    let mut best: Option<((DVec3, f64), i32)> = None;

    for &idx in candidates {
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
                count_partition(brushes, candidates, plane.normal, plane.distance);

            let score = spanning * SPLIT_PENALTY + (front - back).abs() * IMBALANCE_PENALTY;

            match best {
                None => best = Some((key, score)),
                Some((_, best_score)) if score < best_score => {
                    best = Some((key, score));
                }
                _ => {}
            }
        }
    }

    best.map(|(plane, _)| plane)
}

/// Shrink a region AABB to the half-space selected by `side`. Only effective
/// for axis-aligned planes; non-axis-aligned planes return the parent bounds
/// unchanged. Candidate routing remains conservative and AABB-based; exact
/// solidity and splitter qualification use `RegionPolytope`.
fn tighten_region(region: &Aabb, normal: DVec3, distance: f64, side: AabbSide) -> Aabb {
    let mut child = region.clone();
    // A unit normal aligned to an axis has exactly one component ±1, others 0.
    let axis = if normal.x.abs() > 0.999 {
        Some(0)
    } else if normal.y.abs() > 0.999 {
        Some(1)
    } else if normal.z.abs() > 0.999 {
        Some(2)
    } else {
        None
    };

    let Some(axis) = axis else {
        return child;
    };

    // Plane equation: axis_sign * coord = distance, so coord = distance / axis_sign.
    let axis_sign = match axis {
        0 => normal.x,
        1 => normal.y,
        _ => normal.z,
    };
    let plane_coord = distance / axis_sign;

    match (side, axis_sign > 0.0) {
        (AabbSide::Front, true) | (AabbSide::Back, false) => {
            // coord >= plane_coord side: raise the min face.
            let v = plane_coord;
            match axis {
                0 => child.min.x = child.min.x.max(v),
                1 => child.min.y = child.min.y.max(v),
                _ => child.min.z = child.min.z.max(v),
            }
        }
        (AabbSide::Back, true) | (AabbSide::Front, false) => {
            // coord <= plane_coord side: lower the max face.
            let v = plane_coord;
            match axis {
                0 => child.max.x = child.max.x.min(v),
                1 => child.max.y = child.max.y.min(v),
                _ => child.max.z = child.max.z.min(v),
            }
        }
        (AabbSide::Spanning, _) => {}
    }

    child
}

#[allow(clippy::too_many_arguments)]
fn build_recursive(
    tree: &mut BspTree,
    brushes: &[BrushVolume],
    region: &Aabb,
    region_polytope: &RegionPolytope,
    candidates: &[usize],
    inside: &[usize],
    ancestor_planes: &[(DVec3, f64)],
    leaf_planes: &[(DVec3, f64)],
    tier: SplitTier<'_>,
    recorder: &mut TierRecorder,
    parent: Option<usize>,
    depth: usize,
) -> Result<BspChild> {
    if depth > MAX_RECURSION_DEPTH {
        bail!(
            "BSP recursion depth exceeded {MAX_RECURSION_DEPTH} while partitioning brushes; \
             input may contain pathological brush configuration"
        );
    }

    if let SplitTier::Occluder(planes) = tier {
        return build_tier_region(
            tree,
            brushes,
            region,
            region_polytope,
            candidates,
            inside,
            ancestor_planes,
            leaf_planes,
            planes,
            recorder,
            parent,
            depth,
        );
    }

    // No candidates: region is outside all brushes — empty.
    if candidates.is_empty() {
        return Ok(make_leaf(tree, region, region_polytope, false, leaf_planes));
    }

    // Any brush containing the exact region makes that region solid.
    if !inside.is_empty() {
        return Ok(make_leaf(tree, region, region_polytope, true, leaf_planes));
    }

    // Mixed candidates, no qualifying splitter: cannot separate solid from air.
    // Treat as empty (structural air gap, not an error).
    let Some(splitter) = select_splitter(
        brushes,
        candidates,
        region,
        region_polytope,
        ancestor_planes,
    ) else {
        return Ok(make_leaf(tree, region, region_polytope, false, leaf_planes));
    };

    split_region(
        tree,
        brushes,
        region,
        region_polytope,
        candidates,
        ancestor_planes,
        leaf_planes,
        splitter,
        tier,
        recorder,
        parent,
        depth,
    )
}

/// Emit a node on `splitter` and build both children in the same tier.
#[allow(clippy::too_many_arguments)]
fn split_region(
    tree: &mut BspTree,
    brushes: &[BrushVolume],
    region: &Aabb,
    region_polytope: &RegionPolytope,
    candidates: &[usize],
    ancestor_planes: &[(DVec3, f64)],
    leaf_planes: &[(DVec3, f64)],
    (normal, distance): (DVec3, f64),
    tier: SplitTier<'_>,
    recorder: &mut TierRecorder,
    parent: Option<usize>,
    depth: usize,
) -> Result<BspChild> {
    let (front_candidates, back_candidates) =
        partition_candidates(brushes, candidates, normal, distance);

    let front_region = tighten_region(region, normal, distance, AabbSide::Front);
    let back_region = tighten_region(region, normal, distance, AabbSide::Back);
    let (front_polytope, back_polytope) = region_polytope.clip(normal, distance);

    let front_inside =
        compute_inside_set(brushes, &front_candidates, &front_region, &front_polytope);
    let back_inside = compute_inside_set(brushes, &back_candidates, &back_region, &back_polytope);

    let mut child_ancestors = ancestor_planes.to_vec();
    child_ancestors.push((normal, distance));

    // Front child: satisfies dot(p, normal) >= distance, so inward normal is (normal, distance).
    // Back child: satisfies dot(p, normal) <= distance, so inward normal is (-normal, -distance).
    let mut front_leaf_planes = leaf_planes.to_vec();
    front_leaf_planes.push((normal, distance));

    let mut back_leaf_planes = leaf_planes.to_vec();
    back_leaf_planes.push((-normal, -distance));

    // Reserve a node slot so children can record us as their parent.
    let node_idx = tree.nodes.len();
    tree.nodes.push(BspNode {
        plane_normal: normal,
        plane_distance: distance,
        front: BspChild::Leaf(0),
        back: BspChild::Leaf(0),
        parent,
    });
    if let SplitTier::Occluder(_) = tier {
        recorder.tier_nodes.push(node_idx);
    }

    let front_child = build_recursive(
        tree,
        brushes,
        &front_region,
        &front_polytope,
        &front_candidates,
        &front_inside,
        &child_ancestors,
        &front_leaf_planes,
        tier,
        recorder,
        Some(node_idx),
        depth + 1,
    )?;
    let back_child = build_recursive(
        tree,
        brushes,
        &back_region,
        &back_polytope,
        &back_candidates,
        &back_inside,
        &child_ancestors,
        &back_leaf_planes,
        tier,
        recorder,
        Some(node_idx),
        depth + 1,
    )?;

    tree.nodes[node_idx].front = front_child;
    tree.nodes[node_idx].back = back_child;

    Ok(BspChild::Node(node_idx))
}

fn make_leaf(
    tree: &mut BspTree,
    region: &Aabb,
    region_polytope: &RegionPolytope,
    is_solid: bool,
    leaf_planes: &[(DVec3, f64)],
) -> BspChild {
    let leaf_idx = tree.leaves.len();
    tree.leaves.push(BspLeaf {
        face_indices: Vec::new(),
        bounds: leaf_bounds(region, region_polytope),
        is_solid,
        defining_planes: leaf_planes.to_vec(),
    });
    BspChild::Leaf(leaf_idx)
}

fn leaf_bounds(region: &Aabb, region_polytope: &RegionPolytope) -> Aabb {
    let polytope_bounds = region_polytope.vertex_aabb();
    if !polytope_bounds.is_valid() {
        return region.clone();
    }

    let bounds = Aabb {
        min: polytope_bounds.min.max(region.min),
        max: polytope_bounds.max.min(region.max),
    };
    if bounds.is_valid() {
        bounds
    } else {
        region.clone()
    }
}

#[cfg(test)]
mod tests;
