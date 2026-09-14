//! Cross-stage remote FIRE/HIT coverage owned by netcode.
//!
//! The ingester names sim-owned collision types, so these tests live above sim
//! and exercise one type identity through production FIRE, HIT readiness,
//! ingestion, and fixed-tick AI ordering.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

use glam::{Vec2, Vec3};
use postretro_combat_model::{OpenAuthorizedShot, ShotId};
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::health::{HealthComponent, Hitbox};
use postretro_entities::components::player_movement::PlayerMovementComponent;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::provenance::{
    DescriptorComponentKind, DescriptorProvenance, DescriptorSpawnPath,
};
use postretro_entities::{
    DEFAULT_ENEMY_FACTION_INDEX, EntityId, EntityRegistry, EntityTypeDescriptor, FactionRegistry,
    FactionSentimentState, Transform,
};
use postretro_foundation::{
    BehaviorActivityDescriptor, BehaviorGraphDescriptor, BehaviorGraphEnvelope, FireMode,
    MotionVerb, PlacementOffset, PlacementRotation, ProjectileBodyVisual, ProjectileDescriptor,
    ProjectileVisual, ResolutionMode, RetaliationDescriptor, SplashDescriptor, WeaponDescriptor,
    WeaponPlacementDescriptor,
};
use postretro_net::wire::{self, ClientMessage, HitDeclaration, HitRecord};
use postretro_scripting_core::data_descriptors::{
    AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementDescriptor, SpeedParams,
};
use postretro_scripting_core::reaction_dispatch::ProgressTracker;

use crate::collision::CollisionWorld;
use crate::kinematic_mover::MoverTickStateTable;
use crate::movement::MovementInput;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::sim::touch::TouchSystem;
use crate::sim::{
    PostMovementCommand, RemotePawnCommand, SimCommand, TickEvents,
    simulate_tick_with_presentation_aim,
};
use crate::weapon::FireButtonState;
use crate::{
    HIT_RANGE_TOLERANCE, HostCommandQueues, MovementOwners, NetworkIdAllocator,
    OpenAuthorizedShots, PendingHitDeclarations, host_take_ready_hit_declarations,
    ingest_hit_declaration_for_test,
};
use postretro_ai::AiRuntime;

const CLIENT_ID: u64 = 7;
const TICK_DT: f32 = 1.0 / 60.0;

struct HostSimulation {
    registry: Rc<RefCell<EntityRegistry>>,
    world: CollisionWorld,
    hit_zones: HitZoneStore,
    descriptors: Vec<EntityTypeDescriptor>,
    progress: ProgressTracker,
    ai: AiRuntime,
    movers: MoverTickStateTable,
    touch: TouchSystem,
    factions: FactionRegistry,
    sentiment: RefCell<FactionSentimentState>,
}

impl HostSimulation {
    fn new(
        registry: Rc<RefCell<EntityRegistry>>,
        world: CollisionWorld,
        descriptors: Vec<EntityTypeDescriptor>,
    ) -> Self {
        Self {
            registry,
            world,
            hit_zones: HitZoneStore::new(),
            descriptors,
            progress: ProgressTracker::new(),
            ai: AiRuntime::new(),
            movers: MoverTickStateTable::default(),
            touch: TouchSystem::default(),
            factions: FactionRegistry::default(),
            sentiment: RefCell::new(FactionSentimentState::default()),
        }
    }

    fn tick(
        &mut self,
        remote: &[RemotePawnCommand],
        ingest: impl FnMut(&mut EntityRegistry, &mut dyn FnMut(&mut EntityRegistry)),
    ) -> TickEvents {
        let no_edges = HashMap::new();
        simulate_tick_with_presentation_aim(
            self.registry.clone(),
            &self.world,
            &self.hit_zones,
            None,
            0.0,
            false,
            0.0,
            (0.0, 0.0),
            &mut self.progress,
            postretro_ai::tick_runner!(&mut self.ai),
            &[],
            &mut self.movers,
            remote,
            &neutral_command(),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            TICK_DT,
            &mut self.touch,
            &self.descriptors,
            0,
            &self.factions,
            &self.sentiment,
            None,
            &no_edges,
            &no_edges,
            None,
            ingest,
            |_| {},
        )
    }
}

fn neutral_command() -> SimCommand {
    SimCommand {
        movement: MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            use_pressed: false,
            drop_pressed: false,
        },
        fire_button: FireButtonState {
            pressed: false,
            active: false,
        },
        reload: false,
        firing_slot: 0,
        select_slot: None,
        use_pressed: false,
        drop_pressed: false,
    }
}

fn remote_fire(pawn: EntityId, weapon: EntityId, shot_id: ShotId) -> RemotePawnCommand {
    RemotePawnCommand {
        pawn,
        owner_client_id: CLIENT_ID,
        weapon: Some(weapon),
        shot_id: Some(shot_id),
        fire_tick: 10,
        client_tick: shot_id.client_tick(),
        aim_pitch: 0.0,
        command: SimCommand {
            fire_button: FireButtonState {
                pressed: true,
                active: true,
            },
            ..neutral_command()
        },
    }
}

fn movement() -> PlayerMovementComponent {
    PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
        knockback: Default::default(),
        capsule: CapsuleParams {
            radius: 0.4,
            half_height: 0.8,
            eye_height: 0.5,
        },
        ground: GroundParams {
            speed: SpeedParams {
                walk: 7.0,
                run: 11.0,
                crouch: 3.0,
            },
            accel: 10.0,
            step_height: 0.3,
            max_slope: 45.0,
        },
        air: AirParams {
            forward_steer: 0.0,
            accel: 0.7,
            max_control_speed: 0.5,
            bunny_hop: false,
            jumps: 0,
            jump_velocity: 5.5,
            jump_ceiling: 0.0,
        },
        fall: FallParams {
            terminal_velocity: 40.0,
        },
        stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
        stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
        dash: None,
        forgiveness: None,
        crouch: None,
        slide: None,
        view_feel: None,
    })
}

fn health(hitbox: Option<Hitbox>) -> HealthComponent {
    HealthComponent {
        max: 100.0,
        current: 100.0,
        hitbox,
        death_handled: false,
        pending_kill_credit: None,
        zone_multipliers: Default::default(),
        contributor_ledger: Default::default(),
    }
}

fn target(registry: &mut EntityRegistry, position: Vec3, half_extents: Vec3) -> EntityId {
    let id = registry.spawn(Transform {
        position,
        ..Transform::default()
    });
    registry
        .set_component(
            id,
            health(Some(Hitbox {
                half_extents,
                offset: Vec3::ZERO,
            })),
        )
        .unwrap();
    id
}

fn projectile_weapon(source: &str) -> WeaponComponent {
    WeaponComponent::from_descriptor(&WeaponDescriptor {
        knockback: None,
        damage: 10.0,
        pellet_count: 1,
        spread_degrees: 0.0,
        bloom_per_shot_degrees: 0.0,
        bloom_max_degrees: 0.0,
        bloom_decay_degrees_per_second: 0.0,
        bloom_decay_delay_ms: 0.0,
        movement_spread_degrees: 0.0,
        spread_vertical_bias: 0.0,
        range: 100.0,
        cooldown_ms: 100.0,
        fire_mode: FireMode::Semi,
        resolution: ResolutionMode::Projectile,
        projectile: Some(ProjectileDescriptor {
            speed: 1.0,
            radius: 0.0,
            lifetime_ms: 5_000.0,
            visual: ProjectileVisual {
                body: ProjectileBodyVisual::Sprite {
                    sprite: "sprites/projectiles/test.png".into(),
                    size: 0.25,
                    opacity: 1.0,
                    rotation: 0.0,
                    tint: [1.0; 3],
                    emissive: 0.0,
                    frame_duration_ms: None,
                },
                trail: None,
                light: None,
                impact_light: None,
            },
        }),
        splash: None,
        credit_source: Some(source.into()),
        third_person_model: None,
        viewmodel: None,
        placement: None,
        muzzle_offset: None,
        resource: None,
        lower_ms: 0,
        raise_ms: 0,
        block_during_reload: None,
    })
}

fn descriptor(name: &str, placement: WeaponPlacementDescriptor) -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        faction: None,
        tolerance: None,
        canonical_name: Some(name.into()),
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: Some(WeaponDescriptor {
            placement: Some(placement),
            ..projectile_descriptor()
        }),
        touchable: None,
        mesh: None,
        health: None,
        behavior: None,
    }
}

fn projectile_descriptor() -> WeaponDescriptor {
    WeaponDescriptor {
        knockback: None,
        damage: 10.0,
        pellet_count: 1,
        spread_degrees: 0.0,
        bloom_per_shot_degrees: 0.0,
        bloom_max_degrees: 0.0,
        bloom_decay_degrees_per_second: 0.0,
        bloom_decay_delay_ms: 0.0,
        movement_spread_degrees: 0.0,
        spread_vertical_bias: 0.0,
        range: 2.0,
        cooldown_ms: 100.0,
        fire_mode: FireMode::Semi,
        resolution: ResolutionMode::Projectile,
        projectile: None,
        splash: None,
        credit_source: None,
        third_person_model: None,
        viewmodel: None,
        placement: None,
        muzzle_offset: None,
        resource: None,
        lower_ms: 0,
        raise_ms: 0,
        block_during_reload: None,
    }
}

fn provenance(name: &str) -> DescriptorProvenance {
    DescriptorProvenance {
        canonical_name: name.into(),
        owned_components: BTreeSet::from([DescriptorComponentKind::Weapon]),
        map_overrides: BTreeSet::new(),
        spawn_path: DescriptorSpawnPath::DefaultWeapon,
    }
}

fn wall(z: f32) -> CollisionWorld {
    CollisionWorld::from_triangles_for_test(
        vec![
            Vec3::new(-1.0, -1.0, z),
            Vec3::new(1.0, -1.0, z),
            Vec3::new(1.0, 1.0, z),
            Vec3::new(-1.0, 1.0, z),
        ],
        vec![[0, 1, 2], [0, 2, 3]],
    )
}

fn authorization(events: &TickEvents) -> OpenAuthorizedShot {
    let [shot] = events.authorized_shots.as_slice() else {
        panic!("production remote FIRE must mint one authorization");
    };
    shot.clone()
}

fn delivered(declaration: HitDeclaration) -> HitDeclaration {
    let ClientMessage::HitDeclaration(result) =
        wire::decode(&wire::encode(&ClientMessage::HitDeclaration(declaration))).unwrap()
    else {
        panic!("wire retains HIT message variant");
    };
    result
}

fn ingest(
    registry: &mut EntityRegistry,
    world: &CollisionWorld,
    allocator: &NetworkIdAllocator,
    owners: &MovementOwners,
    open: &mut OpenAuthorizedShots,
    declaration: &HitDeclaration,
) -> (bool, bool) {
    ingest_hit_declaration_for_test(
        registry,
        world,
        &HitZoneStore::new(),
        allocator,
        owners,
        open,
        CLIENT_ID,
        declaration,
    )
}

fn opened(shot: OpenAuthorizedShot) -> OpenAuthorizedShots {
    let mut open = OpenAuthorizedShots::new();
    open.record(shot.shot, shot.owner_client_id);
    open
}

#[test]
fn connected_obstructed_muzzle_declaration_replays_host_splash_from_eye() {
    let name = "weapon.test.obstructed";
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (pawn, weapon, splash_target) = {
        let mut r = registry.borrow_mut();
        let pawn = r.spawn(Transform::default());
        r.set_component(pawn, movement()).unwrap();
        let weapon = r.spawn(Transform::default());
        let mut component = projectile_weapon(name);
        component.range = 2.0;
        component.muzzle_offset = Some(Vec3::new(0.0, 0.0, -0.8));
        component.splash = Some(SplashDescriptor {
            knockback: None,
            radius: 1.0,
            min_fraction: 0.2,
            self_damage: false,
        });
        component.projectile.as_mut().unwrap().speed = 60.0;
        r.set_component(weapon, component).unwrap();
        r.set_component(weapon, provenance(name)).unwrap();
        let victim = target(&mut r, Vec3::new(0.4, 0.5, -0.2), Vec3::splat(0.1));
        (pawn, weapon, victim)
    };
    let mut allocator = NetworkIdAllocator::new();
    let shot_id = ShotId::from_parts(allocator.stamp(pawn), 9);
    let mut host = HostSimulation::new(
        registry.clone(),
        wall(-0.5),
        vec![descriptor(name, WeaponPlacementDescriptor::default())],
    );
    let events = host.tick(&[remote_fire(pawn, weapon, shot_id)], |_, _| {});
    let shot = authorization(&events);
    assert_eq!(shot.shot.fire_origin, Vec3::new(0.0, 0.5, 0.0));
    assert_eq!(
        events.remote_projectile_presentation_launches[0].origin,
        shot.shot.fire_origin
    );

    let mut owners = MovementOwners::new();
    owners.set(pawn, CLIENT_ID);
    let mut open = opened(shot);
    let declaration = delivered(HitDeclaration {
        shot_id: shot_id.raw(),
        records: vec![HitRecord {
            target: u32::MAX,
            point: Vec3::new(0.0, 0.5, -0.5).to_array(),
            zone: None,
        }],
    });
    assert_eq!(
        ingest(
            &mut registry.borrow_mut(),
            &host.world,
            &allocator,
            &owners,
            &mut open,
            &declaration,
        ),
        (true, true)
    );
    assert!(
        registry
            .borrow()
            .get_component::<HealthComponent>(splash_target)
            .unwrap()
            .current
            < 100.0
    );
}

#[test]
fn connected_lateral_muzzle_convergence_matches_host_splash_replay() {
    let name = "weapon.test.lateral";
    let placement = WeaponPlacementDescriptor {
        offset: PlacementOffset {
            right: 0.25,
            up: 0.0,
            forward: 0.0,
        },
        rotation: PlacementRotation::default(),
    };
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (pawn, weapon, direct_target, splash_target) = {
        let mut r = registry.borrow_mut();
        let pawn = r.spawn(Transform::default());
        r.set_component(pawn, movement()).unwrap();
        let weapon = r.spawn(Transform::default());
        let mut component = projectile_weapon(name);
        component.range = 6.0;
        component.muzzle_offset = Some(Vec3::new(1.0, 0.0, -0.5));
        component.splash = Some(SplashDescriptor {
            knockback: None,
            radius: 1.0,
            min_fraction: 1.0,
            self_damage: false,
        });
        component.projectile.as_mut().unwrap().speed = 360.0;
        r.set_component(weapon, component).unwrap();
        r.set_component(weapon, provenance(name)).unwrap();
        let direct = target(&mut r, Vec3::new(0.0, 0.5, -4.0), Vec3::splat(0.2));
        let splash = target(&mut r, Vec3::new(0.75, 0.5, -3.8), Vec3::splat(0.1));
        (pawn, weapon, direct, splash)
    };
    let mut allocator = NetworkIdAllocator::new();
    let shot_id = ShotId::from_parts(allocator.stamp(pawn), 9);
    let direct_network_id = allocator.stamp(direct_target);
    let mut host = HostSimulation::new(
        registry.clone(),
        CollisionWorld::new(),
        vec![descriptor(name, placement)],
    );
    let events = host.tick(&[remote_fire(pawn, weapon, shot_id)], |_, _| {});
    let shot = authorization(&events);
    let direction = shot.shot.projectile_direction.unwrap();
    assert!(direction.distance(Vec3::NEG_Z) > 0.1);
    assert_eq!(
        events.remote_projectile_presentation_launches[0].direction,
        direction
    );

    let mut owners = MovementOwners::new();
    owners.set(pawn, CLIENT_ID);
    let mut open = opened(shot);
    let declaration = delivered(HitDeclaration {
        shot_id: shot_id.raw(),
        records: vec![HitRecord {
            target: direct_network_id.0,
            point: Vec3::new(0.0, 0.5, -3.8).to_array(),
            zone: None,
        }],
    });
    assert_eq!(
        ingest(
            &mut registry.borrow_mut(),
            &host.world,
            &allocator,
            &owners,
            &mut open,
            &declaration,
        ),
        (true, true)
    );
    assert_eq!(
        registry
            .borrow()
            .get_component::<HealthComponent>(splash_target)
            .unwrap()
            .current,
        90.0
    );
}

#[test]
fn remote_projectile_contact_within_muzzle_range_validates() {
    let name = "weapon.test.range";
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (pawn, weapon, victim) = {
        let mut r = registry.borrow_mut();
        let pawn = r.spawn(Transform::default());
        r.set_component(pawn, movement()).unwrap();
        let weapon = r.spawn(Transform::default());
        let mut component = projectile_weapon(name);
        component.range = 2.0;
        component.muzzle_offset = Some(Vec3::new(0.0, 0.0, -1.0));
        r.set_component(weapon, component).unwrap();
        r.set_component(weapon, provenance(name)).unwrap();
        (
            pawn,
            weapon,
            target(&mut r, Vec3::new(0.0, 0.5, -3.49), Vec3::splat(0.5)),
        )
    };
    let mut allocator = NetworkIdAllocator::new();
    let shot_id = ShotId::from_parts(allocator.stamp(pawn), 9);
    let victim_network_id = allocator.stamp(victim);
    let mut host = HostSimulation::new(
        registry.clone(),
        CollisionWorld::new(),
        vec![descriptor(name, WeaponPlacementDescriptor::default())],
    );
    let shot = authorization(&host.tick(&[remote_fire(pawn, weapon, shot_id)], |_, _| {}));
    let max = shot.shot.range * HIT_RANGE_TOLERANCE;
    let contact = shot.shot.fire_origin + Vec3::NEG_Z * (max - 0.01);
    assert!(Vec3::new(0.0, 0.5, 0.0).distance(contact) > max);
    let mut owners = MovementOwners::new();
    owners.set(pawn, CLIENT_ID);
    let mut open = opened(shot);
    assert_eq!(
        ingest(
            &mut registry.borrow_mut(),
            &host.world,
            &allocator,
            &owners,
            &mut open,
            &HitDeclaration {
                shot_id: shot_id.raw(),
                records: vec![HitRecord {
                    target: victim_network_id.0,
                    point: contact.to_array(),
                    zone: None,
                }],
            },
        ),
        (true, true)
    );
}

#[test]
fn rejected_remote_projectile_fire_cannot_later_declare_plausible_damage() {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (pawn, weapon, victim) = {
        let mut r = registry.borrow_mut();
        let pawn = r.spawn(Transform::default());
        r.set_component(pawn, movement()).unwrap();
        let weapon = r.spawn(Transform::default());
        let mut component = projectile_weapon("weapon.test.rejected");
        component.cooldown_remaining_ms = 100.0;
        r.set_component(weapon, component).unwrap();
        (
            pawn,
            weapon,
            target(&mut r, Vec3::new(0.0, 0.5, -5.0), Vec3::splat(0.5)),
        )
    };
    let mut allocator = NetworkIdAllocator::new();
    let shot_id = ShotId::from_parts(allocator.stamp(pawn), 9);
    let victim_network_id = allocator.stamp(victim);
    let mut host = HostSimulation::new(registry.clone(), CollisionWorld::new(), Vec::new());
    let events = host.tick(&[remote_fire(pawn, weapon, shot_id)], |_, _| {});
    assert!(events.authorized_shots.is_empty());
    assert_eq!(events.rejected_remote_projectile_fires[0].shot_id, shot_id);
    let mut owners = MovementOwners::new();
    owners.set(pawn, CLIENT_ID);
    assert_eq!(
        ingest(
            &mut registry.borrow_mut(),
            &host.world,
            &allocator,
            &owners,
            &mut OpenAuthorizedShots::new(),
            &HitDeclaration {
                shot_id: shot_id.raw(),
                records: vec![HitRecord {
                    target: victim_network_id.0,
                    point: Vec3::new(0.0, 0.5, -5.0).to_array(),
                    zone: None,
                }],
            },
        ),
        (false, false)
    );
    assert_eq!(
        registry
            .borrow()
            .get_component::<HealthComponent>(victim)
            .unwrap()
            .current,
        100.0
    );
}

#[test]
fn connected_client_projectile_declares_later_and_host_applies_authorized_credit() {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (pawn, weapon, victim) = {
        let mut r = registry.borrow_mut();
        let pawn = r.spawn(Transform::default());
        r.set_component(pawn, movement()).unwrap();
        let weapon = r.spawn(Transform::default());
        r.set_component(weapon, projectile_weapon("weapon.test.projectile"))
            .unwrap();
        (
            pawn,
            weapon,
            target(&mut r, Vec3::new(0.0, 0.5, -2.0), Vec3::splat(0.5)),
        )
    };
    let mut allocator = NetworkIdAllocator::new();
    let shot_id = ShotId::from_parts(allocator.stamp(pawn), 9);
    let victim_network_id = allocator.stamp(victim);
    let mut host = HostSimulation::new(registry.clone(), CollisionWorld::new(), Vec::new());
    let shot = authorization(&host.tick(&[remote_fire(pawn, weapon, shot_id)], |_, _| {}));
    let mut owners = MovementOwners::new();
    owners.set(pawn, CLIENT_ID);
    let mut open = opened(shot);
    let declaration = delivered(HitDeclaration {
        shot_id: shot_id.raw(),
        records: vec![HitRecord {
            target: victim_network_id.0,
            point: Vec3::new(0.0, 0.5, -2.0).to_array(),
            zone: None,
        }],
    });
    assert_eq!(
        ingest(
            &mut registry.borrow_mut(),
            &host.world,
            &allocator,
            &owners,
            &mut open,
            &declaration,
        ),
        (true, true)
    );
    let r = registry.borrow();
    let health = r.get_component::<HealthComponent>(victim).unwrap();
    assert_eq!(health.current, 90.0);
    assert_eq!(
        health
            .contributor_ledger
            .recorded_damage_by_source("weapon.test.projectile"),
        Some(10.0)
    );
    let credit = health.contributor_ledger.entries().first().unwrap();
    assert_eq!(credit.last_attacker, Some(pawn));
    assert_eq!(credit.last_weapon, Some(weapon));
}

fn retaliation_graph() -> BehaviorGraphDescriptor {
    BehaviorGraphDescriptor {
        knockback: Default::default(),
        envelope: BehaviorGraphEnvelope {
            initial: "engaged".into(),
            activities: BTreeMap::from([(
                "engaged".into(),
                BehaviorActivityDescriptor {
                    animation: None,
                    motion: Some(MotionVerb::ChaseTarget),
                    action: None,
                    on_enter: None,
                    layers: BTreeMap::new(),
                },
            )]),
            transitions: BTreeMap::new(),
        },
        candidate_filter: None,
        retaliation: Some(RetaliationDescriptor::default()),
        patrol: None,
        attacks: BTreeMap::new(),
        engagement_radius: None,
        move_speed: 0.0,
    }
}

fn ai_pawn(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
    let id = registry.spawn(Transform {
        position,
        ..Transform::default()
    });
    registry.set_component(id, movement()).unwrap();
    registry.set_component(id, health(None)).unwrap();
    id
}

fn ai_enemy(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
    let id = registry.spawn(Transform {
        position,
        ..Transform::default()
    });
    let mut brain = BrainComponent::from_graph(&retaliation_graph());
    brain.home_anchor = position;
    registry.set_component(id, brain).unwrap();
    registry
        .entity_state_mut(id)
        .unwrap()
        .set("faction", DEFAULT_ENEMY_FACTION_INDEX);
    registry.set_component(id, health(None)).unwrap();
    id
}

// Regression: ready remote damage must land before this tick's AI candidate snapshot.
#[test]
fn ready_remote_hit_reaches_retaliation_selection_in_the_same_simulation_tick() {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (player, victim, attacker, weapon) = {
        let mut r = registry.borrow_mut();
        let player = ai_pawn(&mut r, Vec3::new(3.0, 0.0, 0.0));
        let victim = ai_enemy(&mut r, Vec3::ZERO);
        r.entity_state_mut(victim)
            .unwrap()
            .set("archetype_tolerance", 4.0);
        let attacker = ai_enemy(&mut r, Vec3::new(10.0, 0.0, 0.0));
        r.set_component(attacker, movement()).unwrap();
        let weapon = r.spawn(Transform::default());
        let mut component = projectile_weapon("test.crossfire.remote");
        component.resolution = ResolutionMode::Hitscan;
        component.projectile = None;
        component.damage = 8.0;
        component.range = 20.0;
        r.set_component(weapon, component).unwrap();
        (player, victim, attacker, weapon)
    };
    let mut allocator = NetworkIdAllocator::new();
    let shot_id = ShotId::from_parts(allocator.stamp(attacker), 11);
    let victim_network_id = allocator.stamp(victim);
    let mut owners = MovementOwners::new();
    owners.set(attacker, CLIENT_ID);
    let mut host = HostSimulation::new(registry.clone(), CollisionWorld::new(), Vec::new());
    host.tick(&[], |_, _| {});
    assert_eq!(
        registry
            .borrow()
            .get_component::<BrainComponent>(victim)
            .unwrap()
            .acquired_target,
        Some(player)
    );

    let shot = authorization(&host.tick(&[remote_fire(attacker, weapon, shot_id)], |_, _| {}));
    assert!(!shot.shot.is_projectile);
    let mut open = opened(shot);
    let mut pending = PendingHitDeclarations::new();
    pending.push(
        CLIENT_ID,
        HitDeclaration {
            shot_id: shot_id.raw(),
            records: vec![HitRecord {
                target: victim_network_id.0,
                point: Vec3::new(0.0, 0.5, 0.0).to_array(),
                zone: None,
            }],
        },
    );
    let mut ready =
        host_take_ready_hit_declarations(&HostCommandQueues::new(), &mut open, &mut pending, 11);
    assert_eq!(ready.len(), 1);
    let ingest_world = CollisionWorld::new();
    host.tick(&[], |registry, _| {
        let declaration = ready.pop().unwrap().declaration;
        assert_eq!(
            ingest(
                registry,
                &ingest_world,
                &allocator,
                &owners,
                &mut open,
                &declaration,
            ),
            (true, true)
        );
    });

    let r = registry.borrow();
    assert_eq!(
        r.get_component::<HealthComponent>(victim).unwrap().current,
        92.0
    );
    let brain = r.get_component::<BrainComponent>(victim).unwrap();
    assert_eq!(brain.acquired_target, Some(attacker));
    assert_eq!(brain.retaliation_acquired_target, Some(attacker));
}
