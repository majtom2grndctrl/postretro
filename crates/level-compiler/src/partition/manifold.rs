// Watertightness diagnostic: flags open edges in the emitted world-face set so
// dropped-face holes surface at compile time instead of in-game.
// See: context/lib/build_pipeline.md §PRL Compilation
//
// The faces emitted by `extract_faces` are the boundary of the solid point-set:
// every brush side that faces empty space, before exterior culling zeroes the
// outward-facing shell. The topological boundary of a solid is a closed
// 2-manifold, so along every line in space the surface covers each point an even
// number of times (0, or 2 where two faces meet, 4 at a brush hinge). A point
// covered an ODD number of times is an open boundary — the signature of a face
// the clipper dropped. This runs on the pre-exterior-cull face set on purpose:
// after culling, the legitimately-open outer shell would swamp the real holes.
//
// Why per-line coverage and not a plain shared-edge count: BSP face extraction is
// inherently T-junction dense — a long floor edge is routinely opposed by two
// shorter wall-fragment edges that meet at a vertex on its interior. A naive
// "every edge shared by exactly two faces" test flags every one of those, firing
// hundreds of times on clean maps. Grouping collinear edges by supporting line
// and testing coverage parity per sub-interval makes a T-junction cancel (the
// long edge's span is covered by the two short edges → even) while a truly
// missing face leaves an odd-covered span. Coverage count is unsigned, so the
// check needs no assumption about consistent face winding.
//
// Diagnostic only — never fails the build. A compiler bug must not halt a level
// designer; it must be visible so it can be fixed.

use std::collections::HashMap;

use glam::DVec3;

use crate::map_data::Face;

/// Vertex weld quantum for reporting, in meters. Matches the hull-robustness
/// quantum in `face_extract`.
const WELD_QUANTUM: f64 = 1e-7;
/// Quantum for the supporting-line direction components (unit vector). Absorbs
/// normalization noise while keeping non-parallel edges on distinct lines.
const DIR_QUANTUM: f64 = 1e-6;
/// Quantum for the line anchor (closest point to origin), in meters. Coarser
/// than the weld quantum so two collinear edges with sub-mm perpendicular noise
/// group onto the same line; still far finer than the spacing of real parallel
/// surfaces.
const ANCHOR_QUANTUM: f64 = 1e-4;
/// Breakpoints closer than this along a line are merged, in meters.
const PARAM_MERGE: f64 = 1e-6;
/// Edges shorter than this are skipped as degenerate, in meters.
const MIN_EDGE_LEN: f64 = 1e-6;
/// Open spans shorter than this are ignored as clip/quantization noise rather
/// than reported as holes, in meters. A hole you can see through is far larger.
const MIN_OPEN_SPAN: f64 = 1e-3;
/// Max open-edge locations to surface. A holed map can produce many open spans;
/// the count conveys severity, the samples convey where to look.
const MAX_SAMPLES: usize = 12;

/// Quantized supporting line: `(direction, anchor)`, both component-quantized.
type LineKey = ([i64; 3], [i64; 3]);

fn qv(p: DVec3, quantum: f64) -> [i64; 3] {
    [
        (p.x / quantum).round() as i64,
        (p.y / quantum).round() as i64,
        (p.z / quantum).round() as i64,
    ]
}

/// Pick a deterministic hemisphere for a line direction, so the two orientations
/// of a shared edge canonicalize to the same key.
fn point_positive(d: DVec3) -> bool {
    if d.x.abs() > 1e-9 {
        d.x > 0.0
    } else if d.y.abs() > 1e-9 {
        d.y > 0.0
    } else {
        d.z > 0.0
    }
}

/// One covered interval along a supporting line: `[t0, t1]` in the line's
/// canonical direction, with the source brush for reporting.
struct Interval {
    t0: f64,
    t1: f64,
    brush_index: usize,
}

/// One open-edge location, for surfacing to the designer.
#[derive(Debug, Clone)]
pub struct OpenEdge {
    /// World-space midpoint of the open span (engine space, meters).
    pub midpoint: DVec3,
    /// Source brush of a face touching this span — a starting point for the fix.
    pub brush_index: usize,
}

/// Result of the watertightness check.
#[derive(Debug, Default)]
pub struct WatertightReport {
    /// Total number of open (odd-coverage) spans found.
    pub open_edge_count: usize,
    /// A deterministic, bounded sample of open-span locations for logging.
    pub samples: Vec<OpenEdge>,
}

impl WatertightReport {
    pub fn is_watertight(&self) -> bool {
        self.open_edge_count == 0
    }
}

/// Check whether the emitted world faces form a closed surface.
///
/// Pass the `faces` from `PartitionResult` *before* exterior culling. Groups
/// every face edge by its supporting line, then walks each line looking for
/// sub-intervals covered an odd number of times — those are open boundaries
/// (dropped faces). T-junctions cancel because the interior surface is still
/// fully covered there. Deterministic: coverage is order-independent and sampled
/// locations are sorted before truncation.
pub fn check_watertight(faces: &[Face]) -> WatertightReport {
    let mut lines: HashMap<LineKey, Vec<Interval>> = HashMap::new();

    for face in faces {
        let n = face.vertices.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let a = face.vertices[i];
            let b = face.vertices[(i + 1) % n];
            let delta = b - a;
            let len = delta.length();
            if len < MIN_EDGE_LEN {
                continue;
            }
            let dir = delta / len;
            let cdir = if point_positive(dir) { dir } else { -dir };
            // Closest point on the line to the origin — identifies the line
            // independent of where along it this edge sits.
            let anchor = a - a.dot(cdir) * cdir;
            let key = (qv(cdir, DIR_QUANTUM), qv(anchor, ANCHOR_QUANTUM));
            let ta = a.dot(cdir);
            let tb = b.dot(cdir);
            let (t0, t1) = if ta <= tb { (ta, tb) } else { (tb, ta) };
            lines.entry(key).or_default().push(Interval {
                t0,
                t1,
                brush_index: face.brush_index,
            });
        }
    }

    let mut open: Vec<OpenEdge> = Vec::new();
    for (key, intervals) in &lines {
        let (dir_q, anchor_q) = key;
        let cdir = DVec3::new(
            dir_q[0] as f64 * DIR_QUANTUM,
            dir_q[1] as f64 * DIR_QUANTUM,
            dir_q[2] as f64 * DIR_QUANTUM,
        );
        let anchor = DVec3::new(
            anchor_q[0] as f64 * ANCHOR_QUANTUM,
            anchor_q[1] as f64 * ANCHOR_QUANTUM,
            anchor_q[2] as f64 * ANCHOR_QUANTUM,
        );

        // Sweep the coverage count between consecutive breakpoints.
        let mut breaks: Vec<f64> = Vec::with_capacity(intervals.len() * 2);
        for iv in intervals {
            breaks.push(iv.t0);
            breaks.push(iv.t1);
        }
        breaks.sort_by(|a, b| a.partial_cmp(b).unwrap());
        breaks.dedup_by(|a, b| (*a - *b).abs() < PARAM_MERGE);

        for w in breaks.windows(2) {
            if w[1] - w[0] < MIN_OPEN_SPAN {
                continue;
            }
            let mid = 0.5 * (w[0] + w[1]);
            let mut coverage = 0u32;
            let mut brush_index = 0usize;
            for iv in intervals {
                if iv.t0 <= mid && mid <= iv.t1 {
                    coverage += 1;
                    brush_index = iv.brush_index;
                }
            }
            if coverage % 2 == 1 {
                open.push(OpenEdge {
                    midpoint: anchor + cdir * mid,
                    brush_index,
                });
            }
        }
    }

    // Stable ordering so the sampled locations don't depend on HashMap order.
    open.sort_by_key(|edge| qv(edge.midpoint, WELD_QUANTUM));
    let open_edge_count = open.len();
    open.truncate(MAX_SAMPLES);

    WatertightReport {
        open_edge_count,
        samples: open,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_data::TextureProjection;

    /// A quad face on a given brush, wound in order.
    fn quad(v: [DVec3; 4], brush_index: usize) -> Face {
        Face {
            vertices: v.to_vec(),
            normal: DVec3::Z,
            distance: 0.0,
            texture: "test".to_string(),
            tex_projection: TextureProjection::default(),
            brush_index,
        }
    }

    /// The six faces of an axis-aligned box, each wound consistently. This is a
    /// closed surface: every edge is shared by exactly two faces.
    fn closed_box() -> Vec<Face> {
        let p = |x: f64, y: f64, z: f64| DVec3::new(x, y, z);
        vec![
            quad(
                [
                    p(0.0, 0.0, 0.0),
                    p(1.0, 0.0, 0.0),
                    p(1.0, 1.0, 0.0),
                    p(0.0, 1.0, 0.0),
                ],
                0,
            ), // -Z
            quad(
                [
                    p(0.0, 0.0, 1.0),
                    p(0.0, 1.0, 1.0),
                    p(1.0, 1.0, 1.0),
                    p(1.0, 0.0, 1.0),
                ],
                0,
            ), // +Z
            quad(
                [
                    p(0.0, 0.0, 0.0),
                    p(0.0, 1.0, 0.0),
                    p(0.0, 1.0, 1.0),
                    p(0.0, 0.0, 1.0),
                ],
                0,
            ), // -X
            quad(
                [
                    p(1.0, 0.0, 0.0),
                    p(1.0, 0.0, 1.0),
                    p(1.0, 1.0, 1.0),
                    p(1.0, 1.0, 0.0),
                ],
                0,
            ), // +X
            quad(
                [
                    p(0.0, 0.0, 0.0),
                    p(0.0, 0.0, 1.0),
                    p(1.0, 0.0, 1.0),
                    p(1.0, 0.0, 0.0),
                ],
                0,
            ), // -Y
            quad(
                [
                    p(0.0, 1.0, 0.0),
                    p(1.0, 1.0, 0.0),
                    p(1.0, 1.0, 1.0),
                    p(0.0, 1.0, 1.0),
                ],
                0,
            ), // +Y
        ]
    }

    #[test]
    fn closed_box_is_watertight() {
        let report = check_watertight(&closed_box());
        assert!(
            report.is_watertight(),
            "closed box reported {} open edges",
            report.open_edge_count
        );
    }

    #[test]
    fn box_missing_a_face_reports_open_edges() {
        let mut faces = closed_box();
        faces.pop(); // Remove the +Y face, opening a square hole.
        let report = check_watertight(&faces);
        // The 4 edges of the removed face are each covered once (odd) now.
        assert_eq!(report.open_edge_count, 4);
        assert!(!report.samples.is_empty());
    }

    /// The critical property: a T-junction must NOT report. Splitting one face
    /// of a closed box into two coplanar halves leaves the surface fully
    /// covered, but its outer edges are now opposed by full-length edges on the
    /// neighbouring faces — the exact configuration a naive shared-edge count
    /// would flag. Per-line coverage parity must see it as still closed.
    #[test]
    fn t_junction_does_not_report() {
        let p = |x: f64, y: f64, z: f64| DVec3::new(x, y, z);
        let mut faces = closed_box();
        faces.pop(); // drop the single +Y quad...
        // ...and re-add it as two coplanar halves split at x = 0.5. Every edge
        // the halves share with the ±X/±Z side faces is now a T-junction.
        faces.push(quad(
            [
                p(0.0, 1.0, 0.0),
                p(0.5, 1.0, 0.0),
                p(0.5, 1.0, 1.0),
                p(0.0, 1.0, 1.0),
            ],
            0,
        ));
        faces.push(quad(
            [
                p(0.5, 1.0, 0.0),
                p(1.0, 1.0, 0.0),
                p(1.0, 1.0, 1.0),
                p(0.5, 1.0, 1.0),
            ],
            0,
        ));
        let report = check_watertight(&faces);
        assert!(
            report.is_watertight(),
            "T-junction split reported {} open edges (should be 0)",
            report.open_edge_count
        );
    }

    #[test]
    fn welds_across_float_noise() {
        // A shared edge whose endpoints differ by sub-quantum noise still groups
        // onto one supporting line: only the two floating strips' outer edges
        // are open, not the welded seam.
        let p = |x: f64, y: f64, z: f64| DVec3::new(x, y, z);
        let eps = WELD_QUANTUM * 0.4;
        let faces = vec![
            quad(
                [
                    p(0.0, 0.0, 0.0),
                    p(1.0, 0.0, 0.0),
                    p(1.0, 1.0, 0.0),
                    p(0.0, 1.0, 0.0),
                ],
                0,
            ),
            // Shares the (0,0,0)-(1,0,0) edge, nudged below the quantum.
            quad(
                [
                    p(0.0, 0.0 + eps, 0.0),
                    p(0.0, -1.0, 0.0),
                    p(1.0, -1.0, 0.0),
                    p(1.0, 0.0 - eps, 0.0),
                ],
                1,
            ),
        ];
        let report = check_watertight(&faces);
        // The seam welds (coverage 2); the 6 other outer edges are open.
        assert_eq!(report.open_edge_count, 6);
    }

    #[test]
    fn empty_input_is_watertight() {
        assert!(check_watertight(&[]).is_watertight());
    }
}
