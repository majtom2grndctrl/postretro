// Coverage proof per cluster: split its edges into pieces along one axis and match each piece one-for-one.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use glam::DVec3;

use super::UnprovenSpan;
use super::clusters::opposed;
use super::edges::{DirectedEdge, face_triangles};
use crate::geometry::GeometryResult;
use crate::ray_traversal::{distance_squared_to_triangle, triangle_points};

/// Whether a stretch of edge lies within τ of a partner's face.
pub(super) struct Measure<'a> {
    pub(super) geometry: &'a GeometryResult,
    /// (τ less the rounding margin)², so a computed distance that clears it
    /// proves the exact one is within τ.
    pub(super) limit_squared: f64,
    /// How far past its own end, along the axis, an edge may stand in as a
    /// partner: τ. Edge ends that meet at one corner project to slightly
    /// different axis points (rounding, or a crack's ends), leaving a stub
    /// only one of them reaches.
    pub(super) end_reach: f64,
}

impl Measure<'_> {
    /// Both `ends` lie within the limit of one triangle of `partner`'s face,
    /// tried from the triangle `partner` bounds. Distance to a triangle is
    /// convex along a segment, so the whole stretch between them does too.
    /// A degenerate triangle's distance is NaN and covers nothing.
    fn covers(&self, partner: &DirectedEdge, ends: [DVec3; 2]) -> bool {
        let within = |tri: usize| {
            let corners = triangle_points(self.geometry, tri).map(|c| c.as_dvec3());
            ends.iter()
                .all(|&p| distance_squared_to_triangle(p, corners) <= self.limit_squared)
        };
        within(partner.triangle)
            || face_triangles(&self.geometry.face_index_ranges[partner.face as usize])
                .any(|tri| tri != partner.triangle && within(tri))
    }

    /// `a` and `b` may pair on a piece: they run opposite ways, and each
    /// one's stretch there lies within τ of the other's face.
    fn pairs(
        &self,
        a: &DirectedEdge,
        a_ends: [DVec3; 2],
        b: &DirectedEdge,
        b_ends: [DVec3; 2],
    ) -> bool {
        opposed(a, b) && self.covers(b, a_ends) && self.covers(a, b_ends)
    }
}

/// What one cluster settles: its edges proven whole, and its unproven spans.
pub(super) struct Settled {
    pub(super) proven_edges: usize,
    pub(super) spans: Vec<UnprovenSpan>,
}

/// An edge placed on the cluster axis: its ends' axis coordinates.
struct Placed<'a> {
    edge: &'a DirectedEdge,
    t_from: f64,
    t_to: f64,
}

impl Placed<'_> {
    /// The edge's point whose axis coordinate is `t`, clamped to the edge:
    /// past an end, that end. Its ends map to its stored corners exactly,
    /// and the map is monotone, so consecutive breaks tile the edge.
    fn at(&self, t: f64) -> DVec3 {
        let (lo, hi) = self.span();
        let t = t.clamp(lo, hi);
        if t == self.t_from {
            return self.edge.from;
        }
        if t == self.t_to {
            return self.edge.to;
        }
        let s = (t - self.t_from) / (self.t_to - self.t_from);
        self.edge.from + (self.edge.to - self.edge.from) * s
    }

    fn span(&self) -> (f64, f64) {
        (self.t_from.min(self.t_to), self.t_from.max(self.t_to))
    }

    /// The unproven stretch between breaks `lo` and `hi`, `from` side first.
    fn unproven(&self, lo: f64, hi: f64) -> UnprovenSpan {
        let (a, b) = (self.at(lo), self.at(hi));
        let endpoints = if self.t_from <= self.t_to {
            [a, b]
        } else {
            [b, a]
        };
        UnprovenSpan::new(endpoints, self.edge)
    }
}

/// One piece's edges on one side of the matching: those running over it,
/// then those standing in from within reach of their ends. `placed` indices.
#[derive(Default, Clone)]
struct Side {
    present: Vec<usize>,
    near: Vec<usize>,
}

impl Side {
    fn all(&self) -> impl Iterator<Item = usize> + '_ {
        self.present.iter().chain(&self.near).copied()
    }
}

/// Settle one cluster (`members` ascending). The axis is its longest edge's
/// line; each edge maps onto it affinely, and the axis breaks at every
/// edge's projected ends. Between two breaks, each edge contributes its own
/// stretch, and edges running along the axis pair with edges running
/// against it only when each one's stretch lies within τ of the other's
/// face. An edge whose end lies within `end_reach` of the piece stands in
/// with that end, and an edge whose piece lies within `end_reach` of its own
/// end needs only a partner that pairs, so a stub where two meeting ends
/// land apart still closes. Every other edge running over the piece must be
/// covered by one one-for-one matching; one left out is unproven there,
/// as is a stub nothing pairs with. The axis only
/// shapes the pieces: every check reads the edges' own stretches, so a poor
/// axis can cost a pair and never prove one.
pub(super) fn settle(edges: &[DirectedEdge], members: &[usize], measure: &Measure<'_>) -> Settled {
    let reference = members
        .iter()
        .copied()
        .max_by(|&i, &j| {
            edges[i]
                .length()
                .total_cmp(&edges[j].length())
                .then(j.cmp(&i))
        })
        .expect("a cluster has an edge");
    let origin = edges[reference].from;
    let axis = edges[reference].direction();
    let placed: Vec<Placed<'_>> = members
        .iter()
        .map(|&i| {
            let edge = &edges[i];
            Placed {
                edge,
                t_from: (edge.from - origin).dot(axis),
                t_to: (edge.to - origin).dot(axis),
            }
        })
        .collect();

    let mut breaks: Vec<f64> = placed.iter().flat_map(|p| [p.t_from, p.t_to]).collect();
    breaks.sort_by(f64::total_cmp);
    breaks.dedup_by(|a, b| a == b);
    let first_at_or_after = |t: f64| breaks.partition_point(|&b| b < t);
    let first_after = |t: f64| breaks.partition_point(|&b| b <= t);
    let pieces = breaks.len().saturating_sub(1);

    let mut along: Vec<Side> = vec![Side::default(); pieces];
    let mut against: Vec<Side> = vec![Side::default(); pieces];
    let mut settled = Settled {
        proven_edges: 0,
        spans: Vec::new(),
    };
    for (index, p) in placed.iter().enumerate() {
        let (lo, hi) = p.span();
        let sides = if p.t_to > p.t_from {
            &mut along
        } else if p.t_to < p.t_from {
            &mut against
        } else {
            // Square to the axis: no piece carries it.
            settled
                .spans
                .push(UnprovenSpan::new([p.edge.from, p.edge.to], p.edge));
            continue;
        };
        let (start, end) = (first_at_or_after(lo), first_at_or_after(hi));
        for side in &mut sides[start..end] {
            side.present.push(index);
        }
        // Pieces wholly within reach past either end.
        let below = first_at_or_after(lo - measure.end_reach);
        let above = first_after(hi + measure.end_reach)
            .saturating_sub(1)
            .max(end);
        for piece in (below..start).chain(end..above) {
            sides[piece].near.push(index);
        }
    }

    // Unproven pieces per `placed` index, ascending.
    let mut unproven: Vec<Vec<usize>> = vec![Vec::new(); placed.len()];
    for piece in 0..pieces {
        let (lo, hi) = (breaks[piece], breaks[piece + 1]);
        let (left, right) = (&along[piece], &against[piece]);
        if left.present.is_empty() && right.present.is_empty() {
            continue;
        }
        let ends = |index: usize| [placed[index].at(lo), placed[index].at(hi)];
        let lefts: Vec<usize> = left.all().collect();
        let rights: Vec<usize> = right.all().collect();
        let pairs: Vec<Vec<bool>> = lefts
            .iter()
            .map(|&a| {
                let a_ends = ends(a);
                rights
                    .iter()
                    .map(|&b| measure.pairs(placed[a].edge, a_ends, placed[b].edge, ends(b)))
                    .collect()
            })
            .collect();
        // An edge whose piece lies within reach of its own end is a stub:
        // it needs a partner that pairs, not one of its own. Every other
        // edge running over the piece must be covered by one one-for-one
        // matching. A matching covering one side's such edges and one
        // covering the other's combine into one covering both
        // (Mendelsohn–Dulmage), so each side is matched on its own, with
        // stubs and stand-ins as partners only.
        let stub = |index: usize| {
            let (start, end) = placed[index].span();
            hi - start <= measure.end_reach || end - lo <= measure.end_reach
        };
        let settle_side = |present: &[usize],
                           partners: usize,
                           pair: &dyn Fn(usize, usize) -> bool| {
            let interior: Vec<usize> = (0..present.len()).filter(|&k| !stub(present[k])).collect();
            let adjacency: Vec<Vec<usize>> = interior
                .iter()
                .map(|&k| (0..partners).filter(|&o| pair(k, o)).collect())
                .collect();
            let matched = covered_by_matching(&adjacency, partners);
            let mut covered: Vec<bool> = (0..present.len())
                .map(|k| (0..partners).any(|o| pair(k, o)))
                .collect();
            for (&k, &m) in interior.iter().zip(&matched) {
                covered[k] = m;
            }
            covered
        };
        let left_covered = settle_side(&left.present, rights.len(), &|l, r| pairs[l][r]);
        let right_covered = settle_side(&right.present, lefts.len(), &|r, l| pairs[l][r]);
        for (slot, &index) in left.present.iter().enumerate() {
            if !left_covered[slot] {
                unproven[index].push(piece);
            }
        }
        for (slot, &index) in right.present.iter().enumerate() {
            if !right_covered[slot] {
                unproven[index].push(piece);
            }
        }
    }

    for (index, p) in placed.iter().enumerate() {
        if p.t_from == p.t_to {
            continue;
        }
        let runs = &unproven[index];
        if runs.is_empty() {
            settled.proven_edges += 1;
            continue;
        }
        let mut start = 0;
        while start < runs.len() {
            let mut end = start;
            while end + 1 < runs.len() && runs[end + 1] == runs[end] + 1 {
                end += 1;
            }
            settled
                .spans
                .push(p.unproven(breaks[runs[start]], breaks[runs[end] + 1]));
            start = end + 1;
        }
    }
    settled
}

/// Which left vertices a maximum matching covers, by Kuhn's augmenting
/// paths over `adjacency` (left → right slots, in order). The same input
/// covers the same vertices; an augmenting path never uncovers a left
/// vertex, so the count is the maximum.
fn covered_by_matching(adjacency: &[Vec<usize>], right_count: usize) -> Vec<bool> {
    fn augment(
        left: usize,
        adjacency: &[Vec<usize>],
        visited: &mut [bool],
        left_of: &mut [Option<usize>],
    ) -> bool {
        for &right in &adjacency[left] {
            if visited[right] {
                continue;
            }
            visited[right] = true;
            let free = match left_of[right] {
                None => true,
                Some(other) => augment(other, adjacency, visited, left_of),
            };
            if free {
                left_of[right] = Some(left);
                return true;
            }
        }
        false
    }

    let mut left_of = vec![None; right_count];
    (0..adjacency.len())
        .map(|left| {
            let mut visited = vec![false; right_count];
            augment(left, adjacency, &mut visited, &mut left_of)
        })
        .collect()
}
