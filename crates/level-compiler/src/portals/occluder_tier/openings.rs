// Openings in an occluder-tier piece: its interior fragments grouped through
// the edges they share, so each opening becomes its own runtime portal.
// See: context/plans/done/occluder-first-partition/plan.md §Block (Task 9), resolved

use glam::DVec3;

/// Fragment edges whose endpoints lie this close to one line, overlapping
/// along it by more than this, are shared. Far above clipping dust, far below
/// any authored gap.
const SHARED_EDGE_EPSILON: f64 = 1e-6;

/// Group `fragments`, coplanar convex polygons cut from one tier piece, into
/// openings: sets of fragments joined through edges they share over a
/// positive length. Below-tier planes cut one opening into fragments that
/// share their cut edges; solid between two openings leaves them sharing
/// none. Each opening lists fragment indices ascending, and openings follow
/// their lowest fragment index, so the grouping follows the fragments' order
/// alone.
pub(super) fn openings(fragments: &[Vec<DVec3>]) -> Vec<Vec<usize>> {
    let bounds: Vec<(DVec3, DVec3)> = fragments
        .iter()
        .map(|fragment| {
            let lo = fragment.iter().copied().fold(DVec3::INFINITY, DVec3::min);
            let hi = fragment
                .iter()
                .copied()
                .fold(DVec3::NEG_INFINITY, DVec3::max);
            (lo - SHARED_EDGE_EPSILON, hi + SHARED_EDGE_EPSILON)
        })
        .collect();

    // Sweep along x, so only fragments whose bounds meet are compared.
    let mut by_x: Vec<usize> = (0..fragments.len()).collect();
    by_x.sort_by(|&a, &b| bounds[a].0.x.total_cmp(&bounds[b].0.x).then(a.cmp(&b)));
    let mut parent: Vec<usize> = (0..fragments.len()).collect();
    for (k, &a) in by_x.iter().enumerate() {
        for &b in &by_x[k + 1..] {
            if bounds[b].0.x > bounds[a].1.x {
                break;
            }
            let meet = bounds[a].0.cmple(bounds[b].1).all() && bounds[b].0.cmple(bounds[a].1).all();
            if meet && share_an_edge(&fragments[a], &fragments[b]) {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                parent[ra.max(rb)] = ra.min(rb);
            }
        }
    }

    let mut openings: Vec<Vec<usize>> = Vec::new();
    let mut opening_of_root = vec![usize::MAX; fragments.len()];
    for fragment in 0..fragments.len() {
        let r = root(&mut parent, fragment);
        if opening_of_root[r] == usize::MAX {
            opening_of_root[r] = openings.len();
            openings.push(Vec::new());
        }
        openings[opening_of_root[r]].push(fragment);
    }
    openings
}

fn root(parent: &mut [usize], mut node: usize) -> usize {
    while parent[node] != node {
        parent[node] = parent[parent[node]];
        node = parent[node];
    }
    node
}

/// True when an edge of `a` and an edge of `b` lie on one line and overlap
/// along it by more than the epsilon. Corners alone do not join.
fn share_an_edge(a: &[DVec3], b: &[DVec3]) -> bool {
    let edges = |polygon: &[DVec3]| -> Vec<(DVec3, DVec3)> {
        (0..polygon.len())
            .map(|i| (polygon[i], polygon[(i + 1) % polygon.len()]))
            .collect()
    };
    let b_edges = edges(b);
    edges(a).into_iter().any(|(a0, a1)| {
        let length = (a1 - a0).length();
        if length <= SHARED_EDGE_EPSILON {
            return false;
        }
        let along = (a1 - a0) / length;
        b_edges.iter().any(|&(b0, b1)| {
            let off_line = |p: DVec3| {
                let rel = p - a0;
                (rel - along * rel.dot(along)).length() > SHARED_EDGE_EPSILON
            };
            if off_line(b0) || off_line(b1) {
                return false;
            }
            let (t0, t1) = ((b0 - a0).dot(along), (b1 - a0).dot(along));
            t0.max(t1).min(length) - t0.min(t1).max(0.0) > SHARED_EDGE_EPSILON
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<DVec3> {
        vec![
            DVec3::new(x0, y0, 0.0),
            DVec3::new(x1, y0, 0.0),
            DVec3::new(x1, y1, 0.0),
            DVec3::new(x0, y1, 0.0),
        ]
    }

    #[test]
    fn fragments_sharing_a_cut_edge_join_and_solid_between_parts_them() {
        let fragments = vec![
            // One window cut in three by below-tier planes, one fragment
            // meeting the other two along part of its edge (a T-junction).
            square(0.0, 0.0, 1.0, 2.0),
            square(1.0, 0.0, 1.5, 1.0),
            square(1.0, 1.0, 1.5, 2.0),
            // A second window past a 1 m pier, and a third touching the
            // second only at a corner.
            square(2.5, 0.0, 4.0, 2.0),
            square(4.0, 2.0, 5.0, 3.0),
        ];
        assert_eq!(openings(&fragments), vec![vec![0, 1, 2], vec![3], vec![4]]);
    }
}
