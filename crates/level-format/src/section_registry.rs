// Stable PRL section registry.
// See: context/lib/build_pipeline.md §PRL section IDs

/// Known section type IDs.
///
/// Retired runtime BSP IDs are retained only so modern PRLs containing them
/// can be rejected; other retired IDs are omitted to avoid accidental reuse.
/// The loader skips unknown IDs gracefully; older `.prl` files produced before
/// the BVH refactor will fail to decode because the geometry format changed
/// and a `Bvh` section is now required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SectionId {
    /// Legacy BSP node section ID. Retained only to detect stale-format PRLs.
    BspNodes = 12,

    /// Legacy BSP leaf section ID. Retained only to detect stale-format PRLs.
    BspLeaves = 13,

    // 14 (LeafPvs) retired — precomputed PVS removed; portal traversal is the only vis path.
    /// Portal graph for runtime portal traversal.
    Portals = 15,

    /// Flat list of texture name strings, indexed by `FaceMeta.texture_index`.
    TextureNames = 16,

    /// Geometry section: 28-byte vertices (position + UV + octahedral normal
    /// + octahedral tangent with bitangent sign) and 8-byte `FaceMeta`.
    Geometry = 17,

    /// AlphaLights section (interim). Flat per-light record array for the
    /// direct-lighting path. Will be replaced by an entity-system
    /// serialisation in Milestone 6+.
    /// See `alpha_lights::AlphaLightsSection`.
    AlphaLights = 18,

    /// Global BVH: flat node + leaf arrays. See `bvh::BvhSection`.
    Bvh = 19,

    /// Legacy SH irradiance volume id. Retired: the section struct was removed
    /// once the loader/compiler moved entirely to `OctahedralShVolume` (id 34).
    /// The registry entry is retained so the id is not reused and missing-section
    /// lookups stay well-defined.
    ShVolume = 20,

    /// Per-light influence volumes (compile-time sphere bounds) for spatial
    /// culling in the fragment shader and CPU-side shadow-slot allocation.
    /// See `light_influence::LightInfluenceSection`.
    LightInfluence = 21,

    /// Directional lightmap atlas: per-texel irradiance + dominant incoming
    /// direction from static (non-dynamic) lights. Sampled at runtime via
    /// per-vertex lightmap UVs; bumped-Lambert correction applies normal-map
    /// response to the baked direction. See `lightmap::LightmapSection`.
    Lightmap = 22,

    /// World-space uniform chunk grid with per-chunk static-light index lists.
    /// Runtime consumes it to bound per-fragment specular iteration to the
    /// lights a chunk can actually be reached by. See
    /// `chunk_light_list::ChunkLightListSection`.
    ChunkLightList = 23,

    /// Per-BVH-leaf ranges into a flat array of per-chunk animated-light index
    /// lists, used by the future per-light weight-map animated-lightmap
    /// pipeline. See `animated_light_chunks::AnimatedLightChunksSection`.
    AnimatedLightChunks = 24,

    /// Per-chunk atlas rectangles plus per-texel (offset, count) lists into a
    /// flat (light_index, weight) pool. Baked at compile time; composed at
    /// runtime into the animated-lightmap contribution atlas. See
    /// `animated_light_weight_maps::AnimatedLightWeightMapsSection`.
    AnimatedLightWeightMaps = 25,

    /// Optional script tags for AlphaLights (one entry per AlphaLights record,
    /// same order). Authored via the FGD `_tags` key; consumed by the runtime
    /// to populate the scripting entity registry's tag column so
    /// `world.query({ component: "light", tag: "<tag>" })` can filter lights.
    /// See `light_tags::LightTagsSection`.
    LightTags = 26,

    /// Per-animated-light delta SH probe grids — each light's contribution at
    /// peak brightness (brightness = 1.0, base color), used by the runtime
    /// compose pass that blends animated lights into the SH irradiance volume.
    /// See `delta_sh_volumes::DeltaShVolumesSection`.
    DeltaShVolumes = 27,

    /// Compiled data-script bytes (QuickJS-compatible JS for `.ts`/`.js` source,
    /// raw Luau for `.luau` source) plus the original source path for future
    /// hot-reload support. Present only when the worldspawn `data_script` KVP
    /// is set. See `data_script::DataScriptSection`.
    DataScript = 28,

    /// Per-entity classname, origin, angles, KVP bag, and tags for non-light,
    /// non-worldspawn map entities. The runtime drives classname dispatch
    /// (`apply_classname_dispatch`) from this section. See
    /// `map_entity::MapEntitySection`.
    MapEntity = 29,

    /// Per-region volumetric fog volumes (AABB + density/scatter
    /// parameters) plus the worldspawn `fog_pixel_scale` downscale factor and
    /// the worldspawn `initial_gravity` scalar (m/s²).
    /// Always emitted by `prl-build` so worldspawn-scoped data is honoured
    /// even when no `fog_volume` brushes are present.
    /// See `fog_volumes::FogVolumesSection`.
    FogVolumes = 30,

    /// Per-cell bitmask of overlapping fog volumes (bit `i` = volume `i`
    /// overlaps this cell). Emitted when the map has canonical fog volumes:
    /// `fog_volume`, `fog_lamp`, or `fog_tube`. Absence is valid only when no
    /// canonical fog volumes exist.
    /// See `fog_cell_masks::FogCellMasksSection`.
    FogCellMasks = 31,

    /// Flat array of 32-byte blake3 cache keys, one per entry in
    /// `TextureNames` (same index order). Each key is the blake3 of the raw
    /// PNG source bytes for that texture; the runtime uses it to locate the
    /// matching baked `.prm` mip sidecar without rehashing PNGs at load time.
    /// See `texture_cache_keys::TextureCacheKeysSection` and the `prm` module
    /// for the sidecar wire format.
    TextureCacheKeys = 32,

    /// SDF atlas of static world geometry: brick top-level index + quantized
    /// `i16` per-brick distances + coarse per-brick `f32` fallback. Consumed
    /// by the runtime SDF static-occluder shadow pass; absence is a valid
    /// "no SDF" load (degradation, not an error).
    /// See `sdf_atlas::SdfAtlasSection`.
    SdfAtlas = 33,

    /// Base irradiance probe volume encoded as a 2D octahedral `Rgba16Float`
    /// atlas. Sibling replacement for `ShVolume` during the one-way migration;
    /// validity, grid metadata, depth moments, and animation descriptors keep
    /// the same meaning. See `sh_volume::OctahedralShVolumeSection`.
    OctahedralShVolume = 34,

    /// Dense octahedral irradiance atlas carrying DIRECT light from STATIC
    /// lights, sampled at runtime by dynamic objects (entities/billboards).
    /// Sibling to `OctahedralShVolume` (the INDIRECT atlas) with byte-identical
    /// tile geometry so the runtime sampler is shared, but with no depth moments
    /// and no animation data — those are read from the indirect section, whose
    /// probe grid is byte-identical in position. Stored BC6H-compressed at rest.
    /// See `direct_sh_volume::DirectShVolumeSection`.
    DirectShVolume = 35,

    /// Baked navigation graph: convex walkable regions joined by portals (the
    /// pathfinding query surface — A* over regions, funnel over portal
    /// segments). Records the canonical agent parameters it was baked with.
    /// `portal_count == 0` is valid (a single isolated region); a present
    /// section always carries `region_count >= 1`. See `navmesh::NavMeshSection`.
    NavMesh = 36,

    /// Per-cell draw index: each cell's owned BVH-leaf spans, baked as a CSR
    /// (offset table + flat span payload) so the runtime camera cull gathers
    /// only visible cells' leaves as candidates. Required when the BVH has
    /// non-empty leaves; omitted only for zero-leaf maps. Missing or invalid
    /// data when required is a load error.
    /// See `cell_draw_index::CellDrawIndexSection`.
    CellDrawIndex = 37,

    /// Runtime visibility cells, preserving the compiler BSP leaf id space
    /// one-to-one. Carries cell bounds, flags, face ranges, and per-cell portal
    /// adjacency ranges without BSP split planes. See `cells::CellsSection`.
    Cells = 38,

    /// Point-to-cell lookup decision tree. Version 1 mirrors the compiler BSP
    /// split planes but its terminal children are cell ids, not BSP leaf
    /// records. See `cell_locator::CellLocatorSection`.
    CellLocator = 39,

    /// Compiler-selected baked-tier level-light indices that may later be
    /// promoted into runtime entity-shadow pools. See
    /// `entity_shadow_lights::EntityShadowLightsSection`.
    EntityShadowLights = 40,

    /// Per-selected-light sparse direct-SH delta tiles, indexed by affinity
    /// cell. See `direct_sh_delta_volumes::DirectShDeltaVolumesSection`.
    DirectShDeltaVolumes = 41,

    /// Per-selected-light baked world-visibility masks for up to four
    /// overlapping selected lights, packed as two BC5 `.rg` mask groups
    /// side by side per layer (slot `s` in group `s / 2`, channel `s % 2`).
    /// See `shadowmask_atlas::ShadowmaskAtlasSection`.
    ShadowmaskAtlas = 42,

    /// Origin-relative brush geometry and waypoint records for deterministic
    /// kinematic movers. See `kinematic_geometry::KinematicGeometrySection`.
    KinematicGeometry = 43,
    /// Invisible AABB brush triggers with declarative mover commands.
    TriggerVolumes = 44,

    /// Per-animated-baked-light sparse direct-SH delta tiles, indexed by
    /// affinity cell. See
    /// `animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection`.
    AnimatedDirectShDeltaVolumes = 45,

    /// View-independent cell-to-cell portal-graph coupling. The optional
    /// section stores a conservative reachability partition plus graded pair
    /// details; absent data deliberately falls back conservatively at runtime.
    /// See `cell_visibility::CellVisibilitySection`.
    CellVisibility = 46,

    /// Dense normal-free direct-scatter samples for billboards. The grid
    /// mirrors `OctahedralShVolume` and carries one `Rgba16Float` sample per
    /// probe. See `billboard_direct_scatter_volume::BillboardDirectScatterVolumeSection`.
    BillboardDirectScatterVolume = 47,

    /// Dense animated direct-scatter deltas for billboards. Its descriptor
    /// mapping and CSR layout mirror `AnimatedDirectShDeltaVolumes` (id 45).
    /// See `animated_billboard_direct_scatter_delta_volumes::AnimatedBillboardDirectScatterDeltaVolumesSection`.
    AnimatedBillboardDirectScatterDeltaVolumes = 48,

    /// Inert cell-cluster metadata and grid-relative SH resource addressing.
    /// See `cluster_directory::ClusterDirectorySection`.
    ClusterDirectory = 49,

    /// Optional independently-readable cluster-major SH payloads for the
    /// streaming residency path. Older loaders skip this section and retain
    /// their whole-section SH behavior.
    ClusterShPayloads = 50,
}

impl SectionId {
    pub fn from_u32(value: u32) -> Option<Self> {
        match value {
            12 => Some(Self::BspNodes),
            13 => Some(Self::BspLeaves),
            15 => Some(Self::Portals),
            16 => Some(Self::TextureNames),
            17 => Some(Self::Geometry),
            18 => Some(Self::AlphaLights),
            19 => Some(Self::Bvh),
            20 => Some(Self::ShVolume),
            21 => Some(Self::LightInfluence),
            22 => Some(Self::Lightmap),
            23 => Some(Self::ChunkLightList),
            24 => Some(Self::AnimatedLightChunks),
            25 => Some(Self::AnimatedLightWeightMaps),
            26 => Some(Self::LightTags),
            27 => Some(Self::DeltaShVolumes),
            28 => Some(Self::DataScript),
            29 => Some(Self::MapEntity),
            30 => Some(Self::FogVolumes),
            31 => Some(Self::FogCellMasks),
            32 => Some(Self::TextureCacheKeys),
            33 => Some(Self::SdfAtlas),
            34 => Some(Self::OctahedralShVolume),
            35 => Some(Self::DirectShVolume),
            36 => Some(Self::NavMesh),
            37 => Some(Self::CellDrawIndex),
            38 => Some(Self::Cells),
            39 => Some(Self::CellLocator),
            40 => Some(Self::EntityShadowLights),
            41 => Some(Self::DirectShDeltaVolumes),
            42 => Some(Self::ShadowmaskAtlas),
            43 => Some(Self::KinematicGeometry),
            44 => Some(Self::TriggerVolumes),
            45 => Some(Self::AnimatedDirectShDeltaVolumes),
            46 => Some(Self::CellVisibility),
            47 => Some(Self::BillboardDirectScatterVolume),
            48 => Some(Self::AnimatedBillboardDirectScatterDeltaVolumes),
            49 => Some(Self::ClusterDirectory),
            50 => Some(Self::ClusterShPayloads),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_ids_round_trip_and_retired_holes_stay_unassigned() {
        let registered = [
            SectionId::BspNodes,
            SectionId::BspLeaves,
            SectionId::Portals,
            SectionId::TextureNames,
            SectionId::Geometry,
            SectionId::AlphaLights,
            SectionId::Bvh,
            SectionId::ShVolume,
            SectionId::LightInfluence,
            SectionId::Lightmap,
            SectionId::ChunkLightList,
            SectionId::AnimatedLightChunks,
            SectionId::AnimatedLightWeightMaps,
            SectionId::LightTags,
            SectionId::DeltaShVolumes,
            SectionId::DataScript,
            SectionId::MapEntity,
            SectionId::FogVolumes,
            SectionId::FogCellMasks,
            SectionId::TextureCacheKeys,
            SectionId::SdfAtlas,
            SectionId::OctahedralShVolume,
            SectionId::DirectShVolume,
            SectionId::NavMesh,
            SectionId::CellDrawIndex,
            SectionId::Cells,
            SectionId::CellLocator,
            SectionId::EntityShadowLights,
            SectionId::DirectShDeltaVolumes,
            SectionId::ShadowmaskAtlas,
            SectionId::KinematicGeometry,
            SectionId::TriggerVolumes,
            SectionId::AnimatedDirectShDeltaVolumes,
            SectionId::CellVisibility,
            SectionId::BillboardDirectScatterVolume,
            SectionId::AnimatedBillboardDirectScatterDeltaVolumes,
            SectionId::ClusterDirectory,
            SectionId::ClusterShPayloads,
        ];

        for section_id in registered {
            assert_eq!(SectionId::from_u32(section_id as u32), Some(section_id));
        }

        assert_eq!(SectionId::from_u32(14), None);
        assert_eq!(SectionId::from_u32(51), None);
    }
}
