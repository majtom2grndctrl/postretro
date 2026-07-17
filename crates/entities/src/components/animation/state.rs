// Animation state descriptor and fade-source data model: declared states,
// interrupt policy, entry-time stamps, and the interrupted-outgoing stash.
// See: context/lib/scripting.md §10.3 (Mesh Animation)

use serde::{Deserialize, Serialize};

/// Default crossfade duration (milliseconds) for a state entry that does not
/// declare `crossfadeMs`. Cosmetic; a device-tuned default, not a contract.
pub const DEFAULT_CROSSFADE_MS: f32 = 150.0;

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
///
/// [`switch_animation_state`]: super::switch_animation_state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptPolicy {
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
pub struct AnimationState {
    pub clip: String,
    #[serde(rename = "loop")]
    pub looping: bool,
    #[serde(rename = "crossfadeMs")]
    pub crossfade_ms: f32,
    #[serde(default)]
    pub interrupt: InterruptPolicy,
    /// Optional authored per-state travel-speed override, in ground units per
    /// animated second (`travelSpeed` on the wire). Finite and `> 0` when
    /// present — validated in [`crate::data_descriptors::MeshDescriptor::build`]
    /// so both FFI front-ends reject the same inputs. When `Some`, it replaces
    /// the clip's load-derived travel speed for this state's speed-scaled
    /// playback; `None` falls back to the derived value. `#[serde(default)]` and
    /// skip-if-absent so an override-free state round-trips without the key.
    #[serde(
        rename = "travelSpeed",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub travel_speed: Option<f32>,
    /// Clip index this state resolves to, filled at level load by
    /// `resolve_mesh_entity_clips` against the model's clip metadata. `None` =
    /// unresolved / unusable: switching *to* this state is a warn + no-op, and
    /// switching *out of* it is a hard cut (no outgoing pose to preserve).
    #[serde(skip, default)]
    pub clip_index: Option<usize>,
}

impl AnimationState {
    /// Resolve this state's calibration with authored override precedence. The
    /// caller supplies the active clip's load-derived speed, when available.
    /// Local simulation and remote presentation share this policy.
    pub fn effective_travel_speed(&self, derived_travel_speed: Option<f32>) -> Option<f32> {
        self.travel_speed.or(derived_travel_speed)
    }
}

/// The source the active fade blends *from*. Set by [`switch_animation_state`]
/// when a switch lands: a smooth interrupt of an active fade records `Snapshot`
/// (the collector then captures the in-flight blend), every other switch records
/// `Clip` (blend from the outgoing clip). A never-rendered same-tick intermediate
/// collapses out before it can record a source.
///
/// [`switch_animation_state`]: super::switch_animation_state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FadeSourceKind {
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
pub type AnimStamp = Option<f64>;

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
/// persisted because it is meaningful only while reconstructing that one capture.
#[derive(Debug, Clone, PartialEq)]
pub enum InterruptedOutgoing {
    /// The interrupted fade blended out of a clip: its state name and the entry
    /// stamp used when its rebase origin is still pending. Sampled at the
    /// interrupt instant on its own rebased timeline, falling back to that
    /// entry-stamp-relative scaled elapsed to reproduce the leg.
    Clip {
        state: String,
        entered_at: f64,
        rate: f32,
        rebase_time: AnimStamp,
        rebase_elapsed: f64,
    },
    /// The interrupted fade was itself a `"smooth"` snapshot fade: the prior
    /// snapshot, referenced by its store tag (the renderer's `SnapshotTag` — an
    /// `entered_at` bit pattern; kept as a plain `u64` here so this component
    /// stays free of any renderer dependency). The capture blends against that
    /// stored pose (a store hit), or degrades to the carried incoming fallback if
    /// its capture frame was culled.
    Snapshot { tag: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::animation::test_support::two_state_animation;
    use crate::components::mesh::MeshComponent;
    use crate::registry::ComponentValue;
    use glam::Vec3;

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
        assert!(as_value.get("shadow_bias_scale").is_none());
        assert!(
            (back.shadow_bias_scale - 1.0).abs() < f32::EPSILON,
            "old/default mesh payloads must deserialize with the bias-scale default"
        );
    }

    #[test]
    fn mesh_serializes_within_component_value_tagged_form() {
        let value = ComponentValue::Mesh(Box::new(MeshComponent::stateless("decraniated".into())));
        let json = serde_json::to_value(&value).unwrap();
        assert_eq!(json["kind"], "mesh");
        assert_eq!(json["model"], "decraniated");
    }

    #[test]
    fn animation_block_serde_round_trips_with_renames() {
        let mut animation = two_state_animation();
        // Exercise the non-default locomotion fields so their renames and
        // skip-if-absent predicates are covered: `speed_scale = false` must
        // serialize (default `true` is skipped), and a per-state `travelSpeed`
        // override on `attack` must serialize while `idle`'s absent override
        // stays off the wire.
        animation.speed_scale = false;
        animation.states.get_mut("attack").unwrap().travel_speed = Some(3.5);
        let value = MeshComponent {
            model: "decraniated".into(),
            animation: Some(animation),
            origin_offset: Vec3::ZERO,
            shadow_bias_scale: 1.0,
            pose_inputs: Some(crate::PoseInputs {
                aim_pitch: 0.25,
                aim_yaw: 0.5,
                heading_yaw: 0.75,
                ..Default::default()
            }),
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
        // `speedScale` rename: present because the non-default `false` is set.
        assert_eq!(json["animation"]["speedScale"], false);
        // `travelSpeed` rename: present on the overridden state, absent (skipped)
        // on the state that left the override unset.
        assert_eq!(states["attack"]["travelSpeed"], 3.5);
        assert!(
            states["idle"].get("travelSpeed").is_none(),
            "absent `travelSpeed` override must not enter the serialized state"
        );
        assert!(
            json.get("pose_inputs").is_none(),
            "transient pose inputs must not enter the serialized component"
        );

        // `clip_index` is `#[serde(skip)]` — runtime-resolved, never serialized.
        assert!(states["idle"].get("clip_index").is_none());

        // Round-trip back. `clip_index` deserializes to None (skip default), so
        // compare against the same shape with unresolved indices.
        let back: MeshComponent = serde_json::from_value(json).unwrap();
        let mut expected = value.clone();
        for s in expected.animation.as_mut().unwrap().states.values_mut() {
            s.clip_index = None;
        }
        expected.pose_inputs = None;
        assert_eq!(back, expected);
    }

    #[test]
    fn animation_block_serde_defaults_absent_locomotion_fields() {
        // Regression: `bool` serde defaults to false unless MeshAnimation
        // supplies its explicit `speedScale = true` default. This is the
        // runtime-reload shape emitted by old/default descriptors, not merely
        // a direct constructor test.
        let value = MeshComponent {
            model: "decraniated".into(),
            animation: Some(two_state_animation()),
            origin_offset: Vec3::ZERO,
            shadow_bias_scale: 1.0,
            pose_inputs: None,
        };
        let json = serde_json::to_value(&value).unwrap();
        let states = &json["animation"]["states"];
        assert!(
            json["animation"].get("speedScale").is_none(),
            "default true speedScale stays absent on the runtime wire shape"
        );
        assert!(
            states
                .as_object()
                .unwrap()
                .values()
                .all(|state| state.get("travelSpeed").is_none()),
            "override-free states omit travelSpeed"
        );

        let back: MeshComponent = serde_json::from_value(json).unwrap();
        let animation = back.animation.expect("serialized animation restores");
        assert!(
            animation.speed_scale,
            "absent speedScale must restore the authored default true"
        );
        assert!(
            animation
                .states
                .values()
                .all(|state| state.travel_speed.is_none()),
            "absent travelSpeed restores no per-state override"
        );
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
}
