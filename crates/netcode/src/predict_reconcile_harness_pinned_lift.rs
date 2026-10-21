// Co-op proof for a default lift holding for a rider pinned under a low ceiling: the host
// runs the pin pretest and blocking pass, and the client converges onto the held lift.
// See: context/lib/networking.md §Current contract · context/lib/movement.md §6

use glam::Vec3;
use postretro_entities::{KinematicMoverComponent, MoverCommand};
use postretro_foundation::{GroundRef, PlayerMovementComponent};

use crate::collision::moving::MoverCollider;
use crate::collision::{CollisionCapsule, CollisionWorld, OVERLAP_TOLERANCE};
use crate::kinematic_mover::apply_mover_command;
use crate::predict_reconcile_harness_test_fixtures::{
    LoopbackHarness, MOVING_PLATFORM_ID, SETTLED_PAWN_TOLERANCE_M, idle_command, mandated_link,
};

/// The lift's top stop, and a ceiling whose underside leaves 1.6 m of headroom
/// there: less than the 2.4 m standing capsule.
const LIFT_TOP_Y: f32 = 3.0;
const CEILING_Y: f32 = 4.6;
const LIFT_SPEED_MPS: f32 = 2.0;

/// Closed axis-aligned box triangles, wound outward.
fn box_triangles(points: &mut Vec<Vec3>, tris: &mut Vec<[u32; 3]>, min: Vec3, max: Vec3) {
    let corner = |i: u32| {
        Vec3::new(
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        )
    };
    let centre = (min + max) * 0.5;
    let base = points.len() as u32;
    points.extend((0..8).map(corner));
    for quad in [
        [0, 2, 6, 4],
        [1, 3, 7, 5],
        [0, 1, 5, 4],
        [2, 3, 7, 6],
        [0, 1, 3, 2],
        [4, 5, 7, 6],
    ] {
        let [a, b, c, d] = quad.map(|i| base + i);
        let (pa, pb, pc) = (points[a as usize], points[b as usize], points[c as usize]);
        if (pb - pa).cross(pc - pa).dot(pa - centre) >= 0.0 {
            tris.extend([[a, b, c], [a, c, d]]);
        } else {
            tris.extend([[a, c, b], [a, d, c]]);
        }
    }
}

fn ceiling_world() -> CollisionWorld {
    let (mut points, mut tris) = (Vec::new(), Vec::new());
    box_triangles(
        &mut points,
        &mut tris,
        Vec3::new(-10.0, CEILING_Y, -10.0),
        Vec3::new(10.0, CEILING_Y + 1.0, 10.0),
    );
    CollisionWorld::from_triangles_for_test(points, tris)
}

/// A 3 m square deck, 0.2 m thick, its top at local y = 0.
fn deck_collider() -> MoverCollider {
    let (mut points, mut tris) = (Vec::new(), Vec::new());
    box_triangles(
        &mut points,
        &mut tris,
        Vec3::new(-1.5, -0.2, -1.5),
        Vec3::new(1.5, 0.0, 1.5),
    );
    MoverCollider::from_local_triangles(MOVING_PLATFORM_ID, &points, &tris).unwrap()
}

fn static_overlap(h: &LoopbackHarness, registry_is_host: bool) -> f32 {
    let (registry, pawn) = if registry_is_host {
        (&h.host_registry, h.host_pawn)
    } else {
        (
            &h.client_registry,
            h.client_pawn.expect("client pawn armed"),
        )
    };
    let capsule = &registry
        .get_component::<PlayerMovementComponent>(pawn)
        .expect("pawn movement")
        .capsule;
    let position = if registry_is_host {
        h.host_position()
    } else {
        h.client_position().expect("client pawn armed")
    };
    h.world.overlap_depth_for_test(
        position,
        CollisionCapsule::new(capsule.radius, capsule.half_height),
    )
}

fn client_lift_blocked(h: &LoopbackHarness) -> bool {
    h.client_registry
        .get_component::<KinematicMoverComponent>(h.client_mover.expect("client lift"))
        .expect("client lift phase")
        .blocked
}

#[test]
fn client_riding_a_default_lift_into_a_low_ceiling_converges_on_the_host_hold() {
    let mut h = LoopbackHarness::with_lift(
        mandated_link(),
        ceiling_world(),
        deck_collider(),
        vec![Vec3::ZERO, Vec3::new(0.0, LIFT_TOP_Y, 0.0)],
        LIFT_SPEED_MPS,
    );
    let idle = idle_command();
    h.step_until_armed(&idle);
    for _ in 0..60 {
        h.step(&idle);
    }
    assert_eq!(h.host_ground(), GroundRef::Mover(MOVING_PLATFORM_ID));
    assert_eq!(h.client_ground(), GroundRef::Mover(MOVING_PLATFORM_ID));

    // A host-only start, as a trigger would fire it; the client learns of it
    // from replicated phase.
    let lift = h.host_mover.expect("host lift");
    let mut phase = h
        .host_registry
        .get_component::<KinematicMoverComponent>(lift)
        .unwrap()
        .clone();
    apply_mover_command(&mut phase, &MoverCommand::Start);
    h.host_registry.set_component(lift, phase).unwrap();

    let mut largest_correction = 0.0_f32;
    let mut largest_correction_from_hold = 0.0_f32;
    let mut hold_reconciled_at = None;
    let mut embedded_client_ticks = 0;
    let mut deepest_embed = 0.0_f32;
    let mut host_ever_held = false;
    for tick in 0..300 {
        // One client tick: the predicted pose, then the reconciled one.
        h.client_predict_and_send(&idle);
        let predicted = static_overlap(&h, false);
        h.advance_clock();
        h.host_tick();
        h.client_receive();
        let reconciled = static_overlap(&h, false);

        host_ever_held |= h
            .host_registry
            .get_component::<KinematicMoverComponent>(lift)
            .unwrap()
            .blocked;
        let host_overlap = static_overlap(&h, true);
        assert!(
            host_overlap <= OVERLAP_TOLERANCE,
            "tick {tick}: the host pawn is {host_overlap} m inside the ceiling"
        );
        let tick_correction = h
            .latest_local_corrections
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);
        largest_correction = largest_correction.max(tick_correction);
        if host_ever_held {
            largest_correction_from_hold = largest_correction_from_hold.max(tick_correction);
        }

        match hold_reconciled_at {
            Some(at) => {
                assert!(
                    predicted <= OVERLAP_TOLERANCE && reconciled <= OVERLAP_TOLERANCE,
                    "tick {tick} ({} after the hold reconcile): client pawn inside the ceiling, \
                     predicted {predicted} m, reconciled {reconciled} m",
                    tick - at
                );
            }
            None => {
                let embed = predicted.max(reconciled);
                if embed > OVERLAP_TOLERANCE {
                    embedded_client_ticks += 1;
                    deepest_embed = deepest_embed.max(embed);
                }
                if client_lift_blocked(&h) && !h.latest_reconcile_windows.is_empty() {
                    hold_reconciled_at = Some(tick);
                    assert!(
                        reconciled <= OVERLAP_TOLERANCE,
                        "the reconcile carrying the hold leaves the client {reconciled} m \
                         inside the ceiling"
                    );
                }
            }
        }
    }
    assert!(
        host_ever_held,
        "the host lift must hold for its pinned rider"
    );
    let hold_reconciled_at =
        hold_reconciled_at.expect("a reconcile carrying the replicated hold must arrive");

    super::drain(&mut h);
    assert!(h.is_drained(), "the harness must reach its drain condition");
    let error = h.position_error();
    assert!(
        error <= SETTLED_PAWN_TOLERANCE_M,
        "the client pawn converges to the host's pose on the held lift; error {error} m"
    );
    assert!(static_overlap(&h, false) <= OVERLAP_TOLERANCE);

    // The whole run's largest includes the host-only start's first catch-up.
    eprintln!("pinned-lift largest correction: {largest_correction:.4} m");
    eprintln!(
        "pinned-lift largest correction from the host hold on: {largest_correction_from_hold:.4} m"
    );
    eprintln!(
        "pinned-lift embedded before the hold reconcile: {embedded_client_ticks} client ticks, \
         deepest {deepest_embed:.4} m (hold reconciled on run tick {hold_reconciled_at})"
    );
}
