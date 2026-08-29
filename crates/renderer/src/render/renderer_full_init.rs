// Full-phase renderer construction: every steady-state pipeline/pass/resource
// built from boot state (the boot phase lives in `renderer_init.rs`).
// See: context/lib/rendering_pipeline.md

use super::renderer_types::{FullRenderer, PromotedStaticLightState};
use super::*;

/// Full-phase construction: builds every steady-state pipeline/pass/resource from
/// boot state alone (no level loaded — `geometry = None` throughout). Factored out
/// of `Renderer::new` so the boot splash presents before this runs. Pure function
/// of its arguments, which makes `finish_full_init` restartable across surface
/// recreation.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn build_full_renderer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surface_format: wgpu::TextureFormat,
    surface_width: u32,
    surface_height: u32,
    has_multi_draw_indirect: bool,
    cube_array_supported: bool,
    bloom_render_profile: BloomRenderProfile,
) -> Result<FullRenderer> {
    // Dummy buffers until `install_level_geometry` replaces them.
    let geometry: Option<&LevelGeometry> = None;
    // Surface dimensions captured from the live boot config so resize-then-finish
    // (surface recreation) builds the full renderer at the current size.
    struct SurfaceConfigDims {
        width: u32,
        height: u32,
    }
    let surface_config = SurfaceConfigDims {
        width: surface_width,
        height: surface_height,
    };

    let has_geometry = geometry.is_some_and(|g| !g.vertices.is_empty() && !g.indices.is_empty());

    let WorldVertexBuffers {
        vertex_buffer,
        index_buffer,
        index_count,
        wireframe_index_buffer,
        wireframe_index_count,
    } = build_world_vertex_buffers(device, geometry);

    let view_proj =
        build_default_view_projection(surface_config.width as f32 / surface_config.height as f32);
    let full_lights = geometry.map(|g| g.lights).unwrap_or(&[]);
    let full_influences = geometry.map(|g| g.light_influences).unwrap_or(&[]);
    let filtered_level_lights = filter_dynamic_lights(full_lights, full_influences);
    let level_lights = filtered_level_lights.lights;
    let dynamic_influences = filtered_level_lights.influences;
    let level_light_source_indices = filtered_level_lights.source_indices;
    let entity_shadow_indices = geometry.map(|g| g.entity_shadow_lights).unwrap_or(&[]);
    let selected_static = filter_selected_static_entity_shadow_lights(
        full_lights,
        full_influences,
        entity_shadow_indices,
    );
    let filtered_shadow_candidates = filter_entity_shadow_candidates_with_selection(
        full_lights,
        full_influences,
        entity_shadow_indices,
    );
    let shadow_candidate_lights = filtered_shadow_candidates.lights;
    let shadow_candidate_influences = filtered_shadow_candidates.influences;
    let shadow_candidate_source_indices = filtered_shadow_candidates.source_indices;
    let shadow_candidate_selection_indices = filtered_shadow_candidates.selection_indices;
    let light_count = level_lights.len() as u32;
    let ambient_floor = DEFAULT_AMBIENT_FLOOR;
    let sh_fast_env = std::env::var("POSTRETRO_SH_FAST").ok();
    let probe_occlusion_enabled =
        sh_volume::probe_occlusion_seed_from_fast_env(sh_fast_env.as_deref());
    let uniform_data = build_initial_uniform_data(view_proj, ambient_floor, light_count);

    let UniformBindGroups {
        uniform_buffer,
        uniform_bind_group_layout,
        uniform_bind_group,
        texture_bind_group_layout,
        lighting_bind_group_layout,
    } = build_uniform_bind_groups(device, &uniform_data);

    for (idx, light) in level_lights.iter().enumerate() {
        if light.is_dynamic && light.light_type == postretro_level_loader::LightType::Directional {
            log::warn!(
                "[Renderer] Dynamic directional light (light_sun) at index {} found — not supported. \
                     Will render unshadowed (diffuse + specular only).",
                idx
            );
        }
    }

    // BGL owned here so forward pipeline layout and shadow pool bind group share it.
    // The BGL carries bindings 3 (SDF shadow factor) and 4 (scene depth) — both
    // owned outside the pool. Binding 5 (point-light cube-array depth) is present
    // only when `cube_array_supported`; the shared BGL, the forward + fog
    // pipelines, and the shader variants all key off the same flag. The pool
    // itself is built later (after depth_view + sdf_shadow_pass exist) so its
    // bind group can reference those targets directly at construction.
    let spot_shadow_bgl = SpotShadowPool::bind_group_layout(device, cube_array_supported);

    let LightingResources {
        lights_buffer,
        influence_buffer,
        spec_lights_buffer,
        chunk_grid_info_buffer,
        chunk_grid_offsets_buffer,
        chunk_grid_indices_buffer,
        lighting_bind_group,
    } = build_lighting_bind_group(
        device,
        &lighting_bind_group_layout,
        &level_lights,
        &dynamic_influences,
        geometry,
    );

    // Sampler pool seeded with the placeholder's mip count of `1`. The
    // pool grows in `install_textures` once `LoadedTexture::mip_count`
    // values arrive from the .prm sidecars. Placeholders always pick up
    // the `1` entry; never miss this lookup.
    let mut mip_count_aniso_samplers: HashMap<u32, wgpu::Sampler> = HashMap::new();
    mip_count_aniso_samplers.insert(1, create_mip_aniso_sampler(device, 1));
    let mut mip_count_character_model_samplers: HashMap<u32, wgpu::Sampler> = HashMap::new();
    mip_count_character_model_samplers.insert(1, create_mip_character_model_sampler(device, 1));

    // Construct an initial placeholder bind group so the world pipeline
    // has a bind group bound even before a level loads. Replaced wholesale
    // by `install_textures` when a `.prl` payload arrives.
    let (loaded_textures, gpu_textures) = build_placeholder_textures(
        device,
        queue,
        &texture_bind_group_layout,
        &mip_count_aniso_samplers,
    );

    let bvh_leaves: Vec<postretro_render_data::geometry::BvhLeaf> =
        geometry.map(|g| g.bvh.leaves.clone()).unwrap_or_default();
    let cell_draw_index: Option<postretro_level_loader::CellDrawIndex> =
        geometry.and_then(|g| g.cell_draw_index.cloned());
    let compute_cull = geometry
        .filter(|g| !g.bvh.leaves.is_empty())
        .map(|g| ComputeCullPipeline::new(device, g.bvh, has_multi_draw_indirect));
    // Candidate-cull path — built in lockstep with `compute_cull`, sized to
    // the same leaf count so it writes the same global slots.
    let candidate_cull = compute_cull
        .as_ref()
        .map(|c| crate::candidate_cull::CandidateCullPipeline::new(device, c.total_leaves()));
    // Sibling shadow cull owners share the camera cull's read-only BVH
    // node/leaf buffers (uploaded once). Built/rebuilt in lockstep with it.
    // Spot instance: one region per pool slot, planes from the slot's cone
    // matrix. Cube instance: one region per (slot, face), planes from that
    // face's 90° perspective matrix — only when the cube pool exists (adapter
    // has CUBE_ARRAY_TEXTURES), since without it no cube depth pass ever runs.
    let shadow_cull = compute_cull.as_ref().map(|c| {
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
    let cube_shadow_cull = if cube_array_supported {
        compute_cull.as_ref().map(|c| {
            crate::shadow_cull::ShadowCullPipeline::new(
                device,
                c.node_buffer(),
                c.leaf_buffer(),
                c.total_leaves(),
                c.bucket_ranges().to_vec(),
                c.has_multi_draw_indirect(),
                crate::lighting::cube_shadow::CUBE_COUNT * crate::lighting::cube_shadow::CUBE_FACES,
            )
        })
    } else {
        None
    };

    let (_depth_texture, depth_view) =
        create_depth_texture(device, surface_config.width, surface_config.height);

    // Post-scene compositor seam: a linear HDR `scene_color` target + sRGB
    // resolve. The scene target is independent from the swapchain format.
    let screen_effects = ScreenEffectsPass::new(
        device,
        surface_config.width,
        surface_config.height,
        surface_format,
    );
    let bloom = BloomPass::new(
        device,
        surface_config.width,
        surface_config.height,
        screen_effects.scene_color_texture(),
        bloom_render_profile,
    );

    let scripted_light_capacity = full_lights.len() + RUNTIME_DYNAMIC_LIGHT_RESERVE;
    let sh_volume_resources = ShVolumeResources::new(
        device,
        queue,
        ShVolumeSections {
            sh: geometry.and_then(|g| g.sh_volume),
            direct: geometry.and_then(|g| g.direct_sh_volume),
            direct_delta: geometry.and_then(|g| g.direct_sh_delta_volumes),
            animated_direct_delta: geometry.and_then(|g| g.animated_direct_sh_delta_volumes),
        },
        // Runtime-spawned lights append after the full-authored prefix.
        scripted_light_capacity,
        probe_occlusion_enabled,
    );

    let sdf_atlas_resources =
        SdfAtlasResources::new(device, queue, geometry.and_then(|g| g.sdf_atlas));
    let lightmap_mode = geometry
        .map(|g| g.lightmap_mode)
        .unwrap_or(postretro_level_loader::LightmapMode::Shadowed);

    let compose_sh_volume = geometry
        .and_then(|g| g.sh_volume)
        .filter(|_| sh_volume_resources.present);
    let compose_delta_sh_volumes = geometry
        .and_then(|g| g.delta_sh_volumes)
        .filter(|_| sh_volume_resources.present);
    let sh_compose = ShComposeResources::new(
        device,
        &sh_volume_resources,
        compose_sh_volume,
        compose_delta_sh_volumes,
        &uniform_bind_group_layout,
    );

    #[cfg(feature = "dev-tools")]
    let sh_delta_volumes_meta =
        collect_delta_volume_meta(geometry.and_then(|g| g.delta_sh_volumes));

    #[cfg(feature = "dev-tools")]
    let sh_probe_readback = sh_diagnostics::ShProbeReadback::new(
        device,
        sh_volume_resources.grid_dimensions,
        sh_volume_resources.atlas_dimensions,
        sh_volume_resources.tile_dimension,
        sh_volume_resources.tile_border,
        sh_volume_resources.atlas_tiles_per_row,
        sh_volume_resources.tiles_per_layer,
        sh_volume_resources.atlas_layer_count,
    );

    let animated_lm_debug = animated_lightmap::AnimatedLmDebugConfig::from_env();
    // Source the animated atlas size from the same resolver the static
    // lightmap texture uses, so the two atlases are guaranteed to match (the
    // compose pass writes at absolute static-atlas coordinates; the forward
    // pass samples both with one normalized lightmap_uv).
    let lightmap_atlas_dimensions = crate::lighting::lightmap::usable_atlas_dimensions(
        geometry.and_then(|g| g.lightmap),
        device.limits().max_texture_dimension_2d,
        device.limits().max_texture_array_layers,
    );
    let slot_to_static_layer = geometry
        .and_then(|g| g.animated_light_weight_maps)
        .map_or(&[][..], |section| section.slot_to_static_layer.as_slice());
    let animated_lightmap = animated_lightmap::with_dummy_fallback(
        animated_lightmap::AnimatedLightmapResources::new(
            device,
            geometry.and_then(|g| g.animated_light_weight_maps),
            geometry.and_then(|g| g.animated_light_chunks),
            &bvh_leaves,
            &sh_volume_resources.animation,
            &uniform_bind_group_layout,
            lightmap_atlas_dimensions,
            animated_lm_debug,
        ),
        || {
            animated_lightmap::AnimatedLightmapResources::dummy(
                device,
                &sh_volume_resources.animation,
                &uniform_bind_group_layout,
                animated_lm_debug,
            )
        },
        "animated lightmap initialization",
    );
    let installed_slot_to_static_layer = animated_lightmap::installed_slot_to_static_layer(
        animated_lightmap.is_active(),
        slot_to_static_layer,
    );

    // Group 4: lightmap atlas. Animated-contribution atlas at binding 3 (real or 1×1 zero dummy).
    let lightmap_bind_group_layout = crate::lighting::lightmap::bind_group_layout(device);
    let lightmap_resources = LightmapResources::new(
        device,
        queue,
        geometry.and_then(|g| g.lightmap),
        geometry.and_then(|g| g.shadowmask_atlas),
        &lightmap_bind_group_layout,
        &animated_lightmap.forward_view,
        &animated_lightmap.direction_forward_view,
        installed_slot_to_static_layer,
    );
    let shadowmask_present = lightmap_resources.shadowmask_present;

    // SDF half-res shadow pass (Task 4). Always allocated — dispatch is
    // gated on `sdf_atlas_resources.present`. Owns the half-res factor
    // target and its own group-1 bind group.
    let sdf_shadow_sh_grid = build_sdf_shadow_sh_grid(
        geometry.and_then(|g| g.sh_volume),
        sh_volume_resources.present,
    );
    let sdf_shadow_pass = SdfShadowPass::new(
        device,
        &sdf_atlas_resources.bind_group_layout,
        &depth_view,
        sh_volume_resources.make_depth_moment_view(),
        sdf_shadow::SdfShadowLightBuffers {
            spec_lights: &spec_lights_buffer,
            chunk_grid_info: &chunk_grid_info_buffer,
            chunk_offsets: &chunk_grid_offsets_buffer,
            chunk_indices: &chunk_grid_indices_buffer,
        },
        sdf_shadow_sh_grid,
        surface_config.width,
        surface_config.height,
    );

    // Cube point-shadow pool — built before the spot pool because the
    // spot-shadow bind group (the shared group-5 BGL) references the cube
    // sampling view at binding 5. Disabled (None) when the adapter lacks
    // CUBE_ARRAY_TEXTURES — in that case binding 5 is omitted from the BGL and
    // NO cube view (not even a dummy) is created, since a `CubeArray` view
    // itself requires the feature. `cube_shadow_pool.is_some()` therefore
    // mirrors `cube_array_supported` exactly.
    let cube_shadow_pool =
        crate::lighting::cube_shadow::CubeShadowPool::new(device, cube_array_supported);
    let cube_sampling_view = cube_shadow_pool.as_ref().map(|p| &p.sampling_view);

    // Now that the SDF shadow factor target + scene depth view both
    // exist, build the spot-shadow pool — its bind group references
    // both targets at bindings 3/4 and (when present) the cube sampling view
    // at binding 5. See `SpotShadowPool::new` docs.
    let spot_shadow_pool = SpotShadowPool::new(
        device,
        &spot_shadow_bgl,
        &sdf_shadow_pass.shadow_view,
        &depth_view,
        cube_sampling_view,
    );
    {
        use crate::lighting::spot_shadow::{
            SHADOW_DEPTH_FORMAT, SHADOW_MAP_RESOLUTION, SHADOW_POOL_SIZE,
        };
        // Depth32Float = 4 B/texel; MiB = bytes >> 20. Derived from the consts
        // so the log can't drift from the actual pool size (was a stale literal).
        let vram_mib = (SHADOW_POOL_SIZE as u64
            * SHADOW_MAP_RESOLUTION as u64
            * SHADOW_MAP_RESOLUTION as u64
            * 4)
            >> 20;
        log::info!(
            "[Renderer] Spot shadow pool initialized ({} × {}×{} {:?} = {} MiB VRAM)",
            SHADOW_POOL_SIZE,
            SHADOW_MAP_RESOLUTION,
            SHADOW_MAP_RESOLUTION,
            SHADOW_DEPTH_FORMAT,
            vram_mib,
        );
    }

    let RendererPipelines {
        pipeline,
        wireframe_cull_status_layout,
        wireframe_cull_status_pipeline,
        wireframe_visible_pipeline,
        depth_prepass_pipeline,
        shadow_vs_bgl,
        shadow_depth_pipeline,
    } = build_renderer_pipelines(
        device,
        &uniform_bind_group_layout,
        &texture_bind_group_layout,
        &lighting_bind_group_layout,
        &sh_volume_resources.bind_group_layout,
        &lightmap_bind_group_layout,
        &spot_shadow_bgl,
        cube_array_supported,
    );

    let ShadowVsResources {
        shadow_vs_stride,
        shadow_vs_uniform_buffer,
        shadow_vs_bind_group,
        cube_shadow_vs_uniform_buffer,
        cube_shadow_vs_bind_group,
    } = build_shadow_vs_resources(device, &shadow_vs_bgl);

    // GPU timing is enabled only when requested AND the device was created
    // with both timestamp features its pass- and encoder-level brackets use.
    // Re-derive from the device's granted features so `build_full_renderer`
    // stays a pure function of boot state.
    let enable_gpu_timing = std::env::var("POSTRETRO_GPU_TIMING").ok().as_deref() == Some("1")
        && gpu_timing_features_supported(device.features());
    let frame_timing = build_frame_timing(device, queue, enable_gpu_timing);

    // See: context/lib/rendering_pipeline.md §7.4
    let smoke_pass = SmokePass::new(
        device,
        DEPTH_FORMAT,
        &uniform_bind_group_layout,
        &lighting_bind_group_layout,
        &sh_volume_resources.bind_group_layout,
    );

    // Skinned-mesh pass: reuses the camera (group 0) + material (group 1)
    // layouts. `upload_identity_palette` pre-fills the palette at startup so
    // an un-sampled run renders in bind pose. Each frame `plan_and_upload`
    // samples every instance's clip into its palette run before the shadow
    // depth loop; `record_draws` then records the forward draw.
    let mut mesh_pass = mesh_pass::MeshPass::new(
        device,
        DEPTH_FORMAT,
        // The depth-only skinned pipeline writes the shadow-map depth format
        // and binds the world spot-shadow `shadow_vs_bgl` at group 0 (the
        // per-render light-space matrix, dynamic-offset per slot).
        crate::lighting::spot_shadow::SHADOW_DEPTH_FORMAT,
        &uniform_bind_group_layout,
        &texture_bind_group_layout,
        &shadow_vs_bgl,
        // Mesh group 4 uses the SUPERSET layout (shared SH entries + the
        // mesh-only dynamic-direct params uniform at binding 16).
        &sh_volume_resources.mesh_bind_group_layout,
        // Cube-array support pins the `Some`-iff-layout invariant: the mesh
        // group-2 BGL carries the b8 cube entry iff this is true, and the
        // no-cube shader strip is applied to the mesh source when it is false.
        cube_array_supported,
    );
    mesh_pass.upload_identity_palette(queue);
    // Build the mesh group-2 dynamic-direct light bind group over the SAME
    // runtime buffers the forward `lighting_bind_group` binds: the
    // `is_dynamic`-filtered `lights_buffer` (b0), the influence-volume buffer
    // (b1), and forward's scripted-descriptor (b2) / anim-sample (b3) buffers.
    // Rebuilt on level load wherever those buffers reallocate (see
    // `set_geometry`).
    // b5–b8 alias the SAME pool-owned shadow resources forward binds at its
    // group 5: the spot pool's D2-array depth view (b5), its comparison
    // sampler (b6), its light-space-matrices uniform buffer (b7), and the cube
    // pool's `CubeArray` sampling view (b8 — `Some` iff `cube_array_supported`,
    // the `Some`-iff-layout invariant). These pool resources are stable for the
    // renderer's lifetime (the pools are never recreated), so they only ever
    // rebind here alongside the b0–b4 reallocation rebind on level load.
    mesh_pass.rebuild_light_bind_group(
        device,
        &lights_buffer,
        &influence_buffer,
        &sh_volume_resources.scripted_light_descriptors,
        &sh_volume_resources.animation.anim_samples,
        &spot_shadow_pool.array_view,
        &spot_shadow_pool.compare_sampler,
        &spot_shadow_pool.matrices_buffer,
        cube_shadow_pool.as_ref().map(|p| &p.sampling_view),
    );
    let mut kinematic_brush = kinematic_brush::KinematicBrushPass::new(
        device,
        DEPTH_FORMAT,
        &uniform_bind_group_layout,
        &texture_bind_group_layout,
        &sh_volume_resources.mesh_bind_group_layout,
        cube_array_supported,
    );
    kinematic_brush.rebuild_light_bind_group(
        device,
        &lights_buffer,
        &influence_buffer,
        &sh_volume_resources.scripted_light_descriptors,
        &sh_volume_resources.animation.anim_samples,
        &spot_shadow_pool.array_view,
        &spot_shadow_pool.compare_sampler,
        &spot_shadow_pool.matrices_buffer,
        cube_shadow_pool.as_ref().map(|p| &p.sampling_view),
    );
    let rigid_occluder_depth = rigid_occluder_depth::RigidOccluderDepthPass::new(
        device,
        crate::lighting::spot_shadow::SHADOW_DEPTH_FORMAT,
        &shadow_vs_bgl,
        kinematic_brush.instance_transform_bind_group_layout(),
    );

    // Gameplay UI owns its quad pipeline, glyphon atlas/renderer, and white
    // texel. Boot splash rendering uses its separate lightweight pass.
    let ui = ui::UiPass::new(device, queue, SCENE_COLOR_FORMAT);

    let fog = FogPass::new(
        device,
        surface_config.width,
        surface_config.height,
        postretro_render_cpu::fog_volume::clamp_fog_pixel_scale(0),
        &depth_view,
        &uniform_bind_group_layout,
        &sh_volume_resources.bind_group_layout,
        &spot_shadow_bgl,
        cube_array_supported,
    );
    if has_geometry {
        log::info!(
            "[Renderer] Textured pipeline ready: {} indices, {} textures, bvh_leaves={}",
            index_count,
            gpu_textures.len(),
            bvh_leaves.len(),
        );
        log::info!(
            "[Renderer] Wireframe overlay pipeline ready: {} line indices",
            wireframe_index_count,
        );
    } else {
        log::info!("[Renderer] Pipeline ready (no geometry loaded)");
    }

    #[cfg(feature = "dev-tools")]
    let debug_lines =
        debug_lines::DebugLineRenderer::new(device, DEPTH_FORMAT, 1, &uniform_bind_group_layout);

    let promoted_static_weight_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Promoted Static Light Weights"),
        size: (entity_shadow_indices.len().max(1) * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let direct_sh_compose = DirectShComposeResources::new(
        device,
        &sh_volume_resources.direct,
        &sh_volume_resources.animation,
        geometry.and_then(|g| g.direct_sh_delta_volumes),
        geometry.and_then(|g| g.animated_direct_sh_delta_volumes),
        &promoted_static_weight_buffer,
        &uniform_bind_group_layout,
    );
    // Only allocate the promoted-slot depth cache when the map has a non-empty
    // entity-shadow selection; an empty/absent selection can never promote a
    // light, so the cache arrays would be pure wasted VRAM.
    let promoted_depth_cache = if entity_shadow_indices.is_empty() {
        None
    } else {
        Some(PromotedDepthCache::new(device))
    };

    Ok(FullRenderer {
        pipeline,
        depth_prepass_pipeline,
        frame_timing,
        vertex_buffer,
        index_buffer,
        index_count,
        uniform_buffer,
        uniform_bind_group,
        lighting_bind_group,
        influence_buffer,
        dynamic_light_capacity: level_lights.len() + RUNTIME_DYNAMIC_LIGHT_RESERVE,
        light_count,
        total_light_count: light_count,
        mesh_dynamic_time: 0.0,
        frame_light_term_mask: LightTermMask::ALL,
        kinematic_mover_draws: Vec::new(),
        kinematic_mover_shadow_draws: Vec::new(),
        mover_occluder_aabbs: Vec::new(),
        ambient_floor,
        indirect_scale: DEFAULT_INDIRECT_SCALE,
        dynamic_direct_scale: DEFAULT_DYNAMIC_DIRECT_SCALE,
        probe_occlusion_enabled,
        sh_volume_resources,
        sdf_atlas_resources,
        sdf_shadow_pass,
        lightmap_mode,
        #[cfg(feature = "dev-tools")]
        sh_delta_volumes_meta,
        #[cfg(feature = "dev-tools")]
        sh_probe_readback,
        #[cfg(feature = "dev-tools")]
        freeze_time: false,
        #[cfg(feature = "dev-tools")]
        frozen_time: 0.0,
        sh_compose,
        direct_sh_compose,
        lightmap_resources,
        animated_lightmap,
        lights_buffer,
        last_lights_upload: Vec::new(),
        last_influence_upload: Vec::new(),
        lights_pack_scratch: Vec::new(),
        influence_pack_scratch: Vec::new(),
        level_lights,
        level_light_source_indices,
        level_light_influences: dynamic_influences,
        entity_shadow_lights: selected_static.lights,
        entity_shadow_light_influences: selected_static.influences,
        entity_shadow_light_source_indices: selected_static.source_indices,
        entity_shadow_spec_light_indices: shadowmask::build_selection_spec_light_indices(
            full_lights,
            entity_shadow_indices,
        ),
        shadowmask_channels: geometry
            .and_then(|g| g.shadowmask_atlas)
            .map(|section| section.channels.clone())
            .unwrap_or_default(),
        shadowmask_present,
        forward_shadowmask_metadata_scratch: Vec::new(),
        shadow_candidate_lights,
        shadow_candidate_source_indices,
        shadow_candidate_selection_indices,
        shadow_candidate_influences,
        light_effective_brightness: Vec::new(),
        last_camera_position: Vec3::ZERO,
        last_view_proj: Mat4::IDENTITY,
        spot_shadow_pool,
        cube_shadow_pool,
        kinematic_brush,
        rigid_occluder_depth,
        promoted_static_states: vec![
            PromotedStaticLightState::default();
            entity_shadow_indices.len()
        ],
        promoted_static_records: Vec::new(),
        promoted_static_weights: vec![0.0; entity_shadow_indices.len()],
        promoted_static_weight_buffer,
        promoted_static_weight_scratch: Vec::new(),
        promoted_static_last_update_time: None,
        promoted_depth_cache,
        promoted_depth_cache_frame_plan: PromotedDepthCacheFramePlan::default(),
        promoted_depth_cache_promoted_count: 0,
        promoted_depth_cache_world_render_skips: 0,
        promoted_depth_cache_cull_dispatch_skips: 0,
        promoted_depth_cache_timing_open: false,
        #[cfg(feature = "dev-tools")]
        direct_sh_debug_override: DirectShDebugOverride::default(),
        #[cfg(feature = "dev-tools")]
        animated_direct_sh_debug_override: AnimatedDirectShDebugOverride::default(),
        cube_shadow_vs_uniform_buffer,
        cube_shadow_vs_bind_group,
        shadow_vs_uniform_buffer,
        shadow_vs_bind_group,
        shadow_depth_pipeline,
        shadow_vs_stride,
        depth_view,
        screen_effects,
        bloom,
        gpu_textures,
        bvh_leaves,
        cell_draw_index,
        compute_cull,
        candidate_cull,
        shadow_cull,
        cube_shadow_cull,
        wireframe_cull_status_pipeline,
        wireframe_visible_pipeline,
        wireframe_index_buffer,
        wireframe_index_count,
        wireframe_cull_status_bgl: wireframe_cull_status_layout,
        world_wireframe_mode: WorldWireframeMode::Off,
        wireframe_enabled: false,
        #[cfg(feature = "dev-tools")]
        debug_lines,
        #[cfg(feature = "dev-tools")]
        bvh_overlay: BvhOverlayState::default(),
        #[cfg(feature = "dev-tools")]
        cell_overlay: CellOverlayState::default(),
        #[cfg(feature = "dev-tools")]
        portal_overlay: PortalOverlayState::default(),
        #[cfg(feature = "dev-tools")]
        agent_overlay: AgentOverlayState::default(),
        #[cfg(feature = "dev-tools")]
        show_navmesh: false,
        light_term_mask: LightTermMask::ALL,
        sdf_shadow_mode: SdfShadowMode::On,
        sdf_force_visibility_one: std::env::var("POSTRETRO_SDF_FORCE_VISIBILITY_ONE")
            .ok()
            .as_deref()
            == Some("1"),
        spec_shadowmask_force_one: std::env::var("POSTRETRO_SPEC_SHADOWMASK_FORCE_ONE")
            .ok()
            .as_deref()
            == Some("1"),
        vsync_enabled: true,
        has_geometry,
        debug_frame: 0,
        debug_prev_bitmask: (u32::MAX, u32::MAX),
        debug_prev_vp_hash: u32::MAX,
        debug_prev_visible: ("init", usize::MAX),
        candidate_cull_oor_logged: false,
        camera_cull_diagnostics: crate::render::CameraCullDiagnostics::default(),
        spatial_diagnostics: crate::render::SpatialDiagnostics::default(),
        bvh_cull_diagnostics: None,
        shadow_debug_enabled: std::env::var("POSTRETRO_SHADOW_DEBUG").ok().as_deref() == Some("1"),
        shadow_debug_prev: (u128::MAX, u128::MAX, u32::MAX, u32::MAX),
        smoke_pass,
        mesh_pass,
        mesh_draws: Vec::new(),
        bone_palette_scratch: Vec::new(),
        mesh_overflow_last_warn: f32::NEG_INFINITY,
        spot_entity_occluders_submitted: 0,
        cube_entity_occluders_submitted: 0,
        promoted_entity_occluders_submitted: 0,
        ui,
        ui_images: ui::UiImageRegistry::default(),
        ui_snapshot: ui::UiReadSnapshot::default(),
        presentation_inputs: Vec::new(),
        ui_theme: ui::theme::UiTheme::engine_default(),
        ui_theme_generation: 0,
        fog,
        fog_cell_masks: None,
        active_fog_aabbs: Vec::new(),
        texture_bind_group_layout,
        lighting_bind_group_layout,
        mip_count_aniso_samplers,
        mip_count_character_model_samplers,
        loaded_textures,
        stored_texture_materials: Vec::new(),
        uniform_bind_group_layout,
        #[cfg(feature = "dev-tools")]
        debug_ui_gpu: None,
    })
}
