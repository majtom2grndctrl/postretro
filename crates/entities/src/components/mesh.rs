// Mesh component: the model handle a skinned-model entity renders.
// See: context/lib/scripting.md §10.3 (Mesh Animation)

use glam::Vec3;
use postretro_foundation::PoseInputs;
use serde::{Deserialize, Serialize};

pub use super::animation::{
    AnimStamp, AnimationState, DEFAULT_CROSSFADE_MS, FadeSourceKind, InterruptPolicy,
    InterruptedOutgoing, MeshAnimation, RATE_CHANGE_EPSILON, RATE_MAX, RATE_MIN, RestartResult,
    SwitchResult, resolve_pending_animation_stamps, restart_animation_clip, switch_animation_state,
};

/// Marks an entity as rendering a skinned model. `model` is the model handle
/// the `prop_mesh` classname handler reads from a map entity's `model` key — the
/// content-canonical path passed to `postretro_model::gltf_loader::load_model`. It
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
pub struct MeshComponent {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<MeshAnimation>,
    #[serde(default, skip_serializing_if = "vec3_is_zero")]
    pub origin_offset: Vec3,
    /// Per-model multiplier for the skinned receiver-side pool-shadow bias.
    /// Omitted persisted and replicated component payloads retain 1.0.
    #[serde(
        default = "default_shadow_bias_scale",
        skip_serializing_if = "shadow_bias_scale_is_default"
    )]
    pub shadow_bias_scale: f32,
    /// Same-tick presentation inputs for the model's pose-modifier stack.
    /// Gameplay and persistence intentionally ignore this transient value.
    #[serde(skip)]
    pub pose_inputs: Option<PoseInputs>,
}

impl MeshComponent {
    /// Convenience for the stateless `prop_mesh` path: a model handle with no
    /// animation block.
    pub fn stateless(model: String) -> Self {
        Self {
            model,
            animation: None,
            origin_offset: Vec3::ZERO,
            shadow_bias_scale: 1.0,
            pose_inputs: None,
        }
    }

    pub fn animated(model: String, animation: MeshAnimation) -> Self {
        Self {
            model,
            animation: Some(animation),
            origin_offset: Vec3::ZERO,
            shadow_bias_scale: 1.0,
            pose_inputs: None,
        }
    }

    pub fn with_origin_offset(mut self, origin_offset: Vec3) -> Self {
        self.origin_offset = origin_offset;
        self
    }

    pub fn with_shadow_bias_scale(mut self, shadow_bias_scale: f32) -> Self {
        self.shadow_bias_scale = shadow_bias_scale;
        self
    }
}

fn vec3_is_zero(value: &Vec3) -> bool {
    *value == Vec3::ZERO
}

fn default_shadow_bias_scale() -> f32 {
    1.0
}

fn shadow_bias_scale_is_default(value: &f32) -> bool {
    *value == default_shadow_bias_scale()
}

pub fn capsule_center_to_feet_origin_offset(radius: f32, height: f32) -> Vec3 {
    let half_height = (height / 2.0 - radius).max(0.0);
    Vec3::new(0.0, -(half_height + radius), 0.0)
}
