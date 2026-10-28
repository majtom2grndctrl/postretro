// Portal generation: emit portal polygons between adjacent BSP leaves.
// See: context/lib/build_pipeline.md §PRL Compilation

use glam::DVec3;

use crate::geometry_utils::{clip_winding_to_half_spaces, make_base_winding, split_polygon};
use crate::partition::{BspChild, BspTree};

/// Tighter than BSP face classification (0.1). Portals are clipped against
/// many ancestor planes in sequence, so a looser epsilon accumulates too much
/// error. Matches ericw-tools' ON_EPSILON for winding operations.
pub(crate) const PORTAL_EPSILON: f64 = 0.01;

/// Slivers below this area are discarded to prevent degenerate geometry
/// from accumulating numerical precision loss during repeated clipping.
const MIN_PORTAL_AREA_M2: f64 = 0.1;

/// A portal connecting two adjacent BSP leaves through a splitting plane.
pub struct Portal {
    /// Convex polygon in engine coordinates.
    pub polygon: Vec<DVec3>,
    pub front_leaf: usize,
    pub back_leaf: usize,
}

/// Generate portals for all adjacent empty leaf pairs in the BSP tree.
pub fn generate_portals(tree: &BspTree) -> Vec<Portal> {
    if tree.nodes.is_empty() {
        return Vec::new();
    }

    let mut portals = Vec::new();
    let ancestor_planes: Vec<(DVec3, f64)> = Vec::new();

    generate_recursive(tree, 0, &ancestor_planes, &mut portals);

    portals
}

type PlaneEntry = (DVec3, f64);

fn generate_recursive(
    tree: &BspTree,
    node_idx: usize,
    ancestor_planes: &[PlaneEntry],
    portals: &mut Vec<Portal>,
) {
    let node = &tree.nodes[node_idx];
    let plane_normal = node.plane_normal;
    let plane_distance = node.plane_distance;

    if let Some(winding) = make_node_portal(plane_normal, plane_distance, ancestor_planes) {
        distribute_portal(tree, &winding, &node.front, &node.back, portals);
    }

    // Front child inherits the plane as-is; back child sees the negated plane.
    let mut front_ancestors = ancestor_planes.to_vec();
    front_ancestors.push((plane_normal, plane_distance));

    if let BspChild::Node(child_idx) = node.front {
        generate_recursive(tree, child_idx, &front_ancestors, portals);
    }

    let mut back_ancestors = ancestor_planes.to_vec();
    back_ancestors.push((-plane_normal, -plane_distance));

    if let BspChild::Node(child_idx) = node.back {
        generate_recursive(tree, child_idx, &back_ancestors, portals);
    }
}

/// Build a portal winding for a node's splitting plane, clipped against all
/// ancestor planes. Returns `None` if the winding is fully clipped or degenerate.
fn make_node_portal(
    plane_normal: DVec3,
    plane_distance: f64,
    ancestor_planes: &[PlaneEntry],
) -> Option<Vec<DVec3>> {
    let winding = make_base_winding(plane_normal, plane_distance);
    let winding = clip_winding_to_half_spaces(winding, ancestor_planes, PORTAL_EPSILON)?;

    if winding.len() < 3 || polygon_area(&winding) < MIN_PORTAL_AREA_M2 {
        return None;
    }

    Some(winding)
}

/// Distribute a portal winding through the BSP subtrees to find the leaf pairs it connects.
fn distribute_portal(
    tree: &BspTree,
    winding: &[DVec3],
    front_child: &BspChild,
    back_child: &BspChild,
    portals: &mut Vec<Portal>,
) {
    match (front_child, back_child) {
        (BspChild::Leaf(f), BspChild::Leaf(b)) => {
            let front_leaf = &tree.leaves[*f];
            let back_leaf = &tree.leaves[*b];
            if !front_leaf.is_solid && !back_leaf.is_solid {
                portals.push(Portal {
                    polygon: winding.to_vec(),
                    front_leaf: *f,
                    back_leaf: *b,
                });
            }
        }

        (BspChild::Node(n), _) => {
            let split_node = &tree.nodes[*n];
            let (front_winding, back_winding) = split_polygon(
                winding,
                split_node.plane_normal,
                split_node.plane_distance,
                PORTAL_EPSILON,
            );

            if let Some(fw) = front_winding
                && fw.len() >= 3
                && polygon_area(&fw) >= MIN_PORTAL_AREA_M2
            {
                distribute_portal(tree, &fw, &split_node.front, back_child, portals);
            }
            if let Some(bw) = back_winding
                && bw.len() >= 3
                && polygon_area(&bw) >= MIN_PORTAL_AREA_M2
            {
                distribute_portal(tree, &bw, &split_node.back, back_child, portals);
            }
        }

        (_, BspChild::Node(n)) => {
            let split_node = &tree.nodes[*n];
            let (front_winding, back_winding) = split_polygon(
                winding,
                split_node.plane_normal,
                split_node.plane_distance,
                PORTAL_EPSILON,
            );

            if let Some(fw) = front_winding
                && fw.len() >= 3
                && polygon_area(&fw) >= MIN_PORTAL_AREA_M2
            {
                distribute_portal(tree, &fw, front_child, &split_node.front, portals);
            }
            if let Some(bw) = back_winding
                && bw.len() >= 3
                && polygon_area(&bw) >= MIN_PORTAL_AREA_M2
            {
                distribute_portal(tree, &bw, front_child, &split_node.back, portals);
            }
        }
    }
}

fn polygon_area(vertices: &[DVec3]) -> f64 {
    if vertices.len() < 3 {
        return 0.0;
    }

    let mut area = DVec3::ZERO;
    let v0 = vertices[0];
    for i in 1..vertices.len() - 1 {
        let edge1 = vertices[i] - v0;
        let edge2 = vertices[i + 1] - v0;
        area += edge1.cross(edge2);
    }
    area.length() * 0.5
}

#[cfg(test)]
mod tests;
