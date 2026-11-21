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
