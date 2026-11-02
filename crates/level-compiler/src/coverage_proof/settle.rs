// Coverage proof per cluster: split its edges into pieces along one axis and match each piece one-for-one.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)
//
// Capped stand-ins, and why they are sound:
// - Two copies of one corner farther apart than δ leave a stretch one edge
//   runs over and its partner stops short of. An edge whose welded end lies
//   within τ (axis) of such a piece may stand in there with its stored end:
//   the pair is checked in 3D like any other, so the stretch lies within τ
//   of the stand-in's face and its end within τ of the partner's.
// - Uncapped, a chain of collinear edges stands in on both sides of each
//   link, so one direction supplies two partners on a piece where it has
//   one edge. The cap stops that: stand-ins may lift a direction's count on
//   a piece only to the most edges of that direction present on any piece
//   within τ. They fill a dip, where one edge has ended and the next not yet
//   begun; a direction short on every piece within τ, as a dropped face
//   leaves it, stays short, and so does a same-way surplus (an overlap).
// - Both sides' caps hold in one matching: each side's run covers its
//   present edges plus dummies that hold back the other side's stand-ins
//   past its cap, and two matchings in one bipartite graph covering those
//   two sets combine into one covering both (Mendelsohn–Dulmage). No pair
//   joins two stand-ins.

use glam::DVec3;

use super::UnprovenSpan;
use super::clusters::opposed;
use super::edges::{DirectedEdge, face_triangles};
use super::face_boxes::FaceBox;
use super::matching::cover;
use crate::geometry::GeometryResult;
use crate::ray_traversal::{distance_squared_to_triangle, triangle_points};

/// Whether a stretch of edge lies within τ of a partner's face.
pub(super) struct Measure<'a> {
    pub(super) geometry: &'a GeometryResult,
    /// Each face's box grown by τ (`face_boxes`).
    pub(super) boxes: &'a [Option<FaceBox>],
    /// (τ less the rounding margin)², so a computed distance that clears it
    /// proves the exact one is within τ.
    pub(super) limit_squared: f64,
    /// How far past its own welded end, along the axis, an edge may stand
    /// in: τ. Axis units only offer the candidate; the pair is checked in 3D
    /// at the edge's stored end.
    pub(super) stand_in_reach: f64,
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

    /// `a` and `b` may pair on some piece: they run opposite ways, and each
    /// one's stored segment meets the other's face box. A necessary
    /// condition only, so it prunes pairs and never proves one.
    fn may_pair(&self, a: &DirectedEdge, b: &DirectedEdge) -> bool {
        let meets = |edge: &DirectedEdge, face: u32| {
            self.boxes[face as usize]
                .as_ref()
                .is_none_or(|bounds| bounds.meets(edge.from, edge.to))
        };
        opposed(a, b) && meets(a, b.face) && meets(b, a.face)
    }

    /// `a` and `b` pair on a piece: they run opposite ways, and each
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

/// An edge placed on the cluster axis: its welded ends' axis coordinates.
struct Placed<'a> {
    edge: &'a DirectedEdge,
    t_from: f64,
    t_to: f64,
}

impl Placed<'_> {
    /// The edge's stored point at the fraction of its welded span that `t`
    /// marks, clamped to the edge: past an end, that end. Its welded ends
    /// map to its stored corners exactly, and the map is monotone, so
    /// consecutive breaks tile the stored edge. Welding moves where an edge
    /// breaks, never where it is measured.
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

/// Settle one cluster (`members` ascending). The axis is its longest edge's
/// welded line; each edge maps onto it affinely by its welded ends, and the
/// axis breaks at every edge's projected welded ends. Between two breaks,
/// every edge running over the piece must be matched one-for-one, by an
/// edge running over it the other way or by a capped stand-in: a pair
/// needs each one's stored stretch within τ of the other's face. An edge
/// left out is unproven there. The axis only shapes the
/// pieces: every check reads the edges' stored stretches in 3D, so a poor
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
    let origin = edges[reference].weld_from;
    let axis = edges[reference].direction();
    let placed: Vec<Placed<'_>> = members
        .iter()
        .map(|&i| {
            let edge = &edges[i];
            Placed {
                edge,
                t_from: (edge.weld_from - origin).dot(axis),
                t_to: (edge.weld_to - origin).dot(axis),
            }
        })
        .collect();

    let mut breaks: Vec<f64> = placed.iter().flat_map(|p| [p.t_from, p.t_to]).collect();
    breaks.sort_by(f64::total_cmp);
    breaks.dedup_by(|a, b| a == b);
    let first_at_or_after = |t: f64| breaks.partition_point(|&b| b < t);
    let first_after = |t: f64| breaks.partition_point(|&b| b <= t);
    let pieces = breaks.len().saturating_sub(1);
    let reach = measure.stand_in_reach;

    // Per side (along, against) and piece, the `placed` indices running over
    // it, and those standing in from within reach past their ends.
    let mut present: [Vec<Vec<usize>>; 2] = [vec![Vec::new(); pieces], vec![Vec::new(); pieces]];
    let mut near: [Vec<Vec<usize>>; 2] = [vec![Vec::new(); pieces], vec![Vec::new(); pieces]];
    let mut settled = Settled {
        proven_edges: 0,
        spans: Vec::new(),
    };
    for (index, p) in placed.iter().enumerate() {
        let (lo, hi) = p.span();
        let side = if p.t_to > p.t_from {
            0
        } else if p.t_to < p.t_from {
            1
        } else {
            // Square to the axis: no piece carries it.
            settled
                .spans
                .push(UnprovenSpan::new([p.edge.from, p.edge.to], p.edge));
            continue;
        };
        let (start, end) = (first_at_or_after(lo), first_at_or_after(hi));
        for on in &mut present[side][start..end] {
            on.push(index);
        }
        // Pieces wholly within reach past either end.
        let below = first_at_or_after(lo - reach);
        let above = first_after(hi + reach).saturating_sub(1).max(end);
        for piece in (below..start).chain(end..above) {
            near[side][piece].push(index);
        }
    }

    // Per side and piece, the most edges of that side present on any piece
    // within reach: what its stand-ins may lift it to.
    let window: [Vec<usize>; 2] = [0, 1].map(|side| {
        (0..pieces)
            .map(|piece| {
                let first = breaks[1..].partition_point(|&b| b < breaks[piece] - reach);
                let last = breaks[..pieces].partition_point(|&b| b <= breaks[piece + 1] + reach);
                (first..last)
                    .map(|q| present[side][q].len())
                    .max()
                    .unwrap_or(0)
            })
            .collect()
    });

    // Per edge, the edges running the other way it may pair with on some
    // piece.
    let mut partners: Vec<Vec<usize>> = vec![Vec::new(); placed.len()];
    for a in 0..placed.len() {
        for b in a + 1..placed.len() {
            let (ta, tb) = (&placed[a], &placed[b]);
            let opposite = (ta.t_to > ta.t_from && tb.t_to < tb.t_from)
                || (ta.t_to < ta.t_from && tb.t_to > tb.t_from);
            if opposite && measure.may_pair(ta.edge, tb.edge) {
                partners[a].push(b);
                partners[b].push(a);
            }
        }
    }

    // Unproven pieces per `placed` index, ascending. Each side is covered
    // by its own run, which starts from the pairs that side's run matched on
    // the piece before, which mostly carry over.
    let mut unproven: Vec<Vec<usize>> = vec![Vec::new(); placed.len()];
    let mut carried: [Vec<(usize, usize)>; 2] = [Vec::new(), Vec::new()];
    for piece in 0..pieces {
        let (lo, hi) = (breaks[piece], breaks[piece + 1]);
        if present.iter().all(|side| side[piece].is_empty()) {
            continue;
        }
        let ends = |index: usize| [placed[index].at(lo), placed[index].at(hi)];
        for own in 0..2 {
            let other = 1 - own;
            let mine = &present[own][piece];
            let (theirs, standing) = (&present[other][piece], &near[other][piece]);
            let slot_of = |index: usize| {
                theirs.binary_search(&index).ok().or_else(|| {
                    standing
                        .binary_search(&index)
                        .ok()
                        .map(|k| theirs.len() + k)
                })
            };
            let right = theirs.len() + standing.len();
            // Dummies hold back every stand-in past the cap.
            let cap = window[other][piece].saturating_sub(theirs.len());
            let dummies = standing.len().saturating_sub(cap);
            let candidates: Vec<Vec<usize>> = (0..dummies)
                .map(|_| (theirs.len()..right).collect())
                .chain(
                    mine.iter()
                        .map(|&x| partners[x].iter().filter_map(|&y| slot_of(y)).collect()),
                )
                .collect();
            let start: Vec<(usize, usize)> = carried[own]
                .iter()
                .filter_map(|&(x, y)| Some((dummies + mine.binary_search(&x).ok()?, slot_of(y)?)))
                .collect();
            let right_index = |r: usize| {
                if r < theirs.len() {
                    theirs[r]
                } else {
                    standing[r - theirs.len()]
                }
            };
            let cover = cover(&candidates, right, dummies, &start, |l, r| {
                if l < dummies {
                    return true;
                }
                let (x, y) = (mine[l - dummies], right_index(r));
                measure.pairs(placed[x].edge, ends(x), placed[y].edge, ends(y))
            });
            carried[own] = (dummies..candidates.len())
                .filter_map(|l| Some((mine[l - dummies], right_index(cover.partner[l]?))))
                .collect();
            for l in cover.uncovered {
                unproven[mine[l - dummies]].push(piece);
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
