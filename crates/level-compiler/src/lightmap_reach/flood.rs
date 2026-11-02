// Per-light ball flood over the full compiler BSP's open leaves: seeds, R, and
// the portal graph the flood crosses.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use glam::DVec3;

use crate::lightmap_bake::RAY_EPSILON;
use crate::map_data::MapLight;
use crate::partition::{BspChild, BspTree, find_leaf_for_point};
use crate::portals::{Portal, PortalTolerance, generate_portals_with};

use super::LeafSet;

/// The flood's own full-tree portals: every piece of every node winding, with
/// no area floor and a clip tolerance of a micrometre. The shipped set
/// ([`PortalTolerance::SHIPPED`]) drops openings under 0.1 m² or about 1 cm
/// wide, which a ray crosses; the flood must cross every opening a ray can.
/// A piece narrower than this tolerance lies between open/solid boundaries
/// whose faces (the precondition the coverage proof checks) come within τ of
/// any ray through it, so the tracer blocks that ray (build_pipeline.md step
/// 12, Exact skips).
const FLOOD_PORTALS: PortalTolerance = PortalTolerance {
    epsilon: 1.0e-6,
    min_area: 0.0,
};

/// Slack on the seed sphere over `light_size`, in metres: every emitter
/// target lies within `light_size` of the light's `f32` origin to within
/// `f32` rounding far below this.
const SEED_SLACK: f64 = 1.0e-3;

/// R's absolute margin, in `RAY_EPSILON`s. A traced point lies at most one
/// footprint radius plus a lift and a skipped `RAY_EPSILON` (two in all) past
/// a texel centre within range; clip segments and lead-ins reach 2.25.
const REACH_MARGIN_EPSILONS: f64 = 4.0;

/// R's relative margin, per metre of range, size, footprint radius and the
/// light's distance from the world origin. It covers the bake's `f32`
/// distance rounding (`chart_cull`'s 4e-6 on range), the footprint radius
/// slack (1e-5, `slab::RADIUS_SLACK`), and the few `f32` adds that place a
/// footprint point, its lift and its trace start (each 2⁻²⁴ of the
/// coordinate).
const REACH_SLACK_PER_METER: f64 = 2.0e-5;

/// The portal graph over the full tree's leaves.
pub(super) struct PortalGraph {
    portals: Vec<Portal>,
    /// Per leaf: (neighbour leaf, portal index).
    adjacency: Vec<Vec<(u32, u32)>>,
}

impl PortalGraph {
    pub(super) fn new(tree: &BspTree) -> Self {
        let portals = generate_portals_with(tree, FLOOD_PORTALS);
        let mut adjacency = vec![Vec::new(); tree.leaves.len()];
        for (index, portal) in portals.iter().enumerate() {
            let (front, back) = (portal.front_leaf, portal.back_leaf);
            adjacency[front].push((back as u32, index as u32));
            adjacency[back].push((front as u32, index as u32));
        }
        Self { portals, adjacency }
    }

    #[cfg(test)]
    pub(super) fn portals(&self) -> &[Portal] {
        &self.portals
    }
}

/// A point or spot light's emitter as the walk traces to it: the `f32`
/// origin every ray target reads, in `f64`, and its size.
#[derive(Debug, Clone, Copy)]
pub(super) struct Emitter {
    pub(super) centre: DVec3,
    pub(super) size: f64,
}

impl Emitter {
    pub(super) fn of(light: &MapLight) -> Self {
        Self {
            centre: light.origin.as_vec3().as_dvec3(),
            size: f64::from(light.light_size.max(0.0)),
        }
    }
}

/// R: a bound on the distance from the emitter centre of every point of
/// every segment the sampler traces for a texel the light lights. A texel
/// with a direct term lies within the clamped `falloff_range`; its traced
/// points lie within a footprint radius and two `RAY_EPSILON`s of it; an
/// emitter target lies within `size`.
pub(super) fn reach_radius(light: &MapLight, emitter: Emitter, footprint_radius: f64) -> f64 {
    let range = f64::from(light.falloff_range.max(1.0e-4));
    let terms = range + emitter.size + footprint_radius;
    terms
        + REACH_MARGIN_EPSILONS * f64::from(RAY_EPSILON)
        + REACH_SLACK_PER_METER * (terms + emitter.centre.length())
}

/// The open leaves `emitter` can carry light into within `reach`: from every
/// open leaf the emitter sphere overlaps, across every portal whose polygon
/// comes within `reach` of the centre, never into a closed leaf. `None` when
/// the centre's own leaf is closed.
pub(super) fn ball_flood(
    tree: &BspTree,
    closed: &[bool],
    graph: &PortalGraph,
    emitter: Emitter,
    reach: f64,
) -> Option<LeafSet> {
    let source = find_leaf_for_point(tree, emitter.centre);
    if closed.get(source).is_none_or(|&closed| closed) {
        return None;
    }
    let mut reached = LeafSet::new(tree.leaves.len());
    let mut frontier = Vec::new();
    let seed = |leaf: usize, reached: &mut LeafSet, frontier: &mut Vec<usize>| {
        if !closed[leaf] && reached.insert(leaf) {
            frontier.push(leaf);
        }
    };
    seed(source, &mut reached, &mut frontier);
    for leaf in sphere_leaves(tree, emitter.centre, emitter.size + SEED_SLACK) {
        seed(leaf, &mut reached, &mut frontier);
    }
    while let Some(leaf) = frontier.pop() {
        for &(other, portal) in &graph.adjacency[leaf] {
            let other = other as usize;
            if closed[other] || reached.contains(other) {
                continue;
            }
            if point_polygon_distance(emitter.centre, &graph.portals[portal as usize].polygon)
                <= reach
            {
                reached.insert(other);
                frontier.push(other);
            }
        }
    }
    Some(reached)
}

/// Every leaf whose half-spaces each come within `radius` of `centre`: a
/// superset of the leaves the sphere overlaps.
pub(super) fn sphere_leaves(tree: &BspTree, centre: DVec3, radius: f64) -> Vec<usize> {
    let mut leaves = Vec::new();
    if tree.nodes.is_empty() {
        if !tree.leaves.is_empty() {
            leaves.push(0);
        }
        return leaves;
    }
    let mut stack = vec![BspChild::Node(0)];
    while let Some(child) = stack.pop() {
        match child {
            BspChild::Leaf(leaf) => leaves.push(leaf),
            BspChild::Node(node) => {
                let node = &tree.nodes[node];
                let distance = centre.dot(node.plane_normal) - node.plane_distance;
                if distance >= -radius {
                    stack.push(node.front.clone());
                }
                if distance <= radius {
                    stack.push(node.back.clone());
                }
            }
        }
    }
    leaves
}

/// Distance from `point` to the convex `polygon`, never above the exact
/// value by more than rounding: the plane distance when the projection is
/// not clearly outside an edge (a lower bound), else the nearest edge.
pub(super) fn point_polygon_distance(point: DVec3, polygon: &[DVec3]) -> f64 {
    let edge_distance = || {
        (0..polygon.len())
            .map(|i| {
                let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
                distance_to_segment(point, a, b)
            })
            .fold(f64::INFINITY, f64::min)
    };
    if polygon.len() < 3 {
        return edge_distance();
    }
    let mut normal = DVec3::ZERO;
    for i in 0..polygon.len() {
        let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
        normal += a.cross(b);
    }
    let Some(normal) = normal.try_normalize() else {
        return edge_distance();
    };
    let outside = (0..polygon.len()).any(|i| {
        let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
        let edge = b - a;
        // Inward is `normal × edge` for a counter-clockwise polygon about
        // `normal`, which Newell's normal makes it.
        let inward = normal.cross(edge);
        let scale = edge.length() * (point - a).length().max(1.0);
        inward.dot(point - a) < -1.0e-9 * scale
    });
    if outside {
        edge_distance()
    } else {
        (point - polygon[0]).dot(normal).abs()
    }
}

fn distance_to_segment(p: DVec3, a: DVec3, b: DVec3) -> f64 {
    let edge = b - a;
    let length_squared = edge.length_squared();
    let t = if length_squared > 0.0 {
        ((p - a).dot(edge) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + edge * t)
}
