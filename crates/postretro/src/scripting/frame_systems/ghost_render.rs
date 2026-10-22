// Ghost render collector: each ghost's mesh group at its mapped pose, culled at the
// mapped cell, with its link's look through the per-ghost flash clamp.
// See: context/lib/rendering_pipeline.md §7.9, §7.8 · context/lib/entity_model.md §7d

use std::collections::HashMap;

use glam::{Mat4, Vec3};
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::{EntityId, EntityRegistry, SlotTable};
use postretro_level_loader::LevelWorld;
use postretro_model::ModelHandle;
use postretro_render_cpu::flash_clamp::GhostLookClamp;
use postretro_render_cpu::flash_limiter::{
    FLASH_LIMITER_SLOT, LimiterFrameInput, LimiterFrameUniform, flash_limiter_enabled_value,
    pack_limiter_frame,
};
use postretro_render_cpu::ghost_instance::GhostInstanceLook;
use postretro_render_cpu::ghost_transparency::GhostTransparency;
use postretro_render_cpu::mesh_instances::{
    MeshInstanceInput, MeshPaletteCacheKey, MeshPresentation,
};
use postretro_render_cpu::mesh_pass::mesh_visible;
use postretro_visibility::VisibleCells;

use super::attachments::{self, SocketPoseResolver};
use super::mesh_anim::MeshClipTables;
use super::mesh_render::{
    presentation_body_rotation, resolve_sample, should_resample, state_fingerprint,
};
use crate::ghost_links::GhostMember;

/// One ghost: a member entity seen through one link. Generation-checked, so a
/// re-tenanted slot is a new ghost, and a mover in two links is two ghosts.
pub(crate) type GhostDrawKey = (u16, EntityId);

/// The look `member` presents this frame through `clamp`: its link look as the
/// member reads it ([`GhostMember::look`]) at the member's presented opacity
/// (link opacity times membership), limited with its opacity, tint redness
/// and drawn brightness sharing one flash window. The one place a ghost draw
/// collector turns a link look into a drawn look; the skinned, mover and
/// billboard ghost collectors each call it for every presented ghost, culled
/// or not, between their clamp's `begin_frame` and `end_frame`.
pub(crate) fn presented_look(
    clamp: &mut GhostLookClamp<GhostDrawKey>,
    member: &GhostMember,
) -> GhostInstanceLook {
    let link = member.look;
    clamp.present(
        (member.link, member.entity),
        GhostInstanceLook {
            opacity: link.opacity * member.membership,
            saturation: link.saturation,
            tint: link.tint,
        },
    )
}

/// The last instance group built for a ghost: the holder then its attachments,
/// at mapped transforms under ghost palette keys. A fading ghost re-emits it
/// unchanged, since its entity may be gone and its live pose may be outside
/// the volume.
#[derive(Debug, Default)]
struct GhostGroup {
    instances: Vec<MeshInstanceInput>,
    /// The holder's mapped model origin: where the ghost culls.
    mapped_origin: Vec3,
    /// Still a ghost this frame; groups of departed ghosts are dropped.
    seen: bool,
}

/// One frame's read-only inputs beside the members and their looks.
pub(crate) struct GhostDrawFrame<'a> {
    pub world: &'a LevelWorld,
    pub visible: &'a VisibleCells,
    /// The accumulated animation clock the real mesh draw samples by.
    pub anim_time: f64,
    pub camera_pos: Vec3,
    pub tables: &'a MeshClipTables,
    pub socket_poses: &'a SocketPoseResolver<'a>,
    /// Models whose palette must resample every visible frame (precise hit
    /// zones, pose modifiers), as for the real draw.
    pub force_resample_model: &'a dyn Fn(&ModelHandle) -> bool,
    /// This presented frame's limiter input, or `None` for a capture, which
    /// never involves the limiter.
    pub limiter: Option<LimiterFrameUniform>,
    /// How ghosts draw translucent (`Renderer::ghost_transparency`), so the
    /// limiter judges a dithered ghost on the coverage it presents.
    pub transparency: GhostTransparency,
}

/// Game-side ghost draw collection. Owns no wgpu: it hands the renderer
/// [`MeshPresentation::Ghost`] instances through `set_ghost_mesh_draws`.
///
/// Per ghost member with a mesh: rebuild its group from the presented pose
/// while it is a live member (the same sample, pose inputs and attachment
/// emission as the real draw, under `mapping.matrix()`), or keep the cached
/// group while it fades; limit its presented opacity and tint on the ghost's
/// one flash window, which opacity, tint redness and drawn brightness share; then
/// cull the group at its mapped origin against the viewer's visible cells,
/// whatever the real position's visibility.
///
/// Palette caching: ghost instances take [`MeshPaletteCacheKey::Ghost`] keys,
/// never the real entity's. A live ghost samples the same pose as its real
/// draw, but a fading ghost must keep showing its last in-volume pose while
/// the real entity moves on; a shared key would serve it the live pose. The
/// snapshot store, keyed by entity and tag, is shared safely: a ghost forwards
/// the real draw's idempotent capture and samples the same fade.
#[derive(Debug, Default)]
pub(crate) struct GhostRenderCollector {
    instances: Vec<MeshInstanceInput>,
    groups: HashMap<GhostDrawKey, GhostGroup>,
    clamp: GhostLookClamp<GhostDrawKey>,
    /// Entered-state fingerprints per ghost, for the resample policy's
    /// state-change force; rebuilt each frame into the scratch and swapped.
    last_state: HashMap<GhostDrawKey, u64>,
    last_state_scratch: HashMap<GhostDrawKey, u64>,
    frame_index: u64,
    world_pose: Vec<Mat4>,
    holder_handles: HashMap<String, ModelHandle>,
    attachment_handles: HashMap<String, ModelHandle>,
}

impl GhostRenderCollector {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Drop every ghost group and the clamp's history: level unload.
    pub(crate) fn clear(&mut self) {
        *self = Self::new();
    }

    /// Collect this frame's ghost instances. `members` is the frame's ghost
    /// set (`GhostLinks::members`), each carrying its own look. No allocation
    /// once every ghost's group and model handle exist.
    pub(crate) fn collect(
        &mut self,
        registry: &EntityRegistry,
        members: &[GhostMember],
        frame: &GhostDrawFrame<'_>,
    ) {
        self.instances.clear();
        self.last_state_scratch.clear();
        self.clamp.set_transparency(frame.transparency);
        self.clamp.begin_frame(frame.limiter.as_ref());
        for member in members {
            let key = (member.link, member.entity);
            if !member.frozen {
                self.rebuild_group(registry, member, frame);
            }
            // A ghost whose entity draws no mesh has no group: another kind's
            // collector presents it, or nothing does.
            let Some(group) = self.groups.get_mut(&key) else {
                continue;
            };
            group.seen = true;
            // Every presented ghost passes its clamp, culled or not: the
            // limiter judges the source's presented value, not its framing.
            let ghost_look = presented_look(&mut self.clamp, member);
            if ghost_look.opacity <= 0.0
                || !mesh_visible(frame.world, frame.visible, group.mapped_origin)
            {
                continue;
            }
            for instance in &group.instances {
                let mut ghost = instance.clone();
                ghost.presentation = MeshPresentation::Ghost(ghost_look);
                if member.frozen {
                    // Hold the cached pose: never resample a departed pose, and
                    // never replay a one-time capture.
                    ghost.resample = false;
                    ghost.capture = None;
                }
                self.instances.push(ghost);
            }
        }
        self.clamp.end_frame();
        self.groups
            .retain(|_, group| std::mem::take(&mut group.seen));
        std::mem::swap(&mut self.last_state, &mut self.last_state_scratch);
        self.frame_index = self.frame_index.wrapping_add(1);
    }

    /// The ghost instances to hand the renderer this frame.
    pub(crate) fn instances(&self) -> &[MeshInstanceInput] {
        &self.instances
    }

    /// Settle this frame's look clamp once the renderer knows whether the
    /// frame presented: one that presented nothing leaves the clamp as if it
    /// never ran and carries its time into the next frame.
    pub(crate) fn finish_presentation(&mut self, presented: bool) {
        self.clamp.finish_presentation(presented);
    }

    /// Rebuild a live member's group from its presented pose: the holder at
    /// `mapping.matrix() * holder_transform`, then its attachments sampled from
    /// the holder's modifier-applied pose, all mapped as one rigid group. An
    /// entity with no mesh drops any group it had.
    fn rebuild_group(
        &mut self,
        registry: &EntityRegistry,
        member: &GhostMember,
        frame: &GhostDrawFrame<'_>,
    ) {
        let key = (member.link, member.entity);
        let Ok(mesh) = registry.get_component::<MeshComponent>(member.entity) else {
            self.groups.remove(&key);
            return;
        };
        let Self {
            groups,
            last_state,
            last_state_scratch,
            frame_index,
            world_pose,
            holder_handles,
            attachment_handles,
            ..
        } = self;
        let handle = intern_handle(holder_handles, &mesh.model);
        let source = member.source;
        let holder_transform = Mat4::from_scale_rotation_translation(
            source.scale,
            presentation_body_rotation(
                source.rotation,
                mesh.pose_inputs.map(|inputs| inputs.heading_yaw),
            ),
            source.position + mesh.origin_offset,
        );
        let seed = member.entity.to_raw();
        let (sample, capture) = resolve_sample(
            mesh.animation.as_ref(),
            &handle,
            frame.tables,
            frame.anim_time,
            seed,
        );
        let state_changed = state_fingerprint(mesh.animation.as_ref()).is_some_and(|stamp| {
            last_state_scratch.insert(key, stamp);
            last_state.get(&key) != Some(&stamp)
        });

        let group = groups.entry(key).or_default();
        group.instances.clear();
        group.instances.push(MeshInstanceInput {
            model: handle.clone(),
            transform: holder_transform,
            shadow_bias_scale: mesh.shadow_bias_scale,
            phase_seed: seed,
            palette_cache_key: MeshPaletteCacheKey::Entity(seed),
            sample,
            pose_inputs: mesh.pose_inputs,
            capture,
            resample: false,
            forward_visible: true,
            dynamic_shadow_visible: false,
            presentation: MeshPresentation::World,
        });
        let has_skinned_attachment = attachments::emit_for_holder(
            &mut group.instances,
            world_pose,
            attachment_handles,
            frame.socket_poses,
            &handle,
            holder_transform,
            sample,
            mesh.pose_inputs,
            mesh.shadow_bias_scale,
            seed,
            true,
            false,
            &mesh.attachments,
        );

        let mapping = member.mapping.matrix();
        for instance in &mut group.instances {
            instance.transform = mapping * instance.transform;
            instance.palette_cache_key = instance.palette_cache_key.ghost_through(member.link);
        }
        group.mapped_origin = group.instances[0].transform.w_axis.truncate();

        // The real draw's time-slicing policy, by the ghost's own distance.
        let force = state_changed
            || sample.fade.is_some()
            || capture.is_some()
            || has_skinned_attachment
            || (frame.force_resample_model)(&handle);
        let distance = group.mapped_origin.distance(frame.camera_pos);
        group.instances[0].resample = should_resample(distance, *frame_index, seed, force);
    }
}

/// The per-ghost clamp's limiter input for this presented frame: the ghost
/// membership ramp's presented-frame time (`GhostLinks::presented_elapsed`, the
/// same clock the ramp stepped by) and the player's fail-safe enable flag, read
/// from the slot store this frame's resolve snapshot copies. The keyed clamp
/// starts fresh on its own when the limiter turns on.
pub(crate) fn ghost_limiter_frame(slots: &SlotTable, elapsed_seconds: f32) -> LimiterFrameUniform {
    let enabled = flash_limiter_enabled_value(
        slots
            .get(FLASH_LIMITER_SLOT)
            .and_then(|record| record.value.as_ref()),
    );
    pack_limiter_frame(LimiterFrameInput { elapsed_seconds }, enabled, false)
}

fn intern_handle(handles: &mut HashMap<String, ModelHandle>, model: &str) -> ModelHandle {
    if let Some(handle) = handles.get(model) {
        return handle.clone();
    }
    let handle = ModelHandle::from(model);
    handles.insert(model.to_owned(), handle.clone());
    handle
}

#[cfg(test)]
#[path = "ghost_render_tests.rs"]
mod tests;
