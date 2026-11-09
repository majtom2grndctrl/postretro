use glam::{DVec3, Vec3};

use super::{InsideBrushRule, JointVisibility, PosedOccluder};
use crate::bvh_build::build_bvh;
use crate::geometry::{FaceIndexRange, GeometryResult};
use crate::lightmap_bake::{DEFAULT_AREA_SAMPLE_COUNT, RAY_EPSILON, area_sample_target};
use crate::map_data::{
    BrushPlane, BrushVolume, FalloffModel, KinematicMoveMode, LightType, MapKinematicMover,
    MapKinematicWaypoint, MapLight, ShadowType,
};
use crate::mover_light_gate::{MoverPoses, Terminus};
use crate::partition::Aabb;
use crate::sh_bake::{RaytracingCtx, soft_visibility_seed};
use crate::sh_bake::{bake_probe_direct_rgb_posed, probe_direct_joint_visibility};
use crate::texel_footprint::ProbeEarlyOut;
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

fn box_brush(min: DVec3, max: DVec3) -> BrushVolume {
    let planes = [
        (DVec3::X, max.x),
        (DVec3::NEG_X, -min.x),
        (DVec3::Y, max.y),
        (DVec3::NEG_Y, -min.y),
        (DVec3::Z, max.z),
        (DVec3::NEG_Z, -min.z),
    ];
    BrushVolume {
        planes: planes
            .into_iter()
            .map(|(normal, distance)| BrushPlane { normal, distance })
            .collect(),
        sides: Vec::new(),
        aabb: Aabb { min, max },
    }
}

/// A mover holding `brushes` at its first waypoint, which sits at the
/// origin; its last waypoint is 100 m up, out of every test's way.
fn occluder(brushes: Vec<BrushVolume>, rule: InsideBrushRule) -> PosedOccluder {
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
    let poses = MoverPoses::resolve(&[mover], &waypoints);
    let pose = poses.mover(0).expect("posed").pose(Terminus::First).clone();
    PosedOccluder::new(pose, rule)
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
