// Ghosts are never interaction targets: hit, splash, trigger and AI queries run
// against a presenting ghost's mapped pose and find nothing there.
// See: context/lib/entity_model.md §7d

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use glam::Vec3;
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::kinematic_mover::MoverCommand;
use postretro_entities::components::trigger_volume::{
    TriggerActivation, TriggerFireMode, TriggerVolumeComponent,
};
use postretro_entities::data_descriptors::{
    BehaviorActivityDescriptor, BehaviorGraphDescriptor, BehaviorGraphEnvelope, MotionVerb,
};
use postretro_entities::provenance::DescriptorSpawnPath;
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::ir::{IrNode, IrValue};
use postretro_foundation::{
    CANDIDATE_DISTANCE_INPUT, FACTION_STATE_FIELD, HealthDescriptor, HitboxDescriptor,
    SplashDescriptor,
};
use postretro_physics::collision::CollisionWorld;
use postretro_sim::ai_host::{AiTickInputs, SimAiHost};
use postretro_sim::scripting_systems::hit_zones::HitZoneStore;
use postretro_sim::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use postretro_sim::trigger_system::{AuthoritativePlayer, PlayerId, TriggerSystem};

use super::{INSIDE, Scene};
use crate::ghost_links::opt_ins_test_fixtures::descriptor;

/// Where the link carries `INSIDE`: the presenting ghost's mapped position.
fn mapped(scene: &Scene, entity: EntityId) -> Vec3 {
    let ghost = scene.ghost(entity).expect("the member presents a ghost");
    assert_eq!(ghost.membership, 1.0, "a fully presenting ghost");
    ghost.mapped.position
}

fn hittable(registry: &mut EntityRegistry, id: EntityId) {
    registry
        .set_component(
            id,
            HealthComponent::from_descriptor(&HealthDescriptor {
                max: 100.0,
                hitbox: Some(HitboxDescriptor {
                    half_extents: [0.5, 1.0, 0.5],
                    offset: None,
                }),
                zone_multipliers: HashMap::new(),
            }),
        )
        .unwrap();
}

fn health(registry: &EntityRegistry, id: EntityId) -> f32 {
    registry
        .get_component::<HealthComponent>(id)
        .unwrap()
        .current
}

/// A ghost-opted monster presenting in full through the armed link.
fn presenting_monster() -> (Scene, EntityId, Vec3) {
    let mut scene = Scene::new(vec![descriptor("phase_wraith", true)]);
    let wraith = scene.entity("phase_wraith", INSIDE, DescriptorSpawnPath::MapPlacement);
    hittable(&mut scene.registry, wraith);
    scene.frame();
    let ghost_at = mapped(&scene, wraith);
    assert!(ghost_at.distance(INSIDE) > 50.0, "the copies are far apart");
    (scene, wraith, ghost_at)
}

/// Fire a zero-radius shot through `target` from 3 m away along +z, on the
/// entity ray query hitscan resolves through, and return the entity it struck.
fn shot_through(
    registry: EntityRegistry,
    shooter: EntityId,
    target: Vec3,
) -> (EntityRegistry, Option<EntityId>) {
    let registry = Rc::new(RefCell::new(registry));
    let shot = {
        let mut registry = registry.borrow_mut();
        let id = registry.spawn(Transform {
            position: target - Vec3::Z * 3.0,
            ..Default::default()
        });
        registry
            .set_component(
                id,
                postretro_entities::components::projectile::ProjectileComponent {
                    source_sounds: None,
                    predicted_visible: true,
                    source_action: None,
                    source_shot: None,
                    direction: Vec3::Z.to_array(),
                    speed: 600.0,
                    radius: 0.0,
                    remaining_range: 6.0,
                    remaining_lifetime: 1.0,
                    damage: 10.0,
                    knockback_impulse: [0.0; 3],
                    credit_source: "probe".into(),
                    owner_pawn: shooter,
                    owner_weapon: shooter,
                    spawned: false,
                    predicted_shot_id: None,
                    elapsed_flight_age: 0.0,
                    flipbook_active: false,
                    impact_light: None,
                    splash: None,
                    source_weapon: None,
                    activation: None,
                },
            )
            .unwrap();
        id
    };
    let contacts = postretro_sim::sim::advance(
        &registry,
        &CollisionWorld::default(),
        &HitZoneStore::default(),
        0.0,
        1.0 / 60.0,
        &mut |_| {},
    );
    let struck = contacts
        .iter()
        .find(|contact| contact.projectile == shot)
        .and_then(|contact| contact.target);
    let registry = Rc::try_unwrap(registry)
        .ok()
        .expect("the shot released the registry")
        .into_inner();
    (registry, struck)
}

// Acceptance (AC22): a shot along the ghost's mapped position hits nothing,
// while the same shot through the real monster hits it. Hitscan and projectile
// flight share this entity ray query; the ghost is no registry entity.
#[test]
fn a_shot_through_a_ghost_hits_nothing_while_the_real_member_is_hit() {
    let (mut scene, wraith, ghost_at) = presenting_monster();
    let shooter = scene.viewer;

    let registry = std::mem::replace(&mut scene.registry, EntityRegistry::new());
    let (registry, struck) = shot_through(registry, shooter, ghost_at);
    assert_eq!(struck, None, "nothing stands at the mapped pose");
    let (registry, struck) = shot_through(registry, shooter, INSIDE);
    assert_eq!(
        struck,
        Some(wraith),
        "control: the query sees the real member"
    );
    scene.registry = registry;
    scene.frame();
    assert!(scene.ghost(wraith).is_some());
    assert_eq!(
        scene
            .registry
            .iter_with_kind(postretro_entities::ComponentKind::Transform)
            .count(),
        3,
        "the link, the viewer and the monster: a presenting ghost is no registry entity"
    );
}

// Acceptance (AC22): a splash centred on the ghost damages nothing through it.
#[test]
fn a_splash_on_a_ghost_damages_nothing_while_one_on_the_real_member_does() {
    let (mut scene, wraith, ghost_at) = presenting_monster();
    let shooter = scene.viewer;
    let splash = SplashDescriptor {
        radius: 3.0,
        min_fraction: 1.0,
        self_damage: false,
        knockback: None,
    };
    let blast = |registry: &mut EntityRegistry, center: Vec3| {
        postretro_sim::sim::splash::emit_splash_damage(
            registry,
            &HitZoneStore::default(),
            &CollisionWorld::default(),
            center,
            center,
            &splash,
            25.0,
            shooter,
            shooter,
            "probe".into(),
            &mut |_| {},
        )
    };
    assert!(
        !blast(&mut scene.registry, ghost_at),
        "no target at the ghost"
    );
    assert_eq!(health(&scene.registry, wraith), 100.0);
    assert!(
        blast(&mut scene.registry, INSIDE),
        "control: the real member is hit"
    );
    assert!(health(&scene.registry, wraith) < 100.0);
}

// Acceptance (AC22): a trigger volume at a ghost pawn's mapped position does
// not fire, while one at the partner's real position does.
#[test]
fn a_trigger_at_a_ghost_pawn_does_not_fire_while_one_at_the_real_pawn_does() {
    let mut scene = Scene::new(vec![descriptor("player", true)]);
    let partner = scene.pawn(INSIDE, DescriptorSpawnPath::NetworkSlot);
    scene.frame();
    let ghost_at = mapped(&scene, partner);

    let fires_at = |registry: &mut EntityRegistry, center: Vec3| {
        let trigger = registry.spawn(Transform {
            position: center,
            ..Default::default()
        });
        registry
            .set_component(
                trigger,
                TriggerVolumeComponent::new(
                    TriggerActivation::Touch,
                    "probe".into(),
                    "probe.fire".into(),
                    String::new(),
                    MoverCommand::Start,
                    TriggerFireMode::Multiple,
                    0.0,
                    true,
                ),
            )
            .unwrap();
        let mut bridge = TriggerVolumeBridge::new();
        bridge.insert_for_test(
            trigger,
            center - Vec3::splat(2.0),
            center + Vec3::splat(2.0),
        );
        let report = TriggerSystem::default().run_authoritative_tick(
            registry,
            &bridge,
            &[AuthoritativePlayer {
                id: PlayerId::Remote(7),
                pawn: partner,
            }],
            &HashMap::new(),
            1.0 / 60.0,
        );
        let _ = registry.despawn(trigger);
        !report.fires.is_empty()
    };
    assert!(
        !fires_at(&mut scene.registry, ghost_at),
        "the ghost touches nothing"
    );
    assert!(
        fires_at(&mut scene.registry, INSIDE),
        "control: the real pawn fires it"
    );
}

/// An enemy that acquires any hostile candidate within 4 m.
fn enemy(registry: &mut EntityRegistry, at: Vec3) -> EntityId {
    let graph = BehaviorGraphDescriptor {
        knockback: Default::default(),
        envelope: BehaviorGraphEnvelope {
            initial: "watch".into(),
            activities: BTreeMap::from([(
                "watch".into(),
                BehaviorActivityDescriptor {
                    sound: None,
                    animation: None,
                    // An engaging activity keeps its acquired target; speed 0
                    // keeps the enemy where it stands.
                    motion: Some(MotionVerb::ChaseTarget),
                    action: None,
                    on_enter: None,
                    layers: BTreeMap::new(),
                },
            )]),
            transitions: BTreeMap::new(),
        },
        candidate_filter: Some(IrNode::Le {
            a: Box::new(IrNode::Input {
                name: CANDIDATE_DISTANCE_INPUT.to_string(),
                owner: None,
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(4.0),
            }),
        }),
        retaliation: None,
        patrol: None,
        attacks: Default::default(),
        engagement_radius: None,
        move_speed: 0.0,
    };
    let id = registry.spawn(Transform {
        position: at,
        ..Default::default()
    });
    let mut brain = BrainComponent::from_graph(&graph);
    brain.home_anchor = at;
    assert!(brain.enter_activity_at(0, 0));
    registry.set_component(id, brain).unwrap();
    registry
        .set_component(
            id,
            postretro_entities::components::mesh::MeshComponent::stateless(
                "models/wraith.glb".into(),
            ),
        )
        .unwrap();
    registry
        .entity_state_mut(id)
        .expect("a brain carries entity state")
        .set(
            FACTION_STATE_FIELD,
            postretro_entities::DEFAULT_ENEMY_FACTION_INDEX,
        );
    registry
        .set_component(
            id,
            postretro_entities::components::agent::AgentComponent::new(0.35, 1.8, 0.4, 0.0),
        )
        .unwrap();
    hittable(registry, id);
    id
}

fn acquired_after_ticks(registry: &mut EntityRegistry, enemy: EntityId) -> Option<EntityId> {
    let mut runtime = postretro_ai::AiRuntime::new();
    let factions = postretro_entities::FactionRegistry::default();
    let sentiment = RefCell::new(postretro_entities::FactionSentimentState::default());
    let mut callback = |_: &mut EntityRegistry| {};
    let mut host = SimAiHost::new(&mut callback);
    for _ in 0..4 {
        let _ = postretro_ai::run_ai_tick_with_host(
            registry,
            &mut runtime,
            1.0 / 60.0,
            AiTickInputs {
                nav_graph: None,
                collision_world: None,
                descriptors: &[],
                descriptor_generation: 0,
                factions: &factions,
                faction_sentiment: &sentiment,
            },
            &mut host,
        );
    }
    registry
        .get_component::<BrainComponent>(enemy)
        .unwrap()
        .acquired_target
}

// Acceptance (AC22): an enemy facing a ghost pawn's mapped position acquires
// nothing, while the same enemy beside the real pawn acquires it.
#[test]
fn an_enemy_facing_a_ghost_pawn_acquires_nothing_while_the_real_pawn_is_acquired() {
    let mut scene = Scene::new(vec![descriptor("player", true)]);
    let partner = scene.pawn(INSIDE, DescriptorSpawnPath::NetworkSlot);
    hittable(&mut scene.registry, partner);
    scene.frame();
    let ghost_at = mapped(&scene, partner);
    // The viewer stands far from both enemies, out of their reach.
    let watcher = enemy(&mut scene.registry, ghost_at + Vec3::Z * 2.0);
    assert_eq!(acquired_after_ticks(&mut scene.registry, watcher), None);

    let control = enemy(&mut scene.registry, INSIDE + Vec3::Z * 2.0);
    assert_eq!(
        acquired_after_ticks(&mut scene.registry, control),
        Some(partner),
        "control: the same enemy acquires the real pawn"
    );
}
