// Attachment draw-input emission and game-side socket-pose sampling.
// See: context/lib/rendering_pipeline.md §9

use std::collections::HashMap;

use glam::Mat4;
use postretro_entities::PoseInputs;
use postretro_entities::components::mesh::{AttachmentBinding, MeshAttachment};
use postretro_model::ModelHandle;
use postretro_model::anim::{
    BlendSource, Loop, sample_blended_world_modified, sample_clip_looped_world_modified,
    sample_rest_pose_world_modified,
};
use postretro_model::sample_params::{ClipSample, FadeSource, MeshSampleParams};
use postretro_model::skeleton::AnimationClip;
use postretro_render_cpu::mesh_instances::{MeshInstanceInput, MeshPaletteCacheKey};

use super::hit_zones::{HitZoneStore, ModelHitZones};

/// Read-only access to the model data needed to reproduce the renderer's
/// modifier-applied world pose for a holder socket. It deliberately reads the
/// same retained skeleton, clips, and pose stack as the palette path.
pub(crate) struct SocketPoseResolver<'a> {
    models: Option<&'a HitZoneStore>,
}

fn attachment_model_handle(handles: &mut HashMap<String, ModelHandle>, model: &str) -> ModelHandle {
    if let Some(handle) = handles.get(model) {
        return handle.clone();
    }
    let handle = ModelHandle::from(model);
    handles.insert(model.to_owned(), handle.clone());
    handle
}

impl<'a> SocketPoseResolver<'a> {
    pub(crate) fn new(models: &'a HitZoneStore) -> Self {
        Self {
            models: Some(models),
        }
    }

    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self { models: None }
    }

    fn model(&self, handle: &ModelHandle) -> Option<&ModelHitZones> {
        self.models?.get(handle)
    }
}

/// Emit attachment draw inputs for one already-collected holder.
///
/// Every attachment inherits the holder's cull decision. Skinned bindings share
/// a single modifier-applied world-pose sample; rigid bindings consume their
/// load-resolved rest matrix directly. No attachment independently looks up its
/// socket table or cell.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_for_holder(
    instances: &mut Vec<MeshInstanceInput>,
    world_pose: &mut Vec<Mat4>,
    attachment_handles: &mut HashMap<String, ModelHandle>,
    resolver: &SocketPoseResolver<'_>,
    holder: &ModelHandle,
    holder_transform: Mat4,
    holder_sample: MeshSampleParams,
    holder_pose_inputs: Option<PoseInputs>,
    holder_shadow_bias_scale: f32,
    holder_seed: u32,
    forward_visible: bool,
    attachments: &[MeshAttachment],
) -> bool {
    if attachments.is_empty() {
        return false;
    }

    let mut has_skinned_binding = false;
    let mut sampled_world_pose = None;

    for (attachment_index, attachment) in attachments.iter().enumerate() {
        let socket_matrix = match attachment.binding {
            AttachmentBinding::Skinned(joint) => {
                has_skinned_binding = true;
                let sampled = *sampled_world_pose.get_or_insert_with(|| {
                    sample_modified_world_pose(
                        resolver,
                        holder,
                        holder_sample,
                        holder_pose_inputs.as_ref(),
                        world_pose,
                    )
                });
                sampled.then(|| world_pose.get(joint).copied()).flatten()
            }
            AttachmentBinding::Rigid(matrix) => Some(matrix),
            AttachmentBinding::Unresolved => None,
        };
        let Some(socket_matrix) = socket_matrix else {
            continue;
        };

        instances.push(MeshInstanceInput {
            model: attachment_model_handle(attachment_handles, attachment.model.as_str()),
            transform: holder_transform * socket_matrix,
            shadow_bias_scale: holder_shadow_bias_scale,
            // Attachment props never capture snapshots or sample clips. Their
            // cache identity is distinct from the holder; the phase seed is
            // irrelevant because `sample` never contains a fade or capture.
            phase_seed: holder_seed,
            palette_cache_key: MeshPaletteCacheKey::Attachment {
                holder: holder_seed,
                attachment_index,
            },
            sample: MeshSampleParams::rigid(),
            pose_inputs: None,
            capture: None,
            // The renderer samples the one-entry identity palette on its first
            // cache miss, then reuses it; rigid props never need pose updates.
            resample: false,
            forward_visible,
            is_viewmodel: false,
        });
    }

    has_skinned_binding
}

/// Sample a holder's modifier-applied world pose using the exact params emitted
/// for its body. Snapshot fades are renderer-owned: unlike a clip source they
/// cannot be reconstructed here, so their attachment falls back to the primary
/// modified pose for that fade window (the same degrade shape as hit zones).
fn sample_modified_world_pose(
    resolver: &SocketPoseResolver<'_>,
    holder: &ModelHandle,
    params: MeshSampleParams,
    pose_inputs: Option<&PoseInputs>,
    out: &mut Vec<Mat4>,
) -> bool {
    let Some(model) = resolver.model(holder) else {
        return false;
    };

    if params.is_rest_pose() {
        sample_rest_pose(model, pose_inputs, out);
        return true;
    }

    let Some(primary) = clip_blend_source(model, params.primary) else {
        sample_rest_pose(model, pose_inputs, out);
        return true;
    };

    match params.fade {
        Some(fade) => match fade.from {
            FadeSource::Clip(leg) => match clip_blend_source(model, leg) {
                Some(from) => sample_blended_world_modified(
                    &from,
                    &primary,
                    fade.weight,
                    &model.skeleton,
                    &model.pose_stack,
                    pose_inputs,
                    out,
                ),
                None => sample_primary_world_pose(model, params.primary, pose_inputs, out),
            },
            // `Snapshot` can only be materialized by the renderer's snapshot
            // store. Do not substitute its fallback here: that could disagree
            // with a successful renderer capture. The primary clip is stable.
            FadeSource::Snapshot { .. } => {
                sample_primary_world_pose(model, params.primary, pose_inputs, out)
            }
        },
        None => sample_primary_world_pose(model, params.primary, pose_inputs, out),
    }
    true
}

fn sample_primary_world_pose(
    model: &ModelHitZones,
    primary: ClipSample,
    pose_inputs: Option<&PoseInputs>,
    out: &mut Vec<Mat4>,
) {
    let Some(clip) = model.clips.get(primary.clip_index) else {
        sample_rest_pose(model, pose_inputs, out);
        return;
    };
    sample_clip_looped_world_modified(
        clip,
        &model.skeleton,
        primary.time,
        primary.loop_policy,
        &model.pose_stack,
        pose_inputs,
        out,
    );
}

fn clip_blend_source<'a>(model: &'a ModelHitZones, sample: ClipSample) -> Option<BlendSource<'a>> {
    model
        .clips
        .get(sample.clip_index)
        .map(|clip| BlendSource::Clip {
            clip,
            time: sample.time,
            loop_policy: sample.loop_policy,
        })
}

fn sample_rest_pose(model: &ModelHitZones, pose_inputs: Option<&PoseInputs>, out: &mut Vec<Mat4>) {
    if let Some(inputs) = pose_inputs {
        sample_rest_pose_world_modified(&model.skeleton, &model.pose_stack, inputs, out);
        return;
    }

    // The model sampler has a modified-rest entry point only when there are
    // per-entity inputs. An empty clip is the same rest-local composition when
    // there is nothing to apply, and keeps the unmodified world sampler untouched.
    let rest = AnimationClip::default();
    sample_clip_looped_world_modified(
        &rest,
        &model.skeleton,
        0.0,
        Loop::Clamp,
        &model.pose_stack,
        None,
        out,
    );
}
