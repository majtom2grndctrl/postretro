// Renderer resource lifecycle: level geometry/texture/material upload and
// model loading.
// See: context/lib/resource_management.md

use super::renderer_types::PromotedStaticLightState;
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
    /// eligibility flag is true only for map-authored billboard emitters. Duplicate
    /// calls are reported and rejected rather than silently overriding an
    /// accepted descriptor's cadence or emissive strength.
    pub fn register_smoke_collection(
        &mut self,
        collection: &str,
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
        full.smoke_pass.register_collection(
            device,
            queue,
            collection,
            texture_root,
            prm_cache_root,
            registration,
        );
    }

    /// Release all level-owned GPU resources while keeping the device, queue,
    /// surface, UI, and window-facing state alive for the no-level Frontend.
    pub fn release_level_resources(&mut self) {
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
            indices: &[],
            bvh: &empty_bvh,
            lights: &[],
            light_influences: &[],
            sh_volume: None,
            lightmap: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
            delta_sh_volumes: None,
            direct_sh_volume: None,
            direct_sh_delta_volumes: None,
            animated_direct_sh_delta_volumes: None,
            entity_shadow_lights: &[],
            shadowmask_atlas: None,
            sdf_atlas: None,
            lightmap_mode: postretro_level_loader::LightmapMode::default(),
            cell_draw_index: None,
            kinematic_geometry: None,
            texture_materials: &empty_materials,
        };
        self.install_level_geometry(&empty_geometry);

        self.full_mut().smoke_pass.clear_collections();
        self.full_mut().mesh_pass.release_level_resources();
        self.full_mut().mesh_draws.clear();
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
    /// See: context/lib/boot_sequence.md §3 (Level Install Order)
    pub fn install_level_geometry(&mut self, geometry: &LevelGeometry<'_>) {
        let Self {
            device,
            queue,
            has_multi_draw_indirect,
            full,
            ..
        } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        let has_multi_draw_indirect = *has_multi_draw_indirect;

        let has_geometry = !geometry.vertices.is_empty() && !geometry.indices.is_empty();

        // --- Vertex / index buffers ---
        let (vertex_data, index_data, index_count) = if has_geometry {
            let count = geometry.indices.len() as u32;
            (
                cast_world_vertices_to_bytes(geometry.vertices),
                bytemuck_cast_slice_u32(geometry.indices),
                count,
            )
        } else {
            (
                vec![0u8; postretro_render_data::geometry::WorldVertex::STRIDE],
                vec![0u8; 4],
                0u32,
            )
        };
        full.vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("World Vertex Buffer"),
            contents: &vertex_data,
            usage: wgpu::BufferUsages::VERTEX,
        });
        full.index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("World Index Buffer"),
            contents: &index_data,
            usage: wgpu::BufferUsages::INDEX,
        });
        full.index_count = index_count;

        // --- Wireframe index buffer ---
        let (wireframe_index_data, wireframe_index_count) = if has_geometry {
            let line_indices = build_line_indices_from_triangles(geometry.indices);
            let count = line_indices.len() as u32;
            (bytemuck_cast_slice_u32(&line_indices), count)
        } else {
            (vec![0u8; 4], 0u32)
        };
        full.wireframe_index_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Wireframe Line Index Buffer"),
                contents: &wireframe_index_data,
                usage: wgpu::BufferUsages::INDEX,
            });
        full.wireframe_index_count = wireframe_index_count;

        // --- Lights + lighting bind group ---
        let filtered_level_lights =
            filter_dynamic_lights(geometry.lights, geometry.light_influences);
        let level_lights = filtered_level_lights.lights;
        let dynamic_influences = filtered_level_lights.influences;
        let level_light_source_indices = filtered_level_lights.source_indices;
        let selected_static = filter_selected_static_entity_shadow_lights(
            geometry.lights,
            geometry.light_influences,
            geometry.entity_shadow_lights,
        );
        let filtered_shadow_candidates = filter_entity_shadow_candidates_with_selection(
            geometry.lights,
            geometry.light_influences,
            geometry.entity_shadow_lights,
        );
        let shadow_candidate_lights = filtered_shadow_candidates.lights;
        let shadow_candidate_influences = filtered_shadow_candidates.influences;
        let shadow_candidate_source_indices = filtered_shadow_candidates.source_indices;
        let shadow_candidate_selection_indices = filtered_shadow_candidates.selection_indices;
        full.light_count = level_lights.len() as u32;
        full.total_light_count = full.light_count;
        let level_light_count = level_lights.len();
        let selected_static_count = selected_static.lights.len();
        let dynamic_light_capacity = level_light_count + RUNTIME_DYNAMIC_LIGHT_RESERVE;
        full.dynamic_light_capacity = dynamic_light_capacity;

        let light_record_capacity = (dynamic_light_capacity + selected_static_count).max(1);
        let mut lights_data = Vec::with_capacity(light_record_capacity * GPU_LIGHT_SIZE);
        if !level_lights.is_empty() {
            lights_data.extend_from_slice(&pack_lights(&level_lights));
        }
        lights_data.resize(light_record_capacity * GPU_LIGHT_SIZE, 0);
        let lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Direct Lights Storage Buffer"),
            contents: &lights_data,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        full.lights_buffer = lights_buffer;
        full.level_lights = level_lights;
        full.level_light_source_indices = level_light_source_indices;
        full.entity_shadow_lights = selected_static.lights;
        full.entity_shadow_light_influences = selected_static.influences;
        full.entity_shadow_light_source_indices = selected_static.source_indices;
        full.entity_shadow_spec_light_indices = shadowmask::build_selection_spec_light_indices(
            geometry.lights,
            geometry.entity_shadow_lights,
        );
        full.shadowmask_channels = geometry
            .shadowmask_atlas
            .map(|section| section.channels.clone())
            .unwrap_or_default();
        full.shadowmask_present = false;
        full.forward_shadowmask_metadata_scratch.clear();
        full.promoted_static_states =
            vec![PromotedStaticLightState::default(); geometry.entity_shadow_lights.len()];
        full.promoted_static_records.clear();
        full.promoted_static_weights = vec![0.0; geometry.entity_shadow_lights.len()];
        full.promoted_static_weight_scratch.clear();
        full.promoted_static_last_update_time = None;
        // Match the init-time policy: the cache exists only for a non-empty
        // selection. A same-selection reload keeps the existing cache and just
        // clears its layer state; a swap to an empty selection frees the cache
        // (VRAM back to zero); a swap from empty to selection-bearing allocates
        // it. Mirrors the conditional weight-buffer allocation below.
        if geometry.entity_shadow_lights.is_empty() {
            full.promoted_depth_cache = None;
        } else if let Some(cache) = &mut full.promoted_depth_cache {
            cache.reset_level();
        } else {
            full.promoted_depth_cache = Some(PromotedDepthCache::new(device));
        }
        full.promoted_depth_cache_frame_plan = PromotedDepthCacheFramePlan::default();
        full.promoted_depth_cache_promoted_count = 0;
        full.promoted_depth_cache_world_render_skips = 0;
        full.promoted_depth_cache_cull_dispatch_skips = 0;
        full.promoted_depth_cache_timing_open = false;
        full.promoted_entity_occluders_submitted = 0;
        full.promoted_static_weight_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Promoted Static Light Weights"),
            size: (geometry.entity_shadow_lights.len().max(1) * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        full.shadow_candidate_lights = shadow_candidate_lights;
        full.shadow_candidate_source_indices = shadow_candidate_source_indices;
        full.shadow_candidate_influences = shadow_candidate_influences;
        full.shadow_candidate_selection_indices = shadow_candidate_selection_indices;

        let influence_record_capacity = shadowmask::influence_capacity_with_shadowmask_metadata(
            dynamic_light_capacity,
            selected_static_count,
        );
        let mut influence_data = Vec::with_capacity(influence_record_capacity * 16);
        if !dynamic_influences.is_empty() {
            influence::pack_influence_into(&mut influence_data, &dynamic_influences);
        }
        influence_data.resize(influence_record_capacity * 16, 0);
        let influence_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Light Influence Storage Buffer"),
            contents: &influence_data,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        full.influence_buffer = influence_buffer;
        full.level_light_influences = dynamic_influences;

        let spec_lights_data = {
            let shadowmask_channels = shadowmask::build_spec_light_shadowmask_channels(geometry);
            let packed = pack_spec_lights(geometry.lights, &shadowmask_channels);
            if packed.is_empty() {
                vec![0u8; SPEC_LIGHT_SIZE]
            } else {
                packed
            }
        };
        let spec_lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Spec-Only Lights Storage Buffer"),
            contents: &spec_lights_data,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let chunk_grid = match geometry.chunk_light_list {
            Some(sec) => ChunkGrid::from_section(sec),
            None => ChunkGrid::fallback(),
        };
        let chunk_grid_info_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Chunk Grid Info Uniform"),
            contents: &chunk_grid.grid_info,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let chunk_grid_offsets_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Chunk Grid Offset Table"),
                contents: &chunk_grid.offset_table,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
        let chunk_grid_indices_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Chunk Grid Index List"),
                contents: &chunk_grid.index_list,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });

        full.lighting_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lighting Bind Group"),
            layout: &full.lighting_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: full.lights_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: full.influence_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: spec_lights_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: chunk_grid_info_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: chunk_grid_offsets_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: chunk_grid_indices_buffer.as_entire_binding(),
                },
            ],
        });

        // --- SH volume, sh_compose, lightmap, animated lightmap ---
        full.sh_volume_resources = ShVolumeResources::new(
            device,
            queue,
            ShVolumeSections {
                sh: geometry.sh_volume,
                direct: geometry.direct_sh_volume,
                direct_delta: geometry.direct_sh_delta_volumes,
                animated_direct_delta: geometry.animated_direct_sh_delta_volumes,
            },
            // Runtime-spawned lights append after the full-authored prefix.
            geometry.lights.len() + RUNTIME_DYNAMIC_LIGHT_RESERVE,
            full.probe_occlusion_enabled,
        );

        // Rebuild the mesh group-2 dynamic-direct light bind group over the
        // just-reallocated runtime buffers — the `is_dynamic`-filtered
        // `lights_buffer` (b0), the fresh `influence_buffer` (b1), and the new
        // `sh_volume_resources` scripted-descriptor (b2) / anim-sample (b3)
        // buffers. The forward `lighting_bind_group` above is rebuilt for the same
        // reason; this mirrors it for the mesh pass so a level swap does not leave
        // the mesh group-2 bind group dangling at the prior level's buffers.
        // b5–b8 re-reference the SAME pool-owned shadow resources (stable for the
        // renderer's lifetime — the pools are never recreated), supplied here so the
        // shadow bindings rebind alongside the reallocated b0–b4. The cube view is
        // `Some` iff `cube_shadow_pool` is present (the `Some`-iff-layout invariant).
        let cube_sampling_view = full.cube_shadow_pool.as_ref().map(|p| &p.sampling_view);
        full.mesh_pass.rebuild_light_bind_group(
            device,
            &full.lights_buffer,
            &full.influence_buffer,
            &full.sh_volume_resources.scripted_light_descriptors,
            &full.sh_volume_resources.animation.anim_samples,
            &full.spot_shadow_pool.array_view,
            &full.spot_shadow_pool.compare_sampler,
            &full.spot_shadow_pool.matrices_buffer,
            cube_sampling_view,
        );
        full.kinematic_brush.rebuild_light_bind_group(
            device,
            &full.lights_buffer,
            &full.influence_buffer,
            &full.sh_volume_resources.scripted_light_descriptors,
            &full.sh_volume_resources.animation.anim_samples,
            &full.spot_shadow_pool.array_view,
            &full.spot_shadow_pool.compare_sampler,
            &full.spot_shadow_pool.matrices_buffer,
            cube_sampling_view,
        );

        full.sdf_atlas_resources = SdfAtlasResources::new(device, queue, geometry.sdf_atlas);
        full.lightmap_mode = geometry.lightmap_mode;
        let compose_sh_volume = geometry
            .sh_volume
            .filter(|_| full.sh_volume_resources.present);
        let compose_delta_sh_volumes = geometry
            .delta_sh_volumes
            .filter(|_| full.sh_volume_resources.present);
        full.sh_compose = ShComposeResources::new(
            device,
            &full.sh_volume_resources,
            compose_sh_volume,
            compose_delta_sh_volumes,
            &full.uniform_bind_group_layout,
        );
        full.direct_sh_compose = DirectShComposeResources::new(
            device,
            &full.sh_volume_resources.direct,
            &full.sh_volume_resources.animation,
            geometry.direct_sh_delta_volumes,
            geometry.animated_direct_sh_delta_volumes,
            &full.promoted_static_weight_buffer,
            &full.uniform_bind_group_layout,
        );
        #[cfg(feature = "dev-tools")]
        {
            full.sh_delta_volumes_meta = collect_delta_volume_meta(geometry.delta_sh_volumes);
            // Atlas shape (hence readback buffer size) changes per level — rebuild.
            full.sh_probe_readback = sh_diagnostics::ShProbeReadback::new(
                device,
                full.sh_volume_resources.grid_dimensions,
                full.sh_volume_resources.atlas_dimensions,
                full.sh_volume_resources.tile_dimension,
                full.sh_volume_resources.tile_border,
                full.sh_volume_resources.atlas_tiles_per_row,
                full.sh_volume_resources.tiles_per_layer,
                full.sh_volume_resources.atlas_layer_count,
            );
        }

        let lightmap_bgl = crate::lighting::lightmap::bind_group_layout(device);
        let animated_lm_debug = animated_lightmap::AnimatedLmDebugConfig::from_env();
        let bvh_leaves: Vec<postretro_render_data::geometry::BvhLeaf> = geometry.bvh.leaves.clone();
        // Match the animated atlas to the static lightmap atlas the same way the
        // constructor does — one resolver, one device limit, guaranteed-equal
        // dimensions (see `usable_atlas_dimensions`).
        let lightmap_atlas_dimensions = crate::lighting::lightmap::usable_atlas_dimensions(
            geometry.lightmap,
            device.limits().max_texture_dimension_2d,
            device.limits().max_texture_array_layers,
        );
        let slot_to_static_layer = geometry
            .animated_light_weight_maps
            .map_or(&[][..], |section| section.slot_to_static_layer.as_slice());

        let animated_lightmap = animated_lightmap::with_dummy_fallback(
            animated_lightmap::AnimatedLightmapResources::new(
                device,
                geometry.animated_light_weight_maps,
                geometry.animated_light_chunks,
                &bvh_leaves,
                &full.sh_volume_resources.animation,
                &full.uniform_bind_group_layout,
                lightmap_atlas_dimensions,
                animated_lm_debug,
            ),
            || {
                animated_lightmap::AnimatedLightmapResources::dummy(
                    device,
                    &full.sh_volume_resources.animation,
                    &full.uniform_bind_group_layout,
                    animated_lm_debug,
                )
            },
            "animated lightmap install",
        );
        let installed_slot_to_static_layer = animated_lightmap::installed_slot_to_static_layer(
            animated_lightmap.is_active(),
            slot_to_static_layer,
        );
        full.lightmap_resources = LightmapResources::new(
            device,
            queue,
            geometry.lightmap,
            geometry.shadowmask_atlas,
            &lightmap_bgl,
            &animated_lightmap.forward_view,
            &animated_lightmap.direction_forward_view,
            installed_slot_to_static_layer,
        );
        full.shadowmask_present = full.lightmap_resources.shadowmask_present;
        full.animated_lightmap = animated_lightmap;

        // SDF half-res shadow pass — rebind to the freshly-loaded SH
        // depth-moment texture + static-light buffers. The pass itself is always
        // allocated; the dispatch is gated on `sdf_atlas_resources.present`,
        // which `install_level_geometry` may have just flipped.
        let sdf_shadow_sh_grid =
            build_sdf_shadow_sh_grid(geometry.sh_volume, full.sh_volume_resources.present);
        full.sdf_shadow_pass.rebuild_for_level(
            device,
            &full.depth_view,
            full.sh_volume_resources.make_depth_moment_view(),
            sdf_shadow::SdfShadowLightBuffers {
                spec_lights: &spec_lights_buffer,
                chunk_grid_info: &chunk_grid_info_buffer,
                chunk_offsets: &chunk_grid_offsets_buffer,
                chunk_indices: &chunk_grid_indices_buffer,
            },
            sdf_shadow_sh_grid,
        );

        // --- BVH + compute cull ---
        full.bvh_leaves = bvh_leaves;
        // Per-cell draw index for the candidate-cull path. Cloned alongside the
        // BVH leaves; the empty-geometry install path clears it to `None`, so
        // `release_level_resources` drops it for free.
        full.cell_draw_index = geometry.cell_draw_index.cloned();
        // Reset per-level so a corrupt index on a later level still warns once.
        full.candidate_cull_oor_logged = false;
        full.compute_cull = if !full.bvh_leaves.is_empty() {
            Some(ComputeCullPipeline::new(
                device,
                geometry.bvh,
                has_multi_draw_indirect,
            ))
        } else {
            None
        };
        // Rebuild the candidate-cull path in lockstep with `compute_cull`, sized
        // to the freshly-installed leaf count. Empty-geometry install → `None`,
        // so `release_level_resources` drops it for free.
        full.candidate_cull = full
            .compute_cull
            .as_ref()
            .map(|c| crate::candidate_cull::CandidateCullPipeline::new(device, c.total_leaves()));

        // Rebuild both shadow cull owners against the freshly-uploaded BVH
        // buffers — their per-region bind groups reference the camera cull's
        // node/leaf storage, so a stale reference would point at the old BVH.
        // Spot: one region per pool slot. Cube: one region per (slot, face)
        // layer, only when the cube pool exists (adapter CUBE_ARRAY_TEXTURES).
        full.shadow_cull = full.compute_cull.as_ref().map(|c| {
            crate::shadow_cull::ShadowCullPipeline::new(
                device,
                c.node_buffer(),
                c.leaf_buffer(),
                c.total_leaves(),
                c.bucket_ranges().to_vec(),
                c.has_multi_draw_indirect(),
                crate::lighting::spot_shadow::SHADOW_POOL_SIZE,
            )
        });
        full.cube_shadow_cull = if full.cube_shadow_pool.is_some() {
            full.compute_cull.as_ref().map(|c| {
                crate::shadow_cull::ShadowCullPipeline::new(
                    device,
                    c.node_buffer(),
                    c.leaf_buffer(),
                    c.total_leaves(),
                    c.bucket_ranges().to_vec(),
                    c.has_multi_draw_indirect(),
                    crate::lighting::cube_shadow::CUBE_COUNT
                        * crate::lighting::cube_shadow::CUBE_FACES,
                )
            })
        } else {
            None
        };

        full.has_geometry = has_geometry;
        full.last_lights_upload.clear();
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
        clear_kinematic_mover_frame_state(
            &mut full.kinematic_mover_draws,
            &mut full.kinematic_mover_shadow_draws,
            &mut full.mover_occluder_aabbs,
        );

        if has_geometry {
            log::info!(
                "[Renderer] Geometry installed: {} indices, bvh_leaves={}",
                full.index_count,
                full.bvh_leaves.len(),
            );
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
