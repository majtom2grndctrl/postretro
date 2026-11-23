//! Body-frame placement of hit zones and socket anchors: a view-frame player
//! body takes the half turn on its authoritative transform (aim yaw), never the
//! lower-body heading the draw uses; a model-frame body never takes the turn.
//! See: context/lib/entity_model.md §4 (Facing convention)

use super::*;

use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::mesh::{AnimationState, InterruptPolicy};
use postretro_foundation::PoseInputs;
use postretro_model::skeleton::{Joint, RestLocal};

/// The pawn's aim yaw: its transform rotation, forward −Z in the view frame.
const AIM: f32 = 0.3;
/// Model-space chest, on the body's front (glTF +Z). A tagged leaf: one sphere.
const CHEST: Vec3 = Vec3::new(0.0, 1.0, 0.5);
const CHEST_RADIUS: f32 = 0.15;
/// Model-space right hand, off the body's vertical axis. A body fronting +Z
/// keeps its right side on −X.
const HAND_R: Vec3 = Vec3::new(-0.4, 1.0, 0.3);
/// Capsule-centre transform with the feet-authored mesh hung below it.
const POSITION: Vec3 = Vec3::new(2.0, 0.9, -1.0);
const ORIGIN_OFFSET: Vec3 = Vec3::new(0.0, -0.9, 0.0);
const EPS: f32 = 1.0e-4;

fn rest_joint(parent: Option<usize>, translation: Vec3) -> Joint {
    Joint {
        parent,
        inverse_bind: Mat4::IDENTITY.to_cols_array_2d(),
        rest_local: RestLocal {
            translation,
            ..RestLocal::default()
        },
    }
}

/// Root, a tagged front-side chest leaf, and an untagged `hand_r` socket joint.
/// One clip with no joint tracks, so every sample holds the rest pose.
fn body_model() -> ModelHitZones {
    let skeleton = Skeleton {
        joints: vec![
            rest_joint(None, Vec3::ZERO),
            rest_joint(Some(0), CHEST),
            rest_joint(Some(0), HAND_R),
        ],
    };
    let clips = vec![AnimationClip {
        name: "idle".into(),
        duration: 1.0,
        joints: Vec::new(),
        travel_speed: None,
        events: Vec::new(),
    }];
    let joint_zones = vec![
        None,
        Some(JointZone {
            tag: "chest".to_string(),
            radius: Some(CHEST_RADIUS),
        }),
        None,
    ];
    let derived_bound = derive_bound(&skeleton, &clips, &joint_zones);
    assert!(derived_bound.is_some(), "the chest zone yields a bound");
    ModelHitZones {
        skeleton: Arc::new(skeleton),
        clips: Arc::new(clips),
        joint_zones,
        sockets: HashMap::from([("hand_r".to_string(), SocketBinding::SkinnedJoint(2))]),
        derived_bound,
        legs: Vec::new(),
        pose_stack: Arc::new(PoseModifierStack::default()),
    }
}

fn body_store() -> HitZoneStore {
    let mut store = HitZoneStore::new();
    store.models.insert("body".to_string(), body_model());
    store
}

/// An animated body carrying the pose inputs the presentation pass writes:
/// aim [`AIM`] and lower-body `heading`.
fn body_mesh(body_frame: BodyFrame, heading: f32) -> MeshComponent {
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
    let mut animation = MeshAnimation::new(states, "idle".into());
    animation.entered_at = Some(0.0);
    let mut mesh = MeshComponent::animated("body".into(), animation)
        .with_origin_offset(ORIGIN_OFFSET)
        .with_body_frame(body_frame);
    mesh.pose_inputs = Some(PoseInputs {
        aim_yaw: AIM,
        heading_yaw: heading,
        ..PoseInputs::default()
    });
    mesh
}

fn spawn_body(reg: &mut EntityRegistry, mesh: MeshComponent) -> EntityId {
    let id = reg.spawn(Transform {
        position: POSITION,
        rotation: Quat::from_rotation_y(AIM),
        scale: Vec3::ONE,
    });
    reg.set_component(
        id,
        HealthComponent {
            max: 100.0,
            current: 100.0,
            hitbox: None,
            death_handled: false,
            pending_kill_credit: None,
            zone_multipliers: HashMap::new(),
            contributor_ledger: Default::default(),
        },
    )
    .unwrap();
    reg.set_component(id, mesh).unwrap();
    id
}

/// World point of a model-space point placed at `rotation` on the spawned
/// transform.
fn placed(rotation: Quat, model_point: Vec3) -> Vec3 {
    Mat4::from_scale_rotation_translation(Vec3::ONE, rotation, POSITION + ORIGIN_OFFSET)
        .transform_point3(model_point)
}

/// A horizontal ray at `point`'s height that crosses the pawn perpendicular to
/// its aim, passing through `point`.
fn crossing_hit(reg: &EntityRegistry, store: &HitZoneStore, point: Vec3) -> Option<EntityRayHit> {
    let across = Quat::from_rotation_y(AIM) * Vec3::X;
    nearest_entity_hit(reg, store, 0.0, point - across * 5.0, across, 20.0, 0.0)
}

/// A downward ray through `point`.
fn vertical_hit(reg: &EntityRegistry, store: &HitZoneStore, point: Vec3) -> Option<EntityRayHit> {
    nearest_entity_hit(
        reg,
        store,
        0.0,
        point + Vec3::Y * 5.0,
        Vec3::NEG_Y,
        20.0,
        0.0,
    )
}

// Acceptance (body-space seams): a player body's front-side zone is hit by a ray
// crossing in front of the pawn and missed by the same ray crossing behind it.
#[test]
fn player_body_front_zone_is_hit_in_front_of_the_pawn_and_missed_behind() {
    let mut reg = EntityRegistry::new();
    let store = body_store();
    spawn_body(&mut reg, body_mesh(BodyFrame::View, AIM));

    let forward = Quat::from_rotation_y(AIM) * Vec3::NEG_Z;
    let chest_height = POSITION + ORIGIN_OFFSET + Vec3::Y * CHEST.y;
    let in_front = chest_height + forward * CHEST.z;
    let behind = chest_height - forward * CHEST.z;

    let hit = crossing_hit(&reg, &store, in_front).expect("a ray crossing in front hits");
    assert_eq!(hit.zone.as_deref(), Some("chest"));
    assert!(
        (hit.point - in_front).length() < CHEST_RADIUS + EPS,
        "the chest sits along the aim: hit {:?}, chest {in_front:?}",
        hit.point
    );
    assert!(
        crossing_hit(&reg, &store, behind).is_none(),
        "the same ray crossing behind the pawn misses the front-side zone"
    );
}

// Acceptance (body-space seams): with heading = aim, a posed `hand_r` socket on a
// player body lands where the body draws it at the current-tick transform.
#[test]
fn player_body_hand_socket_lands_on_the_drawn_socket_when_heading_is_aim() {
    let mut reg = EntityRegistry::new();
    let store = body_store();
    let id = spawn_body(&mut reg, body_mesh(BodyFrame::View, AIM));

    let socket = store
        .posed_socket_world(&reg, id, "hand_r", 0.0)
        .expect("hand_r resolves");
    let mesh = reg.get_component::<MeshComponent>(id).unwrap();
    let transform = reg.get_component::<Transform>(id).unwrap();
    let drawn = Mat4::from_scale_rotation_translation(
        transform.scale,
        mesh.presentation_rotation(transform.rotation),
        transform.position + mesh.origin_offset,
    )
    .transform_point3(HAND_R);
    assert!(
        (socket - drawn).length() < EPS,
        "socket {socket:?} must land on the drawn hand {drawn:?}"
    );

    // The turn is what puts it there: the unturned placement is across the body.
    let unturned = placed(Quat::from_rotation_y(AIM), HAND_R);
    assert!((socket - unturned).length() > 0.5);
    // And the drawn right hand is on the pawn's right of its view forward.
    let right = Quat::from_rotation_y(AIM) * Vec3::X;
    assert!((socket - (POSITION + ORIGIN_OFFSET)).dot(right) > 0.3);
}

// Acceptance (body-space seams, pin R2): with lower-body heading φ ≠ aim θ, the
// precise zone and the socket anchor sit on the transform plus the turn (aim θ),
// never on the heading the body is drawn at.
#[test]
fn player_body_zones_and_sockets_follow_aim_not_lower_body_heading() {
    let heading = AIM + 1.2;
    let mut reg = EntityRegistry::new();
    let store = body_store();
    let id = spawn_body(&mut reg, body_mesh(BodyFrame::View, heading));

    let by_aim = BodyFrame::View.model_rotation(Quat::from_rotation_y(AIM));
    let by_heading = BodyFrame::View.model_rotation(Quat::from_rotation_y(heading));
    let mesh = reg.get_component::<MeshComponent>(id).unwrap();
    assert!(
        mesh.presentation_rotation(Quat::from_rotation_y(AIM))
            .abs_diff_eq(by_heading, EPS),
        "the fixture draws the body at the heading"
    );

    // Socket anchor: on the aim, not on the drawn heading.
    let socket = store
        .posed_socket_world(&reg, id, "hand_r", 0.0)
        .expect("hand_r resolves");
    let aim_hand = placed(by_aim, HAND_R);
    let drawn_hand = placed(by_heading, HAND_R);
    assert!(
        (socket - aim_hand).length() < EPS,
        "socket {socket:?} sits on aim + turn {aim_hand:?}"
    );
    assert!((socket - drawn_hand).length() > 0.3);

    // Hit zone: a ray down through the aim-placed chest hits; one down through
    // the heading-placed chest, where the body is drawn, misses.
    let aim_chest = placed(by_aim, CHEST);
    let drawn_chest = placed(by_heading, CHEST);
    let hit = vertical_hit(&reg, &store, aim_chest).expect("the aim-placed chest is hit");
    assert_eq!(hit.zone.as_deref(), Some("chest"));
    assert!(
        vertical_hit(&reg, &store, drawn_chest).is_none(),
        "the presentation heading never places a hit zone"
    );
}

// Regression guard: a model-frame body (an enemy) keeps today's placement. The
// turn is never applied to it, whatever its pose inputs carry.
#[test]
fn model_frame_body_zones_and_sockets_take_no_turn() {
    let transform = Transform {
        position: POSITION,
        rotation: Quat::from_rotation_y(AIM),
        scale: Vec3::new(1.0, 1.5, 1.0),
    };
    assert_eq!(
        model_matrix(&transform, ORIGIN_OFFSET, BodyFrame::Model),
        Some(Mat4::from_scale_rotation_translation(
            transform.scale,
            transform.rotation,
            transform.position + ORIGIN_OFFSET,
        )),
        "the model frame composes the transform rotation unchanged"
    );

    let mut reg = EntityRegistry::new();
    let store = body_store();
    let id = spawn_body(&mut reg, body_mesh(BodyFrame::Model, AIM + 1.2));
    let unturned = Quat::from_rotation_y(AIM);

    let socket = store
        .posed_socket_world(&reg, id, "hand_r", 0.0)
        .expect("hand_r resolves");
    assert!((socket - placed(unturned, HAND_R)).length() < EPS);

    // The chest faces the transform's own +Z: the ray crossing there hits it.
    let model_front = unturned * Vec3::Z;
    let chest_height = POSITION + ORIGIN_OFFSET + Vec3::Y * CHEST.y;
    let hit = crossing_hit(&reg, &store, chest_height + model_front * CHEST.z)
        .expect("the unturned front-side chest is hit");
    assert_eq!(hit.zone.as_deref(), Some("chest"));
    assert!(crossing_hit(&reg, &store, chest_height - model_front * CHEST.z).is_none());
}
