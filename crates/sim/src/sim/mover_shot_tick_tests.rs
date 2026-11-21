// Host-tick shots against movers at the tick's published pose.
// See: context/lib/networking.md §Combat authority · context/lib/entity_model.md §5
use std::cell::RefCell;
use std::rc::Rc;

use glam::{Quat, Vec3};
use postretro_entities::components::health::{HealthComponent, Hitbox};
use postretro_entities::components::projectile::ProjectileComponent;
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::SplashDescriptor;

use super::projectile_stage::{PredictedProjectileResolution, advance, advance_predicted};
use super::tests::{remote_command, trigger_movement};
use super::weapon_stage::run_remote_weapon_commands;
use crate::collision::CollisionWorld;
use crate::collision::moving::{CombinedCollisionWorld, MoverCollider};
use crate::collision::test_movers::{DoorPoses, door_slab};
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::weapon::tests::{assert_vec3_approx, projectile_weapon_component, wall_world};

const DOOR: u32 = 3;
/// Forward of the eye by more than the near door's distance.
const MUZZLE: Vec3 = Vec3::new(0.5, 0.0, -0.8);
/// `trigger_movement`'s eye over a pawn at the origin.
const EYE: Vec3 = Vec3::new(0.0, 0.5, 0.0);

fn thin_door() -> [MoverCollider; 1] {
    [door_slab(DOOR, 0.05, 3.0, 2.0)]
}

/// A door across the -Z aim, its near face at `z + 0.05`.
fn door_across_z(z: f32) -> DoorPoses {
    DoorPoses::default().place(
        DOOR,
        Vec3::new(0.0, -1.0, z),
        Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
    )
}

/// The host's rebuild of one remote projectile FIRE: its frozen launch.
fn rebuilt_launch(static_world: &CollisionWorld, poses: &DoorPoses) -> (Vec3, Vec3) {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (pawn, weapon) = {
        let mut registry = registry.borrow_mut();
        let pawn = registry.spawn(Transform::default());
        registry.set_component(pawn, trigger_movement()).unwrap();
        let weapon = registry.spawn(Transform::default());
        registry
            .set_component(weapon, projectile_weapon_component(Some(MUZZLE)))
            .unwrap();
        (pawn, weapon)
    };
    let movers = thin_door();
    // Facing yaw 0 and level pitch aim down -Z.
    let result = run_remote_weapon_commands(
        &registry,
        &[remote_command(pawn, Some(weapon), 42, 9, true, false)],
        &[],
        None,
        &CombinedCollisionWorld::new(static_world, &movers, poses),
        &HitZoneStore::new(),
        0.0,
        1.0 / 60.0,
    );
    let [authorized] = result.authorized_shots.as_slice() else {
        panic!("accepted remote projectile fire mints one authorization");
    };
    (
        authorized.shot.fire_origin,
        authorized
            .shot
            .projectile_direction
            .expect("projectile FIRE freezes a direction"),
    )
}

#[test]
fn rebuilt_client_fire_across_a_host_closed_door_launches_from_the_eye() {
    let (origin, direction) = rebuilt_launch(&CollisionWorld::new(), &door_across_z(-0.5));
    assert_vec3_approx(origin, EYE);
    assert_vec3_approx(direction, Vec3::NEG_Z);
}

#[test]
fn rebuilt_client_fire_converges_on_a_closed_door_not_the_wall_behind() {
    let (origin, direction) = rebuilt_launch(&wall_world(), &door_across_z(-3.0));
    assert!(
        origin.distance(EYE) > 0.1,
        "a muzzle clear of the door launches from the muzzle: {origin:?}"
    );
    assert_vec3_approx(direction, (Vec3::new(0.0, 0.5, -2.95) - origin).normalize());
    let (open_origin, open_direction) = rebuilt_launch(&wall_world(), &DoorPoses::default());
    assert_vec3_approx(open_origin, origin);
    assert_vec3_approx(
        open_direction,
        (Vec3::new(0.0, 0.5, -5.0) - origin).normalize(),
    );
}

// ---- Projectiles and splash (H2, H4, H5; client half of C2) ----

/// Rockets fly -Z from the origin at 10 m/s; each test step is 0.1 s.
const STEP: f32 = 0.1;

struct Rig {
    registry: Rc<RefCell<EntityRegistry>>,
    static_world: CollisionWorld,
    movers: Vec<MoverCollider>,
    poses: DoorPoses,
}

impl Rig {
    fn new(static_world: CollisionWorld) -> Self {
        Self {
            registry: Rc::new(RefCell::new(EntityRegistry::new())),
            static_world,
            movers: vec![door_slab(DOOR, 0.05, 3.0, 2.0)],
            poses: DoorPoses::default(),
        }
    }

    /// The door across the -Z flight, near face at `z + 0.05`.
    fn close_door_at(&mut self, z: f32) {
        self.poses = door_across_z(z);
    }

    fn open_door(&mut self) {
        self.poses = DoorPoses::default().place(
            DOOR,
            Vec3::new(0.0, 10.0, -3.0),
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
        );
    }

    fn target(&self, position: Vec3) -> EntityId {
        let mut registry = self.registry.borrow_mut();
        let target = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        registry
            .set_component(
                target,
                HealthComponent {
                    max: 20.0,
                    current: 20.0,
                    hitbox: Some(Hitbox {
                        half_extents: Vec3::splat(0.25),
                        offset: Vec3::ZERO,
                    }),
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .unwrap();
        target
    }

    fn health(&self, target: EntityId) -> f32 {
        self.registry
            .borrow()
            .get_component::<HealthComponent>(target)
            .unwrap()
            .current
    }

    fn rocket(
        &self,
        position: Vec3,
        direction: Vec3,
        radius: f32,
        splash: Option<f32>,
    ) -> EntityId {
        let mut registry = self.registry.borrow_mut();
        let owner_pawn = registry.spawn(Transform {
            position: Vec3::new(0.0, 0.0, 50.0),
            ..Transform::default()
        });
        let owner_weapon = registry.spawn(Transform::default());
        let projectile = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        registry
            .set_component(
                projectile,
                ProjectileComponent {
                    source_sounds: None,
                    predicted_visible: true,
                    source_action: None,
                    source_shot: None,
                    knockback_impulse: [0.0; 3],
                    direction: direction.to_array(),
                    speed: 10.0,
                    radius,
                    remaining_range: 100.0,
                    remaining_lifetime: 10.0,
                    damage: 10.0,
                    credit_source: "test.rocket".to_string(),
                    owner_pawn,
                    owner_life: 0,
                    owner_weapon,
                    attackerless: false,
                    spawned: false,
                    predicted_shot_id: None,
                    elapsed_flight_age: 0.0,
                    flipbook_active: false,
                    impact_light: None,
                    splash: splash.map(|radius| SplashDescriptor {
                        knockback: None,
                        radius,
                        min_fraction: 1.0,
                        self_damage: false,
                    }),
                    source_weapon: None,
                    activation: None,
                    damage_type: None,
                },
            )
            .unwrap();
        projectile
    }

    fn edit_rocket(&self, rocket: EntityId, edit: impl FnOnce(&mut ProjectileComponent)) {
        let mut registry = self.registry.borrow_mut();
        let mut component = registry
            .get_component::<ProjectileComponent>(rocket)
            .unwrap()
            .clone();
        edit(&mut component);
        registry.set_component(rocket, component).unwrap();
    }

    fn world(&self) -> CombinedCollisionWorld<'_> {
        CombinedCollisionWorld::new(&self.static_world, &self.movers, &self.poses)
    }

    /// One host step; returns this step's contact points.
    fn step(&self) -> Vec<Vec3> {
        advance(
            &self.registry,
            &self.world(),
            &HitZoneStore::new(),
            0.0,
            STEP,
            &mut |_| {},
        )
        .into_iter()
        .map(|contact| contact.point)
        .collect()
    }

    fn exists(&self, id: EntityId) -> bool {
        self.registry.borrow().exists(id)
    }
}

#[test]
fn host_projectile_impacts_a_closed_door_and_spares_the_target_behind() {
    let mut rig = Rig::new(CollisionWorld::new());
    rig.close_door_at(-3.0);
    let target = rig.target(Vec3::new(0.0, 0.0, -5.0));
    let rocket = rig.rocket(Vec3::ZERO, Vec3::NEG_Z, 0.1, None);
    let mut contacts = Vec::new();
    for _ in 0..10 {
        contacts.extend(rig.step());
    }
    assert!(!rig.exists(rocket));
    let [contact] = contacts.as_slice() else {
        panic!("one contact: {contacts:?}");
    };
    assert_vec3_approx(*contact, Vec3::new(0.0, 0.0, -2.85));
    assert_eq!(rig.health(target), 20.0);
}

#[test]
fn host_projectile_meets_a_door_that_closes_in_its_path_and_passes_one_that_opens() {
    // Closes in flight.
    let mut rig = Rig::new(CollisionWorld::new());
    rig.open_door();
    let target = rig.target(Vec3::new(0.0, 0.0, -5.0));
    rig.rocket(Vec3::ZERO, Vec3::NEG_Z, 0.1, None);
    rig.step();
    rig.close_door_at(-3.0);
    for _ in 0..10 {
        rig.step();
    }
    assert_eq!(rig.health(target), 20.0, "the door closed in front of it");

    // Opens before arrival.
    let mut rig = Rig::new(CollisionWorld::new());
    rig.close_door_at(-3.0);
    let target = rig.target(Vec3::new(0.0, 0.0, -5.0));
    rig.rocket(Vec3::ZERO, Vec3::NEG_Z, 0.1, None);
    rig.step();
    rig.open_door();
    for _ in 0..10 {
        rig.step();
    }
    assert!(
        rig.health(target) < 20.0,
        "the door opened before it arrived"
    );
}

#[test]
fn trap_projectile_impacts_a_closed_door() {
    let mut rig = Rig::new(CollisionWorld::new());
    rig.close_door_at(-3.0);
    let target = rig.target(Vec3::new(0.0, 0.0, -5.0));
    let rocket = rig.rocket(Vec3::ZERO, Vec3::NEG_Z, 0.1, None);
    rig.edit_rocket(rocket, |component| component.attackerless = true);
    for _ in 0..10 {
        rig.step();
    }
    assert_eq!(rig.health(target), 20.0);
}

#[test]
fn projectile_a_door_snaps_onto_detonates_that_tick_and_spares_the_far_side() {
    let mut rig = Rig::new(CollisionWorld::new());
    // Off the flight line, so the rocket reaches the door untouched.
    let near = rig.target(Vec3::new(1.5, 0.0, -2.0));
    let far = rig.target(Vec3::new(1.5, 0.0, -4.0));
    let rocket = rig.rocket(Vec3::ZERO, Vec3::NEG_Z, 0.0, Some(3.0));
    for _ in 0..3 {
        rig.step();
    }
    // The rocket sits at z = -3; the door snaps closed around it.
    rig.close_door_at(-3.0);
    let contacts = rig.step();
    assert!(
        !rig.exists(rocket),
        "an enclosed rocket detonates that tick"
    );
    let [contact] = contacts.as_slice() else {
        panic!("one contact: {contacts:?}");
    };
    assert_vec3_approx(*contact, Vec3::new(0.0, 0.0, -3.0));
    assert!(
        rig.health(near) < 20.0,
        "the side it came from takes the blast"
    );
    assert_eq!(rig.health(far), 20.0, "the door shields the far side");
}

#[test]
fn rocket_launched_inside_a_door_detonates_behind_its_flight_even_with_no_lifetime() {
    for lifetime in [10.0, 0.0] {
        let mut rig = Rig::new(CollisionWorld::new());
        rig.close_door_at(-3.0);
        let behind = rig.target(Vec3::new(0.0, 0.0, -1.5));
        let ahead = rig.target(Vec3::new(0.0, 0.0, -4.5));
        let rocket = rig.rocket(Vec3::new(0.0, 0.0, -3.0), Vec3::NEG_Z, 0.1, Some(3.0));
        rig.edit_rocket(rocket, |component| {
            component.spawned = true;
            component.remaining_lifetime = lifetime;
        });
        assert!(
            rig.step().is_empty(),
            "the launch tick consumes spawn grace"
        );
        assert_eq!(
            rig.step().len(),
            1,
            "lifetime {lifetime}: detonates next tick"
        );
        assert!(rig.health(behind) < 20.0, "lifetime {lifetime}");
        assert_eq!(rig.health(ahead), 20.0, "lifetime {lifetime}");
    }
}

#[test]
fn mover_between_blast_and_target_blocks_splash() {
    // A wall at z = -6; the door stands on its side at x = 1, between the blast
    // and one target. Both targets sit inside the blast radius.
    let mut rig = Rig::new(crate::weapon::tests::wall_world_at(-6.0));
    rig.poses = DoorPoses::default().place(DOOR, Vec3::new(1.0, -1.0, -5.0), Quat::IDENTITY);
    let shielded = rig.target(Vec3::new(2.0, 0.0, -5.5));
    let open = rig.target(Vec3::new(-1.5, 0.0, -5.5));
    rig.rocket(Vec3::ZERO, Vec3::NEG_Z, 0.1, Some(3.0));
    for _ in 0..10 {
        rig.step();
    }
    assert!(rig.health(open) < 20.0, "the blast side takes damage");
    assert_eq!(rig.health(shielded), 20.0, "the mover blocks splash");
}

#[test]
fn zero_radius_blast_on_either_face_of_a_rotated_door_damages_that_side_only() {
    for (start, direction) in [
        (Vec3::ZERO, Vec3::NEG_Z),
        (Vec3::new(0.0, 0.0, -6.0), Vec3::Z),
    ] {
        let mut rig = Rig::new(CollisionWorld::new());
        rig.movers = vec![door_slab(DOOR, 0.25, 3.0, 2.0)];
        rig.poses = DoorPoses::default().place(
            DOOR,
            Vec3::new(0.0, -1.0, -3.0),
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 + 0.5),
        );
        let start_side = rig.target(start + direction * 1.5);
        let far_side = rig.target(start + direction * 4.5);
        rig.rocket(start, direction, 0.0, Some(3.0));
        for _ in 0..10 {
            rig.step();
        }
        assert!(rig.health(start_side) < 20.0, "from {start:?}");
        assert_eq!(rig.health(far_side), 20.0, "from {start:?}");
    }
}

#[test]
fn client_predicted_projectile_meets_a_door_in_its_prediction() {
    for snap in [false, true] {
        let mut rig = Rig::new(CollisionWorld::new());
        if !snap {
            rig.close_door_at(-3.0);
        }
        let rocket = rig.rocket(Vec3::ZERO, Vec3::NEG_Z, 0.1, None);
        rig.edit_rocket(rocket, |component| {
            component.predicted_shot_id = Some(postretro_foundation::ShotId::from_parts(
                4,
                1,
                postretro_foundation::ActivationLane::Primary,
                0,
            ));
        });
        let mut impacts = Vec::new();
        for frame in 0..10 {
            if snap && frame == 3 {
                // The predicted door snaps shut around the rocket at z = -3.
                rig.close_door_at(-3.0);
            }
            advance_predicted(
                &rig.registry,
                &rig.world(),
                &HitZoneStore::new(),
                0.0,
                STEP,
                &mut |resolution| {
                    if let PredictedProjectileResolution::Impact { impact, .. } = resolution {
                        impacts.push(impact);
                    }
                },
            );
        }
        let [impact] = impacts.as_slice() else {
            panic!("snap {snap}: one impact: {impacts:?}");
        };
        assert_eq!(impact.target, None);
        let expected_z = if snap { -3.0 } else { -2.85 };
        assert_vec3_approx(impact.point, Vec3::new(0.0, 0.0, expected_z));
    }
}

// ---- One published pose per tick (H6, P15) ----

mod same_tick {
    use super::*;
    use crate::kinematic_mover::{MoverTickStateTable, apply_mover_command};
    use crate::sim::tests::{sim_command_at, weapon_component};
    use crate::sim::{PostMovementCommand, simulate_tick};
    use postretro_entities::components::inventory::Inventory;
    use postretro_entities::{
        BlockPolicy, KinematicMoverComponent, KinematicMoverConfig, KinematicMoverMode,
        MoverCommand,
    };
    use postretro_scripting_core::reaction_dispatch::ProgressTracker;

    const DT: f32 = 1.0 / 60.0;
    const OPEN: Vec3 = Vec3::new(5.0, 10.0, 0.0);
    const CLOSED: Vec3 = Vec3::new(5.0, -1.0, 0.0);

    struct World {
        registry: Rc<RefCell<EntityRegistry>>,
        static_world: CollisionWorld,
        colliders: Vec<MoverCollider>,
        movers: MoverTickStateTable,
        door: EntityId,
        weapon: EntityId,
        ticks: u32,
        /// Reverse the door's course from inside the AI stage.
        reverse_in_ai: bool,
    }

    impl World {
        /// A door above the line it closes onto in one tick; the local pawn
        /// at the origin holds a hitscan weapon aimed along +X.
        fn new(policy: BlockPolicy) -> Self {
            Self::with_door(OPEN, CLOSED, policy, CollisionWorld::new())
        }

        fn with_door(
            open: Vec3,
            closed: Vec3,
            policy: BlockPolicy,
            static_world: CollisionWorld,
        ) -> Self {
            let registry = Rc::new(RefCell::new(EntityRegistry::new()));
            let (door, weapon) = {
                let mut registry = registry.borrow_mut();
                let door = registry.spawn(Transform {
                    position: open,
                    ..Transform::default()
                });
                let mut mover = KinematicMoverComponent::new(
                    DOOR,
                    KinematicMoverConfig {
                        waypoints: vec![open, closed],
                        waypoint_names: vec!["open".into(), "closed".into()],
                        speed_mps: open.distance(closed) / DT,
                        wait_ms: 0.0,
                        mode: KinematicMoverMode::Once,
                        started: false,
                        spin_axis: Vec3::ZERO,
                        initial_spin_rate_rad_s: 0.0,
                        spin_accel_rad_s2: 0.0,
                        carry_yaw: false,
                    },
                );
                mover.block_policy = policy;
                registry.set_component(door, mover).unwrap();
                let pawn = registry.spawn(Transform::default());
                let weapon = registry.spawn(Transform::default());
                let mut component = weapon_component("weapon.test.mover");
                component.spread_degrees = 0.0;
                registry.set_component(weapon, component).unwrap();
                let mut inventory = Inventory::default();
                inventory.wieldables[0] = Some(weapon);
                registry.set_component(pawn, inventory).unwrap();
                registry.set_component(pawn, trigger_movement()).unwrap();
                registry.mark_local_player_pawn(pawn).unwrap();
                (door, weapon)
            };
            Self {
                registry,
                static_world,
                colliders: vec![door_slab(DOOR, 0.1, 6.0, 3.0)],
                movers: MoverTickStateTable::default(),
                door,
                weapon,
                ticks: 0,
                reverse_in_ai: false,
            }
        }

        /// One tick; the door's pose the AI was handed, and the hitscan impact.
        fn tick(&mut self, fire: bool) -> (Option<Vec3>, Vec<Vec3>) {
            let mut ai_door = None;
            let (reverse_in_ai, door) = (self.reverse_in_ai, self.door);
            self.ticks += 1;
            let events = simulate_tick(
                self.registry.clone(),
                &self.static_world,
                &HitZoneStore::new(),
                None,
                postretro_collision::EnvironmentView::level_only(0.0),
                Some(self.weapon),
                0.0,
                &mut ProgressTracker::new(),
                |registry, _, inputs, _| {
                    if reverse_in_ai {
                        let mut mover = registry
                            .get_component::<KinematicMoverComponent>(door)
                            .unwrap()
                            .clone();
                        apply_mover_command(&mut mover, &MoverCommand::Reverse);
                        registry.set_component(door, mover).unwrap();
                    }
                    ai_door = inputs
                        .mover_nav
                        .poses
                        .pose(DOOR)
                        .map(|pose| pose.transform.position);
                    crate::ai_host::AiTickResult {
                        events: Vec::new(),
                        projectile_spawns: Vec::new(),
                    }
                },
                &self.colliders,
                &mut self.movers,
                &[],
                &sim_command_at(self.ticks, fire, false),
                |_| PostMovementCommand {
                    aim_origin: Vec3::new(0.0, 0.5, 0.0),
                    aim_direction: Vec3::X,
                },
                DT,
                None,
                |_| {},
            );
            (ai_door, events.weapon_impact_points)
        }

        fn set_speed(&self, speed_mps: f32) {
            let mut registry = self.registry.borrow_mut();
            let mut mover = registry
                .get_component::<KinematicMoverComponent>(self.door)
                .unwrap()
                .clone();
            mover.speed_mps = speed_mps;
            registry.set_component(self.door, mover).unwrap();
        }

        fn command(&self, command: MoverCommand) {
            let mut registry = self.registry.borrow_mut();
            let mut mover = registry
                .get_component::<KinematicMoverComponent>(self.door)
                .unwrap()
                .clone();
            apply_mover_command(&mut mover, &command);
            registry.set_component(self.door, mover).unwrap();
        }

        /// A rocket at x = 4.5 flying +X one metre a tick: this tick's segment
        /// crosses the door's plane.
        fn rocket(&self) -> EntityId {
            let rig = Rig {
                registry: self.registry.clone(),
                static_world: CollisionWorld::new(),
                movers: Vec::new(),
                poses: DoorPoses::default(),
            };
            let rocket = rig.rocket(Vec3::new(4.5, 0.5, 0.0), Vec3::X, 0.05, None);
            rig.edit_rocket(rocket, |component| component.speed = 1.0 / DT);
            rocket
        }
    }

    #[test]
    fn on_the_tick_a_door_closes_rocket_sight_and_hitscan_all_meet_it() {
        let mut world = World::new(BlockPolicy::default());
        let (ai_door, _) = world.tick(false);
        assert_eq!(ai_door, Some(OPEN), "the door stands open before it closes");
        world.command(MoverCommand::Start);
        let rocket = world.rocket();
        let (ai_door, hitscan) = world.tick(true);
        assert_eq!(ai_door, Some(CLOSED), "AI reads the pose Movers published");
        assert!(
            !world.registry.borrow().exists(rocket),
            "the rocket meets the door this tick"
        );
        let [impact] = hitscan.as_slice() else {
            panic!("one pellet: {hitscan:?}");
        };
        assert_vec3_approx(*impact, Vec3::new(4.9, 0.5, 0.0));
    }

    // P15: the door's course reverses mid-tick, after Movers published its
    // pose, as the blocking pass's reversal does (it writes the mover
    // component; `run_mover_blocking_pass` reads the pose table only by
    // shared reference). The AI stage stands in for that write so the test
    // controls it; the rocket before it and the hitscan after it both meet
    // the pose Movers published on T, and the reversal shows from T + 1.
    #[test]
    fn a_reversal_mid_tick_shows_from_the_next_tick() {
        // Four metres a tick: on T the door sits at z = 0, mid-travel.
        let open = Vec3::new(5.0, -1.0, 4.0);
        let closed = Vec3::new(5.0, -1.0, -4.0);
        let mut world = World::with_door(open, closed, BlockPolicy::Reverse, CollisionWorld::new());
        world.set_speed(4.0 / DT);
        world.tick(false);
        world.command(MoverCommand::Start);
        let rocket = world.rocket();
        world.reverse_in_ai = true;
        let (ai_door, hitscan) = world.tick(true);
        world.reverse_in_ai = false;
        let published = world.movers.get(DOOR).map(|state| state.transform.position);
        assert_eq!(ai_door, published, "AI read the tick's published pose");
        let door_z = published.expect("door published").z;
        assert!(
            door_z.abs() < 3.0,
            "the door covers the shot line: {door_z}"
        );
        assert!(!world.registry.borrow().exists(rocket), "the rocket met it");
        let [impact] = hitscan.as_slice() else {
            panic!("one pellet: {hitscan:?}");
        };
        assert_vec3_approx(*impact, Vec3::new(4.9, 0.5, 0.0));
        let (next_door, _) = world.tick(false);
        assert!(
            next_door.is_some_and(|position| position.z > door_z),
            "the reversal shows from T + 1: {next_door:?}"
        );
    }
}
