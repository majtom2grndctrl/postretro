// Mesh component: model handle and presentation state for mesh entities.
// See: context/lib/scripting.md §10.3 (Mesh Animation)

use glam::{Mat4, Vec3};
use postretro_foundation::PoseInputs;
use serde::{Deserialize, Serialize};

pub use super::animation::{
    AnimStamp, AnimationState, DEFAULT_CROSSFADE_MS, FadeSourceKind, InterruptPolicy,
    InterruptedOutgoing, MeshAnimation, RATE_CHANGE_EPSILON, RATE_MAX, RATE_MIN, RestartResult,
    SwitchResult, resolve_pending_animation_stamps, restart_animation_clip, switch_animation_state,
};

/// Load-resolved location of a descriptor-authored attachment socket on this
/// mesh's holder model. This is presentation-only runtime state: it is rebuilt
/// after model loading and never crosses persistence or replication boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum AttachmentBinding {
    /// A socket on a skinned holder, indexed in the model's topological joint
    /// order.
    Skinned(usize),
    /// A socket on a rigid holder, expressed in model-space rest coordinates.
    Rigid(Mat4),
    /// The holder socket or attached model could not be resolved at load time.
    /// Render collection skips this attachment.
    #[default]
    Unresolved,
}

/// Descriptor-authored prop model mounted at one named socket of a mesh
/// holder. The authoring pair is serializable; [`binding`](Self::binding) is a
/// transient, load-time cache so a stale joint or matrix is never persisted or
/// replicated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshAttachment {
    pub socket: String,
    pub model: String,
    #[serde(skip)]
    pub binding: AttachmentBinding,
}

impl MeshAttachment {
    pub fn unresolved(socket: String, model: String) -> Self {
        Self {
            socket,
            model,
            binding: AttachmentBinding::Unresolved,
        }
    }
}

/// Marks an entity as rendering a mesh model. `model` is the model handle
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
    /// Descriptor-authored forward-visibility opt-out. Shadow-depth collection
    /// retains this mesh while the forward collector excludes it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub shadow_only: bool,
    /// Descriptor-authored prop models mounted at named holder sockets. Socket
    /// names and model handles persist; the resolved binding does not.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<MeshAttachment>,
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
            shadow_only: false,
            attachments: Vec::new(),
            pose_inputs: None,
        }
    }

    pub fn animated(model: String, animation: MeshAnimation) -> Self {
        Self {
            model,
            animation: Some(animation),
            origin_offset: Vec3::ZERO,
            shadow_bias_scale: 1.0,
            shadow_only: false,
            attachments: Vec::new(),
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

    pub fn with_shadow_only(mut self, shadow_only: bool) -> Self {
        self.shadow_only = shadow_only;
        self
    }

    /// Attach descriptor-authored socket/model pairs. Every materialized entry
    /// begins unresolved; level load fills the transient binding from the
    /// holder model's socket table.
    pub fn with_attachments(
        mut self,
        attachments: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        self.attachments = attachments
            .into_iter()
            .map(|(socket, model)| MeshAttachment::unresolved(socket, model))
            .collect();
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

fn is_false(value: &bool) -> bool {
    !*value
}

pub fn capsule_center_to_feet_origin_offset(radius: f32, height: f32) -> Vec3 {
    let half_height = (height / 2.0 - radius).max(0.0);
    Vec3::new(0.0, -(half_height + radius), 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_binding_is_transient_but_authoring_pair_round_trips() {
        let mut mesh = MeshComponent::stateless("models/holder.gltf".to_string())
            .with_attachments([("hand".to_string(), "models/prop.gltf".to_string())]);
        mesh.attachments[0].binding = AttachmentBinding::Rigid(Mat4::from_translation(Vec3::X));

        let json = serde_json::to_value(&mesh).expect("mesh component serializes");
        assert_eq!(json["attachments"][0]["socket"], "hand");
        assert_eq!(json["attachments"][0]["model"], "models/prop.gltf");
        assert!(
            json["attachments"][0].get("binding").is_none(),
            "resolved attachment bindings must not persist or replicate"
        );

        let restored: MeshComponent = serde_json::from_value(json).expect("mesh component parses");
        assert_eq!(restored.attachments[0].socket, "hand");
        assert_eq!(restored.attachments[0].model, "models/prop.gltf");
        assert_eq!(
            restored.attachments[0].binding,
            AttachmentBinding::Unresolved,
            "deserialization must never retain a stale resolved binding"
        );
    }

    #[test]
    fn shadow_only_defaults_false_and_serializes_when_enabled() {
        let default = MeshComponent::stateless("models/holder.gltf".to_string());
        assert!(!default.shadow_only);
        assert!(
            serde_json::to_value(&default)
                .unwrap()
                .get("shadow_only")
                .is_none(),
            "the default preserves existing serialized mesh payloads"
        );

        let shadow_only = default.with_shadow_only(true);
        let serialized = serde_json::to_value(&shadow_only).unwrap();
        assert_eq!(serialized["shadow_only"], true);
    }
}
