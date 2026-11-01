// Watertight hit test: shared edges and vertices, cracks, the tolerance bound, windings, front faces.
// See: context/lib/build_pipeline.md §Ray traversal

use glam::{DVec3, Vec3};
use proptest::prelude::*;

use super::test_fixtures::{MT_DET_FLOOR, Scene, XorShift, axis_frame, moller_trumbore};
use super::{
    RAY_EPSILON, TRACER_TOLERANCE_METERS, TraceRay, box_pad, segment_clear,
    segment_clear_brute_force, with_tolerance,
};

const TAU: f32 = TRACER_TOLERANCE_METERS;

/// Triangles sharing an edge or a vertex with bit-identical corners, the
/// point on it a ray aims at, and each triangle's geometric normal.
struct SharedCase {
    triangles: Vec<[Vec3; 3]>,
    target: Vec3,
    normals: Vec<DVec3>,
}

impl SharedCase {
    /// Each triangle's corners start at a random one, winding kept, so a
    /// shared edge sits in a different slot of each triangle's corner list,
    /// as face fans leave it.
    fn new(rng: &mut XorShift, triangles: Vec<[Vec3; 3]>, target: Vec3) -> Self {
        let triangles: Vec<[Vec3; 3]> = triangles
            .into_iter()
            .map(|mut tri| {
                tri.rotate_left(rng.index(3));
                tri
            })
            .collect();
        let normals = triangles
            .iter()
            .map(|[a, b, c]| (*b - *a).as_dvec3().cross((*c - *a).as_dvec3()).normalize())
            .collect();
        Self {
            triangles,
            target,
            normals,
        }
    }
}

/// A map coordinate: the origin, or 10–500 m out either way.
fn map_offset(rng: &mut XorShift) -> DVec3 {
    let mut coord = || {
        if rng.unit() < 0.25 {
            0.0
        } else {
            rng.sign() * rng.range(10.0, 500.0)
        }
    };
    DVec3::new(coord(), coord(), coord())
}

fn f32_point(p: DVec3) -> Vec3 {
    p.as_vec3()
}

/// Two triangles in different faces sharing an axis-aligned edge, creased
/// at a random dihedral (flat included), and a point on the edge's interior.
fn shared_edge(rng: &mut XorShift) -> SharedCase {
    let axis = rng.index(3);
    let (_, side_b, side_c) = axis_frame(axis);
    let start = f32_point(map_offset(rng));
    let length = rng.range(0.2, 20.0) as f32;
    let mut end = start;
    end[axis] += length;
    let mid = (start.as_dvec3() + end.as_dvec3()) * 0.5;
    let spread = rng.range(0.2, 5.0);
    let first = rng.range(0.0, std::f64::consts::TAU);
    let crease = rng.range(-2.5, 2.5) * f64::from(u8::from(rng.unit() < 0.75));
    let apex = |angle: f64| f32_point(mid + (side_b * angle.cos() + side_c * angle.sin()) * spread);
    let (a1, a2) = (apex(first), apex(first + std::f64::consts::PI + crease));
    let mut target = start;
    target[axis] = (f64::from(start[axis]) + rng.range(0.25, 0.75) * f64::from(length)) as f32;
    SharedCase::new(rng, vec![[start, end, a1], [end, start, a2]], target)
}

/// Four triangles in different faces around a shared vertex, flat (every
/// edge axis-aligned) or raised into a pyramid, aimed at the vertex.
fn shared_vertex(rng: &mut XorShift) -> SharedCase {
    let (along, side_b, side_c) = axis_frame(rng.index(3));
    let centre = map_offset(rng);
    let radius = rng.range(0.2, 5.0);
    let height = if rng.unit() < 0.5 {
        0.0
    } else {
        rng.range(-1.0, 1.0) * radius
    };
    let apex = f32_point(centre + along * height);
    let ring = [side_b, side_c, -side_b, -side_c].map(|dir| f32_point(centre + dir * radius));
    let triangles = (0..4).map(|k| [apex, ring[k], ring[(k + 1) % 4]]).collect();
    SharedCase::new(rng, triangles, apex)
}

/// A segment aimed at `case.target` from any direction that crosses every
/// triangle's plane the same way, so it passes from one side of the shared
/// surface to the other, from 3 mm to 10 km out; and whether every
/// triangle's front faces it.
///
/// Rounding the endpoints to f32 moves the crossing off the target by up to
/// half an ulp of the coordinate, 15 µm at 500 m: the ray crosses within
/// rounding of the edge or vertex, not exactly on it. An f32 ray cannot pass
/// exactly through a point at an arbitrary angle. The triangles cover that
/// neighbourhood together, so the crossing still lies on one of them, and
/// the watertight test must hit it at τ = 0.
fn crossing_segment(rng: &mut XorShift, case: &SharedCase) -> (Vec3, Vec3, bool) {
    loop {
        let dir = rng.unit_vec();
        let facing: Vec<f64> = case.normals.iter().map(|n| n.dot(dir)).collect();
        let same_side = facing.iter().all(|&f| f > 0.05) || facing.iter().all(|&f| f < -0.05);
        if !same_side {
            continue;
        }
        let distance = rng.log_range(-2.5, 4.0);
        let target = case.target.as_dvec3();
        let from = f32_point(target - dir * distance);
        let to = f32_point(target + dir * rng.range(0.05, 1.0));
        return (from, to, facing[0] < 0.0);
    }
}

/// Every entry point's verdict on the segment, at the current tolerance:
/// each reports a surface, the front-filtered ones only when every triangle
/// faces the ray.
fn assert_every_entry_point_hits(case: &SharedCase, from: Vec3, to: Vec3, front: bool, tau: f32) {
    let scene = Scene::new(&case.triangles);
    for (entry, hit) in scene.entry_points_hit(from, to) {
        if entry.starts_with("front:") && !front {
            continue;
        }
        assert!(
            hit,
            "{entry} at τ = {tau}: {from} -> {to} misses {:?} at {}",
            case.triangles, case.target
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_500))]

    #[test]
    fn every_entry_point_hits_a_ray_through_a_shared_edge_or_vertex(seed in any::<u64>()) {
        let mut rng = XorShift(seed | 1);
        let case = if rng.unit() < 0.6 { shared_edge(&mut rng) } else { shared_vertex(&mut rng) };
        let (from, to, front) = crossing_segment(&mut rng, &case);
        for tau in [0.0, TAU] {
            with_tolerance(tau, || assert_every_entry_point_hits(&case, from, to, front, tau));
        }
    }
}

/// The shared-edge property fails with Möller–Trumbore restored: over the
/// same cases, its segment test lets some rays through between the two
/// triangles while the watertight test at τ = 0 blocks every one.
#[test]
fn retired_hit_test_lets_rays_through_shared_edges() {
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    let (mut slipped, mut cases) = (0, 0);
    for _ in 0..20_000 {
        let case = shared_edge(&mut rng);
        let (from, to, _) = crossing_segment(&mut rng, &case);
        let length = (to - from).length();
        let dir = (to - from) / length;
        let origin = from + dir * RAY_EPSILON;
        let max_distance = length - RAY_EPSILON;
        let watertight = TraceRay::new(origin, dir);
        cases += 1;
        let blocked_now = with_tolerance(0.0, || {
            case.triangles
                .iter()
                .any(|&tri| watertight.hit(tri, 0.0, max_distance).is_some())
        });
        assert!(blocked_now, "watertight τ = 0 misses {from} -> {to}");
        let blocked_before = case.triangles.iter().any(|&tri| {
            moller_trumbore(origin, dir, tri, MT_DET_FLOOR)
                .is_some_and(|t| t > 0.0 && t < max_distance)
        });
        slipped += usize::from(!blocked_before);
    }
    assert!(
        slipped > 0,
        "Möller–Trumbore blocked all {cases} shared-edge rays"
    );
}

/// A ray straight down through `(x, z)` onto the y = 0 plane.
fn down_through(x: f32, z: f32) -> (Vec3, Vec3) {
    (Vec3::new(x, 1.0, z), Vec3::new(x, -1.0, z))
}

fn blocked(triangles: &[[Vec3; 3]], from: Vec3, to: Vec3) -> bool {
    let scene = Scene::new(triangles);
    let traced = !segment_clear(&scene.bvh, &scene.primitives, &scene.geometry, from, to);
    let reference = !segment_clear_brute_force(&scene.geometry, from, to);
    assert_eq!(
        traced, reference,
        "BVH and brute force disagree on {from} -> {to}"
    );
    traced
}

/// A crease crack: the floor (y = 0) stops `gap` short of the wall plane
/// x = 0, whose face starts `gap` above the floor plane. A ray into the
/// corner crosses each plane `gap` from that face.
fn crease_crack(gap: f32) -> Vec<[Vec3; 3]> {
    vec![
        [
            Vec3::new(-gap, 0.0, -1.0),
            Vec3::new(-2.0, 0.0, -1.0),
            Vec3::new(-gap, 0.0, 1.0),
        ],
        [
            Vec3::new(0.0, gap, -1.0),
            Vec3::new(0.0, gap, 1.0),
            Vec3::new(0.0, 2.0, -1.0),
        ],
    ]
}

fn into_the_corner() -> (Vec3, Vec3) {
    (Vec3::new(-1.0, 1.0, 0.1), Vec3::new(0.5, -0.5, 0.1))
}

#[test]
fn narrow_crease_crack_blocks_and_fails_at_zero_tolerance() {
    let triangles = crease_crack(0.4 * TAU);
    let (from, to) = into_the_corner();
    assert!(blocked(&triangles, from, to), "a crack under τ blocks");
    assert!(
        !with_tolerance(0.0, || blocked(&triangles, from, to)),
        "τ = 0 lets it through"
    );
}

/// The oversize-face cut adds a vertex on a cut face's boundary edge that
/// the neighbour across it lacks. Displaced 10 µm off that edge, as a
/// rounded crossing can leave it, the T-junction opens a sliver of floor
/// that belongs to neither face.
#[test]
fn t_junction_crack_blocks_and_fails_at_zero_tolerance() {
    let offset = 1.0e-5;
    let (p, q) = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0));
    let t_vertex = Vec3::new(1.0, 0.0, offset);
    let apex = Vec3::new(1.0, 0.0, 1.0);
    let triangles = vec![
        [p, Vec3::new(1.0, 0.0, -1.0), q],
        [p, apex, t_vertex],
        [t_vertex, apex, q],
    ];
    let (from, to) = down_through(1.0, offset * 0.5);
    assert!(
        blocked(&triangles, from, to),
        "a T-junction crack under τ blocks"
    );
    assert!(
        !with_tolerance(0.0, || blocked(&triangles, from, to)),
        "τ = 0 lets it through"
    );
}

/// A crack wide enough that a ray through its middle passes more than the
/// pad from both faces is a hole, and the ray clears.
#[test]
fn crack_wider_than_twice_the_pad_clears_a_ray_through_its_middle() {
    let pad = box_pad(2.0);
    let gap = 4.0 * pad;
    let triangles = vec![
        [
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(-2.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, 1.0),
        ],
        [
            Vec3::new(gap, 0.0, -1.0),
            Vec3::new(gap, 0.0, 1.0),
            Vec3::new(2.0, 0.0, -1.0),
        ],
    ];
    let (from, to) = down_through(gap * 0.5, 0.0);
    assert!(!blocked(&triangles, from, to));
}

/// Silhouette over-shadowing stays within τ, measured in the triangle's
/// plane: beside an edge, and beyond a sliver's 1° vertex, where offsetting
/// the edges by τ would reach about 115τ.
#[test]
fn tolerance_reaches_exactly_tau_past_an_edge_and_a_sliver_vertex() {
    let near = TAU * (1.0 - 1e-3);
    let far = TAU * (1.0 + 1e-3);
    let square = vec![[
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
    ]];
    let (from, to) = down_through(-near, 0.5);
    assert!(
        blocked(&square, from, to),
        "just under τ past the edge blocks"
    );
    let (from, to) = down_through(-far, 0.5);
    assert!(
        !blocked(&square, from, to),
        "just over τ past the edge clears"
    );

    let half = (0.5f32).to_radians().tan();
    let sliver = vec![[
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, half),
        Vec3::new(1.0, 0.0, -half),
    ]];
    let (from, to) = down_through(-near, 0.0);
    assert!(
        blocked(&sliver, from, to),
        "just under τ beyond the vertex blocks"
    );
    let (from, to) = down_through(-far, 0.0);
    assert!(
        !blocked(&sliver, from, to),
        "just over τ beyond the vertex clears"
    );
}

/// A 1 mm triangle crossed 0.5° from its plane: Möller–Trumbore's `det` is
/// below its 1e-8 floor there, so it rejected a real crossing.
#[test]
fn small_triangle_at_a_grazing_angle_blocks() {
    let triangle = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0e-3, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0e-3),
    ];
    let angle = (0.5f32).to_radians();
    let dir = Vec3::new(angle.cos(), -angle.sin(), 0.0);
    let crossing = Vec3::new(2.5e-4, 0.0, 2.5e-4);
    let (from, to) = (crossing - dir, crossing + dir * 0.5);
    assert!(blocked(&[triangle], from, to));
    let origin = from + dir * RAY_EPSILON;
    assert!(
        moller_trumbore(origin, dir, triangle, 0.0).is_some(),
        "the crossing is real"
    );
    assert_eq!(
        moller_trumbore(origin, dir, triangle, MT_DET_FLOOR),
        None,
        "the old det floor rejects it"
    );
}

/// The plane x = 1 crossed at exactly t = 1 by a ray from the origin along
/// +x, in either winding: a far bound just past the crossing hits, one just
/// short of it does not, whichever sign `det` takes.
#[test]
fn both_windings_hit_up_to_the_segment_end() {
    let wound = [
        Vec3::new(1.0, -1.0, -1.0),
        Vec3::new(1.0, 1.0, -1.0),
        Vec3::new(1.0, 0.0, 1.0),
    ];
    let [a, b, c] = wound;
    let ray = TraceRay::new(Vec3::ZERO, Vec3::X);
    let mut fronts = Vec::new();
    for triangle in [wound, [a, c, b]] {
        let hit = ray
            .hit(triangle, 0.0, 1.0f32.next_up())
            .expect("a hit just short of the end");
        assert_eq!(hit.t, 1.0);
        fronts.push(hit.front);
        assert_eq!(
            ray.hit(triangle, 0.0, 1.0),
            None,
            "a hit at the end is past it"
        );
        assert_eq!(ray.hit(triangle, 0.0, 1.0f32.next_down()), None);
        assert!(blocked(&[triangle], Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0)));
    }
    assert_ne!(fronts[0], fronts[1], "the windings take opposite det signs");
}

/// For a ray whose dominant axis is each of ±x, ±y and ±z, a triangle whose
/// front faces it is hit as a front, and the same triangle reversed is hit
/// as a back: `det`'s sign follows the geometric normal. The front-filtered
/// callers see the first and skip the second.
#[test]
fn front_faces_follow_the_geometric_normal_on_every_dominant_axis() {
    for axis in 0..3 {
        for sign in [1.0f32, -1.0] {
            let mut dir = Vec3::splat(0.2);
            dir[axis] = sign;
            let dir = dir.normalize();
            let (_, b, c) = axis_frame(axis);
            let (b, c) = (b.as_vec3(), c.as_vec3());
            let centre = dir * 2.0;
            // Wound so its normal opposes the ray.
            let facing = {
                let tri = [centre - b - c, centre + b - c, centre + c];
                let n = (tri[1] - tri[0]).cross(tri[2] - tri[0]);
                if n.dot(dir) < 0.0 {
                    tri
                } else {
                    [tri[0], tri[2], tri[1]]
                }
            };
            let reversed = [facing[0], facing[2], facing[1]];
            for (triangle, front) in [(facing, true), (reversed, false)] {
                let normal = (triangle[1] - triangle[0]).cross(triangle[2] - triangle[0]);
                let hit = TraceRay::new(Vec3::ZERO, dir)
                    .hit(triangle, 0.0, 10.0)
                    .expect("the ray crosses the triangle");
                assert_eq!(hit.front, front, "axis {axis} sign {sign}");
                assert_eq!(hit.front, normal.dot(dir) < 0.0, "det sign vs normal");
                let scene = Scene::new(&[triangle]);
                let found = scene.entry_points_hit(Vec3::ZERO, dir * 4.0);
                for (entry, hit) in found {
                    if entry.starts_with("front:") {
                        assert_eq!(hit, front, "{entry}, axis {axis} sign {sign}");
                    } else {
                        assert!(hit, "{entry}, axis {axis} sign {sign}");
                    }
                }
            }
        }
    }
}
