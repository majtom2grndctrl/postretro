// Coverage proof seam overlaps: where two same-way edges on one line may both run over a stretch no longer than τ.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use std::collections::HashMap;

use glam::DVec3;

use super::clusters::opposed;
use super::edges::DirectedEdge;
use super::settle::{Measure, Placed};

/// A turning edge leaves the line when its direction is at least this far
/// off the line's, as a cosine (30°). A chain's next link runs along the
/// line and turns nowhere.
const MAX_TURN_COS: f64 = 0.866;

/// Each face's boundary edges by the corner they leave and the corner they
/// enter. A corner two of one face's edges leave, or enter, maps to none:
/// its turn is ambiguous, and no seam is read there.
pub(super) struct Turns {
    leaving: HashMap<(u32, [u64; 3]), Option<usize>>,
    entering: HashMap<(u32, [u64; 3]), Option<usize>>,
}

impl Turns {
    pub(super) fn new(edges: &[DirectedEdge]) -> Self {
        let mut turns = Self {
            leaving: HashMap::with_capacity(edges.len()),
            entering: HashMap::with_capacity(edges.len()),
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
        }
        turns
    }

    fn leaving(&self, face: u32, corner: DVec3) -> Option<usize> {
        self.leaving.get(&(face, bits(corner))).copied().flatten()
    }

    fn entering(&self, face: u32, corner: DVec3) -> Option<usize> {
        self.entering.get(&(face, bits(corner))).copied().flatten()
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
/// - (c) the edges where `x`'s face and `y`'s face turn off the line at
///   those corners run opposite ways within τ of each other: the seam whose
///   ends landing apart left the overlap.
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
    let (x_turn, y_turn) = (&measure.edges[x_turn], &measure.edges[y_turn]);
    turns_off(x_turn, x.edge) && turns_off(y_turn, y.edge) && seam(measure, x_turn, y_turn)
}

fn turns_off(turn: &DirectedEdge, edge: &DirectedEdge) -> bool {
    turn.direction().dot(edge.direction()).abs() <= MAX_TURN_COS
}

/// `leaving` and `entering`, which leave and enter the line, run opposite
/// ways, and over the shorter one's length from the line each lies within τ
/// of the other's face.
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
