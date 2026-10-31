// Tests for SH ray generation and bounded BVH traversal against full scans.
// See: context/lib/build_pipeline.md §Compiler pipeline

use bvh::bvh::Bvh;
use glam::Vec3;

use super::test_fixtures::*;
use super::trace::*;
use super::*;
use crate::bvh_build::{BvhPrimitive, build_bvh, collect_primitives};
use crate::geometry::GeometryResult;

#[test]
fn sphere_directions_are_unit_and_stable() {
    let dirs = sphere_directions(32, 1);
    assert_eq!(dirs.len(), 32);
    for d in &dirs {
        let length = d.length();
        assert!(
            (length - 1.0).abs() < 1.0e-4,
            "direction not unit length: {}",
            length
        );
    }
    let again = sphere_directions(32, 1);
    assert_eq!(dirs, again);
}

#[test]
fn sphere_directions_cover_sphere_integral() {
    let dirs = sphere_directions(RAYS_PER_PROBE, SAMPLING_LATTICE_OFFSET);
    let weight = (4.0 * std::f32::consts::PI) / RAYS_PER_PROBE as f32;
    let integral: f32 = dirs.iter().map(|_| 1.0 * weight).sum();
    let expected = 4.0 * std::f32::consts::PI;
    assert!(
        (integral - expected).abs() < 1.0e-3,
        "integral of 1 over sphere: expected {expected}, got {integral}"
    );
}

#[test]
fn build_bvh_traversal_interop() {
    let geo = one_triangle_geometry([[-1.0, 0.0, -1.0], [2.0, 0.0, -1.0], [0.0, 0.0, 2.0]]);
    let (bvh, _prims, _) = build_bvh(&geo).unwrap();
    let prims = collect_primitives(&geo);
    let ray = bvh::ray::Ray::new(
        nalgebra::Point3::new(0.0, 5.0, 0.0),
        nalgebra::Vector3::new(0.0, -1.0, 0.0),
    );
    let hits = bvh.traverse(&ray, &prims);
    assert!(!hits.is_empty(), "ray should hit the triangle face AABB");
}

/// Two triangles creased along the shared edge x = 0, y = 0 (z in -1..1),
/// opening upward (left face y = -x, right face y = x), in one face (one
/// BVH leaf) or as two faces (two leaves). A far floor, a far wall and
/// scattered tiles give the tree nodes for the bound to prune.
fn crease_geometry(shared_leaf: bool) -> GeometryResult {
    let left = [[0.0, 0.0, -1.0], [0.0, 0.0, 1.0], [-1.0, 1.0, 0.0]];
    let right = [[0.0, 0.0, -1.0], [0.0, 0.0, 1.0], [1.0, 1.0, 0.0]];
    let mut triangles = vec![
        left,
        right,
        [
            [-20.0, -6.0, -20.0],
            [20.0, -6.0, -20.0],
            [20.0, -6.0, 20.0],
        ],
        [
            [-20.0, -6.0, -20.0],
            [20.0, -6.0, 20.0],
            [-20.0, -6.0, 20.0],
        ],
        [[8.0, -6.0, -20.0], [8.0, 10.0, -20.0], [8.0, 10.0, 20.0]],
    ];
    for k in 0..12 {
        let x = -7.0 + k as f32 * 1.25;
        triangles.push([[x, -3.0, -4.0], [x + 0.5, -3.0, -4.0], [x, -2.5, -3.5]]);
    }
    let mut geometry = multi_triangle_geometry(&triangles);
    if shared_leaf {
        geometry.face_index_ranges[0].index_count = 6;
        geometry.face_index_ranges.remove(1);
        geometry.geometry.faces.remove(1);
    }
    geometry
}

fn hit_parts(hit: Option<Hit>) -> Option<(f32, Vec3, Vec3)> {
    hit.map(|h| (h.distance, h.normal, h.point))
}

/// Rays aimed at points on the crease edge from several origins.
fn crease_edge_rays() -> Vec<(Vec3, Vec3)> {
    let origins = [
        Vec3::new(0.0, 5.0, 0.0),
        Vec3::new(3.0, 4.0, 1.0),
        Vec3::new(-2.0, 3.0, -1.0),
        Vec3::new(0.25, 6.0, -0.5),
    ];
    let mut rays = Vec::new();
    for origin in origins {
        for z in [-0.75, -0.3, 0.0, 0.4, 0.9] {
            rays.push((origin, (Vec3::new(0.0, 0.0, z) - origin).normalize()));
        }
    }
    rays
}

#[test]
fn bounded_closest_hit_matches_full_scan_on_shared_crease_edges() {
    for shared_leaf in [true, false] {
        let geo = crease_geometry(shared_leaf);
        let (bvh, prims, _) = build_bvh(&geo).unwrap();
        let ctx = RaytracingCtx {
            bounce_materials: None,
            bvh: &bvh,
            primitives: &prims,
            geometry: &geo,
        };
        let mut edge_hits = 0;
        for (origin, dir) in crease_edge_rays() {
            let full = hit_parts(closest_hit_full_scan(&ctx, origin, dir, f32::INFINITY));
            let bounded = hit_parts(closest_hit(&ctx, origin, dir, f32::INFINITY));
            assert_eq!(
                bounded, full,
                "shared_leaf {shared_leaf}, ray {origin} {dir}"
            );
            let Some((distance, _, point)) = full else {
                panic!("ray {origin} {dir} must hit the crease");
            };
            if point.x.abs() < 1e-4 && point.y.abs() < 1e-4 {
                edge_hits += 1;
            }
            // Finite bounds at, just under and just over the hit distance.
            for max in [
                distance,
                distance.next_down(),
                distance.next_up(),
                distance * 0.5,
            ] {
                assert_eq!(
                    hit_parts(closest_hit(&ctx, origin, dir, max)),
                    hit_parts(closest_hit_full_scan(&ctx, origin, dir, max)),
                    "shared_leaf {shared_leaf}, ray {origin} {dir}, max {max}"
                );
            }
        }
        // A ray exactly on the shared edge can slip between the two
        // triangles and hit something farther; most must land on the edge so
        // the tie is exercised.
        assert!(
            edge_hits * 2 > crease_edge_rays().len(),
            "{edge_hits} rays landed on the edge"
        );
    }
}

#[test]
fn bounded_segment_clear_matches_full_scan_at_segment_end() {
    let geo = crease_geometry(false);
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &prims,
        geometry: &geo,
    };
    let (mut clear, mut blocked) = (0, 0);
    for x in [-0.6f32, -0.2, 0.0, 0.2, 0.55] {
        let from = Vec3::new(x, 5.0, 0.3);
        // The crease surface under `from` sits at y = |x|.
        let surface = x.abs();
        for end_y in [
            surface + 0.01,
            surface + RAY_EPSILON,
            surface.next_up(),
            surface,
            surface.next_down(),
            surface - RAY_EPSILON,
            surface - 0.01,
            -1.0,
            -6.0,
            -7.0,
        ] {
            let to = Vec3::new(x, end_y, 0.3);
            let full = segment_clear_full_scan(&ctx, from, to);
            assert_eq!(segment_clear(&ctx, from, to), full, "sh: {from} -> {to}");
            assert_eq!(
                crate::lightmap_bake::segment_clear(&bvh, &prims, &geo, from, to),
                crate::lightmap_bake::segment_clear_full_scan(&bvh, &prims, &geo, from, to),
                "lightmap: {from} -> {to}"
            );
            if full {
                clear += 1;
            } else {
                blocked += 1;
            }
        }
    }
    assert!(clear > 0 && blocked > 0, "{clear} clear, {blocked} blocked");
}

#[test]
fn bounded_traversal_matches_stock_rejection_of_undefined_slab() {
    // Rays lying in a box face plane, parallel to it: the floor's flat box
    // (y = -6), the wall's (x = 8), and the crease leaf's top face (y = 1).
    // The stock slab test sees NaN there and rejects the box; the bounded
    // query must answer as the unbounded reference scan does.
    let geo = crease_geometry(true);
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &prims,
        geometry: &geo,
    };
    let rays = [
        (Vec3::new(-30.0, -6.0, 0.0), Vec3::X),
        (Vec3::new(8.0, 2.0, -30.0), Vec3::Z),
        (Vec3::new(-5.0, 1.0, 0.0), Vec3::X),
        (Vec3::new(0.0, 1.0, -5.0), Vec3::Z),
    ];
    for (origin, dir) in rays {
        for max in [f32::INFINITY, 12.0] {
            assert_eq!(
                hit_parts(closest_hit(&ctx, origin, dir, max)),
                hit_parts(closest_hit_full_scan(&ctx, origin, dir, max)),
                "ray {origin} {dir}, max {max}"
            );
        }
        let to = origin + dir * 40.0;
        assert_eq!(
            segment_clear(&ctx, origin, to),
            segment_clear_full_scan(&ctx, origin, to),
            "segment {origin} -> {to}"
        );
    }
}

#[test]
fn sub_epsilon_start_hit_never_tightens_bound() {
    // Each ray starts on the triangle the traversal reaches first, so that
    // triangle reports a hit under RAY_EPSILON. Closest-hit ignores it; were
    // it to shrink the bound, the farther real hit would be pruned.
    let geo = crease_geometry(true);
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &prims,
        geometry: &geo,
    };
    let starts = [
        (Vec3::new(0.5, 0.5, 0.0), Vec3::new(-0.3, -1.0, 0.1)),
        (Vec3::new(-0.4, 0.4, 0.2), Vec3::new(0.2, -1.0, -0.1)),
        (Vec3::new(0.25, 0.25, -0.5), Vec3::new(0.0, -1.0, 0.0)),
    ];
    for (origin, toward) in starts {
        let dir = toward.normalize();
        let bounded = hit_parts(closest_hit(&ctx, origin, dir, f32::INFINITY));
        assert_eq!(
            bounded,
            hit_parts(closest_hit_full_scan(&ctx, origin, dir, f32::INFINITY)),
            "ray {origin} {dir}"
        );
        let (distance, _, _) = bounded.expect("the ray reaches the floor or a tile");
        assert!(
            distance > 1.0,
            "kept hit {distance} is past the start triangle"
        );
    }
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

/// Cut offsets across a sloped face. Each 3 cm strip is a sliver: split on
/// its diagonal over the 30 m strip length, each triangle is ~1:1000.
const STRIP_CUTS: [f32; 15] = [
    -20.0, -16.0, -15.97, -12.0, -8.0, -7.97, -4.0, 0.0, 0.03, 4.0, 8.0, 8.03, 12.0, 16.0, 20.0,
];

/// Cuts with a wide strip on both sides.
const WIDE_STRIP_CUTS: [f32; 5] = [-12.0, -4.0, 4.0, 12.0, 16.0];

/// A sloped face `y = centre.y + a (x - centre.x) + b (z - centre.z)`, cut
/// into strips along `x = const` planes (`cut_x`) or `z = const` planes.
/// Strips span 30 m along the cuts.
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

    /// The face point `cut` across the cuts and `along` them, both offsets
    /// from the centre. Shared edge vertices come out bit-identical.
    fn at(&self, cut: f32, along: f32) -> Vec3 {
        if self.cut_x {
            self.point(self.centre.x + cut, self.centre.z + along)
        } else {
            self.point(self.centre.x + along, self.centre.z + cut)
        }
    }

    fn strip_triangles(&self) -> Vec<[[f32; 3]; 3]> {
        let mut triangles = Vec::new();
        for pair in STRIP_CUTS.windows(2) {
            let (c0, c1) = (pair[0], pair[1]);
            let p00 = self.at(c0, -15.0).to_array();
            let p10 = self.at(c1, -15.0).to_array();
            let p11 = self.at(c1, 15.0).to_array();
            let p01 = self.at(c0, 15.0).to_array();
            triangles.push([p00, p10, p11]);
            triangles.push([p00, p11, p01]);
        }
        triangles
    }
}

/// The lightmap occlusion answer with no box test: each triangle in turn is
/// handed to the occluder cache, which tests it before any traversal.
fn lightmap_clear_by_every_triangle(
    bvh: &Bvh<f32, 3>,
    prims: &[BvhPrimitive],
    geo: &GeometryResult,
    from: Vec3,
    to: Vec3,
) -> bool {
    (0..geo.geometry.indices.len()).step_by(3).all(|tri| {
        crate::lightmap_bake::segment_clear_remembering(
            bvh,
            prims,
            geo,
            from,
            to,
            &std::cell::Cell::new(Some(tri)),
        )
    })
}

/// A flat triangle's box is flat, and the stock box test can reject it, by
/// rounding, for a ray the triangle test hits at the triangle's edge. The
/// traversal then reports the segment clear; the occluder cache, holding
/// that triangle, reports it blocked. The ray is steep: the rounding is in
/// the slab bounds, not the angle.
#[test]
fn lightmap_segment_clear_occluder_cache_catches_flat_box_rounding_leak() {
    let geo = multi_triangle_geometry(&[[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]]]);
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    // Crosses z = 0 at the triangle's x = 1 edge.
    let from = Vec3::from_array([1068354961, 1063499892, 1065798143].map(f32::from_bits));
    let to = Vec3::from_array([1059964654, 1061407677, 3211917106].map(f32::from_bits));

    assert!(
        crate::lightmap_bake::segment_clear(&bvh, &prims, &geo, from, to),
        "the stock box test must reject this ray's box for the leak to exist"
    );
    let empty = std::cell::Cell::new(None);
    assert!(crate::lightmap_bake::segment_clear_remembering(
        &bvh, &prims, &geo, from, to, &empty
    ));
    assert_eq!(empty.get(), None, "a clear traversal records no occluder");
    let holding = std::cell::Cell::new(Some(0));
    assert!(!crate::lightmap_bake::segment_clear_remembering(
        &bvh, &prims, &geo, from, to, &holding
    ));
    assert_eq!(holding.get(), Some(0));
}

/// Randomized parity against the unbounded reference scans, on geometry
/// that stresses Möller–Trumbore rounding: vertices 100–500 m from the
/// origin, large rotated triangles, and two crossing sloped faces cut into
/// strips along axis-aligned planes, as BSP splits leave them, with 3 cm
/// slivers sharing the cut edges. Rays graze toward shared edges, the
/// faces' crossing line (two hits millimetres apart) and large triangles;
/// segments end just short of, at and just past the nearest hit.
///
/// The rays stay inside the prune slack's envelope. On a sliver, MT's `t`
/// error grows with ray length over graze, so rays toward the sloped faces
/// keep `length <= 100 m * graze`, and near misses past a strip's outer end
/// aim only at wide strips: near a sliver's ends the error reaches metres,
/// past any finite slack. That is the open case the fixture digest gate
/// watches.
#[test]
fn bounded_queries_match_full_scan_on_random_rounding_stress() {
    let mut rng = XorShift(0x2545_F491_4F6C_DD1D);
    let last_occluder = std::cell::Cell::new(None);
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
    let mut triangles: Vec<[[f32; 3]; 3]> =
        faces.iter().flat_map(SlopedFace::strip_triangles).collect();
    let large_start = triangles.len();
    let large_count = 48;
    for _ in 0..large_count {
        let centre_dir = rng.unit_vec();
        triangles.push([(); 3].map(|()| {
            let dir = (centre_dir + rng.unit_vec() * 0.35).normalize();
            (dir * rng.range(100.0, 500.0)).to_array()
        }));
    }
    let geo = multi_triangle_geometry(&triangles);
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &prims,
        geometry: &geo,
    };

    let (mut hits, mut misses, mut clear, mut blocked) = (0, 0, 0, 0);
    for i in 0..3_000 {
        let (origin, dir) = if i < 1_600 {
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
            let [p0, p1, p2] = triangles[large_start + rng.index(large_count)].map(Vec3::from);
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

        let full = hit_parts(closest_hit_full_scan(&ctx, origin, dir, f32::INFINITY));
        assert_eq!(
            hit_parts(closest_hit(&ctx, origin, dir, f32::INFINITY)),
            full,
            "ray {i}: {origin} {dir}"
        );
        let ends = if let Some((distance, _, _)) = full {
            hits += 1;
            for max in [distance, distance.next_up()] {
                assert_eq!(
                    hit_parts(closest_hit(&ctx, origin, dir, max)),
                    hit_parts(closest_hit_full_scan(&ctx, origin, dir, max)),
                    "ray {i}: {origin} {dir}, max {max}"
                );
            }
            [distance - 1e-3, distance, distance + 1e-3, distance + 2e-2]
        } else {
            misses += 1;
            [1.0, 50.0, 400.0, 1_000.0]
        };
        for end in ends {
            let to = origin + dir * end;
            let full_clear = segment_clear_full_scan(&ctx, origin, to);
            assert_eq!(
                segment_clear(&ctx, origin, to),
                full_clear,
                "sh, ray {i}: {origin} -> {to}"
            );
            let lightmap_full =
                crate::lightmap_bake::segment_clear_full_scan(&bvh, &prims, &geo, origin, to);
            assert_eq!(
                crate::lightmap_bake::segment_clear(&bvh, &prims, &geo, origin, to),
                lightmap_full,
                "lightmap, ray {i}: {origin} -> {to}"
            );
            // The occluder cache carries across rays, so later queries test
            // an unrelated triangle first. It may only add a blocked answer
            // the box test lost to rounding, so a disagreement must be a
            // real hit by some triangle.
            let remembered = crate::lightmap_bake::segment_clear_remembering(
                &bvh,
                &prims,
                &geo,
                origin,
                to,
                &last_occluder,
            );
            if remembered != lightmap_full {
                assert!(
                    lightmap_full
                        && !lightmap_clear_by_every_triangle(&bvh, &prims, &geo, origin, to),
                    "lightmap remembering, ray {i}: {origin} -> {to}"
                );
            }
            if full_clear {
                clear += 1;
            } else {
                blocked += 1;
            }
        }
    }
    assert!(
        hits > 2_000 && misses > 100 && clear > 3_000 && blocked > 3_000,
        "{hits} hits, {misses} misses, {clear} clear, {blocked} blocked"
    );
}
