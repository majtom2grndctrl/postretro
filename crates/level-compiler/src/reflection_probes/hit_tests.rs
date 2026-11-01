// Reflection-probe closest hit: front-face hits, the face mask, brute-force parity and tolerance-hit attributes.
// See: context/lib/build_pipeline.md §Ray traversal · resource_management.md §5

use glam::Vec3;
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

use super::*;
use crate::bvh_build::build_bvh;
use crate::geometry::FaceIndexRange;
use crate::ray_traversal::TRACER_TOLERANCE_METERS;
use crate::reflection_probes::reflection_probes_test_fixtures::{RoomFace, box_room};

#[test]
fn closest_hit_returns_the_floor_face_uv_and_barycentrics_below_a_point() {
    let room = box_room([-2.0, 0.0, -2.0], [2.0, 3.0, 2.0]);
    let (bvh, primitives, _) = build_bvh(&room).unwrap();
    let tracer = SceneTracer::new(&bvh, &primitives, &room);

    let origin = Vec3::new(0.5, 1.0, -0.25);
    let down = Vec3::NEG_Y;
    let hit = tracer
        .closest_hit(origin, down, 100.0)
        .expect("the floor is below");
    assert_eq!(hit.face, RoomFace::Floor as u32);
    assert!((hit.distance - 1.0).abs() < 1e-5, "{}", hit.distance);
    let weights: f32 = hit.barycentric.iter().sum();
    assert!((weights - 1.0).abs() < 1e-5);
    assert!(hit.barycentric.iter().all(|w| (0.0..=1.0).contains(w)));

    let surface = tracer.surface(&hit, origin, down);
    assert!((surface.position - Vec3::new(0.5, 0.0, -0.25)).length() < 1e-5);
    assert!((surface.normal - Vec3::Y).length() < 1e-3);
    assert_eq!(surface.texture_index, RoomFace::Floor as u32);
    // The fixture maps 1 m to 32 texels from the room's min corner.
    assert!(
        (surface.uv[0] - 2.5 * 32.0).abs() < 1e-3,
        "{:?}",
        surface.uv
    );
    assert!(
        (surface.uv[1] - 1.75 * 32.0).abs() < 1e-3,
        "{:?}",
        surface.uv
    );
    assert!((surface.texel_density - 32.0).abs() < 1e-2);
}

// A masked face is skipped and reported; a min distance accepts a face
// just past it, so a re-trace from a hull exit never steps over one.
#[test]
fn closest_hit_masked_skips_masked_faces_and_reports_the_nearest_one() {
    let room = box_room([-2.0, 0.0, -2.0], [2.0, 3.0, 2.0]);
    let (bvh, primitives, _) = build_bvh(&room).unwrap();
    let tracer = SceneTracer::new(&bvh, &primitives, &room);
    let floor = RoomFace::Floor as u32;
    let mask = tracer.face_mask([floor]);
    let none = FaceMask::default();
    let origin = Vec3::new(0.5, 1.0, -0.25);

    let down = tracer.closest_hit_masked(origin, Vec3::NEG_Y, RAY_EPSILON, 100.0, &mask, &none);
    assert_eq!(down.hit, None, "nothing faces the ray past the floor");
    assert_eq!(down.skipped, tracer.closest_hit(origin, Vec3::NEG_Y, 100.0));
    // A masked face behind the kept hit is hidden, not reported.
    let up = tracer.closest_hit_masked(origin, Vec3::Y, RAY_EPSILON, 100.0, &mask, &none);
    assert_eq!(up.hit.map(|h| h.face), Some(RoomFace::Ceiling as u32));
    assert_eq!(up.skipped, None);
    // An ignored face is neither kept nor reported.
    let ignored = tracer.closest_hit_masked(origin, Vec3::NEG_Y, RAY_EPSILON, 100.0, &mask, &mask);
    assert_eq!((ignored.hit, ignored.skipped), (None, None));

    let past = |min| {
        tracer
            .closest_hit_masked(origin, Vec3::NEG_Y, min, 100.0, &none, &none)
            .hit
    };
    assert_eq!(
        past(0.9995).map(|h| h.face),
        Some(floor),
        "half a millimetre short"
    );
    assert_eq!(past(1.0005), None);
}

#[test]
fn closest_hit_culls_back_faces_as_the_world_pass_does() {
    let room = box_room([-2.0, 0.0, -2.0], [2.0, 3.0, 2.0]);
    let (bvh, primitives, _) = build_bvh(&room).unwrap();
    let tracer = SceneTracer::new(&bvh, &primitives, &room);

    // From below the floor, looking up: the floor shows its back to the
    // ray, so the ray passes it and lands on the ceiling's front.
    let hit = tracer
        .closest_hit(Vec3::new(0.0, -1.0, 0.0), Vec3::Y, 100.0)
        .expect("the ceiling faces the ray");
    assert_eq!(hit.face, RoomFace::Ceiling as u32);
    assert!((hit.distance - 4.0).abs() < 1e-5, "{}", hit.distance);
    // And from above the ceiling, looking down, the floor.
    let hit = tracer
        .closest_hit(Vec3::new(0.0, 4.0, 0.0), Vec3::NEG_Y, 100.0)
        .expect("the floor faces the ray");
    assert_eq!(hit.face, RoomFace::Floor as u32);
    assert!((hit.distance - 4.0).abs() < 1e-5, "{}", hit.distance);
}

/// UV units per metre of x on [`triangle_faces`] geometry.
const UV_PER_METER: f32 = 100.0;

/// Geometry with one face per triangle, each wound as given. A corner's UV
/// is `(x, z)` at [`UV_PER_METER`] and its lightmap UV is `(x, 0)` clamped
/// to `[0, 1]`, so an interpolated attribute names the point it was read at.
fn triangle_faces(triangles: &[[Vec3; 3]]) -> GeometryResult {
    let mut vertices = Vec::with_capacity(triangles.len() * 3);
    let mut faces = Vec::with_capacity(triangles.len());
    let mut face_index_ranges = Vec::with_capacity(triangles.len());
    for (i, &[a, b, c]) in triangles.iter().enumerate() {
        let normal = (b - a).cross(c - a).normalize_or(Vec3::Y);
        vertices.extend([a, b, c].map(|p| {
            Vertex::new(
                p.to_array(),
                [p.x * UV_PER_METER, p.z * UV_PER_METER],
                normal.to_array(),
                [1.0, 0.0, 0.0],
                true,
                [p.x.clamp(0.0, 1.0), 0.0],
                1,
            )
        }));
        faces.push(FaceMeta {
            leaf_index: 0,
            texture_index: 0,
        });
        face_index_ranges.push(FaceIndexRange {
            index_offset: (i * 3) as u32,
            index_count: 3,
        });
    }
    GeometryResult {
        geometry: GeometrySection {
            vertices,
            indices: (0..triangles.len() as u32 * 3).collect(),
            faces,
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges,
    }
}

// A tolerance hit on a sliver has corner weights far outside [0, 1]: here
// about [0.5, 15.25, −14.75]. Zeroing the negative one and renormalizing
// named a point 0.47 m along the sliver; the weights now name the point
// nearest the crossing, so every interpolated attribute comes from within τ
// of where the ray crossed the plane.
#[test]
fn a_tolerance_hit_on_a_sliver_interpolates_at_the_nearest_point() {
    let sliver = [
        Vec3::ZERO,
        Vec3::new(1.0, 0.0, 1.0e-5),
        Vec3::new(1.0, 0.0, -1.0e-5),
    ];
    let geometry = triangle_faces(&[sliver]);
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let tracer = SceneTracer::new(&bvh, &primitives, &geometry);
    let origin = Vec3::new(0.5, 0.999, 3.0e-4);
    let crossing = Vec3::new(0.5, 0.0, 3.0e-4);

    let hit = tracer
        .closest_hit(origin, Vec3::NEG_Y, 10.0)
        .expect("the crossing lies 0.3 mm off the sliver, inside τ");
    assert!(
        hit.barycentric.iter().all(|w| (0.0..=1.0).contains(w)),
        "{:?}",
        hit.barycentric
    );
    let sum: f32 = hit.barycentric.iter().sum();
    assert!((sum - 1.0).abs() < 1e-5, "{:?}", hit.barycentric);

    let reach = TRACER_TOLERANCE_METERS + 1.0e-6;
    let on = tracer.position_on(&hit);
    assert!(
        on.distance(crossing) <= reach,
        "{on} is {} from the crossing",
        on.distance(crossing)
    );
    let surface = tracer.surface(&hit, origin, Vec3::NEG_Y);
    assert!(surface.position.distance(crossing) <= reach, "{surface:?}");
    // UV and lightmap UV read x = 0.5, not the clamp's 0.97.
    assert!(
        (surface.uv[0] - 0.5 * UV_PER_METER).abs() <= reach * UV_PER_METER,
        "{:?}",
        surface.uv
    );
    assert!(
        (surface.lightmap_uv[0] - 0.5).abs() <= reach + 1.0 / 65535.0,
        "{:?}",
        surface.lightmap_uv
    );
}

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

    /// Log-uniform in `[10^lo, 10^hi]`.
    fn log_range(&mut self, lo: f32, hi: f32) -> f32 {
        10f32.powf(self.range(lo, hi))
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

/// The SH bake's rounding-stress cuts (`sh_bake/trace_tests.rs`): each 3 cm
/// strip, split on its diagonal over 30 m, is a pair of ~1:1000 slivers.
const STRIP_CUTS: [f32; 15] = [
    -20.0, -16.0, -15.97, -12.0, -8.0, -7.97, -4.0, 0.0, 0.03, 4.0, 8.0, 8.03, 12.0, 16.0, 20.0,
];

/// Cuts with a wide strip on both sides.
const WIDE_STRIP_CUTS: [f32; 5] = [-12.0, -4.0, 4.0, 12.0, 16.0];

/// A sloped face `y = centre.y + a (x − centre.x) + b (z − centre.z)`, cut
/// into strips along `x = const` planes (`cut_x`) or `z = const` planes.
#[derive(Clone, Copy)]
struct SlopedFace {
    centre: Vec3,
    a: f32,
    b: f32,
    cut_x: bool,
}

impl SlopedFace {
    fn point(&self, x: f32, z: f32) -> Vec3 {
        let y = self.centre.y + self.a * (x - self.centre.x) + self.b * (z - self.centre.z);
        Vec3::new(x, y, z)
    }

    fn normal(&self) -> Vec3 {
        Vec3::new(-self.a, 1.0, -self.b).normalize()
    }

    /// The face point `cut` across the cuts and `along` them; shared edge
    /// vertices come out bit-identical.
    fn at(&self, cut: f32, along: f32) -> Vec3 {
        if self.cut_x {
            self.point(self.centre.x + cut, self.centre.z + along)
        } else {
            self.point(self.centre.x + along, self.centre.z + cut)
        }
    }

    fn strip_triangles(&self) -> Vec<[Vec3; 3]> {
        STRIP_CUTS
            .windows(2)
            .flat_map(|pair| {
                let (c0, c1) = (pair[0], pair[1]);
                let p00 = self.at(c0, -15.0);
                let p10 = self.at(c1, -15.0);
                let p11 = self.at(c1, 15.0);
                let p01 = self.at(c0, 15.0);
                [[p00, p10, p11], [p00, p11, p01]]
            })
            .collect()
    }
}

/// Every front-facing hit `nearest` could keep, by brute force over every
/// triangle: the shipped hit test, no tree and no box test, its `f32`
/// distance strictly inside `(min, max)`. `(distance, face, masked)`.
fn front_hits(
    geometry: &GeometryResult,
    masked: impl Fn(u32) -> bool,
    (origin, direction): (Vec3, Vec3),
    (min, max): (f32, f32),
) -> Vec<(f32, u32, bool)> {
    let ray = TraceRay::new(origin, direction);
    let mut hits = Vec::new();
    for (face, range) in geometry.face_index_ranges.iter().enumerate() {
        let start = range.index_offset as usize;
        let end = start + range.index_count as usize;
        for tri in (start..end).step_by(3) {
            let points = triangle_points(geometry, tri);
            if let Some(hit) = ray.hit(points, min, max)
                && hit.front
            {
                let t = hit.t as f32;
                if t > min && t < max {
                    hits.push((t, face as u32, masked(face as u32)));
                }
            }
        }
    }
    hits
}

/// The first of `hits` in hit order: the nearest, and at one distance the
/// lowest face, which on [`triangle_faces`] geometry is the lowest triangle.
fn first_of(hits: impl Iterator<Item = (f32, u32)>) -> Option<(f32, u32)> {
    hits.min_by(|a, b| a.partial_cmp(b).expect("hit distances are never NaN"))
}

/// Whether `hit` is the brute-force `expected`: the same distance and face.
fn matches(hit: Option<SurfaceHit>, expected: Option<(f32, u32)>) -> bool {
    hit.map(|hit| (hit.distance, hit.face)) == expected
}

// AC 1: the probe tracer's own traversal (a min distance, a mask, a
// shrinking bound) gives the brute-force answer on the SH bake's
// rounding-stress fixture: shared sliver edges, two sloped faces' crossing
// line and large triangles far from the origin. The first kept front face
// in hit order matches in distance and face (a tie names the lower
// triangle), and so does the first masked one before it, from the origin's
// epsilon, up to a hit's own distance, and from it as a re-trace does. The
// rays stay inside the prune slack's envelope, as that fixture's do.
#[test]
fn nearest_matches_brute_force_on_random_rounding_stress() {
    // The SH test's seed: the same triangles and rays.
    let mut rng = XorShift(0x2545_F491_4F6C_DD1D);
    let centre = Vec3::new(210.0, 190.0, -260.0);
    let faces = [
        SlopedFace {
            centre,
            a: 0.37,
            b: -0.21,
            cut_x: true,
        },
        SlopedFace {
            centre,
            a: 0.33,
            b: -0.18,
            cut_x: false,
        },
    ];
    let mut triangles: Vec<[Vec3; 3]> =
        faces.iter().flat_map(SlopedFace::strip_triangles).collect();
    let large_start = triangles.len();
    let large_count = 48;
    for _ in 0..large_count {
        let centre_dir = rng.unit_vec();
        triangles.push([(); 3].map(|()| {
            let dir = (centre_dir + rng.unit_vec() * 0.35).normalize();
            dir * rng.range(100.0, 500.0)
        }));
    }
    let geometry = triangle_faces(&triangles);
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let tracer = SceneTracer::new(&bvh, &primitives, &geometry);
    // Every third face masked, so masked and kept faces share edges.
    let is_masked = |face: u32| face.is_multiple_of(3);
    let mask = tracer.face_mask((0..triangles.len() as u32).filter(|&f| is_masked(f)));
    let none = FaceMask::default();

    let (mut hits, mut skips) = (0, 0);
    for i in 0..3_000 {
        let (origin, direction) = if i < 1_600 {
            // Near-grazing toward a shared cut edge, or just past a wide
            // strip's outer end.
            let face = faces[rng.index(2)];
            let jitter = [0.0, 0.0, 1e-5, -1e-5, 1e-3, -1e-3][rng.index(6)];
            let (cut, along) = if rng.unit() < 0.25 {
                let cut = WIDE_STRIP_CUTS[rng.index(WIDE_STRIP_CUTS.len())];
                (cut, rng.sign() * (15.0 + rng.log_range(-4.0, -1.0)))
            } else {
                let cut = STRIP_CUTS[1 + rng.index(STRIP_CUTS.len() - 2)];
                (cut, rng.range(-14.0, 14.0))
            };
            let target = face.at(cut + jitter, along);
            let theta = rng.range(0.0, std::f32::consts::TAU);
            let tangent = (Vec3::new(1.0, face.a, 0.0) * theta.cos()
                + Vec3::new(0.0, face.b, 1.0) * theta.sin())
            .normalize();
            let graze = rng.log_range(-2.0, -0.5);
            let dir = (tangent + face.normal() * (graze * rng.sign())).normalize();
            (target - dir * rng.range(0.5, 100.0 * graze), dir)
        } else if i < 2_300 {
            // Toward the faces' crossing line, where 0.04 dx = 0.03 dz.
            let k = rng.range(-11.0, 11.0);
            let target = faces[0].point(centre.x + k, centre.z + k * 4.0 / 3.0);
            let dir = loop {
                let dir = rng.unit_vec();
                if faces.iter().all(|f| dir.dot(f.normal()).abs() >= 0.1) {
                    break dir;
                }
            };
            (target - dir * rng.range(0.5, 10.0), dir)
        } else {
            // Toward a large triangle's interior or edge, grazing to 1e-3.
            let [p0, p1, p2] = triangles[large_start + rng.index(large_count)];
            let (mut w1, mut w2) = (rng.unit(), rng.unit());
            if w1 + w2 > 1.0 {
                (w1, w2) = (1.0 - w1, 1.0 - w2);
            }
            if rng.unit() < 0.3 {
                w2 = 1.0 - w1;
            }
            let target = p0 + (p1 - p0) * w1 + (p2 - p0) * w2;
            let normal = (p1 - p0).cross(p2 - p0).normalize();
            let (e0, e1) = normal.any_orthonormal_pair();
            let theta = rng.range(0.0, std::f32::consts::TAU);
            let tangent = e0 * theta.cos() + e1 * theta.sin();
            let graze = rng.log_range(-3.0, 0.0);
            let dir = (tangent + normal * (graze * rng.sign())).normalize();
            (target - dir * rng.range(1.0, 200.0), dir)
        };
        let ray = (origin, direction);

        // Unmasked: the plain closest hit.
        let all = front_hits(&geometry, |_| false, ray, (RAY_EPSILON, f32::INFINITY));
        let expected = first_of(all.iter().map(|&(t, face, _)| (t, face)));
        assert!(
            matches(
                tracer.closest_hit(origin, direction, f32::INFINITY),
                expected
            ),
            "ray {i}: {origin} {direction}: {expected:?}"
        );
        let mut bounds = vec![(RAY_EPSILON, f32::INFINITY)];
        if let Some((distance, _)) = expected {
            hits += 1;
            bounds.extend([
                (RAY_EPSILON, distance),
                (RAY_EPSILON, distance.next_up()),
                (distance, f32::INFINITY),
                (distance.next_down(), f32::INFINITY),
            ]);
        }
        for (min, max) in bounds {
            let found = front_hits(&geometry, is_masked, ray, (min, max));
            let kept = first_of(
                found
                    .iter()
                    .filter(|&&(.., masked)| !masked)
                    .map(|&(t, face, _)| (t, face)),
            );
            let before_kept = |hit: (f32, u32)| kept.is_none_or(|first| hit < first);
            let skipped = first_of(
                found
                    .iter()
                    .filter(|&&(t, face, masked)| masked && before_kept((t, face)))
                    .map(|&(t, face, _)| (t, face)),
            );
            let traced = tracer.closest_hit_masked(origin, direction, min, max, &mask, &none);
            assert!(
                matches(traced.hit, kept),
                "ray {i}: {origin} {direction} in ({min}, {max}): {:?} vs {kept:?}",
                traced.hit
            );
            assert!(
                matches(traced.skipped, skipped),
                "ray {i}: {origin} {direction} in ({min}, {max}): {:?} vs {skipped:?}",
                traced.skipped
            );
            skips += usize::from(skipped.is_some());
            let unmasked = tracer.closest_hit_masked(origin, direction, min, max, &none, &none);
            let any = first_of(found.iter().map(|&(t, face, _)| (t, face)));
            assert!(
                matches(unmasked.hit, any) && unmasked.skipped.is_none(),
                "ray {i}: {origin} {direction} in ({min}, {max}): {unmasked:?} vs {any:?}"
            );
        }
    }
    // The draw reaches kept and masked hits alike.
    assert!(hits > 100 && skips > 50, "{hits} hits, {skips} skips");
}

/// Two triangles facing up on y = 0 that share their hypotenuse x + z = 1:
/// the one holding the origin corner, and the one holding (1, 0, 1).
fn seam_triangles() -> [[Vec3; 3]; 2] {
    let (x, z) = (Vec3::X, Vec3::Z);
    [[Vec3::ZERO, z, x], [x, z, x + z]]
}

// Regression: hits at one `f32` distance went to whichever triangle the
// traversal reached first, so a masked trace dropped a skipped face that an
// unmasked one kept. A ray crossing 0.14 mm inside one triangle of a seam
// takes a real hit on it and a tolerance hit on its neighbour, at the same
// distance. In either index order the lower triangle comes first, kept or
// skipped, and once ignored it yields to the other.
#[test]
fn hits_at_one_distance_go_to_the_lower_triangle_kept_skipped_or_ignored() {
    // 0.1 mm off the seam along x and z, inside the origin's triangle.
    let crossing = Vec3::new(0.5 - 1.0e-4, 0.0, 0.5 - 1.0e-4);
    let origin = crossing + Vec3::Y;
    let [near, far] = seam_triangles();
    for triangles in [[near, far], [far, near]] {
        let geometry = triangle_faces(&triangles);
        let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
        let tracer = SceneTracer::new(&bvh, &primitives, &geometry);
        let none = FaceMask::default();
        let trace = |mask: &FaceMask, ignored: &FaceMask| {
            tracer.closest_hit_masked(origin, Vec3::NEG_Y, RAY_EPSILON, 10.0, mask, ignored)
        };

        let first = tracer
            .closest_hit(origin, Vec3::NEG_Y, 10.0)
            .expect("the seam faces the ray");
        assert_eq!(first.face, 0, "{triangles:?}");
        assert!((first.distance - 1.0).abs() < 1e-6, "{first:?}");

        let lower = tracer.face_mask([0]);
        let skipping_lower = trace(&lower, &none);
        assert_eq!(skipping_lower.skipped, Some(first), "{triangles:?}");
        let second = skipping_lower.hit.expect("the upper triangle is kept");
        // The tie itself, exactly: both hits at one `f32` distance.
        assert_eq!(
            (second.face, second.distance),
            (1, first.distance),
            "{triangles:?}"
        );

        let skipping_upper = trace(&tracer.face_mask([1]), &none);
        assert_eq!(skipping_upper.hit, Some(first), "{triangles:?}");
        assert_eq!(
            skipping_upper.skipped, None,
            "the kept triangle precedes it"
        );

        let both = tracer.face_mask([0, 1]);
        let skipping_both = trace(&both, &none);
        assert_eq!(
            (skipping_both.hit, skipping_both.skipped),
            (None, Some(first))
        );
        let ignoring_lower = trace(&both, &lower);
        assert_eq!(
            (ignoring_lower.hit, ignoring_lower.skipped),
            (None, Some(second)),
            "{triangles:?}"
        );
    }
}
