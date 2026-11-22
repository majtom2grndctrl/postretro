// Remote presentation: sample each remote's interpolation buffer, place riders
// on their movers, and drive remote avatar locomotion and animation.
// See: context/lib/networking.md §Host-side remote-pawn presentation

use super::*;

const MIN_PRESENTED_HORIZONTAL_SPEED_SQ: f32 = 1.0e-8;

fn horizontal_speed(velocity: Vec3) -> f32 {
    Vec3::new(velocity.x, 0.0, velocity.z).length()
}

fn is_horizontal_motion(velocity: &Vec3) -> bool {
    velocity.x * velocity.x + velocity.z * velocity.z > MIN_PRESENTED_HORIZONTAL_SPEED_SQ
}

impl ClientReplication {
    /// Sample every mapped remote entity's interpolation buffer at the render server
    /// tick `render_server_tick` (already `estimated_server_tick - interpolation_delay`)
    /// and write the resolved pose through the registry's remote-presentation helper.
    ///
    /// Game-logic-owned: runs after this frame's network receive/apply and before the
    /// render collectors read entities (the renderer stays read-only). Each write sets
    /// the entity's visible `Transform` to the freshly-interpolated pose. The buffer
    /// already resolved that pose at the correct *server-time* target, so the
    /// remote-presentation write is *alpha-agnostic*: it collapses previous == current
    /// and the render-stage `interpolated_transform` blend reproduces the pose verbatim
    /// at any frame alpha (the sim sub-tick fraction is an unrelated time base and must
    /// not re-blend an already-resolved pose — see
    /// `EntityRegistry::set_presentation_transform`). An entity with no buffered
    /// samples yet is left at its last-applied pose. Returns presentation stats for
    /// diagnostics and adaptive delay feedback.
    ///
    /// A rider whose bracketing samples stand on one mover is placed relative to
    /// that mover's presented pose at `mover_alpha`, the frame's render alpha
    /// (`crate::rider_rebase`); any other remote keeps its sampled world pose.
    pub(crate) fn sample_into_registry(
        &mut self,
        registry: &mut EntityRegistry,
        render_server_tick: f64,
        frame_anim_time: f64,
        mover_alpha: f32,
    ) -> InterpolationSampleStats {
        let mut stats = InterpolationSampleStats::default();
        // Diagnostics-only: open this render frame's interpolation window at the
        // resolved target tick. No effect on the sampled poses.
        self.netdiag.begin_frame(render_server_tick);
        self.presented_player_inputs.aim_pitches.clear();
        self.presented_player_inputs.heading_yaws.clear();
        self.presented_state_ticks.clear();
        // Collect (network_id, entity_id) first to avoid borrowing `self.map` while
        // writing back through the registry.
        let mapped: Vec<(NetworkId, EntityId)> = self.map.iter().map(|(&n, &e)| (n, e)).collect();
        for (network_id, entity_id) in mapped {
            if !registry.exists(entity_id) {
                continue;
            }
            // The local predicted pawn is prediction/reconcile-driven, not remote-
            // interpolated: skip it so its reconciled pose is not clobbered.
            if self.local_pawn == Some(network_id) {
                continue;
            }
            if (self.bindings.mover_network_ids.contains_key(&network_id)
                || self.bindings.region_network_ids.contains_key(&network_id))
                || self.bindings.is_presentation_bound(network_id)
            {
                continue;
            }
            let Some(pose) = self.interp.presented_pose(network_id, render_server_tick) else {
                continue; // no samples buffered yet
            };
            self.presented_state_ticks
                .insert(network_id, pose.state_tick);
            let mut pose = pose;
            if let Some(rebased) = pose.rider_bracket.and_then(|bracket| {
                crate::rider_rebase::rebase_rider(
                    &bracket,
                    &pose.transform,
                    |mover_id, tick| {
                        self.mover_history
                            .authoritative_transform_at_or_before(mover_id, tick)
                    },
                    |mover_id| {
                        crate::rider_rebase::presented_mover(registry, mover_id, mover_alpha)
                    },
                )
            }) {
                pose.transform = rebased.transform;
                pose.intrinsic_velocity = pose
                    .intrinsic_velocity
                    .map(|velocity| rebased.turn(velocity));
            }
            let mut transform = pose.transform;
            if let Some(facts) = self.projectile_presentations.get(&network_id)
                && facts.sprite_size.is_none()
            {
                transform.scale = Vec3::splat(facts.model_scale);
            }
            let _ = registry.set_presentation_transform(entity_id, transform);
            self.present_remote_player_capsule(
                registry,
                network_id,
                entity_id,
                pose.capsule_half_height,
            );
            // Diagnostics-only: record the presented remote pose to detect stepped /
            // low-effective-rate interpolation output (position + orientation).
            self.netdiag.record_remote(
                network_id.0,
                pose.transform.position,
                pose.transform.rotation,
            );
            stats.presented += 1;
            self.update_remote_enemy_walk_playback_rate(
                registry,
                network_id,
                entity_id,
                pose.speed_xz,
                frame_anim_time,
            );
            if self.remote_player_locomotion.contains_key(&network_id) {
                if pose.aim_pitch.is_finite() {
                    self.presented_player_inputs
                        .aim_pitches
                        .insert(network_id, pose.aim_pitch);
                }
                let intrinsic_velocity = pose
                    .intrinsic_velocity
                    .filter(|velocity| velocity.is_finite());
                let (aim_yaw, _, _) = pose.transform.rotation.to_euler(glam::EulerRot::YXZ);
                self.apply_due_remote_player_correction(
                    registry,
                    network_id,
                    entity_id,
                    Some(render_server_tick),
                );
                let direction = self.update_remote_player_locomotion(
                    registry,
                    network_id,
                    entity_id,
                    intrinsic_velocity.unwrap_or(Vec3::ZERO),
                    pose.stance,
                    aim_yaw,
                    frame_anim_time,
                    RemotePlayerPresentation::Interpolated,
                );
                if let Some(velocity) = intrinsic_velocity.filter(is_horizontal_motion) {
                    let heading_yaw =
                        crate::sim::player_travel_heading_yaw(velocity, direction, 0.0);
                    self.presented_player_inputs
                        .heading_yaws
                        .insert(network_id, heading_yaw);
                }
            }
            // Diagnostic: a HeldNewest after sustained starvation is the visible
            // freeze the buffer falls back to; logged sparingly at trace.
            if pose.holds_newest {
                stats.held_newest += 1;
                if self
                    .interp
                    .held_newest_needs_feedback(network_id, self.acked_server_tick)
                {
                    stats.starvation_feedback += 1;
                }
                log::trace!(
                    "[Net] remote {network_id:?} holding last pose (interp buffer starved)"
                );
            }
        }
        stats
    }

    /// Apply held-pose walk playback before time sync can name a render tick.
    /// The baseline pose already visible in the registry is frozen until the
    /// estimator initializes, so its displayed XZ speed is zero.
    pub(crate) fn apply_held_remote_enemy_walk_playback_rates(
        &self,
        registry: &mut EntityRegistry,
        frame_anim_time: f64,
    ) {
        for (&network_id, &entity_id) in &self.map {
            if !registry.exists(entity_id)
                || self.local_pawn == Some(network_id)
                || (self.bindings.mover_network_ids.contains_key(&network_id)
                    || self.bindings.region_network_ids.contains_key(&network_id))
                || self.bindings.is_presentation_bound(network_id)
            {
                continue;
            }
            self.update_remote_enemy_walk_playback_rate(
                registry,
                network_id,
                entity_id,
                0.0,
                frame_anim_time,
            );
        }
    }

    /// Before time sync can name a presentation tick, remote avatars hold their
    /// newest applied pose. Preserve that pose's pitch, clear motion-derived
    /// heading, and select client-local locomotion for a body at rest: the held
    /// pose shows no travel, though its newest posture and footing still pick
    /// crouch and jump states. With no presented tick to wait for, every queued host state is due
    /// and the newest applies, as it would have on receipt.
    pub(crate) fn apply_held_remote_player_presentation(
        &mut self,
        registry: &mut EntityRegistry,
        frame_anim_time: f64,
    ) {
        self.presented_player_inputs.aim_pitches.clear();
        self.presented_player_inputs.heading_yaws.clear();
        // Held poses show the newest applied snapshot; no per-remote tick.
        self.presented_state_ticks.clear();
        let mapped: Vec<(NetworkId, EntityId)> = self.map.iter().map(|(&n, &e)| (n, e)).collect();
        for (network_id, entity_id) in mapped {
            if !registry.exists(entity_id)
                || self.local_pawn == Some(network_id)
                || (self.bindings.mover_network_ids.contains_key(&network_id)
                    || self.bindings.region_network_ids.contains_key(&network_id))
                || self.bindings.is_presentation_bound(network_id)
            {
                continue;
            }
            // The held body is the newest applied pose, so it stands on the
            // newest replicated capsule.
            let capsule_half_height = self.interp.newest_capsule_half_height(network_id);
            self.present_remote_player_capsule(
                registry,
                network_id,
                entity_id,
                capsule_half_height,
            );
            if !self.remote_player_locomotion.contains_key(&network_id) {
                continue;
            }
            if let Some(aim_pitch) = self.interp.newest_aim_pitch(network_id) {
                self.presented_player_inputs
                    .aim_pitches
                    .insert(network_id, aim_pitch);
            }
            let stance = self.interp.newest_stance(network_id);
            self.apply_due_remote_player_correction(registry, network_id, entity_id, None);
            self.update_remote_player_locomotion(
                registry,
                network_id,
                entity_id,
                Vec3::ZERO,
                stance,
                0.0,
                frame_anim_time,
                RemotePlayerPresentation::HeldNewest,
            );
        }
    }

    pub(crate) fn cache_remote_presentation_complete(
        &mut self,
        network_id: NetworkId,
        complete: bool,
    ) {
        if complete && self.map.contains_key(&network_id) {
            self.remote_presentation_complete.insert(network_id);
        } else {
            self.remote_presentation_complete.remove(&network_id);
        }
    }

    /// Record (or clear) the immutable descriptor data used to derive a remote
    /// enemy's stride playback rate for every rate-scaled state. Called by
    /// descriptor-aware receive glue, while
    /// all per-frame registry writes remain in [`Self::sample_into_registry`].
    pub(crate) fn cache_remote_enemy_walk_playback(
        &mut self,
        network_id: NetworkId,
        reference: Option<RemoteEnemyWalkPlayback>,
    ) {
        match reference {
            Some(reference) => {
                self.remote_enemy_walk_playback
                    .insert(network_id, reference);
            }
            None => {
                self.remote_enemy_walk_playback.remove(&network_id);
            }
        }
    }

    /// Retain the descriptor identity already carried by the snapshot while
    /// descriptor-aware presentation materialization has it in scope. The cache keeps
    /// the class tag; readers decode it, so the representation (pawn, projectile or
    /// descriptor entity) stays available beside the canonical name.
    pub(crate) fn cache_remote_entity_class(&mut self, network_id: NetworkId, entity_class: &str) {
        self.remote_entity_classes
            .insert(network_id, entity_class.to_string());
    }

    /// Record (or clear) a remote player avatar's descriptor capsule radius,
    /// beside its locomotion reference at materialization.
    pub(crate) fn cache_remote_player_capsule_radius(
        &mut self,
        network_id: NetworkId,
        radius: Option<f32>,
    ) {
        match radius.filter(|radius| radius.is_finite()) {
            Some(radius) => {
                self.remote_player_capsule_radii.insert(network_id, radius);
            }
            None => {
                self.remote_player_capsule_radii.remove(&network_id);
            }
        }
    }

    /// Seat a remote player avatar's feet-authored mesh on the sole of the
    /// capsule presented with its pose. The transform is the host's capsule
    /// centre, which a crouch or low life-state posture lowers with the feet
    /// planted; the materialized offset is the standing one, so without this
    /// the crouched body sinks into the floor and its cull point leaves every
    /// visible cell. A pose with no movement state keeps the current offset.
    pub(super) fn present_remote_player_capsule(
        &self,
        registry: &mut EntityRegistry,
        network_id: NetworkId,
        entity_id: EntityId,
        capsule_half_height: Option<f32>,
    ) {
        if let Some(&radius) = self.remote_player_capsule_radii.get(&network_id)
            && let Some(half_height) = capsule_half_height
        {
            crate::sim::set_player_mesh_capsule_origin(registry, entity_id, radius, half_height);
        }
    }

    /// Record immutable descriptor locomotion data after remote-player mesh
    /// materialization. `None` leaves an unusual/meshless descriptor transform-only
    /// and prevents it from being mistaken for an avatar by the pose-input pass.
    pub(crate) fn cache_remote_player_locomotion(
        &mut self,
        network_id: NetworkId,
        reference: Option<RemotePlayerLocomotionReference>,
    ) {
        match reference {
            // Re-materializing a mapped avatar (a weapon swap) refreshes the
            // descriptor data and keeps any correction it is honoring.
            Some(reference) => match self.remote_player_locomotion.get_mut(&network_id) {
                Some(locomotion) => locomotion.reference = reference,
                None => {
                    self.remote_player_locomotion
                        .insert(network_id, RemotePlayerLocomotion::new(reference));
                }
            },
            None => {
                self.remote_player_locomotion.remove(&network_id);
            }
        }
    }

    /// Seed a freshly materialized remote avatar's forward/backward memory from
    /// the host's state. Inside the dead zone the memory decides, so an unseeded
    /// joiner would first derive the opposite fold and flip once the next delta
    /// corrects it. Names are checked as declared: the host enters a backward
    /// state only when its clip resolves.
    pub(crate) fn seed_remote_player_direction(
        &self,
        registry: &mut EntityRegistry,
        network_id: NetworkId,
        entity_id: EntityId,
        state: &str,
    ) {
        let Some(locomotion) = self.remote_player_locomotion.get(&network_id) else {
            return;
        };
        if let Some(direction) = locomotion.reference.states.direction_of(state, |_| true) {
            crate::sim::store_locomotion_direction(registry, entity_id, direction);
        }
    }

    /// Apply the newest queued host state for a remote player that the
    /// presentation has reached: every one at or before `presented_tick`, or all
    /// of them for `None`.
    pub(super) fn apply_due_remote_player_correction(
        &mut self,
        registry: &mut EntityRegistry,
        network_id: NetworkId,
        entity_id: EntityId,
        presented_tick: Option<f64>,
    ) {
        let due = self
            .remote_player_locomotion
            .get_mut(&network_id)
            .and_then(|locomotion| locomotion.take_due_correction(presented_tick));
        if let Some(state) = due
            && !self
                .apply_remote_player_animation_correction(registry, network_id, entity_id, &state)
        {
            log::trace!(
                "[Net] remote player {network_id:?} did not take mesh animation state `{state}`"
            );
        }
    }

    /// Apply host animation authority for a remote player. A disagreement with the
    /// client-local velocity prediction deliberately enters through the normal
    /// transition API, preserving the descriptor's short avatar crossfade instead
    /// of replacing the displayed pose with a hard snap.
    ///
    /// Jump one-shots are the exception: this client times them from its own
    /// presented takeoff and landing, and the host stamps its own with the frame's
    /// animation clock, so after a host hitch its `jump_up` or `jump_down` can
    /// outlast this client's. A host one-shot applies only while this client's
    /// phase still plays it, never re-entering one the client has left.
    pub(super) fn apply_remote_player_animation_correction(
        &mut self,
        registry: &mut EntityRegistry,
        network_id: NetworkId,
        entity_id: EntityId,
        authoritative_state: &str,
    ) -> bool {
        let Some(locomotion) = self.remote_player_locomotion.get(&network_id) else {
            return false;
        };
        let phase = registry
            .get_component::<MeshComponent>(entity_id)
            .ok()
            .and_then(|mesh| mesh.animation.as_ref())
            .map(|animation| animation.locomotion_jump)
            .unwrap_or_default();
        if !locomotion
            .reference
            .states
            .admits_state(authoritative_state, phase)
        {
            // The host has named another state, so a hold on an earlier one is
            // stale; released, this client's own derivation, already in the
            // matching phase, switches on the same frame.
            if let Some(locomotion) = self.remote_player_locomotion.get_mut(&network_id)
                && locomotion
                    .authoritative_correction_state
                    .as_deref()
                    .is_some_and(|held| held != authoritative_state)
            {
                locomotion.authoritative_correction_state = None;
            }
            return false;
        }
        let predicted_state = locomotion.client_derived_state.as_deref();
        let correcting = predicted_state.is_some_and(|predicted| predicted != authoritative_state);
        if correcting {
            log::trace!(
                "[Net] remote player {network_id:?} correcting locomotion {predicted_state:?} -> {authoritative_state}"
            );
        }
        let applied = apply_mesh_animation_state(registry, entity_id, authoritative_state, true);
        if applied
            && correcting
            && let Some(reference) = self.remote_player_locomotion.get_mut(&network_id)
        {
            reference.authoritative_correction_state = Some(authoritative_state.to_string());
        }
        applied
    }

    /// Apply the rate for one presented remote pose. Only an enemy whose current
    /// mesh state is one of the descriptor graph's rate-scaled states, or a
    /// directional variant of one, receives its displayed XZ speed; every other state
    /// plays its fixed `playbackRate` (default 1). That speed includes knockback, which the
    /// host's steering-velocity rate excludes (sim
    /// `update_brain_animation_playback_rates`): a known divergence until
    /// enemies carry velocity on the wire.
    pub(super) fn update_remote_enemy_walk_playback_rate(
        &self,
        registry: &mut EntityRegistry,
        network_id: NetworkId,
        entity_id: EntityId,
        speed_xz: f32,
        frame_anim_time: f64,
    ) {
        let Some(reference) = self.remote_enemy_walk_playback.get(&network_id) else {
            return;
        };
        // The host's per-state policy (`MeshAnimation::stride_playback_target`)
        // over the same local descriptor fields: `scaleWithSpeed: false` opts
        // out, `maxPlaybackRate` bounds the clamp, and an unscaled state plays
        // its fixed `playbackRate` from the frame it is entered.
        let Some(target) = registry
            .get_component::<MeshComponent>(entity_id)
            .ok()
            .and_then(|mesh| mesh.animation.as_ref())
            .and_then(|animation| {
                let stride_ratio = reference
                    .derived_speed_of(&animation.current_state)
                    .filter(|_| animation.speed_scale)
                    .map(|derived| {
                        let effective = animation
                            .states
                            .get(&animation.current_state)
                            .map_or(derived, |state| state.effective_travel_speed(derived));
                        MeshAnimation::locomotion_rate_ratio(
                            speed_xz,
                            effective,
                            reference.move_speed,
                        )
                    });
                let target = animation.stride_playback_target(stride_ratio);
                animation
                    .playback_rate_target_needs_update(target)
                    .then_some(target)
            })
        else {
            return;
        };

        // `sample_into_registry` intentionally allocates its mapped-id vector
        // before this helper to split the map and registry borrows. This helper
        // itself clones the mesh only when the animation predicate says a rebase
        // write is necessary.
        let Ok(mut mesh) = registry.get_component::<MeshComponent>(entity_id).cloned() else {
            return;
        };
        let Some(animation) = mesh.animation.as_mut() else {
            return;
        };
        animation.apply_playback_rate_target(target, frame_anim_time);
        let _ = registry.set_component(entity_id, mesh);
    }

    /// Derive one remote avatar's locomotion state and rate from the interpolated
    /// intrinsic player velocity, classified forward or backward against the
    /// presented aim, and the replicated posture and footing presented with it. The
    /// host's mesh state remains the correction authority; this is only the
    /// responsive between-snapshot path. The jump phase advances from the
    /// presented footing even while a correction is held, so the sequence memory
    /// follows the presented motion rather than the host state. Returns the
    /// direction the avatar's legs present this frame.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn update_remote_player_locomotion(
        &mut self,
        registry: &mut EntityRegistry,
        network_id: NetworkId,
        entity_id: EntityId,
        velocity: Vec3,
        stance: crate::sim::LocomotionStance,
        aim_yaw: f32,
        frame_anim_time: f64,
        presentation: RemotePlayerPresentation,
    ) -> LocomotionDirection {
        let Some(locomotion) = self.remote_player_locomotion.get_mut(&network_id) else {
            return LocomotionDirection::Forward;
        };
        // A held newest pose is ahead of the first interpolated one, so the
        // footing would run backward across the handover: a held rise would read
        // its older grounded tick as a landing. That first interpolated pose
        // resyncs the phase from its own footing instead.
        let resync_jump_phase = match presentation {
            RemotePlayerPresentation::HeldNewest => {
                locomotion.resync_jump_phase = true;
                false
            }
            RemotePlayerPresentation::Interpolated => locomotion.resync_jump_phase,
        };
        let speed_xz = horizontal_speed(velocity);
        let Some(animation) = registry
            .get_component::<MeshComponent>(entity_id)
            .ok()
            .and_then(|mesh| mesh.animation.as_ref())
        else {
            return LocomotionDirection::Forward;
        };
        // Consumed only once a mesh animation can take the resync.
        if resync_jump_phase {
            locomotion.resync_jump_phase = false;
        }
        let reference = &locomotion.reference;
        let usable = |state: &str| animation.is_state_usable(state);
        let stored_direction = animation.locomotion_direction;
        let stored_jump = animation.locomotion_jump;
        let current_state = animation.current_state.as_str();
        // A host correction is honored until this client's own derivation agrees
        // with it rather than for its crossfade alone: releasing earlier reverts
        // to the disagreeing prediction, and the host's next resend corrects
        // again.
        let correction_held = locomotion
            .authoritative_correction_state
            .as_deref()
            .is_some_and(|authoritative| authoritative == current_state);
        // The held state seeds the classification rather than overriding it:
        // inside the dead zone the legs fold with the corrected clip, and outside
        // it travel still decides, keeping the twist bounded while the presented
        // motion catches up.
        let previous = if correction_held {
            reference
                .states
                .direction_of(current_state, usable)
                .unwrap_or(stored_direction)
        } else {
            stored_direction
        };
        let direction = crate::sim::classify_locomotion_direction(velocity, aim_yaw, previous);
        let mut input = crate::sim::LocomotionInput {
            speed_xz,
            walk_speed: reference.walk_speed,
            run_speed: reference.run_speed,
            direction,
            posture: stance.posture,
            footing: stance.footing,
            jump_phase: stored_jump,
            current_state,
        };
        input.jump_phase = if resync_jump_phase {
            reference.states.resync_jump_phase(stance.footing, usable)
        } else {
            reference
                .states
                .advance_jump_phase(&input, frame_anim_time, usable, |state| {
                    reference.clip_durations.get(state).copied()
                })
        };
        let jump = input.jump_phase;
        let (target_state, tier) = reference.states.select(&input, usable);
        let target_state = target_state.unwrap_or(&reference.idle_state);
        let at_target = current_state == target_state;
        let tier_speed = match tier {
            crate::sim::LocomotionTierKind::Idle
            | crate::sim::LocomotionTierKind::Slide
            | crate::sim::LocomotionTierKind::Jump => None,
            crate::sim::LocomotionTierKind::Walk => Some(reference.walk_speed),
            crate::sim::LocomotionTierKind::Run => Some(reference.run_speed),
            crate::sim::LocomotionTierKind::Crouch => Some(reference.crouch_speed),
        };
        let moving = tier_speed.is_some();
        let derived_travel_speed =
            tier_speed.and_then(|_| reference.derived_travel_speeds.get(target_state).copied());
        let fallback_speed = tier_speed.unwrap_or(1.0);

        if locomotion.client_derived_state.as_deref() != Some(target_state) {
            locomotion.client_derived_state = Some(target_state.to_string());
        }
        if !correction_held || at_target {
            locomotion.authoritative_correction_state = None;
        }
        // A held correction plays the host's state at its authored rate.
        let target_reached = at_target
            || (!correction_held
                && matches!(
                    switch_animation_state(registry, entity_id, target_state),
                    SwitchResult::Switched | SwitchResult::AlreadyInState
                ));
        if direction != stored_direction || jump != stored_jump {
            crate::sim::store_locomotion_memory(registry, entity_id, direction, Some(jump));
        }

        let raw_ratio = registry
            .get_component::<MeshComponent>(entity_id)
            .ok()
            .and_then(|mesh| mesh.animation.as_ref())
            .and_then(|animation| {
                if !moving || !target_reached || !animation.speed_scale {
                    return Some(1.0);
                }
                let state = animation.states.get(target_state)?;
                let effective = state.effective_travel_speed(derived_travel_speed);
                Some(MeshAnimation::locomotion_rate_ratio(
                    speed_xz.max(0.0),
                    effective,
                    fallback_speed,
                ))
            });
        let needs_rebase = raw_ratio.is_some_and(|raw_ratio| {
            registry
                .get_component::<MeshComponent>(entity_id)
                .ok()
                .and_then(|mesh| mesh.animation.as_ref())
                .is_some_and(|animation| animation.playback_rate_needs_update(raw_ratio))
        });
        if let Some(raw_ratio) = raw_ratio.filter(|_| needs_rebase)
            && let Ok(mut mesh) = registry.get_component::<MeshComponent>(entity_id).cloned()
            && let Some(animation) = mesh.animation.as_mut()
        {
            animation.update_playback_rate(raw_ratio, frame_anim_time);
            let _ = registry.set_component(entity_id, mesh);
        }
        direction
    }

    pub(crate) fn presented_player_inputs(&self) -> &ClientPresentationInputs {
        &self.presented_player_inputs
    }

    /// The presented pose source for a mapped entity at a render tick (tests).
    #[cfg(test)]
    pub(crate) fn presented_source(
        &self,
        network_id: NetworkId,
        render_server_tick: f64,
    ) -> Option<crate::interpolation::PoseSource> {
        self.interp
            .presented_pose(network_id, render_server_tick)
            .map(|p| p.source)
    }

    /// The host tick, in snapshot-stamp units, whose replicated state `entity`'s
    /// presented pose shows this frame: the render tick, except across a blend
    /// (its leading sample) or on the near side of a teleport cut (before the
    /// cut's tick). `None` for an entity not sampled this frame, and for every
    /// remote before time sync names a render tick.
    pub fn presented_state_tick(&self, entity: EntityId) -> Option<f64> {
        let network_id = self.network_id_for_entity(entity)?;
        self.presented_state_ticks.get(&network_id).copied()
    }
}
