// Animation surface for skinned mesh entities: the `MeshAnimation` runtime
// state plus the module barrel re-exporting the state model, playback-rate
// machinery, transition verbs, and the resolve pass.
// See: context/lib/scripting.md §10.3 (Mesh Animation)

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

mod playback;
mod resolve;
mod state;
mod transitions;

pub use playback::{RATE_CHANGE_EPSILON, RATE_MAX, RATE_MIN};
pub use resolve::resolve_pending_animation_stamps;
pub use state::{
    AnimStamp, AnimationState, DEFAULT_CROSSFADE_MS, FadeSourceKind, InterruptPolicy,
    InterruptedOutgoing,
};
pub use transitions::{
    RestartResult, SwitchResult, restart_animation_clip, switch_animation_state,
};

/// The authored playback rate a state samples at before any locomotion scaling.
/// Also the serde-recovery default for the runtime-only rate fields.
pub(super) fn default_playback_rate() -> f32 {
    1.0
}

/// Default for [`MeshAnimation::speed_scale`]: locomotion rate-scaling is on
/// unless the archetype's `locomotion.speedScale` toggle explicitly disables it.
/// Absent `locomotion` block ⇒ `true`, preserving today's speed-scaled playback.
pub(super) fn default_speed_scale() -> bool {
    true
}

/// serde skip-if-absent predicate for [`MeshAnimation::speed_scale`]: the field
/// is omitted on serialize when it holds its default (`true`), so an archetype
/// that did not turn rate-scaling off round-trips without the key.
fn speed_scale_is_default(value: &bool) -> bool {
    *value == default_speed_scale()
}

/// Per-entity animation runtime state, present only on descriptor-spawned
/// entities that declared an `animations` block. `prop_mesh` entities leave
/// [`MeshComponent::animation`] as `None` and hold the model's authored rest
/// pose.
///
/// [`MeshComponent::animation`]: super::mesh::MeshComponent::animation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshAnimation {
    /// Declared state map: state name → clip + loop + crossfade + interrupt.
    /// Copied in at spawn; never mutated at runtime except for `clip_index`
    /// resolution at level load.
    pub states: HashMap<String, AnimationState>,
    /// The state entered at spawn. Always names a declared state (parse-time
    /// validation of the descriptor's `animations` block). `"defaultState"` on
    /// the wire (boundary inventory).
    #[serde(rename = "defaultState")]
    pub default_state: String,
    /// The currently-active state name. Seeded to `default_state` at spawn.
    pub current_state: String,
    /// Clock timestamp the current state was entered at. `None` until the
    /// resolve pass fills it (pending).
    pub entered_at: AnimStamp,
    /// The state being faded *out of*, if a fade is active. Its outgoing clip
    /// keeps playing on its own timeline during the fade.
    pub previous_state: Option<String>,
    /// Clock timestamp the previous state was entered at — its own stamp, so the
    /// outgoing clip advances on its own timeline. `None` if no fade is active
    /// or the stamp is still pending.
    pub previous_entered_at: AnimStamp,
    /// Runtime-only playback rate for the current state. It is deliberately
    /// skipped by component serde: direct deserialization must restore the
    /// authored-rate default instead of the `f32` zero default.
    #[serde(skip, default = "default_playback_rate")]
    pub rate: f32,
    /// Animation-clock origin for the current state's rebased timeline. `None`
    /// with no entry stamp is pending; otherwise serde recovery uses `entered_at`.
    #[serde(skip, default)]
    pub rebase_time: AnimStamp,
    /// Accumulated current-state elapsed time at `rebase_time`.
    #[serde(skip, default)]
    pub rebase_elapsed: f64,
    /// Runtime-only playback rate snapshot for the outgoing fade leg.
    #[serde(skip, default = "default_playback_rate")]
    pub previous_rate: f32,
    /// Rebased timeline origin for the outgoing fade leg.
    #[serde(skip, default)]
    pub previous_rebase_time: AnimStamp,
    /// Accumulated outgoing-leg elapsed time at `previous_rebase_time`.
    #[serde(skip, default)]
    pub previous_rebase_elapsed: f64,
    /// Whether locomotion rate-scaling applies to this entity's playback.
    /// Threaded from the archetype's `locomotion.speedScale` toggle at spawn
    /// (`data_archetype.rs`); `true` (default, absent block) keeps speed-scaled
    /// playback, `false` plays the authored cadence unscaled. `speedScale` on
    /// the wire; `#[serde(default)]` and skip-if-absent so the default
    /// round-trips without the key.
    #[serde(
        rename = "speedScale",
        default = "default_speed_scale",
        skip_serializing_if = "speed_scale_is_default"
    )]
    pub speed_scale: bool,
    /// What the active fade blends from (interrupted-state clip vs snapshot).
    /// Set by [`switch_animation_state`] per the entered state's interrupt policy.
    pub fade_source: FadeSourceKind,
    /// The outgoing source of the fade a `"smooth"` interrupt took over, stashed
    /// so the capture can reconstruct the in-flight blended pose at the interrupt
    /// instant. `Some` only between a smooth interrupt and the new fade's
    /// resolution; cleared on a non-interrupt switch, a hard cut/collapse, and
    /// when the fade completes. Runtime-only — `#[serde(skip)]` like
    /// `clip_index`, since it carries no durable meaning across a reload.
    #[serde(skip, default)]
    pub interrupted_outgoing: Option<InterruptedOutgoing>,
}

impl MeshAnimation {
    /// Build the runtime animation state for a freshly spawned descriptor
    /// entity: current = default, entry stamp pending, no active fade. Called by
    /// the data-archetype spawn path (`data_archetype.rs`) when materializing a
    /// descriptor entity with an `animations` block.
    pub fn new(states: HashMap<String, AnimationState>, default_state: String) -> Self {
        Self {
            current_state: default_state.clone(),
            default_state,
            states,
            entered_at: None,
            previous_state: None,
            previous_entered_at: None,
            rate: default_playback_rate(),
            rebase_time: None,
            rebase_elapsed: 0.0,
            previous_rate: default_playback_rate(),
            previous_rebase_time: None,
            previous_rebase_elapsed: 0.0,
            speed_scale: default_speed_scale(),
            fade_source: FadeSourceKind::Clip,
            interrupted_outgoing: None,
        }
    }

    /// Override the locomotion `speed_scale` toggle, threading the archetype's
    /// `locomotion.speedScale` value through the spawn path at the
    /// [`MeshAnimation::new`] call site. Chainable so the descriptor spawn path
    /// keeps its single-expression construction.
    pub fn with_speed_scale(mut self, speed_scale: bool) -> Self {
        self.speed_scale = speed_scale;
        self
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Shared fixtures for the animation submodule test suites.
    use super::*;
    use crate::components::mesh::MeshComponent;
    use crate::registry::{EntityId, EntityRegistry, Transform};
    use glam::Vec3;

    pub(crate) fn usable_state(clip: &str, looping: bool, clip_index: usize) -> AnimationState {
        AnimationState {
            clip: clip.into(),
            looping,
            crossfade_ms: DEFAULT_CROSSFADE_MS,
            interrupt: InterruptPolicy::Smooth,
            travel_speed: None,
            clip_index: Some(clip_index),
        }
    }

    pub(crate) fn two_state_animation() -> MeshAnimation {
        let mut states = HashMap::new();
        states.insert("idle".into(), usable_state("idle_clip", true, 0));
        states.insert("attack".into(), usable_state("attack_clip", false, 1));
        MeshAnimation::new(states, "idle".into())
    }

    pub(crate) fn spawn_animated(reg: &mut EntityRegistry) -> EntityId {
        let id = reg.spawn(Transform::default());
        reg.set_component(
            id,
            MeshComponent {
                model: "decraniated".into(),
                animation: Some(two_state_animation()),
                origin_offset: Vec3::ZERO,
                shadow_bias_scale: 1.0,
                shadow_only: false,
                attachments: Vec::new(),
                pose_inputs: None,
            },
        )
        .unwrap();
        id
    }
}
