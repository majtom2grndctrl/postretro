// Renderer resource lifecycle: level geometry/texture/material upload and
// model loading.
// See: context/lib/resource_management.md

use super::*;

impl Renderer {
    /// First caller's `spec_intensity` and `lifetime` win — per-collection, not per-emitter.
    pub fn register_smoke_collection(
        &mut self,
        collection: &str,
        frames: &[SpriteFrame],
        spec_intensity: f32,
        lifetime: f32,
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
            frames,
            spec_intensity,
            lifetime,
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
            sdf_atlas: None,
            lightmap_mode: crate::prl::LightmapMode::default(),
            cell_draw_index: None,
            texture_materials: &empty_materials,
        };
        self.install_level_geometry(&empty_geometry);

        self.full_mut().smoke_pass.clear_collections();
        self.full_mut().mesh_pass.release_level_resources();
        self.full_mut().mesh_draws.clear();
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
        let (level_lights, dynamic_influences) =
            filter_dynamic_lights(geometry.lights, geometry.light_influences);
        let (shadow_candidate_lights, _) =
            filter_entity_shadow_candidates(geometry.lights, geometry.light_influences);
        full.light_count = level_lights.len() as u32;

        let lights_data = if !level_lights.is_empty() {
            pack_lights(&level_lights)
        } else {
            vec![0u8; GPU_LIGHT_SIZE]
        };
        let lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Direct Lights Storage Buffer"),
            contents: &lights_data,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        full.lights_buffer = lights_buffer;
        full.level_lights = level_lights;
        full.shadow_candidate_lights = shadow_candidate_lights;

        let influence_data = if !dynamic_influences.is_empty() {
            influence::pack_influence(&dynamic_influences)
        } else {
            vec![0u8; 16]
        };
        let influence_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Light Influence Storage Buffer"),
            contents: &influence_data,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let spec_lights_data = {
            let packed = pack_spec_lights(geometry.lights);
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
                    resource: influence_buffer.as_entire_binding(),
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
            geometry.sh_volume,
            geometry.direct_sh_volume,
            full.level_lights.len(),
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
            &influence_buffer,
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
        #[cfg(feature = "dev-tools")]
        {
            full.sh_delta_volumes_meta = collect_delta_volume_meta(geometry.delta_sh_volumes);
            // Atlas dims (hence readback buffer size) change per level — rebuild.
            full.sh_probe_readback = sh_diagnostics::ShProbeReadback::new(
                device,
                full.sh_volume_resources.grid_dimensions,
                full.sh_volume_resources.atlas_dimensions,
                full.sh_volume_resources.tile_dimension,
                full.sh_volume_resources.tile_border,
                full.sh_volume_resources.atlas_tiles_per_row,
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

        let animated_lightmap_result = animated_lightmap::AnimatedLightmapResources::new(
            device,
            geometry.animated_light_weight_maps,
            geometry.animated_light_chunks,
            &bvh_leaves,
            &full.sh_volume_resources.animation,
            &full.uniform_bind_group_layout,
            lightmap_atlas_dimensions,
            animated_lm_debug,
        );
        match animated_lightmap_result {
            Ok(al) => {
                full.lightmap_resources = LightmapResources::new(
                    device,
                    queue,
                    geometry.lightmap,
                    &lightmap_bgl,
                    &al.forward_view,
                    &al.direction_forward_view,
                );
                full.animated_lightmap = al;
            }
            Err(msg) => {
                log::error!(
                    "[Renderer] animated lightmap install failed: {msg} — level may render without lightmap"
                );
            }
        }

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

        // Rebuild the shadow cull owner against the freshly-uploaded BVH
        // buffers — its per-slot bind groups reference the camera cull's
        // node/leaf storage, so a stale reference would point at the old BVH.
        full.shadow_cull = full.compute_cull.as_ref().map(|c| {
            crate::shadow_cull::ShadowCullPipeline::new(
                device,
                c.node_buffer(),
                c.leaf_buffer(),
                c.total_leaves(),
                c.bucket_ranges().to_vec(),
                c.has_multi_draw_indirect(),
            )
        });

        full.has_geometry = has_geometry;
        full.last_lights_upload.clear();
        full.lights_pack_scratch.clear();
        full.light_effective_brightness.clear();
        full.stored_texture_materials = geometry.texture_materials.to_vec();

        if has_geometry {
            log::info!(
                "[Renderer] Geometry installed: {} indices, bvh_leaves={}",
                full.index_count,
                full.bvh_leaves.len(),
            );
        }
    }
}
