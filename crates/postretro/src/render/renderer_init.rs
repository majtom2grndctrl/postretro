// Renderer construction and GPU init: adapter/device setup, pipeline and
// bind-group creation, and the dev debug-UI bootstrap.
// See: context/lib/rendering_pipeline.md

use super::*;

impl Renderer {
    /// Geometry and textures installed later via `install_level_geometry` / `install_textures`.
    pub fn new(window: &Arc<Window>) -> Result<Self> {
        // Dummy buffers until `install_level_geometry` replaces them.
        let geometry: Option<&LevelGeometry> = None;
        let size = window.inner_size();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let surface = instance
            .create_surface(window.clone())
            .context("failed to create wgpu surface")?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .context("no suitable GPU adapter found")?;

        log::info!("[Renderer] GPU adapter: {}", adapter.get_info().name);

        let downlevel = adapter.get_downlevel_capabilities();
        let has_multi_draw_indirect = downlevel
            .flags
            .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION);
        if has_multi_draw_indirect {
            log::info!("[Renderer] Indirect execution supported (multi_draw_indexed_indirect)");
        } else {
            log::info!(
                "[Renderer] Indirect execution not supported — using singular draw_indexed_indirect fallback"
            );
        }

        // Cube-array support gates the dynamic point-light shadow pool. Absent →
        // the cube pool is disabled (None) and point shadows are cleanly off; the
        // spot path is entirely unaffected (no panic, no validation error).
        let cube_array_supported = downlevel
            .flags
            .contains(wgpu::DownlevelFlags::CUBE_ARRAY_TEXTURES);
        if cube_array_supported {
            log::info!("[Renderer] Cube-array textures supported (dynamic point shadows enabled)");
        } else {
            log::info!(
                "[Renderer] Cube-array textures unsupported — dynamic point-light shadows disabled"
            );
        }

        // FrameTiming=None → zero runtime cost when timing isn't requested or supported.
        let adapter_features = adapter.features();
        let gpu_timing_requested =
            std::env::var("POSTRETRO_GPU_TIMING").ok().as_deref() == Some("1");
        let gpu_timing_supported = adapter_features.contains(wgpu::Features::TIMESTAMP_QUERY);
        let enable_gpu_timing = gpu_timing_requested && gpu_timing_supported;
        // BC5-compressed normal maps are a hard requirement (not optional like
        // GPU timing): the .prm baker emits BC5 normal slots unconditionally.
        let (device, queue) = request_renderer_device(
            &adapter,
            cube_array_supported,
            enable_gpu_timing,
            gpu_timing_requested,
            gpu_timing_supported,
        )?;
        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: surface_caps.alpha_modes[0],
            desired_maximum_frame_latency: 2,
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);
        log::info!("[Renderer] vsync on");

        let has_geometry =
            geometry.is_some_and(|g| !g.vertices.is_empty() && !g.indices.is_empty());

        let WorldVertexBuffers {
            vertex_buffer,
            index_buffer,
            index_count,
            wireframe_index_buffer,
            wireframe_index_count,
        } = build_world_vertex_buffers(&device, geometry);

        let view_proj = build_default_view_projection(
            surface_config.width as f32 / surface_config.height as f32,
        );
        let full_lights = geometry.map(|g| g.lights).unwrap_or(&[]);
        let full_influences = geometry.map(|g| g.light_influences).unwrap_or(&[]);
        let (level_lights, dynamic_influences) =
            filter_dynamic_lights(full_lights, full_influences);
        let (shadow_candidate_lights, _) =
            filter_entity_shadow_candidates(full_lights, full_influences);
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
        } = build_uniform_bind_groups(&device, &uniform_data);

        for (idx, light) in level_lights.iter().enumerate() {
            if light.is_dynamic && light.light_type == crate::prl::LightType::Directional {
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
        let spot_shadow_bgl = SpotShadowPool::bind_group_layout(&device, cube_array_supported);

        let LightingResources {
            lights_buffer,
            influence_buffer,
            spec_lights_buffer,
            chunk_grid_info_buffer,
            chunk_grid_offsets_buffer,
            chunk_grid_indices_buffer,
            lighting_bind_group,
        } = build_lighting_bind_group(
            &device,
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
        mip_count_aniso_samplers.insert(1, create_mip_aniso_sampler(&device, 1));

        // Construct an initial placeholder bind group so the world pipeline
        // has a bind group bound even before a level loads. Replaced wholesale
        // by `install_textures` when a `.prl` payload arrives.
        let (loaded_textures, gpu_textures) = build_placeholder_textures(
            &device,
            &queue,
            &texture_bind_group_layout,
            &mip_count_aniso_samplers,
        );

        let bvh_leaves: Vec<crate::geometry::BvhLeaf> =
            geometry.map(|g| g.bvh.leaves.clone()).unwrap_or_default();
        let compute_cull = geometry
            .filter(|g| !g.bvh.leaves.is_empty())
            .map(|g| ComputeCullPipeline::new(&device, g.bvh, has_multi_draw_indirect));
        // Sibling shadow cull owner shares the camera cull's read-only BVH
        // node/leaf buffers (uploaded once). Built/rebuilt in lockstep with it.
        let shadow_cull = compute_cull.as_ref().map(|c| {
            crate::shadow_cull::ShadowCullPipeline::new(
                &device,
                c.node_buffer(),
                c.leaf_buffer(),
                c.total_leaves(),
                c.bucket_ranges().to_vec(),
                c.has_multi_draw_indirect(),
            )
        });

        let (_depth_texture, depth_view) =
            create_depth_texture(&device, surface_config.width, surface_config.height);

        // Post-scene compositor seam: `scene_color` offscreen target + identity
        // resolve. Allocated at the sRGB surface format / surface size /
        // single-sample for byte-identical resolve (see `screen_effects.rs`).
        let screen_effects = ScreenEffectsPass::new(
            &device,
            surface_config.width,
            surface_config.height,
            surface_format,
        );

        let sh_volume_resources = ShVolumeResources::new(
            &device,
            &queue,
            geometry.and_then(|g| g.sh_volume),
            geometry.and_then(|g| g.direct_sh_volume),
            level_lights.len(),
            probe_occlusion_enabled,
        );

        let sdf_atlas_resources =
            SdfAtlasResources::new(&device, &queue, geometry.and_then(|g| g.sdf_atlas));
        let lightmap_mode = geometry
            .map(|g| g.lightmap_mode)
            .unwrap_or(crate::prl::LightmapMode::Shadowed);

        let compose_sh_volume = geometry
            .and_then(|g| g.sh_volume)
            .filter(|_| sh_volume_resources.present);
        let compose_delta_sh_volumes = geometry
            .and_then(|g| g.delta_sh_volumes)
            .filter(|_| sh_volume_resources.present);
        let sh_compose = ShComposeResources::new(
            &device,
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
            &device,
            sh_volume_resources.grid_dimensions,
            sh_volume_resources.atlas_dimensions,
            sh_volume_resources.tile_dimension,
            sh_volume_resources.tile_border,
            sh_volume_resources.atlas_tiles_per_row,
        );

        let animated_lm_debug = animated_lightmap::AnimatedLmDebugConfig::from_env();
        // Source the animated atlas size from the same resolver the static
        // lightmap texture uses, so the two atlases are guaranteed to match (the
        // compose pass writes at absolute static-atlas coordinates; the forward
        // pass samples both with one normalized lightmap_uv).
        let lightmap_atlas_dimensions = crate::lighting::lightmap::usable_atlas_dimensions(
            geometry.and_then(|g| g.lightmap),
            device.limits().max_texture_dimension_2d,
        );
        let animated_lightmap = animated_lightmap::AnimatedLightmapResources::new(
            &device,
            geometry.and_then(|g| g.animated_light_weight_maps),
            geometry.and_then(|g| g.animated_light_chunks),
            &bvh_leaves,
            &sh_volume_resources.animation,
            &uniform_bind_group_layout,
            lightmap_atlas_dimensions,
            animated_lm_debug,
        )
        .map_err(|msg| anyhow::anyhow!("[Renderer] animated lightmap init failed: {msg}"))?;

        // Group 4: lightmap atlas. Animated-contribution atlas at binding 3 (real or 1×1 zero dummy).
        let lightmap_bind_group_layout = crate::lighting::lightmap::bind_group_layout(&device);
        let lightmap_resources = LightmapResources::new(
            &device,
            &queue,
            geometry.and_then(|g| g.lightmap),
            &lightmap_bind_group_layout,
            &animated_lightmap.forward_view,
            &animated_lightmap.direction_forward_view,
        );

        // SDF half-res shadow pass (Task 4). Always allocated — dispatch is
        // gated on `sdf_atlas_resources.present`. Owns the half-res factor
        // target and its own group-1 bind group.
        let sdf_shadow_sh_grid = build_sdf_shadow_sh_grid(
            geometry.and_then(|g| g.sh_volume),
            sh_volume_resources.present,
        );
        let sdf_shadow_pass = SdfShadowPass::new(
            &device,
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
            crate::lighting::cube_shadow::CubeShadowPool::new(&device, cube_array_supported);
        let cube_sampling_view = cube_shadow_pool.as_ref().map(|p| &p.sampling_view);

        // Now that the SDF shadow factor target + scene depth view both
        // exist, build the spot-shadow pool — its bind group references
        // both targets at bindings 3/4 and (when present) the cube sampling view
        // at binding 5. See `SpotShadowPool::new` docs.
        let spot_shadow_pool = SpotShadowPool::new(
            &device,
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
            &device,
            surface_format,
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
        } = build_shadow_vs_resources(&device, &shadow_vs_bgl);

        let frame_timing = build_frame_timing(&device, &queue, enable_gpu_timing);

        // See: context/lib/rendering_pipeline.md §7.4
        let smoke_pass = SmokePass::new(
            &device,
            surface_format,
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
            &device,
            surface_format,
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
        mesh_pass.upload_identity_palette(&queue);
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
            &device,
            &lights_buffer,
            &influence_buffer,
            &sh_volume_resources.scripted_light_descriptors,
            &sh_volume_resources.animation.anim_samples,
            &spot_shadow_pool.array_view,
            &spot_shadow_pool.compare_sampler,
            &spot_shadow_pool.matrices_buffer,
            cube_shadow_pool.as_ref().map(|p| &p.sampling_view),
        );

        // UI quad / 9-slice + text pass — sibling to fog. Owns all UI GPU state
        // (quad pipeline, glyphon atlas/renderer, white texel). The splash phase
        // and the gameplay path both record through it.
        let ui = ui::UiPass::new(&device, &queue, surface_format);

        let mut fog = FogPass::new(
            &device,
            surface_config.width,
            surface_config.height,
            crate::fx::fog_volume::clamp_fog_pixel_scale(0),
            &depth_view,
            &uniform_bind_group_layout,
            &sh_volume_resources.bind_group_layout,
            &spot_shadow_bgl,
            cube_array_supported,
        );
        // Swapchain may differ from the hardcoded Rgba8UnormSrgb default.
        fog.rebuild_composite_for_format(&device, surface_format);

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
        let debug_lines = debug_lines::DebugLineRenderer::new(
            &device,
            surface_format,
            DEPTH_FORMAT,
            1,
            &uniform_bind_group_layout,
        );

        Ok(Self {
            device,
            queue,
            surface,
            surface_config,
            is_surface_configured: true,
            pipeline,
            depth_prepass_pipeline,
            frame_timing,
            vertex_buffer,
            index_buffer,
            index_count,
            uniform_buffer,
            uniform_bind_group,
            lighting_bind_group,
            light_count,
            mesh_dynamic_time: 0.0,
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
            lightmap_resources,
            animated_lightmap,
            lights_buffer,
            last_lights_upload: Vec::new(),
            lights_pack_scratch: Vec::new(),
            level_lights,
            shadow_candidate_lights,
            light_effective_brightness: Vec::new(),
            last_camera_position: Vec3::ZERO,
            last_view_proj: Mat4::IDENTITY,
            spot_shadow_pool,
            cube_shadow_pool,
            cube_shadow_vs_uniform_buffer,
            cube_shadow_vs_bind_group,
            shadow_vs_uniform_buffer,
            shadow_vs_bind_group,
            shadow_depth_pipeline,
            shadow_vs_stride,
            depth_view,
            screen_effects,
            gpu_textures,
            bvh_leaves,
            compute_cull,
            shadow_cull,
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
            show_navmesh: false,
            lighting_isolation: LightingIsolation::Normal,
            dynamic_direct_isolation: DynamicDirectIsolation::Combined,
            sdf_shadow_mode: SdfShadowMode::On,
            sdf_force_visibility_one: std::env::var("POSTRETRO_SDF_FORCE_VISIBILITY_ONE")
                .ok()
                .as_deref()
                == Some("1"),
            vsync_enabled: true,
            has_geometry,
            debug_frame: 0,
            debug_prev_bitmask: (u32::MAX, u32::MAX),
            debug_prev_vp_hash: u32::MAX,
            debug_prev_visible: ("init", usize::MAX),
            shadow_debug_enabled: std::env::var("POSTRETRO_SHADOW_DEBUG").ok().as_deref()
                == Some("1"),
            shadow_debug_prev: (u128::MAX, u128::MAX, u32::MAX, u32::MAX),
            smoke_pass,
            mesh_pass,
            mesh_draws: Vec::new(),
            bone_palette_scratch: Vec::new(),
            mesh_overflow_last_warn: f32::NEG_INFINITY,
            spot_entity_occluders_submitted: 0,
            cube_entity_occluders_submitted: 0,
            ui,
            splash_logo_size: None,
            ui_images: ui::UiImageRegistry::default(),
            ui_snapshot: ui::UiReadSnapshot::default(),
            ui_theme: ui::theme::UiTheme::engine_default(),
            ui_theme_generation: 0,
            fog,
            fog_cell_masks: None,
            active_fog_aabbs: Vec::new(),
            texture_bind_group_layout,
            lighting_bind_group_layout,
            mip_count_aniso_samplers,
            loaded_textures,
            has_multi_draw_indirect,
            stored_texture_materials: Vec::new(),
            uniform_bind_group_layout,
            #[cfg(feature = "dev-tools")]
            debug_ui_gpu: None,
        })
    }
}
