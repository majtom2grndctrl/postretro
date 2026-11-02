// Coverage proof seam overlaps: where two same-way edges on one line may both run over a stretch no longer than τ.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use std::collections::HashMap;

use glam::DVec3;

use super::clusters::opposed;
use super::edges::DirectedEdge;
use super::settle::{Measure, Placed};

/// Most faces a closing fan may pass through between the two overlapping
/// edges' faces. A corner where several brushes meet holds a few faces
/// between any two; the gate maps need one. A corner needing more stays
/// unproven: a false hole, never a missed one.
const FAN_CAP: usize = 4;

/// Most candidate fan edges one seam may try, so a corner crowded with
/// edges costs bounded time. Running out leaves the piece unproven.
const SEARCH_CAP: usize = 64;

/// Each face's boundary edges by the corner they leave and the corner they
/// enter. A corner two of one face's edges leave, or enter, maps to none:
/// its turn is ambiguous, and no seam is read there. Every edge is also
/// filed by the grid cell its end corner lies in, for the fan search.
pub(super) struct Turns {
    leaving: HashMap<(u32, [u64; 3]), Option<usize>>,
    entering: HashMap<(u32, [u64; 3]), Option<usize>>,
    by_end: HashMap<[i64; 3], Vec<usize>>,
    /// Grid cell edge: at least τ, so every corner within τ of a point lies
    /// in that point's cell or a neighbour.
    cell: f64,
}

impl Turns {
    pub(super) fn new(edges: &[DirectedEdge], cell: f64) -> Self {
        let mut turns = Self {
            leaving: HashMap::with_capacity(edges.len()),
            entering: HashMap::with_capacity(edges.len()),
            by_end: HashMap::new(),
            cell,
        };
        for (index, edge) in edges.iter().enumerate() {
            for (map, corner) in [
                (&mut turns.leaving, edge.from),
                (&mut turns.entering, edge.to),
            ] {
                map.entry((edge.face, bits(corner)))
                    .and_modify(|slot| *slot = None)
                    .or_insert(Some(index));
            }
            let key = turns.cell_of(edge.to);
            turns.by_end.entry(key).or_default().push(index);
        }
        turns
    }

    fn leaving(&self, face: u32, corner: DVec3) -> Option<usize> {
        self.leaving.get(&(face, bits(corner))).copied().flatten()
    }

    fn entering(&self, face: u32, corner: DVec3) -> Option<usize> {
        self.entering.get(&(face, bits(corner))).copied().flatten()
    }

    fn cell_of(&self, p: DVec3) -> [i64; 3] {
        (p / self.cell).floor().to_array().map(|c| c as i64)
    }

    /// Edges ending in `p`'s grid cell or a neighbour, in a fixed order.
    fn ending_near(&self, p: DVec3) -> impl Iterator<Item = usize> + '_ {
        let [x, y, z] = self.cell_of(p);
        (-1..=1)
            .flat_map(move |dx| (-1..=1).flat_map(move |dy| (-1..=1).map(move |dz| [dx, dy, dz])))
            .filter_map(move |[dx, dy, dz]| self.by_end.get(&[x + dx, y + dy, z + dz]))
            .flatten()
            .copied()
    }
}

/// Corners are widened from stored `f32`s with `-0.0` read as `0.0`, so
/// equal bits are an equal corner.
fn bits(p: DVec3) -> [u64; 3] {
    p.to_array().map(f64::to_bits)
}

/// `x` and `y`, both running the same way over the piece `lo`..`hi`,
/// overlap there at a seam, so one of them may go unmatched while the other
/// is matched. Symmetric: either may end where the other starts.
pub(super) fn seam_overlap(
    measure: &Measure<'_>,
    x: &Placed<'_>,
    y: &Placed<'_>,
    lo: f64,
    hi: f64,
) -> bool {
    overlap_at(measure, x, y, lo, hi) || overlap_at(measure, y, x, lo, hi)
}

/// `x`'s end and `y`'s start bound the piece, and explain it:
/// - (a) the piece lies between the two along the axis, and both edges run
///   over it;
/// - (b) the two corners lie within τ of each other in 3D, and each edge's
///   stretch on the piece within τ of its own corner, so an axis that
///   foreshortens an edge cannot stretch the overlap. Each stretch also lies
///   within τ of the other's face: the two cover the same stretch;
/// - (c) the surface closes around the corner between the edges where `x`'s
///   face and `y`'s face turn off the line (`closes`): two copies of one
///   corner landing apart left the overlap.
fn overlap_at(measure: &Measure<'_>, x: &Placed<'_>, y: &Placed<'_>, lo: f64, hi: f64) -> bool {
    let (end, start) = (x.edge.to, y.edge.from);
    if lo < x.t_to.min(y.t_from) || hi > x.t_to.max(y.t_from) {
        return false;
    }
    let near = |p: DVec3, q: DVec3| (p - q).length_squared() <= measure.limit_squared;
    let (x_ends, y_ends) = ([x.at(lo), x.at(hi)], [y.at(lo), y.at(hi)]);
    let short = near(end, start)
        && x_ends.iter().all(|&p| near(p, end))
        && y_ends.iter().all(|&p| near(p, start));
    if !short || !measure.covers(y.edge, x_ends) || !measure.covers(x.edge, y_ends) {
        return false;
    }
    let (Some(x_turn), Some(y_turn)) = (
        measure.turns.leaving(x.edge.face, end),
        measure.turns.entering(y.edge.face, start),
    ) else {
        return false;
    };
    let corner = Corner {
        measure,
        line: x.edge,
        ends: [end, start],
        faces: [x.edge.face, y.edge.face],
        entering: &measure.edges[y_turn],
    };
    let x_turn = &measure.edges[x_turn];
    let mut budget = SEARCH_CAP;
    corner.off_line(x_turn.to)
        && corner.off_line(corner.entering.from)
        && corner.closes(x_turn, &mut Vec::new(), &mut budget)
}

/// The corner where `x`'s end and `y`'s start land apart.
struct Corner<'m, 'a> {
    measure: &'m Measure<'a>,
    line: &'m DirectedEdge,
    /// `x`'s end and `y`'s start.
    ends: [DVec3; 2],
    /// `x`'s and `y`'s faces, which no fan face may be.
    faces: [u32; 2],
    /// `y`'s turning edge, which the fan must close on.
    entering: &'m DirectedEdge,
}

impl Corner<'_, '_> {
    /// `p` lies farther off `x`'s line than clustering links an edge to
    /// it: an edge ending there turns off the line. A chain's next link,
    /// or a fan edge along the line, stays on it.
    fn off_line(&self, p: DVec3) -> bool {
        let v = p - self.line.from;
        let axis = self.line.direction();
        (v - axis * v.dot(axis)).length() > self.measure.link_radius
    }

    fn at_corner(&self, p: DVec3) -> bool {
        self.ends
            .iter()
            .all(|&c| (p - c).length_squared() <= self.measure.limit_squared)
    }

    /// The surface closes from `leaving` round the corner to `entering`:
    /// directly, as a seam (a fan of no faces), or through a fan of up to
    /// `FAN_CAP` other faces. Each link is an opposed pair within τ of each
    /// other, and each fan face's in-edge and out-edge meet at one of its
    /// corners within τ of both ends, both off the line. A fan face's turn
    /// must be its own only edge in and out there.
    fn closes(&self, leaving: &DirectedEdge, fan: &mut Vec<u32>, budget: &mut usize) -> bool {
        if seam(self.measure, leaving, self.entering) {
            return true;
        }
        if fan.len() == FAN_CAP {
            return false;
        }
        for index in self.measure.turns.ending_near(self.ends[0]) {
            if *budget == 0 {
                return false;
            }
            *budget -= 1;
            let edge = &self.measure.edges[index];
            let face = edge.face;
            if self.faces.contains(&face) || fan.contains(&face) || !self.at_corner(edge.to) {
                continue;
            }
            let turns = &self.measure.turns;
            if turns.entering(face, edge.to) != Some(index) {
                continue;
            }
            let Some(out) = turns.leaving(face, edge.to) else {
                continue;
            };
            let out = &self.measure.edges[out];
            if !self.off_line(edge.from)
                || !self.off_line(out.to)
                || !seam(self.measure, leaving, edge)
            {
                continue;
            }
            fan.push(face);
            if self.closes(out, fan, budget) {
                return true;
            }
            fan.pop();
        }
        false
    }
}

/// `leaving` and `entering` run opposite ways, and over the shorter one's
/// length from its corner each lies within τ of the other's face.
fn seam(measure: &Measure<'_>, leaving: &DirectedEdge, entering: &DirectedEdge) -> bool {
    if !opposed(leaving, entering) {
        return false;
    }
    let reach = leaving.length().min(entering.length());
    let leaving_ends = [
        leaving.from,
        if reach == leaving.length() {
            leaving.to
        } else {
            leaving.from + leaving.direction() * reach
        },
    ];
    let entering_ends = [
        if reach == entering.length() {
            entering.from
        } else {
            entering.to - entering.direction() * reach
        },
        entering.to,
    ];
    measure.covers(entering, leaving_ends) && measure.covers(leaving, entering_ends)
}
