use std::sync::Arc;

use glam::{DVec3, Vec3};

use super::posed_solid::box_brush;
use super::{InsideBrushRule, JointVisibility, PosedOccluder, PosedSolid};
use crate::bvh_build::build_bvh;
use crate::geometry::{FaceIndexRange, GeometryResult};
use crate::lightmap_bake::{DEFAULT_AREA_SAMPLE_COUNT, RAY_EPSILON, area_sample_target};
use crate::map_data::{
    BrushVolume, FalloffModel, KinematicMoveMode, LightType, MapKinematicMover,
    MapKinematicWaypoint, MapLight, ShadowType,
};
use crate::mover_light_gate::{MoverPoses, Terminus};
use crate::sh_bake::{RaytracingCtx, soft_visibility_seed};
use crate::sh_bake::{bake_probe_direct_rgb_posed, probe_direct_joint_visibility};
use crate::texel_footprint::ProbeEarlyOut;
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

/// A mover holding `brushes` at its first waypoint, which sits at the
/// origin; its last waypoint is 100 m up, out of every test's way.
fn occluder(brushes: Vec<BrushVolume>, rule: InsideBrushRule) -> PosedOccluder {
    PosedOccluder::new(Arc::new(solid(brushes)), rule)
}

/// [`occluder`]'s mover posed at its first waypoint, as a solid.
fn solid(brushes: Vec<BrushVolume>) -> PosedSolid {
    let mover = MapKinematicMover {
        mover_id: 0,
        name: "mover".to_string(),
        tags: Vec::new(),
        origin: DVec3::ZERO,
        authored_origin: None,
        path: "first".to_string(),
        speed: 1.0,
        wait_ms: 0.0,
        spin_axis: [0.0; 3],
        spin_speed_deg_s: 0.0,
        spin_accel_deg_s2: 0.0,
        carry_yaw: false,
        block_policy: "displace".to_string(),
        crush_damage: 0.0,
        crush_interval_ms: 0.0,
        auto_close_ms: None,
        open_event: None,
        close_event: None,
        blocked_event: None,
        crush_event: None,
        open_sound: None,
        close_sound: None,
        blocked_sound: None,
        crush_sound: None,
        move_mode: KinematicMoveMode::Once,
        start_on_spawn: false,
        brush_volumes: brushes,
        crush_damage_type: None,
        light_ramp_start: 0.0,
        light_ramp_end: 1.0,
        light_bounces: None,
    };
    let waypoints = [
        ("first", "last", DVec3::ZERO),
        ("last", "", DVec3::Y * 100.0),
    ]
    .map(|(name, next, origin)| MapKinematicWaypoint {
        name: name.to_string(),
        next: next.to_string(),
        origin,
    });
    let poses = MoverPoses::resolve(std::slice::from_ref(&mover), &waypoints);
    let pose = poses.mover(0).expect("posed").pose(Terminus::First);
    PosedSolid::new(&mover, pose, &TextureNamesSection { names: Vec::new() })
}

fn soft_light(origin: DVec3, light_size: f32) -> MapLight {
    MapLight {
        origin,
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: 50.0,
        light_size,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: None,
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: vec![],
        shadow_type: ShadowType::StaticLightMap,
    }
}

fn geometry_of(triangles: &[[[f32; 3]; 3]]) -> GeometryResult {
    let vertex = |position| {
        Vertex::new(
            position,
            [0.0; 2],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0; 2],
            0,
        )
    };
    GeometryResult {
        geometry: GeometrySection {
            vertices: triangles.iter().flatten().map(|&p| vertex(p)).collect(),
            indices: (0..triangles.len() as u32 * 3).collect(),
            faces: triangles
                .iter()
                .map(|_| FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                })
                .collect(),
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: (0..triangles.len() as u32)
            .map(|face| FaceIndexRange {
                index_offset: face * 3,
                index_count: 3,
            })
            .collect(),
    }
}

/// A probe at the origin under a 0.6 m emitter 3 m up. A static triangle at
/// y 2 hides the emitter's −x side; the posed slab at y 1.5 hides its +z
/// side. Both cut through the penumbra, so the sample set escalates and each
/// trace blocks a different part of it.
struct Scene {
    geometry: GeometryResult,
    light: MapLight,
    occluder: PosedOccluder,
}

const PROBE: Vec3 = Vec3::ZERO;
const PROBE_INDEX: u64 = 7;
const GLOBAL_LIGHT_INDEX: u64 = 3;

fn scene(slab_min: DVec3, slab_max: DVec3) -> Scene {
    Scene {
        geometry: geometry_of(&[[[-0.1, 2.0, -50.0], [-0.1, 2.0, 50.0], [-50.0, 2.0, 0.0]]]),
        light: soft_light(DVec3::new(0.0, 3.0, 0.0), 0.6),
        occluder: occluder(
            vec![box_brush(slab_min, slab_max)],
            InsideBrushRule::SkipContainingBrush,
        ),
    }
}

fn split_scene() -> Scene {
    scene(DVec3::new(-5.0, 1.5, 0.05), DVec3::new(5.0, 1.6, 5.0))
}

/// L0 red of the probe's direct-SH bake for the scene's one light, with
/// and without the pose.
fn baked_l0(scene: &Scene, posed: bool) -> f32 {
    let (bvh, primitives, _) = build_bvh(&scene.geometry).expect("bvh");
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &scene.geometry,
    };
    bake_probe_direct_rgb_posed(
        &ctx,
        PROBE,
        &[&scene.light],
        &[GLOBAL_LIGHT_INDEX],
        &[ProbeEarlyOut::Agreed],
        PROBE_INDEX,
        posed.then_some(&scene.occluder),
    )[0]
}

fn joint(scene: &Scene) -> JointVisibility {
    let (bvh, primitives, _) = build_bvh(&scene.geometry).expect("bvh");
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &scene.geometry,
    };
    let l = (scene.light.origin.as_vec3() - PROBE).normalize();
    probe_direct_joint_visibility(
        &ctx,
        PROBE,
        l,
        &scene.light,
        soft_visibility_seed(PROBE_INDEX, 0, GLOBAL_LIGHT_INDEX),
        ProbeEarlyOut::Agreed,
        &scene.occluder,
    )
}

/// Each trace counted over every sample of the full lattice, by hand.
fn brute_force_counts(scene: &Scene) -> (u32, u32) {
    let (bvh, primitives, _) = build_bvh(&scene.geometry).expect("bvh");
    let l = (scene.light.origin.as_vec3() - PROBE).normalize();
    let origin = PROBE + l * RAY_EPSILON;
    let seed = soft_visibility_seed(PROBE_INDEX, 0, GLOBAL_LIGHT_INDEX);
    let (mut static_clear, mut posed_clear) = (0, 0);
    for i in 0..DEFAULT_AREA_SAMPLE_COUNT {
        let target = area_sample_target(PROBE, &scene.light, seed, i, DEFAULT_AREA_SAMPLE_COUNT);
        if crate::ray_traversal::segment_clear(&bvh, &primitives, &scene.geometry, origin, target) {
            static_clear += 1;
            if !scene.occluder.blocks(PROBE, origin, target) {
                posed_clear += 1;
            }
        }
    }
    (static_clear, posed_clear)
}

// Bake row B1: q is the posed clear count over the joint set's static clear
// count, same seed, and the posed bake scales the light's term by its byte.
#[test]
fn posed_direct_bake_scales_a_light_by_its_joint_sample_surviving_fraction() {
    let scene = split_scene();
    let (static_clear, posed_clear) = brute_force_counts(&scene);
    assert!(
        0 < posed_clear && posed_clear < static_clear && static_clear < DEFAULT_AREA_SAMPLE_COUNT,
        "fixture must block part of the emitter under each trace: static {static_clear}, posed {posed_clear}"
    );
    let joint = joint(&scene);
    assert_eq!(
        joint,
        JointVisibility {
            samples: DEFAULT_AREA_SAMPLE_COUNT,
            static_clear,
            posed_clear,
        }
    );
    let expected_q = posed_clear as f32 / static_clear as f32;
    let baked_q = baked_l0(&scene, true) / baked_l0(&scene, false);
    assert!(
        (baked_q - expected_q).abs() <= 1.0 / 510.0 + 1.0e-6,
        "baked q {baked_q} against joint q {expected_q}"
    );
    assert!(joint.meets_mover());
}

// Bake row B1: a light whose samples never meet the posed mover stores no
// entry, and the posed bake leaves its term bit for bit.
#[test]
fn a_light_whose_samples_miss_the_pose_keeps_its_term_and_meets_no_mover() {
    let scene = scene(DVec3::new(10.0, 1.5, 10.0), DVec3::new(11.0, 1.6, 11.0));
    let joint = joint(&scene);
    assert!(!joint.meets_mover());
    assert_eq!(joint.surviving_byte(), 255);
    assert_eq!(
        baked_l0(&scene, true).to_bits(),
        baked_l0(&scene, false).to_bits()
    );
}

// A mover that covers the whole emitter stores q = 0 even where the four
// probe samples agree, and a hard emitter's one ray gives q of 0 or 1.
#[test]
fn a_sealing_pose_stores_zero_and_a_hard_light_stores_zero_or_one() {
    let mut sealed = scene(DVec3::new(-5.0, 1.5, -5.0), DVec3::new(5.0, 1.6, 5.0));
    sealed.geometry = geometry_of(&[]);
    assert_eq!(joint(&sealed).surviving_byte(), 0);
    assert_eq!(baked_l0(&sealed, true), 0.0);

    let mut hard = split_scene();
    hard.light.light_size = 0.0;
    hard.geometry = geometry_of(&[]);
    let hard_joint = joint(&hard);
    assert_eq!(hard_joint.samples, 1);
    // The hard ray runs straight up the y axis, past the slab's z ≥ 0.05 edge.
    assert_eq!(hard_joint.surviving_byte(), 255);
}

// The inside-brush rule: a probe inside one of the mover's brushes is tested
// without that brush, and the mover's other brush still occludes; under the
// dark rule every segment starts in contact.
#[test]
fn a_probe_inside_a_posed_brush_skips_only_the_brush_that_contains_it() {
    let containing = box_brush(DVec3::splat(-0.5), DVec3::splat(0.5));
    let beside = box_brush(DVec3::new(2.0, -1.0, -1.0), DVec3::new(2.2, 1.0, 1.0));
    let up = soft_light(DVec3::new(0.0, 3.0, 0.0), 0.0);
    let east = soft_light(DVec3::new(4.0, 0.0, 0.0), 0.0);
    let blocked = |rule, light: &MapLight| {
        let occluder = occluder(vec![containing.clone(), beside.clone()], rule);
        let from = PROBE + (light.origin.as_vec3() - PROBE).normalize() * RAY_EPSILON;
        occluder.blocks(PROBE, from, light.origin.as_vec3())
    };
    assert!(!blocked(InsideBrushRule::SkipContainingBrush, &up));
    assert!(blocked(InsideBrushRule::SkipContainingBrush, &east));
    assert!(blocked(InsideBrushRule::Dark, &up));
    assert!(blocked(InsideBrushRule::Dark, &east));
}

/// The unit box's six faces, two triangles each, built from its corners
/// rather than from brush sides: the box as a static brush the tracer meets.
fn static_unit_box() -> GeometryResult {
    let corner = |x: u8, y: u8, z: u8| [f32::from(x), f32::from(y), f32::from(z)];
    let quads = [
        [
            corner(1, 0, 0),
            corner(1, 1, 0),
            corner(1, 1, 1),
            corner(1, 0, 1),
        ],
        [
            corner(0, 0, 0),
            corner(0, 0, 1),
            corner(0, 1, 1),
            corner(0, 1, 0),
        ],
        [
            corner(0, 1, 0),
            corner(0, 1, 1),
            corner(1, 1, 1),
            corner(1, 1, 0),
        ],
        [
            corner(0, 0, 0),
            corner(1, 0, 0),
            corner(1, 0, 1),
            corner(0, 0, 1),
        ],
        [
            corner(0, 0, 1),
            corner(1, 0, 1),
            corner(1, 1, 1),
            corner(0, 1, 1),
        ],
        [
            corner(0, 0, 0),
            corner(0, 1, 0),
            corner(1, 1, 0),
            corner(1, 0, 0),
        ],
    ];
    let triangles: Vec<[[f32; 3]; 3]> = quads
        .iter()
        .flat_map(|[a, b, c, d]| [[*a, *b, *c], [*a, *c, *d]])
        .collect();
    geometry_of(&triangles)
}

// The one posed-occluder predicate: a posed brush meets shadow segments and
// gather rays exactly as the static tracer meets the same brush placed
// statically, with τ = 0.5 mm and its grazing-ray rule. A segment or ray
// passing 0.3 mm outside a face's edge is blocked, 0.7 mm outside is clear,
// and one running parallel to a face 0.3 mm off it is blocked only where it
// crosses another face's plane within τ. Growing the posed planes by τ would
// block the parallel segment that crosses no such plane.
#[test]
fn a_posed_brush_meets_grazing_segments_and_rays_as_the_static_tracer_does() {
    let posed = solid(vec![box_brush(DVec3::ZERO, DVec3::ONE)]);
    let geometry = static_unit_box();
    let (bvh, primitives, _) = build_bvh(&geometry).expect("bvh");
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
    };
    let segment = |from: Vec3, to: Vec3| {
        let static_blocked =
            !crate::ray_traversal::segment_clear(&bvh, &primitives, &geometry, from, to);
        let posed_blocked = posed.segment_blocked(from, to, |_| false);
        assert_eq!(
            posed_blocked, static_blocked,
            "segment {from} → {to}: posed {posed_blocked}, static {static_blocked}"
        );
        posed_blocked
    };
    let ray = |origin: Vec3, dir: Vec3| {
        let static_hit = crate::sh_bake::gather_hit(&ctx, origin, dir).map(|hit| hit.distance);
        let posed_hit = posed
            .nearest_hit(origin, dir, f32::INFINITY)
            .map(|hit| hit.distance);
        match (static_hit, posed_hit) {
            (None, None) => None,
            (Some(a), Some(b)) => {
                assert!(
                    (a - b).abs() <= 1.0e-6,
                    "ray {origin} along {dir}: posed hit {b}, static {a}"
                );
                Some(b)
            }
            _ => panic!("ray {origin} along {dir}: posed {posed_hit:?}, static {static_hit:?}"),
        }
    };
    const GRAZE: f32 = 3.0e-4;
    const MISS: f32 = 7.0e-4;

    // Down past the +x face, crossing the top and bottom planes just
    // outside their edges.
    let down = |x: f32| (Vec3::new(x, 2.0, 0.5), Vec3::new(x, -1.0, 0.5));
    let (from, to) = down(1.0 + GRAZE);
    assert!(segment(from, to), "0.3 mm outside the edge is within τ");
    assert!(ray(from, Vec3::NEG_Y).is_some());
    let (from, to) = down(1.0 + MISS);
    assert!(!segment(from, to), "0.7 mm outside the edge is past τ");
    assert!(ray(from, Vec3::NEG_Y).is_none());

    // Obliquely past the corner edge x = 1, z = 1, crossing each side's
    // plane 0.3 mm outside the other side.
    let corner = Vec3::new(1.0 + GRAZE * 0.5, 0.5, 1.0 + GRAZE * 0.5);
    let across = Vec3::new(1.0, 0.0, -1.0).normalize();
    assert!(segment(corner - across * 2.0, corner + across * 2.0));

    // Parallel to the top face, 0.3 mm above it: inside the face's
    // footprint it crosses no plane and is clear; across the whole box it
    // crosses the ±x side planes within τ of their top edges.
    let above = 1.0 + GRAZE;
    assert!(!segment(
        Vec3::new(0.2, above, 0.5),
        Vec3::new(0.8, above, 0.5)
    ));
    assert!(segment(
        Vec3::new(-1.0, above, 0.5),
        Vec3::new(2.0, above, 0.5)
    ));
    let hit = ray(Vec3::new(-1.0, above, 0.5), Vec3::X).expect("the −x side within τ");
    assert!((hit - (1.0 - RAY_EPSILON)).abs() <= 1.0e-5, "hit at {hit}");
    assert!(ray(Vec3::new(0.2, above, 0.5), Vec3::Y).is_none());
}
