// Mesh component: model handle and presentation state for mesh entities.
// See: context/lib/scripting.md §10.3 (Mesh Animation)

use glam::{Mat4, Quat, Vec3};
use postretro_foundation::PoseInputs;
use serde::{Deserialize, Serialize};

pub use super::animation::{
    AnimStamp, AnimationState, DEFAULT_CROSSFADE_MS, FadeSourceKind, InterruptPolicy,
    InterruptedOutgoing, LocomotionDirection, LocomotionJumpPhase, MeshAnimation,
    PlaybackRateTarget, RATE_CHANGE_EPSILON, RATE_MAX, RATE_MIN, RestartResult, SwitchCount,
    SwitchResult, ZeroPhaseStates, resolve_pending_animation_stamps, restart_animation_clip,
    switch_animation_state,
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

/// Half turn about +Y from a model's front (glTF +Z) to the view frame's forward
/// (−Z). The one constant both facing seams compose: placement
/// (`MapEntity::model_rotation_quat`) and the body seam ([`BodyFrame::View`]).
/// See: context/lib/entity_model.md §4 (Facing convention)
pub const MODEL_FRONT_TO_VIEW_FORWARD: Quat = Quat::from_xyzw(0.0, 1.0, 0.0, 0.0);

/// Which frame a mesh holder's transform rotation is in. Every model fronts
/// glTF +Z. A placed entity's rotation already turns that front along its
/// arrow ([`Model`](Self::Model)). A player pawn keeps its rotation in the view
/// frame, forward −Z, so camera, movement and the wire stay unturned; its body
/// takes the half turn wherever that rotation becomes a model→world matrix
/// ([`View`](Self::View)).
///
/// Each peer derives it when it materializes the body, like `shadow_only`: it
/// never crosses the wire or enters a save.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BodyFrame {
    /// The transform rotation turns model +Z to the entity's front.
    #[default]
    Model,
    /// The transform rotation is a view-frame rotation (forward −Z); the body
    /// seam adds [`MODEL_FRONT_TO_VIEW_FORWARD`].
    View,
}

impl BodyFrame {
    /// Model→world rotation for a holder whose transform (or presentation)
    /// rotation is `frame_rotation`.
    pub fn model_rotation(self, frame_rotation: Quat) -> Quat {
        match self {
            Self::Model => frame_rotation,
            Self::View => frame_rotation * MODEL_FRONT_TO_VIEW_FORWARD,
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
/// `AgentComponent`. Player pawns hang from their *live* capsule's sole
/// ([`player_capsule_feet_origin_offset`]), reseated as crouch or posture
/// resizes the capsule (`entity_model.md` §7b). Hit zones and foot probes read
/// the same offset, so it is not purely presentational.
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
    /// Frame of the holder's transform rotation. Presentation state each peer
    /// derives at materialization; never persisted or replicated.
    #[serde(skip)]
    pub body_frame: BodyFrame,
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
            body_frame: BodyFrame::Model,
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
            body_frame: BodyFrame::Model,
        }
    }

    pub fn with_body_frame(mut self, body_frame: BodyFrame) -> Self {
        self.body_frame = body_frame;
        self
    }

    /// Rotation the body is drawn at: the lower-body `heading_yaw` from this
    /// mesh's pose inputs when an animated mesh carries one, else the
    /// transform's own rotation, then the body-frame turn. A stateless mesh's
    /// pose inputs are foot-probe defaults (heading 0), so its heading never
    /// replaces the transform yaw.
    pub fn presentation_rotation(&self, transform_rotation: Quat) -> Quat {
        let heading = self
            .animation
            .as_ref()
            .and(self.pose_inputs)
            .map(|inputs| inputs.heading_yaw)
            .filter(|yaw| yaw.is_finite());
        self.body_frame.model_rotation(
            heading
                .map(Quat::from_rotation_y)
                .unwrap_or(transform_rotation),
        )
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
    player_capsule_feet_origin_offset(radius, height / 2.0 - radius)
}

/// Render-origin offset that drops a feet-authored mesh from a player capsule's
/// centre to its sole, for a capsule of `radius` and cylinder `half_height`.
///
/// A player's transform is its capsule centre. Crouch, slide and a low
/// life-state posture shrink `half_height` with the feet planted, which lowers
/// the centre, so the offset must follow the *live* capsule: one frozen at the
/// standing size buries the model (and the point it is culled at) under the
/// floor (`entity_model.md` §7b).
pub fn player_capsule_feet_origin_offset(radius: f32, half_height: f32) -> Vec3 {
    Vec3::new(0.0, -(half_height.max(0.0) + radius), 0.0)
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

    // Each peer derives a player body's frame when it materializes the body; a
    // save or a replicated payload carrying it would let one peer's frame
    // override another's, and a restored body would skip its derivation step.
    #[test]
    fn player_body_frame_never_serializes_and_a_restored_mesh_has_no_body_frame() {
        let body = MeshComponent::stateless("models/player.gltf".to_string())
            .with_shadow_only(true)
            .with_body_frame(BodyFrame::View);

        let json = serde_json::to_value(&body).expect("mesh component serializes");
        let object = json.as_object().expect("mesh component is a JSON object");
        assert!(
            object
                .keys()
                .all(|key| !key.contains("frame") && !key.contains("body")),
            "a player body's mesh payload carries no body-frame key: {json}"
        );
        assert_eq!(
            object.len(),
            2,
            "only the model and the authored shadow_only flag persist: {json}"
        );

        let restored: MeshComponent = serde_json::from_value(json).expect("mesh component parses");
        assert_eq!(
            restored.body_frame,
            BodyFrame::Model,
            "a deserialized mesh carries no body frame until its peer derives one"
        );
        assert_eq!(restored.model, body.model);
        assert!(restored.shadow_only);
    }
}
