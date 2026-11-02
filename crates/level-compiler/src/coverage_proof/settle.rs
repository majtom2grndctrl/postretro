// Coverage proof per cluster: split its edges into pieces along one axis and match each piece one-for-one.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use glam::DVec3;

use super::UnprovenSpan;
use super::clusters::opposed;
use super::edges::{DirectedEdge, face_triangles};
use super::matching::PieceGraph;
use super::seams::{Turns, seam_overlap};
use crate::geometry::GeometryResult;
use crate::ray_traversal::{distance_squared_to_triangle, triangle_points};

/// Whether a stretch of edge lies within τ of a partner's face.
pub(super) struct Measure<'a> {
    pub(super) geometry: &'a GeometryResult,
    pub(super) edges: &'a [DirectedEdge],
    pub(super) turns: &'a Turns,
    /// (τ less the rounding margin)², so a computed distance that clears it
    /// proves the exact one is within τ.
    pub(super) limit_squared: f64,
    /// How far past its own end, along the axis, an edge may stand in as a
    /// partner: τ. Edge ends that meet at one corner project to slightly
    /// different axis points (rounding, or a crack's ends), leaving a stretch
    /// only one of them reaches. Axis units only offer the candidate: a stand-in
    /// pairs only when its end point lies within τ of the partner's face and
    /// the partner's whole stretch within τ of its face, in 3D, and it is used
    /// up like any partner.
    pub(super) end_reach: f64,
}

impl Measure<'_> {
    /// Both `ends` lie within the limit of one triangle of `partner`'s face,
    /// tried from the triangle `partner` bounds. Distance to a triangle is
    /// convex along a segment, so the whole stretch between them does too.
    /// A degenerate triangle's distance is NaN and covers nothing.
    pub(super) fn covers(&self, partner: &DirectedEdge, ends: [DVec3; 2]) -> bool {
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
pub(super) struct Placed<'a> {
    pub(super) edge: &'a DirectedEdge,
    pub(super) t_from: f64,
    pub(super) t_to: f64,
}

impl Placed<'_> {
    /// The edge's point whose axis coordinate is `t`, clamped to the edge:
    /// past an end, that end. Its ends map to its stored corners exactly,
    /// and the map is monotone, so consecutive breaks tile the edge.
    pub(super) fn at(&self, t: f64) -> DVec3 {
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
/// with that end. Every edge running over the piece must be covered by one
/// one-for-one matching, save one of two same-way edges overlapping at a
/// seam (`seams`); one left out is unproven there. The axis only shapes the
/// pieces: every check reads the edges' own stretches in 3D, so a poor axis
/// can cost a pair and never prove one.
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
        let sides = [&along[piece], &against[piece]];
        if sides.iter().all(|side| side.present.is_empty()) {
            continue;
        }
        let ends = |index: usize| [placed[index].at(lo), placed[index].at(hi)];
        let [lefts, rights] = sides.map(|side| side.all().collect::<Vec<usize>>());
        let graph = PieceGraph {
            pairs: lefts
                .iter()
                .map(|&a| {
                    let a_ends = ends(a);
                    rights
                        .iter()
                        .map(|&b| measure.pairs(placed[a].edge, a_ends, placed[b].edge, ends(b)))
                        .collect()
                })
                .collect(),
            required: sides.map(|side| side.present.len()),
            total: [lefts.len(), rights.len()],
        };
        let mut uncovered = graph.uncovered();
        if uncovered.iter().any(|slots| !slots.is_empty()) {
            let seams = sides.map(|side| seam_pairs(measure, &placed, &side.present, lo, hi));
            let proven = graph.proven_with_seams([&seams[0], &seams[1]]);
            if proven {
                uncovered = [Vec::new(), Vec::new()];
            }
        }
        for (side, slots) in sides.iter().zip(uncovered) {
            for slot in slots {
                unproven[side.present[slot]].push(piece);
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

/// The pairs of `present` slots that overlap at a seam on the piece.
fn seam_pairs(
    measure: &Measure<'_>,
    placed: &[Placed<'_>],
    present: &[usize],
    lo: f64,
    hi: f64,
) -> Vec<(usize, usize)> {
    let mut seams = Vec::new();
    for a in 0..present.len() {
        for b in a + 1..present.len() {
            if seam_overlap(measure, &placed[present[a]], &placed[present[b]], lo, hi) {
                seams.push((a, b));
            }
        }
    }
    seams
}
