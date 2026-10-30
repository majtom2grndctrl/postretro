// The level-geometry install contract: the borrowed level sections that
// `install_level_geometry` consumes, and the section-45 roster it reserves.
// See: context/lib/boot_sequence.md §3

use super::*;

/// SH ownership reaching the renderer installation boundary. Streaming keeps
/// only loader metadata and a retained positional manifest; renderer pool
/// installation arrives later through bounded drain batches.
pub enum LevelGeometryShStorage<'a> {
    Legacy,
    Streaming(&'a postretro_level_loader::ShStreamManifest),
}

/// A level whose lightmap cell blocks (ids 22/42) stream. Install builds an
/// empty pool; blocks arrive through `Renderer::drain_lightmap_residency`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelGeometryLightmapStreaming {
    /// The level's content identity, which every drain batch must carry
    /// (`LightmapStreamManifest::content_tag`).
    pub content_tag: [u8; 32],
    /// The first pool generation's layer cap. Later drains carry the live
    /// cap; raising it past this takes effect at the next growth.
    pub pool_cap_layers: u32,
}

pub struct LevelGeometry<'a> {
    pub vertices: &'a [postretro_render_data::geometry::WorldVertex],
    /// One owner entry per vertex: its face's id-42 owners, the forward
    /// pass's second vertex buffer. Any length other than `vertices.len()`
    /// (a level without id 42 hands an all-empty stream; hand-built geometry
    /// may hand none) installs as all-empty: no static non-SDF specular.
    pub vertex_spec_owners: &'a [postretro_level_format::shadowmask_atlas::FaceOwners],
    pub indices: &'a [u32],
    pub bvh: &'a BvhTree,
    pub lights: &'a [MapLight],
    pub light_influences: &'a [LightInfluence],
    /// `None` means no `OctahedralShVolumeSection`; renderer binds dummy
    /// 1×1 atlas resources and shader skips octahedral SH sampling.
    pub sh_volume: Option<&'a postretro_level_format::sh_volume::OctahedralShVolumeSection>,
    pub sh_storage: LevelGeometryShStorage<'a>,
    /// Id-22 cell-block index. `None` or zero blocks is placeholder mode: the
    /// 1×1 white placeholder, and bumped-Lambert falls back to flat white.
    /// The index only; install hands the blobs to `install_level_geometry`.
    pub lightmap: Option<&'a postretro_level_format::lightmap::LightmapBlockIndex>,
    /// `Some` when the level streams its cell blocks: install hands no
    /// payloads and builds an empty streamed pool over `lightmap` and
    /// `shadowmask_atlas`. `None` installs every block at once.
    pub lightmap_streaming: Option<LevelGeometryLightmapStreaming>,
    /// `None` → `has_chunk_grid == 0`: SDF light selection scans the full
    /// spec buffer and billboard specular skips its static lights.
    pub chunk_light_list:
        Option<&'a postretro_level_format::chunk_light_list::ChunkLightListSection>,
    /// `None` when the map has zero animated lights.
    pub animated_light_chunks:
        Option<&'a postretro_level_format::animated_light_chunks::AnimatedLightChunksSection>,
    /// `None` → 1×1 zero atlas bound on group 4.
    pub animated_light_weight_maps: Option<
        &'a postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection,
    >,
    /// `None` → compose pass falls back to a base→total copy.
    pub delta_sh_volumes:
        Option<&'a postretro_level_format::delta_sh_volumes::DeltaShVolumesSection>,
    /// V4 node-aware stored-tile atlas of baked static-light direct SH,
    /// sampled by dynamic pipelines (mesh + billboard). `None` → renderer binds
    /// a 4×4 BC6H zero dummy and dynamic shaders skip the direct sample
    /// (indirect-only).
    pub direct_sh_volume:
        Option<&'a postretro_level_format::direct_sh_volume::DirectShVolumeSection>,
    /// Sparse per-selected-light direct SH deltas used to compose the sampled
    /// direct atlas when static lights promote into shadow pools.
    pub direct_sh_delta_volumes:
        Option<&'a postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection>,
    /// Sparse per-animated-baked-light direct SH deltas. Its descriptor map and
    /// CSR light indices use the independent AnimatedBakedLights namespace.
    pub animated_direct_sh_delta_volumes: Option<
        &'a postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection,
    >,
    /// Normal-free static direct scatter for billboard receivers. The renderer
    /// selects its legacy fallback when this is absent or cannot fit the GPU.
    pub billboard_direct_scatter_volume: Option<
        &'a postretro_level_format::billboard_direct_scatter_volume::BillboardDirectScatterVolumeSection,
    >,
    /// Dense section-48 animated deltas paired by the loader with section 45.
    pub animated_billboard_direct_scatter_delta_volumes: Option<
        &'a postretro_level_format::animated_billboard_direct_scatter_delta_volumes::AnimatedBillboardDirectScatterDeltaVolumesSection,
    >,
    /// Selection-order list of global level-light indices eligible for runtime
    /// entity-shadow promotion.
    pub entity_shadow_lights: &'a [u32],
    /// Id 42's index: per-face owners and one record per id-22 cell block.
    /// Its presence alone plans the mask pool; owners reach the forward pass
    /// through `vertex_spec_owners`. The index only; install hands the groups
    /// to `install_level_geometry` inside each block's payload.
    pub shadowmask_atlas:
        Option<&'a postretro_level_format::shadowmask_atlas::ShadowmaskBlockIndex>,
    /// `None` → no SDF static-occluder atlas; runtime SDF shadow pass disabled.
    /// An empty-geometry section (zero grid dims) is treated the same way.
    pub sdf_atlas: Option<&'a postretro_level_format::sdf_atlas::SdfAtlasSection>,
    /// Lightmap bake mode from the id-22 header: whether baked static-direct
    /// samples include static-light visibility. The compiler writes only
    /// `Shadowed`. `Unshadowed` is recorded but not honoured; it renders as
    /// the baked irradiance with no shadow term.
    pub lightmap_mode: postretro_level_loader::LightmapMode,
    /// Per-cell BVH-leaf draw index (PRL section 37), cross-validated at load.
    /// `None` only for no installed level or an empty-BVH map. Non-empty BVHs
    /// require this index at load; missing or invalid data is a load error.
    /// Whole-BVH tree-walk fallback is a per-frame runtime path for `DrawAll`,
    /// solid/exterior/no-portals visibility, out-of-range gathered cell ids, or no candidate
    /// cull pipeline.
    pub cell_draw_index: Option<&'a postretro_level_loader::CellDrawIndex>,
    /// Runtime-loaded local-space kinematic brush mover geometry (PRL section
    /// 43). Uploaded into a renderer-owned dynamic-object pass, never into the
    /// static world BVH/indirect buffers.
    pub kinematic_geometry: Option<&'a postretro_level_loader::KinematicGeometry>,
    /// Runtime cells in id order. A streamed-SH level indexes their bounds
    /// at install so per-frame sample gating names cells by id.
    pub cells: &'a [postretro_level_loader::CellData],
    pub texture_materials: &'a [postretro_render_data::material::Material],
}

impl LevelGeometry<'_> {
    /// Section 45's roster is global metadata even when its tile payload is
    /// streamed. The light bridge and renderer must reserve the same raw tail.
    pub(crate) fn animated_baked_roster(&self) -> (&[u32], &[u32]) {
        match &self.sh_storage {
            LevelGeometryShStorage::Legacy => {
                animated_baked_roster_from_sources(self.animated_direct_sh_delta_volumes, None)
            }
            LevelGeometryShStorage::Streaming(manifest) => animated_baked_roster_from_sources(
                None,
                manifest.sources().animated_direct_delta.as_ref(),
            ),
        }
    }
}

fn animated_baked_roster_from_sources<'a>(
    legacy: Option<
        &'a postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection,
    >,
    streamed: Option<&'a postretro_level_loader::ShStreamSparseMetadata>,
) -> (&'a [u32], &'a [u32]) {
    if let Some(metadata) = streamed {
        return (
            &metadata.animation_descriptor_indices,
            &metadata.affinity_lights,
        );
    }
    legacy.map_or((&[], &[]), |section| {
        (
            &section.animation_descriptor_indices,
            &section.affinity_lights,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression: streaming omits the whole id-45 section, but its retained
    // roster must still size the forward tail and match the script bridge.
    #[test]
    fn streamed_animated_baked_roster_survives_without_legacy_section() {
        let metadata = postretro_level_loader::ShStreamSparseMetadata {
            section_id: 45,
            internal_version: 4,
            affinity_dims: [1, 1, 1],
            tile_dimension: 6,
            tile_border: 1,
            valid_probe_masks: Vec::new(),
            cell_levels: Vec::new(),
            affinity_offsets: Vec::new(),
            affinity_lights: vec![0, 2],
            animation_descriptor_indices: vec![7, 3, 7],
        };
        let (descriptors, affinity_lights) =
            animated_baked_roster_from_sources(None, Some(&metadata));

        assert_eq!(descriptors, &[7, 3, 7]);
        assert_eq!(affinity_lights, &[0, 2]);
        assert_eq!(
            scripted_light_capacity(2, 2, descriptors),
            5 + RUNTIME_DYNAMIC_LIGHT_RESERVE,
            "the raw three-row animated tail must reserve forward descriptor and sample slots"
        );
    }
}
