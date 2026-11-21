//! The host march of a connected client's splash shot: per-tick world
//! contacts against each tick's poses, the on-path credit rule, release, and
//! the march's lifetime. Rockets fly +X from the origin at one metre a host
//! tick; a door is a slab 0.2 thick across that line.
//! See: context/lib/networking.md §Combat authority (march and credit)

use super::*;
use std::cell::RefCell;
use std::rc::Rc;
use postretro_entities::components::agent::AgentComponent;
use crate::collision::test_movers::{DoorPoses, door_slab};
use glam::Quat;

const DOOR: u32 = 6;

struct Rig {
    f: HitIngestFixture,
    fire_tick: u32,
    tick: u32,
}

impl Rig {
    fn new(world: CollisionWorld, radius: f32) -> Self {
        Self::with(world, radius, |_| {})
    }

    /// A rig whose shot `edit` adjusts before FIRE opens its march.
    fn with(world: CollisionWorld, radius: f32, edit: impl FnOnce(&mut AuthorizedShot)) -> Self {
        let mut f = HitIngestFixture::new(world);
        f.configure_projectile_splash(radius);
        f.shot_mut().range = 30.0;
        edit(f.shot_mut());
        f.movers = vec![door_slab(DOOR, 0.1, 4.0, 3.0)];
        let fire_tick = f.shot_mut().fire_tick;
        f.arm_march();
        Self {
            f,
            fire_tick,
            tick: fire_tick,
        }
    }

    /// The door closed across the line at `x` (its near face at `x - 0.1`),
    /// or open.
    fn door(&mut self, x: Option<f32>) {
        self.f.poses = match x {
            Some(x) => DoorPoses::default().place(DOOR, Vec3::new(x, -2.0, 0.0), Quat::IDENTITY),
            None => DoorPoses::default(),
        };
    }

    fn declare(&mut self, records: Vec<wire::HitRecord>) {
        let declaration = self.f.declaration(records);
        let bound = self.f.ingest_result(7, &declaration);
        assert!(bound.deferred, "binds to the march");
    }

    /// One host tick; what released.
    fn step(&mut self) -> Option<HitDeclarationResult> {
        self.tick += 1;
        self.f
            .march_tick(self.tick)
            .pop()
            .map(|released| released.result)
    }

    /// Step until release; ticks since FIRE and what released.
    fn run(&mut self) -> (u32, HitDeclarationResult) {
        for _ in 0..200 {
            if let Some(result) = self.step() {
                return (self.tick - self.fire_tick, result);
            }
        }
        panic!("the march releases within its reach");
    }

    fn steps(&mut self, count: u32) {
        for _ in 0..count {
            assert!(self.step().is_none(), "nothing releases yet");
        }
    }

    fn target(&mut self, position: Vec3) -> EntityId {
        self.f.spawn_splash_target(position)
    }

    fn enemy_record(&mut self, enemy: EntityId, point: Vec3) -> wire::HitRecord {
        let net = self.f.allocator.stamp(enemy);
        wire::HitRecord {
            normal: [-1.0, 0.0, 0.0],
            target: net.0,
            point: point.to_array(),
            zone: None,
        }
    }

    fn health(&self, id: EntityId) -> f32 {
        self.f
            .registry
            .get_component::<HealthComponent>(id)
            .map_or(0.0, |health| health.current)
    }

    fn march(&self) -> Option<&crate::splash_march::SplashMarch> {
        self.f.open_shots.march(self.f.shot_id)
    }
}

fn at(x: f32) -> wire::HitRecord {
    world_contact(Vec3::new(x, 0.0, 0.0), Vec3::NEG_X)
}

fn point(result: &HitDeclarationResult) -> Vec3 {
    result.projectile_contact.expect("a blast")
}

fn assert_near(actual: Vec3, expected: Vec3) {
    assert!(
        actual.distance(expected) <= 1.0e-4,
        "{actual} vs {expected}"
    );
}

fn assert_refused(result: &HitDeclarationResult) {
    assert!(result.fire_accepted, "FIRE stands");
    assert!(!result.hit_accepted, "the hit is refused");
    assert_eq!(result.projectile_contact, None, "no blast");
    assert!(result.contacts.is_empty());
}

// ---- C3 ----

#[test]
fn client_rocket_at_a_door_before_a_wall_lands_at_the_door_on_the_tick_the_march_meets_it() {
    for declared in [4.9, 8.0] {
        let mut rig = Rig::new(wall_at_x(8.0), 0.0);
        rig.door(Some(5.0));
        rig.declare(vec![at(declared)]);
        let (ticks, result) = rig.run();
        assert_eq!(
            ticks, 5,
            "declared {declared}: the fifth step meets the door"
        );
        assert_near(point(&result), Vec3::new(4.9, 0.0, 0.0));
    }
}

// P1: no step on the FIRE tick; one step a tick after it, each against that
// tick's poses, as a host rocket on the same path.
#[test]
fn march_never_steps_on_the_fire_tick_and_meets_a_door_on_the_host_rockets_tick() {
    let mut rig = Rig::new(CollisionWorld::new(), 0.05);
    rig.f.march_tick(rig.fire_tick);
    assert_eq!(rig.march().unwrap().travelled, 0.0, "no step on FIRE");

    let rocket = host_rocket(&mut rig.f.registry, 0.05);
    let mut march_contact = None;
    let mut rocket_contact = None;
    for tick in 1..=8 {
        // The door slides across the path on tick 3.
        rig.door((tick >= 3).then_some(5.0));
        rig.tick += 1;
        rig.f.march_tick(rig.tick);
        if march_contact.is_none() && rig.march().unwrap().contact.is_some() {
            march_contact = Some(tick);
        }
        let registry = Rc::new(RefCell::new(std::mem::take(&mut rig.f.registry)));
        let world = crate::collision::moving::CombinedCollisionWorld::new(
            &rig.f.collision_world,
            &rig.f.movers,
            &rig.f.poses,
        );
        let contacts = crate::sim::advance(
            &registry,
            &world,
            &rig.f.hit_zone_store,
            0.0,
            1.0 / 60.0,
            &mut |_| {},
        );
        rig.f.registry = Rc::try_unwrap(registry).ok().unwrap().into_inner();
        if rocket_contact.is_none() && contacts.iter().any(|c| c.projectile == rocket) {
            rocket_contact = Some(tick);
        }
    }
    assert_eq!(march_contact, Some(5));
    assert_eq!(
        rocket_contact, march_contact,
        "one tick, one path, one door"
    );
}

/// A host rocket on the march's path: same origin, speed and radius.
fn host_rocket(registry: &mut EntityRegistry, radius: f32) -> EntityId {
    let owner_pawn = registry.spawn(Transform::default());
    let owner_weapon = registry.spawn(Transform::default());
    let projectile = registry.spawn(Transform::default());
    registry
        .set_component(
            projectile,
            postretro_entities::components::projectile::ProjectileComponent {
                source_sounds: None,
                predicted_visible: true,
                source_action: None,
                source_shot: None,
                knockback_impulse: [0.0; 3],
                direction: Vec3::X.to_array(),
                speed: 60.0,
                radius,
                remaining_range: 30.0,
                remaining_lifetime: 10.0,
                damage: 1.0,
                credit_source: "test.host-rocket".to_string(),
                owner_pawn,
                owner_life: 0,
                owner_weapon,
                attackerless: false,
                spawned: false,
                predicted_shot_id: None,
                elapsed_flight_age: 0.0,
                flipbook_active: false,
                impact_light: None,
                splash: None,
                source_weapon: None,
                activation: None,
                damage_type: None,
            },
        )
        .unwrap();
    projectile
}

// P22: a declared point at the launch point, or a rocket launched inside a
// door's hull, releases on the tick after FIRE, never on FIRE.
#[test]
fn launch_point_and_launched_inside_a_hull_release_on_the_tick_after_fire() {
    let mut rig = Rig::new(CollisionWorld::new(), 0.0);
    rig.declare(vec![at(0.0)]);
    assert!(rig.f.march_tick(rig.fire_tick).is_empty(), "never on FIRE");
    let (ticks, result) = rig.run();
    assert_eq!(ticks, 1);
    assert_near(point(&result), Vec3::ZERO);

    let mut rig = Rig::new(CollisionWorld::new(), 0.1);
    rig.door(Some(0.0));
    rig.declare(vec![at(3.0)]);
    let (ticks, result) = rig.run();
    assert_eq!(ticks, 1, "enclosed at launch");
    assert_near(point(&result), Vec3::ZERO);
}

// ---- C4 (P14) ----

#[test]
fn door_sliding_into_the_path_takes_the_blast_and_one_that_opens_passes_it_to_the_wall() {
    // The client saw the door a frame later (5.05) or earlier (4.8); either way
    // the host detonates on its own door.
    for declared in [5.05, 4.8] {
        let mut rig = Rig::new(wall_at_x(8.0), 0.0);
        rig.declare(vec![at(declared)]);
        rig.steps(2);
        rig.door(Some(5.0));
        let (_, result) = rig.run();
        let landed = point(&result);
        assert_near(landed, Vec3::new(4.9, 0.0, 0.0));
        assert!(
            (landed.x - declared).abs() <= 0.25,
            "within the prediction error"
        );
    }

    let mut rig = Rig::new(wall_at_x(8.0), 0.0);
    rig.door(Some(5.0));
    rig.declare(vec![at(8.0)]);
    rig.steps(2);
    rig.door(None);
    let (_, result) = rig.run();
    assert_near(point(&result), Vec3::new(8.0, 0.0, 0.0));
}

// ---- C5 (P4, P5, P16) ----

#[test]
fn declaration_before_or_after_the_march_reaches_a_door_releases_there_shielding_the_far_side() {
    for late in [false, true] {
        let mut rig = Rig::new(CollisionWorld::new(), 0.0);
        let near = rig.target(Vec3::new(4.3, 0.0, 0.5));
        let far = rig.target(Vec3::new(5.6, 0.0, 0.5));
        rig.door(Some(5.0));
        if !late {
            rig.declare(vec![at(4.9)]);
        } else {
            rig.steps(8);
            assert!(rig.march().unwrap().contact.is_some());
            rig.declare(vec![at(4.9)]);
        }
        let (ticks, result) = rig.run();
        assert_eq!(ticks, if late { 9 } else { 5 }, "late {late}");
        assert_near(point(&result), Vec3::new(4.9, 0.0, 0.0));
        assert!(rig.health(near) < 100.0, "late {late}");
        assert_eq!(rig.health(far), 100.0, "late {late}");
    }
}

#[test]
fn declaration_arriving_after_the_march_passed_its_point_detonates_there_on_arrival() {
    let mut rig = Rig::new(CollisionWorld::new(), 0.0);
    rig.steps(6);
    rig.declare(vec![at(3.0)]);
    let (ticks, result) = rig.run();
    assert_eq!(ticks, 7, "the arrival tick");
    assert_near(point(&result), Vec3::new(3.0, 0.0, 0.0));
}

// ---- C6 (P2, P20, P3, P6) ----

#[test]
fn door_closing_behind_the_march_is_never_the_blast_unless_the_client_declared_it() {
    // P2: the client declared the wall beyond.
    let mut rig = Rig::new(wall_at_x(8.0), 0.0);
    rig.declare(vec![at(8.0)]);
    rig.steps(6);
    rig.door(Some(5.0));
    let (_, result) = rig.run();
    assert_near(point(&result), Vec3::new(8.0, 0.0, 0.0));

    // P20: the client saw the door close in front of its rocket.
    let mut rig = Rig::new(wall_at_x(8.0), 0.0);
    let near = rig.target(Vec3::new(4.3, 0.0, 0.5));
    let far = rig.target(Vec3::new(5.6, 0.0, 0.5));
    rig.steps(6);
    rig.door(Some(5.0));
    rig.declare(vec![at(4.9)]);
    let (_, result) = rig.run();
    assert_near(point(&result), Vec3::new(4.9, 0.0, 0.0));
    assert!(rig.health(near) < 100.0, "the side it came from");
    assert_eq!(rig.health(far), 100.0, "the door shields the far side");
}

#[test]
fn door_the_rocket_struck_that_opens_before_release_still_takes_the_blast_with_release_occlusion() {
    let mut rig = Rig::new(CollisionWorld::new(), 0.0);
    let behind = rig.target(Vec3::new(6.0, 0.0, 0.5));
    rig.door(Some(5.0));
    rig.steps(5);
    assert!(rig.march().unwrap().contact.is_some(), "struck on tick 5");
    rig.door(None);
    rig.steps(2);
    rig.declare(vec![at(4.9)]);
    let (_, result) = rig.run();
    assert_near(point(&result), Vec3::new(4.9, 0.0, 0.0));
    assert!(
        rig.health(behind) < 100.0,
        "occlusion reads the release tick, with the door open"
    );
}

// ---- C7 (P11, P18) ----

#[test]
fn client_and_host_disagreeing_on_a_door_land_at_the_nearer_of_host_door_and_declared_point() {
    // Client predicts open and declares the wall; the host has it closed.
    let mut rig = Rig::new(wall_at_x(8.0), 0.0);
    rig.door(Some(5.0));
    rig.declare(vec![at(8.0)]);
    let (_, result) = rig.run();
    assert_near(point(&result), Vec3::new(4.9, 0.0, 0.0));

    // Reversed: the client declares the door; the host has it open.
    let mut rig = Rig::new(wall_at_x(8.0), 0.0);
    rig.declare(vec![at(4.9)]);
    let (_, result) = rig.run();
    assert_near(point(&result), Vec3::new(4.9, 0.0, 0.0));
}

#[test]
fn zero_radius_rocket_at_a_shared_surface_takes_the_hosts_contact_either_side() {
    for (door, surface) in [(false, 8.0), (true, 4.9)] {
        for declared in [surface, surface - 0.01] {
            let world = if door {
                CollisionWorld::new()
            } else {
                wall_at_x(8.0)
            };
            let mut rig = Rig::new(world, 0.0);
            if door {
                rig.door(Some(5.0));
            }
            let near = rig.target(Vec3::new(surface - 0.6, 0.0, 0.5));
            let far = rig.target(Vec3::new(surface + 0.7, 0.0, 0.5));
            rig.declare(vec![at(declared)]);
            let (_, result) = rig.run();
            let landed = point(&result);
            assert_near(landed, Vec3::new(surface, 0.0, 0.0));
            assert!(rig.health(near) < 100.0, "door {door} declared {declared}");
            assert_eq!(rig.health(far), 100.0, "door {door} declared {declared}");
        }
    }
}

// ---- C8 (P7, P8, P28, P27, P19) ----

#[test]
fn declared_enemy_behind_a_host_closed_door_takes_nothing() {
    let mut rig = Rig::new(CollisionWorld::new(), 0.0);
    let enemy = rig.target(Vec3::new(6.0, 0.0, 0.0));
    rig.door(Some(5.0));
    let record = rig.enemy_record(enemy, Vec3::new(5.9, 0.0, 0.0));
    rig.declare(vec![record]);
    let (_, result) = rig.run();
    assert_near(point(&result), Vec3::new(4.9, 0.0, 0.0));
    assert_eq!(rig.health(enemy), 100.0);
}

#[test]
fn declared_enemy_the_host_flight_passes_by_is_the_impact_target_with_its_push() {
    let mut rig = Rig::with(CollisionWorld::new(), 0.0, |shot| {
        shot.knockback = Some(postretro_foundation::KnockbackDescriptor {
            speed: 6.0,
            upward_bias: 0.0,
        });
    });
    // The enemy stands just off the host's line: the host flight passes it.
    let enemy = rig.target(Vec3::new(6.0, 0.0, 0.2));
    rig.f
        .registry
        .set_component(enemy, AgentComponent::new(0.3, 1.8, 0.3, 4.0))
        .unwrap();
    let record = rig.enemy_record(enemy, Vec3::new(5.9, 0.0, 0.15));
    rig.declare(vec![record]);
    let (_, result) = rig.run();
    assert_near(point(&result), Vec3::new(5.9, 0.0, 0.15));
    assert_eq!(
        result.contacts[0].hit,
        postretro_entities::ContactHit::Entity(enemy)
    );
    assert!(rig.health(enemy) < 100.0, "the blast");
    let agent = rig
        .f
        .registry
        .get_component::<AgentComponent>(enemy)
        .unwrap();
    assert!(
        agent.knockback_velocity.x > 0.0,
        "the direct push composes with splash"
    );
}

#[test]
fn declared_enemy_pressed_to_a_shared_wall_is_the_impact_target_and_the_wall_shields() {
    let mut rig = Rig::new(wall_at_x(8.0), 0.0);
    let enemy = rig.target(Vec3::new(7.85, 0.0, 0.0));
    let beyond = rig.target(Vec3::new(8.7, 0.0, 0.5));
    let record = rig.enemy_record(enemy, Vec3::new(7.75, 0.0, 0.0));
    rig.declare(vec![record]);
    let (_, result) = rig.run();
    assert_near(point(&result), Vec3::new(7.75, 0.0, 0.0));
    assert_eq!(
        result.contacts[0].hit,
        postretro_entities::ContactHit::Entity(enemy)
    );
    assert!(rig.health(enemy) < 100.0);
    assert_eq!(rig.health(beyond), 100.0);
}

#[test]
fn declared_enemy_killed_before_release_still_detonates_at_its_point_with_no_target() {
    let mut rig = Rig::new(wall_at_x(8.0), 0.0);
    let enemy = rig.target(Vec3::new(4.0, 0.0, 0.0));
    let record = rig.enemy_record(enemy, Vec3::new(3.9, 0.0, 0.0));
    rig.declare(vec![record]);
    rig.steps(2);
    let mut health = rig
        .f
        .registry
        .get_component::<HealthComponent>(enemy)
        .unwrap()
        .clone();
    health.current = 0.0;
    health.death_handled = true;
    rig.f.registry.set_component(enemy, health).unwrap();
    let (_, result) = rig.run();
    assert_near(point(&result), Vec3::new(3.9, 0.0, 0.0));
    assert_eq!(
        result.contacts[0].hit,
        postretro_entities::ContactHit::World
    );
    assert_eq!(
        result.contacts[0].normal,
        Vec3::NEG_X,
        "the declared normal"
    );
}

#[test]
fn march_out_of_reach_before_a_late_declaration_detonates_only_within_what_it_covered() {
    for (declared, lands) in [(6.0, true), (10.5, false)] {
        let mut rig = Rig::with(CollisionWorld::new(), 0.0, |shot| shot.range = 10.0);
        rig.steps(12);
        assert!(rig.march().unwrap().exhausted);
        rig.declare(vec![at(declared)]);
        let (_, result) = rig.run();
        if lands {
            assert_near(point(&result), Vec3::new(declared, 0.0, 0.0));
        } else {
            assert_refused(&result);
        }
    }
}

// ---- C9 (P17, P9, P10, P23, P29, P33) ----

#[test]
fn off_path_declaration_takes_the_hosts_own_contact_and_never_open_air() {
    let off_path = world_contact(Vec3::new(5.0, 0.0, 2.0), Vec3::NEG_X);
    let mut rig = Rig::new(wall_at_x(8.0), 0.0);
    rig.declare(vec![off_path.clone()]);
    let (_, result) = rig.run();
    assert_near(point(&result), Vec3::new(8.0, 0.0, 0.0));

    // P9: with no contact before range, nothing lands.
    let mut rig = Rig::new(CollisionWorld::new(), 0.0);
    rig.declare(vec![off_path]);
    let (_, result) = rig.run();
    assert_refused(&result);
}

#[test]
fn contact_in_the_final_segment_detonates_and_a_zero_lifetime_resolves_empty() {
    let mut rig = Rig::with(wall_at_x(5.4), 0.0, |shot| shot.range = 5.5);
    rig.declare(vec![at(5.4)]);
    let (_, result) = rig.run();
    assert_near(point(&result), Vec3::new(5.4, 0.0, 0.0));

    let mut rig = Rig::with(wall_at_x(5.4), 0.0, |shot| {
        shot.projectile_lifetime_seconds = Some(0.0);
    });
    rig.declare(vec![at(0.0)]);
    let (ticks, result) = rig.run();
    assert_eq!(ticks, 1);
    assert_near(point(&result), Vec3::ZERO);
    let mut rig = Rig::with(wall_at_x(5.4), 0.0, |shot| {
        shot.projectile_lifetime_seconds = Some(0.0);
    });
    rig.declare(vec![at(5.4)]);
    let (_, result) = rig.run();
    assert_refused(&result);
}

#[test]
fn only_the_first_valid_record_is_judged() {
    let mut rig = Rig::with(wall_at_x(8.0), 0.0, |shot| shot.pellet_count = 3);
    rig.declare(vec![
        world_contact(Vec3::new(f32::NAN, 0.0, 0.0), Vec3::NEG_X),
        world_contact(Vec3::new(5.0, 0.0, 2.0), Vec3::NEG_X),
        at(3.0),
    ]);
    let (_, result) = rig.run();
    assert_near(point(&result), Vec3::new(8.0, 0.0, 0.0));
}

#[test]
fn invalid_or_empty_declarations_set_off_nothing_even_after_a_contact() {
    let mut rig = Rig::with(wall_at_x(8.0), 0.0, |shot| shot.pellet_count = 2);
    rig.declare(vec![
        world_contact(Vec3::new(f32::INFINITY, 0.0, 0.0), Vec3::NEG_X),
        at(1_000.0),
    ]);
    assert_refused(&rig.run().1);

    let mut rig = Rig::new(wall_at_x(8.0), 0.0);
    rig.steps(9);
    assert!(rig.march().unwrap().contact.is_some());
    rig.declare(Vec::new());
    assert_refused(&rig.run().1);
}

#[test]
fn a_declaration_that_never_arrives_sets_off_nothing_and_its_march_expires() {
    let mut rig = Rig::new(wall_at_x(8.0), 0.0);
    let budget = rig.march().unwrap().shot.timeout_budget_ticks;
    rig.steps(20);
    assert!(rig.march().is_some(), "waits for its declaration");
    rig.f.open_shots.prune_stale(rig.fire_tick + budget + 1);
    assert!(rig.march().is_none());
}

// ---- C10 (P25, P26) ----

// P25: a non-late FIRE from a muzzle off the eye, at an enemy that moved: the
// client converged on the enemy it saw, on the eye line; the host's rebuild
// converged on the wall behind. The declared point lies within the muzzle's
// offset of the host path, so it counts, at short and full range.
#[test]
fn non_late_muzzle_offset_rocket_at_a_moved_enemy_lands_at_its_declared_point() {
    const MUZZLE: Vec3 = Vec3::new(0.0, 0.0, 0.4);
    const WALL: f32 = 20.0;
    for distance in [2.0, 18.0] {
        let mut rig = Rig::with(wall_at_x(WALL), 0.1, |shot| {
            shot.fire_origin = MUZZLE;
            shot.projectile_direction = Some((Vec3::new(WALL, 0.0, 0.0) - MUZZLE).normalize());
            shot.splash_on_path_tolerance = Some(0.1 + MUZZLE.length() + 0.25);
        });
        let enemy = rig.target(Vec3::new(distance + 0.1, 0.0, 0.0));
        let record = rig.enemy_record(enemy, Vec3::new(distance, 0.0, 0.0));
        rig.declare(vec![record]);
        let (_, result) = rig.run();
        assert_near(point(&result), Vec3::new(distance, 0.0, 0.0));
        assert_eq!(
            result.contacts[0].hit,
            postretro_entities::ContactHit::Entity(enemy)
        );
    }
}

#[test]
fn declared_point_just_inside_the_tolerance_counts_and_just_outside_does_not() {
    let tolerance = 0.25;
    for (lateral, counts) in [(tolerance - 0.01, true), (tolerance + 0.01, false)] {
        let mut rig = Rig::new(wall_at_x(8.0), 0.0);
        rig.declare(vec![world_contact(
            Vec3::new(5.0, 0.0, lateral),
            Vec3::NEG_X,
        )]);
        let (_, result) = rig.run();
        let expected = if counts {
            Vec3::new(5.0, 0.0, lateral)
        } else {
            Vec3::new(8.0, 0.0, 0.0)
        };
        assert_near(point(&result), expected);
    }
}

// ---- C12 ----

#[test]
fn march_state_ends_with_its_shot_client_or_pawn_and_survives_a_respawn() {
    let mut rig = Rig::new(wall_at_x(8.0), 0.0);
    rig.declare(vec![at(8.0)]);
    rig.run();
    assert!(rig.march().is_none(), "gone once released");

    let mut rig = Rig::new(wall_at_x(8.0), 0.0);
    rig.f.open_shots.remove_client(7);
    assert!(rig.march().is_none(), "gone when its client leaves");

    let mut rig = Rig::new(wall_at_x(8.0), 0.0);
    let pawn = rig.f.pawn;
    rig.f.open_shots.remove_pawn(pawn);
    assert!(rig.march().is_none(), "gone when its pawn is removed");

    // A level unload replaces the whole store, marches with it.
    let mut rig = Rig::new(wall_at_x(8.0), 0.0);
    rig.f.open_shots = OpenAuthorizedShots::new();
    assert!(rig.march().is_none());

    // A respawn bumps the pawn's life; the march keeps going and lands.
    let mut rig = Rig::new(wall_at_x(8.0), 0.0);
    let mut movement = movement_component_with_eye_height(0.5);
    movement.life_counter = movement.life_counter.wrapping_add(1);
    rig.f.registry.set_component(rig.f.pawn, movement).unwrap();
    rig.declare(vec![at(8.0)]);
    assert_near(point(&rig.run().1), Vec3::new(8.0, 0.0, 0.0));
}

#[test]
fn resolved_march_casts_nothing_more_and_no_open_shot_means_no_work() {
    let mut rig = Rig::new(wall_at_x(8.0), 0.0);
    rig.door(Some(5.0));
    rig.steps(6);
    let contact = rig.march().unwrap().contact;
    assert!(contact.is_some());
    rig.door(None);
    rig.steps(4);
    assert_eq!(rig.march().unwrap().contact, contact, "it stopped casting");

    let mut idle = HitIngestFixture::new(CollisionWorld::new());
    idle.open_shots = OpenAuthorizedShots::new();
    assert!(idle.march_tick(100).is_empty());
}

// ---- C13 ----

// No movers, nothing on the path, the host launching where the client did: a
// world-contact blast lands at the host's first contact, as the retired
// replay did, on no later a tick than its hold released.
#[test]
fn level_without_movers_lands_at_the_same_centre_no_later() {
    let mut rig = Rig::new(wall_at_x(7.5), 0.0);
    rig.declare(vec![at(7.5)]);
    let (ticks, result) = rig.run();
    assert_near(point(&result), Vec3::new(7.5, 0.0, 0.0));
    // The retired hold: ceil(7.5 m / 1 m per tick) plus one tick of margin.
    assert!(ticks <= 8 + 1, "released after {ticks} ticks");
}
