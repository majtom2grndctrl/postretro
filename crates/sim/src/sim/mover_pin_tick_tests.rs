// Pinned-body ordering through the authoritative fixed-tick seam: a command
// applied between fixed ticks reaches the mover driver only along a course
// the pin test has seen.
// See: context/lib/movement.md §6 (Moving bases) · context/lib/entity_model.md §5

use std::cell::RefCell;
use std::rc::Rc;

use glam::Vec3;
use postretro_entities::{
    EntityId, EntityRegistry, KinematicMoverComponent, KinematicMoverConfig, KinematicMoverMode,
    MoverCommand, Transform,
};
use postretro_foundation::GroundRef;
use postretro_scripting_core::reaction_dispatch::ProgressTracker;

use super::tests::{sim_command, trigger_movement};
use super::{PostMovementCommand, simulate_tick};
use crate::collision::moving::MoverCollider;
use crate::collision::{CollisionCapsule, CollisionWorld};
use crate::kinematic_mover::{MoverTickStateTable, apply_mover_command};
use crate::scripting_systems::hit_zones::HitZoneStore;

const DT: f32 = 1.0 / 60.0;
const LIFT: u32 = 1;
/// The ceiling sits 1.6 m above the lift's top stop: too low for the 2.4 m
/// standing capsule.
const CEILING: f32 = 4.6;

struct Rig {
    registry: Rc<RefCell<EntityRegistry>>,
    world: CollisionWorld,
    colliders: Vec<MoverCollider>,
    movers: MoverTickStateTable,
    hit_zones: HitZoneStore,
    progress: ProgressTracker,
    lift: EntityId,
    rider: EntityId,
}

impl Rig {
    /// A default lift rising from y = 0 to 3 under a low ceiling, with the
    /// local pawn riding its deck.
    fn new() -> Self {
        let world = CollisionWorld::from_triangles_for_test(
            vec![
                Vec3::new(-10.0, CEILING, -10.0),
                Vec3::new(10.0, CEILING, -10.0),
                Vec3::new(10.0, CEILING, 10.0),
                Vec3::new(-10.0, CEILING, 10.0),
            ],
            vec![[0, 1, 2], [0, 2, 3]],
        );
        let deck = MoverCollider::from_local_triangles(
            LIFT,
            &[
                Vec3::new(-1.5, 0.0, -1.5),
                Vec3::new(1.5, 0.0, -1.5),
                Vec3::new(1.5, 0.0, 1.5),
                Vec3::new(-1.5, 0.0, 1.5),
            ],
            &[[0, 2, 1], [0, 3, 2]],
        )
        .expect("deck collider is valid");
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (lift, rider) = {
            let mut registry = registry.borrow_mut();
            let lift = registry.spawn(Transform::default());
            registry
                .set_component(
                    lift,
                    KinematicMoverComponent::new(
                        LIFT,
                        KinematicMoverConfig {
                            waypoints: vec![Vec3::ZERO, Vec3::new(0.0, 3.0, 0.0)],
                            waypoint_names: vec!["bottom".into(), "top".into()],
                            speed_mps: 2.0,
                            wait_ms: 0.0,
                            mode: KinematicMoverMode::Once,
                            started: false,
                            spin_axis: Vec3::ZERO,
                            initial_spin_rate_rad_s: 0.0,
                            spin_accel_rad_s2: 0.0,
                            carry_yaw: false,
                        },
                    ),
                )
                .unwrap();
            let rider = registry.spawn(Transform {
                position: Vec3::new(0.0, 1.22, 0.0),
                ..Transform::default()
            });
            registry.set_component(rider, trigger_movement()).unwrap();
            registry.mark_local_player_pawn(rider).unwrap();
            (lift, rider)
        };
        Self {
            registry,
            world,
            colliders: vec![deck],
            movers: MoverTickStateTable::default(),
            hit_zones: HitZoneStore::new(),
            progress: ProgressTracker::new(),
            lift,
            rider,
        }
    }

    fn tick(&mut self) {
        let mut ai_runtime = postretro_ai::AiRuntime::new();
        simulate_tick(
            self.registry.clone(),
            &self.world,
            &self.hit_zones,
            None,
            postretro_collision::EnvironmentView::level_only(-20.0),
            None,
            0.0,
            &mut self.progress,
            postretro_ai::test_tick_runner!(&mut ai_runtime),
            &self.colliders,
            &mut self.movers,
            &[],
            &sim_command(false, false),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            DT,
            None,
            |_| {},
        );
    }

    /// Apply a mover command between fixed ticks, as the frame-end script
    /// drain and a player event's fire list do.
    fn command(&self, command: MoverCommand) {
        let mut registry = self.registry.borrow_mut();
        let mut mover = registry
            .get_component::<KinematicMoverComponent>(self.lift)
            .unwrap()
            .clone();
        apply_mover_command(&mut mover, &command);
        registry.set_component(self.lift, mover).unwrap();
    }

    fn lift(&self) -> (KinematicMoverComponent, Vec3) {
        let registry = self.registry.borrow();
        (
            registry
                .get_component::<KinematicMoverComponent>(self.lift)
                .unwrap()
                .clone(),
            registry
                .get_component::<Transform>(self.lift)
                .unwrap()
                .position,
        )
    }

    fn rider_overlaps_ceiling(&self) -> bool {
        let registry = self.registry.borrow();
        let position = registry
            .get_component::<Transform>(self.rider)
            .unwrap()
            .position;
        self.world
            .overlaps_static(position, CollisionCapsule::new(0.4, 0.8))
    }

    /// Ride the lift until it holds for its rider under the ceiling.
    fn ride_into_hold(&mut self) {
        for _ in 0..20 {
            self.tick();
        }
        assert_eq!(
            self.registry
                .borrow()
                .get_component::<postretro_foundation::PlayerMovementComponent>(self.rider)
                .unwrap()
                .ground,
            GroundRef::Mover(LIFT),
            "the pawn rides the lift"
        );
        self.command(MoverCommand::Start);
        for _ in 0..120 {
            self.tick();
            assert!(!self.rider_overlaps_ceiling(), "the hold never embeds");
        }
        assert!(self.lift().0.blocked, "the lift holds for its pinned rider");
    }
}

#[test]
fn pre_tick_pin_test_runs_before_the_mover_tick() {
    let mut rig = Rig::new();
    rig.ride_into_hold();
    let (_, held_at) = rig.lift();
    // A frame-end reaction restarts the held lift: the next tick's driver
    // must not run that course into the rider.
    rig.command(MoverCommand::Start);
    assert!(!rig.lift().0.blocked, "the command cleared the hold");
    rig.tick();
    let (lift, position) = rig.lift();
    assert!(
        lift.blocked,
        "the pin test re-held the lift before it moved"
    );
    assert!(
        (position - held_at).length() < 1.0e-6,
        "the lift never moved"
    );
    assert!(!rig.rider_overlaps_ceiling());

    // A course away from the pin moves: an authored back-off.
    rig.command(MoverCommand::Reverse);
    rig.tick();
    assert!(rig.lift().1.y < held_at.y, "the reversed lift backs off");
    assert!(!rig.rider_overlaps_ceiling());
}

#[test]
fn command_between_two_simulate_ticks_is_pin_tested_before_the_mover_tick() {
    let mut rig = Rig::new();
    rig.ride_into_hold();
    let (_, held_at) = rig.lift();
    // One frame running two fixed ticks; a player event's fire list starts
    // the lift after the first.
    rig.tick();
    rig.command(MoverCommand::Start);
    rig.tick();
    assert!(
        (rig.lift().1 - held_at).length() < 1.0e-6,
        "the second tick's driver never ran the course into the rider"
    );
    assert!(rig.lift().0.blocked, "the course into the pin held again");
    assert!(!rig.rider_overlaps_ceiling());

    // The next frame's fire list reverses it between its two ticks: a course
    // away from the pin moves on the second tick and never embeds the rider
    // on the way back down.
    rig.tick();
    rig.command(MoverCommand::Reverse);
    rig.tick();
    let (lift, position) = rig.lift();
    assert!(
        !lift.blocked && position.y < held_at.y,
        "the reversed lift backs off on the second tick: {position:?}"
    );
    assert!(!rig.rider_overlaps_ceiling());
    for _ in 0..120 {
        rig.tick();
        assert!(!rig.rider_overlaps_ceiling(), "the descent never embeds");
    }
    let (lift, position) = rig.lift();
    assert!(
        !lift.started && position.y.abs() < 1.0e-3,
        "the reversed lift came home unstarted: {position:?}"
    );
}
