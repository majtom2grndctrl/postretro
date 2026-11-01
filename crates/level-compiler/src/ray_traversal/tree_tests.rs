// Bake trees and the occluder cache: padded boxes, containment, leaf order, cache independence.
// See: context/lib/build_pipeline.md §Ray traversal

use std::cell::Cell;

use bvh::aabb::{Aabb, Bounded};
use bvh::bvh::{Bvh, BvhNode};
use glam::{DVec3, Vec3};

use super::test_fixtures::{Scene, XorShift, triangles_geometry};
use super::{
    Occluder, TRACER_TOLERANCE_METERS, TraceRay, assert_padded, box_pad, primitive_triangles,
    segment_clear_brute_force, segment_clear_remembering, triangle_points, with_planted_t,
    with_tolerance, without_cache_gate,
};
use crate::bake_counters::counted;
use crate::bvh_build::{BvhPrimitive, build_bvh, build_bvh_exact};

/// Two crossing creased faces, a far floor, a wall and scattered tiles: a
/// tree several levels deep, with shared crease edges.
fn crease_scene_triangles() -> Vec<[Vec3; 3]> {
    let v = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    let mut triangles = vec![
        [v(0.0, 0.0, -1.0), v(0.0, 0.0, 1.0), v(-1.0, 1.0, 0.0)],
        [v(0.0, 0.0, 1.0), v(0.0, 0.0, -1.0), v(1.0, 1.0, 0.0)],
        [
            v(-20.0, -6.0, -20.0),
            v(20.0, -6.0, -20.0),
            v(20.0, -6.0, 20.0),
        ],
        [
            v(-20.0, -6.0, -20.0),
            v(20.0, -6.0, 20.0),
            v(-20.0, -6.0, 20.0),
        ],
        [v(8.0, -6.0, -20.0), v(8.0, 10.0, -20.0), v(8.0, 10.0, 20.0)],
    ];
    for k in 0..12 {
        let x = -7.0 + k as f32 * 1.25;
        triangles.push([v(x, -3.0, -4.0), v(x + 0.5, -3.0, -4.0), v(x, -2.5, -3.5)]);
    }
    triangles
}

/// The largest coordinate magnitude of `primitives`' exact boxes.
fn extent(primitives: &[BvhPrimitive]) -> f32 {
    primitives
        .iter()
        .flat_map(|p| p.aabb_min.iter().chain(&p.aabb_max))
        .fold(0.0f32, |e, &c| e.max(c.abs()))
}

/// `padded` holds `exact` grown by `pad` on every face, compared exactly.
fn holds_grown(padded: &Aabb<f32, 3>, exact: &Aabb<f32, 3>, pad: f64) -> bool {
    (0..3).all(|axis| {
        f64::from(padded.min[axis]) <= f64::from(exact.min[axis]) - pad
            && f64::from(padded.max[axis]) >= f64::from(exact.max[axis]) + pad
    })
}

/// The `f32` coordinate nearest 1.5 whose sum with `pad` rounds inward of
/// the exact sum, where a plain add would leave the box short.
fn inward_rounding_coordinate(pad: f32) -> f32 {
    let mut c = 1.5f32;
    while f64::from(c + pad) >= f64::from(c) + f64::from(pad) {
        c = c.next_up();
    }
    c
}

#[test]
fn padded_boxes_hold_exact_boxes_grown_by_tau_plus_an_ulp_of_the_extent() {
    let probe_pad = box_pad(300.0);
    let c = inward_rounding_coordinate(probe_pad);
    let mut triangles = crease_scene_triangles();
    // Faces at coordinate 0, where an outward step alone is subnormal; one
    // reaching the inward-rounding coordinate; one far out, setting the extent.
    triangles.push([
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(c, 0.0, 0.0),
        Vec3::new(0.0, 0.0, c),
    ]);
    triangles.push([
        Vec3::new(-300.0, 2.0, 0.0),
        Vec3::new(-299.0, 2.0, 0.0),
        Vec3::new(-300.0, 3.0, 1.0),
    ]);
    let geometry = triangles_geometry(&triangles);
    let (exact_bvh, exact_prims, exact_section) = build_bvh_exact(&geometry).unwrap();
    let (bvh, prims, section) = build_bvh(&geometry).unwrap();
    assert_eq!(
        section, exact_section,
        "the shipped tree keeps the exact boxes"
    );

    let e = extent(&exact_prims);
    assert_eq!(e, 300.0);
    let pad = f64::from(TRACER_TOLERANCE_METERS) + f64::from(e.next_up() - e);
    for (padded, exact) in prims.iter().zip(&exact_prims) {
        assert!(
            holds_grown(&padded.aabb(), &exact.aabb(), pad),
            "primitive {:?}..{:?} not grown from {:?}..{:?}",
            padded.aabb_min,
            padded.aabb_max,
            exact.aabb_min,
            exact.aabb_max
        );
    }
    for (padded, exact) in bvh.nodes.iter().zip(&exact_bvh.nodes) {
        if let (
            BvhNode::Node {
                child_l_aabb,
                child_r_aabb,
                ..
            },
            BvhNode::Node {
                child_l_aabb: exact_l,
                child_r_aabb: exact_r,
                ..
            },
        ) = (padded, exact)
        {
            assert!(holds_grown(child_l_aabb, exact_l, pad));
            assert!(holds_grown(child_r_aabb, exact_r, pad));
        }
    }
}

/// Every point the hit test accepts on a triangle, tolerance hits at τ
/// included, lies in that triangle's padded box.
#[test]
fn padded_boxes_hold_every_point_the_hit_test_accepts() {
    let triangles = crease_scene_triangles();
    let scene = Scene::new(&triangles);
    let mut rng = XorShift(0xD1B5_4A32_D192_ED03);
    let mut accepted = 0;
    for prim in &scene.primitives {
        let aabb = prim.aabb();
        for tri in primitive_triangles(prim) {
            let corners = triangle_points(&scene.geometry, tri);
            for _ in 0..400 {
                // Aim near an edge or a corner, up to 2τ outside.
                let (k, w) = (rng.index(3), rng.unit());
                let on_edge = corners[k]
                    .as_dvec3()
                    .lerp(corners[(k + 1) % 3].as_dvec3(), w);
                let normal = (corners[1] - corners[0])
                    .as_dvec3()
                    .cross((corners[2] - corners[0]).as_dvec3())
                    .normalize();
                let target = on_edge
                    + rng.unit_vec().reject_from(normal).normalize_or_zero()
                        * rng.range(0.0, 2.0)
                        * f64::from(TRACER_TOLERANCE_METERS);
                let dir = rng.unit_vec();
                if dir.dot(normal).abs() < 0.05 {
                    continue;
                }
                let origin = target - dir * rng.range(0.01, 50.0);
                let ray = TraceRay::new(origin.as_vec3(), dir.as_vec3());
                let Some(hit) = ray.hit(corners, 0.0, f32::INFINITY) else {
                    continue;
                };
                accepted += 1;
                let point = origin.as_vec3().as_dvec3() + dir.as_vec3().as_dvec3() * hit.t;
                assert!(
                    (0..3).all(|axis| f64::from(aabb.min[axis]) <= point[axis]
                        && point[axis] <= f64::from(aabb.max[axis])),
                    "accepted point {point} outside {:?}..{:?}",
                    aabb.min,
                    aabb.max
                );
            }
        }
    }
    assert!(accepted > 1_000, "{accepted} accepted hits");
}

#[test]
fn every_tree_the_bvh_constructor_returns_is_padded() {
    let geometry = triangles_geometry(&crease_scene_triangles());
    let (_, prims, _) = build_bvh(&geometry).unwrap();
    assert_padded(&prims, &geometry);
}

#[test]
fn a_fixture_map_with_no_cut_traces_a_padded_tree() {
    let fixture = crate::fixture_pipeline::load_fixture("soft_shadow_test");
    assert_padded(&fixture.primitives, &fixture.geometry);
    super::assert_parents_contain_children(&fixture.bvh, &fixture.primitives);
}

/// A parent whose stored box omits a sliver of one child's contents.
#[test]
#[should_panic(expected = "does not hold")]
fn a_parent_box_omitting_a_child_sliver_trips_the_containment_assertion() {
    let geometry = triangles_geometry(&crease_scene_triangles());
    let (mut bvh, prims, _) = build_bvh(&geometry).unwrap();
    let node = bvh
        .nodes
        .iter_mut()
        .find_map(|node| match node {
            BvhNode::Node { child_l_aabb, .. } => Some(child_l_aabb),
            BvhNode::Leaf { .. } => None,
        })
        .expect("an interior node");
    node.max.x -= 1.0e-3;
    super::assert_parents_contain_children(&bvh, &prims);
}

#[test]
fn every_gate_fixture_bvh_passes_the_containment_assertion() {
    for name in crate::fixture_pipeline::GATE_FIXTURES {
        let fixture = crate::fixture_pipeline::load_fixture(name);
        super::assert_parents_contain_children(&fixture.bvh, &fixture.primitives);
    }
}

/// The leaves a query yields, in order, by primitive index offset.
fn leaf_order(bvh: &Bvh<f32, 3>, prims: &[BvhPrimitive], ray: &TraceRay) -> Vec<u32> {
    let query = ray.query(f32::INFINITY);
    bvh.traverse_iterator(&query, prims)
        .map(|prim| prim.index_offset)
        .collect()
}

/// Padding keeps the topology, so a ray that reaches the same leaves in the
/// exact and the padded tree reaches them in the same order, and a
/// closest-hit tie keeps its winner.
#[test]
fn padded_tree_yields_leaves_in_the_exact_trees_order() {
    let geometry = triangles_geometry(&crease_scene_triangles());
    let (exact_bvh, exact_prims, _) = build_bvh_exact(&geometry).unwrap();
    let (bvh, prims, _) = build_bvh(&geometry).unwrap();
    let mut rng = XorShift(0x2545_F491_4F6C_DD1D);
    let mut compared = 0;
    for _ in 0..4_000 {
        let target = DVec3::new(
            rng.range(-1.0, 1.0),
            rng.range(0.0, 1.0),
            rng.range(-1.0, 1.0),
        );
        let dir = rng.unit_vec();
        let ray = TraceRay::new(
            (target - dir * rng.range(1.0, 30.0)).as_vec3(),
            dir.as_vec3(),
        );
        let exact = leaf_order(&exact_bvh, &exact_prims, &ray);
        let padded = leaf_order(&bvh, &prims, &ray);
        let mut exact_set = exact.clone();
        let mut padded_set = padded.clone();
        exact_set.sort_unstable();
        padded_set.sort_unstable();
        if exact_set == padded_set {
            compared += 1;
            assert_eq!(padded, exact, "ray from {} along {dir}", ray.origin());
        }
    }
    assert!(compared > 3_000, "{compared} rays compared");
}

/// The shadow answer with an empty cache, checked against each primitive
/// cached in turn.
fn assert_cache_independent(scene: &Scene, from: Vec3, to: Vec3) -> bool {
    let empty = segment_clear_remembering(
        &scene.bvh,
        &scene.primitives,
        &scene.geometry,
        from,
        to,
        &Cell::new(None),
    );
    for prim in &scene.primitives {
        for tri in primitive_triangles(prim) {
            let cached = Cell::new(Some(Occluder::new(
                &scene.bvh,
                &scene.primitives,
                prim,
                tri,
            )));
            assert_eq!(
                segment_clear_remembering(
                    &scene.bvh,
                    &scene.primitives,
                    &scene.geometry,
                    from,
                    to,
                    &cached
                ),
                empty,
                "{from} -> {to} with triangle {tri} cached"
            );
        }
    }
    empty
}

#[test]
fn cached_occluders_never_change_the_shadow_answer() {
    let scene = Scene::new(&crease_scene_triangles());
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    let (mut clear, mut blocked) = (0, 0);
    for _ in 0..600 {
        let to = DVec3::new(
            rng.range(-9.0, 9.0),
            rng.range(-7.0, 2.0),
            rng.range(-9.0, 9.0),
        );
        let from = to + rng.unit_vec() * rng.range(0.5, 20.0);
        if assert_cache_independent(&scene, from.as_vec3(), to.as_vec3()) {
            clear += 1;
        } else {
            blocked += 1;
        }
    }
    assert!(
        clear > 50 && blocked > 50,
        "{clear} clear, {blocked} blocked"
    );
}

/// Hits exactly on the crease's axis-aligned shared edge and at its shared
/// vertices, at τ = 0 and at the shipped τ: the cached answers equal the
/// empty cache's, which equals the brute-force reference.
#[test]
fn cached_occluders_agree_with_brute_force_at_shared_edges_and_vertices() {
    let scene = Scene::new(&crease_scene_triangles());
    let origins = [
        Vec3::new(0.0, 5.0, 0.0),
        Vec3::new(3.0, 4.0, 1.0),
        Vec3::new(-2.0, 3.0, -1.0),
        Vec3::new(0.25, 6.0, -0.5),
    ];
    for tau in [0.0, TRACER_TOLERANCE_METERS] {
        with_tolerance(tau, || {
            for origin in origins {
                for z in [-1.0, -0.75, -0.3, 0.0, 0.4, 0.9, 1.0] {
                    let on_edge = Vec3::new(0.0, 0.0, z);
                    let to = on_edge + (on_edge - origin).normalize() * 0.5;
                    let answer = assert_cache_independent(&scene, origin, to);
                    assert_eq!(
                        answer,
                        segment_clear_brute_force(&scene.geometry, origin, to)
                    );
                }
            }
        });
    }
}

/// A one-primitive tree is a root leaf, whose box the traversal tests as
/// the primitive's own.
#[test]
fn one_primitive_tree_gates_its_cached_occluder_on_the_primitives_box() {
    let triangle = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 1.0),
    ];
    let scene = Scene::new(&[triangle]);
    assert!(matches!(scene.bvh.nodes[..], [BvhNode::Leaf { .. }]));
    for (from, to) in [
        (Vec3::new(0.7, 1.0, 0.3), Vec3::new(0.7, -1.0, 0.3)),
        (Vec3::new(1.0, 1.0, 1.0), Vec3::new(1.0, -1.0, 1.0)),
        (Vec3::new(1.0, 1.0, 0.5), Vec3::new(1.0, -1.0, 0.5)),
        (Vec3::new(2.0, 1.0, 0.5), Vec3::new(2.0, -1.0, 0.5)),
        (Vec3::new(0.7, 1.0, 0.3), Vec3::new(0.7, 0.5, 0.3)),
    ] {
        assert_eq!(
            assert_cache_independent(&scene, from, to),
            segment_clear_brute_force(&scene.geometry, from, to),
            "{from} -> {to}"
        );
    }
}

/// A grazing sliver far past a segment's end, its box starting past the
/// prune limit, with its hit test's `t` planted inside the segment: the
/// traversal prunes the box, and the gated cache tests the same box first
/// and skips it too. Ungated, the cache reports a hit the traversal can
/// never reach.
#[test]
fn cached_sliver_past_the_prune_limit_answers_as_an_empty_cache() {
    let sliver = [
        Vec3::new(30.0, -0.01, -0.001),
        Vec3::new(60.0, 0.01, 0.0),
        Vec3::new(30.0, -0.01, 0.001),
    ];
    let floor = [
        Vec3::new(-1.0, -2.0, -1.0),
        Vec3::new(1.0, -2.0, 1.0),
        Vec3::new(1.0, -2.0, -1.0),
    ];
    for triangles in [vec![sliver], vec![floor, sliver]] {
        let scene = Scene::new(&triangles);
        let (from, to) = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0));
        let sliver_prim = scene
            .primitives
            .iter()
            .find(|p| triangle_points(&scene.geometry, p.index_offset as usize) == sliver)
            .expect("the sliver's primitive");
        let cached = || {
            Cell::new(Some(Occluder::new(
                &scene.bvh,
                &scene.primitives,
                sliver_prim,
                sliver_prim.index_offset as usize,
            )))
        };
        let trace = |cache: &Cell<Option<Occluder>>| {
            segment_clear_remembering(
                &scene.bvh,
                &scene.primitives,
                &scene.geometry,
                from,
                to,
                cache,
            )
        };
        with_planted_t(sliver, 5.0, || {
            assert!(
                trace(&Cell::new(None)),
                "the traversal prunes the sliver's box"
            );
            assert!(trace(&cached()), "the gated cache skips it too");
            assert!(
                !without_cache_gate(|| trace(&cached())),
                "ungated, the cache reports the planted hit"
            );
        });
    }
}

/// A segment that ends well short of a box's entry tests none of that box's
/// triangles, cached or not.
#[test]
fn segment_short_of_a_box_tests_none_of_its_triangles() {
    let far = [
        Vec3::new(20.0, -1.0, -1.0),
        Vec3::new(20.0, 1.0, -1.0),
        Vec3::new(20.0, 0.0, 1.0),
    ];
    let aside = [
        Vec3::new(0.0, 5.0, 0.0),
        Vec3::new(1.0, 5.0, 0.0),
        Vec3::new(0.0, 5.0, 1.0),
    ];
    let scene = Scene::new(&[aside, far]);
    let far_prim = scene
        .primitives
        .iter()
        .find(|p| triangle_points(&scene.geometry, p.index_offset as usize) == far)
        .expect("the far primitive");
    let tests = |cache: Option<Occluder>, gated: bool| {
        let ((), counts) = counted(1, || {
            let trace = || {
                assert!(segment_clear_remembering(
                    &scene.bvh,
                    &scene.primitives,
                    &scene.geometry,
                    Vec3::ZERO,
                    Vec3::new(5.0, 0.0, 0.0),
                    &Cell::new(cache),
                ));
            };
            if gated {
                trace()
            } else {
                without_cache_gate(trace)
            }
        });
        counts.shadow_trace.triangle_tests
    };
    let far_cached = Some(Occluder::new(
        &scene.bvh,
        &scene.primitives,
        far_prim,
        far_prim.index_offset as usize,
    ));
    assert_eq!(tests(None, true), 0);
    assert_eq!(tests(far_cached, true), 0);
    assert_eq!(tests(far_cached, false), 1, "ungated, the cache tests it");
}
