// Coverage proof: every traced face edge matched one-for-one, within the tracer's τ, by an edge running the other way.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)
//
// Why a proven span is sound, in the terms of step 12's contract:
// - Edges come from the stored `f32` triangles the bake BVH holds, widened
//   to `f64`; a fan diagonal is matched inside its own face and left out.
// - Each cluster of near-collinear opposed edges is cut into pieces at every
//   edge's projection onto one axis (`settle`). An edge's pieces are its own
//   sub-segments between computed points that run monotonically from one
//   stored corner to the other, so they tile it.
// - A pair is proven on a piece only when both ends of each one's piece lie
//   within τ of a single triangle of the other's face. Distance to a
//   triangle is convex along a segment, so the whole piece is too. Pieces
//   break only at edge ends: a piece no single triangle covers stays
//   unproven rather than splitting where the nearest face changes.
// - The limit is τ less a margin for `f64` rounding of the evaluated points
//   and distances, scaled by the geometry's extent: rounding counts toward a
//   hole, and no quantum merges edges, so the proof holds at any coordinate.
// - Every edge running over a piece enters one maximum bipartite matching,
//   which pairs edges one-for-one whatever the input order (edges are
//   sorted by geometry first); an edge it leaves out is an unproven span.
// - Edge ends meeting at one point land apart along their line (rounding,
//   or a crack's ends). Where they leave a gap, an edge stands in with its
//   end point on pieces within τ past that end, and is used up like any
//   partner. Where they overlap, two same-way edges both run over the
//   stretch: one may go unmatched only at a seam (`seams`), an overlap no
//   longer than τ in 3D between one edge's end and the other's start, with
//   the edges where their faces turn off the line running opposite ways
//   within τ of each other. A chain of short edges turns nowhere, so it
//   excuses nothing.

use std::time::{Duration, Instant};

use glam::DVec3;
use rayon::prelude::*;

use crate::geometry::GeometryResult;
use crate::partition::{OPEN_EDGE_SAMPLE_CAP, OpenEdge};
use crate::ray_traversal::TRACER_TOLERANCE_METERS;

mod clusters;
mod edges;
mod matching;
mod seams;
mod settle;

use edges::DirectedEdge;
use seams::Turns;
use settle::{Measure, Settled};

/// Edges link into one cluster when each overlaps the other within this
/// many τ of its line, room for every crease and T-junction partner, which
/// lie within τ of each other. A partner outside it costs a pair, never
/// proves one.
const LINK_RADIUS_TAUS: f64 = 2.0;

/// The rounding margin, in `f64` epsilons of the geometry's extent: well
/// above the few roundings an evaluated point and its distance to a
/// triangle take.
const ROUNDING_EPSILONS: f64 = 256.0;

/// A stretch of a face edge no edge running the other way matches within τ:
/// a hole.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UnprovenSpan {
    /// World-space ends (engine space, meters), in the edge's direction.
    pub(crate) endpoints: [DVec3; 2],
    pub(crate) midpoint: DVec3,
    pub(crate) length: f64,
    /// The edge's face, an index into the proven geometry's faces.
    pub(crate) face: u32,
    /// The face's source brush.
    pub(crate) brush: Option<u32>,
}

impl UnprovenSpan {
    fn new(endpoints: [DVec3; 2], edge: &DirectedEdge) -> Self {
        Self {
            endpoints,
            midpoint: (endpoints[0] + endpoints[1]) * 0.5,
            length: (endpoints[1] - endpoints[0]).length(),
            face: edge.face,
            brush: edge.brush,
        }
    }

    /// The span as the watertightness check's open-edge location, for
    /// `pipeline::watertight_open_edge_provenance`. `None` without a brush.
    pub(crate) fn open_edge(&self) -> Option<OpenEdge> {
        Some(OpenEdge {
            midpoint: self.midpoint,
            brush_index: self.brush? as usize,
        })
    }
}

/// What the coverage proof found over one geometry.
#[derive(Debug)]
pub(crate) struct CoverageReport {
    /// Directed boundary edges the proof examined.
    pub(crate) edges: usize,
    /// Of those, the edges proven along their whole length.
    pub(crate) edges_proven: usize,
    /// Every unproven span, ordered by midpoint, then ends and brush.
    pub(crate) unproven: Vec<UnprovenSpan>,
    pub(crate) elapsed: Duration,
}

impl CoverageReport {
    /// Total length of the unproven spans, in meters.
    pub(crate) fn unproven_length(&self) -> f64 {
        self.unproven.iter().map(|span| span.length).sum()
    }

    /// The first unproven spans in report order, up to the watertightness
    /// check's sample cap.
    pub(crate) fn samples(&self) -> &[UnprovenSpan] {
        &self.unproven[..self.unproven.len().min(OPEN_EDGE_SAMPLE_CAP)]
    }
}

/// Prove that `geometry`'s faces close within τ: along every edge line, each
/// directed edge stretch matched one-for-one by an edge running the other
/// way, each within τ of the other's face. `geometry` is the set the
/// lightmap walk's BVH holds (after the oversize-face cut);
/// `face_brushes` is its face → brush table.
pub(crate) fn prove_coverage(
    geometry: &GeometryResult,
    face_brushes: &[Option<u32>],
) -> CoverageReport {
    let started = Instant::now();
    let tau = f64::from(TRACER_TOLERANCE_METERS);
    let extent = geometry
        .geometry
        .vertices
        .iter()
        .flat_map(|v| v.position)
        .fold(0.0f64, |m, c| m.max(f64::from(c).abs()));
    let limit = tau - ROUNDING_EPSILONS * f64::EPSILON * (1.0 + extent);
    let edges = edges::boundary_edges(geometry, face_brushes);
    let turns = Turns::new(&edges);
    let measure = Measure {
        geometry,
        edges: &edges,
        turns: &turns,
        limit_squared: limit * limit,
        end_reach: tau,
    };
    let clusters = clusters::clusters(&edges, LINK_RADIUS_TAUS * tau);
    let settled: Vec<Settled> = clusters
        .par_iter()
        .map(|members| settle::settle(&edges, members, &measure))
        .collect();

    let mut report = CoverageReport {
        edges: edges.len(),
        edges_proven: 0,
        unproven: Vec::new(),
        elapsed: Duration::ZERO,
    };
    for cluster in settled {
        report.edges_proven += cluster.proven_edges;
        report.unproven.extend(cluster.spans);
    }
    report.unproven.sort_by(|a, b| {
        let key =
            |s: &UnprovenSpan| [s.midpoint, s.endpoints[0], s.endpoints[1]].map(|p| p.to_array());
        key(a)
            .iter()
            .flatten()
            .zip(key(b).iter().flatten())
            .map(|(x, y)| x.total_cmp(y))
            .find(|o| o.is_ne())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.brush.cmp(&b.brush))
            .then(a.face.cmp(&b.face))
    });
    report.elapsed = started.elapsed();
    report
}

#[cfg(test)]
mod gate_map_tests;
#[cfg(test)]
mod stub_tests;
#[cfg(test)]
mod test_fixtures;
#[cfg(test)]
mod tests;
