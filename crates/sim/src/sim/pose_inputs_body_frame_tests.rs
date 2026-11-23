//! Foot ground probes place a view-frame body (a player) where it is drawn:
//! lower-body heading plus the body turn, at the current-tick (or presented)
//! transform. A model-frame body keeps its transform rotation.
//! See: context/lib/entity_model.md §4 (Facing convention)

use super::*;

use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};
use postretro_entities::components::mesh::{AnimationState, BodyFrame, InterruptPolicy};
use postretro_entities::components::mesh::{MeshAnimation, MeshComponent};
use postretro_entities::{EntityId, EntityRegistry, PoseInputs, Transform};
use postretro_foundation::pose::FootProbe;
use postretro_model::pose_modifier::{JointMask, LegChain, PoseModifierStack};
use postretro_model::skeleton::{AnimationClip, Joint, RestLocal, Skeleton};

use crate::collision::CollisionWorld;
use crate::kinematic_mover::MoverTickStateTable;
use crate::scripting_systems::hit_zones::{HitZoneStore, ModelHitZones, model_matrix};

/// Model-space feet, off the body's vertical axis: a body fronting +Z keeps its
/// left foot on +X.
const FOOT_L: Vec3 = Vec3::new(0.25, -0.7, 0.0);
const FOOT_R: Vec3 = Vec3::new(-0.25, -0.7, 0.0);
/// The pawn's transform (aim) yaw.
const AIM: f32 = 0.2;
/// A lower-body heading folded a radian off the aim (a strafe).
const HEADING: f32 = AIM + 1.0;
const ORIGIN_OFFSET: Vec3 = Vec3::new(0.0, -0.9, 0.0);
/// Model origin 1.0 above the low floor; the capsule centre sits above it.
const MODEL_ORIGIN: Vec3 = Vec3::new(0.0, 1.0, 0.0);
/// The step: ground at y = 0 for x < `STEP_X`, y = `STEP_HEIGHT` beyond it.
const STEP_X: f32 = -0.17;
const STEP_HEIGHT: f32 = 0.2;
const EPS: f32 = 1.0e-4;

fn rest_joint(parent: Option<usize>, translation: Vec3) -> Joint {
    Joint {
        parent,
        inverse_bind: Mat4::IDENTITY.to_cols_array_2d(),
        rest_local: RestLocal {
            translation,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        },
    }
}

/// Root and two foot joints. One clip with no joint tracks, so every sample is
/// the rest pose. Leg `0` (left) drives foot probe `0`.
fn two_leg_model() -> ModelHitZones {
    let leg = |foot_joint: usize| {
        let mut chain_mask = JointMask::new();
        assert!(chain_mask.insert(0));
        assert!(chain_mask.insert(foot_joint));
        LegChain {
            chain_mask,
            foot_joint,
        }
    };
    ModelHitZones {
        skeleton: std::sync::Arc::new(Skeleton {
            joints: vec![
                rest_joint(None, Vec3::ZERO),
                rest_joint(Some(0), FOOT_L),
                rest_joint(Some(0), FOOT_R),
            ],
        }),
        clips: std::sync::Arc::new(vec![AnimationClip {
            name: "idle".to_string(),
            duration: 1.0,
            joints: Vec::new(),
            travel_speed: None,
            events: Vec::new(),
        }]),
        joint_zones: vec![None, None, None],
        sockets: HashMap::new(),
        derived_bound: None,
        legs: vec![leg(1), leg(2)],
        pose_stack: std::sync::Arc::new(PoseModifierStack::default()),
    }
}

fn quad(points: &mut Vec<Vec3>, triangles: &mut Vec<[u32; 3]>, corners: [Vec3; 4]) {
    let base = points.len() as u32;
    points.extend(corners);
    triangles.extend([[base, base + 2, base + 1], [base, base + 3, base + 2]]);
}

/// Flat floor with a step up of [`STEP_HEIGHT`] at `x = STEP_X`. Winding faces
/// up, as `determinism_tests::floor_world`.
fn step_world() -> CollisionWorld {
    let (mut points, mut triangles) = (Vec::new(), Vec::new());
    for (x0, x1, y) in [(-50.0, STEP_X, 0.0), (STEP_X, 50.0, STEP_HEIGHT)] {
        quad(
            &mut points,
            &mut triangles,
            [
                Vec3::new(x0, y, -50.0),
                Vec3::new(x1, y, -50.0),
                Vec3::new(x1, y, 50.0),
                Vec3::new(x0, y, 50.0),
            ],
        );
    }
    CollisionWorld::from_triangles_for_test(points, triangles)
}

fn step_ground(x: f32) -> f32 {
    if x < STEP_X { 0.0 } else { STEP_HEIGHT }
}

/// Ground `y = slope * x`; its up normal is `(-slope, 1, 0)` normalized.
fn slope_world(slope: f32) -> CollisionWorld {
    let (mut points, mut triangles) = (Vec::new(), Vec::new());
    let y = |x: f32| slope * x;
    quad(
        &mut points,
        &mut triangles,
        [
            Vec3::new(-50.0, y(-50.0), -50.0),
            Vec3::new(50.0, y(50.0), -50.0),
            Vec3::new(50.0, y(50.0), 50.0),
            Vec3::new(-50.0, y(-50.0), 50.0),
        ],
    );
    CollisionWorld::from_triangles_for_test(points, triangles)
}

fn transform_at(aim: f32, model_origin: Vec3) -> Transform {
    Transform {
        position: model_origin - ORIGIN_OFFSET,
        rotation: Quat::from_rotation_y(aim),
        scale: Vec3::ONE,
    }
}

/// A legged animated body at [`MODEL_ORIGIN`] with aim `aim`, carrying the pose
/// inputs the presentation pass writes before the probes run.
fn leg_body(
    body_frame: BodyFrame,
    aim: f32,
    heading: f32,
) -> (EntityRegistry, EntityId, HitZoneStore) {
    let mut states = HashMap::new();
    states.insert(
        "idle".to_string(),
        AnimationState {
            clip: "idle".into(),
            looping: true,
            crossfade_ms: 0.0,
            interrupt: InterruptPolicy::Smooth,
            travel_speed: None,
            max_playback_rate: None,
            scale_with_speed: true,
            playback_rate: None,
            clip_index: Some(0),
        },
    );
    let mut mesh =
        MeshComponent::animated("two_legs".into(), MeshAnimation::new(states, "idle".into()))
            .with_origin_offset(ORIGIN_OFFSET)
            .with_body_frame(body_frame);
    mesh.pose_inputs = Some(PoseInputs {
        aim_pitch: 0.1,
        aim_yaw: aim,
        heading_yaw: heading,
        ..PoseInputs::default()
    });
    let mut registry = EntityRegistry::new();
    let id = registry.spawn(transform_at(aim, MODEL_ORIGIN));
    registry.set_component(id, mesh).unwrap();
    let mut store = HitZoneStore::new();
    store.insert_for_test(
        postretro_model::ModelHandle::from("two_legs".to_string()),
        two_leg_model(),
    );
    (registry, id, store)
}

/// The per-tick probe pass, against the registry's current-tick transforms.
fn run_probes(registry: &mut EntityRegistry, world: &CollisionWorld, store: &HitZoneStore) {
    super::update_foot_ground_probes(
        registry,
        world,
        &[],
        &MoverTickStateTable::default(),
        store,
        0.0,
    );
}

fn inputs(registry: &EntityRegistry, id: EntityId) -> PoseInputs {
    registry
        .get_component::<MeshComponent>(id)
        .unwrap()
        .pose_inputs
        .expect("probed body carries pose inputs")
}

/// Both feet probed directly with the model placed by `model_to_world`.
fn reference_feet(world: &CollisionWorld, model_to_world: Mat4) -> [FootProbe; 2] {
    let joints = [
        Mat4::IDENTITY,
        Mat4::from_translation(FOOT_L),
        Mat4::from_translation(FOOT_R),
    ];
    let world_to_model = model_to_world.inverse();
    [1, 2].map(|joint| {
        super::probe_foot(
            joint,
            &joints,
            &model_to_world,
            &world_to_model,
            FOOT_PLANTING_REACH,
            FOOT_PENETRATION_ALLOWANCE,
            world,
            &[],
            &MoverTickStateTable::default(),
        )
    })
}

fn placement(transform: &Transform, rotation: Quat) -> Mat4 {
    Mat4::from_scale_rotation_translation(
        transform.scale,
        rotation,
        transform.position + ORIGIN_OFFSET,
    )
}

fn assert_probe_eq(actual: FootProbe, expected: FootProbe, what: &str) {
    assert!(actual.hit && expected.hit, "{what}: both find ground");
    assert!(
        (actual.contact_height - expected.contact_height).abs() < EPS
            && (actual.normal - expected.normal).length() < EPS,
        "{what}: {actual:?} vs {expected:?}"
    );
}

// Acceptance (body-space seams): with heading ≠ aim on a step under one foot,
// each foot's contact height is the ground under the drawn body's foot at the
// current-tick transform.
#[test]
fn view_body_foot_probes_plant_the_drawn_feet_on_a_step() {
    let world = step_world();
    let (mut registry, id, store) = leg_body(BodyFrame::View, AIM, HEADING);
    run_probes(&mut registry, &world, &store);

    let transform = *registry.get_component::<Transform>(id).unwrap();
    let drawn = registry
        .get_component::<MeshComponent>(id)
        .unwrap()
        .presentation_rotation(transform.rotation);
    let drawn_matrix = placement(&transform, drawn);
    let aim_matrix = model_matrix(&transform, ORIGIN_OFFSET, BodyFrame::View).unwrap();

    // The fixture is discriminating: the left foot drawn and the left foot
    // placed by the aim stand on different sides of the step.
    let drawn_left = drawn_matrix.transform_point3(FOOT_L);
    let aim_left = aim_matrix.transform_point3(FOOT_L);
    assert_ne!(step_ground(drawn_left.x), step_ground(aim_left.x));

    let probed = inputs(&registry, id);
    assert_eq!(probed.foot_count, 2);
    for (slot, foot) in [FOOT_L, FOOT_R].into_iter().enumerate() {
        let foot_world = drawn_matrix.transform_point3(foot);
        let expected = step_ground(foot_world.x) - MODEL_ORIGIN.y;
        let probe = probed.feet[slot];
        assert!(probe.hit, "foot {slot} finds the step");
        assert!(
            (probe.contact_height - expected).abs() < EPS,
            "foot {slot}: contact {} under the drawn foot, expected {expected}",
            probe.contact_height
        );
    }
    assert!(
        (probed.feet[0].contact_height - (STEP_HEIGHT - MODEL_ORIGIN.y)).abs() < EPS,
        "the drawn left foot stands on the step, not the low floor the aim places it over"
    );

    // The probe's read-modify-write keeps the aim and heading fields.
    assert_eq!(probed.aim_yaw, AIM);
    assert_eq!(probed.heading_yaw, HEADING);
    assert_eq!(probed.aim_pitch, 0.1);
}

// Acceptance (body-space seams): on a slope whose normal is not vertical, the
// probe's model-space normal is the world normal in the drawn body's frame.
#[test]
fn view_body_foot_probe_normal_is_the_world_normal_in_the_drawn_frame() {
    let slope = 0.3;
    let world = slope_world(slope);
    let (mut registry, id, store) = leg_body(BodyFrame::View, AIM, HEADING);
    run_probes(&mut registry, &world, &store);

    let transform = *registry.get_component::<Transform>(id).unwrap();
    let drawn = BodyFrame::View.model_rotation(Quat::from_rotation_y(HEADING));
    let by_aim = BodyFrame::View.model_rotation(transform.rotation);
    let normal_world = Vec3::new(-slope, 1.0, 0.0).normalize();
    let expected_normal = drawn.inverse() * normal_world;
    assert!(
        (expected_normal - by_aim.inverse() * normal_world).length() > 0.1,
        "the drawn and aim frames disagree on the model-space normal"
    );

    let probed = inputs(&registry, id);
    let drawn_matrix = placement(&transform, drawn);
    for (slot, foot) in [FOOT_L, FOOT_R].into_iter().enumerate() {
        let probe = probed.feet[slot];
        assert!(probe.hit, "foot {slot} finds the walkable slope");
        assert!(
            (probe.normal - expected_normal).length() < EPS,
            "foot {slot}: normal {:?}, expected {expected_normal:?}",
            probe.normal
        );
        let foot_world = drawn_matrix.transform_point3(foot);
        let expected = slope * foot_world.x - MODEL_ORIGIN.y;
        assert!(
            (probe.contact_height - expected).abs() < EPS,
            "foot {slot}: contact {}, expected {expected}",
            probe.contact_height
        );
    }
}

// Acceptance (body-space seams): with heading = aim, the same fixture's probes
// match the authoritative-transform placement plus the turn.
#[test]
fn view_body_foot_probes_match_aim_plus_turn_when_heading_is_aim() {
    for world in [step_world(), slope_world(0.3)] {
        let (mut registry, id, store) = leg_body(BodyFrame::View, AIM, AIM);
        run_probes(&mut registry, &world, &store);
        let transform = *registry.get_component::<Transform>(id).unwrap();
        let expected = reference_feet(
            &world,
            model_matrix(&transform, ORIGIN_OFFSET, BodyFrame::View).unwrap(),
        );
        let probed = inputs(&registry, id);
        assert_eq!(probed.foot_count, 2);
        assert_probe_eq(probed.feet[0], expected[0], "left foot");
        assert_probe_eq(probed.feet[1], expected[1], "right foot");
    }

    // The turn decides the step fixture: turned, the left foot (model +X) lands
    // on the low floor behind the step; unturned it would stand on the step.
    let world = step_world();
    let (mut registry, id, store) = leg_body(BodyFrame::View, AIM, AIM);
    run_probes(&mut registry, &world, &store);
    assert!((inputs(&registry, id).feet[0].contact_height + MODEL_ORIGIN.y).abs() < EPS);
}

// Regression guard: a model-frame body (an enemy) keeps today's probes, on its
// transform rotation, never turned and never moved by a heading.
#[test]
fn model_frame_body_foot_probes_keep_the_transform_rotation() {
    let world = step_world();
    for heading in [AIM, HEADING] {
        let (mut registry, id, store) = leg_body(BodyFrame::Model, AIM, heading);
        run_probes(&mut registry, &world, &store);
        let transform = *registry.get_component::<Transform>(id).unwrap();
        let expected = reference_feet(&world, placement(&transform, transform.rotation));
        let probed = inputs(&registry, id);
        assert_probe_eq(probed.feet[0], expected[0], "left foot");
        assert_probe_eq(probed.feet[1], expected[1], "right foot");
    }
}

// Acceptance (pin R4), pose-inputs half: the host re-probes a connected client's
// pawn after `present_client_pawns` writes the delayed transform and presented
// heading. The re-probe reads the heading the mesh carries at that moment, so
// the feet land under the presented heading plus the turn, not the live tick's.
// The host-seat ordering itself is `host_presentation.rs`'s.
#[test]
fn presented_reprobe_plants_feet_under_the_presented_heading_plus_turn() {
    let world = step_world();
    // Fixed tick: the live pose, its heading on the aim.
    let (mut registry, id, store) = leg_body(BodyFrame::View, AIM, AIM);
    run_probes(&mut registry, &world, &store);

    // Presentation: the delayed sample draws elsewhere, with its own aim and a
    // folded heading (what `present_avatar_pose_inputs` writes).
    let presented_aim = 0.1;
    let presented_heading = presented_aim + 1.0;
    let presented = transform_at(presented_aim, MODEL_ORIGIN + Vec3::new(0.0, 0.0, 0.5));
    registry.set_presentation_transform(id, presented).unwrap();
    let mut mesh = registry.get_component::<MeshComponent>(id).unwrap().clone();
    let pose = mesh.pose_inputs.as_mut().unwrap();
    pose.aim_yaw = presented_aim;
    pose.heading_yaw = presented_heading;
    registry.set_component(id, mesh).unwrap();

    super::update_foot_ground_probes_for(
        &mut registry,
        id,
        presented,
        &world,
        &[],
        &MoverTickStateTable::default(),
        &store,
        0.25,
    );

    let under_presented = reference_feet(
        &world,
        placement(
            &presented,
            BodyFrame::View.model_rotation(Quat::from_rotation_y(presented_heading)),
        ),
    );
    let under_live_heading = reference_feet(
        &world,
        placement(
            &presented,
            BodyFrame::View.model_rotation(Quat::from_rotation_y(AIM)),
        ),
    );
    assert!(
        (under_presented[0].contact_height - under_live_heading[0].contact_height).abs() > 0.1,
        "the fixture tells the two headings apart"
    );
    let probed = inputs(&registry, id);
    assert_probe_eq(probed.feet[0], under_presented[0], "left foot");
    assert_probe_eq(probed.feet[1], under_presented[1], "right foot");
    assert_eq!(probed.heading_yaw, presented_heading);
    assert_eq!(probed.aim_yaw, presented_aim);
}
