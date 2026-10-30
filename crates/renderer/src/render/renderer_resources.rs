// Renderer level resources: sprite-collection registration, level release,
// and the phased level-geometry install.
// See: context/lib/resource_management.md

mod lightmap_shadow;
mod lights;
mod sh_family;
mod shadow_pools;
mod world_geometry;

use super::renderer_types::FullRenderer;
use super::smoke::SpriteCollectionAssetSource;
use super::*;

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
    /// Upload one draw contract per collection. Level installation resolves every
    /// emitter and projectile consumer's frame count before this boundary; the
    /// smoke pass owns its baked-sidecar attempt and PNG-decode fallback. The
    /// baked-sidecar eligibility is an asset-level flag established by map-authored
    /// billboard emitters, so collections using the same asset share its cache
    /// decision. Duplicate calls for an id are reported and rejected rather than
    /// silently overriding an accepted descriptor's cadence or emissive strength.
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
        full.smoke_pass.register_collection(
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

    /// Release all level-owned GPU resources while keeping the device, queue,
    /// surface, UI, and window-facing state alive for the no-level Frontend.
    pub fn release_level_resources(&mut self) {
        self.queue.assert_empty("level unload");
        let empty_keys = TextureCacheKeysSection::default();
        let empty_texture_names: Vec<String> = Vec::new();
        let empty_materials: Vec<Material> = Vec::new();
        self.install_textures(
            &empty_texture_names,
            &empty_keys,
            Path::new(""),
            &empty_materials,
        );

        let empty_bvh = BvhTree {
            nodes: Vec::new(),
            leaves: Vec::new(),
            root_node_index: 0,
        };
        let empty_geometry = LevelGeometry {
            vertices: &[],
            vertex_spec_owners: &[],
            indices: &[],
            bvh: &empty_bvh,
            lights: &[],
            light_influences: &[],
            sh_volume: None,
            sh_storage: LevelGeometryShStorage::Legacy,
            lightmap: None,
            lightmap_streaming: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
            delta_sh_volumes: None,
            direct_sh_volume: None,
            direct_sh_delta_volumes: None,
            animated_direct_sh_delta_volumes: None,
            billboard_direct_scatter_volume: None,
            animated_billboard_direct_scatter_delta_volumes: None,
            entity_shadow_lights: &[],
            shadowmask_atlas: None,
            sdf_atlas: None,
            lightmap_mode: postretro_level_loader::LightmapMode::default(),
            cell_draw_index: None,
            kinematic_geometry: None,
            cells: &[],
            texture_materials: &empty_materials,
        };
        self.install_level_geometry(&empty_geometry, Default::default());

        self.full_mut().smoke_pass.clear_collections();
        self.full_mut().billboard_ghost.clear();
        self.full_mut().mesh_pass.release_level_resources();
        self.full_mut().mesh_draws.clear();
        self.full_mut().ghost_mesh_draws.clear();
        let full = self.full_mut();
        clear_kinematic_mover_frame_state(
            &mut full.kinematic_mover_draws,
            &mut full.kinematic_mover_shadow_draws,
            &mut full.mover_occluder_aabbs,
        );
        self.full_mut().bone_palette_scratch.clear();
        self.full_mut().fog_cell_masks = None;
        self.full_mut().active_fog_aabbs.clear();
        self.upload_fog_volumes(&[], &[], 0);
        self.upload_fog_points(&[]);
        self.set_fog_pixel_scale(0);
    }

    /// Replaces dummy buffers with real geometry; rebuilds lighting, SH, lightmap, and cull pipeline.
    /// Takes the level's GPU-only lightmap and shadowmask payloads by value and
    /// drops them once their textures exist.
    /// Call `validate_level_geometry_ranges` before the first install step;
    /// release indirect draws rely on its checked leaf/index-buffer mapping.
    /// See: context/lib/boot_sequence.md §3 (Level Install Order)
    pub fn install_level_geometry(
        &mut self,
        geometry: &LevelGeometry<'_>,
        gpu_lighting_payloads: postretro_level_loader::GpuLightingPayloads,
    ) {
        debug_assert!(
            validate_level_geometry_ranges(&geometry.bvh.leaves, geometry.indices.len()).is_ok(),
            "level geometry must pass the indirect index-range check before installation"
        );
        let Self {
            device,
            queue,
            has_multi_draw_indirect,
            spot_shadow_map_resolution,
            full,
            ..
        } = self;
        let _installation = queue.installation();
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        let mut install_marks = super::geometry_install_marks::GeometryInstallMarks::start();

        // Drop the prior generation at the level boundary. Construction waits
        // until the fresh shared SH resources below exist, because streamed
        // group-3 bind groups retain those always-resident animation and
        // billboard-scatter bindings.
        let streaming_manifest = match &geometry.sh_storage {
            LevelGeometryShStorage::Legacy => None,
            LevelGeometryShStorage::Streaming(manifest) => Some(*manifest),
        };
        full.sh_streaming = None;
        let has_multi_draw_indirect = *has_multi_draw_indirect;
        let requested_spot_resolution = *spot_shadow_map_resolution;

        shadow_pools::rebuild_spot_shadow_pool_for_tier(device, full, requested_spot_resolution);
        let has_geometry = world_geometry::install_world_buffers(device, full, geometry);
        let selection = lights::install_level_lights(device, full, geometry);
        let level_light_count = selection.level_light_count;
        let animated_baked_descriptor_indices = selection.animated_baked_descriptor_indices;
        shadow_pools::reset_level_shadow_caches(device, full, geometry, &selection);
        let light_buffers = lights::install_lighting_bind_group(device, full, geometry, selection);
        let sh_allocation_ledger = sh_family::install_sh_volume(
            device,
            queue,
            full,
            geometry,
            streaming_manifest,
            level_light_count,
            animated_baked_descriptor_indices,
        );
        lights::rebind_mesh_light_groups(device, full);

        install_marks.mark("buffers_and_sh_streaming");
        sh_family::install_compose_family(device, queue, full, geometry, sh_allocation_ledger);
        let (lightmap_plan, bvh_leaves) =
            lightmap_shadow::plan_lightmap_family(device, full, geometry, &gpu_lighting_payloads);
        install_marks.mark("sdf_atlas_and_compose");
        lightmap_shadow::install_lightmap(
            device,
            queue,
            full,
            geometry,
            gpu_lighting_payloads,
            lightmap_plan,
        );
        lightmap_shadow::rebind_sdf_shadow(
            device,
            full,
            geometry,
            &light_buffers,
            streaming_manifest,
        );

        install_marks.mark("lightmap_and_sdf_shadow");
        world_geometry::install_bvh_and_cull(
            device,
            full,
            geometry,
            bvh_leaves,
            has_multi_draw_indirect,
        );

        install_marks.mark("bvh_and_cull");
        install_level_draw_state(device, full, geometry, has_geometry);

        install_marks.log(has_geometry);
        if has_geometry {
            log::info!(
                "[Renderer] Geometry installed: {} indices, bvh_leaves={}",
                full.index_count,
                full.bvh_leaves.len(),
            );
        }
    }
}

/// Per-level draw-side state that follows the installed geometry: shadow
/// reach, light upload scratch, materials, mover geometry, and the fluid and
/// planar resets.
fn install_level_draw_state(
    device: &wgpu::Device,
    full: &mut FullRenderer,
    geometry: &LevelGeometry<'_>,
    has_geometry: bool,
) {
    // Shadow reach follows the installed BVH: the first fill after install
    // walks this level's tree, and no range from the previous level survives.
    full.shadow_world =
        super::shadow_world_draws::ShadowWorldDraws::install(Some(geometry.bvh), full.index_count);

    full.has_geometry = has_geometry;
    full.last_lights_upload.clear();
    full.authored_animated_tail.clear();
    full.last_influence_upload.clear();
    full.lights_pack_scratch.clear();
    full.influence_pack_scratch.clear();
    full.light_effective_brightness.clear();
    full.stored_texture_materials = geometry.texture_materials.to_vec();
    full.kinematic_brush.install_geometry(
        device,
        geometry.kinematic_geometry,
        geometry.texture_materials.len(),
    );
    // Mover ids are the new level's: drop the last frame's mover ghosts.
    full.kinematic_ghost.clear();
    // Fluid surfaces install after this through `install_fluid_surfaces`;
    // every geometry install (unload included) drops the previous level's,
    // its reflection target with them.
    full.fluid_surface.clear(device);
    full.planar_reflection.clear_level();
    clear_kinematic_mover_frame_state(
        &mut full.kinematic_mover_draws,
        &mut full.kinematic_mover_shadow_draws,
        &mut full.mover_occluder_aabbs,
    );
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
