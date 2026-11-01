// The full-phase renderer: every steady-state pipeline, pass and resource,
// and the slot for the active level's resource set.
// See: context/lib/rendering_pipeline.md

use super::*;

/// Full-phase renderer: all the steady-state pipelines, passes, and resources.
/// Built by `finish_full_init` from boot state alone (no level loaded). Every
/// field here was previously inline on `Renderer`; the split lets the boot
/// splash present before any of this is constructed.
pub(in crate::render) struct FullRenderer {
    pub(in crate::render) pipeline: wgpu::RenderPipeline,
    pub(in crate::render) depth_prepass_pipeline: wgpu::RenderPipeline,
    /// `Some` when `POSTRETRO_GPU_TIMING=1` AND the adapter supports both base
    /// and encoder-level timestamp queries; `None` → no timing writes.
    pub(in crate::render) frame_timing: Option<FrameTiming>,
    pub(in crate::render) uniform_buffer: wgpu::Buffer,
    pub(in crate::render) uniform_bind_group: wgpu::BindGroup,

    /// Retained so each level build can create material bind groups; the
    /// layout is snapshotted into its `LevelBuildContext`.
    pub(in crate::render) texture_bind_group_layout: wgpu::BindGroupLayout,
    /// Retained so each level build can create its lighting bind group; the
    /// layout is snapshotted into its `LevelBuildContext`.
    pub(in crate::render) lighting_bind_group_layout: wgpu::BindGroupLayout,
    /// The add-only sampler maps, one sampler per distinct uploaded
    /// `mip_count` (keyed by `LoadedTexture::mip_count`), a named exception to
    /// per-load ownership. `aniso`: Post Retro linear+anisotropic samplers,
    /// identical except for `lod_max_clamp = (mip_count - 1) as f32`; world
    /// and mover material bind groups bind their matching sampler at group-1
    /// binding 5, and placeholders pick up the `1` entry seeded at
    /// construction. `character`: nearest magnification with linear
    /// minification and mip filtering, no anisotropy, for character
    /// materials. Engine-lifetime: a build creates any sampler it is missing
    /// into its set's own map, and publishing the set merges that map in,
    /// keeping an existing entry. A build never writes these.
    pub(in crate::render) samplers: SamplerMaps,
    /// The active level's resources: every buffer, texture, bind group and
    /// per-level state one load builds, the only set frames read. `None`
    /// with no level published; release drops it and creates nothing. See
    /// `level_resource_set.rs`.
    pub(in crate::render) level: Option<LevelResourceSet>,
    /// Built, unpublished sets, each keyed by its `LevelSetId`. Never drawn.
    pub(in crate::render) resident_levels: Vec<LevelResourceSet>,
    /// The last minted `LevelSetId`.
    pub(in crate::render) next_level_set_id: u64,
    /// Publishes that found their set built at another scene extent and
    /// rebuilt its extent-bound groups and fog target (the counted fallback).
    pub(in crate::render) extent_rebuilds: u64,
    /// Publishes whose reflection targets the kept reflection tier re-keyed
    /// (the tier moved after the set's build): the counted fallback beside
    /// `extent_rebuilds`.
    pub(in crate::render) reflection_tier_rebuilds: u64,
    /// Bumped each time the scene-sized targets are recreated. A set records
    /// the generation its extent-bound groups bind, so its publish rebuilds
    /// them when the targets changed since, even at an equal extent.
    pub(in crate::render) scene_targets_generation: u64,
    /// This full renderer's identity: an off-main build carries it, and its
    /// receipt refuses a set built against another (pre-suspend) renderer.
    pub(in crate::render) build_token: std::sync::Arc<()>,
    /// World materials `install_textures` staged for the next build, which
    /// takes them into its set.
    pub(in crate::render) pending_textures: Option<LevelTextures>,
    /// Models uploaded with no active set, taken by the next build.
    pub(in crate::render) pending_models: mesh_pass::LevelModels,
    /// The shared, content-keyed model cache: a named exception to per-load
    /// ownership. Entries join at a commit and are freed at a commit; a
    /// release frees none. Dies with this full renderer (a suspend).
    pub(in crate::render) model_cache: super::model_cache::ModelCache,
    /// Samplers those staged models created, taken by the next build.
    pub(in crate::render) pending_samplers: SamplerMaps,
    /// Sprite collections registered with no active set, taken by the next
    /// build.
    pub(in crate::render) pending_smoke: smoke::SmokeLevelCollections,
    /// Renderer-wide, add-only lightmap generation floor. Every set reserves
    /// its first generation above it and every live set's high water, and
    /// each retired set folds its high water back in, so no set's first drain
    /// accepts another live or earlier set's generation.
    pub(in crate::render) lightmap_generation_floor: u64,
    /// Off-main GPU creation for level loads. Lives as long as this full
    /// renderer, so a rebuilt one after suspend spawns its own.
    pub(in crate::render) upload_thread: super::upload_thread::UploadThread,
    /// This renderer's install-time pipelines, published once by the upload
    /// thread's first job. Every level install binds these; none builds one.
    pub(in crate::render) install_pipelines:
        std::sync::Arc<super::install_pipelines::InstallPipelinesSlot>,

    /// GPU half of the debug UI. Lazily constructed by `ensure_debug_ui_gpu`
    /// on first panel open; stays resident for the rest of the session.
    /// `None` until then; never allocated in a no-`dev-tools` build.
    #[cfg(feature = "dev-tools")]
    pub(in crate::render) debug_ui_gpu: Option<debug_ui::DebugUiGpu>,

    /// The frame's forward `Uniforms.time` value, cached by
    /// `update_per_frame_uniforms` so the skinned-mesh group-2 params uniform
    /// (`MeshLightParams.time`) is written from the SAME render-clock value the
    /// forward pass uses that frame. The scripted-light animated curves the mesh
    /// dynamic loop evaluates depend on this phase coherence.
    pub(in crate::render) mesh_dynamic_time: f32,
    /// Captured alongside the group-0 uniform at the start of the render
    /// frame. Consumers must use this, never the UI-mutated live mask, so a
    /// toggle takes effect atomically on the following frame.
    pub(in crate::render) frame_light_term_mask: LightTermMask,
    /// Indirect scale captured with group 0, before diagnostics can change it.
    pub(in crate::render) frame_indirect_scale: f32,
    /// Camera-PVS-visible kinematic mover instances for the beauty pass.
    pub(in crate::render) kinematic_mover_draws: Vec<kinematic_brush::KinematicMoverInstance>,
    /// Every present kinematic mover transform, including camera-PVS-culled
    /// shadow casters.
    pub(in crate::render) kinematic_mover_shadow_draws:
        Vec<kinematic_brush::KinematicMoverInstance>,
    /// Conservative world AABBs for every present mover, keyed by authored
    /// mover id. Static-light promotion and rigid shadow occluder recording
    /// read this renderer-owned state.
    pub(in crate::render) mover_occluder_aabbs: Vec<rigid_occluder_depth::MoverOccluderAabb>,
    pub(in crate::render) ambient_floor: f32,
    pub(in crate::render) indirect_scale: f32,
    /// DYNAMIC baked-static-direct SH scale (0..1). Debug instrument for the
    /// entity/billboard direct term, independent of `indirect_scale`. Mirrors
    /// the `indirect_scale` knob — uploaded to the billboard group-0 tail and
    /// the mesh group-4 `DynamicDirectParams` each frame.
    pub(in crate::render) dynamic_direct_scale: f32,
    /// Runtime SH probe-occlusion toggle. Default-on; `POSTRETRO_SH_FAST=1`
    /// seeds it off for benchmark/headless runs, and the diagnostics panel can
    /// flip it later. Uploaded through `ShGridInfo`.
    pub(in crate::render) probe_occlusion_enabled: bool,

    /// Half-resolution per-light SDF shadow pass. Always allocated.
    /// Dispatch is gated on `sdf_atlas_resources.present` and the active
    /// `SdfShadowMode`.
    pub(in crate::render) sdf_shadow_pass: SdfShadowPass,

    /// Dev-tools toggle: when set, `uniforms.time` is pinned to `frozen_time`,
    /// so all curve-driven animation (SH compose, animated lightmap, scripted
    /// lights) holds still — a debugging aid for isolating time-driven artifacts.
    #[cfg(feature = "dev-tools")]
    pub(in crate::render) freeze_time: bool,
    /// Time held while `freeze_time` is set; tracks live time otherwise, so
    /// enabling the freeze holds whatever animation phase is currently showing.
    #[cfg(feature = "dev-tools")]
    pub(in crate::render) frozen_time: f32,

    /// Cached from `update_per_frame_uniforms` so the shadow pass can re-rank lights.
    pub(in crate::render) last_camera_position: Vec3,
    /// Cached camera `view_proj` from `update_per_frame_uniforms`; the shadow
    /// pool derives camera frustum planes from it for cone-frustum culling.
    pub(in crate::render) last_view_proj: Mat4,
    /// The active set's spot pool: its shadow tier's textures and this
    /// frame's slot assignment. Replaced whole at publish.
    pub(in crate::render) spot_shadow_pool: SpotShadowPool,
    /// The active set's shadow tier (before any publish, full init's at the
    /// boot shadow resolution), held so a build at its resolution reuses it.
    pub(in crate::render) shadow_tier: Arc<super::super::shadow_tier::ShadowTier>,
    /// Resolution → weak handle to every live shadow tier; a build looks its
    /// tier up here.
    pub(in crate::render) shadow_tiers:
        super::super::shadow_tier::TierRegistry<super::super::shadow_tier::ShadowTier>,
    /// The forward and fog group-5 layout every set's spot shadow group binds.
    pub(in crate::render) spot_shadow_bgl: wgpu::BindGroupLayout,
    /// Dynamic point-light cube-array shadow pool. `None` when the adapter lacks
    /// `CUBE_ARRAY_TEXTURES` — point shadows then cleanly off, spot unaffected.
    /// `Some` iff `cube_array_supported`, so its presence mirrors group-5 binding
    /// 5's presence in the shared BGL.
    pub(in crate::render) cube_shadow_pool: Option<crate::lighting::cube_shadow::CubeShadowPool>,
    pub(in crate::render) kinematic_brush: kinematic_brush::KinematicBrushPass,
    /// Mover ghosts (`rendering_pipeline.md` §7.9): their own instance list over
    /// the mover pass's geometry, refilled each frame via
    /// `set_kinematic_ghost_draws`; never in shadow depth or occluder bounds.
    pub(in crate::render) kinematic_ghost: kinematic_ghost::KinematicGhostPass,
    /// Translucent fluid surfaces; idle until a level with fluid faces installs.
    pub(in crate::render) fluid_surface: fluid_surface::FluidSurfacePass,
    /// Planar water reflection: the mirror view, its targets and pipelines,
    /// built at full init at every tier.
    pub(in crate::render) planar_reflection: planar_reflection::PlanarReflectionPass,
    pub(in crate::render) rigid_occluder_depth: rigid_occluder_depth::RigidOccluderDepthPass,
    /// The active set's promoted cache over its tier's shared textures.
    /// `None` only when the level has neither selected-static nor section-45
    /// animated-baked promotion candidates. Either source can use the same
    /// static world-depth cache, so an animated-only map still allocates it.
    /// Rebuilt cold at publish.
    pub(in crate::render) promoted_depth_cache: Option<PromotedDepthCache>,
    /// Missing cache-plan entries are defensive degradation, warned once per
    /// installed level rather than once per rendered frame.
    pub(in crate::render) promoted_depth_cache_missing_layer_warned: bool,
    pub(in crate::render) promoted_depth_cache_frame_plan: PromotedDepthCacheFramePlan,
    pub(in crate::render) promoted_depth_cache_promoted_count: u32,
    pub(in crate::render) promoted_depth_cache_world_render_skips: u32,
    pub(in crate::render) promoted_depth_cache_timing_open: bool,
    pub(in crate::render) dynamic_depth_cache: DynamicDepthCacheGpu,
    pub(in crate::render) dynamic_depth_cache_frame_plan: DynamicDepthCachePlan,
    pub(in crate::render) dynamic_depth_cache_diagnostics:
        dynamic_depth_cache::DynamicCacheDiagnostics,
    #[cfg(feature = "dev-tools")]
    pub(in crate::render) direct_sh_debug_override: DirectShDebugOverride,
    #[cfg(feature = "dev-tools")]
    pub(in crate::render) animated_direct_sh_debug_override: AnimatedDirectShDebugOverride,
    /// Exactness/debug bypass for streamed SH compose. Kept in all builds so
    /// VM-free capture can compare shipped gating against the resident oracle.
    pub(in crate::render) force_full_resident_sh_compose: bool,
    /// Per-(cube slot, face) light-space matrix uniforms, dynamic-offset like
    /// `shadow_vs_uniform_buffer`. Slot `slot*6 + face` carries that face's
    /// matrix; the skinned-depth pass selects it by dynamic offset.
    pub(in crate::render) cube_shadow_vs_uniform_buffer: wgpu::Buffer,
    pub(in crate::render) cube_shadow_vs_bind_group: wgpu::BindGroup,
    /// Dynamic-offset into a single buffer; offset selects the per-slot light-space matrix.
    pub(in crate::render) shadow_vs_uniform_buffer: wgpu::Buffer,
    pub(in crate::render) shadow_vs_bind_group: wgpu::BindGroup,
    pub(in crate::render) shadow_depth_pipeline: wgpu::RenderPipeline,
    /// Rounded up to `min_uniform_buffer_offset_alignment`.
    pub(in crate::render) shadow_vs_stride: u32,

    pub(in crate::render) depth_view: wgpu::TextureView,
    /// Ghost-only depth (`rendering_pipeline.md` §7.9), at the scene extent and
    /// rebuilt with `depth_view`. Seeded from scene depth on frames with
    /// skinned or mover ghosts; each ghost's depth-only draw writes it and
    /// its colour draw tests it, so scene depth stays the opaque scene's.
    pub(in crate::render) ghost_depth: ghost_depth::GhostDepthTarget,
    /// This frame's skinned and mover ghosts, farthest first; retained
    /// scratch rebuilt on frames with such ghosts.
    pub(in crate::render) ghost_order: ghost_order::GhostDrawOrder,

    /// Post-scene compositor seam: owns the `scene_color` offscreen target every
    /// gameplay scene/UI pass renders into, plus the resolve pass that blits it
    /// to the swapchain (the sole gameplay-path swapchain writer). Recreated on
    /// resize alongside `depth_view`. See `render/screen_effects.rs`.
    pub(in crate::render) screen_effects: ScreenEffectsPass,

    /// HDR bloom chain. Samples the post-fog `scene_color` target and
    /// composites before capture and presentation resolve.
    pub(in crate::render) bloom: BloomPass,

    pub(in crate::render) wireframe_cull_status_pipeline: wgpu::RenderPipeline,
    pub(in crate::render) wireframe_visible_pipeline: wgpu::RenderPipeline,
    pub(in crate::render) wireframe_cull_status_bgl: wgpu::BindGroupLayout,
    pub(in crate::render) world_wireframe_mode: WorldWireframeMode,
    pub(in crate::render) wireframe_enabled: bool,

    #[cfg(feature = "dev-tools")]
    pub(in crate::render) debug_lines: debug_lines::DebugLineRenderer,
    #[cfg(feature = "dev-tools")]
    pub(in crate::render) bvh_overlay: BvhOverlayState,
    #[cfg(feature = "dev-tools")]
    pub(in crate::render) cell_overlay: CellOverlayState,
    #[cfg(feature = "dev-tools")]
    pub(in crate::render) portal_overlay: PortalOverlayState,
    #[cfg(feature = "dev-tools")]
    pub(in crate::render) agent_overlay: AgentOverlayState,
    /// Navmesh overlay toggle, flipped by `Alt+Shift+N`. Read at the emit call
    /// site to decide whether to push region/portal debug lines this frame.
    #[cfg(feature = "dev-tools")]
    pub(in crate::render) show_navmesh: bool,

    /// Live dev-tools value. `update_per_frame_uniforms` snapshots it into
    /// `frame_light_term_mask` before scene recording begins.
    pub(in crate::render) light_term_mask: LightTermMask,

    /// Debug selector for the SDF static-occluder shadow path. Panel-only
    /// dropdown, surfaces through
    /// `FrameUniforms.sdf_shadow_mode`.
    pub(in crate::render) sdf_shadow_mode: SdfShadowMode,

    /// Dev toggle: force per-light SDF visibility to 1.0 in the forward shader.
    /// Panel checkbox; surfaces through `FrameUniforms.sdf_force_visibility_one`.
    /// Drives the no-double-count visual A/B (forced-1.0 must match the
    /// pre-change render). Seeded from the `POSTRETRO_SDF_FORCE_VISIBILITY_ONE`
    /// env flag at construction so a headless/no-UI run can exercise it too.
    pub(in crate::render) sdf_force_visibility_one: bool,

    /// Dev toggle: force static-light shadowmask visibility to 1.0 in the
    /// forward world-specular path. Panel checkbox; surfaces through
    /// `FrameUniforms.spec_shadowmask_force_one`. Seeded from
    /// `POSTRETRO_SPEC_SHADOWMASK_FORCE_ONE` for repeatable headless A/B
    /// captures. SDF and dynamic/mover paths remain unaffected.
    pub(in crate::render) spec_shadowmask_force_one: bool,

    /// Toggled by Alt+Shift+V; `true` = AutoVsync, `false` = AutoNoVsync.
    pub(in crate::render) vsync_enabled: bool,

    pub(in crate::render) debug_frame: u64,
    pub(in crate::render) debug_prev_bitmask: (u32, u32),
    pub(in crate::render) debug_prev_vp_hash: u32,
    pub(in crate::render) debug_prev_visible: (&'static str, usize),
    /// Camera-cull diagnostics for the current Spatial tab frame (candidate vs
    /// tree-walk path, candidate/total/submitted leaves). With `dev-tools`,
    /// refreshed before the debug UI reads it, then recomputed during pass
    /// recording; without it, stays at its default. Diagnostic only — never
    /// gates behavior.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub(in crate::render) camera_cull_diagnostics: CameraCullDiagnostics,
    /// Last CPU-side visibility/locator snapshot published by the app after
    /// camera visibility runs. Read by the Spatial diagnostics tab.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub(in crate::render) spatial_diagnostics: SpatialDiagnostics,

    /// Full tree-walk cull-cost estimate, refreshed from the current frame's
    /// visibility independent of which GPU cull strategy ran. This is the
    /// baseline the candidate path beats — it must not be a side effect of the
    /// tree-walk dispatch, or candidate-eligible frames starve it to zero.
    /// `None` when no cull pipeline is loaded (no level / no BVH). Read by the
    /// baseline diagnostics panel (dev-tools).
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub(in crate::render) bvh_cull_diagnostics: Option<crate::compute_cull::BvhCullDiagnostics>,

    /// `POSTRETRO_SHADOW_DEBUG=1`: env-gated shadow-pipeline diagnostics. Cached
    /// at construction so the hot path pays one bool test, not a `getenv`, per
    /// frame. When set, `emit_shadow_debug` logs (via `log::info!`) a compact
    /// per-frame line tracing which shadow decision flips as the camera pitches —
    /// camera pose + per-candidate-light shadow-slot status + the entity
    /// shadow-caster keep/drop tally. Read-only: it never changes culling or
    /// selection behavior. See `context/lib/rendering_pipeline.md` §4, §7.1.
    pub(in crate::render) shadow_debug_enabled: bool,
    /// Last `emit_shadow_debug` fingerprint, so the diagnostic logs on CHANGE
    /// (and every ~120 frames as a heartbeat) instead of spamming every frame.
    /// `(slot_occupancy, cube_occupancy, in_pvs, off_pvs)`.
    pub(in crate::render) shadow_debug_prev: (u128, u128, u32, u32),

    /// Idle (no draw) on maps with no registered collections. See §7.4.
    pub(in crate::render) smoke_pass: SmokePass,

    /// Skinned-mesh forward pass. Idle (no draw) until a model is uploaded via
    /// `load_skinned_model` (driven by the level-load model sweep at level
    /// install, once per distinct `prop_mesh` model).
    pub(in crate::render) mesh_pass: mesh_pass::MeshPass,

    /// Per-frame skinned-mesh instance list: surviving (model handle,
    /// interpolated transform, phase seed) tuples. Refilled each frame via
    /// `set_mesh_draws` from the render-frame mesh collector (which classifies
    /// forward visibility per view via `mesh_pass::mesh_visible_in_cell` and may
    /// retain additional selected-static-light shadow casters as non-forward
    /// instances). Empty when no mesh entity is visible or shadow-relevant.
    /// Planned into per-model draw groups + palette runs each frame by
    /// `mesh_instances::plan_mesh_frame`.
    pub(in crate::render) mesh_draws: Vec<mesh_instances::MeshInstanceInput>,

    /// Per-frame ghost instances (`rendering_pipeline.md` §7.9): unlit,
    /// translucent, already culled at their mapped positions and carrying their
    /// presented look. Refilled each frame via `set_ghost_mesh_draws`; planned
    /// into `MeshFramePlans::ghost`, which never reaches shadow depth.
    pub(in crate::render) ghost_mesh_draws: Vec<mesh_instances::MeshInstanceInput>,

    /// Projectile billboard ghosts, per sprite collection, refilled each frame
    /// via `set_billboard_ghost_draws`; always blended.
    pub(in crate::render) billboard_ghost: billboard_ghost::BillboardGhostPass,

    /// Reusable bone-palette scratch for per-frame per-instance sampling.
    /// `sample_clip` clears then refills it per instance, so steady-state frames
    /// allocate nothing. Lives on the renderer (not in the GPU pass) — it is
    /// CPU-side pose data the pass merely uploads.
    pub(in crate::render) bone_palette_scratch: Vec<postretro_model::BonePaletteEntry>,

    /// Wall-clock of the last palette/instance-overflow warning (render clock),
    /// for rate-limiting (mirrors `EmitterBridge`'s `last_warn_time`). Overflow
    /// drops the excess instances; the warning fires at most once per second.
    pub(in crate::render) mesh_overflow_last_warn: f32,

    /// CPU-side count of skinned and rigid ENTITY occluders submitted into spot
    /// shadow slots last frame, summed across slots (each counted once per slot
    /// it casts into) — no GPU readback. Verifies the "enemy outside the cone is
    /// not drawn" acceptance criterion: an occluder the per-light cone cull
    /// rejects is never added here. Reset to 0 at the start of the spot-shadow
    /// depth loop.
    pub(in crate::render) spot_entity_occluders_submitted: u32,

    /// CPU-side count of skinned and rigid ENTITY occluders submitted into CUBE
    /// (point-light) shadow faces last frame, summed across all occupied slots ×
    /// 6 faces (each counted once per face it casts into). Mirrors
    /// `spot_entity_occluders_submitted` — no GPU readback. Verifies that entity
    /// occluders render only for `entity_occluder_eligible` point lights and only
    /// when their bound intersects a face frustum. Reset to 0 at the start of the
    /// cube-shadow depth loop.
    pub(in crate::render) cube_entity_occluders_submitted: u32,

    /// CPU-side count of skinned and rigid ENTITY occluder submissions into
    /// promoted static-light shadow slots/faces last frame. This is a subset of
    /// the spot and cube totals above. Verifies that warm promoted slots submit
    /// entities to their cleared live pool while static world depth stays cached
    /// separately.
    pub(in crate::render) promoted_entity_occluders_submitted: u32,

    /// Instanced UI quad / 9-slice pass for panels and images plus glyphon text.
    /// Built alongside `fog`; records the splash (splash phase) and an empty draw
    /// list on the gameplay path (`render_frame_indirect`). Owns all UI GPU state.
    pub(in crate::render) ui: ui::UiPass,

    /// Key→bind-group registry for gameplay/frontend `image` widget assets. The
    /// gameplay UI pass resolves image batches' asset keys through it. The boot
    /// splash does NOT use this — it owns its own logo texture in `boot_splash`.
    pub(in crate::render) ui_images: ui::UiImageRegistry,

    /// Once-per-frame published read snapshot: the splash version/tagline line
    /// and the gameplay-path descriptor tree. Set by the App via `set_ui_snapshot`
    /// just before each render call; read when the UI pass records. Stored here so
    /// both render signatures stay stable.
    pub(in crate::render) ui_snapshot: ui::UiReadSnapshot,

    /// Presented-frame time for the photosensitivity limiter, set by the App
    /// beside the UI snapshot and taken by the resolve that consumes it. A
    /// frame that never resolves leaves it for the next. Never script time,
    /// which dev tools freeze.
    pub(in crate::render) limiter_frame: postretro_render_cpu::flash_limiter::PendingLimiterFrame,

    /// Frame-local passive presentation instances from the app-side pool. The
    /// renderer lowers them through its FontSystem-owned template path; they are
    /// deliberately separate from the retained UI snapshot and carry no focus,
    /// hit-test, or input state.
    pub(in crate::render) presentation_inputs: Vec<PresentationDrawInput>,

    /// Active UI theme: the token table every descriptor tree resolves its
    /// color/spacing/font slots against at build time. Defaults to
    /// `UiTheme::engine_default()` at construction; `set_ui_theme` installs an
    /// override (e.g. from a mod's theme document) and bumps `ui_theme_generation`.
    /// The gameplay render path resolves descriptor-tree tokens against this
    /// instance. The boot splash is renderer-owned (`BootSplashPass`) and does
    /// not use the theme.
    pub(in crate::render) ui_theme: ui::theme::UiTheme,
    /// Monotonic UI theme generation, bumped by `set_ui_theme`. The retained
    /// gameplay tree records the generation it was built against; a bump
    /// invalidates the resolved tokens baked into it, so `layout_gameplay_tree`
    /// rebuilds the tree on the next frame even when the descriptor is unchanged.
    pub(in crate::render) ui_theme_generation: u64,

    /// Volumetric fog raymarch + composite. Active only when the level has
    /// at least one fog volume uploaded; otherwise the dispatch + composite
    /// are skipped (see `FogPass::active`).
    pub(in crate::render) fog: FogPass,
}

// A streamed level's base lattice wins: its legacy resources still hold the
// placeholder (origin 0, 1 m cells), which no sampler reads.
pub(in crate::render) use postretro_render_cpu::sh_volume::sh_sampler_lattice;
