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

/// A deck corner, so rotation errors show as distance.
const DECK_CORNER: Vec3 = Vec3::new(1.75, 0.0, 1.75);

/// Worst same-frame step, in metres at the deck corner and radians, of the
/// client's drawn mover across a snapshot apply: the pose drawn at each alpha
/// just before the apply against the pose drawn just after it. The glide then
/// advances as production advances it. Spin changes force mispredictions.
fn worst_apply_step(legacy: bool) -> (f32, f32) {
    let mut h =
        LoopbackHarness::with_spinning_travelling_platform(mandated_link(), SPIN_RATE_RAD_S);
    h.client_replication.set_legacy_mover_corrections(legacy);
    let host_mover = h.host_mover.unwrap();
    let client_mover = h.client_mover.unwrap();
    let commands = [(200, 0.0), (300, -171.9), (420, 171.9), (520, -90.0)];
    let drawn = |h: &LoopbackHarness, alpha: f32| {
        let pose = h
            .client_registry
            .interpolated_transform(client_mover, alpha)
            .unwrap();
        (pose.position + pose.rotation * DECK_CORNER, pose.rotation)
    };
    let (mut worst_m, mut worst_rad) = (0.0_f32, 0.0_f32);
    for index in 0..640 {
        for &(_, rate_deg_s) in commands.iter().filter(|(at, _)| *at == index) {
            let mut mover = h
                .host_registry
                .get_component::<KinematicMoverComponent>(host_mover)
                .unwrap()
                .clone();
            apply_mover_command(&mut mover, &MoverCommand::SetSpinRate(rate_deg_s));
            h.host_registry.set_component(host_mover, mover).unwrap();
        }
        h.client_predict_and_send(&idle_command());
        h.advance_clock();
        h.host_tick();
        let before: Vec<_> = ALPHAS.iter().map(|alpha| drawn(&h, *alpha)).collect();
        h.client_receive();
        if index >= WARMUP_FRAMES {
            for (alpha, (corner, rotation)) in ALPHAS.iter().zip(before) {
                let (after_corner, after_rotation) = drawn(&h, *alpha);
                worst_m = worst_m.max((after_corner - corner).length());
                // atan2, not acos: f32 acos loses milliradians near identity.
                let turn = (after_rotation * rotation.inverse()).normalize();
                worst_rad = worst_rad.max(2.0 * turn.xyz().length().atan2(turn.w.abs()));
            }
        }
        // Production advances the glide after the frame's apply and ticks; a
        // glide seeded this frame is drawn whole first.
        crate::advance_mover_correction_glides(&mut h.client_registry, DT);
    }
    (worst_m, worst_rad)
}

// Regression: a snapshot correction rewrote the mover's current pose without its
// previous one, so the drawn mover stepped under everyone standing on it.
#[test]
fn mover_correction_holds_the_drawn_deck_on_the_frame_it_lands() {
    let (before_m, before_rad) = worst_apply_step(true);
    let (after_m, after_rad) = worst_apply_step(false);
    eprintln!(
        "[riders] conditioned spin changes, worst same-frame drawn step: before {before_m:.4} m \
         {before_rad:.4} rad, after {after_m:.6} m {after_rad:.6} rad"
    );
    assert!(
        before_m > 1.0e-3,
        "the legacy apply steps the deck: {before_m} m"
    );
    assert!(
        after_m <= 1.0e-4 && after_rad <= 1.0e-4,
        "{after_m} m {after_rad} rad"
    );
}

/// Everything authoritative a frame leaves behind, formatted bit-exactly:
/// host pawns' poses and movement, the host mover's phase and pose, the client
/// pawn's predicted pose and movement, the client mover's phase and current
/// pose, the client's mover history, and the host's sent snapshot sequence.
fn authority_digest(h: &LoopbackHarness) -> String {
    use postretro_foundation::PlayerMovementComponent;
    let body = |registry: &postretro_entities::EntityRegistry, id| {
        format!(
            "{:?} {:?}",
            registry.get_component::<Transform>(id).ok(),
            registry.get_component::<PlayerMovementComponent>(id).ok()
        )
    };
    let mover = |registry: &postretro_entities::EntityRegistry, id| {
        format!(
            "{:?} {:?}",
            registry.get_component::<Transform>(id).ok(),
            registry.get_component::<KinematicMoverComponent>(id).ok()
        )
    };
    format!(
        "{} {} {} {} {} {} {:?} {:?} {}",
        body(&h.host_registry, h.host_pawn),
        body(&h.host_registry, h.host_second_rider.unwrap()),
        mover(&h.host_registry, h.host_mover.unwrap()),
        h.client_pawn
            .map_or(String::new(), |pawn| body(&h.client_registry, pawn)),
        mover(&h.client_registry, h.client_mover.unwrap()),
        format!("{:?}", h.client_replication.mover_history()),
        h.last_sent_client_tick,
        h.client_replication.latest_sequence(),
        h.server_tick,
    )
}

// Presentation never writes authority: with rebasing and smoothing on or off,
// every authoritative pose, phase, ground, command tick, mover history entry
// and snapshot sequence is bit-equal frame by frame.
#[test]
fn rider_presentation_leaves_authority_bit_equal() {
    let run = |presentation_on: bool| {
        let mut h =
            LoopbackHarness::with_spinning_travelling_platform(mandated_link(), SPIN_RATE_RAD_S);
        h.client_replication
            .set_world_space_riders(!presentation_on);
        h.client_replication
            .set_legacy_mover_corrections(!presentation_on);
        h.host_seat
            .as_mut()
            .unwrap()
            .set_world_space_riders(!presentation_on);
        let host_mover = h.host_mover.unwrap();
        let commands = [(200, 0.0), (300, -171.9), (420, 171.9)];
        let mut digests = Vec::new();
        for index in 0..520 {
            for &(_, rate_deg_s) in commands.iter().filter(|(at, _)| *at == index) {
                let mut mover = h
                    .host_registry
                    .get_component::<KinematicMoverComponent>(host_mover)
                    .unwrap()
                    .clone();
                apply_mover_command(&mut mover, &MoverCommand::SetSpinRate(rate_deg_s));
                h.host_registry.set_component(host_mover, mover).unwrap();
            }
            h.step(&idle_command());
            crate::advance_mover_correction_glides(&mut h.client_registry, DT);
            // Authority as serialization reads it, before the host seat
            // presents (the next host frame restores it).
            digests.push(authority_digest(&h));
            h.host_present(0.5);
        }
        digests
    };
    let (on, off) = (run(true), run(false));
    for (frame, (on, off)) in on.iter().zip(&off).enumerate() {
        assert_eq!(on, off, "frame {frame}");
    }
}

/// The client's view of the second rider walking off the spinning, travelling
/// deck: the worst frame-to-frame move of its drawn position, and the worst
/// move of its delayed world pose (its own motion), over the frames around the
/// moment it leaves.
fn walk_off_deck(eased: bool) -> (f32, f32) {
    let mut h =
        LoopbackHarness::with_spinning_travelling_platform(mandated_link(), SPIN_RATE_RAD_S);
    h.client_replication.set_rider_handoff_bypass(!eased);
    // A floor just below the deck, so the rider steps down onto world ground.
    h.world = crate::collision::CollisionWorld::from_triangles_for_test(
        vec![
            Vec3::new(-500.0, -0.3, -500.0),
            Vec3::new(500.0, -0.3, -500.0),
            Vec3::new(500.0, -0.3, 500.0),
            Vec3::new(-500.0, -0.3, 500.0),
        ],
        vec![[0, 2, 1], [0, 3, 2]],
    );
    let mut delay = InterpolationDelayState::new();
    let host_rider = h.host_second_rider.unwrap();
    let (mut drawn_prev, mut world_prev): (Option<Vec3>, Option<Vec3>) = (None, None);
    let (mut worst_drawn, mut worst_world) = (0.0_f32, 0.0_f32);
    for index in 0..420 {
        if index == WARMUP_FRAMES + 60 {
            h.host_second_rider_wish = glam::Vec2::new(0.0, 1.0);
        }
        h.step(&idle_command());
        crate::advance_mover_correction_glides(&mut h.client_registry, DT);
        let inputs = client_sample_interpolation(
            &mut h.client_registry,
            &mut h.client_replication,
            &h.time_sync,
            &mut delay,
            f64::from(DT),
            f64::from(index) * f64::from(DT),
            1.0,
        );
        if index < WARMUP_FRAMES || inputs.render_server_tick.is_none() {
            continue;
        }
        let network_id = h.allocator.stamp(host_rider);
        let Some(rider) = h.client_replication.entity_for_network_id(network_id) else {
            continue;
        };
        let drawn = h
            .client_registry
            .get_component::<Transform>(rider)
            .unwrap()
            .position;
        let world = h
            .client_replication
            .newest_sample_transform(network_id)
            .map(|transform| transform.position)
            .unwrap_or(drawn);
        if let (Some(previous), Some(world_previous)) = (drawn_prev, world_prev) {
            worst_drawn = worst_drawn.max((drawn - previous).length());
            worst_world = worst_world.max((world - world_previous).length());
        }
        drawn_prev = Some(drawn);
        world_prev = Some(world);
    }
    (worst_drawn, worst_world)
}

// Regression: a delayed remote leaving its deck popped back by the deck's
// motion over the interpolation delay as its interval fell to world space.
#[test]
fn remote_rider_leaving_a_deck_never_pops() {
    let (popped, own_motion) = walk_off_deck(false);
    let (eased, _) = walk_off_deck(true);
    eprintln!(
        "[riders] walking off the deck, worst frame-to-frame drawn move: before {popped:.4} m, \
         after {eased:.4} m (own motion {own_motion:.4} m per sample)"
    );
    assert!(popped > 0.2, "the fixture leaves a moving deck: {popped} m");
    assert!(eased < popped * 0.5, "eased {eased} m vs popped {popped} m");
}
