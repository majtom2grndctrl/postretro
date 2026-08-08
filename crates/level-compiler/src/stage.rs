// Compiler stage identity vocabulary: the stable `StageId` enum and its
// `StageDescriptor`. Split out of `pipeline.rs` so the lightweight consumers
// (`reporter`, `tui`) and the library target depend on the stage vocabulary
// without pulling in the pipeline orchestration (which references binary-root
// CLI symbols). `pipeline` re-exports these, so existing `crate::pipeline::StageId`
// paths keep working.
// See: context/lib/build_pipeline.md

/// Stable identity for one ordered compiler stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StageId {
    Parsing,
    DataScript,
    TextureValidation,
    Partitioning,
    Visibility,
    Geometry,
    BvhBuild,
    NavMesh,
    LightmapBake,
    ShBake,
    DeltaShBake,
    DirectShBake,
    AnimatedDirectShBake,
    EntityShadowLights,
    DirectShDeltaBake,
    ShadowmaskAtlas,
    ChunkLightList,
    AnimatedLightChunks,
    AnimatedWeightMaps,
    SdfAtlasBake,
    TextureMips,
    Packing,
}

/// A stage's stable identity, Build Summary label, and predicted presence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageDescriptor {
    pub id: StageId,
    pub label: &'static str,
    pub predicted_present: bool,
}

impl StageId {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Parsing => "Parsing",
            Self::DataScript => "DataScript",
            Self::TextureValidation => "TexValidation",
            Self::Partitioning => "Partitioning",
            Self::Visibility => "Visibility",
            Self::Geometry => "Geometry",
            Self::BvhBuild => "BVH Build",
            Self::NavMesh => "NavMesh",
            Self::LightmapBake => "Lightmap Bake",
            Self::ShBake => "SH Bake",
            Self::DeltaShBake => "Delta SH Bake",
            Self::DirectShBake => "Direct SH Bake",
            Self::AnimatedDirectShBake => "Animated Direct SH Bake",
            Self::EntityShadowLights => "EntityShadowLights",
            Self::DirectShDeltaBake => "Direct SH Delta Bake",
            Self::ShadowmaskAtlas => "ShadowmaskAtlas",
            Self::ChunkLightList => "ChunkLightList",
            Self::AnimatedLightChunks => "AnimLightChunks",
            Self::AnimatedWeightMaps => "AnimWeightMaps",
            Self::SdfAtlasBake => "SDF Atlas Bake",
            Self::TextureMips => "TextureMips",
            Self::Packing => "Packing",
        }
    }

    pub const fn progress_label(self) -> &'static str {
        match self {
            Self::Parsing => "Parsing map...",
            Self::DataScript => "Data script compilation...",
            Self::TextureValidation => "Texture color-space validation...",
            Self::Partitioning => "BSP partitioning...",
            Self::Visibility => "Visibility computation...",
            Self::Geometry => "Geometry extraction...",
            Self::BvhBuild => "BVH build...",
            Self::NavMesh => "NavMesh bake...",
            Self::LightmapBake => "Lightmap bake...",
            Self::ShBake => "SH volume bake...",
            Self::DeltaShBake => "Delta SH volume bake...",
            Self::DirectShBake => "Direct SH volume bake...",
            Self::AnimatedDirectShBake => "Animated direct SH delta bake...",
            Self::EntityShadowLights => "Entity shadow light selection...",
            Self::DirectShDeltaBake => "Direct SH delta volume bake...",
            Self::ShadowmaskAtlas => "Shadowmask atlas bake...",
            Self::ChunkLightList => "Chunk light list bake...",
            Self::AnimatedLightChunks => "Animated light chunks...",
            Self::AnimatedWeightMaps => "Animated light weight maps...",
            Self::SdfAtlasBake => "SDF atlas bake...",
            Self::TextureMips => "Texture mip bake...",
            Self::Packing => "Packing and writing...",
        }
    }
}
