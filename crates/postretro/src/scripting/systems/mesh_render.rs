// Mesh render collector: walks MeshComponent entities and gathers per-instance
// skinned-draw inputs (model handle + interpolated transform) for the renderer.
// See: context/lib/entity_model.md §5 · context/lib/rendering_pipeline.md §9

use std::collections::HashMap;

use super::attachments::{self, SocketPoseResolver};
use super::hit_zones::HitZoneStore;
use super::mesh_anim::{self, MeshClipTables};
use postretro_entities::registry::{ComponentKind, ComponentValue, EntityRegistry, Transform};
use postretro_level_loader::LevelWorld;
use postretro_model::ModelHandle;
use postretro_model::sample_params::MeshSampleParams;
use postretro_render_cpu::mesh_instances::{MeshInstanceInput, MeshPaletteCacheKey};
use postretro_render_cpu::mesh_pass::mesh_visible;
use postretro_render_data::cone_frustum::Aabb;
use postretro_render_data::influence::LightInfluence;
use postretro_visibility::VisibleCells;

/// Animation time-slicing distance thresholds + per-bucket resample strides.
/// DISTANT skinned instances re-sample their pose every Nth frame and re-upload a
/// cached palette on the skipped frames, trading pose freshness for CPU sampling
/// cost. Off-screen instances already cost nothing (culled before planning); this
/// cuts the steady-state per-instance sample rate for the ones that are visible
/// but far.
///
/// TUNABLE, not a contract: the ~20 m / ~40 m split and the 1 / 2 / 4 strides are
/// picked so a near monster stays frame-fresh while a distant crowd de-syncs its
/// sampling. Adjust against the camera FOV and the representative wave size; the
/// acceptance test pins the *shape* (near every frame, far at stride) and
/// survives a retune of the exact numbers.
const RESAMPLE_NEAR_DISTANCE: f32 = 20.0;
/// Upper distance threshold (meters): beyond this an instance falls in the
/// farthest bucket ([`RESAMPLE_STRIDE_FAR`]). TUNABLE — see
/// [`RESAMPLE_NEAR_DISTANCE`].
const RESAMPLE_FAR_DISTANCE: f32 = 40.0;
/// Near bucket (`distance <= RESAMPLE_NEAR_DISTANCE`): resample every frame —
/// stride 1 means the modulo test is always true.
const RESAMPLE_STRIDE_NEAR: u64 = 1;
/// Mid bucket (`RESAMPLE_NEAR_DISTANCE < distance <= RESAMPLE_FAR_DISTANCE`):
/// resample every 2nd frame. TUNABLE — see [`RESAMPLE_NEAR_DISTANCE`].
const RESAMPLE_STRIDE_MID: u64 = 2;
/// Far bucket (`distance > RESAMPLE_FAR_DISTANCE`): resample every 4th frame.
/// TUNABLE — see [`RESAMPLE_NEAR_DISTANCE`].
const RESAMPLE_STRIDE_FAR: u64 = 4;

/// The resample stride (in frames) for an instance at `distance` meters from the
/// camera. A larger stride means a lower re-sample rate (more cached re-uploads).
/// Pure data logic — the bucketing half of the time-slicing decision, factored
/// out so a collector unit test asserts the near/mid/far rates without a device.
fn resample_stride(distance: f32) -> u64 {
    if distance <= RESAMPLE_NEAR_DISTANCE {
        RESAMPLE_STRIDE_NEAR
    } else if distance <= RESAMPLE_FAR_DISTANCE {
        RESAMPLE_STRIDE_MID
    } else {
        RESAMPLE_STRIDE_FAR
    }
}

/// Whether an instance re-samples its pose this frame (time-slicing).
/// `force` short-circuits the stride test (a just-changed state or an active
/// crossfade must resample so the transition is never frozen on a skipped frame).
/// Otherwise the per-entity phase `(frame_index + seed) % stride == 0` decides:
/// folding `seed` in de-syncs distant instances so a far crowd does not resample
/// in lock-step. Stride 1 (near bucket) makes the modulo always true → every
/// frame. Pure data logic; the renderer-side cache may still upgrade a `false` to
/// a resample on a cache miss.
fn should_resample(distance: f32, frame_index: u64, seed: u32, force: bool) -> bool {
    if force {
        return true;
    }
    let stride = resample_stride(distance);
    (frame_index.wrapping_add(seed as u64)) % stride == 0
}

/// Per-frame scratch state for the skinned-mesh render path. Owned by the game
/// layer (not the renderer) so the wgpu boundary stays inside `MeshPass` —
/// mirrors `ParticleRenderCollector`'s ownership split.
///
/// Runs in the render-frame collection sub-stage (NOT the game-logic tick): it
/// reads the registry + the world + this frame's visible-cell set, applies the
/// pure `mesh_pass::mesh_visible` cull, and emits per-instance draw inputs
/// (model handle + interpolated world transform). Forward visibility, dynamic
/// shadow eligibility, and promoted-static relevance remain distinct. Explicit
/// shadow-only instances stay eligible for dynamic depth even outside camera PVS;
/// other off-PVS instances are retained only for selected static lights. It never touches wgpu — the
/// renderer consumes [`instances`] and owns the GPU upload + draw recording.
///
/// [`instances`]: MeshRenderCollector::instances
pub(crate) struct MeshRenderCollector {
    /// Per-frame instance list: surviving `MeshInstanceInput` values — each
    /// carrying a model handle, interpolated world transform, phase seed,
    /// resolved sample params (`MeshSampleParams`), an optional capture
    /// instruction, and a resample flag. Cleared + refilled each collection so
    /// capacity carries across frames.
    instances: Vec<MeshInstanceInput>,
    /// Monotonic frame index, bumped once per collection. Drives the per-bucket
    /// resample stride phase (`(frame_index + seed) % stride`). Owned here so the
    /// time-slicing decision stays entirely game-side and testable without
    /// threading a counter through the render loop. `wrapping`-incremented; the
    /// modulo phase is unaffected by the eventual wrap.
    frame_index: u64,
    /// Per-entity last-seen state fingerprint (the entered-state stamp bits),
    /// keyed by entity seed. A change between frames means the entity (re)entered
    /// a state this frame, which forces a resample so the transition is never
    /// frozen on a skipped frame. Bounded by the live animated-entity count;
    /// entries absent from a frame drop so it never grows past the active set.
    last_state: HashMap<u32, u64>,
    /// Scratch for the rebuilt `last_state` map each frame — swapped with
    /// `last_state` so a steady-state frame reuses both allocations (no per-frame
    /// map churn).
    last_state_scratch: HashMap<u32, u64>,
    /// Count of instances that resampled this frame (the time-slicing metric).
    /// Tallied at the bucketing decision — the game-side counter a collector unit
    /// test asserts the reduced rate against without a GPU device.
    resample_count: u32,
    /// Reused modifier-applied world-pose buffer for skinned socket bindings.
    /// One holder fills it once and all of that holder's skinned attachments read
    /// their matrices from the same sample.
    attachment_world_pose: Vec<glam::Mat4>,
    /// Persistent path-to-handle cache for attachment draw inputs. ModelHandle
    /// paths are interned here so steady-state collection does not allocate.
    attachment_model_handles: HashMap<String, ModelHandle>,
    /// Persistent path-to-handle cache for the first-person weapon presentation.
    /// This stays separate from attachment handles because a viewmodel is never a
    /// holder attachment and must enter the planner's dedicated plan.
    viewmodel_model_handles: HashMap<String, ModelHandle>,
}

impl MeshRenderCollector {
    pub(crate) fn new() -> Self {
        Self {
            instances: Vec::new(),
            frame_index: 0,
            last_state: HashMap::new(),
            last_state_scratch: HashMap::new(),
            resample_count: 0,
            attachment_world_pose: Vec::new(),
            attachment_model_handles: HashMap::new(),
            viewmodel_model_handles: HashMap::new(),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.instances.clear();
        self.last_state.clear();
        self.last_state_scratch.clear();
        self.resample_count = 0;
        self.attachment_world_pose.clear();
        self.attachment_model_handles.clear();
        self.viewmodel_model_handles.clear();
    }

    /// Walk `ComponentKind::Mesh` entities, cull each against the frame's
    /// visible set, and emit the survivors' draw inputs (handle, interpolated
    /// transform, resolved sample params, optional capture instruction).
    ///
    /// Clears the instance list first (reusing capacity), then for each mesh
    /// entity: read-borrows its `MeshComponent` (the model handle + optional
    /// animation state) and its `Transform`. The cull tests the entity's
    /// **current-tick** rendered model origin (`Transform.position +
    /// MeshComponent.origin_offset`) via the pure `mesh_pass::mesh_visible`;
    /// survivors emit their **interpolated** transform (the registry's
    /// interpolated-transform accessor at the frame `alpha`, the same alpha the
    /// player camera reads from `frame_timing`) plus that same origin offset so
    /// the model renders smoothly between ticks.
    ///
    /// Animation: `anim_time` is the accumulated game-layer animation clock
    /// (`frame_dt × time_scale`); `tables` is the level-load clip table set. For
    /// an animated entity the collector resolves its current/previous states into
    /// per-instance `MeshSampleParams` (clip-local times, crossfade weight,
    /// snapshot fade) and emits a one-time capture instruction on a `"smooth"`
    /// interrupt frame. A stateless `prop_mesh` entity (no animation block)
    /// explicitly selects the model's authored rest pose.
    ///
    /// The per-instance phase seed is the raw `EntityId`, folded into a
    /// deterministic phase offset so a spawned wave does not animate lock-step
    /// (looping states only — one-shot states play from entry, no phase). It also
    /// keys the snapshot store on a `"smooth"` capture.
    ///
    /// Animation time-slicing: `camera_pos` is this frame's camera eye
    /// position. Each survivor's distance to it picks a resample stride bucket
    /// ([`resample_stride`]); the per-entity phase `(frame_index + seed) % stride`
    /// then decides whether the instance re-samples this frame. A state change
    /// (entered-stamp fingerprint moved) or an active crossfade FORCES a resample
    /// so a transition is never frozen on a skipped frame. The per-frame resample
    /// tally is exposed via [`resample_count`] (the game-side acceptance metric).
    ///
    /// [`instances`]: MeshRenderCollector::instances
    /// [`resample_count`]: MeshRenderCollector::resample_count
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn collect(
        &mut self,
        registry: &EntityRegistry,
        world: &LevelWorld,
        visible: &VisibleCells,
        alpha: f32,
        anim_time: f64,
        tables: &MeshClipTables,
        camera_pos: glam::Vec3,
    ) {
        let socket_poses = SocketPoseResolver::empty();
        self.collect_inner(
            registry,
            world,
            visible,
            alpha,
            anim_time,
            tables,
            &socket_poses,
            camera_pos,
            |_| false,
        );
    }

    /// Production collect path. Precise skeletal hit zones force a palette
    /// resample every visible frame to align presentation animation timing with
    /// authoritative capsule samples. A non-empty pose stack also forces a
    /// resample because its presentation inputs may change independently of
    /// animation timing. Pose modifiers intentionally leave authoritative
    /// capsules unmodified.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn collect_with_hit_zones(
        &mut self,
        registry: &EntityRegistry,
        world: &LevelWorld,
        visible: &VisibleCells,
        alpha: f32,
        anim_time: f64,
        tables: &MeshClipTables,
        camera_pos: glam::Vec3,
        hit_zones: &HitZoneStore,
    ) {
        let socket_poses = SocketPoseResolver::new(hit_zones);
        self.collect_inner(
            registry,
            world,
            visible,
            alpha,
            anim_time,
            tables,
            &socket_poses,
            camera_pos,
            |handle| hit_zones.has_precise_zones(handle) || hit_zones.has_pose_modifiers(handle),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_inner(
        &mut self,
        registry: &EntityRegistry,
        world: &LevelWorld,
        visible: &VisibleCells,
        alpha: f32,
        anim_time: f64,
        tables: &MeshClipTables,
        socket_poses: &SocketPoseResolver<'_>,
        camera_pos: glam::Vec3,
        force_resample_model: impl Fn(&ModelHandle) -> bool,
    ) {
        self.instances.clear();
        // Rebuild the last-state map into the scratch so entries absent this
        // frame (despawned / culled-out entities) drop — bounding it by the live
        // animated-entity count. Swapped back at the end; both allocations carry.
        self.last_state_scratch.clear();
        self.resample_count = 0;
        let frame_index = self.frame_index;

        for (id, value) in registry.iter_with_kind(ComponentKind::Mesh) {
            let ComponentValue::Mesh(mesh) = value else {
                continue;
            };
            // Cull on the CURRENT-TICK rendered model origin (stable per-tick
            // visibility), not the sub-tick interpolated position. This must
            // include the same MeshComponent origin offset applied to the emitted
            // instance transform so visibility classifies the position downstream
            // forward/shadow consumers use.
            let Ok(current) = registry.get_component::<Transform>(id) else {
                continue;
            };
            let current_model_position = current.position + mesh.origin_offset;
            let portal_visible = mesh_visible(world, visible, current_model_position);
            let forward_visible = portal_visible && !mesh.shadow_only;
            let dynamic_shadow_visible = portal_visible || mesh.shadow_only;
            let handle = ModelHandle::from(mesh.model.clone());
            let current_presentation_rotation = presentation_body_rotation(
                current.rotation,
                mesh.pose_inputs.map(|inputs| inputs.heading_yaw),
            );
            let current_model_transform = glam::Mat4::from_scale_rotation_translation(
                current.scale,
                current_presentation_rotation,
                current_model_position,
            );
            let current_model_bounds = tables
                .model_bounds(&handle)
                .transformed(&current_model_transform);
            let selected_static_shadow_relevant =
                selected_static_shadow_light_reaches_bounds(world, &current_model_bounds);
            if !dynamic_shadow_visible && !selected_static_shadow_relevant {
                continue;
            }
            // Draw at the interpolated transform (smooth between ticks). Fall
            // back to the current transform if the interpolated read fails (a
            // stale id is not expected mid-iteration, but never fail the frame).
            let transform = registry
                .interpolated_transform(id, alpha)
                .unwrap_or(*current);

            let seed = id.to_raw();
            let (sample, capture) =
                resolve_sample(mesh.animation.as_ref(), &handle, tables, anim_time, seed);

            // Time-slicing decision. Distance from the CURRENT-TICK rendered
            // model origin (the same stable per-tick value the cull used). For an
            // ANIMATED entity a state change this frame (entered-stamp
            // fingerprint moved vs. last frame) OR an active crossfade forces a
            // resample so the transition is never frozen. A STATELESS entity has
            // no state to change — it follows pure stride bucketing and is never
            // tracked, keeping `last_state` bounded by the animated-entity count.
            let state_changed = match state_fingerprint(mesh.animation.as_ref()) {
                Some(fingerprint) => {
                    let changed = self.last_state.get(&seed) != Some(&fingerprint);
                    self.last_state_scratch.insert(seed, fingerprint);
                    changed
                }
                None => false,
            };
            let holder_transform = glam::Mat4::from_scale_rotation_translation(
                transform.scale,
                presentation_body_rotation(
                    transform.rotation,
                    mesh.pose_inputs.map(|inputs| inputs.heading_yaw),
                ),
                transform.position + mesh.origin_offset,
            );
            let holder_index = self.instances.len();
            self.instances.push(MeshInstanceInput {
                model: handle.clone(),
                transform: holder_transform,
                shadow_bias_scale: mesh.shadow_bias_scale,
                phase_seed: seed,
                palette_cache_key: MeshPaletteCacheKey::Entity(seed),
                sample,
                pose_inputs: mesh.pose_inputs,
                capture,
                // Attachments are emitted next and report whether this holder
                // has a skinned socket. Fill the final decision afterward so
                // the holder remains immediately before its attachments.
                resample: false,
                forward_visible,
                dynamic_shadow_visible,
                is_viewmodel: false,
            });
            let has_skinned_attachment = attachments::emit_for_holder(
                &mut self.instances,
                &mut self.attachment_world_pose,
                &mut self.attachment_model_handles,
                socket_poses,
                &handle,
                holder_transform,
                sample,
                mesh.pose_inputs,
                mesh.shadow_bias_scale,
                seed,
                forward_visible,
                dynamic_shadow_visible,
                &mesh.attachments,
            );
            let force = state_changed
                || sample.fade.is_some()
                || capture.is_some()
                || has_skinned_attachment
                || force_resample_model(&handle);
            let distance = current_model_position.distance(camera_pos);
            let resample = should_resample(distance, frame_index, seed, force);
            if resample {
                self.resample_count += 1;
            }
            self.instances[holder_index].resample = resample;
        }

        // Swap the rebuilt map in (the old one becomes next frame's scratch) and
        // advance the frame phase. `wrapping_add` so the modulo phase keeps going
        // past `u64::MAX` without a panic.
        std::mem::swap(&mut self.last_state, &mut self.last_state_scratch);
        self.frame_index = self.frame_index.wrapping_add(1);
    }

    /// The per-instance draw inputs to plan this frame (cull already applied).
    pub(crate) fn instances(&self) -> &[MeshInstanceInput] {
        &self.instances
    }

    /// Append the local weapon's world-space presentation after world collection.
    /// The caller owns camera/view-feel composition, camera-to-world conversion,
    /// and descriptor lookup; this collector only carries the finished transform
    /// across the renderer boundary.
    /// A viewmodel deliberately has no attachments, no world cull, and no shadow
    /// relevance: planner partitioning keeps it out of every depth plan.
    pub(crate) fn collect_viewmodel(
        &mut self,
        model: &str,
        transform: glam::Mat4,
        weapon_seed: u32,
    ) {
        let handle = if let Some(handle) = self.viewmodel_model_handles.get(model) {
            handle.clone()
        } else {
            let handle = ModelHandle::from(model);
            self.viewmodel_model_handles
                .insert(model.to_owned(), handle.clone());
            handle
        };
        self.instances.push(MeshInstanceInput {
            model: handle,
            transform,
            shadow_bias_scale: 1.0,
            phase_seed: weapon_seed,
            // A connected client may use its pawn id as the stable seed because it
            // intentionally has no weapon entity. Keep the viewmodel in a distinct
            // cache-key namespace so it cannot alias the body palette.
            palette_cache_key: MeshPaletteCacheKey::Attachment {
                holder: weapon_seed,
                attachment_index: usize::MAX,
            },
            sample: MeshSampleParams::rigid(),
            pose_inputs: None,
            capture: None,
            resample: false,
            forward_visible: true,
            dynamic_shadow_visible: false,
            is_viewmodel: true,
        });
    }

    /// Count of instances that resampled their pose this frame. The
    /// game-side acceptance metric: near instances tally every frame, far ones at
    /// the bucket stride, and a state-changing / crossfading distant instance is
    /// counted on the frame it transitions. Reset at the top of each collection.
    ///
    /// The metric's only in-engine consumer today is the time-slicing acceptance
    /// test; `allow(dead_code)` off the test build until a diagnostics overlay
    /// surfaces it (the `state_elapsed` precedent).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn resample_count(&self) -> u32 {
        self.resample_count
    }
}

/// Pose split inputs are world-space: render the model root at lower-body heading,
/// then let the modifier rotate the upper body by `aim_yaw - heading_yaw`. Gameplay
/// and replicated transforms retain aim yaw; only mesh presentation receives this
/// travel-facing root override.
fn presentation_body_rotation(
    transform_rotation: glam::Quat,
    heading_yaw: Option<f32>,
) -> glam::Quat {
    heading_yaw
        .filter(|yaw| yaw.is_finite())
        .map(glam::Quat::from_rotation_y)
        .unwrap_or(transform_rotation)
}

fn selected_static_shadow_light_reaches_bounds(world: &LevelWorld, bounds: &Aabb) -> bool {
    world.entity_shadow_lights.iter().any(|&light_index| {
        let influence = world
            .light_influences
            .get(light_index as usize)
            .cloned()
            .or_else(|| influence_from_light(world.lights.get(light_index as usize)));
        let Some(influence) = influence else {
            return false;
        };
        let closest = influence.center.clamp(bounds.min, bounds.max);
        closest.distance_squared(influence.center) <= influence.radius.max(0.0).powi(2)
    })
}

fn influence_from_light(
    light: Option<&postretro_level_loader::MapLight>,
) -> Option<LightInfluence> {
    let light = light?;
    Some(LightInfluence {
        center: glam::Vec3::new(
            light.origin[0] as f32,
            light.origin[1] as f32,
            light.origin[2] as f32,
        ),
        radius: light.falloff_range,
    })
}

/// The state fingerprint for an animated entity: its current entered-state stamp
/// bits (a pending stamp reads `0`), or `None` for a STATELESS entity (no
/// animation block — nothing to change, so it is never tracked and never forces a
/// resample). A change between frames means the entity (re)entered a state — the
/// signal that forces a resample. The current state name does not need hashing
/// in: a switch always moves the entered stamp (the resolve pass restamps on
/// entry), so the stamp bits alone capture a (re)entry.
fn state_fingerprint(
    animation: Option<&postretro_entities::components::mesh::MeshAnimation>,
) -> Option<u64> {
    animation.map(|anim| anim.entered_at.map(|t| t.to_bits()).unwrap_or(0))
}

/// Resolve one entity's sample params + optional capture instruction.
///
/// Stateless (`animation == None`) entities explicitly select the authored rest
/// pose. They never sample clip zero implicitly.
///
/// Animated, with a clip table: delegate to [`mesh_anim::animate_entity`], which
/// computes clip-local times, the crossfade weight, the snapshot fade, and the
/// `"smooth"`-interrupt capture instruction. If the current state is unresolved
/// (no usable clip) the entity falls back to the legacy clip-zero sample so it
/// still renders rather than vanishing.
fn resolve_sample(
    animation: Option<&postretro_entities::components::mesh::MeshAnimation>,
    handle: &ModelHandle,
    tables: &MeshClipTables,
    anim_time: f64,
    seed: u32,
) -> (
    MeshSampleParams,
    Option<postretro_model::sample_params::CaptureInstruction>,
) {
    if animation.is_none() {
        return (MeshSampleParams::rest(), None);
    }

    let table = tables.get(handle);

    // Animated entity with a resolved clip table → state-driven sampling.
    if let (Some(anim), Some(table)) = (animation, table) {
        // Per-instance phase from the CURRENT state's clip duration so a looping
        // wave de-syncs; one-shot states ignore it inside `animate_entity`.
        let phase = current_state_phase(anim, table, seed);
        if let Some(result) = mesh_anim::animate_entity(anim, anim_time, phase) {
            let mut capture = result.capture;
            if let Some(c) = capture.as_mut() {
                c.seed = seed; // key the snapshot store on the entity id
            }
            return (result.sample, capture);
        }
    }

    // Animated but unresolved / un-uploaded: legacy fallback. The primary clip
    // is index 0; phase folds in against its duration (0 if uncached).
    let duration = table.and_then(|t| t.duration(0)).unwrap_or(0.0);
    let phase = postretro_model::sample_params::instance_phase(seed, duration);
    (MeshSampleParams::stateless(anim_time as f32 + phase), None)
}

/// The per-instance phase offset for an entity's current animation state,
/// derived from its clip duration (looping de-sync). A state with no resolved
/// clip yields phase 0.
fn current_state_phase(
    anim: &postretro_entities::components::mesh::MeshAnimation,
    table: &super::mesh_anim::ModelClipTable,
    seed: u32,
) -> f32 {
    let duration = anim
        .states
        .get(&anim.current_state)
        .and_then(|s| s.clip_index)
        .and_then(|i| table.duration(i))
        .unwrap_or(0.0);
    postretro_model::sample_params::instance_phase(seed, duration)
}

impl Default for MeshRenderCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use postretro_entities::components::mesh::MeshComponent;
    use postretro_entities::registry::EntityRegistry;
    use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
    use postretro_level_loader::{CellData, CellLocatorChild, CellLocatorNodeData, LevelWorld};
    use postretro_level_loader::{FalloffModel, LightType, MapLight, ShadowType};
    use postretro_render_data::influence::LightInfluence;

    fn spawn_mesh(registry: &mut EntityRegistry, model: &str, position: Vec3) {
        let id = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        registry
            .set_component(id, MeshComponent::stateless(model.into()))
            .unwrap();
    }

    // The collector reuses the SAME pure cull the renderer pass documents
    // (`mesh_pass::mesh_visible`); membership behavior is covered by `mesh_pass`'s
    // own cull tests against a synthetic visible-set. Here we verify the
    // collector's emit + transform composition against a minimal single-cell
    // world (cell 0 spans all space, so any position lands in cell 0).

    fn single_cell_world() -> LevelWorld {
        LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            cells: vec![CellData {
                bounds_min: Vec3::splat(-1.0e6),
                bounds_max: Vec3::splat(1.0e6),
                face_start: 0,
                face_count: 0,
                portal_ref_start: 0,
                portal_ref_count: 0,
                is_solid: false,
                is_exterior: false,
                is_drawable: false,
            }],
            cell_portal_refs: vec![],
            cell_locator_root: postretro_level_loader::CellLocatorChild::Cell(0),
            cell_locator_nodes: vec![],
            portals: vec![],
            has_portals: false,
            cell_visibility: None,
            texture_names: vec![],
            texture_cache_keys: TextureCacheKeysSection { keys: vec![] },
            bvh: postretro_render_data::geometry::BvhTree {
                nodes: vec![],
                leaves: vec![],
                root_node_index: 0,
            },
            lights: vec![],
            light_influences: vec![],
            sh_volume: None,
            lightmap: None,
            lightmap_mode: postretro_level_loader::LightmapMode::Shadowed,
            sdf_atlas: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
            delta_sh_volumes: None,
            direct_sh_volume: None,
            direct_sh_delta_volumes: None,
            animated_direct_sh_delta_volumes: None,
            billboard_direct_scatter_volume: None,
            animated_billboard_direct_scatter_delta_volumes: None,
            entity_shadow_lights: vec![],
            shadowmask_atlas: None,
            data_script: None,
            map_entities: Vec::new(),
            kinematic_geometry: postretro_level_loader::KinematicGeometry::default(),
            trigger_volumes: Vec::new(),
            fog_volumes: Vec::new(),
            fog_pixel_scale: 4,
            initial_gravity: -9.81,
            fog_cell_masks: None,
            navmesh: None,
            cell_draw_index: None,
        }
    }

    fn two_cell_world_split_at_x_zero() -> LevelWorld {
        let mut world = single_cell_world();
        world.cells = vec![
            CellData {
                bounds_min: Vec3::new(0.0, -100.0, -100.0),
                bounds_max: Vec3::new(100.0, 100.0, 100.0),
                face_start: 0,
                face_count: 0,
                portal_ref_start: 0,
                portal_ref_count: 0,
                is_solid: false,
                is_exterior: false,
                is_drawable: false,
            },
            CellData {
                bounds_min: Vec3::new(-100.0, -100.0, -100.0),
                bounds_max: Vec3::new(0.0, 100.0, 100.0),
                face_start: 0,
                face_count: 0,
                portal_ref_start: 0,
                portal_ref_count: 0,
                is_solid: false,
                is_exterior: false,
                is_drawable: false,
            },
        ];
        world.cell_locator_root = CellLocatorChild::Node(0);
        world.cell_locator_nodes = vec![CellLocatorNodeData {
            plane_normal: Vec3::X,
            plane_distance: 0.0,
            front: CellLocatorChild::Cell(0),
            back: CellLocatorChild::Cell(1),
        }];
        world
    }

    fn single_cell_world_with_covering_shadow_light() -> LevelWorld {
        let mut world = single_cell_world();
        world.lights = vec![MapLight {
            origin: [0.0, 0.0, 0.0],
            light_type: LightType::Spot,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 20.0,
            cone_angle_inner: 0.3,
            cone_angle_outer: 0.6,
            cone_direction: [0.0, 0.0, -1.0],
            is_dynamic: true,
            casts_entity_shadows: true,
            animated_slot: None,
            tags: vec![],
            cell_index: 0,
            shadow_type: ShadowType::StaticLightMap,
        }];
        world.light_influences = vec![LightInfluence {
            center: Vec3::ZERO,
            radius: 20.0,
        }];
        world
    }

    fn single_cell_world_with_selected_static_shadow_light() -> LevelWorld {
        let mut world = single_cell_world_with_covering_shadow_light();
        world.lights[0].is_dynamic = false;
        world.entity_shadow_lights = vec![0];
        world
    }

    #[test]
    fn collect_emits_one_visible_mesh_instance() {
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_cell_world();
        spawn_mesh(&mut registry, "decraniated", Vec3::new(1.0, 2.0, 3.0));

        // Cell 0 is the only visible cell; the mesh lands in it → draws.
        collector.collect(
            &registry,
            &world,
            &VisibleCells::Culled(vec![0]),
            1.0,
            0.0,
            &MeshClipTables::new(),
            glam::Vec3::ZERO,
        );
        assert_eq!(collector.instances().len(), 1);
        // Translation column carries the entity position; handle preserved.
        let inst = &collector.instances()[0];
        assert_eq!(inst.model.as_str(), "decraniated");
        assert!(
            inst.forward_visible,
            "the default mesh component remains visible to the forward pass",
        );
        let t = inst.transform.w_axis;
        assert_eq!([t.x, t.y, t.z], [1.0, 2.0, 3.0]);
    }

    #[test]
    fn collect_viewmodel_appends_only_a_world_space_viewmodel_instance() {
        let mut collector = MeshRenderCollector::new();
        let transform = glam::Mat4::from_translation(Vec3::new(0.3, -0.2, -0.6));

        collector.collect_viewmodel("models/pistol/view.gltf", transform, 42);

        let instances = collector.instances();
        assert_eq!(instances.len(), 1);
        assert!(instances[0].is_viewmodel);
        assert!(instances[0].forward_visible);
        assert!(!instances[0].dynamic_shadow_visible);
        assert_eq!(instances[0].model.as_str(), "models/pistol/view.gltf");
        assert_eq!(instances[0].transform, transform);
        assert_eq!(instances[0].phase_seed, 42);
        assert_eq!(
            instances[0].palette_cache_key,
            MeshPaletteCacheKey::Attachment {
                holder: 42,
                attachment_index: usize::MAX,
            }
        );
        assert!(instances[0].pose_inputs.is_none());
    }

    // Regression: default-only fixtures let dropped collector/planner copies preserve 1.0.
    #[test]
    fn collector_and_planner_carry_mesh_shadow_bias_scale_verbatim() {
        struct OneJointModel;

        impl postretro_render_cpu::mesh_instances::JointCounts for OneJointModel {
            fn joint_count(&self, _model: &postretro_model::ModelHandle) -> Option<u32> {
                Some(1)
            }

            fn model_bounds(
                &self,
                _model: &postretro_model::ModelHandle,
            ) -> postretro_render_data::cone_frustum::Aabb {
                Default::default()
            }
        }

        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                MeshComponent::stateless("decraniated".into()).with_shadow_bias_scale(2.5),
            )
            .unwrap();

        let mut collector = MeshRenderCollector::new();
        collector.collect(
            &registry,
            &single_cell_world(),
            &VisibleCells::DrawAll,
            1.0,
            0.0,
            &MeshClipTables::new(),
            Vec3::ZERO,
        );
        let plan = postretro_render_cpu::mesh_instances::plan_mesh_frame(
            collector.instances(),
            &OneJointModel,
        );
        let planned = &plan.groups[0].instances[0];

        assert!(
            (planned.shadow_bias_scale - 2.5).abs() < f32::EPSILON,
            "authored mesh shadow bias must survive collector and planner copies"
        );
    }

    #[test]
    fn collect_stateless_mesh_renders_at_transform_without_feet_offset() {
        // A non-agent mesh (e.g. a static prop) is authored feet-at-origin and
        // must render with its origin exactly at `Transform.position` — no
        // capsule-center correction is applied to entities with no agent.
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_cell_world();
        spawn_mesh(&mut registry, "prop", Vec3::new(2.0, 4.0, 6.0));

        collector.collect(
            &registry,
            &world,
            &VisibleCells::DrawAll,
            1.0,
            0.0,
            &MeshClipTables::new(),
            glam::Vec3::ZERO,
        );
        assert_eq!(collector.instances().len(), 1);
        let t = collector.instances()[0].transform.w_axis;
        assert_eq!(
            [t.x, t.y, t.z],
            [2.0, 4.0, 6.0],
            "a non-agent mesh renders at its raw transform (feet at origin)"
        );
    }

    #[test]
    fn collect_mesh_origin_offset_drops_capsule_center_to_feet() {
        // Agent-driven and remote enemy meshes have `Transform.position` at the
        // capsule CENTER but are authored feet-at-origin. The render-facing mesh
        // offset, not local Agent state, drops the rendered origin by the
        // capsule's center-to-sole distance so host and client presentation match.
        use postretro_entities::components::mesh::capsule_center_to_feet_origin_offset;

        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_cell_world();

        // Spawn a mesh and attach an agent capsule (radius 0.35, height 1.8 →
        // half_height = 1.8/2 - 0.35 = 0.55; center-to-sole = 0.55 + 0.35 = 0.9).
        let center_y = 0.9_f32;
        let id = registry.spawn(Transform {
            position: Vec3::new(1.0, center_y, 2.0),
            ..Transform::default()
        });
        registry
            .set_component(
                id,
                MeshComponent::stateless("knight".into())
                    .with_origin_offset(capsule_center_to_feet_origin_offset(0.35, 1.8)),
            )
            .unwrap();

        collector.collect(
            &registry,
            &world,
            &VisibleCells::DrawAll,
            1.0,
            0.0,
            &MeshClipTables::new(),
            glam::Vec3::ZERO,
        );
        assert_eq!(collector.instances().len(), 1);
        let t = collector.instances()[0].transform.w_axis;
        // XZ unchanged; Y dropped by half_height + radius = 0.9 → feet at y≈0.
        assert!((t.x - 1.0).abs() < 1.0e-6);
        assert!((t.z - 2.0).abs() < 1.0e-6);
        assert!(
            (t.y - (center_y - 0.9)).abs() < 1.0e-5,
            "agent mesh feet should sit at capsule center minus (half_height + radius); got y={}",
            t.y
        );
    }

    #[test]
    fn collect_visibility_uses_current_model_position_with_origin_offset() {
        // Regression: visibility classified the un-offset gameplay transform, so
        // a mesh whose rendered model origin crossed into another cell could be
        // culled before the forward/shadow plan saw it.
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = two_cell_world_split_at_x_zero();
        let id = registry.spawn(Transform {
            position: Vec3::new(-1.0, 0.0, 0.0),
            ..Transform::default()
        });
        registry
            .set_component(
                id,
                MeshComponent::stateless("offset-prop".into())
                    .with_origin_offset(Vec3::new(2.0, 0.0, 0.0)),
            )
            .unwrap();

        collector.collect(
            &registry,
            &world,
            &VisibleCells::Culled(vec![0]),
            1.0,
            0.0,
            &MeshClipTables::new(),
            glam::Vec3::ZERO,
        );
        assert_eq!(
            collector.instances().len(),
            1,
            "transform position is in hidden cell 1, but rendered model origin is in visible cell 0",
        );
        let t = collector.instances()[0].transform.w_axis;
        assert_eq!([t.x, t.y, t.z], [1.0, 0.0, 0.0]);
    }

    #[test]
    fn collect_emits_two_instances_of_same_model_at_distinct_transforms() {
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_cell_world();
        spawn_mesh(&mut registry, "decraniated", Vec3::new(1.0, 0.0, 0.0));
        spawn_mesh(&mut registry, "decraniated", Vec3::new(5.0, 0.0, 0.0));

        collector.collect(
            &registry,
            &world,
            &VisibleCells::DrawAll,
            1.0,
            0.0,
            &MeshClipTables::new(),
            glam::Vec3::ZERO,
        );
        assert_eq!(collector.instances().len(), 2);
        let xs: Vec<f32> = collector
            .instances()
            .iter()
            .map(|i| i.transform.w_axis.x)
            .collect();
        assert!(
            xs.contains(&1.0) && xs.contains(&5.0),
            "distinct transforms: {xs:?}"
        );
        // Same model handle on both.
        assert!(
            collector
                .instances()
                .iter()
                .all(|i| i.model.as_str() == "decraniated")
        );
    }

    #[test]
    fn collect_emits_distinct_models_with_their_handles() {
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_cell_world();
        spawn_mesh(&mut registry, "grunt", Vec3::new(1.0, 0.0, 0.0));
        spawn_mesh(&mut registry, "drone", Vec3::new(2.0, 0.0, 0.0));

        collector.collect(
            &registry,
            &world,
            &VisibleCells::DrawAll,
            1.0,
            0.0,
            &MeshClipTables::new(),
            glam::Vec3::ZERO,
        );
        assert_eq!(collector.instances().len(), 2);
        let handles: Vec<&str> = collector
            .instances()
            .iter()
            .map(|i| i.model.as_str())
            .collect();
        assert!(handles.contains(&"grunt") && handles.contains(&"drone"));
    }

    #[test]
    fn collect_excludes_mesh_in_nonvisible_cell() {
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_cell_world();
        spawn_mesh(&mut registry, "decraniated", Vec3::new(1.0, 2.0, 3.0));

        // The mesh lands in cell 0, but only cell 1 is visible → culled out.
        collector.collect(
            &registry,
            &world,
            &VisibleCells::Culled(vec![1]),
            1.0,
            0.0,
            &MeshClipTables::new(),
            glam::Vec3::ZERO,
        );
        assert!(collector.instances().is_empty());
    }

    #[test]
    fn collect_uses_interpolated_transform_at_alpha() {
        // The mesh's current position is (10,0,0); previous-tick is (0,0,0) (the
        // spawn seed). At alpha 0.5 the collector must emit the midpoint (5,0,0)
        // — proving it reads the interpolated transform, not current or spawn.
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_cell_world();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(id, MeshComponent::stateless("m".into()))
            .unwrap();
        // Snapshot freezes the spawn (origin) as previous-tick, then move
        // current to (10,0,0).
        registry.snapshot_transforms();
        registry
            .set_component(
                id,
                Transform {
                    position: Vec3::new(10.0, 0.0, 0.0),
                    ..Transform::default()
                },
            )
            .unwrap();

        collector.collect(
            &registry,
            &world,
            &VisibleCells::DrawAll,
            0.5,
            0.0,
            &MeshClipTables::new(),
            glam::Vec3::ZERO,
        );
        assert_eq!(collector.instances().len(), 1);
        let t = collector.instances()[0].transform.w_axis;
        assert!(
            (t.x - 5.0).abs() < 1.0e-4,
            "interpolated x at alpha 0.5 is 5.0, got {}",
            t.x
        );
    }

    #[test]
    fn collect_clears_between_frames_without_dropping_capacity() {
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_cell_world();
        spawn_mesh(&mut registry, "decraniated", Vec3::ZERO);
        collector.collect(
            &registry,
            &world,
            &VisibleCells::DrawAll,
            1.0,
            0.0,
            &MeshClipTables::new(),
            glam::Vec3::ZERO,
        );
        let cap_after_first = collector.instances.capacity();
        assert!(cap_after_first >= 1);

        let ids: Vec<_> = registry
            .iter_with_kind(ComponentKind::Mesh)
            .map(|(id, _)| id)
            .collect();
        for id in ids {
            registry.despawn(id).unwrap();
        }
        collector.collect(
            &registry,
            &world,
            &VisibleCells::DrawAll,
            1.0,
            0.0,
            &MeshClipTables::new(),
            glam::Vec3::ZERO,
        );
        assert!(collector.instances().is_empty());
        assert_eq!(collector.instances.capacity(), cap_after_first);
    }

    // --- Animated-state sample-param resolution through `collect` ---------------

    use postretro_entities::components::mesh::{
        AnimationState, AttachmentBinding, InterruptPolicy, MeshAnimation, MeshAttachment,
    };
    use postretro_entities::components::mesh::{
        resolve_pending_animation_stamps, switch_animation_state,
    };
    use postretro_model::ModelHandle;
    use postretro_model::gltf_loader::JointZone;
    use postretro_model::sample_params::FadeSource;
    use postretro_model::skeleton::{Joint, RestLocal, Skeleton};
    use postretro_render_cpu::mesh_pass::ClipMetadata;
    use postretro_render_data::cone_frustum::Aabb;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn clip_meta(pairs: &[(&str, f32)]) -> Vec<ClipMetadata> {
        pairs
            .iter()
            .map(|(name, duration)| ClipMetadata {
                name: (*name).to_string(),
                duration: *duration,
            })
            .collect()
    }

    fn state(clip: &str, looping: bool, crossfade_ms: f32, idx: Option<usize>) -> AnimationState {
        AnimationState {
            clip: clip.into(),
            looping,
            crossfade_ms,
            interrupt: InterruptPolicy::Smooth,
            travel_speed: None,
            clip_index: idx,
        }
    }

    /// Tables for a model "grunt" with idle (idx 0, 2s) + walk (idx 1, 2s).
    fn grunt_tables() -> MeshClipTables {
        let mut t = MeshClipTables::new();
        t.insert_with_bounds(
            ModelHandle::from("grunt"),
            &clip_meta(&[("idle", 2.0), ("walk", 2.0)]),
            Aabb::default(),
        );
        t
    }

    fn spawn_animated(
        reg: &mut EntityRegistry,
        pos: Vec3,
    ) -> postretro_entities::registry::EntityId {
        // Both states carry a nonzero crossfade so a switch starts a fade — needed
        // to exercise the smooth-interrupt capture path.
        let mut states = HashMap::new();
        states.insert("idle".into(), state("idle", true, 100.0, Some(0)));
        states.insert("walk".into(), state("walk", true, 200.0, Some(1)));
        let id = reg.spawn(Transform {
            position: pos,
            ..Transform::default()
        });
        reg.set_component(
            id,
            MeshComponent {
                model: "grunt".into(),
                animation: Some(MeshAnimation::new(states, "idle".into())),
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

    #[test]
    fn collect_stateless_selects_rest_pose_even_when_clip_zero_exists() {
        // Regression: stateless bodies sampled clip zero while their sockets
        // sampled rest pose, visibly detaching mounted props.
        let mut reg = EntityRegistry::new();
        let world = single_cell_world();
        let mut collector = MeshRenderCollector::new();
        let mut tables = MeshClipTables::new();
        tables.insert_with_bounds(
            ModelHandle::from("prop"),
            &clip_meta(&[("spin", 4.0)]),
            Aabb::default(),
        );
        spawn_mesh(&mut reg, "prop", Vec3::new(1.0, 0.0, 0.0));

        collector.collect(
            &reg,
            &world,
            &VisibleCells::DrawAll,
            1.0,
            3.0,
            &tables,
            glam::Vec3::ZERO,
        );
        let insts = collector.instances();
        assert_eq!(insts.len(), 1);
        assert_eq!(insts[0].sample, MeshSampleParams::rest());
        assert!(insts[0].capture.is_none());
    }

    #[test]
    fn collect_animated_plays_default_state_then_switched_state() {
        let mut reg = EntityRegistry::new();
        let world = single_cell_world();
        let mut collector = MeshRenderCollector::new();
        let tables = grunt_tables();
        let id = spawn_animated(&mut reg, Vec3::ZERO);
        resolve_pending_animation_stamps(&mut reg, 0.0);

        // Default state idle (clip 0) at spawn.
        collector.collect(
            &reg,
            &world,
            &VisibleCells::DrawAll,
            1.0,
            0.5,
            &tables,
            glam::Vec3::ZERO,
        );
        assert_eq!(
            collector.instances()[0].sample.primary.clip_index,
            0,
            "plays default idle"
        );

        // Switch to walk; the new state's clip (1) drives the primary leg.
        switch_animation_state(&mut reg, id, "walk");
        resolve_pending_animation_stamps(&mut reg, 1.0);
        collector.collect(
            &reg,
            &world,
            &VisibleCells::DrawAll,
            1.0,
            5.0,
            &tables,
            glam::Vec3::ZERO,
        );
        assert_eq!(
            collector.instances()[0].sample.primary.clip_index,
            1,
            "setAnimationState switch plays the new state's clip",
        );
    }

    #[test]
    fn collector_carries_only_entity_pose_inputs() {
        let mut reg = EntityRegistry::new();
        let world = single_cell_world();
        let mut collector = MeshRenderCollector::new();
        let id = spawn_animated(&mut reg, Vec3::ZERO);
        let expected = postretro_entities::PoseInputs {
            aim_pitch: 0.2,
            aim_yaw: 0.7,
            heading_yaw: 0.4,
            ..Default::default()
        };
        let mut mesh = reg.get_component::<MeshComponent>(id).unwrap().clone();
        mesh.pose_inputs = Some(expected);
        reg.set_component(id, mesh).unwrap();

        collector.collect(
            &reg,
            &world,
            &VisibleCells::DrawAll,
            1.0,
            0.0,
            &grunt_tables(),
            Vec3::ZERO,
        );

        assert_eq!(collector.instances()[0].pose_inputs, Some(expected));
        let rendered_forward = collector.instances()[0]
            .transform
            .transform_vector3(-Vec3::Z)
            .normalize();
        let expected_forward = glam::Quat::from_rotation_y(expected.heading_yaw) * -Vec3::Z;
        assert!(
            rendered_forward.distance(expected_forward) <= 1.0e-5,
            "the mesh root follows lower-body travel heading while aim yaw remains in pose inputs"
        );
    }

    #[test]
    fn collect_smooth_interrupt_emits_capture_keyed_by_seed() {
        // idle→walk starts a fade (walk fades in over 200ms). Interrupting that
        // fade with walk→idle (default = smooth) records a snapshot fade source;
        // the collector then emits a capture instruction keyed by the entity seed
        // and a snapshot fade leg, INSIDE the new idle fade window.
        let mut reg = EntityRegistry::new();
        let world = single_cell_world();
        let mut collector = MeshRenderCollector::new();
        let tables = grunt_tables();
        let id = spawn_animated(&mut reg, Vec3::ZERO);
        resolve_pending_animation_stamps(&mut reg, 0.0);

        // idle→walk: walk begins fading in from idle.
        switch_animation_state(&mut reg, id, "walk");
        resolve_pending_animation_stamps(&mut reg, 1.0);
        // Interrupt the walk fade with walk→idle (smooth). The entered idle has a
        // 100ms crossfade, so a fade window is open and the source is a snapshot.
        switch_animation_state(&mut reg, id, "idle");
        resolve_pending_animation_stamps(&mut reg, 1.02);

        // Collect 0.02s into idle's 100ms fade — capture due this frame.
        collector.collect(
            &reg,
            &world,
            &VisibleCells::DrawAll,
            1.0,
            1.04,
            &tables,
            glam::Vec3::ZERO,
        );
        let inst = &collector.instances()[0];
        let capture = inst
            .capture
            .as_ref()
            .expect("smooth interrupt emits a capture instruction");
        assert_eq!(capture.seed, id.to_raw(), "capture keyed by entity seed");
        assert!(
            matches!(
                inst.sample.fade.map(|f| f.from),
                Some(FadeSource::Snapshot { .. })
            ),
            "the interrupted fade blends from a snapshot source",
        );
        assert_eq!(
            inst.sample.primary.clip_index, 0,
            "primary is the entered idle"
        );
    }

    // --- Animation time-slicing -------------------------------------------------

    /// A camera position far enough that an instance at the origin lands past
    /// `RESAMPLE_FAR_DISTANCE` (the stride-4 far bucket). Placed along +X.
    fn far_camera() -> Vec3 {
        Vec3::new(RESAMPLE_FAR_DISTANCE + 10.0, 0.0, 0.0)
    }

    fn hit_zone_store_for_model(model: &str) -> HitZoneStore {
        let mut store = HitZoneStore::new();
        store.insert_for_test(
            ModelHandle::from(model),
            crate::scripting_systems::hit_zones::ModelHitZones {
                skeleton: Arc::new(Skeleton {
                    joints: vec![Joint {
                        parent: None,
                        inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
                        rest_local: RestLocal::default(),
                    }],
                }),
                clips: Arc::new(Vec::new()),
                joint_zones: vec![Some(JointZone {
                    tag: "core".to_string(),
                    radius: Some(0.25),
                })],
                sockets: std::collections::HashMap::new(),
                derived_bound: Some(Aabb::default()),
                legs: Vec::new(),
                pose_stack: Arc::new(postretro_model::pose_modifier::PoseModifierStack::default()),
            },
        );
        store
    }

    fn attachment(binding: AttachmentBinding, model: &str) -> MeshAttachment {
        MeshAttachment {
            socket: "socket".to_string(),
            model: model.to_string(),
            binding,
        }
    }

    fn socket_pose_store(
        model: &str,
        pose_stack: postretro_model::pose_modifier::PoseModifierStack,
    ) -> HitZoneStore {
        use postretro_model::skeleton::{AnimationClip, Interp, JointTracks, Track};

        let skeleton = Skeleton {
            joints: vec![
                Joint {
                    parent: None,
                    inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
                    rest_local: RestLocal::default(),
                },
                Joint {
                    parent: Some(0),
                    inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
                    rest_local: RestLocal {
                        translation: Vec3::new(0.0, 2.0, 0.0),
                        ..RestLocal::default()
                    },
                },
            ],
        };
        let clip = AnimationClip {
            name: "idle".to_string(),
            duration: 0.0,
            joints: vec![
                JointTracks {
                    translation: Track::new(
                        vec![0.0],
                        vec![Vec3::new(3.0, 0.0, 0.0)],
                        Interp::Linear,
                    )
                    .expect("one finite key builds"),
                    ..JointTracks::default()
                },
                JointTracks::default(),
            ],
            travel_speed: None,
        };
        let mut store = HitZoneStore::new();
        store.insert_for_test(
            ModelHandle::from(model),
            crate::scripting_systems::hit_zones::ModelHitZones {
                skeleton: Arc::new(skeleton),
                clips: Arc::new(vec![clip]),
                joint_zones: vec![None, None],
                sockets: std::collections::HashMap::new(),
                derived_bound: None,
                legs: Vec::new(),
                pose_stack: Arc::new(pose_stack),
            },
        );
        store
    }

    fn assert_mat4_approx(actual: glam::Mat4, expected: glam::Mat4, context: &str) {
        for (index, (actual, expected)) in actual
            .to_cols_array()
            .into_iter()
            .zip(expected.to_cols_array())
            .enumerate()
        {
            assert!(
                (actual - expected).abs() < 1.0e-5,
                "{context}: matrix element {index}: expected {expected}, got {actual}",
            );
        }
    }

    #[test]
    fn resample_stride_buckets_by_distance() {
        // The pure bucketing function: near → stride 1, mid → 2, far → 4.
        assert_eq!(resample_stride(0.0), RESAMPLE_STRIDE_NEAR);
        assert_eq!(
            resample_stride(RESAMPLE_NEAR_DISTANCE),
            RESAMPLE_STRIDE_NEAR
        );
        assert_eq!(
            resample_stride(RESAMPLE_NEAR_DISTANCE + 0.1),
            RESAMPLE_STRIDE_MID
        );
        assert_eq!(resample_stride(RESAMPLE_FAR_DISTANCE), RESAMPLE_STRIDE_MID);
        assert_eq!(
            resample_stride(RESAMPLE_FAR_DISTANCE + 0.1),
            RESAMPLE_STRIDE_FAR
        );
    }

    #[test]
    fn near_instance_resamples_every_frame() {
        // A stateless instance at the origin with the camera on top of it (near
        // bucket, stride 1) must resample on every single collect — the resample
        // count equals the instance count each frame.
        let mut reg = EntityRegistry::new();
        let world = single_cell_world();
        let mut collector = MeshRenderCollector::new();
        spawn_mesh(&mut reg, "decraniated", Vec3::ZERO);

        for _ in 0..8 {
            collector.collect(
                &reg,
                &world,
                &VisibleCells::DrawAll,
                1.0,
                0.0,
                &MeshClipTables::new(),
                Vec3::ZERO,
            );
            assert_eq!(
                collector.resample_count(),
                1,
                "a near instance resamples every frame",
            );
            assert!(
                collector.instances()[0].resample,
                "near instance carries resample = true",
            );
        }
    }

    #[test]
    fn far_instance_resamples_at_reduced_rate() {
        // A stateless instance at the origin with the camera in the far bucket
        // (stride 4) must resample only every 4th frame, NOT every frame — the
        // acceptance metric: the per-frame resample count drops accordingly. Over
        // a window of 4N frames the far instance resamples exactly N times, while
        // a near instance would have resampled 4N times.
        let mut reg = EntityRegistry::new();
        let world = single_cell_world();
        let mut collector = MeshRenderCollector::new();
        spawn_mesh(&mut reg, "decraniated", Vec3::ZERO);

        let frames = (RESAMPLE_STRIDE_FAR * 5) as usize;
        let mut resampled = 0u32;
        for _ in 0..frames {
            collector.collect(
                &reg,
                &world,
                &VisibleCells::DrawAll,
                1.0,
                0.0,
                &MeshClipTables::new(),
                far_camera(),
            );
            resampled += collector.resample_count();
        }
        // Exactly frames / stride resamples (the modulo fires once per stride).
        assert_eq!(
            resampled,
            frames as u32 / RESAMPLE_STRIDE_FAR as u32,
            "far instance resamples at 1/stride the near rate",
        );
        // And strictly fewer than the every-frame rate (the reduction is real).
        assert!(
            resampled < frames as u32,
            "far instance resamples strictly less often than every frame",
        );
    }

    #[test]
    fn precise_hit_zone_model_forces_resample_despite_far_stride() {
        // Hit zones sample current-clock capsules. A visible model with precise
        // zones must therefore not draw a stale cached palette on skipped
        // time-slice frames.
        let mut reg = EntityRegistry::new();
        let world = single_cell_world();
        let mut collector = MeshRenderCollector::new();
        let hit_zones = hit_zone_store_for_model("decraniated");
        spawn_mesh(&mut reg, "decraniated", Vec3::ZERO);

        for _ in 0..(RESAMPLE_STRIDE_FAR * 2) {
            collector.collect_with_hit_zones(
                &reg,
                &world,
                &VisibleCells::DrawAll,
                1.0,
                0.0,
                &MeshClipTables::new(),
                far_camera(),
                &hit_zones,
            );
            assert_eq!(
                collector.resample_count(),
                1,
                "precise hit-zone models resample every visible frame",
            );
            assert!(
                collector.instances()[0].resample,
                "resample flag is forced for hit-zone-capable model",
            );
        }
    }

    #[test]
    fn pose_modified_model_forces_resample_despite_far_stride() {
        let mut reg = EntityRegistry::new();
        let world = single_cell_world();
        let mut collector = MeshRenderCollector::new();
        let mut metadata = HitZoneStore::new();
        metadata.mark_pose_modified_for_test(ModelHandle::from("decraniated"));
        spawn_mesh(&mut reg, "decraniated", Vec3::ZERO);

        for _ in 0..(RESAMPLE_STRIDE_FAR * 2) {
            collector.collect_with_hit_zones(
                &reg,
                &world,
                &VisibleCells::DrawAll,
                1.0,
                0.0,
                &MeshClipTables::new(),
                far_camera(),
                &metadata,
            );
            assert_eq!(collector.resample_count(), 1);
            assert!(collector.instances()[0].resample);
        }
    }

    #[test]
    fn far_crowd_desyncs_rather_than_resampling_in_lockstep() {
        // Two distant stateless instances with distinct seeds must not resample on
        // the SAME frames — folding the entity seed into the stride phase de-syncs
        // them, so on most frames at most one of the two resamples (the per-frame
        // count is rarely both at once). Verify the two never both skip forever and
        // that there exists a frame where their resample decisions differ.
        let mut reg = EntityRegistry::new();
        let world = single_cell_world();
        let mut collector = MeshRenderCollector::new();
        spawn_mesh(&mut reg, "decraniated", Vec3::ZERO);
        spawn_mesh(&mut reg, "decraniated", Vec3::ZERO);

        let mut saw_difference = false;
        for _ in 0..(RESAMPLE_STRIDE_FAR * 4) {
            collector.collect(
                &reg,
                &world,
                &VisibleCells::DrawAll,
                1.0,
                0.0,
                &MeshClipTables::new(),
                far_camera(),
            );
            let flags: Vec<bool> = collector.instances().iter().map(|i| i.resample).collect();
            assert_eq!(flags.len(), 2);
            if flags[0] != flags[1] {
                saw_difference = true;
            }
        }
        assert!(
            saw_difference,
            "distinct seeds de-sync: there is a frame where the two far instances disagree",
        );
    }

    #[test]
    fn distant_state_change_forces_resample() {
        // A DISTANT animated instance (far bucket, stride 4) still resamples on the
        // frame it changes state — the transition must never be frozen by the
        // time-slice. Drive it to a frame the stride would otherwise SKIP, then
        // switch state on that frame and confirm it resamples anyway.
        let mut reg = EntityRegistry::new();
        let world = single_cell_world();
        let mut collector = MeshRenderCollector::new();
        let tables = grunt_tables();
        let id = spawn_animated(&mut reg, Vec3::ZERO);
        resolve_pending_animation_stamps(&mut reg, 0.0);
        let cam = far_camera();

        // Advance frames until we reach one the far stride would skip (resample
        // false). The spawn frame forces a resample (new state fingerprint), so we
        // need to roll past the forced frames into a steady skip.
        let mut skip_frame = None;
        for f in 0..(RESAMPLE_STRIDE_FAR * 3) {
            collector.collect(&reg, &world, &VisibleCells::DrawAll, 1.0, 0.5, &tables, cam);
            if !collector.instances()[0].resample {
                skip_frame = Some(f);
                break;
            }
        }
        assert!(
            skip_frame.is_some(),
            "a far animated instance must eventually hit a skipped frame",
        );

        // Now switch state — this collect must resample despite the stride.
        switch_animation_state(&mut reg, id, "walk");
        resolve_pending_animation_stamps(&mut reg, 1.0);
        collector.collect(&reg, &world, &VisibleCells::DrawAll, 1.0, 1.0, &tables, cam);
        assert!(
            collector.instances()[0].resample,
            "a distant instance resamples on the frame its state changes",
        );
    }

    #[test]
    fn distant_active_crossfade_forces_resample() {
        // A DISTANT instance mid-crossfade resamples every frame the fade is in
        // flight (the blend weight advances each frame — a frozen pose would
        // visibly hitch). After the switch+resolve, several consecutive collects
        // inside the fade window must all resample even at the far stride.
        let mut reg = EntityRegistry::new();
        let world = single_cell_world();
        let mut collector = MeshRenderCollector::new();
        let tables = grunt_tables();
        let id = spawn_animated(&mut reg, Vec3::ZERO);
        resolve_pending_animation_stamps(&mut reg, 0.0);
        let cam = far_camera();

        // Start a fade: idle→walk (walk fades in over 200ms).
        switch_animation_state(&mut reg, id, "walk");
        resolve_pending_animation_stamps(&mut reg, 1.0);

        // Collect at several points INSIDE the 200ms fade window. Each must
        // resample because a fade is active (forced regardless of the stride).
        for anim_time in [1.0, 1.05, 1.1, 1.15] {
            collector.collect(
                &reg,
                &world,
                &VisibleCells::DrawAll,
                1.0,
                anim_time,
                &tables,
                cam,
            );
            let inst = &collector.instances()[0];
            assert!(
                inst.sample.fade.is_some(),
                "fade is active at anim_time {anim_time}",
            );
            assert!(
                inst.resample,
                "a distant instance resamples while a crossfade is in flight (t={anim_time})",
            );
        }
    }

    #[test]
    fn collect_drops_nonvisible_mesh_even_when_dynamic_shadow_light_reaches_it() {
        // Regression: light influence alone retained a mesh from a nonvisible
        // cell, so the shadow pass could draw phantom entity shadows through
        // portal-occluded space. Mesh collection is strictly cell-membership gated.
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_cell_world_with_covering_shadow_light();
        spawn_mesh(&mut registry, "decraniated", Vec3::new(1.0, 0.0, 0.0));

        collector.collect(
            &registry,
            &world,
            &VisibleCells::Culled(vec![1]),
            1.0,
            0.0,
            &MeshClipTables::new(),
            glam::Vec3::ZERO,
        );
        assert!(
            collector.instances().is_empty(),
            "a mesh in cell 0 must not be retained when only cell 1 is visible",
        );
    }

    #[test]
    fn collect_keeps_nonvisible_mesh_when_selected_static_light_reaches_it() {
        // Static-light entity shadows need a shadow-caster set wider than the
        // forward-visible set. Non-forward instances still skip the color pass via
        // `forward_visible = false`, but can cast into promoted static shadow maps.
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_cell_world_with_selected_static_shadow_light();
        spawn_mesh(&mut registry, "decraniated", Vec3::new(1.0, 0.0, 0.0));

        collector.collect(
            &registry,
            &world,
            &VisibleCells::Culled(vec![1]),
            1.0,
            0.0,
            &MeshClipTables::new(),
            glam::Vec3::ZERO,
        );

        assert_eq!(collector.instances().len(), 1);
        assert!(
            !collector.instances()[0].forward_visible,
            "selected static-light shadow casters outside the forward set must not draw forward",
        );
        assert!(
            !collector.instances()[0].dynamic_shadow_visible,
            "static relevance alone must not admit an off-PVS mesh to dynamic depth",
        );
    }

    #[test]
    fn collect_retains_selected_static_shadow_caster_by_bounds_not_origin() {
        // Regression: off-PVS selected-static casters were retained only when
        // their model origin was inside the light. The renderer culls shadow
        // casters by transformed model bounds, so collection must use the same
        // conservative sphere-vs-AABB relevance.
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let mut world = single_cell_world_with_selected_static_shadow_light();
        world.lights[0].falloff_range = 5.0;
        world.light_influences[0].radius = 5.0;
        spawn_mesh(&mut registry, "wide-prop", Vec3::new(6.0, 0.0, 0.0));

        let mut tables = MeshClipTables::new();
        tables.insert_with_bounds(
            ModelHandle::from("wide-prop"),
            &[],
            Aabb {
                min: Vec3::new(-2.0, -0.5, -0.5),
                max: Vec3::new(0.0, 0.5, 0.5),
            },
        );

        collector.collect(
            &registry,
            &world,
            &VisibleCells::Culled(vec![1]),
            1.0,
            0.0,
            &tables,
            glam::Vec3::ZERO,
        );

        assert_eq!(collector.instances().len(), 1);
        assert!(
            !collector.instances()[0].forward_visible,
            "origin is outside radius, but transformed bounds intersect the selected static light",
        );
    }

    #[test]
    fn collect_keeps_visible_mesh_for_forward_and_shadow_plan() {
        // Visible meshes still enter the shared frame plan consumed by both the
        // forward pass and entity shadow depth pass.
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_cell_world_with_covering_shadow_light();
        spawn_mesh(&mut registry, "decraniated", Vec3::new(1.0, 0.0, 0.0));

        collector.collect(
            &registry,
            &world,
            &VisibleCells::Culled(vec![0]),
            1.0,
            0.0,
            &MeshClipTables::new(),
            glam::Vec3::ZERO,
        );
        assert_eq!(collector.instances().len(), 1);
        assert!(
            collector.instances()[0].forward_visible,
            "a visible mesh remains in the shared forward/shadow plan",
        );
    }

    #[test]
    fn collect_keeps_shadow_only_mesh_planned_but_out_of_forward_draws() {
        struct OneJointModel;

        impl postretro_render_cpu::mesh_instances::JointCounts for OneJointModel {
            fn joint_count(&self, _model: &postretro_model::ModelHandle) -> Option<u32> {
                Some(1)
            }

            fn model_bounds(
                &self,
                _model: &postretro_model::ModelHandle,
            ) -> postretro_render_data::cone_frustum::Aabb {
                Default::default()
            }
        }

        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                MeshComponent::stateless("local-body".into()).with_shadow_only(true),
            )
            .unwrap();

        let mut collector = MeshRenderCollector::new();
        collector.collect(
            &registry,
            &single_cell_world(),
            &VisibleCells::DrawAll,
            1.0,
            0.0,
            &MeshClipTables::new(),
            Vec3::ZERO,
        );

        assert_eq!(collector.instances().len(), 1);
        assert!(
            !collector.instances()[0].forward_visible,
            "shadowOnly must suppress the forward draw while retaining the body",
        );
        assert!(collector.instances()[0].dynamic_shadow_visible);

        let plan = postretro_render_cpu::mesh_instances::plan_mesh_frame(
            collector.instances(),
            &OneJointModel,
        );
        assert_eq!(
            plan.instance_count, 1,
            "shadow-only body reaches the SSBO plan"
        );
        assert!(
            !plan.groups[0].instances[0].forward_visible,
            "the planned body remains excluded from the forward draw",
        );
        assert!(plan.groups[0].instances[0].dynamic_shadow_visible);
    }

    #[test]
    fn collect_keeps_off_pvs_shadow_only_mesh_for_dynamic_depth() {
        // Regression: collection dropped the local descriptor shadow-only body
        // before dynamic-light cone culling whenever its cell was outside PVS.
        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                MeshComponent::stateless("local-body".into()).with_shadow_only(true),
            )
            .unwrap();
        let mut collector = MeshRenderCollector::new();

        collector.collect(
            &registry,
            &single_cell_world(),
            &VisibleCells::Culled(vec![1]),
            1.0,
            0.0,
            &MeshClipTables::new(),
            Vec3::ZERO,
        );

        assert_eq!(collector.instances().len(), 1);
        assert!(!collector.instances()[0].forward_visible);
        assert!(
            collector.instances()[0].dynamic_shadow_visible,
            "explicit shadow-only intent survives absent PVS for per-light cone culling",
        );
    }

    #[test]
    fn collect_emits_skinned_attachment_at_modified_world_socket_pose() {
        use glam::{Quat, Vec3};
        use postretro_model::pose_modifier::{
            JointMask, ModifierEntry, PoseModifier, PoseModifierStack,
        };

        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_cell_world();
        let mut child_mask = JointMask::new();
        assert!(child_mask.insert(1));
        let store = socket_pose_store(
            "holder",
            PoseModifierStack::new(vec![ModifierEntry {
                mask: child_mask,
                modifier: PoseModifier::AimPitchBend {
                    bend_weights: vec![1.0],
                },
            }]),
        );
        let mut states = HashMap::new();
        states.insert("idle".to_string(), state("idle", true, 0.0, Some(0)));
        let id = registry.spawn(Transform {
            position: Vec3::new(4.0, -1.0, 2.0),
            rotation: Quat::from_rotation_y(0.4),
            scale: Vec3::new(2.0, 3.0, 4.0),
        });
        let mut mesh = MeshComponent::animated(
            "holder".to_string(),
            MeshAnimation::new(states, "idle".to_string()),
        );
        let inputs = postretro_entities::PoseInputs {
            aim_pitch: 0.5,
            ..Default::default()
        };
        mesh.pose_inputs = Some(inputs);
        mesh.attachments
            .push(attachment(AttachmentBinding::Skinned(1), "hand-prop"));
        registry.set_component(id, mesh).unwrap();
        resolve_pending_animation_stamps(&mut registry, 0.0);

        let mut tables = MeshClipTables::new();
        tables.insert_with_bounds(
            ModelHandle::from("holder"),
            &clip_meta(&[("idle", 0.0)]),
            Aabb::default(),
        );
        collector.collect_with_hit_zones(
            &registry,
            &world,
            &VisibleCells::DrawAll,
            1.0,
            0.0,
            &tables,
            Vec3::ZERO,
            &store,
        );

        let instances = collector.instances();
        assert_eq!(instances.len(), 2, "holder plus one resolved attachment");
        let socket_matrix = glam::Mat4::from_translation(Vec3::new(3.0, 0.0, 0.0))
            * glam::Mat4::from_translation(Vec3::new(0.0, 2.0, 0.0))
            * glam::Mat4::from_rotation_x(-inputs.aim_pitch);
        assert_mat4_approx(
            instances[1].transform,
            instances[0].transform * socket_matrix,
            "attachment composes the modified child world pose after the holder transform",
        );
        assert_eq!(instances[1].model, ModelHandle::from("hand-prop"));
        assert_eq!(
            instances[1].pose_inputs, None,
            "props carry no holder inputs"
        );
        assert_eq!(instances[1].sample, MeshSampleParams::rigid());
        assert!(instances[1].capture.is_none(), "props never capture fades");
    }

    #[test]
    fn stateless_holder_and_attachment_share_rest_pose_policy() {
        // Regression: the body sampled clip zero while the socket sampled rest,
        // so a moving clip-zero joint detached an animation-free holder's prop.
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_cell_world();
        let store = socket_pose_store(
            "holder",
            postretro_model::pose_modifier::PoseModifierStack::default(),
        );
        let id = registry.spawn(Transform::default());
        let mut mesh = MeshComponent::stateless("holder".to_string());
        mesh.attachments
            .push(attachment(AttachmentBinding::Skinned(1), "prop"));
        registry.set_component(id, mesh).unwrap();

        let mut tables = MeshClipTables::new();
        tables.insert_with_bounds(
            ModelHandle::from("holder"),
            &clip_meta(&[("clip-zero", 1.0)]),
            Aabb::default(),
        );
        collector.collect_with_hit_zones(
            &registry,
            &world,
            &VisibleCells::DrawAll,
            1.0,
            0.5,
            &tables,
            Vec3::ZERO,
            &store,
        );

        let instances = collector.instances();
        assert_eq!(instances.len(), 2);
        assert_eq!(
            instances[0].sample,
            MeshSampleParams::rest(),
            "the holder body explicitly selects rest pose",
        );
        assert_mat4_approx(
            instances[1].transform,
            glam::Mat4::from_translation(Vec3::new(0.0, 2.0, 0.0)),
            "the attachment resolves from the same rest pose, not moving clip zero",
        );
    }

    #[test]
    fn rigid_attachment_uses_rest_matrix_and_identity_palette_without_pose_sampling() {
        use glam::{Quat, Vec3};

        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_cell_world();
        let id = registry.spawn(Transform {
            position: Vec3::new(2.0, 3.0, 4.0),
            rotation: Quat::from_rotation_z(0.3),
            scale: Vec3::new(1.5, 2.0, 0.5),
        });
        let rigid_socket = glam::Mat4::from_translation(Vec3::new(0.0, 1.0, -2.0));
        let mut mesh = MeshComponent::stateless("rigid-holder".to_string());
        mesh.attachments.push(attachment(
            AttachmentBinding::Rigid(rigid_socket),
            "rigid-prop",
        ));
        registry.set_component(id, mesh).unwrap();

        collector.collect(
            &registry,
            &world,
            &VisibleCells::DrawAll,
            1.0,
            17.0,
            &MeshClipTables::new(),
            Vec3::ZERO,
        );

        let instances = collector.instances();
        assert_eq!(instances.len(), 2, "rigid holder and prop both emit");
        let holder_transform = glam::Mat4::from_scale_rotation_translation(
            Vec3::new(1.5, 2.0, 0.5),
            Quat::from_rotation_z(0.3),
            Vec3::new(2.0, 3.0, 4.0),
        );
        assert_mat4_approx(
            instances[1].transform,
            holder_transform * rigid_socket,
            "rigid attachment reads its pre-resolved rest matrix directly",
        );
        assert_eq!(instances[1].pose_inputs, None);
        assert_eq!(instances[1].sample, MeshSampleParams::rigid());
        assert!(!instances[1].resample, "rigid prop has no pose to resample");
        assert_ne!(
            instances[0].palette_cache_key, instances[1].palette_cache_key,
            "rigid attachment must not alias its holder's palette-cache entry",
        );
    }

    #[test]
    fn attachments_inherit_shadow_only_holder_visibility() {
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_cell_world_with_selected_static_shadow_light();
        let id = registry.spawn(Transform {
            position: Vec3::new(1.0, 0.0, 0.0),
            ..Transform::default()
        });
        let mut mesh = MeshComponent::stateless("holder".to_string());
        mesh.attachments.push(attachment(
            AttachmentBinding::Rigid(glam::Mat4::from_translation(Vec3::X)),
            "prop",
        ));
        registry.set_component(id, mesh).unwrap();

        collector.collect(
            &registry,
            &world,
            &VisibleCells::Culled(vec![1]),
            1.0,
            0.0,
            &MeshClipTables::new(),
            Vec3::ZERO,
        );

        let instances = collector.instances();
        assert_eq!(
            instances.len(),
            2,
            "shadow-retained holder retains its prop"
        );
        assert!(
            instances.iter().all(|instance| !instance.forward_visible),
            "holder and prop share the same shadow-only visibility class",
        );
        assert!(
            instances
                .iter()
                .all(|instance| !instance.dynamic_shadow_visible),
            "promoted-static relevance must not leak into dynamic depth",
        );
    }

    #[test]
    fn skinned_attachment_forces_far_holder_resample_without_hit_zones_or_modifiers() {
        let mut registry = EntityRegistry::new();
        let mut collector = MeshRenderCollector::new();
        let world = single_cell_world();
        let store = socket_pose_store(
            "holder",
            postretro_model::pose_modifier::PoseModifierStack::default(),
        );
        let id = registry.spawn(Transform::default());
        let mut mesh = MeshComponent::stateless("holder".to_string());
        mesh.attachments
            .push(attachment(AttachmentBinding::Skinned(1), "prop"));
        registry.set_component(id, mesh).unwrap();

        for _ in 0..(RESAMPLE_STRIDE_FAR * 2) {
            collector.collect_with_hit_zones(
                &registry,
                &world,
                &VisibleCells::DrawAll,
                1.0,
                3.0,
                &MeshClipTables::new(),
                far_camera(),
                &store,
            );
            assert_eq!(collector.instances().len(), 2);
            assert_eq!(
                collector.resample_count(),
                1,
                "the holder palette must stay at the socket sample's anim-time",
            );
            assert!(collector.instances()[0].resample);
        }
    }

    #[test]
    fn remote_descriptor_attachment_emits_through_the_presentation_collector() {
        use postretro_entities::{EntityTypeDescriptor, MeshDescriptor};

        let mut registry = EntityRegistry::new();
        let id = registry.spawn(Transform::default());
        let descriptor = EntityTypeDescriptor {
            canonical_name: Some("remote-holder".to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: Some(MeshDescriptor {
                model: "remote-holder-model".to_string(),
                shadow_only: false,
                attachments: [("socket".to_string(), "remote-prop".to_string())]
                    .into_iter()
                    .collect(),
                shadow_bias_scale: 1.0,
                animations: HashMap::new(),
                default_state: None,
                locomotion: None,
            }),
            health: None,
            behavior: None,
        };
        assert!(
            crate::scripting::builtins::net_descriptor::materialize_net_mesh_presentation(
                "remote-holder",
                &[descriptor],
                &mut registry,
                id,
                None,
            ),
            "remote descriptor materializes its presentation mesh",
        );
        let mut mesh = registry.get_component::<MeshComponent>(id).unwrap().clone();
        mesh.attachments[0].binding = AttachmentBinding::Rigid(glam::Mat4::IDENTITY);
        registry.set_component(id, mesh).unwrap();

        let mut collector = MeshRenderCollector::new();
        collector.collect(
            &registry,
            &single_cell_world(),
            &VisibleCells::DrawAll,
            1.0,
            0.0,
            &MeshClipTables::new(),
            Vec3::ZERO,
        );
        assert_eq!(collector.instances().len(), 2);
        assert_eq!(
            collector.instances()[1].model,
            ModelHandle::from("remote-prop")
        );
    }
}
