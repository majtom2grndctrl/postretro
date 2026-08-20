// Host-side delay-buffered presentation of connected-client pawns (Fix B).
// See: context/lib/networking.md
//
// Code-adjacent rationale not covered there:
//
// Sub-tick smoothness: the host renders well above tick rate (measured 250-330 fps).
// Sampling at an INTEGER target tick would only change the presented pose once per 60 Hz
// tick — it would step across the ~4-5 render frames per tick and reproduce the
// choppiness this module exists to remove. Instead the render target is FRACTIONAL:
// `(newest_recorded_tick - delay) + alpha`, where `alpha` sweeps [0, 1) across the render
// frames between two authoritative ticks. The buffer lerps/slerps between the two
// bracketing authoritative samples at that fraction, so the presented pose advances
// smoothly per render frame rather than per tick.
//
// Authority must never see the delayed pose: `run_host_movement_tick` reads the pawn's
// registry `Transform` as its start-of-tick position, and snapshot serialization reads it
// to replicate the authoritative pose. Three seams enforce this per frame:
//   1. record  — after each fixed tick's movement, capture the authoritative `Transform`
//                 into the buffer (keyed by `NetworkId`).
//   2. restore — before the tick loop (and thus before serialize), rewrite the registry
//                 `Transform` from the buffer's newest authoritative sample, undoing the
//                 previous frame's presentation write.
//   3. present — after serialize, sample the buffer at the delayed fractional target and
//                 write it via `set_presentation_transform` (which seeds `previous ==
//                 current`, so the render blend reproduces the sampled pose at any alpha).

use postretro_entities::{EntityRegistry, Transform};

use super::NetworkIdAllocator;
use super::command_queue::{INPUT_BUFFER_TARGET, MovementOwners};
use super::interpolation::{RemoteInterpolationBuffer, TransformSample};

/// Delay, in host authoritative ticks, between a client pawn's newest recorded pose
/// and the pose the host presents. Reuses the input playout margin (`INPUT_BUFFER_TARGET`,
/// ~2 ticks / ~33 ms) so input playout and remote presentation share one latency budget.
/// Tunable per the plan's "Fix B host clock source" open question.
pub(crate) const PRESENTATION_DELAY_TICKS: u32 = INPUT_BUFFER_TARGET as u32;

/// Record each connected-client pawn's authoritative `Transform` for `tick` into the
/// buffer, keyed by the pawn's stable `NetworkId`. Call once per completed fixed tick,
/// AFTER movement has written the authoritative pose and BEFORE the tick stamp advances,
/// so `tick` names the tick whose end-of-tick pose this sample carries.
///
/// A pawn with no allocated `NetworkId` or no live `Transform` (a stale id racing a
/// despawn) contributes no sample. `intrinsic_velocity`/`aim_pitch` are unused by the
/// host presentation write (which is `Transform`-only), so they are left absent/zero;
/// dense per-tick samples keep the presentation in the buffer's interpolate branch, never
/// the velocity-extrapolation branch.
pub(crate) fn record_client_pawn_poses(
    buffer: &mut RemoteInterpolationBuffer,
    owners: &MovementOwners,
    allocator: &NetworkIdAllocator,
    registry: &EntityRegistry,
    tick: u32,
) {
    for (pawn, _client_id) in owners.iter() {
        let Some(network_id) = allocator.network_id_for_entity(pawn) else {
            continue;
        };
        let Ok(transform) = registry.get_component::<Transform>(pawn) else {
            continue;
        };
        buffer.record(
            network_id,
            TransformSample {
                server_tick: tick,
                transform: *transform,
                intrinsic_velocity: None,
                aim_pitch: 0.0,
            },
        );
    }
}

/// Rewrite each connected-client pawn's registry `Transform` from the buffer's newest
/// authoritative sample. Call before the fixed-tick loop (and thus before movement and
/// snapshot serialization read the pawn) to undo the previous frame's delayed presentation
/// write, so authoritative reads never see a delayed pose. A pawn with no buffered sample
/// yet (freshly spawned, no tick recorded) keeps its live registry pose.
///
/// Precondition: this recovers the TRUE authoritative pose only because every
/// authoritative `Transform` write to an owned client pawn happens inside the fixed-tick
/// loop and is captured by that tick's end-of-tick `record`. A write that lands AFTER the
/// tick loop but before the next frame's restore — e.g. a same-`EntityId` respawn or
/// teleport of a still-owned pawn — is not in the buffer yet and would be silently
/// reverted by this function on the very next frame.
pub(crate) fn restore_client_pawn_authoritative_poses(
    buffer: &RemoteInterpolationBuffer,
    owners: &MovementOwners,
    allocator: &NetworkIdAllocator,
    registry: &mut EntityRegistry,
) {
    for (pawn, _client_id) in owners.iter() {
        let Some(network_id) = allocator.network_id_for_entity(pawn) else {
            continue;
        };
        let Some(transform) = buffer.newest_transform(network_id) else {
            continue;
        };
        // `set_component` touches only the current `Transform`; the following present
        // write reseats `previous_transforms` before any render read, so leaving the
        // previous slot at the last delayed pose is harmless.
        let _ = registry.set_component(pawn, transform);
    }
}

/// Sample each connected-client pawn's buffer at the delayed fractional target and write
/// the resulting pose via `set_presentation_transform`. Call once per host render frame,
/// AFTER snapshot serialization (which must read the authoritative pose) and BEFORE the
/// render collectors read entities.
///
/// `current_tick` is the host's authoritative tick after the fixed-tick loop, i.e. one
/// past the newest recorded sample; `alpha` is the render sub-tick accumulator fraction.
pub(crate) fn present_client_pawns(
    buffer: &RemoteInterpolationBuffer,
    owners: &MovementOwners,
    allocator: &NetworkIdAllocator,
    registry: &mut EntityRegistry,
    current_tick: u32,
    alpha: f32,
) {
    // Coupling guard: the delayed target must land inside the samples the buffer still
    // holds, or it silently slides into `RemoteInterpolationBuffer`'s hold-oldest/
    // extrapolate branch instead of interpolating between two authoritative samples —
    // defeating this module's whole point. That requires `PRESENTATION_DELAY_TICKS + 1`
    // (delay plus the in-flight tick) to stay below the buffer's per-entity cap. The cap
    // (`MAX_SAMPLES_PER_ENTITY` in `netcode::interpolation`) is private to that module, so
    // it is duplicated here as a literal; keep the two in sync. This guards the "Fix B
    // host clock source" open question against silently raising the delay past the cap.
    debug_assert!(
        (PRESENTATION_DELAY_TICKS as usize) + 1 < 16,
        "PRESENTATION_DELAY_TICKS ({PRESENTATION_DELAY_TICKS}) + 1 must stay below \
         MAX_SAMPLES_PER_ENTITY (16, netcode::interpolation) or the delayed presentation \
         target slides into the hold-oldest/extrapolate branch"
    );
    let target = presentation_target_tick(current_tick, alpha, PRESENTATION_DELAY_TICKS);
    for (pawn, _client_id) in owners.iter() {
        let Some(network_id) = allocator.network_id_for_entity(pawn) else {
            continue;
        };
        let Some(pose) = buffer.presented_pose(network_id, target) else {
            continue;
        };
        let _ = registry.set_presentation_transform(pawn, pose.transform);
    }
}

/// The fractional server-tick render target. `current_tick` is one past the newest
/// recorded authoritative tick, so the newest sample sits at `current_tick - 1`; the
/// target trails it by `delay_ticks` and is advanced by the render sub-tick `alpha`,
/// producing a value that sweeps smoothly between two authoritative samples across the
/// render frames of one fixed tick. Not wrap-aware, matching the client's f64 render
/// target (`ClientTimeSync::estimated_server_tick`): the u32 tick wraps only after ~2.3
/// years of continuous play.
#[must_use]
pub(crate) fn presentation_target_tick(current_tick: u32, alpha: f32, delay_ticks: u32) -> f64 {
    f64::from(current_tick) - 1.0 - f64::from(delay_ticks) + f64::from(alpha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Quat, Vec3};
    use postretro_entities::EntityId;
    use postretro_net::wire::NetworkId;

    const POS_EPS: f32 = 1e-4;

    fn pose_at(x: f32, yaw: f32) -> Transform {
        Transform {
            position: Vec3::new(x, 0.0, 0.0),
            rotation: Quat::from_rotation_y(yaw),
            scale: Vec3::ONE,
        }
    }

    /// Spawn a registered, owned client pawn at `pose` and return its ids.
    fn spawn_owned_pawn(
        registry: &mut EntityRegistry,
        owners: &mut MovementOwners,
        allocator: &mut NetworkIdAllocator,
        client_id: u64,
        pose: Transform,
    ) -> (EntityId, NetworkId) {
        let pawn = registry.spawn(pose);
        owners.set(pawn, client_id);
        let network_id = allocator.stamp(pawn);
        (pawn, network_id)
    }

    #[test]
    fn presentation_target_trails_newest_recorded_tick_by_the_delay() {
        // current_tick = 103 => newest recorded sample is tick 102. Delay 2, alpha 0 =>
        // target 100 (two ticks behind newest). Alpha sweeps the target toward 101.
        assert!((presentation_target_tick(103, 0.0, 2) - 100.0).abs() < 1e-9);
        assert!((presentation_target_tick(103, 0.5, 2) - 100.5).abs() < 1e-9);
        assert!((presentation_target_tick(103, 1.0, 2) - 101.0).abs() < 1e-9);
    }

    #[test]
    fn record_then_present_interpolates_between_bracketing_authoritative_ticks() {
        let mut registry = EntityRegistry::new();
        let mut owners = MovementOwners::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut buffer = RemoteInterpolationBuffer::new();
        let (pawn, _network_id) = spawn_owned_pawn(
            &mut registry,
            &mut owners,
            &mut allocator,
            7,
            pose_at(0.0, 0.0),
        );

        // Record a straight line of authoritative poses across ticks 100..=104: the pawn
        // walks +2 units/tick and turns +0.1 rad/tick. Each record stamps the pre-advance
        // tick, so we set the registry pose then record it, mirroring the tick-loop order.
        for tick in 100..=104u32 {
            let x = (tick - 100) as f32 * 2.0;
            let yaw = (tick - 100) as f32 * 0.1;
            registry.set_component(pawn, pose_at(x, yaw)).unwrap();
            record_client_pawn_poses(&mut buffer, &owners, &allocator, &registry, tick);
        }

        // current_tick = 105 (one past the newest recorded tick 104), delay 2, alpha 0.5:
        // target = 105 - 1 - 2 + 0.5 = 102.5, halfway between ticks 102 (x=4) and 103 (x=6).
        present_client_pawns(&buffer, &owners, &allocator, &mut registry, 105, 0.5);
        let presented = *registry.get_component::<Transform>(pawn).unwrap();
        assert!(
            (presented.position.x - 5.0).abs() < POS_EPS,
            "expected midpoint x 5.0, got {}",
            presented.position.x
        );
        // Rotation slerps halfway between yaw 0.2 and 0.3 -> ~0.25 rad.
        let yaw = presented.rotation.to_euler(glam::EulerRot::YXZ).0;
        assert!((yaw - 0.25).abs() < 1e-3, "expected yaw ~0.25, got {yaw}");

        // set_presentation_transform seeds previous == current, so the render blend
        // reproduces the sampled pose at ANY alpha (no re-blend by the sim sub-tick).
        let blended = registry
            .interpolated_transform(pawn, 0.0)
            .expect("pawn has a transform");
        assert!((blended.position.x - presented.position.x).abs() < POS_EPS);
    }

    #[test]
    fn present_varies_smoothly_per_render_frame_within_one_fixed_tick() {
        // The core Fix B requirement: at high render rate, sampling at a fractional target
        // must move the presented pose EVERY render frame, not step once per 60 Hz tick.
        let mut registry = EntityRegistry::new();
        let mut owners = MovementOwners::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut buffer = RemoteInterpolationBuffer::new();
        let (pawn, _network_id) = spawn_owned_pawn(
            &mut registry,
            &mut owners,
            &mut allocator,
            7,
            pose_at(0.0, 0.0),
        );
        for tick in 100..=104u32 {
            let x = (tick - 100) as f32 * 2.0;
            registry.set_component(pawn, pose_at(x, 0.0)).unwrap();
            record_client_pawn_poses(&mut buffer, &owners, &allocator, &registry, tick);
        }

        // Four render frames at the same fixed tick (current_tick constant), alpha rising:
        // the presented x must be strictly increasing across the frames.
        let mut last_x = f32::NEG_INFINITY;
        for step in 0..4u32 {
            let alpha = step as f32 / 4.0;
            present_client_pawns(&buffer, &owners, &allocator, &mut registry, 105, alpha);
            let x = registry
                .get_component::<Transform>(pawn)
                .unwrap()
                .position
                .x;
            assert!(
                x > last_x,
                "presented x must advance per render frame: step {step} x {x} !> {last_x}"
            );
            last_x = x;
        }
    }

    #[test]
    fn restore_rewrites_authoritative_pose_after_a_presentation_write() {
        // Restore must put the authoritative pose back so the next tick's movement and the
        // snapshot serialization never read the delayed presentation pose.
        let mut registry = EntityRegistry::new();
        let mut owners = MovementOwners::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut buffer = RemoteInterpolationBuffer::new();
        let (pawn, _network_id) = spawn_owned_pawn(
            &mut registry,
            &mut owners,
            &mut allocator,
            7,
            pose_at(0.0, 0.0),
        );
        for tick in 100..=104u32 {
            let x = (tick - 100) as f32 * 2.0;
            registry.set_component(pawn, pose_at(x, 0.0)).unwrap();
            record_client_pawn_poses(&mut buffer, &owners, &allocator, &registry, tick);
        }
        // Authoritative newest is tick 104 -> x = 8.0. A present writes a delayed pose.
        present_client_pawns(&buffer, &owners, &allocator, &mut registry, 105, 0.0);
        assert!(
            (registry
                .get_component::<Transform>(pawn)
                .unwrap()
                .position
                .x
                - 8.0)
                .abs()
                > 1.0,
            "the present write left a clearly delayed pose"
        );

        restore_client_pawn_authoritative_poses(&buffer, &owners, &allocator, &mut registry);
        assert!(
            (registry
                .get_component::<Transform>(pawn)
                .unwrap()
                .position
                .x
                - 8.0)
                .abs()
                < POS_EPS,
            "restore rewrites the newest authoritative pose (x = 8.0)"
        );
    }

    #[test]
    fn only_owned_pawns_are_recorded_and_presented() {
        // Gating: the path keys off MovementOwners. A pawn the host does not treat as a
        // connected-client pawn (not in owners) — e.g. the host's own pawn — is neither
        // buffered nor overwritten, even though it has an allocated NetworkId.
        let mut registry = EntityRegistry::new();
        let mut owners = MovementOwners::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut buffer = RemoteInterpolationBuffer::new();

        let (client_pawn, client_net) = spawn_owned_pawn(
            &mut registry,
            &mut owners,
            &mut allocator,
            7,
            pose_at(0.0, 0.0),
        );
        // Host's own pawn: registered/stamped for outbound replication, but NOT an owner.
        let host_pawn = registry.spawn(pose_at(50.0, 0.0));
        let host_net = allocator.stamp(host_pawn);

        for tick in 100..=104u32 {
            let x = (tick - 100) as f32 * 2.0;
            registry
                .set_component(client_pawn, pose_at(x, 0.0))
                .unwrap();
            // Move the host pawn too; it must never be recorded.
            registry
                .set_component(host_pawn, pose_at(50.0 + x, 0.0))
                .unwrap();
            record_client_pawn_poses(&mut buffer, &owners, &allocator, &registry, tick);
        }

        assert_eq!(
            buffer.sample_count(client_net),
            5,
            "client pawn is buffered"
        );
        assert_eq!(
            buffer.sample_count(host_net),
            0,
            "host pawn is never buffered"
        );

        // Present must not touch the host pawn's live pose (x = 58.0 after the loop).
        present_client_pawns(&buffer, &owners, &allocator, &mut registry, 105, 0.0);
        assert!(
            (registry
                .get_component::<Transform>(host_pawn)
                .unwrap()
                .position
                .x
                - 58.0)
                .abs()
                < POS_EPS,
            "the host's own pawn keeps its live pose"
        );
    }
}
