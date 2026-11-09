// Full-phase renderer construction: every steady-state pipeline/pass/resource
// built from boot state (the boot phase lives in `renderer_init.rs`).
// See: context/lib/rendering_pipeline.md

use super::install_pipelines::{self, PipelineBuildGate};
use super::renderer_types::FullRenderer;
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
    extents: postretro_render_cpu::render_extent::RenderExtents,
    cube_array_supported: bool,
    bloom_render_profile: BloomRenderProfile,
    spot_shadow_map_resolution: u32,
    pipeline_build_gate: Option<PipelineBuildGate>,
) -> Result<FullRenderer> {
    // Every scene target builds at the committed scene extent, so a full init
    // after surface recreation or a saved render resolution needs no rebuild.
    let scene = extents.scene;

    let view_proj = build_default_view_projection(scene.aspect());
    // No level: every level resource belongs to the first install's set.
    let light_count = 0;
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

    // BGL owned here so the forward pipeline layout and every level set's spot
    // shadow bind group share it. The BGL carries bindings 3 (SDF shadow
    // factor) and 4 (scene depth) — both owned outside the pool. Binding 5
    // (point-light cube-array depth) is present only when
    // `cube_array_supported`; the shared BGL, the forward + fog pipelines, and
    // the shader variants all key off the same flag. Each level set builds its
    // group at its build, over the extent-bound views of that moment.
    let spot_shadow_bgl = SpotShadowPool::bind_group_layout(device, cube_array_supported);

    // Sampler pool seeded with the placeholder's mip count of `1`. The
    // pool grows in `install_textures` once `LoadedTexture::mip_count`
    // values arrive from the .prm sidecars. Placeholders always pick up
    // the `1` entry; never miss this lookup.
    let mut samplers = SamplerMaps::default();
    samplers
        .aniso
        .insert(1, create_mip_aniso_sampler(device, 1));
    samplers
        .character
        .insert(1, create_mip_character_model_sampler(device, 1));

    let (_depth_texture, depth_view) = create_depth_texture(device, scene.width, scene.height);
    let ghost_depth = ghost_depth::GhostDepthTarget::new(device, scene.width, scene.height);

    // Post-scene compositor seam: a linear HDR `scene_color` target + sRGB
    // resolve. The scene target is independent from the swapchain format.
    let screen_effects = ScreenEffectsPass::new(device, scene, extents.surface, surface_format);
    let bloom = BloomPass::new(
        device,
        scene.width,
        scene.height,
        screen_effects.scene_color_texture(),
        bloom_render_profile,
    );

    // The no-level SH volume exists only for its bind-group layouts, which
    // the pipelines below and every level's SH volume share.
    let scripted_light_capacity = scripted_light_capacity(0, 0, &[]);
    let mut sh_allocation_ledger = sh_residency::ShAllocationLedger::new();
    let sh_volume_resources = ShVolumeResources::new(
        device,
        queue,
        ShVolumeSections {
            depth_tiles: None,
            sh: None,
            stream_base_present: false,
            stream_animation_descriptors: None,
            indirect_delta_present: false,
            direct: None,
            direct_delta: None,
            animated_direct_delta: None,
            billboard_direct_scatter: None,
            animated_billboard_direct_scatter_delta: None,
        },
        // Runtime-spawned lights append after the full-authored prefix.
        scripted_light_capacity,
        probe_occlusion_enabled,
        super::mover_channel::MoverChannelResources::empty(device),
        &mut sh_allocation_ledger,
    );

    // The no-level SDF atlas exists only for its bind-group layout.
    let sdf_atlas_resources = SdfAtlasResources::new(device, queue, None);

    // Group 4: the lightmap pool + animated atlas; group 6: the vertex block
    // table. Layouts only: every level's set builds its own groups.
    let lightmap_bind_group_layout = crate::lighting::lightmap::bind_group_layout(device);
    let block_table_bind_group_layout =
        crate::lighting::lightmap::block_table_bind_group_layout(device);

    // SDF half-res shadow pass (Task 4). Always allocated: it owns the
    // half-res factor target the shadow pool's group 5 binds. Its per-level
    // groups live in each level's set; dispatch needs one.
    let sdf_shadow_pass = SdfShadowPass::new(
        device,
        &sdf_atlas_resources.bind_group_layout,
        scene.width,
        scene.height,
    );

    // Cube point-shadow pool. Every level set's spot-shadow bind group (the
    // shared group-5 BGL) references its sampling view at binding 5. Disabled (None) when the adapter lacks
    // CUBE_ARRAY_TEXTURES — in that case binding 5 is omitted from the BGL and
    // NO cube view (not even a dummy) is created, since a `CubeArray` view
    // itself requires the feature. `cube_shadow_pool.is_some()` therefore
    // mirrors `cube_array_supported` exactly.
    let cube_shadow_pool =
        crate::lighting::cube_shadow::CubeShadowPool::new(device, cube_array_supported);

    // The boot shadow tier: the spot pool's textures at the boot resolution,
    // registered so the first build at it reuses them. Each level set binds
    // them (with the SDF shadow factor, scene depth and, when present, the
    // cube sampling view) in its own group 5.
    let shadow_tier = Arc::new(shadow_tier::ShadowTier::new(
        device,
        spot_shadow_map_resolution,
    ));
    let mut shadow_tiers = shadow_tier::TierRegistry::default();
    shadow_tiers.register(spot_shadow_map_resolution, &shadow_tier);
    let spot_shadow_pool = SpotShadowPool::over(shadow_tier.spot.clone());
    // No level, no promotion candidate or dynamic light: the first publish
    // points both caches at its set's tier.
    let promoted_depth_cache: Option<PromotedDepthCache> = None;
    let dynamic_depth_cache = DynamicDepthCacheGpu::over(Default::default());
    {
        use crate::lighting::spot_shadow::{SHADOW_DEPTH_FORMAT, SHADOW_POOL_SIZE};
        // Depth32Float = 4 B/texel; MiB = bytes >> 20. Derived from the fixed
        // slot count and configured resolution so diagnostics match allocation.
        let vram_mib = (SHADOW_POOL_SIZE as u64
            * spot_shadow_map_resolution as u64
            * spot_shadow_map_resolution as u64
            * 4)
            >> 20;
        log::info!(
            "[Renderer] Spot shadow pool initialized ({} × {}×{} {:?} = {} MiB VRAM)",
            SHADOW_POOL_SIZE,
            spot_shadow_map_resolution,
            spot_shadow_map_resolution,
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
        &block_table_bind_group_layout,
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
        queue,
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
    let mesh_pass = mesh_pass::MeshPass::new(
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
    // The mesh and mover group-2 light bind groups are level resources: each
    // install builds them over its own light buffers into its set.
    let kinematic_brush = kinematic_brush::KinematicBrushPass::new(
        device,
        DEPTH_FORMAT,
        &uniform_bind_group_layout,
        &texture_bind_group_layout,
        &sh_volume_resources.mesh_bind_group_layout,
        cube_array_supported,
    );
    // Mover ghosts: camera + world material + their own instances.
    // See: context/lib/rendering_pipeline.md §7.9
    let kinematic_ghost = kinematic_ghost::KinematicGhostPass::new(
        device,
        DEPTH_FORMAT,
        &uniform_bind_group_layout,
        &texture_bind_group_layout,
    );
    let billboard_ghost = billboard_ghost::BillboardGhostPass::new(
        device,
        DEPTH_FORMAT,
        &uniform_bind_group_layout,
        &smoke_pass,
    );
    // Fluid surfaces share the camera, world-material, lighting and SH
    // layouts; their scene inputs (group 5) and the reflection pass's
    // pipeline and inputs are built here over the scene depth and colour,
    // level or not. See: context/lib/rendering_pipeline.md §7.3a, §10
    let fluid_surface = fluid_surface::FluidSurfacePass::new(
        device,
        DEPTH_FORMAT,
        &uniform_bind_group_layout,
        &texture_bind_group_layout,
        &lighting_bind_group_layout,
        &sh_volume_resources.bind_group_layout,
        &depth_view,
        screen_effects.scene_color_view(),
    );
    // Every planar pipeline builds here at every tier, so a tier change
    // compiles nothing. See: context/lib/rendering_pipeline.md §7.3a
    let planar_reflection = planar_reflection::PlanarReflectionPass::new(
        device,
        &uniform_bind_group_layout,
        &texture_bind_group_layout,
        smoke_pass.instance_bind_group_layout(),
    );
    let rigid_occluder_depth = rigid_occluder_depth::RigidOccluderDepthPass::new(
        device,
        crate::lighting::spot_shadow::SHADOW_DEPTH_FORMAT,
        &shadow_vs_bgl,
        kinematic_brush.instance_transform_bind_group_layout(),
    );

    // Gameplay UI owns its quad pipeline, glyphon atlas/renderer, and white
    // texel, and records into the native-res UI layer. Boot splash rendering
    // uses its separate lightweight pass.
    let ui = ui::UiPass::new(device, queue, UI_LAYER_FORMAT);

    let fog = FogPass::new(
        device,
        &uniform_bind_group_layout,
        &sh_volume_resources.bind_group_layout,
        &spot_shadow_bgl,
        cube_array_supported,
    );
    log::info!("[Renderer] Pipeline ready (no geometry loaded)");

    #[cfg(feature = "dev-tools")]
    let debug_lines =
        debug_lines::DebugLineRenderer::new(device, DEPTH_FORMAT, 1, &uniform_bind_group_layout);

    // Full initialization builds only no-level bindings. Keep its accounting
    // local; `install_level_geometry` publishes the report for a real level.
    let _bootstrap_sh_residency = sh_allocation_ledger.finish();
    // Last, so a failed init above leaves no thread behind. A spawn failure
    // fails full init like any other resource it cannot create.
    let upload_thread = UploadThread::spawn(device.clone(), queue.clone())
        .context("spawning the render-upload thread")?;
    // The upload thread's first job: every install-time pipeline, built once
    // for this renderer, off the main thread. Level installs read the slot.
    let install_pipelines = install_pipelines::submit_install_pipeline_build(
        &upload_thread,
        uniform_bind_group_layout.clone(),
        pipeline_build_gate,
    );
    Ok(FullRenderer {
        pipeline,
        depth_prepass_pipeline,
        frame_timing,
        level: None,
        resident_levels: Vec::new(),
        next_level_set_id: 0,
        extent_rebuilds: 0,
        reflection_tier_rebuilds: 0,
        scene_targets_generation: 0,
        build_token: std::sync::Arc::new(()),
        pending_textures: None,
        pending_models: Default::default(),
        model_cache: Default::default(),
        pending_samplers: Default::default(),
        pending_smoke: Default::default(),
        lightmap_generation_floor: 0,
        uniform_buffer,
        uniform_bind_group,
        mesh_dynamic_time: 0.0,
        frame_light_term_mask: LightTermMask::ALL,
        frame_indirect_scale: DEFAULT_INDIRECT_SCALE,
        kinematic_mover_draws: Vec::new(),
        kinematic_mover_shadow_draws: Vec::new(),
        mover_occluder_aabbs: Vec::new(),
        mover_light_states: Default::default(),
        ambient_floor,
        indirect_scale: DEFAULT_INDIRECT_SCALE,
        dynamic_direct_scale: DEFAULT_DYNAMIC_DIRECT_SCALE,
        probe_occlusion_enabled,
        sdf_shadow_pass,
        #[cfg(feature = "dev-tools")]
        freeze_time: false,
        #[cfg(feature = "dev-tools")]
        frozen_time: 0.0,
        last_camera_position: Vec3::ZERO,
        last_view_proj: Mat4::IDENTITY,
        spot_shadow_pool,
        shadow_tier,
        shadow_tiers,
        spot_shadow_bgl: spot_shadow_bgl.clone(),
        cube_shadow_pool,
        kinematic_brush,
        kinematic_ghost,
        fluid_surface,
        planar_reflection,
        rigid_occluder_depth,
        promoted_depth_cache,
        promoted_depth_cache_missing_layer_warned: false,
        promoted_depth_cache_frame_plan: PromotedDepthCacheFramePlan::default(),
        promoted_depth_cache_promoted_count: 0,
        promoted_depth_cache_world_render_skips: 0,
        promoted_depth_cache_timing_open: false,
        dynamic_depth_cache,
        dynamic_depth_cache_frame_plan: DynamicDepthCachePlan::default(),
        dynamic_depth_cache_diagnostics: Default::default(),
        #[cfg(feature = "dev-tools")]
        direct_sh_debug_override: DirectShDebugOverride::default(),
        #[cfg(feature = "dev-tools")]
        animated_direct_sh_debug_override: AnimatedDirectShDebugOverride::default(),
        force_full_resident_sh_compose: false,
        cube_shadow_vs_uniform_buffer,
        cube_shadow_vs_bind_group,
        shadow_vs_uniform_buffer,
        shadow_vs_bind_group,
        shadow_depth_pipeline,
        shadow_vs_stride,
        depth_view,
        ghost_depth,
        ghost_order: ghost_order::GhostDrawOrder::default(),
        screen_effects,
        bloom,
        wireframe_cull_status_pipeline,
        wireframe_visible_pipeline,
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
        #[cfg(feature = "dev-tools")]
        show_remote_entity_markers: false,
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
        debug_frame: 0,
        debug_prev_bitmask: (u32::MAX, u32::MAX),
        debug_prev_vp_hash: u32::MAX,
        debug_prev_visible: ("init", usize::MAX),
        camera_cull_diagnostics: crate::render::CameraCullDiagnostics::default(),
        spatial_diagnostics: crate::render::SpatialDiagnostics::default(),
        bvh_cull_diagnostics: None,
        shadow_debug_enabled: std::env::var("POSTRETRO_SHADOW_DEBUG").ok().as_deref() == Some("1"),
        shadow_debug_prev: (u128::MAX, u128::MAX, u32::MAX, u32::MAX),
        smoke_pass,
        mesh_pass,
        mesh_draws: Vec::new(),
        ghost_mesh_draws: Vec::new(),
        billboard_ghost,
        bone_palette_scratch: Vec::new(),
        mesh_overflow_last_warn: f32::NEG_INFINITY,
        spot_entity_occluders_submitted: 0,
        cube_entity_occluders_submitted: 0,
        promoted_entity_occluders_submitted: 0,
        ui,
        ui_images: ui::UiImageRegistry::default(),
        ui_snapshot: ui::UiReadSnapshot::default(),
        limiter_frame: Default::default(),
        presentation_inputs: Vec::new(),
        ui_theme: ui::theme::UiTheme::engine_default(),
        ui_theme_generation: 0,
        fog,
        texture_bind_group_layout,
        lighting_bind_group_layout,
        samplers,
        upload_thread,
        install_pipelines,
        #[cfg(feature = "dev-tools")]
        debug_ui_gpu: None,
    })
}
