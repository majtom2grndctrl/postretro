// Renderer level resources: sprite-collection registration, the phased build
// of one load's resource set, and the oracle wrappers over build, publish and
// release (`publish.rs`).
// See: context/lib/resource_management.md · context/plans/done/level-load-pipeline/index.md (Per-load renderer resources)

mod lightmap_shadow;
mod lights;
mod publish;
mod sh_family;
mod shadow_pools;
mod world_geometry;

pub use lightmap_shadow::lightmap_family_inputs;
pub(in crate::render) use publish::rebuild_level_extent_groups;

use super::level_build::LevelBuildContext;
use super::level_resource_set::{LevelSetId, PreviousLevelSet};
use super::renderer_types::FullRenderer;
use super::smoke::SpriteCollectionAssetSource;
use super::*;
use postretro_render_cpu::sh_residency::{ShResidencyDrainError, ShResidencyPlan};

/// Discard the cull-split per-frame mover inputs when level geometry changes.
fn clear_kinematic_mover_frame_state(
    draws: &mut Vec<kinematic_brush::KinematicMoverInstance>,
    shadow_draws: &mut Vec<kinematic_brush::KinematicMoverInstance>,
    occluder_aabbs: &mut Vec<rigid_occluder_depth::MoverOccluderAabb>,
) {
    draws.clear();
    shadow_draws.clear();
    occluder_aabbs.clear();
}

impl Renderer {
    /// Upload one draw contract per collection into the active level set (or,
    /// with none, stage it for the next build). Level installation resolves
    /// every emitter and projectile consumer's frame count before this
    /// boundary; the smoke pass owns its baked-sidecar attempt and PNG-decode
    /// fallback. The baked-sidecar eligibility is an asset-level flag
    /// established by map-authored billboard emitters, so collections using
    /// the same asset share its cache decision. Duplicate calls for an id are
    /// reported and rejected rather than silently overriding an accepted
    /// descriptor's cadence or emissive strength.
    pub fn register_smoke_collection(
        &mut self,
        collection_id: &str,
        asset: &str,
        texture_root: &Path,
        prm_cache_root: &Path,
        registration: SpriteCollectionRegistration,
    ) {
        let Self {
            device,
            queue,
            full,
            ..
        } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        queue.assert_empty("smoke collection install");
        let collections = match full.level.as_mut() {
            Some(level) => &mut level.smoke,
            None => &mut full.pending_smoke,
        };
        full.smoke_pass.register_collection(
            collections,
            device,
            queue.raw(),
            collection_id,
            SpriteCollectionAssetSource {
                asset,
                texture_root,
                prm_cache_root,
            },
            registration,
        );
    }

    /// Release the active level's renderer resources while keeping the
    /// device, queue, surface, UI, and window-facing state alive for the
    /// no-level Frontend: releases the active set (and drops anything staged
    /// for the next build). Resident, unpublished sets stay. Creates no GPU
    /// object; the renderer-wide floor keeps the dropped set's lightmap
    /// generations.
    pub fn release_level_resources(&mut self) {
        if let Some(id) = self.active_level_set() {
            self.release_level_set(id);
        }
        let full = self.full_mut();
        full.pending_textures = None;
        full.pending_models = Default::default();
        full.pending_smoke = Default::default();
        full.pending_samplers = Default::default();
    }

    /// Build one level's renderer resources into a fresh set and publish it
    /// as the active set, retiring the previous one: the main-thread oracle
    /// install. With no materials staged by `install_textures`, the previous
    /// active set's carry over; its models, sprite collections and fog pixel
    /// scale carry over too, as they did when those lived renderer-wide.
    /// Call `validate_level_geometry_ranges` before the first install step;
    /// release indirect draws rely on its checked leaf/index-buffer mapping.
    /// See: context/lib/boot_sequence.md §3 (Level Install Order)
    pub fn install_level_geometry(
        &mut self,
        geometry: &LevelGeometry<'_>,
        gpu_lighting_payloads: postretro_level_loader::GpuLightingPayloads,
    ) -> Result<(), LevelInstallError> {
        // Every check that can fail on the level's data runs before the
        // previous set is touched.
        let sh_plan = plan_level_sh_residency(geometry)?;
        let full = self.full_mut();
        let fog_pixel_scale = match full.level.as_mut() {
            Some(previous) => {
                if full.pending_textures.is_none() {
                    full.pending_textures = Some(previous.take_textures());
                }
                full.pending_models.absorb(&mut previous.models);
                full.pending_smoke.absorb(&mut previous.smoke);
                previous.extent.fog.pixel_scale()
            }
            None => DEFAULT_LEVEL_FOG_PIXEL_SCALE,
        };
        let id =
            self.build_level_set_at(geometry, gpu_lighting_payloads, fog_pixel_scale, sh_plan)?;
        self.publish_level_set(id, PreviousLevelSet::Retire);
        Ok(())
    }

    /// Build one level's renderer resources into a fresh, unpublished set
    /// and return its key. Takes the materials `install_textures` staged
    /// (else binds one placeholder), any models and sprite collections staged
    /// with no active set, and the level's GPU-only lightmap and shadowmask
    /// payloads by value, dropping them once their textures exist. The set's
    /// extent-bound groups are built at the current scene extent, its spot
    /// shadow pool is looked up by the current shadow tier, and its lightmap
    /// generations start above every live set's. Changes nothing the active
    /// set or its frames read: no cache resets, no retirement, no write to
    /// the renderer's sampler maps. Call `validate_level_geometry_ranges`
    /// first.
    pub fn build_level_set(
        &mut self,
        geometry: &LevelGeometry<'_>,
        gpu_lighting_payloads: postretro_level_loader::GpuLightingPayloads,
    ) -> Result<LevelSetId, LevelInstallError> {
        let sh_plan = plan_level_sh_residency(geometry)?;
        self.build_level_set_at(
            geometry,
            gpu_lighting_payloads,
            DEFAULT_LEVEL_FOG_PIXEL_SCALE,
            sh_plan,
        )
    }

    /// `sh_plan` is the streamed level's residency plan (`None` for a legacy
    /// level). On `Err` no set exists and the staged materials, models and
    /// sprite collections stay staged for the next build.
    fn build_level_set_at(
        &mut self,
        geometry: &LevelGeometry<'_>,
        gpu_lighting_payloads: postretro_level_loader::GpuLightingPayloads,
        fog_pixel_scale: u32,
        sh_plan: Option<ShResidencyPlan>,
    ) -> Result<LevelSetId, LevelInstallError> {
        debug_assert!(
            validate_level_geometry_ranges(&geometry.bvh.leaves, geometry.indices.len()).is_ok(),
            "level geometry must pass the indirect index-range check before installation"
        );
        let tiers = self.level_build_tiers();
        let context = self.capture_level_build_context(tiers, true);
        let Self { queue, full, .. } = self;
        let _installation = queue.installation();
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        // This main-thread build is the oracle: it may wait for the
        // renderer's one pipeline build, and builds no pipeline itself.
        let pipelines = full.install_pipelines.wait_for_oracle();
        let mut level = build_level_set_core(
            &context,
            &pipelines,
            queue.raw(),
            LevelSetCoreInputs {
                geometry,
                gpu_lighting_payloads,
                sh_plan,
                lightmap_plan: None,
                fog_pixel_scale,
            },
        )?;
        // Past the last step that can fail, so a failed build leaves the
        // staged materials, models and sprite collections for the next one.
        let textures = match full.pending_textures.take() {
            Some(textures) => textures,
            None => super::renderer_textures::placeholder_level_textures(
                &context.device,
                queue.raw(),
                &context.material_bind(),
            ),
        };
        level.install_textures(textures);
        level.samplers.absorb(&mut full.pending_samplers);
        level.models = std::mem::take(&mut full.pending_models);
        level.smoke = std::mem::take(&mut full.pending_smoke);
        let id = level.id;
        full.resident_levels.push(level);
        Ok(id)
    }
}

/// One build's geometry inputs. `lightmap_plan` is the CPU plan prep built
/// on the pool; `None` plans in the build (the oracle).
pub(in crate::render) struct LevelSetCoreInputs<'a, 'g> {
    pub(in crate::render) geometry: &'a LevelGeometry<'g>,
    pub(in crate::render) gpu_lighting_payloads: postretro_level_loader::GpuLightingPayloads,
    pub(in crate::render) sh_plan: Option<ShResidencyPlan>,
    pub(in crate::render) lightmap_plan:
        Option<postretro_render_cpu::lightmap_family::LightmapFamilyPlan>,
    pub(in crate::render) fog_pixel_scale: u32,
}

/// Build one level's renderer set from `context`, on whichever thread runs
/// it: the main-thread oracle and the upload thread's geometry part both
/// call this. Intra-install order: world buffers, lights, the shadow
/// tier and promoted state, the lighting group, SH volume and streamed
/// residency (over the promoted weight buffer), the mesh and mover light
/// groups, the compose family, the lightmap family (animated atlas first),
/// the SDF shadow groups over the fresh SH depth moments, BVH and cull,
/// kinematic geometry, and the extent-bound groups at the context's extent.
/// The set's materials, models and sprite collections are left empty for the
/// caller to install. Writes nothing renderer-wide. Its GPU-only lightmap and
/// shadowmask payloads drop once their textures exist.
pub(in crate::render) fn build_level_set_core(
    context: &LevelBuildContext,
    pipelines: &super::install_pipelines::InstallPipelines,
    writes: &dyn super::queue_writes::QueueWrites,
    inputs: LevelSetCoreInputs<'_, '_>,
) -> Result<LevelResourceSet, LevelInstallError> {
    let LevelSetCoreInputs {
        geometry,
        gpu_lighting_payloads,
        sh_plan,
        lightmap_plan,
        fog_pixel_scale,
    } = inputs;
    let device = &context.device;
    let mut install_marks = super::geometry_install_marks::GeometryInstallMarks::start();

    let streaming_manifest = match &geometry.sh_storage {
        LevelGeometryShStorage::Legacy => None,
        LevelGeometryShStorage::Streaming(manifest) => Some(*manifest),
    };

    let world = world_geometry::install_world_buffers(device, geometry);
    let (lights, selection) = lights::install_level_lights(device, geometry);
    let level_light_count = selection.level_light_count;
    let animated_baked_descriptor_indices = selection.animated_baked_descriptor_indices;
    let shadow = shadow_pools::level_shadow_tier(
        device,
        context.shadow_tier_for_build(),
        context.cube_array_supported,
        geometry,
        &selection,
    );
    let promoted = shadow_pools::level_promoted_state(device, geometry, &selection);
    let (lighting, light_buffers) = lights::install_lighting_bind_group(
        device,
        &context.lighting_bgl,
        &lights,
        geometry,
        selection,
    );
    let sh = sh_family::install_sh_volume(
        device,
        writes,
        context.probe_occlusion_enabled,
        pipelines,
        geometry,
        streaming_manifest,
        level_light_count,
        animated_baked_descriptor_indices,
        &promoted.promoted_static_weight_buffer,
        sh_plan,
    )
    .map_err(LevelInstallError::StreamedSh)?;
    let (mesh_lights, kinematic_lights) = lights::rebind_mesh_light_groups(
        device,
        context,
        &shadow,
        &lights.lights_buffer,
        lighting.mirror_lighting.lights_buffer(),
        &lighting.influence_buffer,
        &sh.sh_volume_resources,
    );

    install_marks.mark("buffers_and_sh_streaming");
    let compose = sh_family::install_compose_family(
        device,
        writes,
        pipelines,
        geometry,
        &sh.sh_volume_resources,
        sh.sh_streaming.as_ref(),
        &promoted.promoted_static_weight_buffer,
        sh.ledger,
    );
    let (lightmap_family, bvh_leaves) = lightmap_shadow::plan_lightmap_family(
        device,
        &sh.sh_volume_resources,
        pipelines,
        geometry,
        &gpu_lighting_payloads,
        lightmap_plan,
    );
    install_marks.mark("sdf_atlas_and_compose");
    let lightmap = lightmap_shadow::install_lightmap(
        device,
        writes,
        geometry,
        gpu_lighting_payloads,
        lightmap_family,
        context.generation_floor,
    );
    let sdf_shadow = lightmap_shadow::rebind_sdf_shadow(
        device,
        context,
        geometry,
        &light_buffers,
        streaming_manifest,
        &sh.sh_volume_resources,
        sh.sh_streaming.as_ref(),
    );

    install_marks.mark("lightmap_and_sdf_shadow");
    let cull = world_geometry::install_bvh_and_cull(
        device,
        pipelines,
        geometry,
        bvh_leaves,
        context.has_multi_draw_indirect,
    );

    install_marks.mark("bvh_and_cull");
    // Shadow reach follows the installed BVH: the first fill after install
    // walks this level's tree, and no range from the previous level survives.
    let shadow_world =
        super::shadow_world_draws::ShadowWorldDraws::install(Some(geometry.bvh), world.index_count);
    let kinematic = kinematic_brush::KinematicLevelState::install(
        device,
        geometry.kinematic_geometry,
        geometry.texture_materials.len(),
        kinematic_lights,
    );
    let extent = publish::build_level_extent_groups(device, context, &shadow, fog_pixel_scale);

    let has_geometry = world.has_geometry;
    install_marks.log(has_geometry);
    if has_geometry {
        log::info!(
            "[Renderer] Geometry installed: {} indices, bvh_leaves={}",
            world.index_count,
            cull.bvh_leaves.len(),
        );
    }
    Ok(LevelResourceSet {
        id: context.id,
        shadow,
        extent,
        samplers: Default::default(),
        models: Default::default(),
        smoke: Default::default(),
        fluid_targets: Default::default(),
        mirror_targets: Default::default(),
        planar_app_candidate: None,
        loaded_textures: Vec::new(),
        gpu_textures: Vec::new(),
        stored_texture_materials: geometry.texture_materials.to_vec(),
        vertex_buffer: world.vertex_buffer,
        spec_owner_buffer: world.spec_owner_buffer,
        index_buffer: world.index_buffer,
        index_count: world.index_count,
        wireframe_index_buffer: world.wireframe_index_buffer,
        wireframe_index_count: world.wireframe_index_count,
        has_geometry,
        bvh_leaves: cull.bvh_leaves,
        cell_draw_index: cull.cell_draw_index,
        compute_cull: cull.compute_cull,
        candidate_cull: cull.candidate_cull,
        candidate_cull_oor_logged: false,
        shadow_world,
        lights_buffer: lights.lights_buffer,
        influence_buffer: lighting.influence_buffer,
        lighting_bind_group: lighting.lighting_bind_group,
        mirror_lighting: lighting.mirror_lighting,
        dynamic_light_capacity: lights.dynamic_light_capacity,
        light_count: lights.light_count,
        animated_baked_light_count: lights.animated_baked_light_count,
        total_light_count: lights.total_light_count,
        level_lights: lights.level_lights,
        level_light_source_indices: lights.level_light_source_indices,
        level_light_influences: lighting.level_light_influences,
        entity_shadow_lights: lights.entity_shadow_lights,
        entity_shadow_light_influences: lights.entity_shadow_light_influences,
        entity_shadow_light_source_indices: lights.entity_shadow_light_source_indices,
        entity_shadow_spec_light_indices: lights.entity_shadow_spec_light_indices,
        shadow_candidate_lights: lighting.shadow_candidate_lights,
        shadow_candidate_source_indices: lighting.shadow_candidate_source_indices,
        shadow_candidate_selection_indices: lighting.shadow_candidate_selection_indices,
        shadow_candidate_animated_baked_indices: lighting.shadow_candidate_animated_baked_indices,
        shadow_candidate_influences: lighting.shadow_candidate_influences,
        light_effective_brightness: Vec::new(),
        animated_light_window_brightness: promoted.animated_light_window_brightness,
        last_lights_upload: Vec::new(),
        authored_animated_tail: Vec::new(),
        last_influence_upload: Vec::new(),
        lights_pack_scratch: Vec::new(),
        influence_pack_scratch: Vec::new(),
        shadowmask_present: lightmap.shadowmask_present,
        forward_shadowmask_metadata_scratch: Vec::new(),
        promoted_static_states: promoted.promoted_static_states,
        promoted_animated_states: promoted.promoted_animated_states,
        promoted_baked_records: Vec::new(),
        promoted_baked_cache_layers: Vec::new(),
        promoted_static_weights: promoted.promoted_static_weights,
        promoted_static_weight_buffer: promoted.promoted_static_weight_buffer,
        promoted_static_weight_scratch: Vec::new(),
        promoted_baked_last_update_time: None,
        promotions_start_whole: false,
        sh_volume_resources: sh.sh_volume_resources,
        sh_residency_report: compose.sh_residency_report,
        lightmap_residency_report: lightmap.lightmap_residency_report,
        sh_streaming: sh.sh_streaming,
        sdf_atlas_resources: compose.sdf_atlas_resources,
        lightmap_mode: compose.lightmap_mode,
        #[cfg(feature = "dev-tools")]
        sh_delta_volumes_meta: compose.sh_delta_volumes_meta,
        #[cfg(feature = "dev-tools")]
        sh_probe_readback: compose.sh_probe_readback,
        sh_compose: compose.sh_compose,
        direct_sh_compose: compose.direct_sh_compose,
        billboard_direct_scatter_compose: compose.billboard_direct_scatter_compose,
        lightmap_resources: lightmap.lightmap_resources,
        animated_lightmap: lightmap.animated_lightmap,
        sdf_shadow,
        mesh_lights,
        kinematic,
        fluid_surfaces: None,
        fog: Default::default(),
        fog_cell_masks: None,
        active_fog_aabbs: Vec::new(),
    })
}

/// A new set's fog pixel scale until worldspawn's arrives
/// (`Renderer::set_fog_pixel_scale`).
const DEFAULT_LEVEL_FOG_PIXEL_SCALE: u32 = 0;

impl FullRenderer {
    /// The lightmap generation floor a new set reserves: above the
    /// renderer-wide add-only floor and every live set's high water at
    /// dispatch. The floor is reserved at dispatch, so beside a running level
    /// that set can still accept generations above it until the new set
    /// commits. That is safe because the old session retires before the
    /// commit and the streaming controllers' generation clocks are
    /// process-monotonic, so no generation is accepted twice.
    pub(in crate::render) fn reserve_lightmap_generation_floor(&mut self) -> u64 {
        let live = self
            .level
            .iter()
            .chain(&self.resident_levels)
            .map(|level| level.lightmap_resources.generation_high_water())
            .max()
            .unwrap_or(0);
        self.lightmap_generation_floor = self.lightmap_generation_floor.max(live);
        self.lightmap_generation_floor
    }

    /// Drop a level set the renderer no longer draws, folding its lightmap
    /// generations into the renderer-wide floor first. Creates nothing.
    pub(in crate::render) fn retire_level_set(&mut self, level: LevelResourceSet) {
        self.lightmap_generation_floor = self
            .lightmap_generation_floor
            .max(level.lightmap_resources.generation_high_water());
        drop(level);
    }
}

/// Per-frame draw state the previous level's ids index: mover draw lists and
/// ghosts, skinned and billboard draw inputs, and the fluid and planar frame
/// state keyed to its surfaces. CPU-only.
fn reset_level_frame_state(full: &mut FullRenderer) {
    full.kinematic_brush.clear_frame_draws();
    // Mover ids are the new level's: drop the last frame's mover ghosts.
    full.kinematic_ghost.clear();
    full.billboard_ghost.clear();
    full.mesh_draws.clear();
    full.ghost_mesh_draws.clear();
    full.bone_palette_scratch.clear();
    full.mesh_pass.clear_for_level_load();
    full.planar_reflection.clear_level();
    full.fog.clear_frame_counts();
    clear_kinematic_mover_frame_state(
        &mut full.kinematic_mover_draws,
        &mut full.kinematic_mover_shadow_draws,
        &mut full.mover_occluder_aabbs,
    );
}

/// Why a level's renderer resources could not be built. Every variant is a
/// load failure: no set exists, and the caller fails the load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LevelInstallError {
    /// The streamed-SH residency plan rejected the level's data, or its
    /// first pool generation does not fit the device.
    StreamedSh(ShResidencyDrainError),
}

impl std::fmt::Display for LevelInstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StreamedSh(error) => {
                write!(f, "cannot install streamed SH level resources: {error}")
            }
        }
    }
}

impl std::error::Error for LevelInstallError {}

/// The streamed level's residency plan, or `None` for a legacy level. A
/// streamed level deliberately omits the legacy SH bodies, so a rejected
/// plan fails the install rather than rendering the level unlit.
fn plan_level_sh_residency(
    geometry: &LevelGeometry<'_>,
) -> Result<Option<ShResidencyPlan>, LevelInstallError> {
    match &geometry.sh_storage {
        LevelGeometryShStorage::Legacy => Ok(None),
        LevelGeometryShStorage::Streaming(manifest) => {
            postretro_render_cpu::sh_residency::plan_sh_residency(manifest, geometry.cells)
                .map(Some)
                .map_err(LevelInstallError::StreamedSh)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Mat4, Vec3};
    use postretro_render_data::cone_frustum::Aabb;

    #[test]
    fn kinematic_mover_frame_state_reset_clears_draws_shadow_casters_and_occluders() {
        let mut draws = vec![kinematic_brush::KinematicMoverInstance {
            mover_id: 7,
            transform: Mat4::IDENTITY,
        }];
        let mut shadow_draws = vec![kinematic_brush::KinematicMoverInstance {
            mover_id: 7,
            transform: Mat4::IDENTITY,
        }];
        let mut occluder_aabbs = vec![rigid_occluder_depth::MoverOccluderAabb {
            mover_id: 7,
            world_aabb: Aabb {
                min: Vec3::ZERO,
                max: Vec3::ONE,
            },
        }];

        clear_kinematic_mover_frame_state(&mut draws, &mut shadow_draws, &mut occluder_aabbs);

        assert!(draws.is_empty());
        assert!(shadow_draws.is_empty());
        assert!(occluder_aabbs.is_empty());
    }
}
