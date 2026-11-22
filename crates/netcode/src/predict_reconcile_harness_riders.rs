// Co-op proof that delayed remote riders stay planted on the mover they stand on,
// drawn through the mover's presented pose under the mandated link profile.
// See: context/lib/networking.md §Host-side remote-pawn presentation

use glam::{Affine3A, Vec3};
use postretro_entities::{KinematicMoverComponent, MoverCommand, Transform};

use crate::kinematic_mover::apply_mover_command;

use crate::interpolation::InterpolationDelayState;
use crate::predict_reconcile_harness_test_fixtures::{
    DT, LoopbackHarness, idle_command, mandated_link,
};
use crate::{client_sample_interpolation, client_sample_interpolation_at};

/// Ten revolutions at this rate take about 21 s, 1257 ticks.
const SPIN_RATE_RAD_S: f32 = 3.0;
const WARMUP_FRAMES: u32 = 120;
const REVOLUTIONS: f32 = 10.0;
const PLANTED_BOUND_M: f32 = 1.0e-4;
const ALPHAS: [f32; 3] = [0.0, 0.5, 1.0];

fn frame(transform: &Transform) -> Affine3A {
    Affine3A::from_scale_rotation_translation(
        transform.scale,
        transform.rotation,
        transform.position,
    )
}

fn local_point(mover: &Transform, world: Vec3) -> Vec3 {
    frame(mover).inverse().transform_point3(world)
}

#[derive(Clone, Copy, Debug)]
enum Seat {
    /// A connected client drawing the host's unowned second rider as a remote.
    Client,
    /// The host drawing the client's pawn from its delay buffer.
    Host,
}

/// Worst distance, over every measured frame and alpha, between the rider's
/// drawn point in the drawn mover's frame and its point on the host's mover.
/// Also returns the host's own drift of that point, which bounds how well the
/// truth itself holds.
fn ride_ten_revolutions(seat: Seat, world_space_riders: bool) -> (f32, f32) {
    let frames =
        WARMUP_FRAMES + (REVOLUTIONS * std::f32::consts::TAU / SPIN_RATE_RAD_S / DT).ceil() as u32;
    ride(seat, world_space_riders, frames, &[])
}

/// Ride for `frames`, issuing each `(frame, degrees_per_second)` spin command on
/// the host before that frame's step. The fixture spins with no acceleration,
/// so each command takes effect within one tick.
fn ride(
    seat: Seat,
    world_space_riders: bool,
    frames: u32,
    spin_commands: &[(u32, f32)],
) -> (f32, f32) {
    let mut h =
        LoopbackHarness::with_spinning_travelling_platform(mandated_link(), SPIN_RATE_RAD_S);
    h.client_replication
        .set_world_space_riders(world_space_riders);
    h.host_seat
        .as_mut()
        .expect("fixture presents the host seat")
        .set_world_space_riders(world_space_riders);
    let mut delay = InterpolationDelayState::new();
    let host_rider = match seat {
        Seat::Client => h.host_second_rider.expect("second rider"),
        Seat::Host => h.host_pawn,
    };
    let host_mover = h.host_mover.expect("host mover");
    let mut truth: Option<Vec3> = None;
    let mut host_drift = 0.0_f32;
    let mut worst = 0.0_f32;
    let mut measured = 0usize;

    for index in 0..frames {
        for &(_, rate_deg_s) in spin_commands.iter().filter(|(at, _)| *at == index) {
            let mut mover = h
                .host_registry
                .get_component::<KinematicMoverComponent>(host_mover)
                .unwrap()
                .clone();
            apply_mover_command(&mut mover, &MoverCommand::SetSpinRate(rate_deg_s));
            h.host_registry.set_component(host_mover, mover).unwrap();
        }
        h.step(&idle_command());
        let anim_time = f64::from(index) * f64::from(DT);
        let inputs = client_sample_interpolation(
            &mut h.client_registry,
            &mut h.client_replication,
            &h.time_sync,
            &mut delay,
            f64::from(DT),
            anim_time,
            0.0,
        );
        if index < WARMUP_FRAMES {
            continue;
        }
        let Some(render_tick) = inputs.render_server_tick else {
            continue;
        };
        // The host's authoritative pose, read before the host seat draws it.
        let host_local = local_point(
            h.host_registry
                .get_component::<Transform>(host_mover)
                .unwrap(),
            h.host_registry
                .get_component::<Transform>(host_rider)
                .unwrap()
                .position,
        );
        let truth = *truth.get_or_insert(host_local);
        host_drift = host_drift.max((host_local - truth).length());

        for alpha in ALPHAS {
            let (registry, rider, mover) = match seat {
                Seat::Client => {
                    client_sample_interpolation_at(
                        &mut h.client_registry,
                        &mut h.client_replication,
                        render_tick + f64::from(alpha),
                        anim_time,
                        alpha,
                    );
                    let network_id = h.allocator.stamp(host_rider);
                    let rider = h
                        .client_replication
                        .entity_for_network_id(network_id)
                        .expect("client presents the second rider");
                    (&h.client_registry, rider, h.client_mover.unwrap())
                }
                Seat::Host => {
                    h.host_present(alpha);
                    (&h.host_registry, host_rider, host_mover)
                }
            };
            let drawn_mover = registry.interpolated_transform(mover, alpha).unwrap();
            let drawn_rider = registry.interpolated_transform(rider, alpha).unwrap();
            worst = worst.max((local_point(&drawn_mover, drawn_rider.position) - truth).length());
            measured += 1;
        }
    }
    assert!(
        measured as u32 > (frames - WARMUP_FRAMES) * 2,
        "measured {measured} frame alphas"
    );
    (worst, host_drift)
}

fn assert_planted(seat: Seat) {
    let (before, _) = ride_ten_revolutions(seat, true);
    let (after, host_drift) = ride_ten_revolutions(seat, false);
    eprintln!(
        "[riders] {seat:?} seat, ten revolutions: worst drawn offset before {before:.4} m, \
         after {after:.6} m (host carry drift {host_drift:.6} m)"
    );
    assert!(
        before > 0.01,
        "the world-space blend shows the slide this fixture exists to catch: {before} m"
    );
    assert!(
        after <= PLANTED_BOUND_M,
        "remote rider must stay at its mover-local point: {after} m (host drift {host_drift} m)"
    );
}

// Regression: co-op riders drew at the interpolation delay while their mover drew
// at predicted current time, so a rider slid across a spinning platform.
#[test]
fn remote_rider_stays_planted_on_spinning_travelling_mover_client_view() {
    assert_planted(Seat::Client);
}

// Pin P11: the host pairs each rider sample with its mover's end-of-tick pose
// under the same tick label.
#[test]
fn remote_rider_stays_planted_on_spinning_travelling_mover_host_seat() {
    assert_planted(Seat::Host);
}

#[test]
fn remote_rider_stays_planted_through_spin_stop_reversal_and_restart() {
    // Stop, reverse, then restart the spin mid-ride; each snaps within a tick,
    // so its change lands inside a snapshot interval.
    let commands = [(300, 0.0), (420, -171.9), (600, 171.9), (760, -90.0)];
    for seat in [Seat::Client, Seat::Host] {
        let (worst, host_drift) = ride(seat, false, 900, &commands);
        eprintln!("[riders] {seat:?} seat through spin changes: worst {worst:.6} m");
        assert!(
            worst <= PLANTED_BOUND_M,
            "{seat:?}: rider must stay planted through spin changes: {worst} m \
             (host drift {host_drift} m)"
        );
    }
}
