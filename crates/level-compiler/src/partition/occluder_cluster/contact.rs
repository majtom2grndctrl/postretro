// Brush contact for occluder clusters: two convex brushes touch when their
// separating-axis distance is within the CSG contact epsilon.
// See: context/lib/resource_management.md §5.2 (Contact)

use glam::DVec3;

use crate::partition::brush_bsp::REGION_CLASSIFICATION_EPSILON;
use crate::partition::region_polytope::RegionPolytope;
use crate::partition::types::Aabb;

/// Directions this close to parallel are one candidate axis.
const PARALLEL_COS: f64 = 1.0 - 1e-9;

/// A brush's convex hull as the separating-axis test reads it.
pub(super) struct Hull {
    pub(super) vertices: Vec<DVec3>,
    normals: Vec<DVec3>,
    edges: Vec<DVec3>,
    aabb: Aabb,
}

impl Hull {
    pub(super) fn of(polytope: &RegionPolytope) -> Self {
        let mut vertices: Vec<DVec3> = Vec::new();
        let mut normals: Vec<DVec3> = Vec::new();
        let mut edges: Vec<DVec3> = Vec::new();
        for (normal, _, winding) in polytope.facets() {
            push_direction(&mut normals, normal);
            for (i, &a) in winding.iter().enumerate() {
                vertices.push(a);
                let b = winding[(i + 1) % winding.len()];
                if let Some(edge) = (b - a).try_normalize() {
                    push_direction(&mut edges, edge);
                }
            }
        }
        Self {
            vertices,
            normals,
            edges,
            aabb: polytope.vertex_aabb(),
        }
    }

    fn projection(&self, axis: DVec3) -> (f64, f64) {
        self.vertices
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), vertex| {
                let p = vertex.dot(axis);
                (lo.min(p), hi.max(p))
            })
    }
}

fn push_direction(directions: &mut Vec<DVec3>, direction: DVec3) {
    if !directions
        .iter()
        .any(|existing| existing.dot(direction).abs() >= PARALLEL_COS)
    {
        directions.push(direction);
    }
}

/// True when `a` and `b` overlap or lie within the contact epsilon of each
/// other on every candidate separating axis: both hulls' face normals and
/// the cross products of their edge directions. Symmetric in its arguments.
pub(super) fn touching(a: &Hull, b: &Hull) -> bool {
    let separated = |axis: DVec3| {
        let (a_lo, a_hi) = a.projection(axis);
        let (b_lo, b_hi) = b.projection(axis);
        (b_lo - a_hi).max(a_lo - b_hi) > REGION_CLASSIFICATION_EPSILON
    };
    if a.normals.iter().chain(&b.normals).any(|&n| separated(n)) {
        return false;
    }
    for &ea in &a.edges {
        for &eb in &b.edges {
            if let Some(axis) = ea.cross(eb).try_normalize()
                && separated(axis)
            {
                return false;
            }
        }
    }
    true
}

/// Every touching pair among `hulls`, as index pairs `(i, j)` with `i < j`.
/// A sweep over AABBs padded by the epsilon finds the candidates.
pub(super) fn touching_pairs(hulls: &[Hull]) -> Vec<(usize, usize)> {
    let pad = REGION_CLASSIFICATION_EPSILON;
    let mut order: Vec<usize> = (0..hulls.len()).collect();
    order.sort_by(|&i, &j| {
        hulls[i]
            .aabb
            .min
            .x
            .total_cmp(&hulls[j].aabb.min.x)
            .then(i.cmp(&j))
    });
    let mut pairs = Vec::new();
    for (k, &i) in order.iter().enumerate() {
        let a = &hulls[i].aabb;
        for &j in &order[k + 1..] {
            let b = &hulls[j].aabb;
            if b.min.x > a.max.x + pad {
                break;
            }
            let overlaps = (0..3)
                .all(|axis| b.min[axis] <= a.max[axis] + pad && a.min[axis] <= b.max[axis] + pad);
            if overlaps && touching(&hulls[i], &hulls[j]) {
                pairs.push((i.min(j), i.max(j)));
            }
        }
    }
    pairs
}
