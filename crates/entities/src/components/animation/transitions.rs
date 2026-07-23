// Registry-mutating animation transition verbs: switch, restart, and the
// network-staging of a declared-but-unresolved state.
// See: context/lib/scripting.md §10.3 (Mesh Animation)

use crate::components::mesh::MeshComponent;
use crate::registry::{EntityId, EntityRegistry, RegistryError};

use super::{FadeSourceKind, InterruptPolicy, InterruptedOutgoing, MeshAnimation};

impl MeshAnimation {
    /// Stage a declared state whose clip has not resolved yet. Network replication
    /// can receive this state before level-load clip resolution completes; it must
    /// take the same incoming-timeline reset as every other state entry so an old
    /// locomotion rebase cannot leak into the later-resolved clip.
    pub fn stage_unresolved_state(&mut self, target: &str) -> bool {
        if !self.states.contains_key(target) || self.is_state_usable(target) {
            return false;
        }

        self.current_state = target.to_string();
        self.entered_at = None;
        self.previous_state = None;
        self.previous_entered_at = None;
        self.clear_previous_playback_time();
        self.fade_source = FadeSourceKind::Clip;
        self.interrupted_outgoing = None;
        self.reset_incoming_playback_time();
        true
    }

    /// A state is usable for switching only when it is declared *and* its clip
    /// resolved at level load (`clip_index.is_some()`).
    fn is_state_usable(&self, state: &str) -> bool {
        self.states
            .get(state)
            .is_some_and(|s| s.clip_index.is_some())
    }
}

/// Outcome of a switch attempt. The caller (the `setAnimationState` reaction)
/// logs the failure variants; this mirrors the `setEmitterRate`
/// validated-setter precedent (validate here, let the caller surface warnings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchResult {
    /// Intent recorded: target state, pending entry stamp, and previous state.
    Switched,
    /// The entity already sits in the target state; no change recorded.
    AlreadyInState,
    /// The entity carries no `MeshComponent`, or it is a stateless `prop_mesh`
    /// entity with no animation block.
    NotAnimated,
    /// The target state is not declared, or its clip did not resolve at level
    /// load (unusable). Current state is unchanged.
    UnknownState,
}

/// Switch an entity's animation state by name. The single validated path the
/// `setAnimationState` reaction, the future AI plan, and future command-buffer
/// guards all route through.
///
/// Records the target state, a pending entry stamp, the previous state, the new
/// fade's SOURCE KIND (per the entered state's interrupt policy: a smooth
/// interrupt of an active fade records `Snapshot`, every other switch records
/// `Clip`), and — on a smooth interrupt only — the interrupted fade's OUTGOING
/// source in `interrupted_outgoing` (the leg that would otherwise be dropped when
/// `previous_state` is overwritten). The per-frame capture inputs (the in-flight
/// blend the snapshot freezes) are computed downstream by the render-frame
/// collector, after the resolve pass fills the pending stamps — so the last
/// same-tick target wins trivially and the resolved stamps give clip-local times
/// a concrete origin.
///
/// Pending-stamp collapse: if the current state's entry stamp is still pending
/// (a switch landed this same tick and the resolve pass has not run), the
/// never-rendered intermediate is dropped — it contributes no fade, and the
/// source records `Clip` (no in-flight pose to capture). A hard cut (no fade)
/// also applies when switching *out of* an unresolved/unusable current state:
/// there is no outgoing pose to preserve.
pub fn switch_animation_state(
    registry: &mut EntityRegistry,
    id: EntityId,
    target: &str,
) -> SwitchResult {
    let mut component = match registry.get_component::<MeshComponent>(id) {
        Ok(c) => c.clone(),
        Err(_) => return SwitchResult::NotAnimated,
    };

    let Some(anim) = component.animation.as_mut() else {
        return SwitchResult::NotAnimated;
    };

    if !anim.is_state_usable(target) {
        return SwitchResult::UnknownState;
    }

    if anim.current_state == target {
        return SwitchResult::AlreadyInState;
    }

    let current_pending = anim.entered_at.is_none();
    let current_usable = anim.is_state_usable(&anim.current_state);
    // An INTERRUPT is a switch that lands while a fade is already in flight: the
    // outgoing (current) state is resolved AND a previous-state fade was active
    // going into this switch. The entered (target) state's interrupt policy then
    // decides the new fade's source kind: `Smooth` → blend from a captured
    // snapshot of the in-flight blended pose (no discontinuity); `Snap` → blend
    // from the interrupted state's clip directly. A non-interrupt switch (no
    // active fade) always blends from the outgoing clip (`Clip`).
    let was_fading = anim.previous_state.is_some() && !current_pending && current_usable;
    let target_policy = anim
        .states
        .get(target)
        .map(|s| s.interrupt)
        .unwrap_or_default();
    let smooth_interrupt = was_fading && target_policy == InterruptPolicy::Smooth;

    // On a smooth interrupt, stash the interrupted fade's OUTGOING source BEFORE
    // it is overwritten below. The interrupted fade was OUT→IN where IN is the
    // current state (about to become `previous_state`) and OUT is the current
    // `previous_state` (the leg that would otherwise be dropped). Capturing OUT
    // here is what lets the collector reconstruct the in-flight blended pose
    // `blend(OUT, IN, w)` at the interrupt instant — without it OUT is
    // unrecoverable. If the interrupted fade was itself a snapshot fade, OUT is
    // that prior snapshot, referenced by its store tag (the interrupted fade's
    // entered stamp = the current state's `entered_at`, the tag it was stored
    // under). Otherwise OUT is the prior clip leg on its own timeline.
    let stash = if smooth_interrupt {
        match anim.fade_source {
            FadeSourceKind::Snapshot => anim
                .entered_at
                .map(|t| InterruptedOutgoing::Snapshot { tag: t.to_bits() }),
            FadeSourceKind::Clip => {
                match (anim.previous_state.clone(), anim.previous_entered_at) {
                    (Some(state), Some(entered_at)) => Some(InterruptedOutgoing::Clip {
                        state,
                        entered_at,
                        rate: anim.previous_rate,
                        rebase_time: anim.previous_rebase_time,
                        rebase_elapsed: anim.previous_rebase_elapsed,
                    }),
                    // A clip fade with no resolved previous stamp cannot be
                    // reproduced; degrade by stashing nothing (the capture then
                    // falls back to the interrupted incoming's clip).
                    _ => None,
                }
            }
        }
    } else {
        None
    };

    if current_pending || !current_usable {
        // No outgoing pose to preserve: collapse the never-rendered intermediate
        // (pending) or hard-cut out of an unresolved current state. The fade
        // source is left as last resolved; the resolve pass treats the absence
        // of `previous_state` as "no fade contribution".
        anim.previous_state = None;
        anim.previous_entered_at = None;
        anim.clear_previous_playback_time();
    } else {
        // Normal switch: the outgoing (current) state becomes the fade source,
        // keeping its own entry stamp so its clip advances on its own timeline.
        anim.snapshot_previous_playback_time();
        anim.previous_state = Some(std::mem::replace(
            &mut anim.current_state,
            target.to_string(),
        ));
        anim.previous_entered_at = anim.entered_at;
    }

    // Record the new fade's source kind. A smooth interrupt records `Snapshot`
    // so the collector emits a one-time snapshot capture of the in-flight blend;
    // every other case (non-interrupt switch, or a `Snap` interrupt) blends from
    // the outgoing clip directly. The decision MUST be made here at switch time:
    // by the time the collector runs (after the resolve pass) the in-flight pose
    // has not been captured yet, and switch time is the only moment that jointly
    // sees the active-fade status (`was_fading`) and the entered state's
    // interrupt policy. The resolve pass clears `previous_state` once a fade
    // completes, so `was_fading` here genuinely means an in-flight fade.
    anim.fade_source = if smooth_interrupt {
        FadeSourceKind::Snapshot
    } else {
        FadeSourceKind::Clip
    };
    // The stash is live only for a smooth interrupt; any other switch clears it
    // (no in-flight blend to preserve).
    anim.interrupted_outgoing = stash;

    anim.current_state = target.to_string();
    // Pending: the resolve pass fills this from the frame's post-advance clock.
    anim.entered_at = None;
    anim.reset_incoming_playback_time();

    // Write the mutated component back. The id was just read successfully, so a
    // write failure would be a logic error, not a recoverable script condition.
    let _: Result<(), RegistryError> = registry.set_component(id, component);
    SwitchResult::Switched
}

/// Outcome of a restart attempt. Mirrors the [`SwitchResult`] shape so callers
/// can distinguish a real restart from the no-op reasons (mostly for tests; the
/// AI tick ignores the variant).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartResult {
    /// The clip was re-stamped from frame 0 (entry stamp set pending, any fade
    /// bookkeeping cleared).
    Restarted,
    /// The entity is not in `target`, so there is no in-state clip to restart.
    /// Use [`switch_animation_state`] to enter the state first.
    NotInState,
    /// The entity carries no animation block, or `target` is unusable
    /// (undeclared / unresolved clip).
    NotAnimated,
}

/// Restart the entity's CURRENT animation clip from frame 0, but only when it is
/// already in `target`. This is the in-state replay seam for a one-shot clip that
/// must re-fire on a repeated action (e.g. an enemy swinging again while it stays
/// in `Attack`): `switch_animation_state` reports `AlreadyInState` and changes
/// nothing, so a fresh playthrough needs an explicit re-stamp.
///
/// A same-state restart has NO distinct outgoing pose — the clip being restarted
/// IS the current pose — so this is a hard cut, never a self-crossfade. It
/// mirrors `switch_animation_state`'s pending-stamp / no-outgoing-pose handling:
/// `entered_at` is set pending (`None`) so the resolve pass refills it from the
/// frame's post-advance clock (clip-local time `anim_time - entered_at` then
/// restarts at 0), and every fade bookkeeping field is cleared so no stale
/// `previous_state` blends a ghost of the prior playthrough.
///
/// No-ops (returns without writing) when the entity is not animated, `target` is
/// unusable, or the entity is not currently in `target` — restarting a clip the
/// entity is not playing is meaningless; the caller enters the state via
/// `switch_animation_state` first.
pub fn restart_animation_clip(
    registry: &mut EntityRegistry,
    id: EntityId,
    target: &str,
) -> RestartResult {
    let mut component = match registry.get_component::<MeshComponent>(id) {
        Ok(c) => c.clone(),
        Err(_) => return RestartResult::NotAnimated,
    };

    let Some(anim) = component.animation.as_mut() else {
        return RestartResult::NotAnimated;
    };

    if !anim.is_state_usable(target) {
        return RestartResult::NotAnimated;
    }

    if anim.current_state != target {
        return RestartResult::NotInState;
    }

    // Hard cut to frame 0: re-stamp the entry pending and drop every fade field.
    // No `previous_state`/snapshot — a same-state restart has no distinct
    // outgoing pose to crossfade from (mirrors the same-tick-collapse / hard-cut
    // path in `switch_animation_state`).
    anim.entered_at = None;
    anim.previous_state = None;
    anim.previous_entered_at = None;
    anim.clear_previous_playback_time();
    anim.reset_incoming_playback_time();
    anim.fade_source = FadeSourceKind::Clip;
    anim.interrupted_outgoing = None;

    // The id was just read successfully, so a write failure would be a logic
    // error, not a recoverable script condition.
    let _: Result<(), RegistryError> = registry.set_component(id, component);
    RestartResult::Restarted
}

#[cfg(test)]
mod tests {
    use super::{RestartResult, SwitchResult, restart_animation_clip, switch_animation_state};
    use crate::components::animation::test_support::{spawn_animated, usable_state};
    use crate::components::animation::{
        AnimationState, DEFAULT_CROSSFADE_MS, FadeSourceKind, InterruptPolicy, InterruptedOutgoing,
        MeshAnimation, resolve_pending_animation_stamps,
    };
    use crate::components::mesh::MeshComponent;
    use crate::registry::{EntityRegistry, Transform};
    use glam::Vec3;
    use std::collections::HashMap;

    #[test]
    fn switch_snapshots_outgoing_rebased_timeline_and_resets_incoming() {
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        resolve_pending_animation_stamps(&mut reg, 0.0);
        {
            let mut mesh = reg.get_component::<MeshComponent>(id).unwrap().clone();
            let anim = mesh.animation.as_mut().unwrap();
            anim.update_playback_rate(0.5, 2.0);
            reg.set_component(id, mesh).unwrap();
        }

        assert_eq!(
            switch_animation_state(&mut reg, id, "attack"),
            SwitchResult::Switched
        );
        let anim = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert!((anim.previous_rate - 0.5).abs() < f32::EPSILON);
        assert_eq!(anim.previous_rebase_time, Some(2.0));
        assert!((anim.previous_scaled_elapsed(2.0) - 2.0).abs() < 1.0e-9);
        assert_eq!(anim.rate, 1.0);
        assert_eq!(anim.rebase_time, None);
        assert_eq!(anim.rebase_elapsed, 0.0);
    }

    #[test]
    fn non_interrupt_switch_records_clip_fade_source() {
        // A switch with NO fade in flight records `Clip` (blend from the outgoing
        // clip) regardless of the entered state's interrupt policy.
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        resolve_pending_animation_stamps(&mut reg, 1.0);
        switch_animation_state(&mut reg, id, "attack");
        let anim = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(
            anim.fade_source,
            FadeSourceKind::Clip,
            "no active fade → clip source"
        );
    }

    #[test]
    fn smooth_interrupt_during_active_fade_records_snapshot_source() {
        // idle→attack starts a fade (attack is the new fade). Interrupting that
        // fade with attack→idle (idle defaults to smooth) records a Snapshot
        // source so the collector captures the in-flight blend.
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        resolve_pending_animation_stamps(&mut reg, 1.0);
        switch_animation_state(&mut reg, id, "attack"); // idle→attack (fade active)
        resolve_pending_animation_stamps(&mut reg, 2.0);
        {
            let mut mesh = reg.get_component::<MeshComponent>(id).unwrap().clone();
            mesh.animation
                .as_mut()
                .unwrap()
                .update_playback_rate(0.5, 2.1);
            reg.set_component(id, mesh).unwrap();
        }
        switch_animation_state(&mut reg, id, "idle"); // interrupt: smooth (default)
        let anim = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(
            anim.fade_source,
            FadeSourceKind::Snapshot,
            "a smooth interrupt during an active fade records a snapshot source",
        );
        assert!((anim.previous_rate - 0.5).abs() < f32::EPSILON);
        assert!(matches!(
            anim.interrupted_outgoing,
            Some(InterruptedOutgoing::Clip {
                ref state,
                rate,
                rebase_time: Some(1.0),
                rebase_elapsed: 0.0,
                ..
            }) if state == "idle" && (rate - 1.0).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn snap_interrupt_during_active_fade_records_clip_source() {
        // Same interrupt scenario but the entered state declares `Snap`: the new
        // fade blends from the interrupted state's clip directly (Clip source).
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        let mut states = HashMap::new();
        states.insert("idle".into(), usable_state("idle_clip", true, 0));
        states.insert("attack".into(), usable_state("attack_clip", false, 1));
        // `dash` is the entered state with an explicit Snap policy.
        states.insert(
            "dash".into(),
            AnimationState {
                clip: "dash_clip".into(),
                looping: false,
                crossfade_ms: DEFAULT_CROSSFADE_MS,
                interrupt: InterruptPolicy::Snap,
                travel_speed: None,
                clip_index: Some(2),
            },
        );
        reg.set_component(
            id,
            MeshComponent {
                model: "m".into(),
                animation: Some(MeshAnimation::new(states, "idle".into())),
                origin_offset: Vec3::ZERO,
                shadow_bias_scale: 1.0,
                shadow_only: false,
                attachments: Vec::new(),
                pose_inputs: None,
            },
        )
        .unwrap();
        resolve_pending_animation_stamps(&mut reg, 1.0);
        switch_animation_state(&mut reg, id, "attack"); // fade active
        resolve_pending_animation_stamps(&mut reg, 2.0);
        switch_animation_state(&mut reg, id, "dash"); // snap interrupt
        let anim = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(
            anim.fade_source,
            FadeSourceKind::Clip,
            "a snap interrupt blends from the interrupted clip (Clip source)",
        );
    }

    #[test]
    fn switch_records_target_pending_stamp_and_previous_state() {
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        // Resolve the spawn stamp so the current state is non-pending.
        resolve_pending_animation_stamps(&mut reg, 1.0);

        let result = switch_animation_state(&mut reg, id, "attack");
        assert_eq!(result, SwitchResult::Switched);

        let anim = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert_eq!(anim.current_state, "attack");
        assert_eq!(anim.previous_state.as_deref(), Some("idle"));
        assert_eq!(anim.previous_entered_at, Some(1.0));
        assert!(anim.entered_at.is_none(), "new entry stamp must be pending");
    }

    #[test]
    fn second_switch_same_tick_collapses_never_rendered_intermediate() {
        // Two switches before any resolve pass: the first leaves current pending,
        // the second must collapse the never-rendered intermediate and keep the
        // last-resolved previous state (idle). Last target wins.
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        resolve_pending_animation_stamps(&mut reg, 1.0);

        // First switch out of idle → attack (idle becomes outgoing).
        switch_animation_state(&mut reg, id, "attack");
        // Second switch this same tick: attack's stamp is still pending.
        switch_animation_state(&mut reg, id, "idle");

        let anim = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert_eq!(anim.current_state, "idle", "last same-tick target wins");
        // The never-rendered `attack` intermediate collapses out: no outgoing
        // fade contribution from it.
        assert_eq!(
            anim.previous_state, None,
            "pending intermediate must not become the fade source"
        );
        assert!(anim.entered_at.is_none());
    }

    #[test]
    fn switch_out_of_unresolved_current_state_hard_cuts() {
        // Current state unresolved (clip_index None) → switching out is a hard
        // cut: no previous_state recorded (no outgoing pose to preserve).
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        let mut states = HashMap::new();
        // `idle` is unresolved (no clip index); `attack` is usable.
        states.insert(
            "idle".into(),
            AnimationState {
                clip: "idle_clip".into(),
                looping: true,
                crossfade_ms: DEFAULT_CROSSFADE_MS,
                interrupt: InterruptPolicy::Smooth,
                travel_speed: None,
                clip_index: None,
            },
        );
        states.insert("attack".into(), usable_state("attack_clip", false, 1));
        reg.set_component(
            id,
            MeshComponent {
                model: "m".into(),
                animation: Some(MeshAnimation::new(states, "idle".into())),
                origin_offset: Vec3::ZERO,
                shadow_bias_scale: 1.0,
                shadow_only: false,
                attachments: Vec::new(),
                pose_inputs: None,
            },
        )
        .unwrap();
        resolve_pending_animation_stamps(&mut reg, 2.0);

        let result = switch_animation_state(&mut reg, id, "attack");
        assert_eq!(result, SwitchResult::Switched);
        let anim = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert_eq!(anim.current_state, "attack");
        assert_eq!(
            anim.previous_state, None,
            "hard cut out of unresolved state records no fade source"
        );
    }

    #[test]
    fn switch_to_unknown_state_does_not_change_state() {
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        resolve_pending_animation_stamps(&mut reg, 1.0);

        let result = switch_animation_state(&mut reg, id, "nonexistent");
        assert_eq!(result, SwitchResult::UnknownState);
        let anim = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert_eq!(anim.current_state, "idle", "state must be unchanged");
        assert_eq!(anim.previous_state, None);
    }

    #[test]
    fn switch_to_unresolved_state_is_unknown_noop() {
        // A declared-but-unresolved target is unusable: warn + no-op (UnknownState).
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        let mut states = HashMap::new();
        states.insert("idle".into(), usable_state("idle_clip", true, 0));
        states.insert(
            "death".into(),
            AnimationState {
                clip: "missing".into(),
                looping: false,
                crossfade_ms: 0.0,
                interrupt: InterruptPolicy::Smooth,
                travel_speed: None,
                clip_index: None,
            },
        );
        reg.set_component(
            id,
            MeshComponent {
                model: "m".into(),
                animation: Some(MeshAnimation::new(states, "idle".into())),
                origin_offset: Vec3::ZERO,
                shadow_bias_scale: 1.0,
                shadow_only: false,
                attachments: Vec::new(),
                pose_inputs: None,
            },
        )
        .unwrap();
        resolve_pending_animation_stamps(&mut reg, 1.0);

        assert_eq!(
            switch_animation_state(&mut reg, id, "death"),
            SwitchResult::UnknownState
        );
        assert_eq!(
            reg.get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .current_state,
            "idle"
        );
    }

    #[test]
    fn switch_on_stateless_entity_reports_not_animated() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        reg.set_component(id, MeshComponent::stateless("prop".into()))
            .unwrap();
        assert_eq!(
            switch_animation_state(&mut reg, id, "idle"),
            SwitchResult::NotAnimated
        );
    }

    #[test]
    fn switch_on_non_mesh_entity_reports_not_animated() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        assert_eq!(
            switch_animation_state(&mut reg, id, "idle"),
            SwitchResult::NotAnimated
        );
    }

    #[test]
    fn switch_to_current_state_is_already_in_state_noop() {
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        resolve_pending_animation_stamps(&mut reg, 1.0);
        assert_eq!(
            switch_animation_state(&mut reg, id, "idle"),
            SwitchResult::AlreadyInState
        );
        let anim = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert_eq!(anim.previous_state, None);
        // The current entry stamp is untouched (not re-stamped pending).
        assert_eq!(anim.entered_at, Some(1.0));
    }

    #[test]
    fn restart_clip_in_state_resets_entry_stamp_pending() {
        // The entity is in `attack` with a resolved stamp; restarting it re-stamps
        // the entry pending (frame 0) without changing the current state.
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        resolve_pending_animation_stamps(&mut reg, 1.0);
        switch_animation_state(&mut reg, id, "attack");
        resolve_pending_animation_stamps(&mut reg, 2.0);
        // Now in `attack`, stamp resolved at 2.0, fade window (0.15s) elapsed by
        // the time we restart so steady state.
        resolve_pending_animation_stamps(&mut reg, 2.5);
        let before = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(before.current_state, "attack");
        assert_eq!(before.entered_at, Some(2.0));

        assert_eq!(
            restart_animation_clip(&mut reg, id, "attack"),
            RestartResult::Restarted
        );
        let after = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(after.current_state, "attack", "state is unchanged");
        assert!(
            after.entered_at.is_none(),
            "restart re-stamps the entry pending (frame 0)"
        );
        assert_eq!(
            after.previous_state, None,
            "a same-state restart records no fade (hard cut)"
        );
        assert_eq!(after.previous_entered_at, None);
        assert_eq!(after.fade_source, FadeSourceKind::Clip);
    }

    #[test]
    fn restart_clip_clears_in_flight_fade_no_self_crossfade() {
        // Restarting mid-fade (idle→attack still crossfading) must hard-cut: clear
        // the `previous_state`/fade bookkeeping so no ghost of the prior pose blends
        // into the restarted clip.
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        resolve_pending_animation_stamps(&mut reg, 1.0);
        switch_animation_state(&mut reg, id, "attack"); // idle→attack fade
        resolve_pending_animation_stamps(&mut reg, 1.0);
        // Mid-window: fade is still in flight (previous_state == idle).
        let during = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(during.previous_state.as_deref(), Some("idle"));

        assert_eq!(
            restart_animation_clip(&mut reg, id, "attack"),
            RestartResult::Restarted
        );
        let after = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(
            after.previous_state, None,
            "restart hard-cuts: the in-flight fade is dropped (no self-crossfade)"
        );
        assert_eq!(after.previous_entered_at, None);
        assert_eq!(after.interrupted_outgoing, None);
        assert!(after.entered_at.is_none());
    }

    #[test]
    fn restart_clip_not_in_target_state_is_noop() {
        // Restarting a state the entity is NOT currently in is a no-op: the caller
        // must enter the state via `switch_animation_state` first.
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        resolve_pending_animation_stamps(&mut reg, 1.0);
        // Currently in `idle`; ask to restart `attack`.
        assert_eq!(
            restart_animation_clip(&mut reg, id, "attack"),
            RestartResult::NotInState
        );
        let anim = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert_eq!(anim.current_state, "idle", "no state change");
        assert_eq!(anim.entered_at, Some(1.0), "entry stamp untouched");
    }

    #[test]
    fn restart_clip_on_stateless_entity_reports_not_animated() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        reg.set_component(id, MeshComponent::stateless("prop".into()))
            .unwrap();
        assert_eq!(
            restart_animation_clip(&mut reg, id, "idle"),
            RestartResult::NotAnimated
        );
    }

    #[test]
    fn restart_clip_unusable_target_reports_not_animated() {
        // An unresolved (clip_index None) current state is unusable: restart is a
        // no-op NotAnimated, never a NaN-producing re-stamp of a dead clip.
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        let mut states = HashMap::new();
        states.insert(
            "idle".into(),
            AnimationState {
                clip: "idle_clip".into(),
                looping: true,
                crossfade_ms: DEFAULT_CROSSFADE_MS,
                interrupt: InterruptPolicy::Smooth,
                travel_speed: None,
                clip_index: None,
            },
        );
        reg.set_component(
            id,
            MeshComponent {
                model: "m".into(),
                animation: Some(MeshAnimation::new(states, "idle".into())),
                origin_offset: Vec3::ZERO,
                shadow_bias_scale: 1.0,
                shadow_only: false,
                attachments: Vec::new(),
                pose_inputs: None,
            },
        )
        .unwrap();
        assert_eq!(
            restart_animation_clip(&mut reg, id, "idle"),
            RestartResult::NotAnimated
        );
    }
}
