// `front_faces_clear` against a brute-force scan on rounding-stress geometry.
// See: context/lib/build_pipeline.md §Ray traversal

use glam::Vec3;

use super::test_fixtures::SceneBuilder;
use super::*;
use crate::bvh_build::build_bvh;
use crate::ray_traversal::{TraceRay, triangle_points};

/// xorshift64: deterministic, no dependency.
struct XorShift(u64);

impl XorShift {
    fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    fn index(&mut self, len: usize) -> usize {
        ((self.unit() * len as f32) as usize).min(len - 1)
    }

    fn sign(&mut self) -> f32 {
        if self.unit() < 0.5 { -1.0 } else { 1.0 }
    }

    fn unit_vec(&mut self) -> Vec3 {
        loop {
            let v = Vec3::new(
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
            );
            let len = v.length();
            if len > 0.1 && len <= 1.0 {
                return v / len;
            }
        }
    }
}

/// Cut offsets across the sloped plane. Each 3 cm strip is a sliver, split on
/// its diagonal over the 30 m strip length.
const CUTS: [f32; 11] = [
    -20.0, -16.0, -15.97, -8.0, -4.0, 0.0, 0.03, 4.0, 8.0, 12.0, 20.0,
];

/// `front_faces_clear`, the reference: every triangle, the shipped hit test,
/// front faces only, over `(0, length)`.
fn front_faces_clear_brute_force(
    geometry: &crate::geometry::GeometryResult,
    from: Vec3,
    to: Vec3,
    admits: impl Fn([Vec3; 3]) -> bool,
) -> bool {
    let delta = to - from;
    let length = delta.length();
    if length <= 0.0 {
        return true;
    }
    let ray = TraceRay::new(from, delta / length);
    !(0..geometry.geometry.indices.len()).step_by(3).any(|tri| {
        let points = triangle_points(geometry, tri);
        admits(points) && ray.hit(points, 0.0, length).is_some_and(|hit| hit.front)
    })
}

/// A sloped plane cut into sliver strips with bit-identical shared edges, the
/// strips wound alternately so both sides face the segments, plus large
/// triangles. Segments aim at shared edges, past strip ends and at large
/// triangles, grazing, ending at the aim point or a hair either side of it. The
/// segment starts at `from`, not `RAY_EPSILON` along it, so the test also
/// pins the interval `(0, length)`.
#[test]
fn front_faces_clear_matches_brute_force_on_rounding_stress() {
    let mut rng = XorShift(0x2545_F491_4F6C_DD1D);
    let centre = Vec3::new(210.0, 190.0, -260.0);
    let (a, b) = (0.37_f32, -0.21_f32);
    let at = |cut: f32, along: f32| {
        let (x, z) = (centre.x + cut, centre.z + along);
        Vec3::new(x, centre.y + a * (x - centre.x) + b * (z - centre.z), z)
    };
    let normal = Vec3::new(-a, 1.0, -b).normalize();

    let mut scene = SceneBuilder::default();
    for (i, pair) in CUTS.windows(2).enumerate() {
        let p00 = at(pair[0], -15.0);
        let p10 = at(pair[1], -15.0);
        let p11 = at(pair[1], 15.0);
        let p01 = at(pair[0], 15.0);
        let mut corners = [p00, p10, p11, p01];
        if i % 2 == 1 {
            corners.reverse();
        }
        scene.polygon(&corners, normal);
    }
    let large_count = 32;
    let mut large = Vec::new();
    for _ in 0..large_count {
        let centre_dir = rng.unit_vec();
        let corners = [(); 3].map(|()| {
            let dir = (centre_dir + rng.unit_vec() * 0.35).normalize();
            dir * rng.range(100.0, 500.0)
        });
        scene.polygon(&corners, Vec3::Y);
        large.push(corners);
    }
    let geometry = scene.build();
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();

    let admit_all = |_: [Vec3; 3]| true;
    // An arbitrary filter, to pin that both sides apply `admits` alike.
    let admit_half = |p: [Vec3; 3]| (p[0].x + p[1].x + p[2].x).to_bits().is_multiple_of(2);

    let (mut clear, mut blocked) = (0, 0);
    for i in 0..3_000 {
        let (origin, dir, aim) = if i < 1_800 {
            let cut = CUTS[1 + rng.index(CUTS.len() - 2)]
                + [0.0, 0.0, 1e-5, -1e-5, 1e-3, -1e-3][rng.index(6)];
            let along = if rng.unit() < 0.25 {
                rng.sign() * (15.0 + rng.range(1e-4, 1e-1))
            } else {
                rng.range(-14.0, 14.0)
            };
            let target = at(cut, along);
            let theta = rng.range(0.0, std::f32::consts::TAU);
            let tangent = (Vec3::new(1.0, a, 0.0) * theta.cos()
                + Vec3::new(0.0, b, 1.0) * theta.sin())
            .normalize();
            let graze = 10f32.powf(rng.range(-2.0, -0.5));
            let dir = (tangent + normal * (graze * rng.sign())).normalize();
            let back = rng.range(0.5, 100.0 * graze);
            (target - dir * back, dir, back)
        } else {
            let [p0, p1, p2] = large[rng.index(large_count)];
            let (mut w1, mut w2) = (rng.unit(), rng.unit());
            if w1 + w2 > 1.0 {
                (w1, w2) = (1.0 - w1, 1.0 - w2);
            }
            let target = p0 + (p1 - p0) * w1 + (p2 - p0) * w2;
            let dir = rng.unit_vec();
            let back = rng.range(1.0, 200.0);
            (target - dir * back, dir, back)
        };
        let far = rng.range(0.0, 400.0);
        for end in [aim - 1e-3, aim, aim + 1e-3, aim + 2e-2, 1e-3, far] {
            let to = origin + dir * end;
            for (name, admits) in [
                ("admit all", &admit_all as &dyn Fn([Vec3; 3]) -> bool),
                ("admit half", &admit_half),
            ] {
                let expected = front_faces_clear_brute_force(&geometry, origin, to, admits);
                assert_eq!(
                    front_faces_clear(&bvh, &primitives, &geometry, origin, to, admits),
                    expected,
                    "{name}, ray {i}: {origin} -> {to}"
                );
                if name == "admit all" {
                    if expected {
                        clear += 1;
                    } else {
                        blocked += 1;
                    }
                }
            }
        }
    }
    assert!(
        clear > 100 && blocked > 100,
        "fixture exercises both outcomes: {clear} clear, {blocked} blocked"
    );
}
