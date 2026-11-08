// Pinned-body ordering through the authoritative fixed-tick seam: a command
// applied between fixed ticks reaches the mover driver only along a course
// the pin test has seen.
// See: context/lib/movement.md §6 (Moving bases) · context/lib/entity_model.md §5

use std::cell::RefCell;
use std::rc::Rc;

use glam::Vec3;
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::{
    BlockPolicy, EntityId, EntityRegistry, KinematicMoverComponent, KinematicMoverConfig,
    KinematicMoverMode, MoverCommand, Transform,
};
use postretro_foundation::GroundRef;
use postretro_scripting_core::reaction_dispatch::ProgressTracker;

use super::tests::{sim_command, trigger_movement};
use super::{PostMovementCommand, simulate_tick};
use crate::collision::moving::MoverCollider;
use crate::collision::{CollisionCapsule, CollisionWorld, SKIN_DISTANCE};
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

    /// A door closing along -X onto a living agent flush against a jamb.
    fn agent_at_jamb(policy: BlockPolicy) -> Self {
        let world = CollisionWorld::from_triangles_for_test(
            vec![
                Vec3::new(-10.0, 0.0, -10.0),
                Vec3::new(10.0, 0.0, -10.0),
                Vec3::new(10.0, 0.0, 10.0),
                Vec3::new(-10.0, 0.0, 10.0),
                Vec3::new(-1.0, 0.0, -3.0),
                Vec3::new(-1.0, 3.0, -3.0),
                Vec3::new(-1.0, 3.0, 3.0),
                Vec3::new(-1.0, 0.0, 3.0),
            ],
            vec![[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7]],
        );
        let leaf = MoverCollider::from_local_triangles(
            LIFT,
            &[
                Vec3::new(0.0, 0.0, -1.5),
                Vec3::new(0.0, 3.0, -1.5),
                Vec3::new(0.0, 3.0, 1.5),
                Vec3::new(0.0, 0.0, 1.5),
            ],
            &[[0, 1, 2], [0, 2, 3]],
        )
        .expect("leaf collider is valid");
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (door, agent) = {
            let mut registry = registry.borrow_mut();
            let waypoints = vec![Vec3::new(3.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0)];
            let door = registry.spawn(Transform {
                position: waypoints[0],
                ..Transform::default()
            });
            let mut mover = KinematicMoverComponent::new(
                LIFT,
                KinematicMoverConfig {
                    waypoints,
                    waypoint_names: vec!["open".into(), "closed".into()],
                    speed_mps: 2.0,
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
            let agent = registry.spawn(Transform {
                position: Vec3::new(-0.6 + SKIN_DISTANCE, 1.2 + SKIN_DISTANCE, 0.0),
                ..Transform::default()
            });
            registry
                .set_component(agent, AgentComponent::new(0.4, 2.4, 0.3, 4.0))
                .unwrap();
            (door, agent)
        };
        let mut rig = Self {
            registry,
            world,
            colliders: vec![leaf],
            movers: MoverTickStateTable::default(),
            hit_zones: HitZoneStore::new(),
            progress: ProgressTracker::new(),
            lift: door,
            rider: agent,
        };
        rig.set_rider_health(100.0);
        rig
    }

    fn set_rider_health(&mut self, current: f32) {
        self.registry
            .borrow_mut()
            .set_component(
                self.rider,
                HealthComponent {
                    max: 100.0,
                    current,
                    hitbox: None,
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .unwrap();
    }

    /// Tick until the door holds.
    fn close_until_held(&mut self) {
        self.command(MoverCommand::Start);
        for _ in 0..180 {
            self.tick();
            if self.lift().0.blocked {
                return;
            }
        }
        panic!("the door never held");
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

#[test]
fn a_door_pinning_a_living_agent_holds_and_steering_can_read_the_yield() {
    let mut rig = Rig::agent_at_jamb(BlockPolicy::Displace);
    rig.close_until_held();
    assert_eq!(rig.movers.agent_yields(), &[(rig.rider, LIFT)]);
    rig.tick();
    assert!(rig.lift().0.blocked, "the door holds while the agent walks");
    assert!(
        rig.registry
            .borrow()
            .get_component::<AgentComponent>(rig.rider)
            .unwrap()
            .pin_escape
            .is_some(),
        "steering read the yield"
    );
    assert_eq!(
        rig.registry
            .borrow()
            .get_component::<HealthComponent>(rig.rider)
            .unwrap()
            .current,
        100.0
    );
}

// The Weapons stage kills after the blocking pass, so the pre-tick test is the
// first to see the corpse; it only adds yields, and the pass is what releases.
#[test]
fn an_agent_killed_after_the_pass_frees_its_door_in_the_next_pass_and_it_moves_the_tick_after() {
    let mut rig = Rig::agent_at_jamb(BlockPolicy::Displace);
    rig.close_until_held();
    rig.tick();
    assert!(rig.lift().0.blocked);
    rig.set_rider_health(0.0);

    let (_, held_at) = rig.lift();
    rig.tick();
    let (door, position) = rig.lift();
    assert_eq!(position, held_at, "held through its own mover tick");
    assert!(!door.blocked, "released by that tick's pass");
    assert!(rig.movers.agent_yields().is_empty());

    rig.tick();
    assert!(rig.lift().1.x < held_at.x, "and moves the tick after");
}

#[test]
fn a_door_closes_through_a_corpse_or_an_agent_with_a_queued_despawn() {
    for policy in [
        BlockPolicy::Displace,
        BlockPolicy::Stop,
        BlockPolicy::Reverse,
    ] {
        for despawning in [false, true] {
            let mut rig = Rig::agent_at_jamb(policy);
            if despawning {
                crate::impact_effects::despawn(
                    &mut rig.registry.borrow_mut(),
                    rig.rider,
                    Some(60_000.0),
                );
            } else {
                rig.set_rider_health(0.0);
            }
            rig.command(MoverCommand::Start);
            for _ in 0..150 {
                rig.tick();
                let (door, _) = rig.lift();
                assert!(!door.blocked && door.direction_sign == 1, "{policy:?}");
            }
            assert!(rig.lift().0.completed, "{policy:?}: the door closed");
        }
    }
}

// --- Stepping clear --------------------------------------------------------------

impl Rig {
    fn door_position(&self) -> Vec3 {
        self.lift().1
    }

    fn agent_position(&self) -> Vec3 {
        self.registry
            .borrow()
            .get_component::<Transform>(self.rider)
            .unwrap()
            .position
    }

    fn agent_health(&self) -> f32 {
        self.registry
            .borrow()
            .get_component::<HealthComponent>(self.rider)
            .unwrap()
            .current
    }

    fn with_agent(&mut self, change: impl FnOnce(&mut AgentComponent)) {
        let mut registry = self.registry.borrow_mut();
        let mut agent = registry
            .get_component::<AgentComponent>(self.rider)
            .unwrap()
            .clone();
        change(&mut agent);
        registry.set_component(self.rider, agent).unwrap();
    }

    /// Tick until the door docks at x = -1, for at most `ticks`. Returns how
    /// long it took.
    fn ticks_until_docked(&mut self, ticks: u32) -> Option<u32> {
        (1..=ticks).find(|_| {
            self.tick();
            self.door_position().x <= -1.0 + 1.0e-3
        })
    }
}

const TWO_SECONDS: u32 = 120;

#[test]
fn a_pinned_agent_steps_clear_and_the_door_docks_within_two_seconds() {
    for policy in [BlockPolicy::Displace, BlockPolicy::Stop] {
        let mut rig = Rig::agent_at_jamb(policy);
        rig.close_until_held();
        assert!(
            rig.ticks_until_docked(TWO_SECONDS).is_some(),
            "{policy:?}: docked within 2 s of the first pin, door at {:?}",
            rig.door_position()
        );
        let at = rig.agent_position();
        assert!(at.z.abs() > 1.5, "{policy:?}: out of the course, at {at:?}");
        assert_eq!(rig.agent_health(), 100.0, "{policy:?}: unharmed");
    }
}

#[test]
fn a_pinned_agent_steps_clear_whatever_held_it_still() {
    // Blocked with a destination, or with no steering and no knockback at all.
    let latched = |agent: &mut AgentComponent| {
        agent.destination = Some(Vec3::new(8.0, 1.2, 0.0));
        agent.blocked = true;
    };
    let frozen = |agent: &mut AgentComponent| {
        agent.knockback.scale = 0.0;
        agent.knockback.control = 0.0;
    };
    let changes: [&dyn Fn(&mut AgentComponent); 2] = [&latched, &frozen];
    for change in changes {
        let mut rig = Rig::agent_at_jamb(BlockPolicy::Displace);
        rig.with_agent(change);
        rig.close_until_held();
        assert!(rig.ticks_until_docked(TWO_SECONDS).is_some());
        assert_eq!(rig.agent_health(), 100.0);
    }
}

#[test]
fn stepping_clear_drops_blocked_and_resumes_authored_motion() {
    let mut rig = Rig::agent_at_jamb(BlockPolicy::Displace);
    let goal = Vec3::new(8.0, 1.2, 0.0);
    rig.with_agent(|agent| {
        agent.destination = Some(goal);
        agent.blocked = true;
    });
    rig.close_until_held();
    rig.tick();
    let agent_then = rig
        .registry
        .borrow()
        .get_component::<AgentComponent>(rig.rider)
        .unwrap()
        .clone();
    assert!(agent_then.pin_escape.is_some(), "it is walking clear");
    assert!(!agent_then.blocked, "the latch dropped");
    for _ in 0..TWO_SECONDS {
        rig.tick();
    }
    let agent_after = rig
        .registry
        .borrow()
        .get_component::<AgentComponent>(rig.rider)
        .unwrap()
        .clone();
    assert_eq!(agent_after.pin_escape, None, "the walk ended");
    assert_eq!(agent_after.destination, Some(goal), "its order stands");
}

#[test]
fn with_no_clear_point_a_pinned_agent_yields_as_a_corpse_unharmed() {
    let mut rig = Rig::agent_at_jamb(BlockPolicy::Displace);
    // Walls either side of the agent make a pit the door closes into.
    rig.world = CollisionWorld::from_triangles_for_test(
        vec![
            Vec3::new(-10.0, 0.0, -10.0),
            Vec3::new(10.0, 0.0, -10.0),
            Vec3::new(10.0, 0.0, 10.0),
            Vec3::new(-10.0, 0.0, 10.0),
            Vec3::new(-1.0, 0.0, -3.0),
            Vec3::new(-1.0, 3.0, -3.0),
            Vec3::new(-1.0, 3.0, 3.0),
            Vec3::new(-1.0, 0.0, 3.0),
            Vec3::new(-1.0, 0.0, 0.45),
            Vec3::new(4.0, 0.0, 0.45),
            Vec3::new(4.0, 3.0, 0.45),
            Vec3::new(-1.0, 3.0, 0.45),
            Vec3::new(-1.0, 0.0, -0.45),
            Vec3::new(4.0, 0.0, -0.45),
            Vec3::new(4.0, 3.0, -0.45),
            Vec3::new(-1.0, 3.0, -0.45),
        ],
        vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [8, 9, 10],
            [8, 10, 11],
            [12, 14, 13],
            [12, 15, 14],
        ],
    );
    rig.close_until_held();
    assert!(
        rig.ticks_until_docked(TWO_SECONDS).is_some(),
        "the door docked, at {:?}",
        rig.door_position()
    );
    assert_eq!(rig.agent_health(), 100.0, "embedded and unharmed");
}

impl Rig {
    /// A `reverse` `ping_pong` lift whose deck spans x and z in [-1, 1] and
    /// docks at y = 0 and y = 2.5, beside a lower floor's slab that starts at
    /// x = 1.05 with its top at y = 0.25, above the bottom stop. A living agent
    /// rides the deck's centre; the lift is parked until started.
    fn agent_on_reverse_lift() -> Self {
        let (mut points, mut tris) = (Vec::new(), Vec::new());
        let mut quad = |corners: [Vec3; 4]| {
            let base = points.len() as u32;
            points.extend(corners);
            tris.push([base, base + 2, base + 1]);
            tris.push([base, base + 3, base + 2]);
        };
        // The slab's top, facing up, and its face toward the shaft, facing -X.
        quad([
            Vec3::new(1.05, 0.25, -5.0),
            Vec3::new(6.0, 0.25, -5.0),
            Vec3::new(6.0, 0.25, 5.0),
            Vec3::new(1.05, 0.25, 5.0),
        ]);
        quad([
            Vec3::new(1.05, -1.0, -5.0),
            Vec3::new(1.05, 0.25, -5.0),
            Vec3::new(1.05, 0.25, 5.0),
            Vec3::new(1.05, -1.0, 5.0),
        ]);
        let world = CollisionWorld::from_triangles_for_test(points, tris);
        let deck = MoverCollider::from_local_triangles(
            LIFT,
            &[
                Vec3::new(-1.0, 0.0, -1.0),
                Vec3::new(1.0, 0.0, -1.0),
                Vec3::new(1.0, 0.0, 1.0),
                Vec3::new(-1.0, 0.0, 1.0),
            ],
            &[[0, 2, 1], [0, 3, 2]],
        )
        .expect("deck collider is valid");
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (lift, agent) = {
            let mut registry = registry.borrow_mut();
            let lift = registry.spawn(Transform::default());
            let mut mover = KinematicMoverComponent::new(
                LIFT,
                KinematicMoverConfig {
                    waypoints: vec![Vec3::ZERO, Vec3::new(0.0, 2.5, 0.0)],
                    waypoint_names: vec!["bottom".into(), "top".into()],
                    speed_mps: 2.0,
                    wait_ms: 500.0,
                    mode: KinematicMoverMode::PingPong,
                    started: false,
                    spin_axis: Vec3::ZERO,
                    initial_spin_rate_rad_s: 0.0,
                    spin_accel_rad_s2: 0.0,
                    carry_yaw: false,
                },
            );
            mover.block_policy = BlockPolicy::Reverse;
            registry.set_component(lift, mover).unwrap();
            let agent = registry.spawn(Transform {
                position: Vec3::new(0.0, 1.2 + SKIN_DISTANCE, 0.0),
                ..Transform::default()
            });
            let mut component = AgentComponent::new(0.4, 2.4, 0.3, 4.0);
            component.ground = GroundRef::Mover(LIFT);
            registry.set_component(agent, component).unwrap();
            (lift, agent)
        };
        let mut rig = Self {
            registry,
            world,
            colliders: vec![deck],
            movers: MoverTickStateTable::default(),
            hit_zones: HitZoneStore::new(),
            progress: ProgressTracker::new(),
            lift,
            rider: agent,
        };
        rig.set_rider_health(100.0);
        rig
    }

    fn agent(&self) -> AgentComponent {
        self.registry
            .borrow()
            .get_component::<AgentComponent>(self.rider)
            .unwrap()
            .clone()
    }

    fn set_agent_position(&self, position: Vec3) {
        let mut registry = self.registry.borrow_mut();
        let mut transform = *registry.get_component::<Transform>(self.rider).unwrap();
        transform.position = position;
        registry.set_component(self.rider, transform).unwrap();
    }
}

// Regression: a step clear ended as soon as the agent stood on its pinning
// mover and that mover was not held. A `reverse` mover yields by backing off,
// never holding, so the walk ended on its first tick; the rider stayed at the
// rim and the lift shuttled back into the same pin every cycle.
#[test]
fn a_rider_pinned_at_the_rim_of_a_reverse_lift_walks_clear_and_the_lift_then_passes_the_pin() {
    let mut rig = Rig::agent_on_reverse_lift();
    for _ in 0..20 {
        rig.tick();
    }
    assert_eq!(rig.agent().ground, GroundRef::Mover(LIFT), "it rides");
    rig.command(MoverCommand::Start);
    for _ in 0..60 {
        if rig.door_position().y > 1.0 {
            break;
        }
        rig.tick();
    }
    // Raised clear of the slab, the rider moves to the rim. Its capsule
    // overhangs the slab's edge too little to stand on it, so the deck stays
    // its floor, and the way down pins it against that edge.
    let at = rig.agent_position();
    rig.set_agent_position(Vec3::new(0.7, at.y, 0.0));

    let mut ticks = 0;
    while rig.agent().pin_escape.is_none() {
        assert!(ticks < 300, "the descent pinned the rider");
        rig.tick();
        ticks += 1;
    }
    let pinned_at = rig.door_position().y;
    assert!(
        pinned_at > 0.0 && pinned_at < 0.25,
        "pinned on the way down, short of the bottom stop, at {pinned_at}"
    );
    let escape = rig.agent().pin_escape.unwrap();
    let spot = escape
        .deck_offset
        .expect("an inward spot on the deck rides clear both ways");

    // The lift backs off while the rider walks; the walk runs to the spot.
    let mut ticks = 0;
    while rig.agent().pin_escape.is_some() {
        assert!(
            ticks < 90,
            "the walk arrives, at {:?}",
            rig.agent_position()
        );
        rig.tick();
        ticks += 1;
    }
    let on_deck = rig.agent_position() - rig.door_position();
    assert!(
        crate::nav::distance_xz(on_deck, spot) <= 0.12,
        "at the deck spot {spot:?}, at {on_deck:?} on the deck"
    );

    // The lift finishes backing off, turns at its top stop and runs its leg
    // down past where it pinned the rider, to the bottom stop.
    let mut ticks = 0;
    while rig.door_position().y > 1.0e-3 {
        assert!(
            ticks < 360,
            "the lift reached its bottom stop, at {:?}",
            rig.door_position()
        );
        rig.tick();
        ticks += 1;
        assert!(rig.agent().pin_escape.is_none(), "never pinned again");
        assert!(!rig.lift().0.blocked, "never held");
    }
    assert_eq!(rig.agent().ground, GroundRef::Mover(LIFT), "still aboard");
    assert_eq!(rig.agent_health(), 100.0, "unharmed");
}

impl Rig {
    /// Re-seat the door at `closed`, with `open` beyond it.
    fn seat_door(&mut self, closed: Vec3, open: Vec3) {
        let mut registry = self.registry.borrow_mut();
        let mut door = registry
            .get_component::<KinematicMoverComponent>(self.lift)
            .unwrap()
            .clone();
        door.waypoints = vec![closed, open];
        registry.set_component(self.lift, door).unwrap();
        let mut transform = *registry.get_component::<Transform>(self.lift).unwrap();
        transform.position = closed;
        registry.set_component(self.lift, transform).unwrap();
    }
}

#[test]
fn a_corpse_revived_in_a_closed_door_steps_clear_and_the_door_then_opens() {
    let mut rig = Rig::agent_at_jamb(BlockPolicy::Stop);
    // Closed through the corpse's body, opening onto it against the wall.
    rig.seat_door(Vec3::new(-0.4, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0));
    rig.set_rider_health(0.0);
    for _ in 0..10 {
        rig.tick();
    }
    rig.set_rider_health(100.0);
    rig.command(MoverCommand::Start);
    rig.tick();
    rig.tick();
    assert!(rig.lift().0.blocked, "the revived agent holds the door");
    assert!(
        rig.ticks_until_docked(TWO_SECONDS).is_some(),
        "the door docked open, at {:?}",
        rig.door_position()
    );
    let at = rig.agent_position();
    assert!(
        at.z.abs() > 1.5 || at.x - 0.4 > -0.4,
        "out of the door's course, at {at:?}"
    );
    assert_eq!(rig.agent_health(), 100.0);
}

#[test]
fn a_crush_immune_agent_is_pushed_clear_once_the_crusher_stops_pinning_it() {
    use postretro_foundation::defense::{DamageTypeDeclaration, DefenseVocabulary, PoolMultiplier};

    let mut rig = Rig::agent_at_jamb(BlockPolicy::Crush);
    rig.registry
        .borrow_mut()
        .set_defense_vocabulary(std::sync::Arc::new(DefenseVocabulary {
            layers: Vec::new(),
            damage_types: vec![DamageTypeDeclaration {
                name: "crush".to_string(),
                matrix: std::collections::BTreeMap::from([(
                    "health".to_string(),
                    PoolMultiplier::Scale(0.0),
                )]),
            }],
            default_damage_type: None,
        }));
    // The crusher docks through the agent's axis, then stops pressing it.
    let closed = Vec3::new(-0.6, 0.0, 0.0);
    {
        let mut registry = rig.registry.borrow_mut();
        let mut door = registry
            .get_component::<KinematicMoverComponent>(rig.lift)
            .unwrap()
            .clone();
        door.waypoints[1] = closed;
        door.crush_damage = 10.0;
        door.crush_interval_ms = 100.0;
        door.crush_damage_type = Some("crush".to_string());
        registry.set_component(rig.lift, door).unwrap();
    }
    rig.command(MoverCommand::Start);
    for _ in 0..240 {
        rig.tick();
    }
    assert!(rig.lift().0.completed, "the crusher never held");
    assert_eq!(rig.agent_health(), 100.0, "immune");
    let at = rig.agent_position();
    assert!(
        at.x - 0.4 >= closed.x - 1.0e-2,
        "pushed clear of the leaf, at {at:?}"
    );
}
