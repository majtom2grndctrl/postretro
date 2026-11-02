// Coverage proof: every traced face edge matched one-for-one, within the tracer's τ, by an edge running the other way.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)
//
// The rule, and why a proven span is sound:
// - Edges come from the stored `f32` triangles the bake BVH holds, widened
//   to `f64`.
// - Weld (`weld`): stored corners within δ link, single linkage over all
//   pairs. A cluster wider than δ, or holding two corners of one triangle,
//   stays apart; an accepted one takes its smallest-bit member's identity.
//   The map moves no corner more than δ and keeps every triangle's corners
//   distinct, so it maps a closed oriented surface to a closed one, and
//   copies of one corner computed from different brushes stop leaving stubs.
// - Welded identity only cancels a face's fan diagonals and places edge
//   ends on a cluster axis. Every distance is measured on stored geometry:
//   a piece end is the edge's stored point at that fraction of its welded
//   span.
// - Each cluster of near-collinear opposed edges is cut into pieces at
//   every welded end's projection onto one axis (`settle`). Every edge
//   running over a piece must be matched one-for-one by the other
//   direction's edges; nothing is excused. Where copies farther apart than
//   δ leave a gap, an edge ending within τ may stand in with its stored
//   end, capped so a direction never counts more edges than it has present
//   within τ (`settle`). A pair needs each one's stretch within τ of one
//   triangle of the other's face, and distance to a triangle is convex
//   along a segment, so the ends settle the piece. An edge left out is an
//   unproven span.
// - The limit is τ less a margin for `f64` rounding scaled by the
//   geometry's extent, with no δ in it: rounding counts toward a hole, and
//   no quantum merges edges, so the proof holds at any coordinate.
// - Beyond strict balance, the weld hides only a stretch no longer than δ
//   between two copies of one corner that no edge joins, and a stand-in
//   only a dip in one direction's count within τ of where it is present.

use std::time::{Duration, Instant};

use glam::DVec3;
use rayon::prelude::*;

use crate::geometry::GeometryResult;
use crate::partition::{OPEN_EDGE_SAMPLE_CAP, OpenEdge};
use crate::ray_traversal::TRACER_TOLERANCE_METERS;

mod clusters;
mod edges;
mod face_boxes;
mod matching;
mod settle;
mod weld;

use edges::DirectedEdge;
use settle::{Measure, Settled};
use weld::{Weld, WeldRule};

/// Edges link into one cluster when each overlaps the other within this
/// many τ of its line, room for every crease and T-junction partner, which
/// lie within τ of each other. A partner outside it costs a pair, never
/// proves one.
const LINK_RADIUS_TAUS: f64 = 2.0;

/// The rounding margin, in `f64` epsilons of the geometry's extent: well
/// above the few roundings an evaluated point and its distance to a
/// triangle take.
const ROUNDING_EPSILONS: f64 = 256.0;

/// δ: stored corners this close, from different triangles, weld into one
/// vertex. A named literal, not derived from τ: the copies come from face
/// extraction's `f64` arithmetic, not from the tracer. Twice the measured
/// δ_min, 50 µm: the smallest ladder δ at which the gate maps (hallway,
/// hallway-mini, movement-feel, post-cut) read no unproven span, by the
/// weld-consensus δ procedure with capped stand-ins
/// (`context/plans/in-progress/lightmap-reach-cull/weld-sweep.md`;
/// `gate_map_tests` re-checks the maps). At most τ/4.
const WELD_RADIUS_METERS: f64 = 100e-6;

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
    prove_welded(
        geometry,
        face_brushes,
        WeldRule {
            radius: WELD_RADIUS_METERS,
            keep_triangles_whole: true,
        },
    )
}

fn prove_welded(
    geometry: &GeometryResult,
    face_brushes: &[Option<u32>],
    rule: WeldRule,
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
    let weld = Weld::new(geometry, rule);
    let edges = edges::boundary_edges(geometry, face_brushes, &weld);
    // Grown past τ by a rounding slack, so the box test prunes no pair the
    // limit would prove.
    let boxes = face_boxes::face_boxes(
        geometry,
        tau + ROUNDING_EPSILONS * f64::EPSILON * (1.0 + extent),
    );
    let measure = Measure {
        geometry,
        boxes: &boxes,
        limit_squared: limit * limit,
        stand_in_reach: tau,
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
mod hinge_tests;
#[cfg(test)]
mod test_fixtures;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod weld_tests;
