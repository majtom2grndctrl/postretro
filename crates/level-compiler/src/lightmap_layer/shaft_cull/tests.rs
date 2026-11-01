// Shaft-cull margins against the hit test: a randomized search of the watertight f64 test.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use glam::{DVec3, Vec3};

use super::margin::*;
use super::*;
use crate::ray_traversal::TraceRay;

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
    TraceRay::new(origin, dir)
        .hit([a, b, c], 0.0, max_distance)
        .is_some()
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
    TraceRay::new(from, dir)
        .hit([a, b, c], 0.0, length)
        .is_some()
}

/// Sine of the angle between the `f32` ray `from → to` and the triangle's
/// plane.
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
    /// Accepted hits on segments the separation places off the triangle.
    phantom_hits: usize,
    /// Cases whose gap reached the margin: the claim's domain.
    separated: usize,
    /// Accepted hits with the gap at or past the margin: must stay zero.
    violations: usize,
    /// Largest gap / margin over accepted hits with a positive gap.
    worst_ratio: f64,
}

impl Search {
    fn record(&mut self, hit: bool, (gap, margin): (f64, f64)) {
        self.cases += 1;
        if gap >= margin {
            self.separated += 1;
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

// The normal margin is sound against the hit test: a segment that stops short
// of a triangle's plane, from either side and at any grazing angle, is never
// hit once its gap along the triangle's own normal reaches m_N. Segments end a
// random multiple of their own `f32` ends' rounding scale from the plane,
// which bounds the hit test's height error, over a triangle's interior, at
// coordinates up to 4 km.
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
        let scale = UNIT_ROUNDOFF * (foot.abs().max_element() + offset + 6.0 * length);
        let height = scale * stream.log_range(1.0e-2, 1.0e3);
        let to = foot + normal * side * height;
        let from = to - toward_plane * length;
        let (from, to) = (from.as_vec3(), to.as_vec3());
        let bounds = segment_bounds(from, to);
        let separation = bounds.normal_separation(&candidate, bounds.reach(&candidate));
        search.record(traced_hit(from, to, points), separation);
        search.record(lead_in_hit(from, to, points), separation);
    }
    assert_eq!(search.violations, 0, "{search:?}");
    assert!(
        search.phantom_hits > 100 && search.separated > 10_000,
        "the search must reach the rounding regime and the margin: {search:?}"
    );
    assert!(search.worst_ratio > 0.01, "{search:?}");
}

// The axis margin is sound against the hit test above the angle floor: a
// segment at or above the floor that crosses a triangle's plane just outside
// one of its edges, or past one of its corners, is never hit once its gap
// reaches m_A, along the direction from the triangle to the crossing or
// along any other unit axis, N components included. Crossings land a random
// multiple of τ outside the edge, or past its first corner in that corner's
// normal cone, where the tolerance's region is round; so the tolerance's
// hits are the phantom ones, and the worst ratio shows they reach τ itself.
// Steeper rays than the floor only; a ray below it is the accepted gap and
// is not asserted.
#[test]
fn shaft_axis_margin_admits_no_accepted_hit_beyond_it_above_the_angle_floor() {
    let mut stream = Stream(0x5348_4146_545f_4131);
    // The other axes draw from their own stream, so the edge-normal cases
    // are the ones a search without them would draw.
    let mut axis_stream = Stream(0x5348_4146_545f_4132);
    let mut search = Search::default();
    // The corner cases alone, also recorded in `search`.
    let mut round_corners = Search::default();
    let mut other_axes = Search::default();
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
        let scale = f64::from(crate::ray_traversal::TRACER_TOLERANCE_METERS);
        let beyond = stream.log_range(1.0e-2, 60.0);
        // A quarter of the crossings lie past p0 instead, along a direction
        // in its normal cone, between the outward normals of its two edges:
        // the triangle's nearest point is p0 itself, so the gap along that
        // direction is the crossing's distance. The ray runs across it.
        let at_corner = stream.unit() < 0.25;
        let (foot, outward, run) = if at_corner {
            let mut other = normal.cross((opposite - p0).normalize());
            if other.dot(p1 - p0) > 0.0 {
                other = -other;
            }
            let mix = stream.unit();
            let toward = (outward * mix + other * (1.0 - mix)).normalize_or(outward);
            (p0, toward, normal.cross(toward))
        } else {
            (p0 + (p1 - p0) * stream.range(0.05, 0.95), outward, run)
        };
        let crossing = foot + outward * scale * beyond;
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
        let hits = [traced_hit(from, to, points), lead_in_hit(from, to, points)];
        if let Some(separation) = bounds.axis_separation(&candidate, outward) {
            for hit in hits {
                search.record(hit, separation);
                if at_corner {
                    round_corners.record(hit, separation);
                }
            }
        }
        // Mostly that direction tipped toward a random one, N included, by
        // up to a few times the angle that still keeps the gap open; now and
        // then a direction drawn at random.
        let axis = if axis_stream.unit() < 0.125 {
            axis_stream.direction()
        } else {
            let open = scale * beyond / (length + 2.0 * candidate.longest_edge);
            outward + axis_stream.direction() * open * axis_stream.log_range(1.0e-3, 3.0)
        };
        if let Some(separation) = bounds.axis_separation(&candidate, axis) {
            for hit in hits {
                other_axes.record(hit, separation);
            }
        }
    }
    assert_eq!(search.violations, 0, "{search:?}");
    assert!(
        search.phantom_hits > 100 && search.separated > 10_000,
        "the search must reach the rounding regime and the margin: {search:?}"
    );
    // m_A ≥ κτ, so a worst ratio past 0.9/κ means some accepted hit lay at
    // least 0.9τ from the triangle: the search reaches the tolerance's
    // boundary, not only its inside. Near the origin m_A is within a few
    // percent of κτ, so the best ratio approaches 1/κ.
    let boundary = 0.9 / KAPPA;
    assert!(
        search.worst_ratio > boundary,
        "tolerance hits must reach near τ: {search:?}"
    );
    assert!(
        round_corners.phantom_hits > 100 && round_corners.worst_ratio > boundary,
        "the corner cases must reach near τ too: {round_corners:?}"
    );
    assert_eq!(other_axes.violations, 0, "{other_axes:?}");
    assert!(
        other_axes.separated > 2_000,
        "the other axes must reach the margin: {other_axes:?}"
    );
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
    // A sliver just inside the shape cap, 3 m edges, facing the hull across
    // +X. Its narrow angle sits at its first corner, the one ρ measures.
    let sin = 1.0001 / SHAPE_CAP;
    let at = |x: f64| {
        let a = DVec3::new(x, 1.0, -1.0);
        let b = a + DVec3::new(0.0, 0.0, 3.0);
        let c = a + DVec3::new(0.0, sin, (1.0 - sin * sin).sqrt()) * 3.0;
        [a, b, c].map(|p| p.as_vec3())
    };
    let probe = Candidate::of(at(1.0)).unwrap();
    assert!(
        probe.shape <= SHAPE_CAP && probe.shape > 0.999 * SHAPE_CAP,
        "the probe sits just inside the cap: ρ = {}",
        probe.shape
    );
    let mut scene = ShaftScene::default();
    scene.include(&probe);
    let growth = scene.query_growth(&bounds).unwrap();
    let candidate = Candidate::of(at(bounds.hi.x + growth)).unwrap();
    let (gap, margin) = bounds
        .axis_separation(&candidate, DVec3::X)
        .expect("a unit axis");
    assert!(
        gap >= margin,
        "growth {growth}: a triangle that far has gap {gap} under its margin {margin}"
    );
    // About 1 mm: κτ dominates; the capped sliver's rounding terms are far
    // below it.
    assert!(
        (0.001..0.0011).contains(&growth),
        "growth {growth}: κτ sets it near 1 mm, far below the hull's size"
    );
}

// The strict `f64` terms match their derivation where they dominate: a 1 cm
// triangle 3 km from a 1 m segment at the origin, where R_T² carries the hit
// test's rounding past τ and the sampler's endpoint terms stay small. Both
// margins are recomputed from the forms margin.rs states, H as
// w·R_T·P(9A + 54P)/‖N‖ and the in-plane reach expanded in R_T, not from the
// margin code's own expressions, so a coefficient error in either fails. κ
// is read from the code: it is chosen, not derived.
#[test]
fn shaft_strict_terms_match_their_derivation_where_they_dominate() {
    let (w, u) = (2.0_f64.powi(-53), 2.0_f64.powi(-24));
    let tau = f64::from(crate::ray_traversal::TRACER_TOLERANCE_METERS);
    let sin_floor = 2.0_f64.to_radians().sin();
    let (from, to) = (Vec3::ZERO, Vec3::Y);
    let bounds = segment_bounds(from, to);
    let points = [
        Vec3::new(3000.0, 0.5, 0.5),
        Vec3::new(3000.0, 0.51, 0.5),
        Vec3::new(3000.0, 0.5, 0.51),
    ];
    let candidate = Candidate::of(points).unwrap();
    let [a, b, c] = points.map(Vec3::as_dvec3);
    let (e1, e2) = (b - a, c - a);
    let e_t = e1.length().max(e2.length());
    let area = e1.cross(e2).length();
    let (lo, hi) = (from.as_dvec3(), to.as_dvec3());
    let diag = (hi - lo).length();
    let r = a.distance((lo + hi) * 0.5) + 0.5 * diag + e_t;
    let magnitude = lo.abs().max(hi.abs());
    let point_rounding =
        w * (6.0 * (magnitude.max_element() + r) + 20.0 * (e_t + tau).powi(2) / tau);
    let epsilon = magnitude * (10.0 * u) + DVec3::splat(6.0 * u * diag);
    let relative = |margin: f64, expected: f64| ((margin - expected) / expected).abs();

    // m_N = κ[H_s + min(H, τ + δ_r) + e_H(n̂)].
    let height = |p: f64| w * r * p * (9.0 * (24.0 * r + 4.0 * p) + 54.0 * p) / area;
    let strict_height = height(4.0 * e_t);
    let expected = KAPPA
        * (strict_height
            + height(4.0 * e_t + 2.0 * tau).min(tau + point_rounding)
            + (candidate.normal / area).abs().dot(epsilon));
    let (_, margin) = bounds.normal_separation(&candidate, bounds.reach(&candidate));
    assert!(
        strict_height > 0.1 * tau,
        "H_s {strict_height} must be a large share of m_N {expected}"
    );
    assert!(
        relative(margin, expected) < 1.0e-12,
        "m_N {margin}, derived {expected}"
    );

    // m_A = κ[τ + δ_r + (w·q/sin θ0)(864R² + 2016e_T·R + 384e_T²) + e_H(a)],
    // along an in-plane axis.
    let reach =
        w * (e_t / area) / sin_floor * (864.0 * r * r + 2016.0 * e_t * r + 384.0 * e_t * e_t);
    let expected = KAPPA * (tau + point_rounding + reach + epsilon.y);
    let (_, margin) = bounds
        .axis_separation(&candidate, DVec3::Y)
        .expect("a unit axis");
    assert!(
        reach > tau,
        "the strict reach {reach} must outgrow τ here: m_A {expected}"
    );
    assert!(
        relative(margin, expected) < 1.0e-12,
        "m_A {margin}, derived {expected}"
    );
}

// The query growth's bound on m_A, κ(c₀ + c₁R + c₂R²), matches the margin
// and its derivation where each of its terms shows. With the scene holding
// one triangle, the bound at that triangle's own R_T is m_A along ε, the
// axis whose e_H(a) is all of ‖ε‖. Between them two cases give every term a
// visible share: a 1 cm triangle at the origin with the segment 3 km out
// (c₂R², and M and R_T in δ_r), and a 100 m sliver beside a segment at the
// origin (c₀'s strict term and δ_r's edge term). Both are also recomputed
// from the forms margin.rs states, so a coefficient wrong in the bound, or
// in both, fails.
#[test]
fn shaft_query_bound_matches_the_axis_margin_and_its_derivation_where_each_term_shows() {
    // Far above the `f64` evaluation's rounding; a term shows when its share
    // of the margin is a hundred times that.
    const AGREEMENT: f64 = 1.0e-12;
    let (w, u) = (2.0_f64.powi(-53), 2.0_f64.powi(-24));
    let tau = f64::from(crate::ray_traversal::TRACER_TOLERANCE_METERS);
    let sin_floor = 2.0_f64.to_radians().sin();
    let relative = |value: f64, expected: f64| ((value - expected) / expected).abs();
    let (sin, cos) = 1.0_f64.to_radians().sin_cos();
    let sliver = [
        DVec3::new(0.5, 0.5, 0.5),
        DVec3::new(100.5, 0.5, 0.5),
        DVec3::new(0.5 + 100.0 * cos, 0.5 + 100.0 * sin, 0.5),
    ];
    /// A case's name, its segment, its triangle and the terms it shows.
    type Case<'a> = (&'a str, (Vec3, Vec3), [Vec3; 3], &'a [&'a str]);
    let cases: [Case; 2] = [
        (
            "1 cm triangle, segment 3 km out",
            (Vec3::new(3000.0, 0.0, 0.0), Vec3::new(3000.0, 1.0, 0.0)),
            [
                Vec3::new(0.0, 0.5, 0.5),
                Vec3::new(0.0, 0.51, 0.5),
                Vec3::new(0.0, 0.5, 0.51),
            ],
            &["c₂R²", "c₁R, strict", "δ_r, coordinates"],
        ),
        (
            "100 m sliver beside the segment",
            (Vec3::ZERO, Vec3::Y),
            sliver.map(|p| p.as_vec3()),
            &["c₂R²", "c₁R, strict", "c₀, strict", "δ_r, edge"],
        ),
    ];
    for (label, (from, to), points, shows) in cases {
        let bounds = segment_bounds(from, to);
        let candidate = Candidate::of(points).unwrap();
        let mut scene = ShaftScene::default();
        scene.include(&candidate);

        // m_A = κ[τ + δ_r + (w·q/sin θ0)(864R² + 2016e_T·R + 384e_T²) + ‖ε‖],
        // δ_r = w(6(M + R) + 20(e_T + τ)²/τ).
        let [a, b, c] = points.map(Vec3::as_dvec3);
        let (e1, e2) = (b - a, c - a);
        let e_t = e1.length().max(e2.length());
        let q = e_t / e1.cross(e2).length();
        let (lo, hi) = (from.as_dvec3(), to.as_dvec3());
        let (lo, hi) = (lo.min(hi), lo.max(hi));
        let diag = (hi - lo).length();
        let r = a.distance((lo + hi) * 0.5) + 0.5 * diag + e_t;
        let magnitude = lo.abs().max(hi.abs());
        let epsilon = magnitude * (10.0 * u) + DVec3::splat(6.0 * u * diag);
        let strict = w * q / sin_floor;
        let terms = [
            ("τ", tau),
            ("δ_r, coordinates", w * 6.0 * (magnitude.max_element() + r)),
            ("δ_r, edge", w * 20.0 * (e_t + tau).powi(2) / tau),
            ("c₂R²", strict * 864.0 * r * r),
            ("c₁R, strict", strict * 2016.0 * e_t * r),
            ("c₀, strict", strict * 384.0 * e_t * e_t),
            ("‖ε‖", epsilon.length()),
        ];
        let expected = KAPPA * terms.iter().map(|(_, term)| term).sum::<f64>();
        for name in shows {
            let (_, term) = terms
                .iter()
                .find(|(term_name, _)| term_name == name)
                .expect("a named term");
            assert!(
                KAPPA * term > 100.0 * AGREEMENT * expected,
                "{label}: {name}, {term}, is too small a share of m_A {expected} to show"
            );
        }

        let [m0, m1, m2] = scene.axis_margin_bound(&bounds);
        let bound = m0 + m1 * r + m2 * r * r;
        let (_, margin) = bounds
            .axis_separation(&candidate, epsilon)
            .expect("a unit axis");
        assert!(
            relative(margin, expected) < AGREEMENT,
            "{label}: m_A {margin}, derived {expected}"
        );
        assert!(
            relative(bound, expected) < AGREEMENT,
            "{label}: query bound {bound}, derived {expected}"
        );
        assert!(
            bound >= margin * (1.0 - AGREEMENT) && relative(bound, margin) < AGREEMENT,
            "{label}: query bound {bound}, m_A {margin}"
        );
    }
}

// The hull-face axes: the plane through a slab edge tangent to the emitter
// touches the sphere and supports the whole hull at that edge, so a triangle
// just beyond the face is separated on it by the full gap.
#[test]
fn shaft_tangent_plane_through_a_slab_edge_is_a_face_of_the_hull() {
    let corners = [0.0005, 0.0025].map(|y| {
        [
            DVec3::new(0.0, y, 0.0),
            DVec3::new(0.3, y, 0.0),
            DVec3::new(0.0, y, 0.3),
            DVec3::new(0.3, y, 0.3),
        ]
    });
    for (centre, radius) in [
        (DVec3::new(0.15, 2.0, 0.15), 0.751),
        (DVec3::new(0.2, 1.0, 0.1), 0.0),
    ] {
        let hull = Hull {
            corners: [corners[0], corners[1]].concat().try_into().unwrap(),
            centre,
            radius,
            terms: DVec3::splat(0.3),
        };
        // The side at x = 0, run along z, outward -x: through the lower edge
        // when the face leans out over a wide sphere, the upper one otherwise.
        let edge = if radius > 0.0 {
            hull.corners[0]
        } else {
            hull.corners[4]
        };
        let normal = hull
            .tangent_plane(edge, DVec3::Z, -DVec3::X)
            .expect("the edge's line misses the sphere");
        assert!((normal.length() - 1.0).abs() < 1e-12, "{normal}");
        assert!(normal.x < 0.0, "outward: {normal}");
        let offset = normal.dot(edge);
        assert!(
            (normal.dot(centre) + radius - offset).abs() < 1e-12,
            "touches the emitter"
        );
        let (_, hi) = hull.range(normal);
        assert!(
            (hi - offset).abs() < 1e-12,
            "supports the hull: {hi} vs {offset}"
        );
    }
}
