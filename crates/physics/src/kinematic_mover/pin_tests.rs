// Pinned-body rule: no mover but `crush` leaves a body pinned against static
// geometry or another mover. Driver, movement and pass run in host tick order.
// See: context/lib/movement.md §6

use glam::Vec3;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::{BlockPolicy, EntityId, KinematicMoverMode};

use super::MoverAutoCloseTimers;
use super::pin_test_fixtures::*;
use crate::collision::{OVERLAP_TOLERANCE, SKIN_DISTANCE};
use postretro_entities::MoverCommand;

const LIFT: u32 = 1;
const DOOR: u32 = 2;
const LEAF: u32 = 3;

fn give_health(rig: &mut Rig, pawn: EntityId, current: f32) {
    rig.registry
        .set_component(
            pawn,
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
        .expect("health attaches");
}

fn health(rig: &Rig, pawn: EntityId) -> f32 {
    rig.registry
        .get_component::<HealthComponent>(pawn)
        .expect("health is attached")
        .current
}

/// A lift rising from y = 0 to `top` under a ceiling at `ceiling`, with a rider
/// settled on its deck.
fn low_ceiling_lift(policy: BlockPolicy, top: f32, ceiling: f32) -> (Rig, EntityId, EntityId) {
    let world = static_boxes(&[(
        Vec3::new(-10.0, ceiling, -10.0),
        Vec3::new(10.0, ceiling + 1.0, 10.0),
    )]);
    let mut rig = Rig::new(world);
    let lift = rig.add_mover(
        deck(LIFT, 1.5),
        vec![Vec3::ZERO, Vec3::new(0.0, top, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        policy,
    );
    let rider = rig.add_pawn(Vec3::new(0.0, rest_y(0.0), 0.0));
    rig.settle(20);
    rig.assert_rides(rider, LIFT);
    (rig, lift, rider)
}

/// A door leaf 2 m wide in X, closing along -X from x = 3 to x = `closed_x`
/// across a doorway at z = 0.
fn door_leaf(mover_id: u32) -> crate::collision::moving::MoverCollider {
    box_collider(
        mover_id,
        &[(Vec3::new(0.0, 0.0, -0.15), Vec3::new(2.0, 3.0, 0.15))],
    )
}

// --- Pinned bodies, first reproduced -------------------------------------------------------------

// Regression: an unswept carry under `displace` lifts a rider into a too-low ceiling,
// and nothing frees it once the lift stops.
#[test]
fn default_lift_never_carries_its_rider_into_a_low_ceiling() {
    let (mut rig, lift, rider) = low_ceiling_lift(BlockPolicy::Displace, 3.0, 4.6);
    rig.start(lift);
    for tick in 0..240 {
        rig.tick();
        let depth = rig.static_overlap(rider);
        assert!(
            depth <= OVERLAP_TOLERANCE,
            "tick {tick} left the rider {depth} m inside the ceiling"
        );
    }
}

// Regression: a default door closing on a pawn at a static jamb refuses the push and
// keeps closing, leaving the pawn inside its hull.
#[test]
fn default_door_never_closes_into_a_pawn_at_its_jamb() {
    let world = static_boxes(&[
        floor(),
        (Vec3::new(-2.0, 0.0, -2.0), Vec3::new(-1.0, 3.0, 2.0)),
    ]);
    let mut rig = Rig::new(world);
    let door = rig.add_mover(
        door_leaf(DOOR),
        vec![Vec3::new(3.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    let pawn = rig.add_pawn(Vec3::new(-0.5, rest_y(0.0), 0.0));
    rig.settle(20);
    rig.start(door);
    for tick in 0..180 {
        rig.tick();
        assert!(
            !rig.inside_mover_hull(pawn),
            "tick {tick} left the pawn inside the door at {:?}",
            rig.position(pawn)
        );
    }
}

// Regression: two default leaves meeting on a pawn push it back and forth and leave it
// inside one leaf; another mover never counts as a pin blocker.
#[test]
fn default_double_door_never_traps_a_pawn_in_a_leaf() {
    let mut rig = Rig::new(static_boxes(&[floor()]));
    let right = rig.add_mover(
        box_collider(
            DOOR,
            &[(Vec3::new(0.0, 0.0, -1.0), Vec3::new(2.0, 3.0, 1.0))],
        ),
        vec![Vec3::new(3.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    let left = rig.add_mover(
        box_collider(
            LEAF,
            &[(Vec3::new(-2.0, 0.0, -1.0), Vec3::new(0.0, 3.0, 1.0))],
        ),
        vec![Vec3::new(-3.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    let pawn = rig.add_pawn(Vec3::new(0.0, rest_y(0.0), 0.0));
    rig.settle(20);
    rig.start(right);
    rig.start(left);
    for tick in 0..180 {
        rig.tick();
        assert!(
            !rig.inside_mover_hull(pawn),
            "tick {tick} left the pawn inside a leaf at {:?}",
            rig.position(pawn)
        );
    }
}

/// A platform sliding +X under a static wall whose underside clears the deck
/// but not a standing rider.
fn sideways_crusher_rig(damage: f32) -> (Rig, EntityId, EntityId) {
    let world = static_boxes(&[
        floor(),
        (Vec3::new(3.0, 0.6, -5.0), Vec3::new(4.0, 6.0, 5.0)),
    ]);
    let mut rig = Rig::new(world);
    let platform = rig.add_mover(
        box_collider(
            LIFT,
            &[(Vec3::new(-1.0, -0.2, -1.0), Vec3::new(1.0, 0.0, 1.0))],
        ),
        vec![Vec3::new(0.0, 0.2, 0.0), Vec3::new(6.0, 0.2, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Crush,
    );
    rig.edit_mover(platform, |mover| {
        mover.crush_damage = damage;
        mover.crush_interval_ms = 100.0;
    });
    let rider = rig.add_pawn(Vec3::new(0.0, rest_y(0.2), 0.0));
    give_health(&mut rig, rider, 100.0);
    rig.settle(20);
    rig.assert_rides(rider, LIFT);
    (rig, platform, rider)
}

/// A crusher door closing along -X onto a pawn standing against a second,
/// closed mover rather than static geometry.
fn two_mover_crusher_rig(damage: f32) -> (Rig, EntityId, EntityId) {
    let mut rig = Rig::new(static_boxes(&[floor()]));
    let crusher = rig.add_mover(
        box_collider(
            DOOR,
            &[(Vec3::new(0.0, 0.0, -1.0), Vec3::new(2.0, 3.0, 1.0))],
        ),
        vec![Vec3::new(3.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Crush,
    );
    rig.edit_mover(crusher, |mover| {
        mover.crush_damage = damage;
        mover.crush_interval_ms = 100.0;
    });
    rig.add_mover(
        box_collider(
            LEAF,
            &[(Vec3::new(-2.0, 0.0, -2.0), Vec3::new(-1.0, 3.0, 2.0))],
        ),
        vec![Vec3::ZERO, Vec3::new(0.0, 0.0, 4.0)],
        1.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    let pawn = rig.add_pawn(Vec3::new(-0.5, rest_y(0.0), 0.0));
    give_health(&mut rig, pawn, 100.0);
    rig.settle(20);
    (rig, crusher, pawn)
}

// Regression: crush's pin test reads only its own push against static geometry, so a
// rider carried sideways into a wall is never crushed.
#[test]
fn crusher_crushes_a_rider_carried_sideways_into_a_wall() {
    let (mut rig, platform, rider) = sideways_crusher_rig(10.0);
    rig.start(platform);
    for _ in 0..120 {
        rig.tick();
    }
    assert!(
        health(&rig, rider) < 100.0,
        "the crusher carried its rider into the wall unharmed at {:?}",
        rig.position(rider)
    );
}

// Regression: crush never counts another mover as a pin blocker, so a body squeezed
// between the crusher and a second mover is never crushed.
#[test]
fn crusher_crushes_a_pawn_squeezed_against_another_mover() {
    let (mut rig, crusher, pawn) = two_mover_crusher_rig(10.0);
    rig.start(crusher);
    for _ in 0..120 {
        rig.tick();
    }
    assert!(
        health(&rig, pawn) < 100.0,
        "the crusher squeezed the pawn against a mover unharmed at {:?}",
        rig.position(pawn)
    );
}

// Regression: a reversal that brings a `once` mover back to its start leaves it
// started, so every later start no-ops.
#[test]
fn reversed_once_door_runs_again_on_its_next_start() {
    let mut rig = Rig::new(static_boxes(&[floor()]));
    let door = rig.add_mover(
        door_leaf(DOOR),
        vec![Vec3::new(3.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Reverse,
    );
    let pawn = rig.add_pawn(Vec3::new(-0.5, rest_y(0.0), 0.0));
    rig.settle(20);
    rig.start(door);
    for _ in 0..240 {
        rig.tick();
    }
    assert!(
        (rig.mover_position(door) - Vec3::new(3.0, 0.0, 0.0)).length() < 1.0e-3,
        "the door backed off the pawn to its start"
    );
    assert!(!rig.mover(door).started, "and ended unstarted");
    rig.set_position(pawn, Vec3::new(-8.0, rest_y(0.0), 8.0));
    rig.settle(5);
    rig.start(door);
    for _ in 0..180 {
        rig.tick();
    }
    assert!(
        (rig.mover_position(door) - Vec3::new(-1.0, 0.0, 0.0)).length() < 1.0e-3,
        "the trigger's next start left the door at {:?}",
        rig.mover_position(door)
    );
}

// --- Unpinned motion unchanged ---------------------------------------------------

/// Pawn positions after a default slider pushes a pawn across open floor, and
/// after a rider rides a default lift with headroom.
fn unpinned_baseline_positions() -> (Vec3, Vec3) {
    let mut rig = Rig::new(static_boxes(&[floor()]));
    let slider = rig.add_mover(
        box_collider(
            DOOR,
            &[(Vec3::new(-1.0, 0.0, -1.0), Vec3::new(0.0, 2.0, 1.0))],
        ),
        vec![Vec3::new(-1.0, 0.0, 0.0), Vec3::new(6.0, 0.0, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    let pushed = rig.add_pawn(Vec3::new(1.0, rest_y(0.0), 0.0));
    rig.settle(20);
    rig.start(slider);
    for _ in 0..150 {
        rig.tick();
    }
    let pushed_end = rig.position(pushed);

    let mut rig = Rig::new(static_boxes(&[(
        Vec3::new(40.0, 0.0, 40.0),
        Vec3::new(41.0, 1.0, 41.0),
    )]));
    let lift = rig.add_mover(
        deck(LIFT, 1.5),
        vec![Vec3::ZERO, Vec3::new(0.0, 3.0, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    let rider = rig.add_pawn(Vec3::new(0.0, rest_y(0.0), 0.0));
    rig.settle(20);
    rig.assert_rides(rider, LIFT);
    rig.start(lift);
    for _ in 0..150 {
        rig.tick();
    }
    (pushed_end, rig.position(rider))
}

#[test]
fn unpinned_slider_push_and_headroom_lift_match_baseline() {
    // Captured at ab3caf5, before the pinned-body rule: bit-exact pawn
    // positions for the two unpinned cases the rule must leave alone.
    let (pushed, rider) = unpinned_baseline_positions();
    let bits = |v: Vec3| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
    assert_eq!(
        bits(pushed),
        [1083053180, 1067198710, 0],
        "slider-pushed pawn moved off its baseline: {pushed:?}"
    );
    assert_eq!(
        bits(rider),
        [0, 1082591802, 0],
        "headroom rider moved off its baseline: {rider:?}"
    );
}

// --- No non-crush mover leaves a body pinned -------------------------------------

/// One tick's travel of a 2 m/s mover: the most a hold leaves a body inside it.
const TICK_TRAVEL: f32 = 2.0 * DT;

fn assert_clear_of_static(rig: &Rig, pawn: EntityId, what: &str) {
    let depth = rig.static_overlap(pawn);
    assert!(
        depth <= OVERLAP_TOLERANCE,
        "{what}: the pawn ended {depth} m inside static geometry at {:?}",
        rig.position(pawn)
    );
}

#[test]
fn rider_lifted_toward_low_ceiling_never_embeds_under_any_yielding_policy() {
    for policy in [
        BlockPolicy::Displace,
        BlockPolicy::Stop,
        BlockPolicy::Reverse,
    ] {
        let (mut rig, lift, rider) = low_ceiling_lift(policy, 3.0, 4.6);
        rig.start(lift);
        for _ in 0..240 {
            rig.tick();
            assert_clear_of_static(&rig, rider, &format!("{policy:?}"));
        }
        let lift_y = rig.mover_position(lift).y;
        if policy == BlockPolicy::Reverse {
            assert!(
                lift_y.abs() < 1.0e-3,
                "reverse carried its rider back down, lift at {lift_y}"
            );
        } else {
            assert!(
                rig.mover(lift).blocked && lift_y > 1.0 && lift_y < 3.0,
                "{policy:?} holds below its stop, lift at {lift_y}"
            );
        }
    }
}

/// A lift whose deck spans x in [-1, 1], rising past an upper-floor slab whose
/// underside edge starts at x = 1.1; `waypoints` are the deck heights.
fn overhang_rig(waypoints: Vec<f32>) -> (Rig, EntityId, EntityId) {
    let top = *waypoints.last().unwrap();
    let world = static_boxes(&[(Vec3::new(1.1, top - 0.5, -5.0), Vec3::new(6.0, top, 5.0))]);
    let mut rig = Rig::new(world);
    let lift = rig.add_mover(
        deck(LIFT, 1.0),
        waypoints.iter().map(|y| Vec3::new(0.0, *y, 0.0)).collect(),
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    let rider = rig.add_pawn(Vec3::new(0.8, rest_y(0.0), 0.0));
    rig.settle(20);
    rig.assert_rides(rider, LIFT);
    (rig, lift, rider)
}

/// Hold the lift for its overhanging rider, then walk the rider inward:
/// asserts the lift held while the rider overhung the slab and rose on the
/// tick after the rider cleared it, then leaves the rider idle.
fn walk_inward_until_resumed(rig: &mut Rig, lift: EntityId, rider: EntityId) {
    for _ in 0..30 {
        rig.tick();
        assert_clear_of_static(rig, rider, "overhang");
    }
    assert!(
        rig.mover(lift).blocked,
        "the lift holds for its overhanging rider"
    );
    rig.set_input(rider, walk(Vec3::NEG_X));
    let mut cleared_at_end_of_previous = false;
    for tick in 0..120 {
        let before = rig.mover_position(lift).y;
        rig.tick();
        let rose = rig.mover_position(lift).y > before;
        assert_clear_of_static(rig, rider, "overhang walk-in");
        // The rider's rounded cap can let the lift creep up while it still
        // overhangs; the cast decides, so only the release is timed here.
        let x_reach = rig.position(rider).x + RADIUS;
        if cleared_at_end_of_previous {
            assert!(
                rose,
                "tick {tick}: the lift resumes the tick after the rider clears"
            );
            rig.set_input(rider, idle());
            return;
        }
        cleared_at_end_of_previous = x_reach < 1.1 - SKIN_DISTANCE - 1.0e-3;
    }
    panic!("the rider never cleared the slab");
}

#[test]
fn overhanging_rider_holds_lift_then_resumes_the_tick_after_stepping_inward() {
    let (mut rig, lift, rider) = overhang_rig(vec![0.0, 3.0]);
    rig.start(lift);
    walk_inward_until_resumed(&mut rig, lift, rider);
    for _ in 0..120 {
        rig.tick();
        assert_clear_of_static(&rig, rider, "overhang ride");
    }
    let lift_state = rig.mover(lift);
    assert!(
        lift_state.completed && (rig.mover_position(lift).y - 3.0).abs() < 1.0e-3,
        "the lift reached its top stop"
    );
}

#[test]
fn held_go_to_path_node_keeps_target_and_arrives() {
    let (mut rig, lift, rider) = overhang_rig(vec![0.0, 3.0, 3.0001]);
    // Three nodes: the target is the middle one, so arrival is the target's.
    rig.command(lift, MoverCommand::GoToPathNode("node1".to_string()));
    walk_inward_until_resumed(&mut rig, lift, rider);
    assert_eq!(
        rig.mover(lift).target_segment,
        Some(1),
        "the hold kept the target"
    );
    for _ in 0..120 {
        rig.tick();
    }
    let lift_state = rig.mover(lift);
    assert!(
        lift_state.completed
            && lift_state.segment_index == 1
            && (rig.mover_position(lift).y - 3.0).abs() < 1.0e-3,
        "the lift reached its scripted target: {:?}",
        rig.mover_position(lift)
    );
}

#[test]
fn rider_under_low_arch_never_embeds() {
    for policy in [
        BlockPolicy::Displace,
        BlockPolicy::Stop,
        BlockPolicy::Reverse,
    ] {
        let world = static_boxes(&[(Vec3::new(3.0, 1.8, -5.0), Vec3::new(4.0, 6.0, 5.0))]);
        let mut rig = Rig::new(world);
        let platform = rig.add_mover(
            deck(LIFT, 1.0),
            vec![Vec3::ZERO, Vec3::new(6.0, 0.0, 0.0)],
            2.0,
            KinematicMoverMode::Once,
            policy,
        );
        let rider = rig.add_pawn(Vec3::new(0.0, rest_y(0.0), 0.0));
        rig.settle(20);
        rig.assert_rides(rider, LIFT);
        rig.start(platform);
        for _ in 0..240 {
            rig.tick();
            assert_clear_of_static(&rig, rider, &format!("arch {policy:?}"));
        }
        let x = rig.mover_position(platform).x;
        if policy == BlockPolicy::Reverse {
            assert!(x.abs() < 1.0e-3, "reverse backed off to its start, at {x}");
        } else {
            assert!(
                rig.mover(platform).blocked,
                "{policy:?} holds under the arch"
            );
        }
    }
}

/// A carousel deck spinning about +Y at the origin with a rider on its rim at
/// +X, and a static pillar too low to pass a standing rider a quarter turn
/// along the rider's travel. `path` gives the carousel a slow path, so a
/// `reverse` carousel has a heading.
fn carousel_rig(policy: BlockPolicy, path: bool) -> (Rig, EntityId, EntityId) {
    // Positive spin about +Y turns +X toward -Z.
    let world = static_boxes(&[(Vec3::new(-0.4, 1.0, -2.4), Vec3::new(0.4, 6.0, -1.6))]);
    let mut rig = Rig::new(world);
    let waypoints = if path {
        vec![Vec3::ZERO, Vec3::new(0.0, 0.0, 0.3)]
    } else {
        vec![Vec3::ZERO]
    };
    let carousel = rig.add_mover(
        deck(LIFT, 3.0),
        waypoints,
        0.05,
        KinematicMoverMode::Once,
        policy,
    );
    rig.edit_mover(carousel, |mover| {
        mover.spin_axis = Vec3::Y;
        mover.spin_rate_rad_s = 1.0;
        mover.spin_target_rate_rad_s = 1.0;
    });
    let rider = rig.add_pawn(Vec3::new(2.0, rest_y(0.0), 0.0));
    rig.settle(20);
    rig.assert_rides(rider, LIFT);
    (rig, carousel, rider)
}

#[test]
fn carousel_spin_holds_for_rider_under_pillar_under_every_policy() {
    for (policy, path) in [
        (BlockPolicy::Displace, false),
        (BlockPolicy::Stop, false),
        (BlockPolicy::Reverse, false),
        (BlockPolicy::Reverse, true),
    ] {
        let (mut rig, carousel, rider) = carousel_rig(policy, path);
        rig.start(carousel);
        for _ in 0..240 {
            rig.tick();
            assert_clear_of_static(&rig, rider, &format!("carousel {policy:?} path {path}"));
        }
        let state = rig.mover(carousel);
        assert!(
            state.blocked && state.spin_angle_rad > 0.3 && state.spin_angle_rad < 1.6,
            "{policy:?} (path {path}): the spin holds short of the pillar, angle {}",
            state.spin_angle_rad
        );
    }
}

#[test]
fn lift_holds_under_closed_hatch_and_resumes_after_it_opens() {
    let mut rig = Rig::new(static_boxes(&[(
        Vec3::new(30.0, 0.0, 30.0),
        Vec3::new(31.0, 1.0, 31.0),
    )]));
    let lift = rig.add_mover(
        deck(LIFT, 1.5),
        vec![Vec3::ZERO, Vec3::new(0.0, 3.0, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    let hatch = rig.add_mover(
        box_collider(
            LEAF,
            &[(Vec3::new(-2.0, 0.0, -2.0), Vec3::new(2.0, 0.3, 2.0))],
        ),
        vec![Vec3::new(0.0, 4.6, 0.0), Vec3::new(5.0, 4.6, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    let rider = rig.add_pawn(Vec3::new(0.0, rest_y(0.0), 0.0));
    rig.settle(20);
    rig.start(lift);
    for _ in 0..90 {
        rig.tick();
        assert!(
            rig.mover_overlap(rider) <= OVERLAP_TOLERANCE,
            "rider stays out of the hatch"
        );
    }
    assert!(
        rig.mover(lift).blocked,
        "the lift holds under the closed hatch"
    );
    rig.start(hatch);
    let mut clear_at_end_of_previous = false;
    let mut resumed = false;
    for tick in 0..200 {
        let before = rig.mover_position(lift).y;
        rig.tick();
        assert!(
            rig.mover_overlap(rider) <= OVERLAP_TOLERANCE,
            "tick {tick}: rider stays out of the hatch"
        );
        let rose = rig.mover_position(lift).y > before;
        if clear_at_end_of_previous && !rig.mover(lift).completed {
            assert!(
                rose,
                "tick {tick}: the lift resumes the tick after the hatch clears"
            );
            resumed = true;
            break;
        }
        // The hatch's near edge has passed the rider's far side.
        clear_at_end_of_previous = rig.mover_position(hatch).x - 2.0
            > rig.position(rider).x + RADIUS + SKIN_DISTANCE + 0.05;
    }
    assert!(resumed, "the hatch never cleared the rider's carry");
    for _ in 0..120 {
        rig.tick();
    }
    assert!(rig.mover(lift).completed, "the lift reached its top stop");
}

#[test]
fn airborne_rider_in_grace_caught_at_ceiling_counts_as_pinned() {
    for policy in [BlockPolicy::Displace, BlockPolicy::Stop] {
        // The rider has just jumped off its deck: airborne inside the grace
        // window, its head touching the ceiling, its feet half a tick of
        // travel above the deck that is about to rise.
        let raised = rest_y(0.0) + 0.5 * TICK_TRAVEL;
        let ceiling = raised + HALF_HEIGHT + RADIUS + SKIN_DISTANCE;
        let (mut rig, lift, rider) = low_ceiling_lift(policy, 3.0, ceiling);
        rig.set_position(rider, Vec3::new(0.0, raised, 0.0));
        rig.edit_pawn(rider, |component| {
            component.ground = postretro_foundation::GroundRef::Airborne;
            component.set_grounded(false);
            component.velocity = Vec3::ZERO;
        });
        rig.start(lift);
        let report = rig.tick();
        assert_eq!(
            report.blocked_events(LIFT),
            1,
            "{policy:?}: the airborne former rider counts as pinned"
        );
        assert!(
            rig.mover(lift).blocked && rig.mover_position(lift).y.abs() < 1.0e-6,
            "{policy:?}: the lift held before moving into it"
        );
        for _ in 0..30 {
            rig.tick();
            assert_clear_of_static(&rig, rider, &format!("grace {policy:?}"));
        }
    }
}

#[test]
fn one_pinned_pawn_holds_the_mover_in_either_visit_order() {
    for reversed_order in [false, true] {
        // A ceiling over x > 0.5 only: the rider at +X is pinned, the one at
        // -X has headroom.
        let world = static_boxes(&[(Vec3::new(0.5, 4.6, -5.0), Vec3::new(5.0, 6.0, 5.0))]);
        let mut rig = Rig::new(world);
        let lift = rig.add_mover(
            deck(LIFT, 3.0),
            vec![Vec3::ZERO, Vec3::new(0.0, 3.0, 0.0)],
            2.0,
            KinematicMoverMode::Once,
            BlockPolicy::Displace,
        );
        let free = rig.add_pawn(Vec3::new(-2.0, rest_y(0.0), 0.0));
        let pinned = rig.add_pawn(Vec3::new(2.0, rest_y(0.0), 0.0));
        if reversed_order {
            rig.reverse_pawn_order();
        }
        rig.settle(20);
        rig.start(lift);
        for _ in 0..180 {
            rig.tick();
            assert_clear_of_static(&rig, pinned, "pinned rider");
            assert_clear_of_static(&rig, free, "free rider");
        }
        assert!(
            rig.mover(lift).blocked,
            "order {reversed_order}: the lift holds"
        );
    }
}

#[test]
fn first_pinned_preview_holds_rider_in_place_or_reverses_it_away() {
    for policy in [BlockPolicy::Displace, BlockPolicy::Reverse] {
        let (mut rig, lift, rider) = low_ceiling_lift(policy, 3.0, 4.6);
        rig.start(lift);
        let pinned_end = (0..240)
            .find_map(|_| {
                let report = rig.tick();
                (report.blocked_events(LIFT) > 0).then(|| rig.position(rider))
            })
            .expect("the lift meets the pin");
        rig.tick();
        let next = rig.position(rider);
        if policy == BlockPolicy::Reverse {
            assert!(
                next.y < pinned_end.y,
                "reverse carried the rider away: {next:?}"
            );
        } else {
            assert!(
                (next - pinned_end).length() < 1.0e-4,
                "the hold left the rider where it ended: {pinned_end:?} then {next:?}"
            );
        }
    }
}

/// A `once` door closing along -X onto a pawn at a static jamb.
fn jamb_door_rig(policy: BlockPolicy) -> (Rig, EntityId, EntityId) {
    let world = static_boxes(&[
        floor(),
        (Vec3::new(-2.0, 0.0, -2.0), Vec3::new(-1.0, 3.0, 2.0)),
    ]);
    let mut rig = Rig::new(world);
    let door = rig.add_mover(
        door_leaf(DOOR),
        vec![Vec3::new(3.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        policy,
    );
    let pawn = rig.add_pawn(Vec3::new(-0.5, rest_y(0.0), 0.0));
    rig.settle(20);
    (rig, door, pawn)
}

/// Run until the movers hold, asserting no tick leaves the pawn deeper in a
/// mover than one tick's travel; then walk the pawn out along `away` and
/// assert every unfinished mover moves on the tick after `cleared` first holds
/// for the pawn's position, and completes.
fn hold_then_release(
    rig: &mut Rig,
    movers: &[EntityId],
    pawn: EntityId,
    away: Vec3,
    cleared: impl Fn(Vec3) -> bool,
    what: &str,
) {
    for &mover in movers {
        rig.start(mover);
    }
    for tick in 0..150 {
        rig.tick();
        let depth = rig.mover_overlap(pawn);
        assert!(
            depth <= TICK_TRAVEL + OVERLAP_TOLERANCE,
            "{what}: tick {tick} left the pawn {depth} m inside a mover"
        );
        assert!(
            !rig.inside_mover_hull(pawn),
            "{what}: tick {tick} pawn inside a hull"
        );
    }
    assert!(
        movers.iter().any(|mover| rig.mover(*mover).blocked),
        "{what}: a mover holds for the pinned pawn"
    );
    rig.set_input(pawn, walk(away));
    let mut cleared_at_end_of_previous = false;
    let mut resumed = false;
    for tick in 0..90 {
        let before: Vec<(Vec3, bool)> = movers
            .iter()
            .map(|mover| (rig.mover_position(*mover), rig.mover(*mover).completed))
            .collect();
        rig.tick();
        if cleared_at_end_of_previous {
            for (mover, (position, completed)) in movers.iter().zip(before) {
                assert!(
                    completed || rig.mover_position(*mover) != position,
                    "{what}: tick {tick}: mover {} resumes the tick after the pawn clears",
                    rig.mover(*mover).mover_id
                );
            }
            resumed = true;
            break;
        }
        cleared_at_end_of_previous = cleared(rig.position(pawn));
    }
    assert!(
        resumed,
        "{what}: the pawn never cleared the movers' courses"
    );
    rig.set_input(pawn, idle());
    for _ in 0..240 {
        rig.tick();
    }
    for &mover in movers {
        let state = rig.mover(mover);
        assert!(
            state.completed && !state.blocked,
            "{what}: mover {} completed after the pawn left",
            state.mover_id
        );
    }
}

#[test]
fn non_rider_pins_never_leave_more_than_one_tick_of_travel_and_resume() {
    let (mut rig, door, pawn) = jamb_door_rig(BlockPolicy::Displace);
    // The leaf spans z in [-0.15, 0.15].
    hold_then_release(
        &mut rig,
        &[door],
        pawn,
        Vec3::NEG_Z,
        |p| p.z + RADIUS < -0.15 - SKIN_DISTANCE - 1.0e-3,
        "jamb door",
    );

    // A lift descending onto a pawn standing under it.
    let mut rig = Rig::new(static_boxes(&[floor()]));
    let lift = rig.add_mover(
        deck(LIFT, 1.5),
        vec![Vec3::new(0.0, 4.0, 0.0), Vec3::new(0.0, 0.2, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    let pawn = rig.add_pawn(Vec3::new(0.0, rest_y(0.0), 0.0));
    rig.settle(20);
    hold_then_release(
        &mut rig,
        &[lift],
        pawn,
        Vec3::X,
        |p| p.x - RADIUS > 1.5 + SKIN_DISTANCE + 1.0e-3,
        "descending lift",
    );

    // Two thick leaves meeting on a pawn.
    let mut rig = Rig::new(static_boxes(&[floor()]));
    let right = rig.add_mover(
        box_collider(
            DOOR,
            &[(Vec3::new(0.0, 0.0, -1.0), Vec3::new(2.0, 3.0, 1.0))],
        ),
        vec![Vec3::new(3.0, 0.0, 0.0), Vec3::ZERO],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    let left = rig.add_mover(
        box_collider(
            LEAF,
            &[(Vec3::new(-2.0, 0.0, -1.0), Vec3::new(0.0, 3.0, 1.0))],
        ),
        vec![Vec3::new(-3.0, 0.0, 0.0), Vec3::ZERO],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    let pawn = rig.add_pawn(Vec3::new(0.0, rest_y(0.0), 0.0));
    rig.settle(20);
    // Both leaves span z in [-1, 1].
    hold_then_release(
        &mut rig,
        &[right, left],
        pawn,
        Vec3::Z,
        |p| p.z - RADIUS > 1.0 + SKIN_DISTANCE + 1.0e-3,
        "double door",
    );
}

#[test]
fn rotating_door_swinging_a_pawn_into_a_wall_holds_then_resumes() {
    // A leaf hinged at the origin, swinging from +X toward -Z, with a wall
    // behind a pawn standing in its sweep.
    let world = static_boxes(&[
        floor(),
        (Vec3::new(0.0, 0.0, -3.0), Vec3::new(4.0, 3.0, -2.2)),
    ]);
    let mut rig = Rig::new(world);
    let door = rig.add_mover(
        box_collider(
            DOOR,
            &[(Vec3::new(0.0, 0.0, -0.1), Vec3::new(2.0, 3.0, 0.1))],
        ),
        vec![Vec3::ZERO],
        0.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    rig.edit_mover(door, |mover| {
        mover.spin_axis = Vec3::Y;
        mover.spin_rate_rad_s = 1.0;
        mover.spin_target_rate_rad_s = 1.0;
    });
    let pawn = rig.add_pawn(Vec3::new(1.5, rest_y(0.0), -1.7));
    rig.settle(20);
    rig.start(door);
    for tick in 0..120 {
        rig.tick();
        let depth = rig.mover_overlap(pawn);
        // One tick of the leaf tip's arc: 2 m at 1 rad/s.
        assert!(
            depth <= 2.0 * DT + OVERLAP_TOLERANCE,
            "tick {tick}: the leaf left the pawn {depth} m inside it"
        );
        assert_clear_of_static(&rig, pawn, "rotating door");
    }
    let held_angle = rig.mover(door).spin_angle_rad;
    assert!(
        rig.mover(door).blocked,
        "the swing holds for the pinned pawn"
    );
    rig.set_input(pawn, walk(Vec3::X));
    // The leaf's far corners sit sqrt(2^2 + 0.1^2) m from the hinge.
    let leaf_reach = (2.0f32 * 2.0 + 0.1 * 0.1).sqrt();
    let mut cleared_at_end_of_previous = false;
    let mut resumed_at = None;
    for tick in 0..90 {
        let before = rig.mover(door).spin_angle_rad;
        rig.tick();
        if cleared_at_end_of_previous {
            assert!(
                rig.mover(door).spin_angle_rad > before,
                "tick {tick}: the swing resumes the tick after the pawn clears"
            );
            resumed_at = Some(tick);
            break;
        }
        let p = rig.position(pawn);
        cleared_at_end_of_previous =
            Vec3::new(p.x, 0.0, p.z).length() - RADIUS > leaf_reach + SKIN_DISTANCE + 1.0e-3;
    }
    assert!(resumed_at.is_some(), "the pawn never cleared the swing");
    rig.set_input(pawn, idle());
    for _ in 0..30 {
        rig.tick();
    }
    assert!(
        rig.mover(door).spin_angle_rad > held_angle + 0.4,
        "the swing carried on after the pawn left"
    );
}

#[test]
fn vault_onto_rising_mover_under_ceiling_never_embeds() {
    use postretro_foundation::{VaultEntry, VaultParams, VaultTrigger};
    let mut desc = descriptor();
    desc.vault = Some(VaultParams {
        max_height: 1.6,
        swim_max_height: 0.9,
        duration_ms: 320.0,
        from: VaultEntry {
            ground: Some(VaultTrigger::Auto),
            ..VaultEntry::default()
        },
        allow_fire: false,
    });
    // A block whose top starts 1.2 m up and rises, under a ceiling a rider on
    // it meets a short way into the climb.
    let world = static_boxes(&[
        floor(),
        (Vec3::new(4.0, 4.0, -5.0), Vec3::new(10.0, 5.0, 5.0)),
    ]);
    let mut rig = Rig::new(world);
    rig.use_descriptor(desc);
    let block = rig.add_mover(
        box_collider(
            LIFT,
            &[(Vec3::new(0.0, -1.2, -5.0), Vec3::new(5.0, 0.0, 5.0))],
        ),
        vec![Vec3::new(5.0, 1.2, 0.0), Vec3::new(5.0, 3.2, 0.0)],
        1.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    let pawn = rig.add_pawn(Vec3::new(3.5, rest_y(0.0), 0.0));
    rig.settle(20);
    rig.start(block);
    rig.set_input(pawn, walk(Vec3::X));
    let mut held_while_vaulting = false;
    for _ in 0..120 {
        rig.tick();
        let vaulting = matches!(
            rig.pawn(pawn).movement_state,
            postretro_foundation::MovementState::Vault {
                mover: Some(LIFT),
                ..
            }
        );
        held_while_vaulting |= vaulting && rig.mover(block).blocked;
        assert_clear_of_static(&rig, pawn, "vault onto rising mover");
    }
    assert!(
        held_while_vaulting,
        "the mover held for the pawn vaulting onto it"
    );
}

/// A lift rising with a rider toward a ceiling, and a pawn that walks under
/// the deck on the tick the rider meets the ceiling, so both the course and
/// its reversal pin a body. Returns the rig, lift, rider, pawn under it and
/// the Blocked edges fired through that tick.
fn pinned_both_ways_rig(policy: BlockPolicy) -> (Rig, EntityId, EntityId, EntityId, usize) {
    let pawn_top = rest_y(0.0) + HALF_HEIGHT + RADIUS;
    // On the pin tick the deck sits half a tick above the pawn's head and the
    // rider's head half a tick outside the ceiling's skin band; a tick
    // earlier the rider still had room.
    let pin_deck_y = pawn_top + 0.2 + 0.5 * TICK_TRAVEL;
    let ticks_to_pin = 30;
    let start_y = pin_deck_y - ticks_to_pin as f32 * TICK_TRAVEL;
    let ceiling =
        pin_deck_y + 2.0 * (HALF_HEIGHT + RADIUS) + 2.0 * SKIN_DISTANCE + 0.5 * TICK_TRAVEL;
    let world = static_boxes(&[
        floor(),
        (
            Vec3::new(-5.0, ceiling, -5.0),
            Vec3::new(5.0, ceiling + 1.0, 5.0),
        ),
    ]);
    let mut rig = Rig::new(world);
    let lift = rig.add_mover(
        deck(LIFT, 1.5),
        vec![Vec3::new(0.0, start_y, 0.0), Vec3::new(0.0, 6.0, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        policy,
    );
    let rider = rig.add_pawn(Vec3::new(0.0, rest_y(start_y), 0.0));
    rig.settle(20);
    rig.assert_rides(rider, LIFT);
    rig.start(lift);
    let mut under = None;
    let mut blocked_events = 0;
    for _ in 0..2 * ticks_to_pin {
        blocked_events += rig
            .tick_with(|rig| {
                if rig.mover_position(lift).y >= pin_deck_y - 1.0e-4 {
                    under = Some(rig.add_pawn(Vec3::new(0.0, rest_y(0.0), 0.0)));
                }
            })
            .blocked_events(LIFT);
        if under.is_some() {
            break;
        }
    }
    let under = under.expect("the pawn walked under the deck");
    (rig, lift, rider, under, blocked_events)
}

/// Run `ticks`, asserting the lift never changes direction, never moves onto
/// the pawn under it and never embeds its rider; returns the Blocked edges.
fn hold_pinned_both_ways(
    rig: &mut Rig,
    lift: EntityId,
    rider: EntityId,
    under: EntityId,
    ticks: usize,
    mut between_ticks: impl FnMut(&mut Rig, usize),
) -> usize {
    let mut blocked_events = 0;
    let mut direction = rig.mover(lift).direction_sign;
    for tick in 0..ticks {
        let fired = rig.tick().blocked_events(LIFT);
        blocked_events += fired;
        assert_eq!(
            rig.mover(lift).direction_sign,
            direction,
            "tick {tick}: the lift never flips"
        );
        assert!(
            rig.mover_overlap(under) <= OVERLAP_TOLERANCE,
            "tick {tick}: never onto the pawn under the deck"
        );
        assert_clear_of_static(rig, rider, "rider");
        between_ticks(rig, fired);
        direction = rig.mover(lift).direction_sign;
    }
    blocked_events
}

#[test]
fn reverse_lift_pinned_both_ways_holds_without_flipping_and_fires_once() {
    let (mut rig, lift, rider, under, fired) = pinned_both_ways_rig(BlockPolicy::Reverse);
    assert_eq!(
        rig.mover(lift).direction_sign,
        1,
        "the lift held rather than reversing onto the pawn"
    );
    let later = hold_pinned_both_ways(&mut rig, lift, rider, under, 60, |_, _| {});
    assert!(rig.mover(lift).blocked, "the lift holds");
    assert_eq!(fired + later, 1, "one blocked event for the whole pin");
}

#[test]
fn rider_yield_warns_once_per_mover_per_level_load() {
    let logs = postretro_test_log_capture::LogCapture::start();
    let (mut rig, lift, _rider) = low_ceiling_lift(BlockPolicy::Displace, 3.0, 4.6);
    rig.start(lift);
    for _ in 0..120 {
        rig.tick();
    }
    logs.assert_logged_once(log::Level::Warn, "mover 1 yielded to a rider pinned");
    logs.assert_logged_once(log::Level::Warn, "at TrenchBroom (");

    // Released and pinned again in the same level load: silent.
    rig.command(lift, MoverCommand::Stop);
    rig.tick();
    rig.start(lift);
    for _ in 0..30 {
        rig.tick();
    }
    logs.assert_logged_once(log::Level::Warn, "mover 1 yielded to a rider pinned");

    // A level reload resets the warning.
    rig.table.clear();
    rig.command(lift, MoverCommand::Stop);
    rig.tick();
    rig.start(lift);
    for _ in 0..30 {
        rig.tick();
    }
    let warnings = logs
        .records()
        .iter()
        .filter(|record| record.message.contains("mover 1 yielded to a rider pinned"))
        .count();
    assert_eq!(warnings, 2, "the reload warns again");
}

#[test]
fn co_moving_floor_and_roof_never_pin_a_rider_between_them() {
    // A cage lift: a floor and a roof mover on one path at one speed, the
    // rider's head 4 cm under the roof. That is less than a tick of travel
    // plus a skin, so a roof read at rest would pin the rider every tick.
    let headroom = 0.04;
    let roof_y = 2.0 * (HALF_HEIGHT + RADIUS) + SKIN_DISTANCE + headroom;
    let mut rig = Rig::new(static_boxes(&[(
        Vec3::new(30.0, 0.0, 30.0),
        Vec3::new(31.0, 1.0, 31.0),
    )]));
    let path = vec![Vec3::ZERO, Vec3::new(0.0, 3.0, 0.0)];
    let lift = rig.add_mover(
        deck(LIFT, 1.5),
        path.clone(),
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    let roof = rig.add_mover(
        box_collider(
            LEAF,
            &[(
                Vec3::new(-1.5, roof_y, -1.5),
                Vec3::new(1.5, roof_y + 0.3, 1.5),
            )],
        ),
        path,
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    let rider = rig.add_pawn(Vec3::new(0.0, rest_y(0.0), 0.0));
    rig.settle(20);
    rig.assert_rides(rider, LIFT);
    rig.start(lift);
    rig.start(roof);
    let mut fired = 0;
    for tick in 0..120 {
        let report = rig.tick();
        fired += report.blocked_events(LIFT) + report.blocked_events(LEAF);
        assert!(
            !rig.mover(lift).blocked && !rig.mover(roof).blocked,
            "tick {tick}: neither part of the cage holds"
        );
        assert!(
            rig.mover_overlap(rider) <= OVERLAP_TOLERANCE,
            "tick {tick}: the rider stays out of the roof"
        );
        assert!(
            (rig.mover_position(lift) - rig.mover_position(roof)).length() < 1.0e-6,
            "tick {tick}: the cage moves as one"
        );
    }
    assert_eq!(fired, 0, "no blocked event");
    assert!(
        rig.mover(lift).completed
            && rig.mover(roof).completed
            && (rig.mover_position(lift).y - 3.0).abs() < 1.0e-3,
        "the cage arrived at its top stop"
    );
}

#[test]
fn lift_with_tight_headroom_at_its_top_stop_arrives_and_completes() {
    // 5 cm of headroom at the stop: comfortably over one skin, which the
    // rider carry sweep keeps short of the ceiling.
    let headroom = 0.05;
    let ceiling = 3.0 + 2.0 * (HALF_HEIGHT + RADIUS) + SKIN_DISTANCE + headroom;
    for policy in [
        BlockPolicy::Displace,
        BlockPolicy::Stop,
        BlockPolicy::Reverse,
    ] {
        let (mut rig, lift, rider) = low_ceiling_lift(policy, 3.0, ceiling);
        rig.start(lift);
        let mut fired = 0;
        for _ in 0..150 {
            fired += rig.tick().blocked_events(LIFT);
            assert_clear_of_static(&rig, rider, &format!("tight headroom {policy:?}"));
        }
        let state = rig.mover(lift);
        assert!(
            state.completed && !state.blocked && (rig.mover_position(lift).y - 3.0).abs() < 1.0e-3,
            "{policy:?}: the lift reached its top stop: {:?}",
            rig.mover_position(lift)
        );
        assert_eq!(fired, 0, "{policy:?}: no yield on the way up");
        rig.assert_rides(rider, LIFT);
    }
}

// --- Hold, onset and restart -----------------------------------------------------

#[test]
fn blocked_event_fires_once_per_onset_even_under_self_reactions() {
    // A reaction on the held lift's own blocked event runs `moverStart`.
    let (mut rig, lift, rider) = low_ceiling_lift(BlockPolicy::Displace, 3.0, 4.6);
    rig.start(lift);
    let mut fired = 0;
    for _ in 0..180 {
        let now = rig.tick().blocked_events(LIFT);
        fired += now;
        if now > 0 {
            rig.start(lift);
        }
        assert_clear_of_static(&rig, rider, "start reaction");
    }
    assert_eq!(
        fired, 1,
        "moverStart on the held lift fires no second onset"
    );
    assert!(rig.mover(lift).blocked);

    // A reaction running `moverReverse` where both courses pin: the command
    // turns the lift once, and nothing turns it again.
    let (mut rig, lift, rider, under, mut fired) = pinned_both_ways_rig(BlockPolicy::Displace);
    let mut reversals = 0;
    if fired > 0 {
        rig.command(lift, MoverCommand::Reverse);
        reversals += 1;
    }
    let mut direction = rig.mover(lift).direction_sign;
    for tick in 0..60 {
        let now = rig.tick().blocked_events(LIFT);
        fired += now;
        assert_eq!(
            rig.mover(lift).direction_sign,
            direction,
            "tick {tick}: no flip"
        );
        assert!(
            rig.mover_overlap(under) <= OVERLAP_TOLERANCE,
            "never onto the pawn"
        );
        assert_clear_of_static(&rig, rider, "reverse reaction");
        if now > 0 {
            rig.command(lift, MoverCommand::Reverse);
            reversals += 1;
            direction = rig.mover(lift).direction_sign;
        }
    }
    assert_eq!(fired, 1, "one blocked event for the whole pin");
    assert_eq!(reversals, 1, "the reaction ran once");
    assert!(rig.mover(lift).blocked, "the reversed course held too");
}

#[test]
fn stop_release_then_restart_into_pin_fires_again() {
    let (mut rig, lift, _rider) = low_ceiling_lift(BlockPolicy::Displace, 3.0, 4.6);
    rig.start(lift);
    let mut fired = 0;
    for _ in 0..120 {
        fired += rig.tick().blocked_events(LIFT);
    }
    assert_eq!(fired, 1);
    rig.command(lift, MoverCommand::Stop);
    fired += rig.tick().blocked_events(LIFT);
    assert!(!rig.mover(lift).blocked, "moverStop released the hold");
    rig.start(lift);
    for _ in 0..30 {
        fired += rig.tick().blocked_events(LIFT);
    }
    assert_eq!(fired, 2, "the restart into the pin is a new onset");
    assert!(rig.mover(lift).blocked);
}

#[test]
fn reversed_once_mover_ends_unstarted_at_its_start() {
    let mut rig = Rig::new(static_boxes(&[floor()]));
    let door = rig.add_mover(
        door_leaf(DOOR),
        vec![Vec3::new(3.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    rig.start(door);
    rig.settle(30);
    rig.command(door, MoverCommand::Reverse);
    rig.settle(60);
    let state = rig.mover(door);
    assert!(
        !state.started && !state.completed && state.direction_sign == 1,
        "moverReverse back to the start leaves the door unstarted: {state:?}"
    );
    assert!((rig.mover_position(door) - Vec3::new(3.0, 0.0, 0.0)).length() < 1.0e-3);
    rig.start(door);
    rig.settle(150);
    assert!(
        rig.mover(door).completed,
        "the next start runs it to completion"
    );
}

#[test]
fn stop_hold_on_unpinned_contact_fires_once_when_the_cause_becomes_a_pin() {
    // A stop door closing along -X holds on contact with a pawn in an open
    // doorway: nothing is behind the pawn, so the contact is unpinned. A
    // second door then slides in tangentially and parks 2.5 cm off the pawn's
    // back, within the held course's push plus a skin: the hold's cause
    // becomes a pin against another mover.
    let mut rig = Rig::new(static_boxes(&[floor()]));
    let face = RADIUS + 5.5 * TICK_TRAVEL;
    let door = rig.add_mover(
        door_leaf(DOOR),
        vec![Vec3::new(face, 0.0, 0.0), Vec3::new(-3.0, 0.0, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Stop,
    );
    let behind_x = -RADIUS - 0.025;
    let backstop = rig.add_mover(
        box_collider(
            LEAF,
            &[(Vec3::new(-1.0, 0.0, -0.5), Vec3::new(0.0, 3.0, 0.5))],
        ),
        vec![Vec3::new(behind_x, 0.0, 3.0), Vec3::new(behind_x, 0.0, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    let pawn = rig.add_pawn(Vec3::new(0.0, rest_y(0.0), 0.0));
    rig.settle(20);
    rig.start(door);
    let mut fired = 0;
    for _ in 0..30 {
        fired += rig.tick().blocked_events(DOOR);
    }
    assert!(
        rig.mover(door).blocked && fired == 1,
        "the stop door holds on the unpinned contact"
    );
    let held_at = rig.mover_position(door);

    rig.start(backstop);
    for tick in 0..120 {
        fired += rig.tick().blocked_events(DOOR);
        assert!(
            rig.mover(door).blocked,
            "tick {tick}: the door holds throughout"
        );
        assert!(
            rig.mover_overlap(pawn) <= OVERLAP_TOLERANCE,
            "tick {tick}: the pawn stays out of both doors"
        );
    }
    assert!(
        rig.mover(backstop).completed,
        "the backstop parked behind the pawn"
    );
    assert_eq!(
        fired, 1,
        "the cause turning into a pin fires no second onset"
    );

    // The hold is now a pin: a frame-end `moverStart` that clears it is
    // re-held before the door moves, silently. Under an unpinned contact the
    // door would advance a tick and hold anew, firing again.
    rig.start(door);
    fired += rig.tick().blocked_events(DOOR);
    assert!(
        (rig.mover_position(door) - held_at).length() < 1.0e-6,
        "the pin re-held the door before it moved"
    );
    assert_eq!(
        fired, 1,
        "one blocked event for the contact and the pin it became"
    );
}

#[test]
fn reverse_command_on_a_parked_once_door_opens_it() {
    let mut rig = Rig::new(static_boxes(&[floor()]));
    // Waypoint 0 is closed; the door opens to +X.
    let closed = Vec3::new(-1.0, 0.0, 0.0);
    let open = Vec3::new(3.0, 0.0, 0.0);
    let door = rig.add_mover(
        door_leaf(DOOR),
        vec![closed, open],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    rig.start(door);
    rig.settle(30);
    rig.command(door, MoverCommand::Reverse);
    rig.settle(60);
    let parked = rig.mover(door);
    assert!(
        !parked.started
            && !parked.completed
            && (rig.mover_position(door) - closed).length() < 1.0e-3,
        "a reversal brought the door home unstarted: {parked:?}"
    );

    rig.command(door, MoverCommand::Reverse);
    let state = rig.mover(door);
    assert!(
        state.started && state.direction_sign == 1,
        "the only way out of the closed stop is open: {state:?}"
    );
    rig.settle(150);
    assert!(
        rig.mover(door).completed && (rig.mover_position(door) - open).length() < 1.0e-3,
        "the reversed door opened fully: {:?}",
        rig.mover_position(door)
    );
}

#[test]
fn reverse_door_pinned_at_its_start_holds_without_flapping_and_fires_once() {
    // The closing face starts half a tick of travel off a pawn whose back is
    // 2.5 cm off a wall: the run's first tick pins it, and the reversed course
    // from the start goes nowhere.
    let face = RADIUS + 0.5 * TICK_TRAVEL;
    let wall_face = -RADIUS - 0.025;
    let world = static_boxes(&[
        floor(),
        (
            Vec3::new(wall_face - 1.0, 0.0, -2.0),
            Vec3::new(wall_face, 3.0, 2.0),
        ),
    ]);
    let mut rig = Rig::new(world);
    let start = Vec3::new(face, 0.0, 0.0);
    let door = rig.add_mover(
        door_leaf(DOOR),
        vec![start, Vec3::new(face - 4.0, 0.0, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Reverse,
    );
    let pawn = rig.add_pawn(Vec3::new(0.0, rest_y(0.0), 0.0));
    rig.settle(20);
    rig.start(door);
    let mut fired = 0;
    for tick in 0..60 {
        fired += rig.tick().blocked_events(DOOR);
        let state = rig.mover(door);
        assert!(
            state.blocked && state.started && state.direction_sign == 1,
            "tick {tick}: the door holds on its course, never reversing: {state:?}"
        );
        assert!(
            (rig.mover_position(door) - start).length() < 1.0e-6,
            "tick {tick}: the door never left its start"
        );
        assert!(
            rig.mover_overlap(pawn) <= OVERLAP_TOLERANCE,
            "tick {tick}: never into the pawn"
        );
        assert_clear_of_static(&rig, pawn, "reverse at start");
    }
    assert_eq!(fired, 1, "one blocked event for the whole pin");
}

// --- Orderings -------------------------------------------------------------------

#[test]
fn auto_close_expiry_into_pin_yields() {
    // The pawn arrives while the opened door counts down, with its back to a
    // wall and the open leaf's closing face half a tick of travel in front of
    // it: the expiry tick's closing course is the pin.
    let pawn_x = 2.5;
    let leaf_face = pawn_x + RADIUS + 0.5 * TICK_TRAVEL;
    let wall_face = pawn_x - RADIUS - 0.025;
    let world = static_boxes(&[
        floor(),
        (Vec3::new(1.0, 0.0, -2.0), Vec3::new(wall_face, 3.0, 2.0)),
    ]);
    let mut rig = Rig::new(world);
    let timers = MoverAutoCloseTimers::default();
    timers.set_enabled(true);
    rig.auto_close = Some(timers.clone());
    // Waypoint 0 is closed; the door opens to +X, where its leaf's -X face
    // ends at `leaf_face`.
    let door = rig.add_mover(
        door_leaf(DOOR),
        vec![Vec3::new(-1.0, 0.0, 0.0), Vec3::new(leaf_face, 0.0, 0.0)],
        2.0,
        KinematicMoverMode::Once,
        BlockPolicy::Displace,
    );
    rig.edit_mover(door, |mover| mover.auto_close_ms = 500.0);
    rig.start(door);
    while !rig.mover(door).completed {
        rig.tick();
    }
    let open_at = rig.mover_position(door);
    let pawn = rig.add_pawn(Vec3::new(pawn_x, rest_y(0.0), 0.0));
    rig.settle(5);
    assert!(
        timers.remaining_ms(door).is_some_and(|ms| ms > 0.0),
        "the pawn is in place before the countdown expires"
    );

    let mut expired = false;
    for tick in 0..60 {
        let counting = timers.remaining_ms(door).is_some();
        let report = rig.tick();
        let state = rig.mover(door);
        assert!(
            rig.mover_overlap(pawn) <= OVERLAP_TOLERANCE,
            "tick {tick}: the auto-close never moved the door into the pawn"
        );
        assert_clear_of_static(&rig, pawn, "auto-close");
        assert!(
            (rig.mover_position(door) - open_at).length() < 1.0e-6,
            "tick {tick}: the door never left its open stop"
        );
        if counting && timers.remaining_ms(door).is_none() {
            // The expiry tick: the closing course is previewed in the same
            // tick's pass, after the timer set it.
            assert!(
                state.blocked && state.direction_sign < 0,
                "the expiry tick's closing course holds for the pawn: {state:?}"
            );
            assert_eq!(report.blocked_events(DOOR), 1, "one onset at expiry");
            expired = true;
        } else {
            assert_eq!(
                report.blocked_events(DOOR),
                0,
                "tick {tick}: no other onset"
            );
        }
    }
    assert!(expired, "the countdown expired");
    assert!(rig.mover(door).blocked, "the closing course still holds");
}

#[test]
fn trigger_stage_commands_on_held_mover_move_only_off_the_pin() {
    let (mut rig, door, pawn) = jamb_door_rig(BlockPolicy::Displace);
    rig.start(door);
    let mut fired = 0;
    for _ in 0..150 {
        fired += rig.tick().blocked_events(DOOR);
    }
    assert!(rig.mover(door).blocked && fired == 1);
    let held_at = rig.mover_position(door);

    // A trigger's start re-runs the course into the pin: it yields again,
    // silently.
    let report = rig.tick_with(|rig| rig.start(door));
    assert_eq!(report.blocked_events(DOOR), 0, "no second onset");
    rig.tick();
    assert!(rig.mover(door).blocked);
    assert!(
        (rig.mover_position(door) - held_at).length() < 1.0e-6,
        "never into the pawn"
    );

    // A trigger's reverse runs a course away from the pin: it moves.
    rig.tick_with(|rig| rig.command(door, MoverCommand::Reverse));
    rig.tick();
    assert!(
        rig.mover_position(door).x > held_at.x,
        "the reversed door backs off"
    );
    assert!(rig.mover_overlap(pawn) <= OVERLAP_TOLERANCE);
}

#[test]
fn despawned_pinned_pawn_releases_hold_next_tick() {
    let (mut rig, door, pawn) = jamb_door_rig(BlockPolicy::Displace);
    rig.start(door);
    rig.settle(150);
    assert!(rig.mover(door).blocked);
    let held_at = rig.mover_position(door);
    rig.remove_pawn(pawn);
    rig.tick();
    assert!(!rig.mover(door).blocked, "the next pass releases the hold");
    rig.tick();
    assert!(rig.mover_position(door).x < held_at.x, "the door moves on");
}

#[test]
fn block_policy_switch_mid_pin_takes_effect_next_tick() {
    // Held default door switched to crush: resumes the next tick and crushes.
    let (mut rig, door, pawn) = jamb_door_rig(BlockPolicy::Displace);
    give_health(&mut rig, pawn, 100.0);
    rig.edit_mover(door, |mover| {
        mover.crush_damage = 10.0;
        mover.crush_interval_ms = 100.0;
    });
    rig.start(door);
    rig.settle(150);
    assert!(rig.mover(door).blocked);
    let held_at = rig.mover_position(door);
    rig.command(door, MoverCommand::SetBlockPolicy(BlockPolicy::Crush));
    rig.tick();
    assert!(
        rig.mover_position(door).x < held_at.x,
        "the crusher resumes"
    );
    let mut crushed = false;
    for _ in 0..15 {
        crushed |= rig.tick().impacts > 0;
        if crushed {
            break;
        }
    }
    assert!(crushed && health(&rig, pawn) < 100.0, "and crushes");

    // A crusher pressing into a pawn switched to displace yields the next tick.
    let pressed_at = rig.mover_position(door);
    rig.command(door, MoverCommand::SetBlockPolicy(BlockPolicy::Displace));
    rig.tick();
    assert!(rig.mover(door).blocked, "the default yields");
    let after = rig.mover_position(door);
    rig.tick();
    assert!(
        (rig.mover_position(door) - after).length() < 1.0e-6 && after.x <= pressed_at.x,
        "and holds"
    );
}

#[test]
fn frame_end_commands_on_held_mover_never_embed_rider() {
    let commands = [
        MoverCommand::Start,
        MoverCommand::Reverse,
        MoverCommand::GoToPathNode("node1".to_string()),
        MoverCommand::SetSpinRate(90.0),
    ];
    for command in commands {
        let (mut rig, lift, rider) = low_ceiling_lift(BlockPolicy::Displace, 3.0, 4.6);
        // A spin axis, so `SetSpinRate` takes rather than warning and skipping.
        rig.edit_mover(lift, |mover| mover.spin_axis = Vec3::Y);
        rig.start(lift);
        rig.settle(120);
        assert!(rig.mover(lift).blocked);
        // The frame-end script drain runs after the frame's last pass.
        rig.command(lift, command.clone());
        for _ in 0..30 {
            rig.tick();
            assert_clear_of_static(&rig, rider, &format!("{command:?}"));
        }
        if let MoverCommand::SetSpinRate(rate_deg_s) = command {
            let state = rig.mover(lift);
            assert!(
                (state.spin_target_rate_rad_s - rate_deg_s.to_radians()).abs() < 1.0e-6,
                "the spin rate took: {state:?}"
            );
            // Spinning and rising still pins the rider, so the hold freezes
            // the new spin as well as the climb.
            assert!(
                state.blocked && state.spin_angle_rad == 0.0,
                "the held lift never spun: {state:?}"
            );
        }
    }
}

#[test]
fn blocked_event_reverse_backs_door_off_and_restart_closes_it() {
    let (mut rig, door, pawn) = jamb_door_rig(BlockPolicy::Displace);
    rig.start(door);
    let mut fired = 0;
    let mut reacted = false;
    for _ in 0..240 {
        let now = rig.tick().blocked_events(DOOR);
        fired += now;
        if now > 0 && !reacted {
            // The door's blocked_event reaction backs it off.
            rig.command(door, MoverCommand::Reverse);
            reacted = true;
        }
        assert!(rig.mover_overlap(pawn) <= OVERLAP_TOLERANCE);
    }
    assert_eq!(fired, 1);
    assert!(
        !rig.mover(door).started
            && (rig.mover_position(door) - Vec3::new(3.0, 0.0, 0.0)).length() < 1.0e-3,
        "the reaction backed the door off to its start"
    );
    // Its trigger closes it again, into the same pawn: a new onset.
    rig.start(door);
    for _ in 0..150 {
        fired += rig.tick().blocked_events(DOOR);
    }
    assert_eq!(fired, 2);
    assert!(rig.mover(door).blocked);
}

// --- Crush keeps pressing --------------------------------------------------------

#[test]
fn crush_lift_still_damages_ceiling_rider_on_cadence_and_keeps_moving() {
    let (mut rig, lift, rider) = low_ceiling_lift(BlockPolicy::Crush, 3.0, 4.6);
    give_health(&mut rig, rider, 1000.0);
    rig.edit_mover(lift, |mover| {
        mover.crush_damage = 10.0;
        mover.crush_interval_ms = 100.0;
    });
    rig.start(lift);
    // "The armed overlap assertion stays silent": every rig tick runs player
    // movement with the tick overlap check armed as a panicking assertion
    // (test builds, `movement/overlap_check.rs`). The crusher carries its rider
    // into the ceiling, and the check exempts that: it samples the start after
    // carry, and skips a tick that starts embedded. Nothing here disarms it,
    // so finishing without a panic is the proof; the embed assertion below
    // shows the rider really was carried inside the ceiling.
    let mut crush_ticks = Vec::new();
    for tick in 0..120 {
        if rig.tick().impacts > 0 {
            crush_ticks.push(tick);
        }
        assert!(!rig.mover(lift).blocked, "a crusher never holds");
    }
    assert!(rig.mover(lift).completed, "the crusher reached its stop");
    assert!(
        rig.static_overlap(rider) > 0.1,
        "the crusher pressed its rider into the ceiling: {} m",
        rig.static_overlap(rider)
    );
    assert!(
        crush_ticks.len() >= 3,
        "crushed on cadence: {crush_ticks:?}"
    );
    assert!(
        crush_ticks.windows(2).all(|pair| pair[1] - pair[0] >= 6),
        "no faster than its 100 ms cadence: {crush_ticks:?}"
    );
}

#[test]
fn paused_and_completed_crushers_keep_damaging_an_embedded_rider_on_cadence() {
    for paused in [true, false] {
        let what = if paused { "paused" } else { "completed" };
        let (mut rig, lift, rider) = low_ceiling_lift(BlockPolicy::Crush, 3.0, 4.6);
        give_health(&mut rig, rider, 1000.0);
        rig.edit_mover(lift, |mover| {
            mover.crush_damage = 10.0;
            mover.crush_interval_ms = 100.0;
        });
        rig.start(lift);
        // Ride until the crusher has pressed the rider well into the ceiling
        // with its cadence running; then pause it mid-course, or let it arrive.
        let mut crushed = false;
        for _ in 0..240 {
            crushed |= rig.tick().impacts > 0;
            let state = rig.mover(lift);
            let embedded = rig.static_overlap(rider) > 0.1;
            if crushed && embedded && (state.completed || (paused && !state.completed)) {
                break;
            }
        }
        if paused {
            assert!(!rig.mover(lift).completed, "paused short of its stop");
            rig.command(lift, MoverCommand::Stop);
            assert!(!rig.mover(lift).started, "moverStop paused the crusher");
        } else {
            assert!(rig.mover(lift).completed, "the crusher reached its stop");
        }
        assert!(crushed, "{what}: the cadence was running");
        let stopped_at = rig.mover_position(lift);
        let mut crush_ticks = Vec::new();
        for tick in 0..60 {
            if rig.tick().impacts > 0 {
                crush_ticks.push(tick);
            }
            assert!(
                (rig.mover_position(lift) - stopped_at).length() < 1.0e-6,
                "{what}: tick {tick}: the crusher stays put"
            );
            assert!(
                rig.static_overlap(rider) > 0.1,
                "{what}: tick {tick}: nothing frees the embedded rider"
            );
        }
        assert!(
            crush_ticks.len() >= 8,
            "{what}: the embedded rider is still crushed on cadence: {crush_ticks:?}"
        );
        assert!(
            crush_ticks.windows(2).all(|pair| pair[1] - pair[0] >= 6),
            "{what}: no faster than its 100 ms cadence: {crush_ticks:?}"
        );
    }
}

#[test]
fn crusher_damages_sideways_rider_and_two_mover_squeeze_on_cadence() {
    for (name, (mut rig, crusher, pawn)) in [
        ("sideways rider", sideways_crusher_rig(10.0)),
        ("two-mover squeeze", two_mover_crusher_rig(10.0)),
    ] {
        give_health(&mut rig, pawn, 1000.0);
        rig.start(crusher);
        let mut crush_ticks = Vec::new();
        let mut last_x = rig.mover_position(crusher).x;
        for tick in 0..120 {
            if rig.tick().impacts > 0 {
                crush_ticks.push(tick);
            }
            let x = rig.mover_position(crusher).x;
            assert!(
                rig.mover(crusher).completed || x != last_x,
                "{name}: tick {tick}: the crusher keeps moving"
            );
            last_x = x;
        }
        assert!(
            crush_ticks.len() >= 2,
            "{name}: crushed on cadence: {crush_ticks:?}"
        );
        assert!(
            crush_ticks.windows(2).all(|pair| pair[1] - pair[0] >= 6),
            "{name}: no faster than its cadence: {crush_ticks:?}"
        );
    }
}
