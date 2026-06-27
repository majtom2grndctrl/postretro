// Mesh component: the model handle a skinned-model entity renders, plus the
// optional declared animation-state surface and per-entity runtime state.
// See: context/lib/scripting.md §10.3 (Mesh Animation)

use std::collections::HashMap;

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::scripting::registry::{EntityId, EntityRegistry, RegistryError};

/// Default crossfade duration (milliseconds) for a state entry that does not
/// declare `crossfadeMs`. Cosmetic; a device-tuned default, not a contract.
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
    /// Clip index this state resolves to, filled at level load by
    /// `resolve_mesh_entity_clips` against the model's clip metadata. `None` =
    /// unresolved / unusable: switching *to* this state is a warn + no-op, and
    /// switching *out of* it is a hard cut (no outgoing pose to preserve).
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

/// The outgoing source of a fade that a `"smooth"` interrupt took over, stashed
/// across the switch so the capture can reconstruct the in-flight blended pose.
///
/// When a switch interrupts an active OUT→IN fade, IN becomes the new
/// `previous_state` (the interrupted incoming) but OUT — the leg the interrupted
/// fade was blending *out of* — would otherwise be dropped (`previous_state` is
/// overwritten). This stash preserves OUT so the collector can sample the exact
/// pose the entity showed at the interrupt instant: `blend(OUT, IN, w)`.
///
/// Runtime-only: set at switch time and cleared once the new fade resolves. Never
/// persisted (mirrors how `entered_at`/`fade_source` carry no durable meaning).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum InterruptedOutgoing {
    /// The interrupted fade blended out of a clip: its state name and the entry
    /// stamp that clip advanced on. Sampled at the interrupt instant on its own
    /// timeline to reproduce the leg.
    Clip { state: String, entered_at: f64 },
    /// The interrupted fade was itself a `"smooth"` snapshot fade: the prior
    /// snapshot, referenced by its store tag (the renderer's `SnapshotTag` — an
    /// `entered_at` bit pattern; kept as a plain `u64` here so this component
    /// stays free of any renderer dependency). The capture blends against that
    /// stored pose (a store hit), or degrades to the carried incoming fallback if
    /// its capture frame was culled.
    Snapshot { tag: u64 },
}

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
    /// validation of the descriptor's `animations` block). `"defaultState"` on
    /// the wire (boundary inventory).
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
    /// The outgoing source of the fade a `"smooth"` interrupt took over, stashed
    /// so the capture can reconstruct the in-flight blended pose at the interrupt
    /// instant. `Some` only between a smooth interrupt and the new fade's
    /// resolution; cleared on a non-interrupt switch, a hard cut/collapse, and
    /// when the fade completes. Runtime-only — `#[serde(skip)]` like
    /// `clip_index`, since it carries no durable meaning across a reload.
    #[serde(skip, default)]
    pub(crate) interrupted_outgoing: Option<InterruptedOutgoing>,
}

impl MeshAnimation {
    /// Build the runtime animation state for a freshly spawned descriptor
    /// entity: current = default, entry stamp pending, no active fade. Called by
    /// the data-archetype spawn path (`data_archetype.rs`) when materializing a
    /// descriptor entity with an `animations` block.
    pub(crate) fn new(states: HashMap<String, AnimationState>, default_state: String) -> Self {
        Self {
            current_state: default_state.clone(),
            default_state,
            states,
            entered_at: None,
            previous_state: None,
            previous_entered_at: None,
            fade_source: FadeSourceKind::Clip,
            interrupted_outgoing: None,
        }
    }

    /// A state is usable for switching only when it is declared *and* its clip
    /// resolved at level load (`clip_index.is_some()`).
    fn is_state_usable(&self, state: &str) -> bool {
        self.states
            .get(state)
            .is_some_and(|s| s.clip_index.is_some())
    }

    /// The crossfade window (seconds) of the current state, treating a
    /// non-positive `crossfadeMs` (or an undeclared current state) as a hard cut
    /// (`0.0`). The window governs when an active fade reaches weight 1.0.
    fn current_crossfade_seconds(&self) -> f32 {
        self.states
            .get(&self.current_state)
            .map(|s| (s.crossfade_ms / 1000.0).max(0.0))
            .unwrap_or(0.0)
    }

    /// True if the active fade (recorded `previous_state`) has reached weight
    /// `>= 1.0` at `now` — i.e. the crossfade window measured from the current
    /// state's `entered_at` has fully elapsed. False while a stamp is still
    /// pending (`entered_at == None`): a fade that has not started cannot have
    /// completed. A hard-cut window (`crossfade <= 0`) completes immediately on
    /// the first resolved frame.
    fn fade_completed_at(&self, now: f64) -> bool {
        let Some(entered_at) = self.entered_at else {
            return false;
        };
        let crossfade = self.current_crossfade_seconds();
        if crossfade <= 0.0 {
            return true;
        }
        (now - entered_at) as f32 >= crossfade
    }

    /// Whether the per-frame resolve pass must act on this entity. Steady-state
    /// entities (entry stamp resolved, no recorded fade) are skipped — touching
    /// them would clone, no-op mutate, and rewrite the component every frame.
    /// Work is due for exactly three reasons:
    /// - a pending current entry stamp to fill (`entered_at == None`);
    /// - an active fade whose previous stamp is still pending (carry/fill it);
    /// - an active fade that has reached weight 1.0 and must be cleared.
    fn resolve_pass_has_work(&self, now: f64) -> bool {
        self.entered_at.is_none()
            || (self.previous_state.is_some()
                && (self.previous_entered_at.is_none() || self.fade_completed_at(now)))
    }

    /// Clear a completed fade back to steady state: no previous state, no
    /// previous stamp, fade source reset to its `Clip` default, and the
    /// interrupt-outgoing stash dropped. After this the collector samples the
    /// single new clip (the pose at weight 1.0).
    fn clear_completed_fade(&mut self) {
        self.previous_state = None;
        self.previous_entered_at = None;
        self.fade_source = FadeSourceKind::Clip;
        self.interrupted_outgoing = None;
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
///
/// `origin_offset` is a render-presentation offset applied after transform
/// interpolation. It is zero for authored world-origin props. Descriptor AI
/// meshes use it to render feet-at-origin art from capsule-center gameplay
/// transforms, including remote enemies that intentionally carry no local
/// `AgentComponent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct MeshComponent {
    pub(crate) model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) animation: Option<MeshAnimation>,
    #[serde(default, skip_serializing_if = "vec3_is_zero")]
    pub(crate) origin_offset: Vec3,
}

impl MeshComponent {
    /// Convenience for the stateless `prop_mesh` path: a model handle with no
    /// animation block.
    pub(crate) fn stateless(model: String) -> Self {
        Self {
            model,
            animation: None,
            origin_offset: Vec3::ZERO,
        }
    }

    pub(crate) fn animated(model: String, animation: MeshAnimation) -> Self {
        Self {
            model,
            animation: Some(animation),
            origin_offset: Vec3::ZERO,
        }
    }

    pub(crate) fn with_origin_offset(mut self, origin_offset: Vec3) -> Self {
        self.origin_offset = origin_offset;
        self
    }
}

fn vec3_is_zero(value: &Vec3) -> bool {
    *value == Vec3::ZERO
}

pub(crate) fn capsule_center_to_feet_origin_offset(radius: f32, height: f32) -> Vec3 {
    let half_height = (height / 2.0 - radius).max(0.0);
    Vec3::new(0.0, -(half_height + radius), 0.0)
}

/// Outcome of a switch attempt. The caller (the `setAnimationState` reaction)
/// logs the failure variants; this mirrors the `setEmitterRate`
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
                    (Some(state), Some(entered_at)) => {
                        Some(InterruptedOutgoing::Clip { state, entered_at })
                    }
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

    // Write the mutated component back. The id was just read successfully, so a
    // write failure would be a logic error, not a recoverable script condition.
    let _: Result<(), RegistryError> = registry.set_component(id, component);
    SwitchResult::Switched
}

/// Outcome of a restart attempt. Mirrors the [`SwitchResult`] shape so callers
/// can distinguish a real restart from the no-op reasons (mostly for tests; the
/// AI tick ignores the variant).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RestartResult {
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
pub(crate) fn restart_animation_clip(
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
    anim.fade_source = FadeSourceKind::Clip;
    anim.interrupted_outgoing = None;

    // The id was just read successfully, so a write failure would be a logic
    // error, not a recoverable script condition.
    let _: Result<(), RegistryError> = registry.set_component(id, component);
    RestartResult::Restarted
}

/// Resolve every mesh entity's pending entry stamps from the frame's
/// post-advance animation-clock value, and clear fades that have completed.
/// Runs in the render-frame collection sub-stage, immediately before the mesh
/// collector, with a mutable registry.
///
/// Three jobs, on exactly the entities that need them (steady-state entities are
/// skipped — see [`MeshAnimation::resolve_pass_has_work`] — so the hot path does
/// not clone and rewrite untouched components every frame):
/// - A pending `entered_at` (`None`) is filled with `now`.
/// - A pending `previous_entered_at` accompanying an active fade is filled too
///   (a switch out of a freshly-entered state where the previous stamp could not
///   be carried).
/// - A fade that has reached weight 1.0 (window measured from the current
///   state's `crossfadeMs`) is cleared back to steady state, so the next
///   `switch_animation_state` does not mistake a finished fade for an in-flight
///   one and record a spurious snapshot capture. At weight 1.0 the collector
///   already shows only the new clip, so clearing is pose-equivalent.
///
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
                .filter(|a| a.resolve_pass_has_work(now))
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
        // Clear a fade that has reached weight 1.0. Re-checked after filling the
        // current stamp above, so a fade entered with a pending stamp is
        // evaluated against the stamp it was just assigned (a hard-cut window
        // clears on this first resolved frame).
        if anim.previous_state.is_some() && anim.fade_completed_at(now) {
            anim.clear_completed_fade();
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
                origin_offset: Vec3::ZERO,
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
        assert!(as_value.get("origin_offset").is_none());
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
            origin_offset: Vec3::ZERO,
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
                origin_offset: Vec3::ZERO,
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
                origin_offset: Vec3::ZERO,
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
                origin_offset: Vec3::ZERO,
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
    fn resolve_pass_skips_steady_state_entity() {
        // A steady-state animated entity (entry stamp resolved, no recorded
        // fade) must NOT be picked up by the resolve pass: the predicate reports
        // no work, and a second resolve at a later clock leaves the component
        // byte-identical (no needless clone/no-op-mutate/rewrite each frame).
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        resolve_pending_animation_stamps(&mut reg, 1.0);

        let before = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(before.entered_at, Some(1.0));
        assert_eq!(before.previous_state, None);
        // The pass predicate reports no work for a steady-state entity.
        assert!(
            !before.resolve_pass_has_work(2.0),
            "steady-state entity must report no resolve-pass work"
        );

        // Running the pass again at a later clock must not alter the component.
        resolve_pending_animation_stamps(&mut reg, 2.0);
        let after = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(before, after, "steady-state component must be untouched");
    }

    #[test]
    fn resolve_pass_clears_fade_after_crossfade_window() {
        // idle→attack records a fade (attack crossfade = DEFAULT_CROSSFADE_MS =
        // 150ms = 0.15s). Resolve at the switch instant retains the fade; once
        // the clock passes the window, the next resolve clears it back to steady
        // state and resets the fade source to Clip.
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        resolve_pending_animation_stamps(&mut reg, 1.0);
        switch_animation_state(&mut reg, id, "attack"); // records fade (idle→attack)

        // First resolve fills the new entry stamp at 1.0; fade still in flight.
        resolve_pending_animation_stamps(&mut reg, 1.0);
        let during = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(during.entered_at, Some(1.0));
        assert_eq!(
            during.previous_state.as_deref(),
            Some("idle"),
            "fade retained during the window (weight < 1.0)"
        );

        // Advance past the 0.15s window and resolve again → fade cleared.
        resolve_pending_animation_stamps(&mut reg, 1.0 + 0.2);
        let after = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(
            after.previous_state, None,
            "completed fade is cleared once the window elapses"
        );
        assert_eq!(after.previous_entered_at, None);
        assert_eq!(after.fade_source, FadeSourceKind::Clip);

        // A subsequent switch must see no in-flight fade: it records `Clip`, not
        // a spurious `Snapshot` capture.
        switch_animation_state(&mut reg, id, "idle");
        let next = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(
            next.fade_source,
            FadeSourceKind::Clip,
            "no spurious snapshot capture after a completed fade",
        );
    }

    #[test]
    fn resolve_pass_retains_fade_within_crossfade_window() {
        // During the crossfade window (weight < 1.0) the resolve pass must NOT
        // clear the fade: previous_state stays Some so the collector keeps
        // blending the outgoing pose.
        let mut reg = EntityRegistry::new();
        let id = spawn_animated(&mut reg);
        resolve_pending_animation_stamps(&mut reg, 1.0);
        switch_animation_state(&mut reg, id, "attack");
        resolve_pending_animation_stamps(&mut reg, 1.0);

        // Halfway through the 0.15s window: fade must still be present.
        resolve_pending_animation_stamps(&mut reg, 1.0 + 0.075);
        let anim = reg
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert_eq!(
            anim.previous_state.as_deref(),
            Some("idle"),
            "fade retained mid-window (weight < 1.0)"
        );
        assert_eq!(anim.previous_entered_at, Some(1.0));
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
                clip_index: None,
            },
        );
        reg.set_component(
            id,
            MeshComponent {
                model: "m".into(),
                animation: Some(MeshAnimation::new(states, "idle".into())),
                origin_offset: Vec3::ZERO,
            },
        )
        .unwrap();
        assert_eq!(
            restart_animation_clip(&mut reg, id, "idle"),
            RestartResult::NotAnimated
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
