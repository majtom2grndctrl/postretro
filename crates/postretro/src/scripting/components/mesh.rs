// Mesh component: the model handle a skinned-model entity renders, plus the
// optional declared animation-state surface and per-entity runtime state.
// See: context/lib/scripting.md §10.3 (Mesh Animation)

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::scripting::registry::{EntityId, EntityRegistry, RegistryError};

/// Default crossfade duration (milliseconds) for a state entry that does not
/// declare `crossfadeMs`. Cosmetic; tuned on device (plan Open questions).
pub(crate) const DEFAULT_CROSSFADE_MS: f32 = 150.0;

/// How a fade *into* a state takes over when another fade is already in flight.
/// Per-state entry; absent in the descriptor defaults to [`InterruptPolicy::Smooth`].
///
/// This type records the authored intent. The *source-kind* decision it drives
/// (`Smooth` → snapshot fade, `Snap` → outgoing clip) lands in
/// [`switch_animation_state`] when a switch interrupts an active fade; the
/// per-frame *capture inputs* (the in-flight blend the snapshot freezes) are
/// computed downstream by the render-frame collector
/// (`scripting/systems/mesh_anim.rs`), which the renderer's snapshot store
/// evaluates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InterruptPolicy {
    /// Capture the in-flight blended pose once as a static snapshot and blend
    /// the new fade from it — no discontinuity.
    #[default]
    Smooth,
    /// Blend the new fade from the interrupted state's clip; the in-flight blend
    /// drops — a deliberate, fade-window-bounded pop.
    Snap,
}

/// One declared animation state: a named clip plus loop and crossfade policy.
///
/// `looping` carries `#[serde(rename = "loop")]` because `loop` is a Rust
/// keyword; `crossfade_ms` is `"crossfadeMs"` on the wire. `interrupt` defaults
/// to [`InterruptPolicy::Smooth`] when absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AnimationState {
    pub(crate) clip: String,
    #[serde(rename = "loop")]
    pub(crate) looping: bool,
    #[serde(rename = "crossfadeMs")]
    pub(crate) crossfade_ms: f32,
    #[serde(default)]
    pub(crate) interrupt: InterruptPolicy,
    /// Clip index this state resolves to, filled by Task 5's level-load
    /// validation against the model's clip metadata. `None` = unresolved /
    /// unusable: switching *to* this state is a warn + no-op, and switching
    /// *out of* it is a hard cut (no outgoing pose to preserve).
    #[serde(skip, default)]
    pub(crate) clip_index: Option<usize>,
}

/// The source the active fade blends *from*. Set by [`switch_animation_state`]
/// when a switch lands: a smooth interrupt of an active fade records `Snapshot`
/// (the collector then captures the in-flight blend), every other switch records
/// `Clip` (blend from the outgoing clip). A never-rendered same-tick intermediate
/// collapses out before it can record a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FadeSourceKind {
    /// The outgoing (previous) state's clip keeps playing on its own timeline.
    #[default]
    Clip,
    /// A static per-joint snapshot captured for a `"smooth"` interrupt.
    Snapshot,
}

/// An animation clock timestamp. `None` is the "pending" sentinel: the switch
/// stamps a pending entry-time, and the resolve pass fills it from the frame's
/// post-advance clock value. A pending stamp reads as elapsed `0` / not
/// complete.
pub(crate) type AnimStamp = Option<f64>;

/// Per-entity animation runtime state, present only on descriptor-spawned
/// entities that declared an `animations` block. `prop_mesh` entities leave
/// [`MeshComponent::animation`] as `None` and stay stateless (today's behavior:
/// first clip, looped, phase offset).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct MeshAnimation {
    /// Declared state map: state name → clip + loop + crossfade + interrupt.
    /// Copied in at spawn; never mutated at runtime except for `clip_index`
    /// resolution at level load.
    pub(crate) states: HashMap<String, AnimationState>,
    /// The state entered at spawn. Always names a declared state (parse-time
    /// validation, Task 3). `"defaultState"` on the wire (boundary inventory).
    #[serde(rename = "defaultState")]
    pub(crate) default_state: String,
    /// The currently-active state name. Seeded to `default_state` at spawn.
    pub(crate) current_state: String,
    /// Clock timestamp the current state was entered at. `None` until the
    /// resolve pass fills it (pending).
    pub(crate) entered_at: AnimStamp,
    /// The state being faded *out of*, if a fade is active. Its outgoing clip
    /// keeps playing on its own timeline during the fade.
    pub(crate) previous_state: Option<String>,
    /// Clock timestamp the previous state was entered at — its own stamp, so the
    /// outgoing clip advances on its own timeline. `None` if no fade is active
    /// or the stamp is still pending.
    pub(crate) previous_entered_at: AnimStamp,
    /// What the active fade blends from (interrupted-state clip vs snapshot).
    /// Set by [`switch_animation_state`] per the entered state's interrupt policy.
    pub(crate) fade_source: FadeSourceKind,
}

impl MeshAnimation {
    /// Build the runtime animation state for a freshly spawned descriptor
    /// entity: current = default, entry stamp pending, no active fade. Task 3's
    /// descriptor-attach path calls this.
    pub(crate) fn new(states: HashMap<String, AnimationState>, default_state: String) -> Self {
        Self {
            current_state: default_state.clone(),
            default_state,
            states,
            entered_at: None,
            previous_state: None,
            previous_entered_at: None,
            fade_source: FadeSourceKind::Clip,
        }
    }

    /// A state is usable for switching only when it is declared *and* its clip
    /// resolved at level load (`clip_index.is_some()`).
    fn is_state_usable(&self, state: &str) -> bool {
        self.states
            .get(state)
            .is_some_and(|s| s.clip_index.is_some())
    }
}

/// Marks an entity as rendering a skinned model. `model` is the model handle
/// the `prop_mesh` classname handler reads from a map entity's `model` key — the
/// content-canonical path passed to `crate::model::gltf_loader::load_model`. It
/// doubles as the renderer cache key: the level-load model sweep uploads each
/// distinct handle once, and the per-frame draw planner groups instances by it.
///
/// `animation` is `None` for stateless `prop_mesh` entities and `Some` for
/// descriptor-spawned entities that declared an `animations` block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct MeshComponent {
    pub(crate) model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) animation: Option<MeshAnimation>,
}

impl MeshComponent {
    /// Convenience for the stateless `prop_mesh` path: a model handle with no
    /// animation block.
    pub(crate) fn stateless(model: String) -> Self {
        Self {
            model,
            animation: None,
        }
    }
}

/// Outcome of a switch attempt. The caller (the `setAnimationState` reaction,
/// Task 4) logs the failure variants; this mirrors the `setEmitterRate`
/// validated-setter precedent (validate here, let the caller surface warnings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SwitchResult {
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
/// `setAnimationState` reaction (Task 4), the future AI plan, and future
/// command-buffer guards call.
///
/// Records the target state, a pending entry stamp, the previous state, and the
/// new fade's SOURCE KIND (per the entered state's interrupt policy): a smooth
/// interrupt of an active fade records `Snapshot`, every other switch records
/// `Clip`. The per-frame capture inputs (the in-flight blend the snapshot
/// freezes) are computed downstream by the render-frame collector, after the
/// resolve pass fills the pending stamps — so the last same-tick target wins
/// trivially and the resolved stamps give clip-local times a concrete origin.
///
/// Pending-stamp collapse: if the current state's entry stamp is still pending
/// (a switch landed this same tick and the resolve pass has not run), the
/// never-rendered intermediate is dropped — it contributes no fade, and the
/// source records `Clip` (no in-flight pose to capture). A hard cut (no fade)
/// also applies when switching *out of* an unresolved/unusable current state:
/// there is no outgoing pose to preserve.
pub(crate) fn switch_animation_state(
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

    if current_pending || !current_usable {
        // No outgoing pose to preserve: collapse the never-rendered intermediate
        // (pending) or hard-cut out of an unresolved current state. The fade
        // source is left as last resolved; the resolve pass treats the absence
        // of `previous_state` as "no fade contribution".
        anim.previous_state = None;
        anim.previous_entered_at = None;
    } else {
        // Normal switch: the outgoing (current) state becomes the fade source,
        // keeping its own entry stamp so its clip advances on its own timeline.
        anim.previous_state = Some(std::mem::replace(
            &mut anim.current_state,
            target.to_string(),
        ));
        anim.previous_entered_at = anim.entered_at;
    }

    // Record the new fade's source kind. A smooth interrupt records `Snapshot`
    // so the collector emits a one-time snapshot capture of the in-flight blend;
    // every other case (non-interrupt switch, or a `Snap` interrupt) blends from
    // the outgoing clip directly. (Phase 1 left this as last-resolved; Task 5
    // applies the policy here at switch time — `was_fading` is exactly the
    // "switch during an active fade" condition the interrupt criterion names.)
    anim.fade_source = if was_fading && target_policy == InterruptPolicy::Smooth {
        FadeSourceKind::Snapshot
    } else {
        FadeSourceKind::Clip
    };

    anim.current_state = target.to_string();
    // Pending: the resolve pass fills this from the frame's post-advance clock.
    anim.entered_at = None;

    // Write the mutated component back. The id was just read successfully, so a
    // write failure would be a logic error, not a recoverable script condition.
    let _: Result<(), RegistryError> = registry.set_component(id, component);
    SwitchResult::Switched
}

/// Resolve every mesh entity's pending entry stamps from the frame's
/// post-advance animation-clock value. Runs in the render-frame collection
/// sub-stage, immediately before the mesh collector (Task 5), with a mutable
/// registry.
///
/// A pending `entered_at` (`None`) is filled with `now`; a pending
/// `previous_entered_at` accompanying an active fade is filled too (a switch out
/// of a freshly-entered state where the previous stamp could not be carried).
/// This seam fills the stamps so clip-local times and fade windows have a
/// concrete origin; the fade source-kind decision is made earlier (at switch
/// time, in [`switch_animation_state`]) and the per-frame capture inputs are
/// computed downstream by the render-frame collector.
pub(crate) fn resolve_pending_animation_stamps(registry: &mut EntityRegistry, now: f64) {
    use crate::scripting::registry::ComponentKind;

    // Collect ids first so we don't hold an immutable borrow across the mutable
    // writes. Mesh instance counts are small relative to a frame's work.
    let pending: Vec<EntityId> = registry
        .iter_with_kind(ComponentKind::Mesh)
        .filter_map(|(id, value)| match value {
            crate::scripting::registry::ComponentValue::Mesh(mesh) => mesh
                .animation
                .as_ref()
                .filter(|a| a.entered_at.is_none() || a.previous_entered_at.is_none())
                .map(|_| id),
            _ => None,
        })
        .collect();

    for id in pending {
        let Ok(mut component) = registry.get_component::<MeshComponent>(id).cloned() else {
            continue;
        };
        let Some(anim) = component.animation.as_mut() else {
            continue;
        };
        if anim.entered_at.is_none() {
            anim.entered_at = Some(now);
        }
        if anim.previous_state.is_some() && anim.previous_entered_at.is_none() {
            anim.previous_entered_at = Some(now);
        }
        let _ = registry.set_component(id, component);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::registry::{ComponentValue, Transform};

    fn usable_state(clip: &str, looping: bool, clip_index: usize) -> AnimationState {
        AnimationState {
            clip: clip.into(),
            looping,
            crossfade_ms: DEFAULT_CROSSFADE_MS,
            interrupt: InterruptPolicy::Smooth,
            clip_index: Some(clip_index),
        }
    }

    fn two_state_animation() -> MeshAnimation {
        let mut states = HashMap::new();
        states.insert("idle".into(), usable_state("idle_clip", true, 0));
        states.insert("attack".into(), usable_state("attack_clip", false, 1));
        MeshAnimation::new(states, "idle".into())
    }

    fn spawn_animated(reg: &mut EntityRegistry) -> EntityId {
        let id = reg.spawn(Transform::default());
        reg.set_component(
            id,
            MeshComponent {
                model: "decraniated".into(),
                animation: Some(two_state_animation()),
            },
        )
        .unwrap();
        id
    }

    #[test]
    fn mesh_component_serde_round_trip_stateless() {
        let value = MeshComponent::stateless("decraniated".into());
        let json = serde_json::to_string(&value).unwrap();
        let back: MeshComponent = serde_json::from_str(&json).unwrap();
        assert_eq!(value, back);
        // Stateless component omits the animation key entirely.
        let as_value = serde_json::to_value(&value).unwrap();
        assert!(as_value.get("animation").is_none());
    }

    #[test]
    fn mesh_serializes_within_component_value_tagged_form() {
        let value = ComponentValue::Mesh(MeshComponent::stateless("decraniated".into()));
        let json = serde_json::to_value(&value).unwrap();
        assert_eq!(json["kind"], "mesh");
        assert_eq!(json["model"], "decraniated");
    }

    #[test]
    fn animation_block_serde_round_trips_with_renames() {
        let value = MeshComponent {
            model: "decraniated".into(),
            animation: Some(two_state_animation()),
        };
        let json = serde_json::to_value(&value).unwrap();
        // Serde renames: `loop`, `crossfadeMs`, `defaultState`.
        let states = &json["animation"]["states"];
        assert!(states["idle"].get("loop").is_some(), "expected `loop` key");
        assert!(
            states["idle"].get("crossfadeMs").is_some(),
            "expected `crossfadeMs` key"
        );
        assert_eq!(json["animation"]["defaultState"], "idle");
        assert_eq!(json["animation"]["current_state"], "idle");

        // `clip_index` is `#[serde(skip)]` — runtime-resolved, never serialized.
        assert!(states["idle"].get("clip_index").is_none());

        // Round-trip back. `clip_index` deserializes to None (skip default), so
        // compare against the same shape with unresolved indices.
        let back: MeshComponent = serde_json::from_value(json).unwrap();
        let mut expected = value.clone();
        for s in expected.animation.as_mut().unwrap().states.values_mut() {
            s.clip_index = None;
        }
        assert_eq!(back, expected);
    }

    #[test]
    fn interrupt_policy_serde_uses_snake_case_keywords() {
        assert_eq!(
            serde_json::to_value(InterruptPolicy::Smooth).unwrap(),
            serde_json::json!("smooth")
        );
        assert_eq!(
            serde_json::to_value(InterruptPolicy::Snap).unwrap(),
            serde_json::json!("snap")
        );
        let absent: InterruptPolicy =
            serde_json::from_str(&serde_json::to_string(&InterruptPolicy::default()).unwrap())
                .unwrap();
        assert_eq!(absent, InterruptPolicy::Smooth);
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
                clip_index: Some(2),
            },
        );
        reg.set_component(
            id,
            MeshComponent {
                model: "m".into(),
                animation: Some(MeshAnimation::new(states, "idle".into())),
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
                clip_index: None,
            },
        );
        states.insert("attack".into(), usable_state("attack_clip", false, 1));
        reg.set_component(
            id,
            MeshComponent {
                model: "m".into(),
                animation: Some(MeshAnimation::new(states, "idle".into())),
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
                clip_index: None,
            },
        );
        reg.set_component(
            id,
            MeshComponent {
                model: "m".into(),
                animation: Some(MeshAnimation::new(states, "idle".into())),
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
    fn resolve_pass_fills_pending_spawn_stamp() {
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        // Spawn leaves entered_at pending.
        assert!(
            reg.get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .entered_at
                .is_none()
        );

        resolve_pending_animation_stamps(&mut reg, 4.25);
        assert_eq!(
            reg.get_component::<MeshComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .entered_at,
            Some(4.25)
        );
    }

    #[test]
    fn resolve_pass_fills_previous_stamp_for_active_fade() {
        // A switch out of a state whose own stamp was pending leaves
        // previous_entered_at None; but a normal switch carries the previous
        // stamp. Here we cover the carried case plus the new pending current.
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        resolve_pending_animation_stamps(&mut reg, 1.0);
        switch_animation_state(&mut reg, id, "attack");

        resolve_pending_animation_stamps(&mut reg, 3.0);
        let anim = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert_eq!(anim.entered_at, Some(3.0), "new current stamp filled");
        assert_eq!(
            anim.previous_entered_at,
            Some(1.0),
            "previous stamp carried from before the switch, not overwritten"
        );
    }
}
