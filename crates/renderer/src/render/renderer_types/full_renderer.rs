// The full-phase renderer: every steady-state pipeline, pass and resource,
// with its SH binding seams.
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
    pub(in crate::render) vertex_buffer: wgpu::Buffer,
    /// The forward pipeline's second vertex buffer: one `Uint16x4` owner entry
    /// per world vertex, written once at install.
    pub(in crate::render) spec_owner_buffer: wgpu::Buffer,
    pub(in crate::render) index_buffer: wgpu::Buffer,
    pub(in crate::render) index_count: u32,
    pub(in crate::render) uniform_buffer: wgpu::Buffer,
    pub(in crate::render) uniform_bind_group: wgpu::BindGroup,

    /// Retained so `install_textures` can create material bind groups after init.
    pub(in crate::render) texture_bind_group_layout: wgpu::BindGroupLayout,
    /// Retained so `install_level_geometry` can rebuild the lighting bind group.
    pub(in crate::render) lighting_bind_group_layout: wgpu::BindGroupLayout,
    /// Post Retro linear+anisotropic samplers, one per distinct uploaded
    /// `mip_count`. Sampler descriptors are identical except for
    /// `lod_max_clamp = (mip_count - 1) as f32`. Keyed by
    /// `LoadedTexture::mip_count`. Engine-lifetime — persists across level
    /// reloads so re-installing the same mip chain reuses the existing sampler.
    /// Placeholders pick up the `1` entry seeded at construction. World and mover
    /// material bind groups bind their matching sampler at group-1 binding 5.
    pub(in crate::render) mip_count_aniso_samplers: HashMap<u32, wgpu::Sampler>,
    /// Character-model samplers, one per distinct uploaded `mip_count`. These
    /// use nearest magnification with linear minification and mip filtering,
    /// and disable anisotropy. Like the world sampler pool, descriptors differ
    /// only in `lod_max_clamp` and entries persist across level reloads. Character
    /// material bind groups bind their matching sampler at group-1 binding 5.
    pub(in crate::render) mip_count_character_model_samplers: HashMap<u32, wgpu::Sampler>,
    /// Engine-lifetime owners of the loaded textures and views referenced by
    /// material bind groups. Replaced wholesale on every `install_textures`.
    /// Bind groups borrow these handles; dropping the vec invalidates them,
    /// so keep them resident for the level's lifetime.
    #[allow(dead_code)]
    pub(in crate::render) loaded_textures: Vec<LoadedTexture>,
    /// Per-texture material properties derived from texture names. Set by
    /// `install_level_geometry`; consumed by `install_textures` to populate
    /// per-material shininess uniforms.
    pub(in crate::render) stored_texture_materials: Vec<Material>,
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

    /// Always bound; maps with zero lights get a 1-element dummy buffer —
    /// wgpu rejects zero-sized storage buffer bindings.
    pub(in crate::render) lighting_bind_group: wgpu::BindGroup,
    /// The planar mirror's light records and their bind group, the frame's
    /// mirror light reach and its admission tags (`rendering_pipeline.md`
    /// §7.3a, Pool rank).
    pub(in crate::render) mirror_lighting: renderer_mirror_lights::MirrorLighting,
    pub(in crate::render) influence_buffer: wgpu::Buffer,
    /// Maximum dynamic-direct prefix accepted from the scripting bridge.
    /// Promoted static records occupy a separate reserved tail.
    pub(in crate::render) dynamic_light_capacity: usize,
    pub(in crate::render) light_count: u32,
    /// Raw `AnimatedBakedLights` roster length. Every row reserves a forward
    /// record after the dynamic prefix, including holes and entries above the
    /// fixed Pass-B compose-weight cap, so the per-row identity never compacts.
    pub(in crate::render) animated_baked_light_count: usize,
    /// Dynamic tier, raw animated-baked tail, then promoted static records.
    /// World and billboards recover the static suffix from the packed tail
    /// count in the stable frame-uniform ABI; mesh/kinematic receivers consume
    /// the complete count through their dedicated params uniforms.
    pub(in crate::render) total_light_count: u32,
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

    /// Absent/disabled OctahedralShVolume → dummy 1×1 atlas resources;
    /// `has_sh_volume == 0` skips indirect sampling.
    pub(in crate::render) sh_volume_resources: ShVolumeResources,
    /// Finished, plain-Rust accounting for the most recently installed level.
    /// It is absent until `install_level_geometry` crosses the level boundary.
    pub(in crate::render) sh_residency_report: Option<ShResidencyReport>,
    /// Lightmap-family meter for the bound group-4 textures. Rebuilt by every
    /// level install, so a level unload reports the placeholders.
    pub(in crate::render) lightmap_residency_report: LightmapResidencyReport,
    /// Streaming-only residency ownership. The legacy whole-level path keeps
    /// this absent and continues using its existing resources unchanged.
    pub(in crate::render) sh_streaming: Option<sh_streaming::ShResidencyState>,

    /// Static-occluder SDF atlas + bind group. Owned by the renderer; the
    /// bind-group layout is consumed only by the SDF shadow pass — NOT
    /// bound by forward (forward gets only the shadow-factor texture in
    /// group 5). `present` is false when no SDF section is in the PRL;
    /// the shadow pass skips its dispatch in that case.
    pub(in crate::render) sdf_atlas_resources: SdfAtlasResources,
    /// Half-resolution per-light SDF shadow pass. Always allocated.
    /// Dispatch is gated on `sdf_atlas_resources.present` and the active
    /// `SdfShadowMode`.
    pub(in crate::render) sdf_shadow_pass: SdfShadowPass,
    /// Lightmap bake mode, recorded from the id-22 header; `Shadowed` when
    /// no level is installed. The compiler writes only `Shadowed`
    /// (visibility baked in). `Unshadowed` is not honoured: the forward pass
    /// never multiplies SDF visibility into the static term, so it renders
    /// as the baked irradiance with no shadow term. Surfaced by
    /// `Renderer::lightmap_mode`.
    pub(in crate::render) lightmap_mode: postretro_level_loader::LightmapMode,

    /// CPU mirror of animated-light delta volume placements, one entry per
    /// animated light. Empty when the map has no delta SH volumes. Sourced
    /// at level load from the same `DeltaShVolumesSection` `sh_compose` consumes;
    /// surfaced via `Renderer::sh_delta_volumes` for the SH diagnostic overlay.
    #[cfg(feature = "dev-tools")]
    pub(in crate::render) sh_delta_volumes_meta: Vec<sh_volume::DeltaVolumeMeta>,

    /// Async readback of the composed SH atlas so irradiance probe markers
    /// reflect live (base + animated-delta) lighting. Rebuilt per level load.
    #[cfg(feature = "dev-tools")]
    pub(in crate::render) sh_probe_readback: sh_diagnostics::ShProbeReadback,

    /// Dev-tools toggle: when set, `uniforms.time` is pinned to `frozen_time`,
    /// so all curve-driven animation (SH compose, animated lightmap, scripted
    /// lights) holds still — a debugging aid for isolating time-driven artifacts.
    #[cfg(feature = "dev-tools")]
    pub(in crate::render) freeze_time: bool,
    /// Time held while `freeze_time` is set; tracks live time otherwise, so
    /// enabling the freeze holds whatever animation phase is currently showing.
    #[cfg(feature = "dev-tools")]
    pub(in crate::render) frozen_time: f32,

    /// Composes base SH bands into the total bands consumers sample. Must run
    /// before the depth pre-pass so the storage→sampled barrier resolves first.
    pub(in crate::render) sh_compose: ShComposeResources,

    /// Optional direct-SH compose pass. Disabled entirely when a level has no
    /// selected static direct deltas; in that path the base direct atlas remains
    /// bound and no composed direct texture exists.
    pub(in crate::render) direct_sh_compose: DirectShComposeResources,
    /// Optional section-48 compose. Static-only scatter binds its base directly,
    /// while unavailable content keeps the billboard legacy path selected.
    pub(in crate::render) billboard_direct_scatter_compose: BillboardDirectScatterComposeResources,

    /// Absent Lightmap section → 1×1 white/neutral placeholder; no shader branch.
    pub(in crate::render) lightmap_resources: LightmapResources,

    pub(in crate::render) animated_lightmap: animated_lightmap::AnimatedLightmapResources,

    #[allow(dead_code)]
    pub(in crate::render) lights_buffer: wgpu::Buffer,
    /// Last bytes uploaded to `lights_buffer`. Reused each frame to skip a
    /// redundant `queue.write_buffer` when the packed bytes are unchanged.
    pub(in crate::render) last_lights_upload: Vec<u8>,
    /// The bridge's unweighted animated-baked tail records. Promotion scales
    /// this copy by `w` each frame; `last_lights_upload` holds the weighted
    /// result, so re-weighting that would compound `w` on frames without a
    /// bridge upload.
    pub(in crate::render) authored_animated_tail: Vec<u8>,
    /// Cached dynamic influences plus the raw animated-baked tail, index-parallel
    /// to `last_lights_upload`. Shadow-slot updates preserve both before appending
    /// selected-static influences and shadowmask metadata.
    pub(in crate::render) last_influence_upload: Vec<u8>,
    /// Scratch buffer for the fallback full-repack path. Used only when
    /// `last_lights_upload` is not yet sized to the current light set
    /// (first frame or light-count change). The hot path patches
    /// `last_lights_upload` in place via `patch_shadow_slots` — scratch
    /// is not touched in that branch.
    pub(in crate::render) lights_pack_scratch: Vec<u8>,
    /// Scratch buffer for per-frame influence upload: dynamic influences, raw
    /// animated-baked tail, selected-static influences, then shadowmask metadata.
    /// Matches the count-split light buffer without render-hot-path allocation.
    pub(in crate::render) influence_pack_scratch: Vec<u8>,
    #[allow(dead_code)]
    pub(in crate::render) level_lights: Vec<MapLight>,
    /// Original full level-light index for each `level_lights` entry.
    pub(in crate::render) level_light_source_indices: Vec<usize>,
    pub(in crate::render) level_light_influences: Vec<LightInfluence>,
    /// Selected static lights in `EntityShadowLights` order. Source indices are
    /// global level-light indices.
    pub(in crate::render) entity_shadow_lights: Vec<MapLight>,
    pub(in crate::render) entity_shadow_light_influences: Vec<LightInfluence>,
    pub(in crate::render) entity_shadow_light_source_indices: Vec<usize>,
    pub(in crate::render) entity_shadow_spec_light_indices: Vec<u32>,
    /// Load-fixed: the level's id-42 mask pool is bound
    /// (`LightmapResources::shadowmask_present`). Uploaded as the frame
    /// uniforms' mask-present bit.
    pub(in crate::render) shadowmask_present: bool,
    pub(in crate::render) forward_shadowmask_metadata_scratch: Vec<u8>,
    /// Candidate set for the spot/cube shadow pools — sourced from the FULL level
    /// light set filtered by `is_dynamic`. Dynamic-tier lights
    /// (`light_dynamic`/`light_dynamic_spot`) are pool-eligible so dynamic
    /// spotlights shadow static world occluders (pillars). The per-light
    /// `casts_entity_shadows` toggle (FGD `_cast_entity_shadows`) gates only
    /// whether moving-ENTITY occluders draw into the slot, not slot allocation.
    pub(in crate::render) shadow_candidate_lights: Vec<MapLight>,
    /// Original full level-light index for each `shadow_candidate_lights` entry.
    pub(in crate::render) shadow_candidate_source_indices: Vec<usize>,
    /// Selection index for a selected-static shadow candidate. Animated baked
    /// candidates use `shadow_candidate_animated_baked_indices` instead.
    pub(in crate::render) shadow_candidate_selection_indices: Vec<Option<usize>>,
    /// `AnimatedBakedLights` index for a section-45 candidate. Kept separate
    /// from both selected-static selection and `MapLight::animated_slot`.
    pub(in crate::render) shadow_candidate_animated_baked_indices: Vec<Option<usize>>,
    /// Candidate-indexed influence volumes paired with `shadow_candidate_lights`.
    /// Missing/short PRL influence data is represented by an uncullable sentinel
    /// so shadow eligibility follows the same degradation contract as forward
    /// direct-light culling.
    pub(in crate::render) shadow_candidate_influences: Vec<LightInfluence>,
    /// Lights near zero are excluded from shadow slot ranking. Empty = no suppression.
    pub(in crate::render) light_effective_brightness: Vec<f32>,
    /// Forward-lookahead brightness maxima keyed by `AnimatedBakedLights`
    /// index. Animated candidates must never read the dynamic-tier
    /// `light_effective_brightness` index space.
    pub(in crate::render) animated_light_window_brightness: Vec<f32>,
    /// Cached from `update_per_frame_uniforms` so the shadow pass can re-rank lights.
    pub(in crate::render) last_camera_position: Vec3,
    /// Cached camera `view_proj` from `update_per_frame_uniforms`; the shadow
    /// pool derives camera frustum planes from it for cone-frustum culling.
    pub(in crate::render) last_view_proj: Mat4,
    pub(in crate::render) spot_shadow_pool: SpotShadowPool,
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
    pub(in crate::render) promoted_static_states: Vec<PromotedBakedLightState>,
    /// Promotion ramp state keyed by `AnimatedBakedLights` index. Keeping this
    /// state separate prevents collisions with `EntityShadowLights` indices.
    pub(in crate::render) promoted_animated_states: Vec<PromotedBakedLightState>,
    pub(in crate::render) promoted_baked_records: Vec<PromotedBakedLightRecord>,
    /// Cache-layer metadata parallel to `promoted_baked_records`; packed into
    /// the forward shadowmask metadata tail's `meta1.w` lane.
    pub(in crate::render) promoted_baked_cache_layers: Vec<i32>,
    pub(in crate::render) promoted_static_weights: Vec<f32>,
    pub(in crate::render) promoted_static_weight_buffer: wgpu::Buffer,
    pub(in crate::render) promoted_static_weight_scratch: Vec<u8>,
    pub(in crate::render) promoted_baked_last_update_time: Option<f64>,
    /// The next light-slot update is a level's reveal frame: a light promoted
    /// on it starts at full weight, since an entry has nothing to fade from.
    pub(in crate::render) promotions_start_whole: bool,
    /// `None` only when a map has neither selected-static nor section-45
    /// animated-baked promotion candidates. Either source can use the same
    /// static world-depth cache, so an animated-only map still allocates it.
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

    /// GPU textures indexed by texture index.
    pub(in crate::render) gpu_textures: Vec<GpuTexture>,
    pub(in crate::render) bvh_leaves: Vec<postretro_render_data::geometry::BvhLeaf>,
    /// Per-cell BVH-leaf draw index (PRL section 37), cloned from the installed
    /// `LevelGeometry`. `None` only when no level is installed, the installed
    /// map has an empty BVH, or resources were released. Non-empty BVHs require
    /// this index at load; missing or invalid data is a load error. Whole-BVH
    /// tree-walk fallback is a per-frame runtime path for `DrawAll`,
    /// solid/exterior/no-portals visibility, out-of-range gathered cell ids, or no candidate
    /// cull pipeline.
    pub(in crate::render) cell_draw_index: Option<postretro_level_loader::CellDrawIndex>,
    /// `None` for maps with no BVH.
    pub(in crate::render) compute_cull: Option<ComputeCullPipeline>,
    /// Candidate-cull GPU path: gathers only visible cells' BVH
    /// leaves (via the baked `cell_draw_index` CSR) and dispatches one
    /// invocation per candidate leaf, writing the SAME global indirect/status
    /// slots as `compute_cull`. Built in lockstep with `compute_cull`; used only
    /// on candidate-eligible frames (valid index + `Culled` + `PrlPortal` or
    /// `PortalStepLimitFallback`),
    /// otherwise the whole-BVH tree walk runs. `None` for maps with no BVH.
    pub(in crate::render) candidate_cull: Option<crate::candidate_cull::CandidateCullPipeline>,
    /// CPU reach for shadow world depth: the installed level's reach index and
    /// the walk scratch every spot slot and cube face reuses. Rebuilt by every
    /// level install.
    pub(in crate::render) shadow_world: super::shadow_world_draws::ShadowWorldDraws,

    pub(in crate::render) wireframe_cull_status_pipeline: wgpu::RenderPipeline,
    pub(in crate::render) wireframe_visible_pipeline: wgpu::RenderPipeline,
    pub(in crate::render) wireframe_index_buffer: wgpu::Buffer,
    pub(in crate::render) wireframe_index_count: u32,
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

    pub(in crate::render) has_geometry: bool,

    pub(in crate::render) debug_frame: u64,
    pub(in crate::render) debug_prev_bitmask: (u32, u32),
    pub(in crate::render) debug_prev_vp_hash: u32,
    pub(in crate::render) debug_prev_visible: (&'static str, usize),
    /// One-shot guard so the candidate-cull out-of-range-cell warning logs once,
    /// not every frame. Reset on each level install so a later level's corrupt
    /// index still warns once.
    pub(in crate::render) candidate_cull_oor_logged: bool,
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

    /// Per-cell bitmask of overlapping fog volumes, loaded from PRL section 31
    /// at level load. When `Some`, the fog pass ORs the masks of reachable
    /// cells each frame to derive the active fog-volume set, culling volumes
    /// not reachable from the camera.
    pub(in crate::render) fog_cell_masks: Option<Vec<u32>>,

    /// (min, max) AABBs of fog volumes that are active this frame. Refreshed
    /// each frame via `set_fog_aabbs`; consumed by `collect_fog_spot_lights`
    /// to drop dynamic spots whose influence sphere can't scatter into any
    /// active volume. Empty list short-circuits to "pass everything" —
    /// conservative because the fog pass itself is gated by `FogPass::active`.
    pub(in crate::render) active_fog_aabbs: Vec<(Vec3, Vec3)>,
}

impl FullRenderer {
    /// Stream mode swaps only the level-owned group-3 resources. Pipeline
    /// layouts remain structurally identical, including billboard scatter's
    /// whole-resident binding, so every consumer selects through this one seam.
    pub(in crate::render) fn sh_bind_group(&self) -> &wgpu::BindGroup {
        self.sh_streaming
            .as_ref()
            .and_then(sh_streaming::ShResidencyState::bind_group)
            .unwrap_or(&self.sh_volume_resources.bind_group)
    }

    pub(in crate::render) fn sh_mesh_bind_group(&self) -> &wgpu::BindGroup {
        self.sh_streaming
            .as_ref()
            .and_then(sh_streaming::ShResidencyState::mesh_bind_group)
            .unwrap_or(&self.sh_volume_resources.mesh_bind_group)
    }

    /// The lattice group 3's SH sampler reads, for the fluid grid: the
    /// streamed manifest base, else an installed section's. `None` while the
    /// placeholder is bound (no section, or one the device refused) or the
    /// lattice is unusable.
    pub(in crate::render) fn sh_sampler_lattice(
        &self,
    ) -> Option<postretro_render_cpu::fluid_surface::FluidGridLattice> {
        sh_sampler_lattice(
            self.sh_streaming
                .as_ref()
                .map(sh_streaming::ShResidencyState::base_lattice),
            self.sh_volume_resources.present.then_some((
                self.sh_volume_resources.grid_origin,
                self.sh_volume_resources.cell_size,
            )),
        )
    }

    pub(in crate::render) fn sh_depth_moment_view(&self) -> wgpu::TextureView {
        self.sh_streaming
            .as_ref()
            .and_then(sh_streaming::ShResidencyState::depth_moment_view)
            .unwrap_or_else(|| self.sh_volume_resources.make_depth_moment_view())
    }
}

/// A streamed level's base lattice wins: its legacy resources still hold the
/// placeholder (origin 0, 1 m cells), which no sampler reads.
pub(in crate::render) fn sh_sampler_lattice(
    streamed: Option<([f32; 3], [f32; 3])>,
    legacy: Option<([f32; 3], [f32; 3])>,
) -> Option<postretro_render_cpu::fluid_surface::FluidGridLattice> {
    let (origin, cell_size) = streamed.or(legacy)?;
    postretro_render_cpu::fluid_surface::FluidGridLattice::new(origin, cell_size)
}
