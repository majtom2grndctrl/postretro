// Occluder-tier portals: the runtime portal set of a two-tier BSP, one portal
// per opening between adjacent occluder-tier leaves whose interiors meet.
// See: context/plans/in-progress/occluder-first-partition/plan.md §Approach notes

mod openings;

use std::collections::HashSet;

use glam::DVec3;

use super::{
    MIN_PORTAL_AREA_M2, PlaneEntry, Portal, distribute_winding, make_node_portal, polygon_area,
};
use crate::partition::{BspChild, BspTree, CellTree, OccluderTier};
use openings::openings;

/// Collinear and duplicate hull points closer than this are dropped.
const HULL_EPSILON: f64 = 1e-9;

/// Sub-tier fragments below this area are clipping dust. The portal-area
/// cull applies to the tier piece and to the pair's portal, never to the
/// fragments below-tier planes cut a tier piece into: those fragments only
/// tell interior from solid, and culling them drops real opening the tier
/// portal must cover.
const FRAGMENT_DUST_AREA_M2: f64 = 1e-6;

/// A portal corner this close to an axis-aligned plane bounding its leaves
/// lies on it: far above clipping dust, far below any authored gap.
const SNAP_EPSILON: f64 = 1e-9;

/// A portal between two occluder-tier leaves, wound counter-clockwise seen
/// from the front side of the tier node plane it lies on.
pub struct TierPortal {
    pub polygon: Vec<DVec3>,
    /// Index into `OccluderTier::leaves` on the plane's front side.
    pub front_tier_leaf: usize,
    pub back_tier_leaf: usize,
}

/// Generate portals at occluder-tier nodes only. Each node's winding is cut
/// among the tier-leaf pairs it separates, then pushed through both leaves'
/// sub-trees; the fragments whose two sub-leaves are both interior (neither
/// solid nor in `exterior_leaves`) survive at any area above clipping dust.
/// Fragments sharing an edge form one opening, and each opening's convex
/// hull is a portal, so a facade's windows each cross it through their own
/// portal rather than one hull spanning the facade. An opening whose hull
/// falls under the portal-area cull is no portal of its own: the pair's
/// small openings share one hull, which takes the cull. The cull also runs
/// on the tier piece, as on every portal set, never on the fragments. A hull
/// can only widen what its fragments let through, and a pair no interior
/// fragment joins gets no portal, so none leads into solid or joins interior
/// to exterior.
pub fn generate_occluder_tier_portals(
    tree: &BspTree,
    tier: &OccluderTier,
    exterior_leaves: &HashSet<usize>,
) -> Vec<TierPortal> {
    let mut portals = Vec::new();
    if !tree.nodes.is_empty() && tier.node_is_tier[0] {
        generate_recursive(tree, tier, exterior_leaves, 0, &[], &mut portals);
    }
    portals
}

/// The runtime portal set in cell ids: each tier portal joins the interior
/// cells of its two occluder-tier leaves. Every tier portal has interior
/// space on both sides, so both cells exist; none touches a solid or exterior
/// cell. Order follows `tier_portals`.
pub fn runtime_portals(tier_portals: Vec<TierPortal>, cells: &CellTree) -> Vec<Portal> {
    let interior_cell = |tier_leaf: usize| {
        cells.tier_leaf_cell[tier_leaf]
            .expect("a tier portal's occluder-tier leaf holds an interior cell")
    };
    tier_portals
        .into_iter()
        .map(|portal| Portal {
            front_leaf: interior_cell(portal.front_tier_leaf),
            back_leaf: interior_cell(portal.back_tier_leaf),
            polygon: portal.polygon,
        })
        .collect()
}

fn generate_recursive(
    tree: &BspTree,
    tier: &OccluderTier,
    exterior_leaves: &HashSet<usize>,
    node_idx: usize,
    ancestor_planes: &[PlaneEntry],
    portals: &mut Vec<TierPortal>,
) {
    let node = &tree.nodes[node_idx];
    let (normal, distance) = (node.plane_normal, node.plane_distance);

    if let Some(winding) = make_node_portal(normal, distance, ancestor_planes) {
        let in_tier = |n: usize| tier.node_is_tier[n];
        let mut tier_pairs: Vec<(Vec<DVec3>, BspChild, BspChild)> = Vec::new();
        distribute_winding(
            tree,
            &winding,
            &node.front,
            &node.back,
            &in_tier,
            MIN_PORTAL_AREA_M2,
            &mut |piece, front, back| {
                tier_pairs.push((piece.to_vec(), front.clone(), back.clone()))
            },
        );

        let interior =
            |leaf: usize| !tree.leaves[leaf].is_solid && !exterior_leaves.contains(&leaf);
        for (piece, front, back) in tier_pairs {
            let mut fragments: Vec<Vec<DVec3>> = Vec::new();
            distribute_winding(
                tree,
                &piece,
                &front,
                &back,
                &|_| true,
                FRAGMENT_DUST_AREA_M2,
                &mut |fragment, front_leaf, back_leaf| {
                    if let (BspChild::Leaf(f), BspChild::Leaf(b)) = (front_leaf, back_leaf)
                        && interior(*f)
                        && interior(*b)
                    {
                        let bounding = tree.leaves[*f]
                            .defining_planes
                            .iter()
                            .chain(&tree.leaves[*b].defining_planes);
                        fragments.push(
                            fragment
                                .iter()
                                .map(|&corner| snap_to_axis_planes(corner, bounding.clone()))
                                .collect(),
                        );
                    }
                },
            );
            let (front_tier_leaf, back_tier_leaf) = (
                tier.tier_leaf_of(tree, &front),
                tier.tier_leaf_of(tree, &back),
            );
            let mut push = |polygon: Vec<DVec3>| {
                portals.push(TierPortal {
                    polygon,
                    front_tier_leaf,
                    back_tier_leaf,
                })
            };
            let mut small: Vec<DVec3> = Vec::new();
            for opening in openings(&fragments) {
                let corners: Vec<DVec3> = opening
                    .iter()
                    .flat_map(|&f| fragments[f].iter().copied())
                    .collect();
                match coplanar_hull(&corners, normal) {
                    Some(polygon) if polygon_area(&polygon) >= MIN_PORTAL_AREA_M2 => push(polygon),
                    _ => small.extend(corners),
                }
            }
            if let Some(polygon) = coplanar_hull(&small, normal)
                && polygon_area(&polygon) >= MIN_PORTAL_AREA_M2
            {
                push(polygon);
            }
        }
    }

    for (child, plane) in [
        (&node.front, (normal, distance)),
        (&node.back, (-normal, -distance)),
    ] {
        if let BspChild::Node(child_idx) = child
            && tier.node_is_tier[*child_idx]
        {
            let mut child_ancestors = ancestor_planes.to_vec();
            child_ancestors.push(plane);
            generate_recursive(
                tree,
                tier,
                exterior_leaves,
                *child_idx,
                &child_ancestors,
                portals,
            );
        }
    }
}

/// Move `point` exactly onto each axis-aligned plane of `planes` it lies
/// within `SNAP_EPSILON` of. Clipping the base winding leaves corners about
/// 1e-13 m off the planes that cut them; a door whose face bounds a portal
/// covers it only if no corner lies past that face, so the dust must go.
fn snap_to_axis_planes<'a>(
    mut point: DVec3,
    planes: impl Iterator<Item = &'a (DVec3, f64)>,
) -> DVec3 {
    for &(normal, distance) in planes {
        let Some(axis) = (0..3).find(|&axis| normal[axis].abs() == 1.0) else {
            continue;
        };
        if (point.dot(normal) - distance).abs() <= SNAP_EPSILON {
            point[axis] = distance / normal[axis];
        }
    }
    point
}

/// Convex hull of points lying on one plane, wound counter-clockwise about
/// its normal, as `make_base_winding` winds. `None` under three corners.
fn coplanar_hull(points: &[DVec3], normal: DVec3) -> Option<Vec<DVec3>> {
    if points.len() < 3 {
        return None;
    }
    let reference = if normal.z.abs() > 0.9 {
        DVec3::X
    } else {
        DVec3::Z
    };
    let u_axis = normal.cross(reference).normalize();
    let v_axis = normal.cross(u_axis).normalize();

    // Each hull corner is one of `points` itself, not re-projected from the
    // plane frame: a corner on an ancestor plane stays where clipping put it,
    // so a door whose face bounds the portal still covers it exactly.
    let mut flat: Vec<(f64, f64, usize)> = points
        .iter()
        .enumerate()
        .map(|(index, p)| (p.dot(u_axis), p.dot(v_axis), index))
        .collect();
    flat.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    flat.dedup_by(|a, b| (a.0 - b.0).abs() <= HULL_EPSILON && (a.1 - b.1).abs() <= HULL_EPSILON);

    // Andrew's monotone chain: lower hull, then upper, counter-clockwise.
    let cross = |o: (f64, f64, usize), a: (f64, f64, usize), b: (f64, f64, usize)| {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };
    let mut hull: Vec<(f64, f64, usize)> = Vec::with_capacity(flat.len() * 2);
    for pass in [
        flat.as_slice(),
        &flat.iter().rev().copied().collect::<Vec<_>>(),
    ] {
        let start = hull.len();
        for &point in pass {
            while hull.len() >= start + 2
                && cross(hull[hull.len() - 2], hull[hull.len() - 1], point) <= HULL_EPSILON
            {
                hull.pop();
            }
            hull.push(point);
        }
        hull.pop();
    }
    if hull.len() < 3 {
        return None;
    }

    Some(
        hull.into_iter()
            .map(|(_, _, index)| points[index])
            .collect(),
    )
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod classification_test_fixtures;

#[cfg(test)]
mod classification_tests;

#[cfg(test)]
mod cluster_tests;
