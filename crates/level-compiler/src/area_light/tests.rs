// Area-light maths against the research oracle (research.md §1, §2, Pin table O6, O8, O20, O23).

use std::f64::consts::PI;

use glam::{DVec2, DVec3};

use super::*;

const DOWN: DVec3 = DVec3::NEG_Y;
const UP: DVec3 = DVec3::Y;

/// Horizontal rectangle at height `y` facing down, x ∈ [x0, x1], z ∈ [z0, z1].
fn ceiling_rect(x0: f64, x1: f64, z0: f64, z1: f64, y: f64) -> EmitterPolygon {
    EmitterPolygon::new(
        vec![
            DVec3::new(x0, y, z0),
            DVec3::new(x1, y, z0),
            DVec3::new(x1, y, z1),
            DVec3::new(x0, y, z1),
        ],
        DOWN,
    )
}

/// The §1 oracle's emitter: a unit square centred over the origin at height `h`.
fn unit_square(h: f64) -> EmitterPolygon {
    ceiling_rect(-0.5, 0.5, -0.5, 0.5, h)
}

/// A 4 × 4 m panel centred over the origin at height `h`.
fn panel_4x4(h: f64) -> EmitterPolygon {
    ceiling_rect(-2.0, 2.0, -2.0, 2.0, h)
}

fn emitter(polygons: Vec<EmitterPolygon>) -> AreaEmitter {
    AreaEmitter::new(polygons).expect("test emitter has positive area")
}

/// Four outward faces of a 0.5 × 0.5 × 3 m pillar: north (+z), south, east (+x), west.
fn pillar_wrap() -> AreaEmitter {
    let (lo, hi, top) = (-0.25, 0.25, 3.0);
    let face = |a: DVec3, b: DVec3, normal: DVec3| {
        EmitterPolygon::new(vec![a, b, b + DVec3::Y * top, a + DVec3::Y * top], normal)
    };
    emitter(vec![
        face(DVec3::new(lo, 0.0, hi), DVec3::new(hi, 0.0, hi), DVec3::Z),
        face(
            DVec3::new(hi, 0.0, lo),
            DVec3::new(lo, 0.0, lo),
            DVec3::NEG_Z,
        ),
        face(DVec3::new(hi, 0.0, hi), DVec3::new(hi, 0.0, lo), DVec3::X),
        face(
            DVec3::new(lo, 0.0, lo),
            DVec3::new(lo, 0.0, hi),
            DVec3::NEG_X,
        ),
    ])
}

/// Regular hexagon of circumradius `r` at height `h`, facing down (fan sampler path).
fn hexagon(r: f64, h: f64) -> EmitterPolygon {
    let vertices = (0..6)
        .map(|i| {
            let angle = f64::from(i) * PI / 3.0;
            DVec3::new(r * angle.cos(), h, r * angle.sin())
        })
        .collect();
    EmitterPolygon::new(vertices, DOWN)
}

/// Centred-square closed form: E = 4X/√(1+X²)·atan(X/√(1+X²)), X = s/(2h).
fn closed_form_centred_square(side: f64, h: f64) -> f64 {
    let x = side / (2.0 * h);
    let root = (1.0 + x * x).sqrt();
    4.0 * x / root * (x / root).atan()
}

/// Midpoint quadrature of L ∫ cosθ_e · max(0, cosθ_r) / d² dA over a
/// down-facing ceiling rectangle, independent of the edge formula.
fn quadrature(x0: f64, x1: f64, z0: f64, z1: f64, y: f64, p: DVec3, n: DVec3) -> f64 {
    const CELLS: usize = 1000;
    let (dx, dz) = ((x1 - x0) / CELLS as f64, (z1 - z0) / CELLS as f64);
    let mut sum = 0.0;
    for i in 0..CELLS {
        for j in 0..CELLS {
            let q = DVec3::new(x0 + (i as f64 + 0.5) * dx, y, z0 + (j as f64 + 0.5) * dz);
            let w = q - p;
            let d2 = w.length_squared();
            let d = d2.sqrt();
            let cos_e = w.y / d;
            let cos_r = (w.dot(n) / d).max(0.0);
            sum += cos_e * cos_r / d2;
        }
    }
    sum * dx * dz
}

fn tilted(degrees: f64) -> DVec3 {
    let t = degrees.to_radians();
    DVec3::new(t.sin(), t.cos(), 0.0)
}

fn assert_rel(actual: f64, expected: f64, tolerance: f64, what: &str) {
    let rel = ((actual - expected) / expected).abs();
    assert!(
        rel <= tolerance,
        "{what}: {actual} vs {expected} (relative {rel:e} > {tolerance:e})"
    );
}

fn assert_abs(actual: f64, expected: f64, tolerance: f64, what: &str) {
    let diff = (actual - expected).abs();
    assert!(
        diff <= tolerance,
        "{what}: {actual} vs {expected} (diff {diff:e} > {tolerance:e})"
    );
}

/// Cell centres of an n × n lattice over [0,1)².
fn lattice(n: usize) -> impl Iterator<Item = DVec2> {
    (0..n * n).map(move |k| {
        DVec2::new(
            ((k % n) as f64 + 0.5) / n as f64,
            ((k / n) as f64 + 0.5) / n as f64,
        )
    })
}

// --- Polygon term (acceptance rows 1–4) ---

#[test]
fn polygon_irradiance_on_axis_matches_closed_form_and_oracle_table() {
    let table = [
        (0.1, 3.042128),
        (0.5, 1.740840),
        (1.0, 0.752275),
        (2.0, 0.230837),
        (5.0, 0.039474),
        (10.0, 0.009967),
    ];
    for (h, oracle) in table {
        let (e, phi) = polygon_irradiance(&unit_square(h), DVec3::ZERO, UP);
        assert_rel(e, closed_form_centred_square(1.0, h), 1e-9, "closed form");
        assert_abs(e, oracle, 5.0e-7, "oracle table");
        assert_rel(phi.y, e, 1e-12, "irradiance vector points at the panel");
    }
}

#[test]
fn polygon_irradiance_tilted_receiver_clips_to_horizon() {
    let table = [
        (0.0, 0.752275),
        (30.0, 0.651489),
        (60.0, 0.376137),
        (85.0, 0.123245),
        (90.0, 0.087510),
    ];
    for (degrees, oracle) in table {
        let n = tilted(degrees);
        let (e, _) = polygon_irradiance(&unit_square(1.0), DVec3::ZERO, n);
        assert_abs(e, oracle, 5.0e-7, "oracle table");
        let reference = quadrature(-0.5, 0.5, -0.5, 0.5, 1.0, DVec3::ZERO, n);
        assert_rel(e, reference, 1e-5, "midpoint quadrature");
    }
    // Wholly below the horizon: exactly 0, where the unclipped Φ·n is negative.
    let n = tilted(120.0);
    let (e, phi) = polygon_irradiance(&unit_square(1.0), DVec3::ZERO, n);
    assert_eq!(e, 0.0);
    assert_eq!(phi, DVec3::ZERO);
    assert!(polygon_irradiance_vector(&unit_square(1.0), DVec3::ZERO).dot(n) < 0.0);
}

#[test]
fn polygon_irradiance_off_axis_and_edge_line_receivers_are_finite_and_exact() {
    let square = unit_square(1.0);
    let cases = [
        (DVec3::new(2.0, 0.0, 0.0), UP, 0.0437426, 5.0e-8),
        (DVec3::new(-0.5, 0.0, -2.0), UP, 0.0396548, 5.0e-8),
        // Grazing: 0.1 mm below the emitter's plane, facing it.
        (
            DVec3::new(2.0, 1.0 - 1.0e-4, 0.0),
            DVec3::NEG_X,
            1.3554e-5,
            5.0e-10,
        ),
    ];
    for (p, n, oracle, printed) in cases {
        let (e, phi) = polygon_irradiance(&square, p, n);
        assert!(e.is_finite() && phi.is_finite(), "finite at {p}");
        assert_abs(e, oracle, printed, "oracle table");
        let reference = quadrature(-0.5, 0.5, -0.5, 0.5, 1.0, p, n);
        assert_rel(e, reference, 1e-5, "midpoint quadrature");
    }
}

#[test]
fn polygon_irradiance_coplanar_behind_and_own_face_receivers_read_zero() {
    let square = unit_square(1.0);
    let zero = (0.0, DVec3::ZERO);
    for n in [UP, DOWN, DVec3::X, tilted(45.0)] {
        // Coplanar outside, behind, and on the emitter's own face.
        assert_eq!(
            polygon_irradiance(&square, DVec3::new(2.0, 1.0, 0.0), n),
            zero
        );
        assert_eq!(
            polygon_irradiance(&square, DVec3::new(0.0, 2.0, 0.0), n),
            zero
        );
        assert_eq!(
            polygon_irradiance(&square, DVec3::new(0.1, 1.0, 0.2), n),
            zero
        );
    }
    for p in [DVec3::new(0.0, 2.0, 0.0), DVec3::new(0.1, 1.0, 0.2)] {
        assert_eq!(polygon_irradiance_vector(&square, p), DVec3::ZERO);
        assert_eq!(polygon_solid_angle(&square, p), 0.0);
    }
    // A receiver 1 mm under the centre approaches π·L.
    let (e, _) = polygon_irradiance(&square, DVec3::new(0.0, 1.0 - 1.0e-3, 0.0), UP);
    assert_abs(e, PI, 1e-4, "1 mm under the centre");
}

#[test]
fn polygon_irradiance_small_emitter_tends_to_point_light() {
    let centre = DVec3::new(0.8, 3.5, -1.1);
    let n_e = DVec3::new(-0.2, -1.0, 0.3).normalize();
    let n_r = DVec3::new(0.3, 1.0, 0.1).normalize();
    let (t1, t2) = n_e.any_orthonormal_pair();
    for (side, tolerance) in [(0.1, 1e-3), (0.01, 1e-5), (0.001, 1e-7)] {
        let half = side / 2.0;
        let square = EmitterPolygon::new(
            vec![
                centre - t1 * half - t2 * half,
                centre + t1 * half - t2 * half,
                centre + t1 * half + t2 * half,
                centre - t1 * half + t2 * half,
            ],
            n_e,
        );
        let to_light = centre;
        let d = to_light.length();
        let cos_e = (-to_light).dot(n_e) / d;
        let cos_r = to_light.dot(n_r) / d;
        let point = side * side * cos_e * cos_r / (d * d);
        let (e, _) = polygon_irradiance(&square, DVec3::ZERO, n_r);
        assert_rel(e, point, tolerance, "small-emitter limit");
    }
}

#[test]
fn polygon_irradiance_far_from_origin_f32_corners_match_origin_geometry() {
    let base = DVec3::new(1000.0, 2000.0, 12.0);
    let offsets = [
        DVec3::new(-0.7, 1.3, -0.45),
        DVec3::new(0.55, 1.3, -0.45),
        DVec3::new(0.55, 1.3, 0.8),
        DVec3::new(-0.7, 1.3, 0.8),
    ];
    // Corners and receiver as a level stores them: f32 world positions.
    let to_f32 = |v: DVec3| v.as_vec3().as_dvec3();
    let far: Vec<DVec3> = offsets.iter().map(|&o| to_f32(base + o)).collect();
    let near: Vec<DVec3> = far.iter().map(|&v| v - base).collect();
    let receiver_far = to_f32(base + DVec3::new(0.123, 0.0, 0.0371));
    let receiver_near = receiver_far - base;
    let n = tilted(20.0);
    let (e_far, phi_far) = polygon_irradiance(&EmitterPolygon::new(far, DOWN), receiver_far, n);
    let (e_near, phi_near) = polygon_irradiance(&EmitterPolygon::new(near, DOWN), receiver_near, n);
    assert!(e_near > 0.1);
    assert_rel(e_far, e_near, 1e-9, "f32 corners near 1 km");
    assert!((phi_far - phi_near).length() <= 1e-9 * phi_near.length());
}

#[test]
fn polygon_solid_angle_and_vector_match_closed_forms() {
    // Centred a × b rectangle at distance h: Ω = 4·asin(ab / √((a² + 4h²)(b² + 4h²))).
    let (a, b, h): (f64, f64, f64) = (1.0, 1.0, 1.0);
    let expected = 4.0 * (a * b / ((a * a + 4.0 * h * h) * (b * b + 4.0 * h * h)).sqrt()).asin();
    assert_rel(
        polygon_solid_angle(&unit_square(h), DVec3::ZERO),
        expected,
        1e-12,
        "Ω",
    );
    // A receiver whose horizon holds the whole polygon reads Φ·n, unclipped.
    let p = DVec3::new(0.3, -0.4, 0.2);
    let n = tilted(10.0);
    let (e, phi) = polygon_irradiance(&unit_square(1.0), p, n);
    let unclipped = polygon_irradiance_vector(&unit_square(1.0), p);
    assert!((phi - unclipped).length() <= 1e-14);
    assert_rel(unclipped.dot(n), e, 1e-12, "E = Φ·n");
}

#[test]
fn area_emitter_rejects_empty_and_zero_area() {
    assert!(AreaEmitter::new(Vec::new()).is_none());
    let sliver = EmitterPolygon::new(vec![DVec3::ZERO, DVec3::X, DVec3::X * 2.0], DOWN);
    assert!(AreaEmitter::new(vec![sliver]).is_none());
}

#[test]
fn area_emitter_tiled_panel_matches_single_panel() {
    let one = emitter(vec![panel_4x4(3.0)]);
    let four = emitter(vec![
        ceiling_rect(-2.0, 0.0, -2.0, 0.0, 3.0),
        ceiling_rect(0.0, 2.0, -2.0, 0.0, 3.0),
        ceiling_rect(0.0, 2.0, 0.0, 2.0, 3.0),
        ceiling_rect(-2.0, 0.0, 0.0, 2.0, 3.0),
    ]);
    assert_rel(four.total_area, one.total_area, 1e-12, "area");
    assert!((four.centroid - one.centroid).length() <= 1e-12);
    assert_rel(four.circumradius, one.circumradius, 1e-12, "circumradius");
    assert_eq!(
        derived_reach(1.0, four.total_area),
        derived_reach(1.0, one.total_area)
    );
    let receivers = [
        (DVec3::ZERO, UP),
        (DVec3::new(1.3, 0.5, -0.7), tilted(35.0)),
        (DVec3::new(3.0, 1.5, 0.5), DVec3::NEG_X),
        (DVec3::new(0.2, 2.0, 0.1), DVec3::X),
        (DVec3::new(-5.0, 0.0, 6.0), UP),
    ];
    for (p, n) in receivers {
        let (e_one, _) = one.irradiance(p, n);
        let (e_four, _) = four.irradiance(p, n);
        assert_rel(e_four, e_one, 1e-5, "tiled irradiance");
        assert_rel(four.solid_angle(p), one.solid_angle(p), 1e-5, "tiled Ω");
        assert_rel(
            four.nearest_distance(p),
            one.nearest_distance(p),
            1e-12,
            "distance",
        );
    }
}

#[test]
fn area_emitter_nearest_distance_is_point_to_polygon() {
    let panel = emitter(vec![panel_4x4(3.0)]);
    assert_rel(
        panel.nearest_distance(DVec3::new(1.0, 0.0, -1.0)),
        3.0,
        1e-12,
        "under",
    );
    assert_rel(
        panel.nearest_distance(DVec3::new(1.0, 5.0, 0.0)),
        2.0,
        1e-12,
        "above",
    );
    // Beyond a corner: distance to that corner.
    let p = DVec3::new(5.0, 3.0, 6.0);
    assert_rel(panel.nearest_distance(p), 5.0, 1e-12, "beyond corner");
    let pillar = pillar_wrap();
    assert_rel(
        pillar.nearest_distance(DVec3::new(0.0, 1.0, 2.25)),
        2.0,
        1e-12,
        "pillar",
    );
}

// --- Reach and window (rows 3, 13) ---

#[test]
fn derived_reach_scales_with_root_of_intensity_and_area() {
    assert_rel(derived_reach(1.0, 1.0), 16.0, 1e-4, "1 m² reach");
    assert_rel(derived_reach(1.0, 16.0), 64.0, 1e-12, "16 m² reach");
    assert_rel(
        derived_reach(2.0, 1.0) / derived_reach(1.0, 1.0),
        2f64.sqrt(),
        1e-12,
        "doubling intensity",
    );
    assert_eq!(derived_reach(0.0, 1.0), 0.0);
    assert_eq!(derived_reach(-1.0, 1.0), 0.0);
}

#[test]
fn reach_window_hits_half_reach_value_and_zero_at_reach() {
    assert_rel(reach_window(8.0, 16.0), 0.87890625, 1e-12, "half reach");
    assert_eq!(reach_window(16.0, 16.0), 0.0);
    assert_eq!(reach_window(40.0, 16.0), 0.0);
    assert_eq!(reach_window(0.0, 16.0), 1.0);
    assert_eq!(reach_window(1.0, 0.0), 0.0);
}

#[test]
fn windowed_irradiance_on_axis_matches_point_light_far_and_dims_near() {
    // A 1 m², intensity-1 panel: radiance 1, so the term is the lit value.
    let reach = derived_reach(1.0, 1.0);
    let at = |h: f64| {
        emitter(vec![unit_square(h)])
            .windowed_irradiance(DVec3::ZERO, UP, reach)
            .0
    };
    let point_light = |d: f64| 1.0 / (d * d);
    assert_rel(at(4.0) / point_light(4.0), 0.972, 1e-3, "4 m on axis");
    assert!(at(1.0) < point_light(1.0));
    // An override shorter than the derived reach: 0.879 of the unwindowed term
    // at half of it, 0 past it.
    let panel = emitter(vec![unit_square(2.5)]);
    let unwindowed = panel.irradiance(DVec3::ZERO, UP).0;
    let windowed = panel.windowed_irradiance(DVec3::ZERO, UP, 5.0).0;
    assert_rel(
        windowed / unwindowed,
        0.87890625,
        1e-12,
        "half override reach",
    );
    assert_eq!(panel.windowed_irradiance(DVec3::ZERO, UP, 2.5).0, 0.0);
}

#[test]
fn windowed_irradiance_falls_continuously_to_zero_at_reach() {
    let reach = derived_reach(1.0, 1.0);
    let mut previous = f64::INFINITY;
    let steps = 16_000;
    for step in 1..=steps + 100 {
        let h = step as f64 * reach / steps as f64;
        let (e, phi) = emitter(vec![unit_square(h)]).windowed_irradiance(DVec3::ZERO, UP, reach);
        assert!(e.is_finite() && phi.is_finite());
        assert!(e <= previous, "rises at {h}: {e} > {previous}");
        if h < reach {
            // No visible step: neighbouring samples 1 mm apart differ by little.
            assert!(
                previous == f64::INFINITY || previous - e < 0.02,
                "step at {h}"
            );
        } else {
            assert_eq!(e, 0.0, "lit past reach at {h}");
        }
        previous = e;
    }
}

#[test]
fn emitter_and_reach_spheres_hold_polygons_and_reach() {
    let panel = emitter(vec![panel_4x4(3.0)]);
    let (centre, radius) = panel.emitter_sphere();
    assert!((centre - DVec3::new(0.0, 3.0, 0.0)).length() <= 1e-12);
    assert_rel(radius, 8f64.sqrt(), 1e-12, "circumradius");
    let (reach_centre, reach_radius) = panel.reach_sphere(10.0);
    assert_eq!(reach_centre, centre);
    assert_rel(reach_radius, 8f64.sqrt() + 10.0, 1e-12, "reach sphere");
    // A pillar wrap's centroid sits inside the pillar.
    let (pillar_centre, _) = pillar_wrap().emitter_sphere();
    assert!((pillar_centre - DVec3::new(0.0, 1.5, 0.0)).length() <= 1e-12);
}

// --- Sampler (rows 18, 25 and the cores of 6, 10, 35) ---

#[test]
fn area_sampler_targets_stay_in_emitter_sphere_on_faced_polygons() {
    let shapes = [
        ("4 x 4 panel", emitter(vec![panel_4x4(3.0)])),
        (
            "4 x 0.1 strip",
            emitter(vec![ceiling_rect(-2.0, 2.0, -0.05, 0.05, 3.0)]),
        ),
        ("pillar wrap", pillar_wrap()),
        ("hexagon", emitter(vec![hexagon(1.5, 3.0)])),
    ];
    let receivers = [
        (DVec3::ZERO, Some(UP)),
        (DVec3::new(1.7, 0.0, -1.9), Some(UP)),
        (DVec3::new(5.0, 0.0, 3.0), Some(UP)),
        (DVec3::new(2.5, 1.5, 0.0), Some(DVec3::NEG_X)),
        (DVec3::new(0.0, 1.5, 2.0), Some(DVec3::NEG_Z)),
        (
            DVec3::new(2.0, 1.5, 2.0),
            Some(DVec3::new(-1.0, 0.0, -1.0).normalize()),
        ),
        (DVec3::new(0.26, 2.9, 0.26), None),
        (DVec3::new(-3.0, 2.999, 0.4), None),
    ];
    let lift = target_lift_meters();
    for (name, light) in &shapes {
        let (centre, radius) = light.emitter_sphere();
        let mut sampled = 0;
        for &(p, n) in &receivers {
            let Some(sampler) = AreaSampler::new(light, p, n) else {
                continue;
            };
            for u in lattice(8) {
                let s = sampler.sample(u);
                sampled += 1;
                let poly = &light.polygons[s.polygon];
                assert!(s.target.is_finite() && s.direction.is_finite(), "{name}");
                assert!(
                    s.target.distance(centre) <= radius + 1.0e-3,
                    "{name}: target {} outside emitter sphere",
                    s.target
                );
                assert!(
                    poly.signed_distance(p) > 0.0,
                    "{name}: sampled a polygon {p} is behind"
                );
                assert_abs(poly.signed_distance(s.target), lift, 1e-9, name);
                let on_polygon = s.target - poly.normal * lift;
                assert!(
                    polygon_distance(poly, on_polygon) <= 1e-7,
                    "{name}: off polygon"
                );
                assert!((0.0..=1.0).contains(&s.weight), "{name}");
                assert_rel(s.direction.length(), 1.0, 1e-12, name);
            }
        }
        assert!(sampled > 0, "{name}: no receiver sampled");
    }
}

#[test]
fn area_sampler_north_of_pillar_draws_only_north_face() {
    let pillar = pillar_wrap();
    let p = DVec3::new(0.0, 1.5, 2.0);
    let sampler = AreaSampler::new(&pillar, p, Some(DVec3::NEG_Z)).expect("north face faces p");
    assert_rel(
        sampler.total_solid_angle(),
        polygon_solid_angle(&pillar.polygons[0], p),
        1e-15,
        "only the north face",
    );
    assert!(lattice(16).all(|u| sampler.sample(u).polygon == 0));
    // A receiver inside the pillar faces nothing.
    assert!(AreaSampler::new(&pillar, DVec3::new(0.0, 1.5, 0.0), None).is_none());
}

/// One point per cell of an n × n grid over [0,1)², jittered inside its cell by
/// a fixed xorshift stream. Cell centres alone would under-test the fan path:
/// a polygon or triangle pick splits u.x strata off-centre, a lattice
/// quadrature error (1.7% on the hexagon at 64²) that jitter averages out.
fn jittered(n: usize, seed: u64) -> impl Iterator<Item = DVec2> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    (0..n * n).map(move |k| {
        DVec2::new(
            ((k % n) as f64 + next()) / n as f64,
            ((k / n) as f64 + next()) / n as f64,
        )
    })
}

/// Ω · mean(w) over jittered strata: the sampled clipped irradiance per unit L.
fn sampled_irradiance(light: &AreaEmitter, p: DVec3, n: DVec3, grid: usize) -> f64 {
    let sampler = AreaSampler::new(light, p, Some(n)).expect("receiver faces the light");
    let sum: f64 = jittered(grid, 7).map(|u| sampler.sample(u).weight).sum();
    sampler.total_solid_angle() * sum / (grid * grid) as f64
}

#[test]
fn area_sampler_is_uniform_in_solid_angle() {
    let panel = emitter(vec![panel_4x4(3.0)]);
    let hex = emitter(vec![hexagon(1.5, 2.0)]);
    let trapezoid = emitter(vec![EmitterPolygon::new(
        vec![
            DVec3::new(-2.0, 2.5, -1.0),
            DVec3::new(2.0, 2.5, -1.0),
            DVec3::new(1.0, 2.5, 1.5),
            DVec3::new(-0.5, 2.5, 1.5),
        ],
        DOWN,
    )]);
    let pillar = pillar_wrap();
    let cases = [
        ("floor under panel", &panel, DVec3::new(0.5, 0.0, -0.8), UP),
        (
            "wall off axis",
            &panel,
            DVec3::new(3.0, 1.5, 0.5),
            DVec3::NEG_X,
        ),
        (
            "horizon cuts panel",
            &panel,
            DVec3::new(0.0, 2.0, 0.0),
            DVec3::X,
        ),
        ("hexagon fan", &hex, DVec3::new(0.4, 1.0, 0.3), tilted(25.0)),
        ("trapezoid fan", &trapezoid, DVec3::new(-0.3, 1.2, 0.2), UP),
        (
            "pillar diagonal",
            &pillar,
            DVec3::new(1.5, 1.0, 1.5),
            DVec3::new(-1.0, 0.0, -1.0).normalize(),
        ),
    ];
    for (name, light, p, n) in cases {
        let exact = light.irradiance(p, n).0;
        assert!(exact > 0.0, "{name}");
        assert_rel(sampled_irradiance(light, p, n, 64), exact, 1e-2, name);
        // Probes: Ω · mean(direction) is the unclipped irradiance vector.
        let sampler = AreaSampler::new(light, p, None).expect("faced");
        let mean: DVec3 = jittered(64, 11)
            .map(|u| sampler.sample(u).direction)
            .sum::<DVec3>()
            / 4096.0;
        let phi = light.irradiance_vector(p);
        let error = (mean * sampler.total_solid_angle() - phi).length();
        assert!(
            error <= 1e-2 * phi.length(),
            "{name}: probe vector off by {error}"
        );
    }
}

#[test]
fn area_sampler_below_horizon_samples_store_zero_visibility() {
    // A receiver facing away from the panel: every faced sample has weight 0,
    // so Σw = 0 stores 0 without dividing, and the term is 0 too (O8, O20).
    let panel = emitter(vec![panel_4x4(1.0)]);
    let sampler = AreaSampler::new(&panel, DVec3::ZERO, Some(DOWN)).expect("faced");
    let samples: Vec<_> = lattice(8)
        .map(|u| (sampler.sample(u).weight, true))
        .collect();
    assert!(samples.iter().all(|&(w, _)| w == 0.0));
    assert_eq!(weighted_visibility(samples), 0.0);
    assert_eq!(panel.irradiance(DVec3::ZERO, DOWN).0, 0.0);
}

// --- Weighted and one-sample visibility (rows 5, 6, 35 cores) ---

#[test]
fn weighted_visibility_is_exact_at_the_extremes() {
    assert_eq!(weighted_visibility(Vec::new()), 0.0);
    assert_eq!(weighted_visibility([(0.0, true), (0.0, false)]), 0.0);
    let weights = [0.3, 0.1, 0.7, 1.0e-9, 0.123456789, 0.0];
    assert_eq!(weighted_visibility(weights.map(|w| (w, true))), 1.0);
    assert_eq!(weighted_visibility(weights.map(|w| (w, false))), 0.0);
    assert_eq!(weighted_visibility([(f64::NAN, true), (-1.0, true)]), 0.0);
}

#[test]
fn weighted_visibility_orders_near_and_far_half_occluders_by_integrand() {
    // Receiver 1 m under a 4 × 4 panel, 1 m toward its x < 0 half.
    let panel = emitter(vec![panel_4x4(1.0)]);
    let p = DVec3::new(-1.0, 0.0, 0.0);
    let sampler = AreaSampler::new(&panel, p, Some(UP)).expect("faced");
    let samples: Vec<AreaSample> = lattice(32).map(|u| sampler.sample(u)).collect();
    let near_blocked = weighted_visibility(samples.iter().map(|s| (s.weight, s.target.x >= 0.0)));
    let far_blocked = weighted_visibility(samples.iter().map(|s| (s.weight, s.target.x < 0.0)));
    let full = panel.irradiance(p, UP).0;
    let far_share = polygon_irradiance(&ceiling_rect(0.0, 2.0, -2.0, 2.0, 1.0), p, UP).0 / full;
    assert_abs(near_blocked, far_share, 1e-2, "near half blocked");
    assert_abs(far_blocked, 1.0 - far_share, 1e-2, "far half blocked");
    assert!(near_blocked < far_blocked);
    // An unweighted fraction over the panel's area ties them.
    let area_points: Vec<f64> = lattice(32).map(|u| -2.0 + 4.0 * u.x).collect();
    let clear_fraction = |clear: fn(f64) -> bool| {
        area_points.iter().filter(|&&x| clear(x)).count() as f64 / area_points.len() as f64
    };
    assert_eq!(clear_fraction(|x| x >= 0.0), clear_fraction(|x| x < 0.0));
}

#[test]
fn one_sample_visibility_averages_to_the_shadowed_ratio() {
    let panel = emitter(vec![panel_4x4(1.0)]);
    let p = DVec3::new(-1.0, 0.0, 0.0);
    let sampler = AreaSampler::new(&panel, p, Some(UP)).expect("faced");
    let omega = sampler.total_solid_angle();
    let projected = panel.irradiance(p, UP).0;
    let samples: Vec<AreaSample> = lattice(64).map(|u| sampler.sample(u)).collect();
    let mean = |visible: &dyn Fn(&AreaSample) -> bool| {
        samples
            .iter()
            .map(|s| one_sample_visibility(s.weight, visible(s), omega, projected))
            .sum::<f64>()
            / samples.len() as f64
    };
    assert_abs(mean(&|_| true), 1.0, 1e-2, "open");
    let values: Vec<f64> = samples
        .iter()
        .map(|s| one_sample_visibility(s.weight, true, omega, projected))
        .collect();
    assert!(values.iter().any(|&v| v > 0.0 && (v - 1.0).abs() > 0.05));
    let near_half = polygon_irradiance(&ceiling_rect(-2.0, 0.0, -2.0, 2.0, 1.0), p, UP).0;
    assert_abs(
        mean(&|s| s.target.x < 0.0),
        near_half / projected,
        1e-2,
        "far half occluded",
    );
    assert_eq!(one_sample_visibility(0.7, true, omega, 0.0), 0.0);
    assert_eq!(one_sample_visibility(0.7, false, omega, projected), 0.0);
}
