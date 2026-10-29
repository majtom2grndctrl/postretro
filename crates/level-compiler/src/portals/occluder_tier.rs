// Occluder-tier portals: the runtime portal set of a two-tier BSP, one portal
// per opening between two cells that adjacent occluder-tier leaves hold.
// See: context/plans/in-progress/occluder-first-partition/plan.md §Approach notes

mod openings;
mod pockets;

use glam::DVec3;

use super::{
    MIN_PORTAL_AREA_M2, PlaneEntry, Portal, distribute_winding, make_node_portal, polygon_area,
};
use crate::partition::{BspChild, BspTree, CellTree, OccluderTier};
use openings::openings;
pub use pockets::interior_pockets;

/// Hull points whose plane-frame coordinates lie this close share them, so
/// clipping dust neither orders nor separates them; collinear and duplicate
/// points that close are dropped.
const HULL_EPSILON: f64 = 1e-9;

/// Sub-tier fragments below this area are clipping dust. The portal-area
/// cull applies to the tier piece and to each portal, never to the fragments
/// below-tier planes cut a tier piece into: those fragments only tell
/// interior from solid, and culling them drops real opening the tier portal
/// must cover. Pockets join through fragments down to the same area.
const FRAGMENT_DUST_AREA_M2: f64 = 1e-6;

/// A portal corner this close to a plane bounding its leaves lies on it: far
/// above clipping dust, far below any authored gap.
const SNAP_EPSILON: f64 = 1e-9;

/// The fragments of one tier piece joining one front cell to one back cell.
type PairFragments = ((usize, usize), Vec<Vec<DVec3>>);

/// Generate the runtime portals, in `cells`' ids, at occluder-tier nodes
/// only. Each node's winding is cut among the tier-leaf pairs it separates,
/// then pushed through both leaves' sub-trees; the fragments whose two
/// sub-leaves are both interior survive at any area above clipping dust,
/// each joining the cells of its two sub-leaves' pockets. Per cell pair,
/// fragments sharing an edge form one opening, and each opening's convex
/// hull is a portal, so a facade's windows each cross it through their own
/// portal rather than one hull spanning the facade. An opening whose hull
/// falls under the portal-area cull is no portal of its own: the pair's
/// small openings share one hull, which takes the cull. The cull also runs
/// on the tier piece, as on every portal set, never on the fragments. A hull
/// can only widen what its fragments let through, and a cell pair no
/// interior fragment joins gets no portal, so none leads into solid, joins
/// interior to exterior, or joins two pockets of one tier leaf. Each portal
/// is wound counter-clockwise seen from the front side of its tier node's
/// plane; order follows the tree, then each piece's cell pairs in the order
/// their first fragment appears.
pub fn generate_runtime_portals(
    tree: &BspTree,
    tier: &OccluderTier,
    cells: &CellTree,
) -> Vec<Portal> {
    let mut portals = Vec::new();
    if !tree.nodes.is_empty() && tier.node_is_tier[0] {
        generate_recursive(tree, tier, cells, 0, &[], &mut portals);
    }
    portals
}

fn generate_recursive(
    tree: &BspTree,
    tier: &OccluderTier,
    cells: &CellTree,
    node_idx: usize,
    ancestor_planes: &[PlaneEntry],
    portals: &mut Vec<Portal>,
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

        let interior_cell = |leaf: usize| {
            let cell = cells.leaf_cell[leaf];
            (!cells.tree.leaves[cell].is_solid && !cells.exterior_cells.contains(&cell))
                .then_some(cell)
        };
        for (piece, front, back) in tier_pairs {
            // Per cell pair, in the order its first fragment appears.
            let mut pairs: Vec<PairFragments> = Vec::new();
            distribute_winding(
                tree,
                &piece,
                &front,
                &back,
                &|_| true,
                FRAGMENT_DUST_AREA_M2,
                &mut |fragment, front_leaf, back_leaf| {
                    let (BspChild::Leaf(f), BspChild::Leaf(b)) = (front_leaf, back_leaf) else {
                        return;
                    };
                    let (Some(front_cell), Some(back_cell)) =
                        (interior_cell(*f), interior_cell(*b))
                    else {
                        return;
                    };
                    let bounding = tree.leaves[*f]
                        .defining_planes
                        .iter()
                        .chain(&tree.leaves[*b].defining_planes);
                    let fragment = fragment
                        .iter()
                        .map(|&corner| {
                            snap_to_bounding_planes(corner, bounding.clone(), (normal, distance))
                        })
                        .collect();
                    let pair = (front_cell, back_cell);
                    match pairs.iter_mut().find(|(p, _)| *p == pair) {
                        Some((_, fragments)) => fragments.push(fragment),
                        None => pairs.push((pair, vec![fragment])),
                    }
                },
            );
            for ((front_cell, back_cell), fragments) in pairs {
                let mut push = |polygon: Vec<DVec3>| {
                    portals.push(Portal {
                        polygon,
                        front_leaf: front_cell,
                        back_leaf: back_cell,
                    })
                };
                let mut small: Vec<DVec3> = Vec::new();
                for opening in openings(&fragments) {
                    let corners: Vec<DVec3> = opening
                        .iter()
                        .flat_map(|&f| fragments[f].iter().copied())
                        .collect();
                    match coplanar_hull(&corners, normal) {
                        Some(polygon) if polygon_area(&polygon) >= MIN_PORTAL_AREA_M2 => {
                            push(polygon)
                        }
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
            generate_recursive(tree, tier, cells, *child_idx, &child_ancestors, portals);
        }
    }
}

/// Move `point` onto each plane of `planes` it lies within `SNAP_EPSILON`
/// of, then onto the front of the plane of the portal it bounds, the third
/// argument. Clipping the base winding leaves corners about 1e-13 m off the
/// planes that cut them, and sealing certifies a closed door's cover with no
/// tolerance: a door whose face or edge bounds a portal covers it only if no
/// corner lies past that face or edge, so the dust must go.
///
/// An axis-aligned plane takes the corner exactly, after every other plane,
/// so no projection moves it off again. Any other plane takes it by
/// projection along that plane's normal, which rounding leaves a few ulps to
/// either side. Past a door's edge that is a sliver under the sealing area
/// cull; on the portal's own plane it would put the whole portal past a door
/// face lying there. So the corner then steps onto the front of that plane,
/// which faces into the door when the plane is one of the door's own: a
/// door's tier splitters point into it (`DoorSplitter`).
fn snap_to_bounding_planes<'a>(
    mut point: DVec3,
    planes: impl Iterator<Item = &'a (DVec3, f64)> + Clone,
    (portal_normal, portal_distance): (DVec3, f64),
) -> DVec3 {
    let axis_of = |normal: DVec3| (0..3).find(|&axis| normal[axis].abs() == 1.0);
    for &(normal, distance) in planes.clone() {
        let offset = point.dot(normal) - distance;
        if axis_of(normal).is_none() && offset.abs() <= SNAP_EPSILON {
            point -= normal * (offset / normal.length_squared());
        }
    }
    for &(normal, distance) in planes {
        if let Some(axis) = axis_of(normal)
            && (point.dot(normal) - distance).abs() <= SNAP_EPSILON
        {
            point[axis] = distance / normal[axis];
        }
    }
    if (point.dot(portal_normal) - portal_distance).abs() <= SNAP_EPSILON {
        // Each step at least doubles the last, so a corner within rounding
        // of the plane crosses it in a few steps; on an axis-aligned plane
        // the exact snap already put it there.
        let mut step = f64::EPSILON * point.abs().max_element().max(1.0);
        while point.dot(portal_normal) - portal_distance < 0.0 {
            point += portal_normal * step;
            step *= 2.0;
        }
    }
    point
}

/// Each of `values` replaced by the first of its run: in ascending order, a
/// value within `HULL_EPSILON` of its run's first value joins that run.
fn on_hull_grid(values: &[f64]) -> Vec<f64> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|&a, &b| values[a].total_cmp(&values[b]));
    let mut grid = vec![0.0; values.len()];
    let mut run_start = f64::NEG_INFINITY;
    for index in order {
        if values[index] - run_start > HULL_EPSILON {
            run_start = values[index];
        }
        grid[index] = run_start;
    }
    grid
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
    //
    // The chain runs on a `HULL_EPSILON` grid of the frame coordinates.
    // Corners on one line across an axis, such as a sill's corners on a wall
    // whose u axis is vertical, differ by clipping dust in that coordinate;
    // ordered by the dust rather than along the line, they let the chain pop
    // a real extreme corner as collinear and lose the area it bounds. On the
    // grid each such run shares one coordinate, so it orders along the line.
    // Ties fall back to the exact coordinates, so the first of a duplicate
    // run kept is the lowest, as without dust.
    let u: Vec<f64> = points.iter().map(|p| p.dot(u_axis)).collect();
    let v: Vec<f64> = points.iter().map(|p| p.dot(v_axis)).collect();
    let (grid_u, grid_v) = (on_hull_grid(&u), on_hull_grid(&v));
    let mut flat: Vec<(f64, f64, usize)> = (0..points.len())
        .map(|index| (grid_u[index], grid_v[index], index))
        .collect();
    flat.sort_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then(a.1.total_cmp(&b.1))
            .then(u[a.2].total_cmp(&u[b.2]))
            .then(v[a.2].total_cmp(&v[b.2]))
    });
    flat.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);

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
