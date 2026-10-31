// One load's renderer resources: everything a level install builds, held as
// one set the renderer publishes as active and drops whole at release.
// See: context/plans/in-progress/level-load-pipeline/index.md · context/lib/rendering_pipeline.md §4

use super::renderer_types::{PromotedBakedLightRecord, PromotedBakedLightState};
use super::*;

/// One load's world materials: the uploaded textures, their material bind
/// groups, and the per-texture materials they were built from.
pub(in crate::render) struct LevelTextures {
    /// Owners of the textures and views the bind groups reference; dropping
    /// them with the set frees the level's materials.
    pub(in crate::render) loaded_textures: Vec<LoadedTexture>,
    /// GPU textures indexed by texture index; never empty (a level with no
    /// textures holds one placeholder slot).
    pub(in crate::render) gpu_textures: Vec<GpuTexture>,
    pub(in crate::render) stored_texture_materials: Vec<Material>,
}

/// A dynamic-object pass's group 2 over one load's light buffers: the main
/// view's records and the planar mirror's, which differ only in binding 0.
pub(in crate::render) struct LevelLightGroups {
    pub(in crate::render) light: wgpu::BindGroup,
    pub(in crate::render) mirror: wgpu::BindGroup,
}

impl LevelLightGroups {
    pub(in crate::render) fn select(&self, mirror_light_records: bool) -> &wgpu::BindGroup {
        if mirror_light_records {
            &self.mirror
        } else {
            &self.light
        }
    }
}

/// Every renderer resource one level install creates or resets. The renderer
/// holds at most one active set (`FullRenderer::level`); frames that need
/// level data skip when there is none, and releasing a level drops the set
/// and creates nothing.
pub(in crate::render) struct LevelResourceSet {
    // --- World materials ---
    pub(in crate::render) loaded_textures: Vec<LoadedTexture>,
    pub(in crate::render) gpu_textures: Vec<GpuTexture>,
    /// Per-texture material properties derived from texture names.
    pub(in crate::render) stored_texture_materials: Vec<Material>,

    // --- World geometry and cull ---
    pub(in crate::render) vertex_buffer: wgpu::Buffer,
    /// One `Uint16x4` owner entry per world vertex, written once at install.
    pub(in crate::render) spec_owner_buffer: wgpu::Buffer,
    pub(in crate::render) index_buffer: wgpu::Buffer,
    pub(in crate::render) index_count: u32,
    pub(in crate::render) wireframe_index_buffer: wgpu::Buffer,
    pub(in crate::render) wireframe_index_count: u32,
    pub(in crate::render) has_geometry: bool,
    pub(in crate::render) bvh_leaves: Vec<postretro_render_data::geometry::BvhLeaf>,
    /// Per-cell BVH-leaf draw index (PRL section 37). `None` when the level has
    /// an empty BVH.
    pub(in crate::render) cell_draw_index: Option<postretro_level_loader::CellDrawIndex>,
    /// `None` for maps with no BVH.
    pub(in crate::render) compute_cull: Option<ComputeCullPipeline>,
    /// Built in lockstep with `compute_cull`; `None` for maps with no BVH.
    pub(in crate::render) candidate_cull: Option<crate::candidate_cull::CandidateCullPipeline>,
    /// One-shot guard so the candidate-cull out-of-range-cell warning logs
    /// once per level.
    pub(in crate::render) candidate_cull_oor_logged: bool,
    /// CPU reach for shadow world depth over this level's BVH.
    pub(in crate::render) shadow_world: super::shadow_world_draws::ShadowWorldDraws,

    // --- Lights ---
    pub(in crate::render) lights_buffer: wgpu::Buffer,
    pub(in crate::render) influence_buffer: wgpu::Buffer,
    /// Forward group 2 over this level's light, influence, spec-light and
    /// chunk-grid buffers.
    pub(in crate::render) lighting_bind_group: wgpu::BindGroup,
    /// The planar mirror's light records and their bind group, the frame's
    /// mirror light reach and its admission tags (`rendering_pipeline.md`
    /// §7.3a, Pool rank).
    pub(in crate::render) mirror_lighting: renderer_mirror_lights::MirrorLighting,
    /// Maximum dynamic-direct prefix accepted from the scripting bridge.
    pub(in crate::render) dynamic_light_capacity: usize,
    pub(in crate::render) light_count: u32,
    /// Raw `AnimatedBakedLights` roster length.
    pub(in crate::render) animated_baked_light_count: usize,
    /// Dynamic tier, raw animated-baked tail, then promoted static records.
    pub(in crate::render) total_light_count: u32,
    pub(in crate::render) level_lights: Vec<MapLight>,
    pub(in crate::render) level_light_source_indices: Vec<usize>,
    pub(in crate::render) level_light_influences: Vec<LightInfluence>,
    pub(in crate::render) entity_shadow_lights: Vec<MapLight>,
    pub(in crate::render) entity_shadow_light_influences: Vec<LightInfluence>,
    pub(in crate::render) entity_shadow_light_source_indices: Vec<usize>,
    pub(in crate::render) entity_shadow_spec_light_indices: Vec<u32>,
    pub(in crate::render) shadow_candidate_lights: Vec<MapLight>,
    pub(in crate::render) shadow_candidate_source_indices: Vec<usize>,
    pub(in crate::render) shadow_candidate_selection_indices: Vec<Option<usize>>,
    pub(in crate::render) shadow_candidate_animated_baked_indices: Vec<Option<usize>>,
    pub(in crate::render) shadow_candidate_influences: Vec<LightInfluence>,
    pub(in crate::render) light_effective_brightness: Vec<f32>,
    pub(in crate::render) animated_light_window_brightness: Vec<f32>,
    pub(in crate::render) last_lights_upload: Vec<u8>,
    pub(in crate::render) authored_animated_tail: Vec<u8>,
    pub(in crate::render) last_influence_upload: Vec<u8>,
    pub(in crate::render) lights_pack_scratch: Vec<u8>,
    pub(in crate::render) influence_pack_scratch: Vec<u8>,
    /// Load-fixed: the level's id-42 mask pool is bound.
    pub(in crate::render) shadowmask_present: bool,
    pub(in crate::render) forward_shadowmask_metadata_scratch: Vec<u8>,

    // --- Promoted baked lights (the depth caches stay renderer-wide) ---
    pub(in crate::render) promoted_static_states: Vec<PromotedBakedLightState>,
    pub(in crate::render) promoted_animated_states: Vec<PromotedBakedLightState>,
    pub(in crate::render) promoted_baked_records: Vec<PromotedBakedLightRecord>,
    pub(in crate::render) promoted_baked_cache_layers: Vec<i32>,
    pub(in crate::render) promoted_static_weights: Vec<f32>,
    pub(in crate::render) promoted_static_weight_buffer: wgpu::Buffer,
    pub(in crate::render) promoted_static_weight_scratch: Vec<u8>,
    pub(in crate::render) promoted_baked_last_update_time: Option<f64>,
    /// The next light-slot update is this level's reveal frame.
    pub(in crate::render) promotions_start_whole: bool,

    // --- SH, lightmap and SDF families ---
    pub(in crate::render) sh_volume_resources: ShVolumeResources,
    pub(in crate::render) sh_residency_report: ShResidencyReport,
    pub(in crate::render) lightmap_residency_report: LightmapResidencyReport,
    /// Streaming-only residency ownership, its GPU pools included.
    pub(in crate::render) sh_streaming: Option<sh_streaming::ShResidencyState>,
    pub(in crate::render) sdf_atlas_resources: SdfAtlasResources,
    pub(in crate::render) lightmap_mode: postretro_level_loader::LightmapMode,
    #[cfg(feature = "dev-tools")]
    pub(in crate::render) sh_delta_volumes_meta: Vec<sh_volume::DeltaVolumeMeta>,
    #[cfg(feature = "dev-tools")]
    pub(in crate::render) sh_probe_readback: sh_diagnostics::ShProbeReadback,
    pub(in crate::render) sh_compose: ShComposeResources,
    pub(in crate::render) direct_sh_compose: DirectShComposeResources,
    pub(in crate::render) billboard_direct_scatter_compose: BillboardDirectScatterComposeResources,
    pub(in crate::render) lightmap_resources: LightmapResources,
    pub(in crate::render) animated_lightmap: animated_lightmap::AnimatedLightmapResources,

    // --- Pass level states ---
    /// The SDF shadow pass's group 1 and 2 over this level's SH depth moments
    /// and static-light buffers.
    pub(in crate::render) sdf_shadow: sdf_shadow::SdfShadowLevelState,
    /// The skinned-mesh pass's group 2 over this level's light buffers.
    pub(in crate::render) mesh_lights: LevelLightGroups,
    /// Kinematic-brush mover geometry and its group 2.
    pub(in crate::render) kinematic: kinematic_brush::KinematicLevelState,
    /// This level's fluid surfaces; `None` until `install_fluid_surfaces`
    /// finds a drawable face.
    pub(in crate::render) fluid_surfaces: Option<fluid_surface::InstalledFluidSurfaces>,
    /// Canonical fog volumes and their hysteresis.
    pub(in crate::render) fog: fog_pass::FogLevelState,
    /// Per-cell bitmask of overlapping fog volumes (PRL section 31).
    pub(in crate::render) fog_cell_masks: Option<Vec<u32>>,
    /// AABBs of fog volumes active this frame, for fog-spot culling.
    pub(in crate::render) active_fog_aabbs: Vec<(Vec3, Vec3)>,
}

impl LevelResourceSet {
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
        super::renderer_types::sh_sampler_lattice(
            self.sh_streaming
                .as_ref()
                .map(sh_streaming::ShResidencyState::base_lattice),
            self.sh_volume_resources.present.then_some((
                self.sh_volume_resources.grid_origin,
                self.sh_volume_resources.cell_size,
            )),
        )
    }

    /// The world materials this set holds, taken out for a geometry
    /// reinstall that keeps them.
    pub(in crate::render) fn take_textures(&mut self) -> LevelTextures {
        LevelTextures {
            loaded_textures: std::mem::take(&mut self.loaded_textures),
            gpu_textures: std::mem::take(&mut self.gpu_textures),
            stored_texture_materials: std::mem::take(&mut self.stored_texture_materials),
        }
    }
}

impl Renderer {
    /// The active level set. Only paths a level gates call this; world-less
    /// paths read `full().level` and skip on `None`.
    pub(in crate::render) fn level(&self) -> &LevelResourceSet {
        self.full()
            .level
            .as_ref()
            .expect("a level-gated renderer path requires an active level set")
    }

    pub(in crate::render) fn level_mut(&mut self) -> &mut LevelResourceSet {
        self.full_mut()
            .level
            .as_mut()
            .expect("a level-gated renderer path requires an active level set")
    }

    /// The active level's fluid surfaces, `None` with no level or none
    /// installed.
    pub(in crate::render) fn fluid_surfaces(
        &self,
    ) -> Option<&fluid_surface::InstalledFluidSurfaces> {
        self.full()
            .level
            .as_ref()
            .and_then(|level| level.fluid_surfaces.as_ref())
    }

    /// Test view over the fluid pass and the active level's surfaces.
    #[cfg(test)]
    pub(in crate::render) fn fluid_test(&self) -> fluid_surface::FluidTestView<'_> {
        let full = self.full();
        fluid_surface::FluidTestView {
            pass: &full.fluid_surface,
            installed: full
                .level
                .as_ref()
                .and_then(|level| level.fluid_surfaces.as_ref()),
        }
    }

    /// Install a level with no geometry, lights, textures or lighting
    /// sections: the empty level the renderer drew before per-load sets,
    /// for tests that exercise level passes without a fixture map.
    #[cfg(test)]
    pub(in crate::render) fn install_empty_level_for_test(&mut self) {
        let empty_bvh = BvhTree {
            nodes: Vec::new(),
            leaves: Vec::new(),
            root_node_index: 0,
        };
        let geometry = LevelGeometry {
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
            texture_materials: &[],
        };
        self.install_level_geometry(&geometry, Default::default());
    }

    /// Whether a level's renderer resources are installed.
    pub fn has_level_resources(&self) -> bool {
        self.full.as_ref().is_some_and(|full| full.level.is_some())
    }
}
