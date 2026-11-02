// Portal generation: emit portal polygons between adjacent BSP leaves.
// See: context/lib/build_pipeline.md §PRL Compilation

use glam::DVec3;

use crate::geometry_utils::{clip_winding_to_half_spaces, make_base_winding, split_polygon};
use crate::partition::{BspChild, BspTree};

mod occluder_tier;

pub use occluder_tier::{generate_runtime_portals, interior_pockets};

/// Tighter than BSP face classification (0.1). Portals are clipped against
/// many ancestor planes in sequence, so a looser epsilon accumulates too much
/// error. Matches ericw-tools' ON_EPSILON for winding operations.
pub(crate) const PORTAL_EPSILON: f64 = 0.01;

/// Slivers below this area are discarded to prevent degenerate geometry
/// from accumulating numerical precision loss during repeated clipping.
pub(crate) const MIN_PORTAL_AREA_M2: f64 = 0.1;

/// A portal connecting two adjacent BSP leaves through a splitting plane.
/// In the runtime set (`generate_runtime_portals`) the endpoints are cell
/// ids, under the leaf-era field names the PRL format keeps.
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
    distribute_winding(
        tree,
        winding,
        front_child,
        back_child,
        &|_| true,
        MIN_PORTAL_AREA_M2,
        &mut |piece, front, back| {
            let (BspChild::Leaf(f), BspChild::Leaf(b)) = (front, back) else {
                return;
            };
            if !tree.leaves[*f].is_solid && !tree.leaves[*b].is_solid {
                portals.push(Portal {
                    polygon: piece.to_vec(),
                    front_leaf: *f,
                    back_leaf: *b,
                });
            }
        },
    );
}

/// Push a winding through the subtrees on each side of its plane, splitting
/// it at every node `descend` admits, front side first. Each piece of at
/// least `min_area` reaches `on_pair` with the two children it ends between:
/// leaves, or nodes `descend` refused.
fn distribute_winding(
    tree: &BspTree,
    winding: &[DVec3],
    front_child: &BspChild,
    back_child: &BspChild,
    descend: &impl Fn(usize) -> bool,
    min_area: f64,
    on_pair: &mut impl FnMut(&[DVec3], &BspChild, &BspChild),
) {
    let (split_node, split_front) = match (front_child, back_child) {
        (BspChild::Node(n), _) if descend(*n) => (&tree.nodes[*n], true),
        (_, BspChild::Node(n)) if descend(*n) => (&tree.nodes[*n], false),
        _ => {
            on_pair(winding, front_child, back_child);
            return;
        }
    };

    let (front_winding, back_winding) = split_polygon(
        winding,
        split_node.plane_normal,
        split_node.plane_distance,
        PORTAL_EPSILON,
    );
    for (piece, side) in [
        (front_winding, &split_node.front),
        (back_winding, &split_node.back),
    ] {
        let Some(piece) = piece else { continue };
        if piece.len() < 3 || polygon_area(&piece) < min_area {
            continue;
        }
        if split_front {
            distribute_winding(tree, &piece, side, back_child, descend, min_area, on_pair);
        } else {
            distribute_winding(tree, &piece, front_child, side, descend, min_area, on_pair);
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
