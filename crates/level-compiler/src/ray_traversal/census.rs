// Shared-edge census: how a geometry's triangle edges meet, and the widest gap along each one no other triangle shares.
// See: context/lib/build_pipeline.md §Ray traversal

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use bvh::aabb::{Aabb, Bounded};
use bvh::bounding_hierarchy::BHShape;
use bvh::bvh::Bvh;
use glam::DVec3;
use nalgebra::Point3;
use rayon::prelude::*;

use super::hit_test::distance_squared_to_triangle;
use crate::geometry::GeometryResult;

/// Absolute accuracy of each edge's widest gap: the reported value and its
/// upper bound differ by no more than this.
pub(super) const GAP_ACCURACY_METERS: f64 = 1e-7;

/// Nearest-triangle search radii, tried in order. A gap is exact once it is
/// no wider than the radius it was found at; a wider one retries at the next.
pub(super) const SEARCH_RADII_METERS: [f64; 5] = [5e-3, 2e-2, 8e-2, 0.32, 1.0];

/// Probes one edge may take before its gap is reported unconverged, with
/// its bound.
const MAX_PROBES_PER_EDGE: usize = 200_000;

/// T-junction threshold, in `f32` ulps of the largest endpoint coordinate
/// magnitude (at least 1 m): a vertex the compiler placed on a line in `f64`
/// lies within about √3 / 2 ulp of it once each coordinate rounds to `f32`.
pub(super) const T_JUNCTION_ULPS: f64 = 2.0;

/// How a geometry's triangle edges meet, counted per triangle edge (three per
/// triangle).
pub(super) struct Census {
    pub(super) triangles: usize,
    pub(super) faces: usize,
    pub(super) edges: usize,
    /// Edges whose two endpoints are bit-identical.
    pub(super) degenerate: usize,
    /// Edges another triangle shares with bit-identical endpoints, either
    /// direction, all of whose sharers lie in the same face.
    pub(super) shared_within_face: usize,
    /// Edges shared with bit-identical endpoints by a triangle of another face.
    pub(super) shared_across_faces: usize,
    /// Of `shared_across_faces`, those between sub-faces of one cut face.
    pub(super) shared_across_cut_siblings: usize,
    /// Shared edges with more than one sharer.
    pub(super) shared_non_manifold: usize,
    /// Every edge no other triangle shares, in triangle order.
    pub(super) mismatched: Vec<MismatchedEdge>,
    /// The largest T-junction threshold any collinearity test used.
    pub(super) widest_t_junction_threshold: f64,
}

/// An edge no other triangle shares with bit-identical endpoints.
pub(super) struct MismatchedEdge {
    pub(super) triangle: usize,
    pub(super) face: u32,
    pub(super) endpoints: [DVec3; 2],
    /// The face of a triangle edge this one lies along, when it is a
    /// T-junction: collinear and overlapping within the T-junction threshold.
    pub(super) t_junction_with: Option<u32>,
    /// A T-junction partner's endpoints both lie within the threshold of
    /// this edge's: one edge rounded apart, not a vertex on a longer edge.
    pub(super) rounded_apart: bool,
    pub(super) gap: Gap,
}

/// The widest gap along an edge: the largest, over its points, of the
/// distance to the nearest triangle of any other face, leaving out triangles
/// the tracer can never hit (two bit-identical corners).
#[derive(Debug, Clone, Copy)]
pub(super) struct Gap {
    /// The largest distance reached at a probed point: a lower bound.
    pub(super) widest: f64,
    /// An upper bound; within [`GAP_ACCURACY_METERS`] of `widest` once
    /// converged.
    pub(super) upper: f64,
    pub(super) at: DVec3,
    /// The nearest other face at `at`, if one lies within the search radius.
    pub(super) nearest_face: Option<u32>,
    /// `Some(r)` when the gap is wider than the largest search radius `r`:
    /// `widest` is then `r`, a lower bound, and `upper` bounds the gap from
    /// the triangles within `r`.
    pub(super) beyond: Option<f64>,
    pub(super) converged: bool,
}

impl Gap {
    /// The value bands and orderings read: the lower bound.
    pub(super) fn width(&self) -> f64 {
        self.widest
    }
}

struct Triangle {
    corners: [DVec3; 3],
    bits: [[u32; 3]; 3],
    face: u32,
}

/// A triangle's box for the nearest-triangle index.
struct TriangleBox {
    aabb: Aabb<f32, 3>,
    triangle: usize,
    node_index: usize,
}

impl Bounded<f32, 3> for TriangleBox {
    fn aabb(&self) -> Aabb<f32, 3> {
        self.aabb
    }
}

impl BHShape<f32, 3> for TriangleBox {
    fn set_bh_node_index(&mut self, index: usize) {
        self.node_index = index;
    }

    fn bh_node_index(&self) -> usize {
        self.node_index
    }
}

type EdgeKey = ([u32; 3], [u32; 3]);

fn edge_key(a: [u32; 3], b: [u32; 3]) -> EdgeKey {
    if a <= b { (a, b) } else { (b, a) }
}

/// Count `geometry`'s triangle edges by how they meet, and measure the widest
/// gap along every edge no other triangle shares. `face_parent`, after an
/// oversize-face cut, maps each face to the pre-cut face it came from.
pub(super) fn census(geometry: &GeometryResult, face_parent: Option<&[usize]>) -> Census {
    let triangles = triangles_of(geometry);
    let mut sharers: HashMap<EdgeKey, Vec<(usize, u32)>> = HashMap::new();
    for (index, triangle) in triangles.iter().enumerate() {
        for k in 0..3 {
            let key = edge_key(triangle.bits[k], triangle.bits[(k + 1) % 3]);
            sharers.entry(key).or_default().push((index, triangle.face));
        }
    }
    let siblings = |a: u32, b: u32| face_parent.is_some_and(|p| p[a as usize] == p[b as usize]);

    let mut census = Census {
        triangles: triangles.len(),
        faces: geometry.geometry.faces.len(),
        edges: triangles.len() * 3,
        degenerate: 0,
        shared_within_face: 0,
        shared_across_faces: 0,
        shared_across_cut_siblings: 0,
        shared_non_manifold: 0,
        mismatched: Vec::new(),
        widest_t_junction_threshold: 0.0,
    };
    let mut unshared = Vec::new();
    for (index, triangle) in triangles.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (triangle.bits[k], triangle.bits[(k + 1) % 3]);
            if a == b {
                census.degenerate += 1;
                continue;
            }
            let others: Vec<u32> = sharers[&edge_key(a, b)]
                .iter()
                .filter(|&&(other, _)| other != index)
                .map(|&(_, face)| face)
                .collect();
            match others.iter().find(|&&face| face != triangle.face) {
                None if others.is_empty() => unshared.push((index, k)),
                None => census.shared_within_face += 1,
                Some(&face) => {
                    census.shared_across_faces += 1;
                    if siblings(triangle.face, face) {
                        census.shared_across_cut_siblings += 1;
                    }
                }
            }
            if others.len() > 1 {
                census.shared_non_manifold += 1;
            }
        }
    }

    // A triangle with two bit-identical corners has a determinant of exactly
    // zero for every ray, so the tracer never hits it: it covers no gap.
    let mut boxes: Vec<TriangleBox> = triangles
        .iter()
        .enumerate()
        .filter(|(_, triangle)| {
            let [a, b, c] = triangle.bits;
            a != b && b != c && c != a
        })
        .map(|(index, triangle)| {
            let p = triangle.bits.map(|c| c.map(f32::from_bits));
            let lo = |axis: usize| p[0][axis].min(p[1][axis]).min(p[2][axis]);
            let hi = |axis: usize| p[0][axis].max(p[1][axis]).max(p[2][axis]);
            TriangleBox {
                aabb: Aabb::with_bounds(
                    Point3::new(lo(0), lo(1), lo(2)),
                    Point3::new(hi(0), hi(1), hi(2)),
                ),
                triangle: index,
                node_index: 0,
            }
        })
        .collect();
    let index = Bvh::build(&mut boxes);
    let near = Nearby {
        triangles: &triangles,
        index: &index,
        boxes: &boxes,
    };

    let measured: Vec<(MismatchedEdge, f64)> = unshared
        .par_iter()
        .map(|&(tri, k)| {
            let triangle = &triangles[tri];
            let endpoints = [triangle.corners[k], triangle.corners[(k + 1) % 3]];
            let (t_junction_with, rounded_apart, threshold) =
                near.t_junction_partner(triangle.face, endpoints);
            let gap = near.widest_gap(triangle.face, endpoints);
            let edge = MismatchedEdge {
                triangle: tri,
                face: triangle.face,
                endpoints,
                t_junction_with,
                rounded_apart,
                gap,
            };
            (edge, threshold)
        })
        .collect();
    for (edge, threshold) in measured {
        census.widest_t_junction_threshold = census.widest_t_junction_threshold.max(threshold);
        census.mismatched.push(edge);
    }
    census
}

fn triangles_of(geometry: &GeometryResult) -> Vec<Triangle> {
    let geom = &geometry.geometry;
    let mut triangles = Vec::with_capacity(geom.indices.len() / 3);
    for (face, range) in geometry.face_index_ranges.iter().enumerate() {
        let start = range.index_offset as usize;
        let end = start + range.index_count as usize;
        for tri in (start..end).step_by(3).take_while(|&tri| tri + 3 <= end) {
            let corner = |k: usize| geom.vertices[geom.indices[tri + k] as usize].position;
            let positions = [corner(0), corner(1), corner(2)];
            triangles.push(Triangle {
                corners: positions.map(|p| DVec3::new(p[0].into(), p[1].into(), p[2].into())),
                bits: positions.map(|p| p.map(f32::to_bits)),
                face: face as u32,
            });
        }
    }
    triangles
}

/// The triangles near an edge, through a box index over every triangle.
struct Nearby<'a> {
    triangles: &'a [Triangle],
    index: &'a Bvh<f32, 3>,
    boxes: &'a [TriangleBox],
}

impl Nearby<'_> {
    /// Triangles of faces other than `face` whose boxes come within `radius`
    /// of the edge's box: every triangle the tracer can hit within `radius`
    /// of a point of the edge is among them.
    fn candidates(&self, face: u32, [p, q]: [DVec3; 2], radius: f64) -> Vec<usize> {
        let lo = p.min(q) - DVec3::splat(radius);
        let hi = p.max(q) + DVec3::splat(radius);
        // Rounded outward to `f32`, so the query box holds the exact one.
        let down = |v: f64| (v as f32).next_down();
        let up = |v: f64| (v as f32).next_up();
        let query = Aabb::with_bounds(
            Point3::new(down(lo.x), down(lo.y), down(lo.z)),
            Point3::new(up(hi.x), up(hi.y), up(hi.z)),
        );
        let mut found: Vec<usize> = self
            .index
            .traverse(&query, self.boxes)
            .into_iter()
            .map(|shape| shape.triangle)
            .filter(|&tri| self.triangles[tri].face != face)
            .collect();
        found.sort_unstable();
        found
    }

    /// The face of the first triangle edge this edge lies along, whether any
    /// such partner matches it end for end within the threshold, and the
    /// largest threshold the test used. Collinear: both endpoints of the
    /// shorter edge lie within the threshold of the longer edge's line.
    /// Overlapping: the shorter edge's projection overlaps the longer edge by
    /// more than the threshold, so edges meeting only at a vertex do not count.
    fn t_junction_partner(&self, face: u32, [p, q]: [DVec3; 2]) -> (Option<u32>, bool, f64) {
        let mut partner = None;
        let mut rounded_apart = false;
        let mut widest_threshold: f64 = 0.0;
        for tri in self.candidates(face, [p, q], SEARCH_RADII_METERS[0]) {
            let triangle = &self.triangles[tri];
            for k in 0..3 {
                let [a, b] = [triangle.corners[k], triangle.corners[(k + 1) % 3]];
                let threshold = t_junction_threshold([p, q, a, b]);
                widest_threshold = widest_threshold.max(threshold);
                if a == b || !collinear_overlapping([p, q], [a, b], threshold) {
                    continue;
                }
                partner.get_or_insert(triangle.face);
                let near = |x: DVec3, y: DVec3| (x - y).length() <= threshold;
                rounded_apart |= (near(p, a) && near(q, b)) || (near(p, b) && near(q, a));
            }
        }
        (partner, rounded_apart, widest_threshold)
    }

    /// Branch and bound over the edge `p → q` for the largest distance to
    /// the triangles of other faces, at each search radius until the gap fits
    /// one. See [`widest_gap_within`].
    fn widest_gap(&self, face: u32, endpoints: [DVec3; 2]) -> Gap {
        let mut gap = None;
        for radius in SEARCH_RADII_METERS {
            let candidates = self.candidates(face, endpoints, radius);
            let found = widest_gap_within(self.triangles, &candidates, endpoints, radius);
            let fits = found.widest <= radius;
            gap = Some(found);
            if fits {
                break;
            }
        }
        let mut gap = gap.expect("at least one search radius");
        let largest = SEARCH_RADII_METERS[SEARCH_RADII_METERS.len() - 1];
        if gap.widest > largest {
            gap.beyond = Some(largest);
            gap.widest = largest;
        }
        gap
    }
}

fn ulp(value: f64) -> f64 {
    let v = value.abs() as f32;
    f64::from(v.next_up() - v)
}

fn t_junction_threshold(points: [DVec3; 4]) -> f64 {
    let largest = points
        .iter()
        .map(|p| p.abs().max_element())
        .fold(1.0, f64::max);
    T_JUNCTION_ULPS * ulp(largest)
}

fn collinear_overlapping(e: [DVec3; 2], f: [DVec3; 2], threshold: f64) -> bool {
    let length = |[a, b]: [DVec3; 2]| (b - a).length();
    let (long, short) = if length(e) >= length(f) {
        (e, f)
    } else {
        (f, e)
    };
    let along = long[1] - long[0];
    let long_length = along.length();
    if long_length == 0.0 {
        return false;
    }
    let unit = along / long_length;
    let project = |x: DVec3| (x - long[0]).dot(unit);
    let off_line = |x: DVec3| {
        let offset = x - long[0];
        (offset - unit * offset.dot(unit)).length()
    };
    if off_line(short[0]) > threshold || off_line(short[1]) > threshold {
        return false;
    }
    let (t0, t1) = (project(short[0]), project(short[1]));
    let overlap = t0.max(t1).min(long_length) - t0.min(t1).max(0.0);
    overlap > threshold
}

/// One probed point of an edge: its parameter, the distance to each
/// candidate triangle, and the nearest of them.
struct Probe {
    s: f64,
    distances: Vec<f64>,
    nearest: Option<usize>,
    f: f64,
}

/// A parameter interval between two probes, ordered by its upper bound.
struct Interval {
    upper: f64,
    from: usize,
    to: usize,
}

impl PartialEq for Interval {
    fn eq(&self, other: &Self) -> bool {
        self.upper.total_cmp(&other.upper) == Ordering::Equal
    }
}
impl Eq for Interval {}
impl PartialOrd for Interval {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Interval {
    fn cmp(&self, other: &Self) -> Ordering {
        self.upper.total_cmp(&other.upper)
    }
}

/// The widest gap along `p → q` to the `candidates`, by branch and bound on
/// the edge's parameter. The distance to the candidates, f, is the minimum of
/// each candidate's distance g, and two bounds hold on an interval [a, b]:
/// - f is 1-Lipschitz in arc length, so its maximum is at most
///   (f(a) + f(b) + length) / 2;
/// - each g is convex along a line (distance to a convex set), so its
///   maximum is at an end, and f's is at most the least max(g(a), g(b)).
///
/// The second closes an interval one triangle covers in one step; the first
/// bounds the rest. The interval with the largest bound splits until that
/// bound is within [`GAP_ACCURACY_METERS`] of the widest probed value. It
/// stops early once a probe passes `radius`, past which the candidates no
/// longer hold every nearest triangle.
fn widest_gap_within(
    triangles: &[Triangle],
    candidates: &[usize],
    [p, q]: [DVec3; 2],
    radius: f64,
) -> Gap {
    let edge_length = (q - p).length();
    let probe = |s: f64| {
        let point = p + (q - p) * s;
        let distances: Vec<f64> = candidates
            .iter()
            .map(|&tri| distance_squared_to_triangle(point, triangles[tri].corners).sqrt())
            .collect();
        let (nearest, f) =
            distances
                .iter()
                .enumerate()
                .fold((None, f64::INFINITY), |(best, f), (i, &d)| {
                    if d < f { (Some(i), d) } else { (best, f) }
                });
        Probe {
            s,
            distances,
            nearest: nearest.map(|i| candidates[i]),
            f,
        }
    };
    let bound = |probes: &[Probe], from: usize, to: usize| {
        let (a, b) = (&probes[from], &probes[to]);
        let lipschitz = 0.5 * (a.f + b.f + (b.s - a.s) * edge_length);
        let convex = a
            .distances
            .iter()
            .zip(&b.distances)
            .map(|(&ga, &gb)| ga.max(gb))
            .fold(f64::INFINITY, f64::min);
        lipschitz.min(convex)
    };

    let mut probes = vec![probe(0.0), probe(1.0)];
    let mut widest = if probes[1].f > probes[0].f { 1 } else { 0 };
    let mut heap = BinaryHeap::from([Interval {
        upper: bound(&probes, 0, 1),
        from: 0,
        to: 1,
    }]);
    let mut upper = probes[widest].f;
    let mut converged = true;
    while let Some(top) = heap.pop() {
        let best = probes[widest].f;
        if top.upper <= best + GAP_ACCURACY_METERS || best > radius {
            upper = top.upper.max(best);
            break;
        }
        if probes.len() >= MAX_PROBES_PER_EDGE {
            upper = top.upper;
            converged = false;
            break;
        }
        let mid = probes.len();
        probes.push(probe(0.5 * (probes[top.from].s + probes[top.to].s)));
        if probes[mid].f > best {
            widest = mid;
        }
        heap.push(Interval {
            upper: bound(&probes, top.from, mid),
            from: top.from,
            to: mid,
        });
        heap.push(Interval {
            upper: bound(&probes, mid, top.to),
            from: mid,
            to: top.to,
        });
    }
    let at = &probes[widest];
    Gap {
        widest: at.f,
        upper,
        at: p + (q - p) * at.s,
        nearest_face: at.nearest.map(|tri| triangles[tri].face),
        beyond: None,
        converged,
    }
}
