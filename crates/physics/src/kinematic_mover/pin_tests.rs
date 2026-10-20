// Pinned-body rule: no mover but `crush` leaves a body pinned against static
// geometry or another mover. Driver, movement and pass run in host tick order.
// See: context/lib/movement.md §6

use glam::Vec3;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::{BlockPolicy, EntityId, KinematicMoverMode};

use super::pin_test_fixtures::*;
use crate::collision::OVERLAP_TOLERANCE;

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
    // Captured at 411d3fb, before the pinned-body rule: bit-exact pawn
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
