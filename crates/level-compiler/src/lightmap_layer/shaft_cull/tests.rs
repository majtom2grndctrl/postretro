// Shaft-cull margins against the hit test: a randomized f32 Möller–Trumbore search.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use glam::{DVec3, Vec3};

use super::margin::*;
use super::*;
use crate::lightmap_bake::ray_triangle_hit;

/// SplitMix64: a fixed, dependency-free stream, so the search is the same on
/// every run.
struct Stream(u64);

impl Stream {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    /// Log-uniform in `[lo, hi]`.
    fn log_range(&mut self, lo: f64, hi: f64) -> f64 {
        (lo.ln() + (hi.ln() - lo.ln()) * self.unit()).exp()
    }

    fn direction(&mut self) -> DVec3 {
        loop {
            let v = DVec3::new(
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
            );
            let length = v.length();
            if length > 0.1 && length <= 1.0 {
                return v / length;
            }
        }
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[(self.next() % items.len() as u64) as usize]
    }
}

/// A random triangle near `centre`: edges up to `size`, angle at its first
/// corner between 0.6° and 120°, its corners rounded to `f32` as stored.
fn triangle(stream: &mut Stream, centre: DVec3, size: f64) -> [Vec3; 3] {
    let e1 = stream.direction();
    let side = (stream.direction() - e1 * e1.dot(stream.direction())).normalize_or_zero();
    let side = if side == DVec3::ZERO {
        e1.any_orthonormal_vector()
    } else {
        side
    };
    let angle = stream.log_range(0.6, 120.0).to_radians();
    let a = centre + stream.direction() * size * stream.unit();
    let b = a + e1 * size * stream.range(0.3, 1.0);
    let c = a + (e1 * angle.cos() + side * angle.sin()) * size * stream.range(0.3, 1.0);
    [a, b, c].map(|p| p.as_vec3())
}

/// A segment as its own hull: the two ends, no emitter.
fn segment_bounds(from: Vec3, to: Vec3) -> HullBounds {
    let (a, b) = (from.as_dvec3(), to.as_dvec3());
    HullBounds::new(Hull {
        corners: [a, a, a, a, b, b, b, b],
        centre: b,
        radius: 0.0,
        terms: DVec3::ZERO,
    })
}

/// Whether the layer walk's shadow or clip test of this one triangle on
/// `from → to` reports a hit: `segment_clear_remembering`'s ray and
/// predicate.
fn traced_hit(from: Vec3, to: Vec3, [a, b, c]: [Vec3; 3]) -> bool {
    let delta = to - from;
    let length = delta.length();
    if length < RAY_EPSILON {
        return false;
    }
    let dir = delta / length;
    let origin = from + dir * RAY_EPSILON;
    let max_distance = length - RAY_EPSILON;
    ray_triangle_hit(origin, dir, a, b, c).is_some_and(|dist| dist > 0.0 && dist < max_distance)
}

/// Whether a lead-in test of this triangle on `from → to` reports a hit:
/// `front_faces_clear`'s ray and predicate, either face.
fn lead_in_hit(from: Vec3, to: Vec3, [a, b, c]: [Vec3; 3]) -> bool {
    let delta = to - from;
    let length = delta.length();
    if length <= 0.0 {
        return false;
    }
    let dir = delta / length;
    ray_triangle_hit(from, dir, a, b, c).is_some_and(|dist| dist < length)
}

/// Sine of the angle between the `f32` ray `from → to` and the plane of the
/// triangle's `f32` edges.
fn sin_grazing(from: Vec3, to: Vec3, candidate: &Candidate) -> f64 {
    let dir = (to - from).as_dvec3().normalize();
    dir.dot(candidate.normal.normalize()).abs()
}

/// Coordinate offsets the search translates scenes to: the origin, Quake's
/// grid edge, a kilometre and 4 km.
const OFFSETS: [f64; 4] = [0.0, 104.0, 1024.0, 4096.0];

#[derive(Default, Debug)]
struct Search {
    cases: usize,
    /// Accepted hits on segments the certificate places off the triangle.
    phantom_hits: usize,
    /// Cases whose gap reached the margin: the claim's domain.
    certified: usize,
    /// Accepted hits with the gap at or past the margin: must stay zero.
    violations: usize,
    /// Largest gap / margin over accepted hits with a positive gap.
    worst_ratio: f64,
}

impl Search {
    fn record(&mut self, hit: bool, (gap, margin): (f64, f64)) {
        self.cases += 1;
        if gap >= margin {
            self.certified += 1;
        }
        if hit && gap > 0.0 {
            self.phantom_hits += 1;
            self.worst_ratio = self.worst_ratio.max(gap / margin);
            if gap >= margin {
                self.violations += 1;
            }
        }
    }
}

// Requirement 9 of the margin derivation: a segment that stops short of a
// triangle's plane, from either side and at any grazing angle, is never hit
// once its gap along the triangle's own normal reaches m_N. Segments end a
// random multiple of the hit test's rounding scale from the plane, over a
// triangle's interior, at coordinates up to 4 km.
#[test]
fn shaft_normal_margin_admits_no_accepted_hit_beyond_it() {
    let mut stream = Stream(0x5348_4146_545f_4e31);
    let mut search = Search::default();
    for _ in 0..150_000 {
        let offset = stream.pick(&OFFSETS);
        let centre = DVec3::new(
            offset * stream.range(-1.0, 1.0),
            offset * stream.range(-1.0, 1.0),
            offset * stream.range(-1.0, 1.0),
        );
        let size = stream.log_range(0.05, 60.0);
        let points = triangle(&mut stream, centre, size);
        let Some(candidate) = Candidate::of(points) else {
            continue;
        };
        let normal = candidate.normal.normalize();
        let [a, b, c] = candidate.corners;
        // A point over the triangle's interior.
        let (s, t) = (stream.unit(), stream.unit());
        let (s, t) = if s + t > 1.0 {
            (1.0 - s, 1.0 - t)
        } else {
            (s, t)
        };
        let foot = a + (b - a) * s + (c - a) * t;
        let side = if stream.unit() < 0.5 { 1.0 } else { -1.0 };
        let sin = stream.log_range(1.0e-5, 1.0);
        let along = (stream.direction() - normal * normal.dot(stream.direction()))
            .normalize_or(normal.any_orthonormal_vector());
        let toward_plane = along * (1.0 - sin * sin).sqrt() - normal * side * sin;
        let length = stream.log_range(0.01, 30.0);
        let scale = UNIT_ROUNDOFF * candidate.shape * (foot.distance(a) + 2.0 * length + offset);
        let height = scale * stream.log_range(1.0e-2, 60.0);
        let to = foot + normal * side * height;
        let from = to - toward_plane * length;
        let (from, to) = (from.as_vec3(), to.as_vec3());
        let bounds = segment_bounds(from, to);
        let certificate = bounds.normal_certificate(&candidate);
        search.record(traced_hit(from, to, points), certificate);
        search.record(lead_in_hit(from, to, points), certificate);
    }
    assert_eq!(search.violations, 0, "{search:?}");
    assert!(
        search.phantom_hits > 100 && search.certified > 10_000,
        "the search must reach the rounding regime and the margin: {search:?}"
    );
    assert!(search.worst_ratio > 0.01, "{search:?}");
}

// Requirement 9, off the normal: a segment at or above the angle floor that
// crosses a triangle's plane just outside one of its edges is never hit once
// its gap along that edge's outward normal reaches m_A. Steeper rays than the
// floor only; a ray below it is the accepted gap and is not asserted.
#[test]
fn shaft_axis_margin_admits_no_accepted_hit_beyond_it_above_the_angle_floor() {
    let mut stream = Stream(0x5348_4146_545f_4131);
    let mut search = Search::default();
    let mut below_floor = 0;
    for _ in 0..150_000 {
        let offset = stream.pick(&OFFSETS);
        let centre = DVec3::new(
            offset * stream.range(-1.0, 1.0),
            offset * stream.range(-1.0, 1.0),
            offset * stream.range(-1.0, 1.0),
        );
        let size = stream.log_range(0.05, 60.0);
        let points = triangle(&mut stream, centre, size);
        let Some(candidate) = Candidate::of(points) else {
            continue;
        };
        let normal = candidate.normal.normalize();
        let corners = candidate.corners;
        let edge = (stream.next() % 3) as usize;
        let (p0, p1, opposite) = (
            corners[edge],
            corners[(edge + 1) % 3],
            corners[(edge + 2) % 3],
        );
        let run = (p1 - p0).normalize();
        let mut outward = normal.cross(run);
        if outward.dot(opposite - p0) > 0.0 {
            outward = -outward;
        }
        // At or a little below the floor, mostly above it.
        let sin = stream.range(0.8 * SIN_ANGLE_FLOOR, 1.0);
        let length = stream.log_range(0.01, 30.0);
        let reach = p0.distance(corners[0]) + 2.0 * length + offset;
        let scale = UNIT_ROUNDOFF * candidate.shape * (3.0 * reach + candidate.edge_sum) / sin;
        let crossing = p0
            + (p1 - p0) * stream.range(0.05, 0.95)
            + outward * scale * stream.log_range(1.0e-2, 60.0);
        let sign = if stream.unit() < 0.5 { 1.0 } else { -1.0 };
        let dir = run * sign * (1.0 - sin * sin).sqrt() + normal * sin;
        let before = stream.range(0.1, 0.9);
        let from = (crossing - dir * length * before).as_vec3();
        let to = (crossing + dir * length * (1.0 - before)).as_vec3();
        if sin_grazing(from, to, &candidate) < SIN_ANGLE_FLOOR {
            below_floor += 1;
            continue;
        }
        let bounds = segment_bounds(from, to);
        let Some(certificate) = bounds.axis_certificate(&candidate, outward) else {
            continue;
        };
        search.record(traced_hit(from, to, points), certificate);
        search.record(lead_in_hit(from, to, points), certificate);
    }
    assert_eq!(search.violations, 0, "{search:?}");
    assert!(
        search.phantom_hits > 100 && search.certified > 10_000,
        "the search must reach the rounding regime and the margin: {search:?}"
    );
    assert!(search.worst_ratio > 0.01, "{search:?}");
    assert!(
        below_floor > 0,
        "some rays round below the floor and are skipped"
    );
}

// The query growth covers every triangle a margin could need: for a triangle
// at the growth's distance from the box, with the scene's largest shape
// terms, m_A along the axis between them is below that distance.
#[test]
fn shaft_query_growth_exceeds_the_axis_margin_of_a_triangle_that_far_away() {
    let hull = Hull {
        corners: [
            DVec3::new(0.0, 0.0005, 0.0),
            DVec3::new(0.3, 0.0005, 0.0),
            DVec3::new(0.0, 0.0005, 0.3),
            DVec3::new(0.3, 0.0005, 0.3),
            DVec3::new(0.0, 0.0025, 0.0),
            DVec3::new(0.3, 0.0025, 0.0),
            DVec3::new(0.0, 0.0025, 0.3),
            DVec3::new(0.3, 0.0025, 0.3),
        ],
        centre: DVec3::new(0.15, 2.0, 0.15),
        radius: 0.251,
        terms: DVec3::splat(0.3),
    };
    let bounds = HullBounds::new(hull);
    // A sliver at the shape cap, 3 m long, facing the hull across +X.
    let at = |x: f64| {
        let a = DVec3::new(x, 1.0, -1.0);
        let b = a + DVec3::new(0.0, 0.0, 3.0);
        let c = a + DVec3::new(0.0, 3.0 / SHAPE_CAP, 0.0);
        [a, b, c].map(|p| p.as_vec3())
    };
    let probe = Candidate::of(at(1.0)).unwrap();
    let scene = ShaftScene {
        shape: probe.shape,
        shape_edge: probe.shape * probe.longest_edge,
        edge: probe.longest_edge,
    };
    let growth = scene.query_growth(&bounds).unwrap();
    let candidate = Candidate::of(at(bounds.hi.x + growth)).unwrap();
    let (gap, margin) = bounds
        .axis_certificate(&candidate, DVec3::X)
        .expect("a unit axis");
    assert!(
        gap >= margin,
        "growth {growth}: a triangle that far has gap {gap} under its margin {margin}"
    );
    assert!(
        growth < 0.2,
        "growth {growth} stays far below the hull's size"
    );
}
