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

use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform,
    components::mesh::MeshComponent,
};

use super::NetworkIdAllocator;
use super::command_queue::{INPUT_BUFFER_TARGET, MovementOwners};
use super::interpolation::{
    ObservedTeleport, PresentedPose, RemoteInterpolationBuffer, TeleportSample, TransformSample,
};
use crate::collision::CollisionWorld;
use crate::collision::moving::{MoverCollider, MoverPoseSource};
use crate::scripting_systems::hit_zones::HitZoneStore;

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
/// despawn) contributes no sample. Each sample also keeps the tick's player velocity
/// and avatar aim pitch, so the presented avatar's heading and aim come from the same
/// delayed sample as its pose (see [`present_client_pawns`]). Dense per-tick samples
/// keep the presentation in the buffer's interpolate branch, never the
/// velocity-extrapolation branch.
pub fn record_client_pawn_poses(
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
        let movement = registry
            .get_component::<postretro_foundation::PlayerMovementComponent>(pawn)
            .ok();
        // This tick's pose pass has already written the remote aim pitch.
        let aim_pitch = registry
            .get_component::<MeshComponent>(pawn)
            .ok()
            .and_then(|mesh| mesh.pose_inputs)
            .map_or(0.0, |inputs| inputs.aim_pitch);
        buffer.record(
            network_id,
            TransformSample {
                teleport: movement.map(|movement| TeleportSample {
                    generation: movement.teleport.generation,
                    last_teleporter: movement.teleport.last_teleporter,
                }),
                server_tick: tick,
                transform: *transform,
                intrinsic_velocity: movement.map(|movement| movement.velocity),
                aim_pitch,
            },
        );
    }
}

/// Drain after each authoritative tick's [`record_client_pawn_poses`] and before the
/// audio step. Each recorded tick is one host sample, so every generation change
/// between consecutive ticks is its own observed change, reported at that sample's
/// authoritative arrival pose.
pub fn take_client_pawn_teleports(
    buffer: &mut RemoteInterpolationBuffer,
    owners: &MovementOwners,
    allocator: &NetworkIdAllocator,
) -> Vec<ObservedTeleport> {
    buffer.take_observed_teleports(|network_id| {
        let pawn = allocator.entity_for_network_id(network_id)?;
        owners.owner_of(pawn).map(|_| pawn)
    })
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
pub fn restore_client_pawn_authoritative_poses(
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
pub fn present_client_pawns(
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
        present_avatar_pose_inputs(registry, pawn, &pose);
    }
}

/// Re-probe each connected-client pawn's feet from the transform it is drawn at.
/// Call right after [`present_client_pawns`], which wrote the delayed presented pose
/// into the registry; the fixed tick probed from the live pose, which leads it and
/// sits at the arrival while the body is still drawn at a teleport's source. A pawn
/// with no buffered sample keeps its live pose, which is then also what is drawn.
/// `anim_time` is the render frame's animation clock the body is posed at.
pub fn probe_presented_client_pawn_feet(
    owners: &MovementOwners,
    registry: &mut EntityRegistry,
    collision_world: &CollisionWorld,
    mover_colliders: &[MoverCollider],
    mover_poses: &dyn MoverPoseSource,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
) {
    for (pawn, _client_id) in owners.iter() {
        let Ok(&transform) = registry.get_component::<Transform>(pawn) else {
            continue;
        };
        crate::sim::update_foot_ground_probes_for(
            registry,
            pawn,
            transform,
            collision_world,
            mover_colliders,
            mover_poses,
            hit_zone_store,
            anim_time,
        );
    }
}

/// Rewrite the avatar's aim and heading from the presented sample. The fixed tick's
/// pose pass reads the live command aim and velocity, which lead the delayed pose
/// and would turn the body at the source before a teleport's cut reaches it. The
/// pawn's transform yaw is its aim, and travel heading comes from the sample's
/// player velocity, as a connected client presents a remote player.
fn present_avatar_pose_inputs(registry: &mut EntityRegistry, pawn: EntityId, pose: &PresentedPose) {
    let (aim_yaw, _, _) = pose.transform.rotation.to_euler(glam::EulerRot::YXZ);
    if !aim_yaw.is_finite() {
        return;
    }
    let heading_yaw = pose
        .intrinsic_velocity
        .filter(|velocity| velocity.is_finite())
        .map_or(aim_yaw, |velocity| {
            crate::sim::player_travel_heading_yaw(velocity, aim_yaw)
        });
    if let Ok(ComponentValue::Mesh(mesh)) =
        registry.get_component_value_mut(pawn, ComponentKind::Mesh)
        && let Some(inputs) = mesh.pose_inputs.as_mut()
    {
        inputs.aim_yaw = aim_yaw;
        inputs.heading_yaw = heading_yaw;
        if pose.aim_pitch.is_finite() {
            inputs.aim_pitch = pose.aim_pitch;
        }
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

    // The host seat hears each change: each recorded tick is one sample, so draining
    // after each tick's record reports every generation change (a wrap included)
    // once, at that sample's authoritative arrival, while the delayed presentation
    // still shows the source. The first sample reports nothing.
    #[test]
    fn teleport_host_observer_reports_each_tick_change_at_its_authoritative_sample() {
        use postretro_foundation::PlayerMovementComponent;
        let mut registry = EntityRegistry::new();
        let mut owners = MovementOwners::default();
        let mut allocator = NetworkIdAllocator::new();
        let mut buffer = RemoteInterpolationBuffer::default();
        let (pawn, _) = spawn_owned_pawn(
            &mut registry,
            &mut owners,
            &mut allocator,
            7,
            pose_at(0.0, 0.0),
        );
        registry
            .set_component(
                pawn,
                crate::predict_reconcile_harness_test_fixtures::component(),
            )
            .unwrap();
        record_client_pawn_poses(&mut buffer, &owners, &allocator, &registry, 0);
        assert!(take_client_pawn_teleports(&mut buffer, &owners, &allocator).is_empty());
        for (tick, generation, last, x) in [(1, 255, 0, 100.0), (2, 0, 1, 200.0)] {
            let mut movement = registry
                .get_component::<PlayerMovementComponent>(pawn)
                .unwrap()
                .clone();
            movement.teleport.generation = generation;
            movement.teleport.last_teleporter = last;
            registry.set_component(pawn, movement).unwrap();
            registry.set_component(pawn, pose_at(x, 0.5)).unwrap();
            record_client_pawn_poses(&mut buffer, &owners, &allocator, &registry, tick);
            let cues = take_client_pawn_teleports(&mut buffer, &owners, &allocator);
            assert_eq!(cues.len(), 1, "tick {tick}: one change, one cue");
            assert_eq!(cues[0].entity_id, pawn);
            assert_eq!(cues[0].generation, generation);
            assert_eq!(cues[0].last_teleporter, last);
            assert!((cues[0].arrival.x - x).abs() < 1.0e-6);
        }
        // Host delay still displays the source, while cues reported the accepted
        // authoritative arrival samples. No entity anchor can substitute for them.
        present_client_pawns(&buffer, &owners, &allocator, &mut registry, 3, 0.5);
        assert!(
            registry
                .get_component::<Transform>(pawn)
                .unwrap()
                .position
                .length()
                < 1.0e-6
        );
        assert!(take_client_pawn_teleports(&mut buffer, &owners, &allocator).is_empty());
        restore_client_pawn_authoritative_poses(&buffer, &owners, &allocator, &mut registry);
        assert!(
            (registry
                .get_component::<Transform>(pawn)
                .unwrap()
                .position
                .x
                - 200.0)
                .abs()
                < 1.0e-6
        );
    }

    // On the host seat the avatar's aim, body heading and pitch come
    // from the same delayed sample as its pose. The fixed tick's pose pass writes
    // the live, already-rotated aim and travel heading; while the delayed pose
    // still shows the source, the avatar keeps the source's facing and heading,
    // and turns only once the presentation reaches the arrival side of the cut.
    #[test]
    fn teleport_host_avatar_aim_and_heading_stay_on_the_presented_side_of_the_cut() {
        use postretro_entities::components::mesh::MeshComponent;
        use postretro_foundation::{PlayerMovementComponent, PoseInputs};
        use std::f32::consts::FRAC_PI_2;
        let mut registry = EntityRegistry::new();
        let mut owners = MovementOwners::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut buffer = RemoteInterpolationBuffer::new();
        let (pawn, _) = spawn_owned_pawn(
            &mut registry,
            &mut owners,
            &mut allocator,
            7,
            pose_at(0.0, 0.0),
        );
        registry
            .set_component(
                pawn,
                crate::predict_reconcile_harness_test_fixtures::component(),
            )
            .unwrap();
        let mut mesh = MeshComponent::stateless("avatar".into());
        mesh.pose_inputs = Some(PoseInputs::default());
        registry.set_component(pawn, mesh).unwrap();
        // One authoritative tick: pose, movement, and the pose pass's live inputs.
        let tick = |registry: &mut EntityRegistry, x: f32, yaw: f32, generation: u8| {
            let travel = Quat::from_rotation_y(yaw) * Vec3::NEG_Z * 6.0;
            let mut movement = registry
                .get_component::<PlayerMovementComponent>(pawn)
                .unwrap()
                .clone();
            movement.velocity = travel;
            movement.teleport.generation = generation;
            movement.teleport.last_teleporter = 0;
            registry.set_component(pawn, movement).unwrap();
            registry.set_component(pawn, pose_at(x, yaw)).unwrap();
            let mut mesh = registry
                .get_component::<MeshComponent>(pawn)
                .unwrap()
                .clone();
            mesh.pose_inputs = Some(PoseInputs {
                aim_pitch: 0.1 * f32::from(generation + 1),
                aim_yaw: yaw,
                heading_yaw: crate::sim::player_travel_heading_yaw(travel, yaw),
                ..PoseInputs::default()
            });
            registry.set_component(pawn, mesh).unwrap();
        };
        // Facing -Z and walking forward at the source, then a quarter-turn teleport.
        for t in 0..4u32 {
            tick(&mut registry, t as f32 * 0.1, 0.0, 0);
            record_client_pawn_poses(&mut buffer, &owners, &allocator, &registry, t);
        }
        tick(&mut registry, 100.0, FRAC_PI_2, 1);
        record_client_pawn_poses(&mut buffer, &owners, &allocator, &registry, 4);
        let inputs = |registry: &EntityRegistry| {
            registry
                .get_component::<MeshComponent>(pawn)
                .unwrap()
                .pose_inputs
                .unwrap()
        };
        let source_heading = crate::sim::player_travel_heading_yaw(Vec3::NEG_Z, 0.0);
        let arrival_heading = crate::sim::player_travel_heading_yaw(
            Quat::from_rotation_y(FRAC_PI_2) * Vec3::NEG_Z,
            0.0,
        );
        // current_tick 5: the delayed target spans ticks 2..3, still the source.
        for alpha in [0.0, 0.5, 0.99] {
            present_client_pawns(&buffer, &owners, &allocator, &mut registry, 5, alpha);
            assert!(
                registry
                    .get_component::<Transform>(pawn)
                    .unwrap()
                    .position
                    .x
                    < 1.0
            );
            let shown = inputs(&registry);
            assert!(
                shown.aim_yaw.abs() < 1.0e-5,
                "aim turned at the source: {}",
                shown.aim_yaw
            );
            assert!((shown.heading_yaw - source_heading).abs() < 1.0e-5);
            assert!((shown.aim_pitch - 0.1).abs() < 1.0e-5);
        }
        // Two ticks later the delayed target reaches the arrival sample.
        for t in 5..7u32 {
            tick(&mut registry, 100.0, FRAC_PI_2, 1);
            record_client_pawn_poses(&mut buffer, &owners, &allocator, &registry, t);
        }
        present_client_pawns(&buffer, &owners, &allocator, &mut registry, 7, 0.0);
        assert!(
            (registry
                .get_component::<Transform>(pawn)
                .unwrap()
                .position
                .x
                - 100.0)
                .abs()
                < 1.0e-4
        );
        let shown = inputs(&registry);
        assert!((shown.aim_yaw - FRAC_PI_2).abs() < 1.0e-5);
        assert!((shown.heading_yaw - arrival_heading).abs() < 1.0e-5);
        assert!((shown.aim_pitch - 0.2).abs() < 1.0e-5);
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

    // Review round 2: the fixed tick probes feet from the live pose, which is at
    // the arrival while the host seat still draws the pawn at the source. The
    // re-probe after present must plant the feet on the floor the body is drawn
    // over: the source floor (contact -1.0), not the arrival's (-0.8).
    #[test]
    fn teleport_host_seat_foot_probes_follow_presented_source_not_live_arrival() {
        use crate::collision::CollisionWorld;
        use crate::kinematic_mover::MoverTickStateTable;
        use crate::scripting_systems::hit_zones::{HitZoneStore, ModelHitZones};
        use postretro_foundation::PlayerMovementComponent;
        use postretro_model::pose_modifier::{JointMask, LegChain, PoseModifierStack};
        use postretro_model::skeleton::{Joint, RestLocal, Skeleton};
        use std::collections::HashMap;
        use std::sync::Arc;

        // Source floor at y=0 around x=0; arrival floor at y=5 around x=100.
        let patch = |x: f32, y: f32| {
            [(-10.0, -10.0), (10.0, -10.0), (10.0, 10.0), (-10.0, 10.0)]
                .map(|(dx, dz)| Vec3::new(x + dx, y, dz))
        };
        let points = [patch(0.0, 0.0), patch(100.0, 5.0)].concat();
        let world = CollisionWorld::from_triangles_for_test(
            points,
            vec![[0, 2, 1], [0, 3, 2], [4, 6, 5], [4, 7, 6]],
        );
        // One leg: a stateless root with its foot joint resting at model (0,-0.7,0).
        let joint = |parent, translation| Joint {
            parent,
            inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
            rest_local: RestLocal {
                translation,
                ..RestLocal::default()
            },
        };
        let mut chain_mask = JointMask::new();
        assert!(chain_mask.insert(0) && chain_mask.insert(1));
        let mut store = HitZoneStore::new();
        store.insert_for_test(
            postretro_model::ModelHandle::from("legwalker".to_string()),
            ModelHitZones::for_test(
                Arc::new(Skeleton {
                    joints: vec![
                        joint(None, Vec3::ZERO),
                        joint(Some(0), Vec3::new(0.0, -0.7, 0.0)),
                    ],
                }),
                Arc::new(Vec::new()),
                vec![None, None],
                HashMap::new(),
                None,
                vec![LegChain {
                    chain_mask,
                    foot_joint: 1,
                }],
                Arc::new(PoseModifierStack::default()),
            ),
        );

        let source = Transform {
            position: Vec3::new(0.0, 1.0, 0.0),
            ..pose_at(0.0, 0.0)
        };
        let arrival = Transform {
            position: Vec3::new(100.0, 5.8, 0.0),
            ..pose_at(0.0, 0.0)
        };
        let mut registry = EntityRegistry::new();
        let mut owners = MovementOwners::default();
        let mut allocator = NetworkIdAllocator::new();
        let mut buffer = RemoteInterpolationBuffer::default();
        let (pawn, _) = spawn_owned_pawn(&mut registry, &mut owners, &mut allocator, 7, source);
        registry
            .set_component(pawn, MeshComponent::stateless("legwalker".to_string()))
            .unwrap();
        registry
            .set_component(
                pawn,
                crate::predict_reconcile_harness_test_fixtures::component(),
            )
            .unwrap();
        for tick in 0..3 {
            record_client_pawn_poses(&mut buffer, &owners, &allocator, &registry, tick);
        }
        // Tick 3 teleports: the live pose and generation move to the arrival.
        let mut movement = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .clone();
        movement.teleport.generation = movement.teleport.generation.wrapping_add(1);
        registry.set_component(pawn, movement).unwrap();
        registry.set_component(pawn, arrival).unwrap();
        record_client_pawn_poses(&mut buffer, &owners, &allocator, &registry, 3);

        // The fixed tick's pose pass probes the live arrival pose.
        let movers = MoverTickStateTable::default();
        let empty = HashMap::new();
        crate::sim::update_presentation_pose_inputs(
            &mut registry,
            &world,
            &[],
            &movers,
            &store,
            0.0,
            crate::sim::PresentationPoseInputs {
                camera_aim: (0.0, 0.0),
                remote_player_aims: &HashMap::new(),
                remote_aim_pitches: &empty,
                remote_heading_yaws: &empty,
                remote_network_ids: &HashMap::new(),
            },
        );
        let foot = |registry: &EntityRegistry| {
            let inputs = registry
                .get_component::<MeshComponent>(pawn)
                .unwrap()
                .pose_inputs
                .unwrap();
            assert_eq!(inputs.foot_count, 1);
            assert!(inputs.feet[0].hit, "the foot finds ground");
            inputs.feet[0].contact_height
        };
        assert!(
            (foot(&registry) + 0.8).abs() < 1.0e-3,
            "live probe: arrival floor"
        );

        // Delayed presentation (target 1.5) still draws the source; the re-probe
        // follows it.
        present_client_pawns(&buffer, &owners, &allocator, &mut registry, 4, 0.5);
        let drawn = registry.get_component::<Transform>(pawn).unwrap().position;
        assert!(
            (drawn - source.position).length() < POS_EPS,
            "drawn at the source"
        );
        probe_presented_client_pawn_feet(
            &owners,
            &mut registry,
            &world,
            &[],
            &movers,
            &store,
            0.25,
        );
        let contact = foot(&registry);
        assert!(
            (contact + 1.0).abs() < 1.0e-3,
            "feet plant on the source floor under the drawn body, got {contact}"
        );
    }
}
