// Player and brain animation locomotion, presentation pose inputs, capsule
// mesh offsets and foot ground probes, run after movement each tick.
// See: context/lib/entity_model.md §5 · context/lib/movement.md

use super::*;
use postretro_entities::components::mesh::BodyFrame;

/// Select authoritative player locomotion after movement resolves. Remote-owned host
/// pawns carry no brain, so without this pass their descriptor default (`idle`) would
/// be serialized forever and repeatedly correct the client's velocity prediction.
/// State selection and rate calibration intentionally match the client presentation
/// path; ordinary switches use the authored crossfade, preserving no-snap correction.
/// The forward/backward classification compares travel to the pawn's transform yaw,
/// which is its aim. This pass is the sole writer of that memory and of the jump
/// phase on a peer that runs it; the pose-input pass and the host seat's delayed
/// presentation read the classification.
pub fn update_player_animation_locomotion(
    registry: &mut EntityRegistry,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
) {
    let players: Vec<EntityId> = registry
        .iter_with_kind(ComponentKind::PlayerMovement)
        .map(|(id, _)| id)
        .collect();
    for id in players {
        let Ok(movement) = registry.get_component::<PlayerMovementComponent>(id) else {
            continue;
        };
        let velocity = movement.velocity;
        let speed_xz = Vec3::new(velocity.x, 0.0, velocity.z).length();
        let walk_speed = movement.ground_params.speed.walk;
        let run_speed = movement.ground_params.speed.run;
        let crouch_speed = movement.ground_params.speed.crouch;
        let stance = LocomotionStance::from_movement(movement);
        let Ok(&transform) = registry.get_component::<postretro_entities::Transform>(id) else {
            continue;
        };
        let (aim_yaw, _, _) = transform.rotation.to_euler(EulerRot::YXZ);

        let (stored_direction, direction, stored_jump, selection) = {
            let Ok(mesh) = registry.get_component::<MeshComponent>(id) else {
                continue;
            };
            let Some(animation) = mesh.animation.as_ref() else {
                continue;
            };
            let direction =
                classify_locomotion_direction(velocity, aim_yaw, animation.locomotion_direction);
            let mut input = LocomotionInput {
                speed_xz,
                walk_speed,
                run_speed,
                direction,
                posture: stance.posture,
                footing: stance.footing,
                jump_phase: animation.locomotion_jump,
                current_state: &animation.current_state,
            };
            let usable = |state: &str| animation.is_state_usable(state);
            let selection = PlayerLocomotionStates::from_animation(animation).map(|states| {
                input.jump_phase = states.advance_jump_phase(&input, anim_time, usable, |state| {
                    clip_duration(animation, mesh, hit_zone_store, state)
                });
                let (state, tier) = states.select(&input, usable);
                let target = state.unwrap_or(&animation.default_state);
                // Only an actual switch needs an owned name.
                let switch_to = (animation.current_state != target).then(|| target.to_string());
                (switch_to, tier, input.jump_phase)
            });
            (
                animation.locomotion_direction,
                direction,
                animation.locomotion_jump,
                selection,
            )
        };
        let jump = selection.as_ref().map(|(_, _, jump)| *jump);
        if direction != stored_direction || jump.is_some_and(|jump| jump != stored_jump) {
            store_locomotion_memory(registry, id, direction, jump);
        }
        let Some((switch_to, tier, _)) = selection else {
            continue;
        };
        // A refused switch (an unresolved clip) leaves another state playing,
        // which rests at its authored rate.
        let target_reached = switch_to.is_none_or(|target| {
            matches!(
                switch_animation_state(registry, id, &target),
                SwitchResult::Switched | SwitchResult::AlreadyInState
            )
        });
        let (moving, fallback_speed) = match tier {
            LocomotionTierKind::Idle | LocomotionTierKind::Slide | LocomotionTierKind::Jump => {
                (false, 1.0)
            }
            LocomotionTierKind::Walk => (true, walk_speed),
            LocomotionTierKind::Run => (true, run_speed),
            LocomotionTierKind::Crouch => (true, crouch_speed),
        };

        let rate_input = registry
            .get_component::<MeshComponent>(id)
            .ok()
            .and_then(|mesh| {
                let animation = mesh.animation.as_ref()?;
                if !moving || !target_reached || !animation.speed_scale {
                    return Some(1.0);
                }
                let effective = effective_travel_speed(animation, mesh, hit_zone_store);
                Some(MeshAnimation::locomotion_rate_ratio(
                    speed_xz.max(0.0),
                    effective,
                    fallback_speed,
                ))
            });
        let Some(rate_input) = rate_input else {
            continue;
        };
        let needs_rebase = registry
            .get_component::<MeshComponent>(id)
            .ok()
            .and_then(|mesh| mesh.animation.as_ref())
            .is_some_and(|animation| animation.playback_rate_needs_update(rate_input));
        if !needs_rebase {
            continue;
        }
        let Ok(mut mesh) = registry.get_component::<MeshComponent>(id).cloned() else {
            continue;
        };
        let Some(animation) = mesh.animation.as_mut() else {
            continue;
        };
        animation.update_playback_rate(rate_input, anim_time);
        let _ = registry.set_component(id, mesh);
    }
}

/// Update brain-driven walk playback after steering has resolved this tick's
/// velocity. `anim_time` is the slow-mo/freeze-gated animation clock sampled by
/// the renderer. The AI pass's locomotion intent is deliberately not reused: it
/// is a pre-steering, squared-speed read from the prior tick, while this path
/// must match the motion the steering system actually produced.
///
/// The rate reads steering velocity, which excludes knockback, so a shove never
/// quickens the stride; that is the contract the client mirrors. The client's
/// remote walk rate (netcode `update_remote_enemy_walk_playback_rate`) feeds
/// interpolated displacement speed instead, which includes knockback: a known
/// divergence until enemies carry velocity on the wire.
pub(super) fn update_brain_animation_playback_rates(
    registry: &mut EntityRegistry,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
) {
    let mut rebases = Vec::new();

    for (id, _) in registry.iter_with_kind(ComponentKind::Brain) {
        let Ok(brain) = registry.get_component::<BrainComponent>(id) else {
            continue;
        };
        let Ok(agent) = registry.get_component::<AgentComponent>(id) else {
            continue;
        };
        let Some(path_state) = agent_steering::path_state(registry, id) else {
            continue;
        };
        // The stride cadence follows the agent's own travel; a knockback slide
        // or launch never quickens the walk cycle.
        let travel = path_state.steering_velocity();
        let speed_xz = Vec3::new(travel.x, 0.0, travel.z).length();

        let Ok(mesh) = registry.get_component::<MeshComponent>(id) else {
            continue;
        };
        let Some(animation) = mesh.animation.as_ref() else {
            continue;
        };

        // Rate-scale only the graph's rate-scaled states — its locomotion
        // state, and each state an activity plays under its own steering
        // motion (every motion but `hold` and `freeze`: a patrol's walk and a
        // moving attack's stride included) — or a directional variant of one
        // (`_left`, `_right`, `_backward`), and only while the archetype leaves
        // `speedScale` on. The set is derived once per graph (foundation
        // `RateScaledStates`), so this reads a handful of names, never the
        // graph; the remote client derives the same set from the descriptor.
        // Every other state plays its fixed `playbackRate` (default 1).
        // Calibration is `measured_ground_speed / effective_travel_speed`, each
        // variant against its own clip's stride; a state with neither an
        // override nor a derived clip stride falls back to
        // `speed_xz / move_speed`, keeping the shipped in-place walk unchanged.
        // The fallback is the archetype's base speed, never the activity's
        // steered speed, because the client has only the base to divide by.
        // The active state's own policy applies last
        // (`MeshAnimation::stride_playback_target`): `scaleWithSpeed: false`
        // opts it out, `maxPlaybackRate` (default `RATE_MAX`) bounds the
        // clamp, and a state the rule does not scale plays its fixed
        // `playbackRate` (default 1). The client reads the same fields from
        // its local descriptor.
        let is_rate_scaled = animation.speed_scale
            && brain
                .rate_scaled_states()
                .iter()
                .any(|state| TravelVariant::of_state(state, &animation.current_state).is_some());
        let stride_ratio = is_rate_scaled.then(|| {
            let effective = effective_travel_speed(animation, mesh, hit_zone_store);
            MeshAnimation::locomotion_rate_ratio(speed_xz, effective, agent.move_speed)
        });
        let target = animation.stride_playback_target(stride_ratio);
        if animation.playback_rate_target_needs_update(target) {
            rebases.push((id, target));
        }
    }

    for (id, target) in rebases {
        // The read-only predicate above centralizes clamping and the epsilon
        // policy, so clone/write only the components whose timeline will rebase.
        let Ok(mut mesh) = registry.get_component::<MeshComponent>(id).cloned() else {
            continue;
        };
        let Some(animation) = mesh.animation.as_mut() else {
            continue;
        };

        animation.apply_playback_rate_target(target, anim_time);
        let _ = registry.set_component(id, mesh);
    }
}

/// Resolve the effective travel speed for a rate-scaled state's speed-scaled
/// playback: the active state's authored `travelSpeed` override wins, else the
/// model clip's load-derived stride (via [`HitZoneStore`], keyed by the mesh's
/// model handle, with the clip picked by `ModelHitZones::state_clip`, which the
/// client's remote walk reference shares). `None` when neither is calibrated —
/// the degenerate reference (`speed_xz / move_speed`) that keeps the shipped
/// in-place walk byte-for-byte unchanged.
pub(super) fn effective_travel_speed(
    animation: &MeshAnimation,
    mesh: &MeshComponent,
    hit_zone_store: &HitZoneStore,
) -> Option<f32> {
    let state = animation.states.get(&animation.current_state)?;
    let derived = hit_zone_store
        .get_by_name(&mesh.model)
        .and_then(|model| model.state_clip(state.clip_index, &state.clip))
        .and_then(|clip| clip.travel_speed);
    state.effective_travel_speed(derived)
}

/// The clip length of `state`'s clip, read from the model's load-time clips like
/// [`effective_travel_speed`], for ending the player jump sequence's one-shots.
pub(super) fn clip_duration(
    animation: &MeshAnimation,
    mesh: &MeshComponent,
    hit_zone_store: &HitZoneStore,
    state: &str,
) -> Option<f32> {
    let state = animation.states.get(state)?;
    hit_zone_store
        .get_by_name(&mesh.model)?
        .state_clip(state.clip_index, &state.clip)
        .map(|clip| clip.duration)
}

/// Produce renderer-facing pose inputs from the registry's currently displayed
/// transforms. The fixed tick calls this after steering; connected clients call
/// it after remote interpolation because they intentionally skip `simulate_tick`.
/// It also reseats each movement pawn's mesh origin offset on its live capsule
/// (which hit zones read too). None of it enters replication.
pub struct PresentationPoseInputs<'a> {
    pub camera_aim: (f32, f32),
    pub remote_player_aims: &'a HashMap<EntityId, (f32, f32)>,
    pub remote_aim_pitches: &'a HashMap<NetworkId, f32>,
    pub remote_heading_yaws: &'a HashMap<NetworkId, f32>,
    pub remote_network_ids: &'a HashMap<EntityId, NetworkId>,
    /// The local pawn has no locomotion state selector on this peer (a connected
    /// client takes its own pawn's state from the host), so this pass keeps the
    /// pawn's forward/backward memory. Elsewhere the fixed tick's selector owns
    /// that memory and this pass only reads it.
    pub local_pawn_owns_direction: bool,
}

pub fn update_presentation_pose_inputs(
    registry: &mut EntityRegistry,
    collision_world: &CollisionWorld,
    mover_colliders: &[MoverCollider],
    mover_poses: &dyn MoverPoseSource,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    presentation: PresentationPoseInputs<'_>,
) {
    let PresentationPoseInputs {
        camera_aim,
        remote_player_aims,
        remote_aim_pitches,
        remote_heading_yaws,
        remote_network_ids,
        local_pawn_owns_direction,
    } = presentation;
    sync_player_mesh_origin_offsets(registry);
    update_pose_inputs(
        registry,
        camera_aim,
        remote_player_aims,
        remote_aim_pitches,
        remote_heading_yaws,
        remote_network_ids,
        local_pawn_owns_direction,
    );
    update_foot_ground_probes(
        registry,
        collision_world,
        mover_colliders,
        mover_poses,
        hit_zone_store,
        anim_time,
    );
}

/// Keep every player pawn's mesh render origin on its live capsule's sole.
///
/// The descriptor seeds the offset from the standing capsule, but crouch, slide
/// and a low life-state posture shrink the capsule with the feet planted and so
/// lower the centre the transform tracks. Left at the standing size, the model
/// sinks by the half-height difference and its cull point lands below the floor,
/// outside every visible cell, so the pawn vanishes. The fixed tick runs it right
/// after Movement, so that tick's hit zones read the resized body, and again in
/// the presentation pose pass ahead of the pose and foot probes, which also
/// covers a connected client's predicted pawn. Remote
/// avatars on a connected client carry no movement component; their offset is
/// written from the replicated half-height when their pose is presented.
pub(super) fn sync_player_mesh_origin_offsets(registry: &mut EntityRegistry) {
    let pawns: Vec<(EntityId, f32, f32)> = registry
        .iter_with_kind(ComponentKind::PlayerMovement)
        .filter_map(|(id, value)| match value {
            ComponentValue::PlayerMovement(movement) => {
                Some((id, movement.capsule.radius, movement.capsule.half_height))
            }
            _ => None,
        })
        .collect();
    for (id, radius, half_height) in pawns {
        set_player_mesh_capsule_origin(registry, id, radius, half_height);
    }
}

/// Seat a player pawn's mesh render origin on the sole of a capsule of `radius`
/// and cylinder `half_height` (see [`sync_player_mesh_origin_offsets`]). The
/// offset is presentation and hit-zone placement only and never crosses the
/// wire, so it is written in place without a replication-visible component
/// replace. Non-finite dimensions and meshless pawns are left untouched.
pub fn set_player_mesh_capsule_origin(
    registry: &mut EntityRegistry,
    id: EntityId,
    radius: f32,
    half_height: f32,
) {
    if !radius.is_finite() || !half_height.is_finite() {
        return;
    }
    let offset = player_capsule_feet_origin_offset(radius, half_height);
    if let Ok(ComponentValue::Mesh(mesh)) =
        registry.get_component_value_mut(id, ComponentKind::Mesh)
        && mesh.origin_offset != offset
    {
        mesh.origin_offset = offset;
    }
}

/// Write same-tick presentation inputs after AI and steering have settled the
/// entity's target and body rotation. Every animated mesh receives a finite
/// value; entities without a live acquired target hold their body heading with
/// zero pitch, making pose modifiers a visual no-op. Player pawns fold their
/// travel heading by the forward/backward classification; see
/// [`presented_player_heading`]. With `local_pawn_owns_direction` this pass is
/// also the local pawn's direction-memory writer.
pub(super) fn update_pose_inputs(
    registry: &mut EntityRegistry,
    camera_aim: (f32, f32),
    remote_player_aims: &HashMap<EntityId, (f32, f32)>,
    remote_aim_pitches: &HashMap<NetworkId, f32>,
    remote_heading_yaws: &HashMap<NetworkId, f32>,
    remote_network_ids: &HashMap<EntityId, NetworkId>,
    local_pawn_owns_direction: bool,
) {
    const MIN_HORIZONTAL_LEN_SQ: f32 = 1e-8;

    let animated: Vec<EntityId> = registry
        .iter_with_kind(ComponentKind::Mesh)
        .filter_map(|(id, _)| {
            registry
                .get_component::<MeshComponent>(id)
                .ok()
                .is_some_and(|mesh| mesh.animation.is_some())
                .then_some(id)
        })
        .collect();

    for id in animated {
        let Ok(transform) = registry
            .get_component::<postretro_entities::Transform>(id)
            .copied()
        else {
            continue;
        };
        let (raw_heading, _, _) = transform.rotation.to_euler(EulerRot::YXZ);
        let heading_yaw = if raw_heading.is_finite() {
            raw_heading
        } else {
            0.0
        };

        let target_position = registry
            .get_component::<BrainComponent>(id)
            .ok()
            .and_then(|brain| brain.acquired_target)
            .and_then(|target| {
                registry
                    .get_component::<postretro_entities::Transform>(target)
                    .ok()
                    .map(|transform| transform.position)
            });

        let (default_aim_pitch, default_aim_yaw) = target_position
            .map(|target| target - transform.position)
            .filter(|direction| direction.is_finite())
            .map(|direction| {
                let horizontal_len_sq = direction.x * direction.x + direction.z * direction.z;
                let aim_yaw = if horizontal_len_sq > MIN_HORIZONTAL_LEN_SQ {
                    direction.x.atan2(direction.z)
                } else {
                    heading_yaw
                };
                let horizontal_len = horizontal_len_sq.max(0.0).sqrt();
                let pitch = direction
                    .y
                    .atan2(horizontal_len)
                    .clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2);
                (
                    if pitch.is_finite() { pitch } else { 0.0 },
                    if aim_yaw.is_finite() {
                        aim_yaw
                    } else {
                        heading_yaw
                    },
                )
            })
            .unwrap_or((0.0, heading_yaw));

        let (aim_pitch, aim_yaw, heading_yaw) = if registry.local_player_movement_pawn() == Some(id)
        {
            let aim_pitch = camera_aim
                .0
                .clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2);
            let aim_yaw = if camera_aim.1.is_finite() {
                camera_aim.1
            } else {
                heading_yaw
            };
            let player_heading = presented_player_heading(
                registry,
                id,
                aim_yaw,
                heading_yaw,
                local_pawn_owns_direction,
            );
            (
                if aim_pitch.is_finite() {
                    aim_pitch
                } else {
                    0.0
                },
                aim_yaw,
                player_heading,
            )
        } else if let Some(&(aim_pitch, aim_yaw)) = remote_player_aims.get(&id) {
            let aim_yaw = if aim_yaw.is_finite() {
                aim_yaw
            } else {
                heading_yaw
            };
            let player_heading =
                presented_player_heading(registry, id, aim_yaw, heading_yaw, false);
            (
                if aim_pitch.is_finite() {
                    aim_pitch.clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2)
                } else {
                    0.0
                },
                aim_yaw,
                player_heading,
            )
        } else if let Some(network_id) = remote_network_ids.get(&id)
            && let Some(&aim_pitch) = remote_aim_pitches.get(network_id)
        {
            let remote_heading = remote_heading_yaws
                .get(network_id)
                .copied()
                .filter(|yaw| yaw.is_finite())
                .unwrap_or(heading_yaw);
            (
                if aim_pitch.is_finite() {
                    aim_pitch
                } else {
                    0.0
                },
                heading_yaw,
                remote_heading,
            )
        } else {
            (default_aim_pitch, default_aim_yaw, heading_yaw)
        };

        // Single borrow serves both the change check and the clone source, so a
        // stationary/idle crowd whose inputs haven't moved skips the write
        // entirely — mirrors the discipline in
        // `update_brain_animation_playback_rates` above.
        let Ok(current) = registry.get_component::<MeshComponent>(id) else {
            continue;
        };
        // This pass owns only the aim/heading fields. The feet/foot_count are
        // authored by `update_foot_ground_probes` (ordered after this), so carry
        // them forward rather than clobbering with `..Default::default()`.
        let previous = current.pose_inputs.unwrap_or_default();
        let new_pose_inputs = PoseInputs {
            aim_pitch,
            aim_yaw,
            heading_yaw,
            feet: previous.feet,
            foot_count: previous.foot_count,
        };
        if current.pose_inputs == Some(new_pose_inputs) {
            continue;
        }
        let mut mesh = current.clone();
        mesh.pose_inputs = Some(new_pose_inputs);
        let _ = registry.set_component(id, mesh);
    }
}

/// The lower-body heading a host or local pawn presents: travel folded by the
/// forward/backward classification against `aim_yaw`. A freshly entered state
/// seeds the classification where it decides a direction, so a backward clip
/// received from the host folds the legs with it; outside the dead zone travel
/// still decides, keeping the twist bounded. Only `owns_direction` writes the
/// result back as the pawn's memory. Stationary pawns face `fallback`.
pub(super) fn presented_player_heading(
    registry: &mut EntityRegistry,
    id: EntityId,
    aim_yaw: f32,
    fallback: f32,
    owns_direction: bool,
) -> f32 {
    let Some(velocity) = registry
        .get_component::<PlayerMovementComponent>(id)
        .ok()
        .map(|movement| movement.velocity)
        .filter(|velocity| velocity.is_finite())
    else {
        return fallback;
    };
    let Some(animation) = registry
        .get_component::<MeshComponent>(id)
        .ok()
        .and_then(|mesh| mesh.animation.as_ref())
    else {
        return fallback;
    };
    let stored = animation.locomotion_direction;
    // Only a freshly entered state seeds the classification. The state can lag
    // this pass (a connected client's own pawn takes it from the host a round
    // trip late), so seeding from it every frame would undo a band crossing the
    // state has not caught up with yet.
    let entering = animation.entered_at.is_none() || animation.previous_state.is_some();
    let previous = entering
        .then(|| PlayerLocomotionStates::from_animation(animation))
        .flatten()
        .and_then(|states| {
            states.direction_of(&animation.current_state, |state| {
                animation.is_state_usable(state)
            })
        })
        .unwrap_or(stored);
    let direction = classify_locomotion_direction(velocity, aim_yaw, previous);
    if owns_direction && direction != stored {
        store_locomotion_direction(registry, id, direction);
    }
    player_travel_heading_yaw(velocity, direction, fallback)
}

/// Model-space downward reach of each foot ground probe, in model units. Ground
/// farther than this below the animated foot reads as no contact — a swing foot
/// with no plantable surface — sized for the roughly unit-tall models the loader
/// ships. Scaled by the entity's model scale at cast time so the bound stays
/// constant in model space regardless of instance scale.
pub(super) const FOOT_PLANTING_REACH: f32 = 0.5;
/// Upward model-space allowance that lets a probe recover a foot already sunk
/// slightly through the floor. The ray starts this far above the sampled foot,
/// then covers this allowance plus [`FOOT_PLANTING_REACH`] downward.
pub(super) const FOOT_PENETRATION_ALLOWANCE: f32 = 0.15;

/// Sample each leg-tagged entity's UNMODIFIED world foot pose, cast a short
/// downward ray at the collision world under each foot, and write the
/// model-space contact into `PoseInputs::feet` for the renderer's IK solver.
///
/// Ordered AFTER [`update_pose_inputs`], which owns the aim/heading fields: this
/// step is the sole writer of `feet`/`foot_count` and read-modify-writes the
/// mesh's existing `pose_inputs` so those aim fields survive. Leg `i` drives foot
/// probe `i`; `foot_count` is the entity's leg-set length even when a foot finds
/// no ground (that foot reports `hit == false`). The world-pose sample is the
/// unmodified pose shared with hit zones — distinct from the renderer's modified
/// palette. Everything runs against the same fixed-tick registry/collision state
/// through the same per-instance seed, so repeated headless runs of a tick
/// sequence produce identical probes.
pub(super) fn update_foot_ground_probes(
    registry: &mut EntityRegistry,
    collision_world: &CollisionWorld,
    mover_colliders: &[MoverCollider],
    mover_poses: &dyn MoverPoseSource,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
) {
    let mut candidates = hit_zone_store.foot_probe_candidates();
    candidates.clear();
    candidates.extend(
        registry
            .iter_with_kind(ComponentKind::Mesh)
            .filter_map(|(id, value)| {
                let ComponentValue::Mesh(mesh) = value else {
                    return None;
                };
                is_foot_probe_candidate(mesh, hit_zone_store).then_some(id)
            }),
    );

    for &id in candidates.iter() {
        let transform = registry
            .get_component::<postretro_entities::Transform>(id)
            .ok()
            .copied();
        write_foot_ground_probes(
            registry,
            id,
            transform,
            collision_world,
            mover_colliders,
            mover_poses,
            hit_zone_store,
            anim_time,
        );
    }
}

/// Re-probe one entity's feet from a caller-supplied `transform` instead of its
/// registry pose. The host seat draws a connected-client pawn from a delayed
/// presented sample, which can still sit at a teleport's source while the live
/// pose is at the arrival; probing from the presented transform keeps the foot
/// contacts on the floor the body is drawn over. Same probe, same RMW of the
/// aim fields, as the per-tick [`update_foot_ground_probes`] pass.
#[allow(clippy::too_many_arguments)] // mirrors the per-tick pass's inputs.
pub fn update_foot_ground_probes_for(
    registry: &mut EntityRegistry,
    entity: EntityId,
    transform: postretro_entities::Transform,
    collision_world: &CollisionWorld,
    mover_colliders: &[MoverCollider],
    mover_poses: &dyn MoverPoseSource,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
) {
    let Ok(mesh) = registry.get_component::<MeshComponent>(entity) else {
        return;
    };
    if !is_foot_probe_candidate(mesh, hit_zone_store) {
        return;
    }
    write_foot_ground_probes(
        registry,
        entity,
        Some(transform),
        collision_world,
        mover_colliders,
        mover_poses,
        hit_zone_store,
        anim_time,
    );
}

/// Legged models probe; a mesh still carrying feet from an earlier model clears.
pub(super) fn is_foot_probe_candidate(mesh: &MeshComponent, hit_zone_store: &HitZoneStore) -> bool {
    let is_legged = hit_zone_store
        .get_by_name(&mesh.model)
        .is_some_and(|model| !model.legs.is_empty());
    let has_stale_feet = mesh.pose_inputs.is_some_and(|inputs| {
        inputs.foot_count != 0
            || inputs
                .feet
                .iter()
                .any(|probe| *probe != FootProbe::default())
    });
    is_legged || has_stale_feet
}

/// Probe one candidate's feet from `transform` (`None` publishes no contacts)
/// and write them into its `PoseInputs`. The model sits where it is drawn:
/// `transform.position + origin_offset` at `transform.scale`, turned by
/// [`foot_probe_rotation`], so the probes plant the drawn feet.
///
/// The aim and heading fields are read-modify-written. An animated mesh with no
/// pose inputs yet takes them from `transform` ([`unposed_pose_inputs`]) rather
/// than zero: a listen host re-probes a client pawn accepted on a render frame
/// that ran no fixed tick, and an animated body draws at its heading, so a zero
/// heading would draw it at yaw 0, not where it faces, until the next pose pass.
#[allow(clippy::too_many_arguments)] // a flat parameter list keeps this a leaf helper.
pub(super) fn write_foot_ground_probes(
    registry: &mut EntityRegistry,
    id: EntityId,
    transform: Option<postretro_entities::Transform>,
    collision_world: &CollisionWorld,
    mover_colliders: &[MoverCollider],
    mover_poses: &dyn MoverPoseSource,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
) {
    let Ok(mesh) = registry.get_component::<MeshComponent>(id) else {
        return;
    };
    let zones = hit_zone_store.get_by_name(&mesh.model);
    let mut foot_count = 0;
    let mut feet = [FootProbe::default(); MAX_FEET];
    if let (Some(zones), Some(transform)) = (zones, transform) {
        let model_rotation = foot_probe_rotation(mesh, transform.rotation);
        if let Some(model_to_world) =
            model_matrix_at(&transform, model_rotation, mesh.origin_offset)
            && let Some(world_to_model) =
                foot_probe_inverse(&transform, model_rotation, &model_to_world)
        {
            let world_joints =
                sample_world_pose_for_probe(zones, mesh.animation.as_ref(), anim_time, id.to_raw());
            if let Some(world_joints) = world_joints.as_ref() {
                foot_count = zones.legs.len().min(MAX_FEET);
                let downward_reach = FOOT_PLANTING_REACH * transform.scale.y;
                let upward_allowance = FOOT_PENETRATION_ALLOWANCE * transform.scale.y;
                for (slot, leg) in zones.legs.iter().take(foot_count).enumerate() {
                    feet[slot] = probe_foot(
                        leg.foot_joint,
                        world_joints,
                        &model_to_world,
                        &world_to_model,
                        downward_reach,
                        upward_allowance,
                        collision_world,
                        mover_colliders,
                        mover_poses,
                    );
                }
            }
        }
    }

    // Read-modify-write: keep the aim/heading fields update_pose_inputs set. A
    // mesh with none yet takes them from the probed transform (see the doc).
    let mut new_inputs = mesh
        .pose_inputs
        .unwrap_or_else(|| unposed_pose_inputs(mesh, transform));
    new_inputs.feet = feet;
    new_inputs.foot_count = foot_count as u8;
    if mesh.pose_inputs == Some(new_inputs) {
        return;
    }
    let Ok(mut mesh) = registry.get_component::<MeshComponent>(id).cloned() else {
        return;
    };
    mesh.pose_inputs = Some(new_inputs);
    let _ = registry.set_component(id, mesh);
}

/// Pose inputs for a mesh [`update_pose_inputs`] has not reached. An animated
/// mesh with a transform takes that pass's no-target fallback: aim and heading
/// both on the transform's yaw, so the body draws where the transform faces.
/// The probe placed such a mesh at the transform's own rotation, which for an
/// upright transform is that yaw, so the feet stay under the drawn body; a
/// tilted transform probes no contacts. A stateless mesh never draws its
/// heading and keeps the zero default, as does a missing transform.
fn unposed_pose_inputs(
    mesh: &MeshComponent,
    transform: Option<postretro_entities::Transform>,
) -> PoseInputs {
    let Some(transform) = transform.filter(|_| mesh.animation.is_some()) else {
        return PoseInputs::default();
    };
    let (yaw, _, _) = transform.rotation.to_euler(EulerRot::YXZ);
    let yaw = if yaw.is_finite() { yaw } else { 0.0 };
    PoseInputs {
        aim_yaw: yaw,
        heading_yaw: yaw,
        ..PoseInputs::default()
    }
}

/// Rotation the foot probes place the model at. A view-frame body (a player)
/// probes under its drawn rotation, [`MeshComponent::presentation_rotation`]:
/// the lower-body heading its pose inputs carry at probe time, plus the body
/// turn. A model-frame body keeps its transform rotation and never takes the
/// turn; an upright enemy's drawn heading is that rotation's own yaw.
/// See: context/lib/entity_model.md §4 (Facing convention)
pub(super) fn foot_probe_rotation(
    mesh: &MeshComponent,
    transform_rotation: glam::Quat,
) -> glam::Quat {
    match mesh.body_frame {
        BodyFrame::Model => transform_rotation,
        BodyFrame::View => mesh.presentation_rotation(transform_rotation),
    }
}

/// Probe one foot: transform its animated model-space origin to world, cast a
/// bounded downward ray, and convert a walkable hit back to model space for the
/// solver. Returns a miss (`FootProbe::default`) when the foot joint is absent,
/// no ground lies within `reach` below the foot, or the surface is too steep to
/// stand on (the same `COS_WALKABLE` floor threshold movement ground-stick uses).
#[allow(clippy::too_many_arguments)] // a flat parameter list keeps this a leaf helper.
pub(super) fn probe_foot(
    foot_joint: usize,
    world_joints: &[Mat4],
    model_to_world: &Mat4,
    world_to_model: &Mat4,
    downward_reach: f32,
    upward_allowance: f32,
    collision_world: &CollisionWorld,
    mover_colliders: &[MoverCollider],
    mover_poses: &dyn MoverPoseSource,
) -> FootProbe {
    let miss = FootProbe::default();
    let Some(foot) = world_joints.get(foot_joint) else {
        return miss;
    };
    let foot_world = model_to_world.transform_point3(foot.w_axis.truncate());
    if !foot_world.is_finite() || downward_reach <= 0.0 || upward_allowance < 0.0 {
        return miss;
    }

    let ray_origin = foot_world + Vec3::Y * upward_allowance;
    let max_toi = upward_allowance + downward_reach;
    let origin = ray_origin;
    let down = Vec3::NEG_Y;
    // Static-only fast path; fold movers in only when present.
    let hit = if mover_colliders.is_empty() {
        cast_ray(collision_world, origin, down, max_toi).map(|h| (h.time_of_impact, h.normal))
    } else {
        cast_ray_combined(
            collision_world,
            mover_colliders,
            mover_poses,
            origin,
            down,
            max_toi,
        )
        .map(|h| (h.time_of_impact, h.normal))
    };
    let Some((toi, normal_world)) = hit else {
        return miss;
    };

    // Walkable-normal convention: ground under the foot must face mostly up.
    if !is_walkable_ground_normal(normal_world) {
        return miss;
    }

    let contact_world = ray_origin + Vec3::new(0.0, -toi, 0.0);
    let contact_model = world_to_model.transform_point3(contact_world);
    // A world normal converted back to model space uses the transpose of the
    // model→world linear map. Using the inverse as a direction is wrong for
    // non-uniform scale.
    let normal_model = model_to_world
        .transpose()
        .transform_vector3(normal_world)
        .normalize_or_zero();
    if !contact_model.is_finite() || normal_model == Vec3::ZERO {
        return miss;
    }
    FootProbe {
        contact_height: contact_model.y,
        normal: normal_model,
        hit: true,
    }
}

pub(super) fn is_walkable_ground_normal(normal: Vec3) -> bool {
    normal.is_finite() && normal.y >= COS_WALKABLE
}

/// Foot contacts use model-space height, so v1 supports upright placements with
/// positive, invertible scale (uniform or non-uniform). Tilted models would make
/// a world-down ray change model XZ while the current `FootProbe` stores only a
/// height; singular transforms have no valid inverse. Both report fresh misses.
/// Upright is read from `model_to_world`'s own up axis, and normalization from
/// `model_rotation`, the composed rotation that matrix was built from (see
/// [`foot_probe_rotation`]), so both hold for the rotation the probe actually
/// places the model at. A model-frame body's is its transform rotation.
pub(super) fn foot_probe_inverse(
    transform: &postretro_entities::Transform,
    model_rotation: glam::Quat,
    model_to_world: &Mat4,
) -> Option<Mat4> {
    if !transform.scale.is_finite()
        || transform.scale.min_element() <= 0.0
        || !model_rotation.is_normalized()
    {
        return None;
    }
    let model_up = model_to_world.y_axis.truncate().normalize_or_zero();
    if model_up.dot(Vec3::Y) < 1.0 - 1.0e-5 {
        return None;
    }
    let inverse = model_to_world.inverse();
    inverse
        .to_cols_array()
        .iter()
        .all(|value| value.is_finite())
        .then_some(inverse)
}

#[cfg(test)]
#[path = "pose_inputs_body_frame_tests.rs"]
mod body_frame_tests;
