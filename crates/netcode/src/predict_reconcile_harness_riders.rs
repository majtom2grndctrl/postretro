// Co-op proof that delayed remote riders stay planted on the mover they stand on,
// drawn through the mover's presented pose under the mandated link profile.
// See: context/lib/networking.md §Host-side remote-pawn presentation

use glam::{Affine3A, Vec3};
use postretro_entities::{EntityId, Transform};

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

struct ClientView {
    rider: EntityId,
    mover: EntityId,
}

fn client_view(h: &mut LoopbackHarness) -> Option<ClientView> {
    let network_id = h.allocator.stamp(h.host_second_rider?);
    Some(ClientView {
        rider: h.client_replication.entity_for_network_id(network_id)?,
        mover: h.client_mover?,
    })
}

/// Worst distance, over every measured frame and alpha, between the remote
/// rider's drawn point in the drawn mover's frame and its point on the host's
/// mover. Also returns the host's own drift of that point, which bounds how
/// well the truth itself holds.
fn ride_ten_revolutions(world_space_riders: bool) -> (f32, f32) {
    let mut h = LoopbackHarness::with_spinning_travelling_platform(mandated_link(), SPIN_RATE_RAD_S);
    h.client_replication.set_world_space_riders(world_space_riders);
    let mut delay = InterpolationDelayState::new();
    let host_rider = h.host_second_rider.expect("second rider");
    let host_mover = h.host_mover.expect("host mover");
    let frames = WARMUP_FRAMES
        + (REVOLUTIONS * std::f32::consts::TAU / SPIN_RATE_RAD_S / DT).ceil() as u32;
    let mut truth: Option<Vec3> = None;
    let mut host_drift = 0.0_f32;
    let mut worst = 0.0_f32;
    let mut measured = 0usize;

    for index in 0..frames {
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
        let host_local = local_point(
            h.host_registry.get_component::<Transform>(host_mover).unwrap(),
            h.host_registry
                .get_component::<Transform>(host_rider)
                .unwrap()
                .position,
        );
        let truth = *truth.get_or_insert(host_local);
        host_drift = host_drift.max((host_local - truth).length());

        let view = client_view(&mut h).expect("client presents the second rider");
        for alpha in ALPHAS {
            client_sample_interpolation_at(
                &mut h.client_registry,
                &mut h.client_replication,
                render_tick + f64::from(alpha),
                anim_time,
                alpha,
            );
            let drawn_mover = h
                .client_registry
                .interpolated_transform(view.mover, alpha)
                .unwrap();
            let drawn_rider = h
                .client_registry
                .interpolated_transform(view.rider, alpha)
                .unwrap();
            worst = worst.max((local_point(&drawn_mover, drawn_rider.position) - truth).length());
            measured += 1;
        }
    }
    assert!(measured > 3000, "measured {measured} frame alphas");
    (worst, host_drift)
}

// Regression: co-op riders drew at the interpolation delay while their mover drew
// at predicted current time, so a rider slid across a spinning platform.
#[test]
fn remote_rider_stays_planted_on_spinning_travelling_mover_client_view() {
    let (before, _) = ride_ten_revolutions(true);
    let (after, host_drift) = ride_ten_revolutions(false);
    eprintln!(
        "[riders] client view, ten revolutions: worst drawn offset before {before:.4} m, \
         after {after:.6} m (host carry drift {host_drift:.6} m)"
    );
    assert!(
        before > 0.05,
        "the world-space blend shows the slide this fixture exists to catch: {before} m"
    );
    assert!(
        after <= PLANTED_BOUND_M,
        "remote rider must stay at its mover-local point: {after} m (host drift {host_drift} m)"
    );
}
