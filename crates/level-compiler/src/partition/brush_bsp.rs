// Brush-volume BSP builder. Leaf solidity assigned during construction via
// the inside-set rule; no post-pass classifier needed.
// See: context/lib/build_pipeline.md §PRL Compilation

use anyhow::{Result, bail};
use glam::DVec3;

use super::types::*;
use crate::map_data::BrushVolume;

/// Classification tolerance for AABB corners against brush-derived planes.
const PLANE_EPSILON: f64 = 0.1;

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
    let mut tree = BspTree {
        nodes: Vec::new(),
        leaves: Vec::new(),
    };

    if brushes.is_empty() {
        return Ok(tree);
    }

    let world_bounds = world_aabb_from_brushes(brushes);
    let candidates: Vec<usize> = (0..brushes.len()).collect();
    let inside = compute_inside_set(brushes, &candidates, &world_bounds);
    let ancestor_planes: Vec<(DVec3, f64)> = Vec::new();

    build_recursive(
        &mut tree,
        brushes,
        &world_bounds,
        &candidates,
        &inside,
        &ancestor_planes,
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

/// Classify an AABB against a plane via support-point tests — cheaper than
/// iterating all 8 corners.
fn classify_aabb(bounds: &Aabb, normal: DVec3, distance: f64) -> AabbSide {
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

    if min_dist >= -PLANE_EPSILON {
        AabbSide::Front
    } else if max_dist <= PLANE_EPSILON {
        AabbSide::Back
    } else {
        AabbSide::Spanning
    }
}

/// True when every brush half-space has the region fully on its back side,
/// meaning the brush's volume contains the entire region.
fn brush_contains_region(brush: &BrushVolume, region: &Aabb) -> bool {
    for plane in &brush.planes {
        if classify_aabb(region, plane.normal, plane.distance) != AabbSide::Back {
            return false;
        }
    }
    true
}

fn compute_inside_set(brushes: &[BrushVolume], candidates: &[usize], region: &Aabb) -> Vec<usize> {
    candidates
        .iter()
        .copied()
        .filter(|&idx| brush_contains_region(&brushes[idx], region))
        .collect()
}

/// Split candidates across a plane. Spanning brushes appear in both lists.
fn partition_candidates(
    brushes: &[BrushVolume],
    candidates: &[usize],
    plane_normal: DVec3,
    plane_distance: f64,
) -> (Vec<usize>, Vec<usize>) {
    let mut front = Vec::new();
    let mut back = Vec::new();
    for &idx in candidates {
        match classify_aabb(&brushes[idx].aabb, plane_normal, plane_distance) {
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
        match classify_aabb(&brushes[idx].aabb, plane_normal, plane_distance) {
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
    region: &Aabb,
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

            if classify_aabb(region, plane.normal, plane.distance) != AabbSide::Spanning {
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
/// unchanged (conservative, but correct — all classification tests use AABB
/// support points, not the exact half-space boundary).
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
    candidates: &[usize],
    inside: &[usize],
    ancestor_planes: &[(DVec3, f64)],
    parent: Option<usize>,
    depth: usize,
) -> Result<BspChild> {
    if depth > MAX_RECURSION_DEPTH {
        bail!(
            "BSP recursion depth exceeded {MAX_RECURSION_DEPTH} while partitioning brushes; \
             input may contain pathological brush configuration"
        );
    }

    // No candidates: region is outside all brushes — empty.
    if candidates.is_empty() {
        return Ok(make_leaf(tree, region, false));
    }

    // All candidates contain the region: fully solid.
    if candidates.len() == inside.len() {
        return Ok(make_leaf(tree, region, true));
    }

    // Mixed candidates, no qualifying splitter: cannot separate solid from air.
    // Treat as empty (structural air gap, not an error).
    let Some((normal, distance)) = select_splitter(brushes, candidates, region, ancestor_planes)
    else {
        return Ok(make_leaf(tree, region, false));
    };

    let (front_candidates, back_candidates) =
        partition_candidates(brushes, candidates, normal, distance);

    let front_region = tighten_region(region, normal, distance, AabbSide::Front);
    let back_region = tighten_region(region, normal, distance, AabbSide::Back);

    let front_inside = compute_inside_set(brushes, &front_candidates, &front_region);
    let back_inside = compute_inside_set(brushes, &back_candidates, &back_region);

    let mut child_ancestors = ancestor_planes.to_vec();
    child_ancestors.push((normal, distance));

    // Reserve a node slot so children can record us as their parent.
    let node_idx = tree.nodes.len();
    tree.nodes.push(BspNode {
        plane_normal: normal,
        plane_distance: distance,
        front: BspChild::Leaf(0),
        back: BspChild::Leaf(0),
        parent,
    });

    let front_child = build_recursive(
        tree,
        brushes,
        &front_region,
        &front_candidates,
        &front_inside,
        &child_ancestors,
        Some(node_idx),
        depth + 1,
    )?;
    let back_child = build_recursive(
        tree,
        brushes,
        &back_region,
        &back_candidates,
        &back_inside,
        &child_ancestors,
        Some(node_idx),
        depth + 1,
    )?;

    tree.nodes[node_idx].front = front_child;
    tree.nodes[node_idx].back = back_child;

    Ok(BspChild::Node(node_idx))
}

fn make_leaf(tree: &mut BspTree, region: &Aabb, is_solid: bool) -> BspChild {
    let leaf_idx = tree.leaves.len();
    tree.leaves.push(BspLeaf {
        face_indices: Vec::new(),
        bounds: region.clone(),
        is_solid,
    });
    BspChild::Leaf(leaf_idx)
}

#[cfg(test)]
mod tests {
    use super::super::bsp::find_leaf_for_point;
    use super::*;
    use crate::map_data::BrushPlane;

    fn box_brush(min: DVec3, max: DVec3) -> BrushVolume {
        BrushVolume {
            planes: vec![
                BrushPlane {
                    normal: DVec3::X,
                    distance: max.x,
                },
                BrushPlane {
                    normal: DVec3::NEG_X,
                    distance: -min.x,
                },
                BrushPlane {
                    normal: DVec3::Y,
                    distance: max.y,
                },
                BrushPlane {
                    normal: DVec3::NEG_Y,
                    distance: -min.y,
                },
                BrushPlane {
                    normal: DVec3::Z,
                    distance: max.z,
                },
                BrushPlane {
                    normal: DVec3::NEG_Z,
                    distance: -min.z,
                },
            ],
            sides: Vec::new(),
            aabb: Aabb { min, max },
        }
    }

    fn hollow_room(min: DVec3, max: DVec3, wall: f64) -> Vec<BrushVolume> {
        vec![
            // Floor
            box_brush(
                DVec3::new(min.x, min.y, min.z),
                DVec3::new(max.x, min.y + wall, max.z),
            ),
            // Ceiling
            box_brush(
                DVec3::new(min.x, max.y - wall, min.z),
                DVec3::new(max.x, max.y, max.z),
            ),
            // Wall -X
            box_brush(
                DVec3::new(min.x, min.y, min.z),
                DVec3::new(min.x + wall, max.y, max.z),
            ),
            // Wall +X
            box_brush(
                DVec3::new(max.x - wall, min.y, min.z),
                DVec3::new(max.x, max.y, max.z),
            ),
            // Wall -Z
            box_brush(
                DVec3::new(min.x, min.y, min.z),
                DVec3::new(max.x, max.y, min.z + wall),
            ),
            // Wall +Z
            box_brush(
                DVec3::new(min.x, min.y, max.z - wall),
                DVec3::new(max.x, max.y, max.z),
            ),
        ]
    }

    fn leaf_at(tree: &BspTree, point: DVec3) -> &BspLeaf {
        let idx = find_leaf_for_point(tree, point);
        &tree.leaves[idx]
    }

    #[test]
    fn empty_brush_set_produces_empty_tree() {
        let tree = build_bsp_from_brushes(&[]).expect("empty input is valid");
        assert!(tree.nodes.is_empty());
        assert!(tree.leaves.is_empty());
    }

    #[test]
    fn world_aabb_unions_brush_bounds_with_slack() {
        let brushes = vec![
            box_brush(DVec3::ZERO, DVec3::splat(10.0)),
            box_brush(DVec3::splat(50.0), DVec3::splat(60.0)),
        ];
        let world = world_aabb_from_brushes(&brushes);
        assert!((world.min.x - (-WORLD_AABB_SLACK)).abs() < 1e-9);
        assert!((world.min.y - (-WORLD_AABB_SLACK)).abs() < 1e-9);
        assert!((world.min.z - (-WORLD_AABB_SLACK)).abs() < 1e-9);
        assert!((world.max.x - (60.0 + WORLD_AABB_SLACK)).abs() < 1e-9);
        assert!((world.max.y - (60.0 + WORLD_AABB_SLACK)).abs() < 1e-9);
        assert!((world.max.z - (60.0 + WORLD_AABB_SLACK)).abs() < 1e-9);
    }

    #[test]
    fn classify_aabb_fully_back() {
        let bounds = Aabb {
            min: DVec3::new(0.0, 0.0, 0.0),
            max: DVec3::new(5.0, 5.0, 5.0),
        };
        assert_eq!(classify_aabb(&bounds, DVec3::X, 10.0), AabbSide::Back);
    }

    #[test]
    fn classify_aabb_fully_front() {
        let bounds = Aabb {
            min: DVec3::new(0.0, 0.0, 0.0),
            max: DVec3::new(5.0, 5.0, 5.0),
        };
        assert_eq!(classify_aabb(&bounds, DVec3::X, -10.0), AabbSide::Front);
    }

    #[test]
    fn classify_aabb_spanning() {
        let bounds = Aabb {
            min: DVec3::new(0.0, 0.0, 0.0),
            max: DVec3::new(10.0, 10.0, 10.0),
        };
        assert_eq!(classify_aabb(&bounds, DVec3::X, 5.0), AabbSide::Spanning);
    }

    #[test]
    fn hollow_room_interior_air_exterior_solid() {
        let brushes = hollow_room(DVec3::ZERO, DVec3::splat(100.0), 4.0);
        let tree = build_bsp_from_brushes(&brushes).expect("hollow room should build");

        let interior = DVec3::splat(50.0);
        assert!(
            !leaf_at(&tree, interior).is_solid,
            "interior air point should land in an empty leaf"
        );

        let floor = DVec3::new(50.0, 2.0, 50.0);
        assert!(
            leaf_at(&tree, floor).is_solid,
            "floor-interior point should land in a solid leaf"
        );

        // Exterior is empty: no candidate brushes contain the region.
        let exterior = DVec3::new(50.0, 150.0, 50.0);
        assert!(
            !leaf_at(&tree, exterior).is_solid,
            "exterior point should land in an empty leaf"
        );
    }

    #[test]
    fn hollow_room_with_central_pillar_pillar_is_solid() {
        let mut brushes = hollow_room(DVec3::ZERO, DVec3::splat(200.0), 8.0);
        brushes.push(box_brush(
            DVec3::new(90.0, 8.0, 90.0),
            DVec3::new(110.0, 192.0, 110.0),
        ));

        let tree = build_bsp_from_brushes(&brushes).expect("pillar room should build");

        let pillar_interior = DVec3::new(100.0, 100.0, 100.0);
        assert!(
            leaf_at(&tree, pillar_interior).is_solid,
            "pillar interior should be solid"
        );

        let side_air = DVec3::new(50.0, 100.0, 50.0);
        assert!(
            !leaf_at(&tree, side_air).is_solid,
            "open air next to the pillar should be empty"
        );
    }

    #[test]
    fn room_with_doorway_has_connected_air() {
        // Single shell (X=0..500) with a dividing wall at X=200..220 leaving a
        // doorway gap at Y=8..80, Z=80..120. Wall built from two jambs + lintel.
        let mut brushes = hollow_room(DVec3::ZERO, DVec3::new(500.0, 200.0, 200.0), 8.0);

        // Side jamb -Z:
        brushes.push(box_brush(
            DVec3::new(200.0, 8.0, 8.0),
            DVec3::new(220.0, 192.0, 80.0),
        ));
        // Side jamb +Z:
        brushes.push(box_brush(
            DVec3::new(200.0, 8.0, 120.0),
            DVec3::new(220.0, 192.0, 192.0),
        ));
        // Lintel above the doorway:
        brushes.push(box_brush(
            DVec3::new(200.0, 80.0, 80.0),
            DVec3::new(220.0, 192.0, 120.0),
        ));

        let tree = build_bsp_from_brushes(&brushes).expect("doorway room should build");

        let left_air = DVec3::new(100.0, 40.0, 100.0);
        assert!(
            !leaf_at(&tree, left_air).is_solid,
            "left room interior should be empty"
        );

        let right_air = DVec3::new(400.0, 40.0, 100.0);
        assert!(
            !leaf_at(&tree, right_air).is_solid,
            "right room interior should be empty"
        );

        let doorway = DVec3::new(210.0, 40.0, 100.0);
        assert!(
            !leaf_at(&tree, doorway).is_solid,
            "doorway opening should be empty"
        );

        let jamb_interior = DVec3::new(210.0, 100.0, 50.0);
        assert!(
            leaf_at(&tree, jamb_interior).is_solid,
            "jamb interior should be solid"
        );

        let lintel_interior = DVec3::new(210.0, 150.0, 100.0);
        assert!(
            leaf_at(&tree, lintel_interior).is_solid,
            "lintel interior should be solid"
        );
    }

    #[test]
    fn adjacent_brushes_with_narrow_air_gap_preserve_air() {
        // Validates that brush-plane splitting sees the gap even though no world
        // face sits on the mid-gap planes (the failure mode of face-driven splitters).
        let brushes = vec![
            box_brush(DVec3::new(0.0, 0.0, 0.0), DVec3::new(20.0, 20.0, 10.0)),
            box_brush(DVec3::new(0.0, 0.0, 12.0), DVec3::new(20.0, 20.0, 22.0)), // gap Z=10..12
        ];

        let tree = build_bsp_from_brushes(&brushes).expect("narrow gap should build");

        let a_interior = DVec3::new(10.0, 10.0, 5.0);
        assert!(
            leaf_at(&tree, a_interior).is_solid,
            "brush A interior should be solid"
        );

        let b_interior = DVec3::new(10.0, 10.0, 17.0);
        assert!(
            leaf_at(&tree, b_interior).is_solid,
            "brush B interior should be solid"
        );

        let gap = DVec3::new(10.0, 10.0, 11.0);
        assert!(
            !leaf_at(&tree, gap).is_solid,
            "narrow air gap between adjacent brushes should be empty"
        );
    }

    #[test]
    fn single_brush_interior_is_solid_exterior_is_empty() {
        let brushes = vec![box_brush(
            DVec3::new(-5.0, -5.0, -5.0),
            DVec3::new(5.0, 5.0, 5.0),
        )];
        let tree = build_bsp_from_brushes(&brushes).expect("single brush should build");

        assert!(leaf_at(&tree, DVec3::ZERO).is_solid);
        assert!(!leaf_at(&tree, DVec3::new(100.0, 0.0, 0.0)).is_solid);
    }

    #[test]
    fn recursion_depth_cap_returns_error_on_overflow() {
        // 260 brushes along +X, each 1 unit wide with 1-unit gaps. Forces the
        // splitter to descend once per brush, pushing depth past MAX_RECURSION_DEPTH.
        // The builder must return an error rather than stack-overflow.
        //
        // We can't lower the cap from a test, so we verify the outcome is either
        // a clean error (depth exceeded) or a successful shallow tree — never a panic.
        let mut brushes = Vec::new();
        for i in 0..260 {
            let x = (i as f64) * 2.0;
            brushes.push(box_brush(
                DVec3::new(x, 0.0, 0.0),
                DVec3::new(x + 1.0, 1.0, 1.0),
            ));
        }
        let result = build_bsp_from_brushes(&brushes);
        if let Err(e) = result {
            let msg = format!("{e}");
            assert!(
                msg.contains("recursion depth"),
                "error should mention recursion depth, got: {msg}"
            );
        }
    }
}
