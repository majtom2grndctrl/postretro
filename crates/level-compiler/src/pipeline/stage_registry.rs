//! Stable stage identities and ordered build-summary descriptors.
//! See: context/lib/build_pipeline.md §Progress reporting, controls, and logging

use crate::{map_data, map_needs_sdf_atlas};

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
    CellVisibility,
    NavMesh,
    ShBake,
    DeltaShBake,
    DirectShBake,
    AnimatedDirectShBake,
    EntityShadowLights,
    DirectShDeltaBake,
    BillboardDirectScatterBake,
    ChunkLightList,
    AtlasPreparation,
    LightmapBake,
    ShadowmaskAtlas,
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
            Self::CellVisibility => "Cell Visibility",
            Self::NavMesh => "NavMesh",
            Self::ShBake => "SH Bake",
            Self::DeltaShBake => "Delta SH Bake",
            Self::DirectShBake => "Direct SH Bake",
            Self::AnimatedDirectShBake => "Animated Direct SH Bake",
            Self::EntityShadowLights => "EntityShadowLights",
            Self::DirectShDeltaBake => "Direct SH Delta Bake",
            Self::BillboardDirectScatterBake => "Billboard Direct Scatter Bake",
            Self::ChunkLightList => "ChunkLightList",
            Self::AtlasPreparation => "Atlas Preparation",
            Self::LightmapBake => "Lightmap Bake",
            Self::ShadowmaskAtlas => "ShadowmaskAtlas",
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
            Self::CellVisibility => "Cell visibility bake...",
            Self::NavMesh => "NavMesh bake...",
            Self::ShBake => "SH volume bake...",
            Self::DeltaShBake => "Delta SH volume bake...",
            Self::DirectShBake => "Direct SH volume bake...",
            Self::AnimatedDirectShBake => "Animated direct SH delta bake...",
            Self::EntityShadowLights => "Entity shadow light selection...",
            Self::DirectShDeltaBake => "Direct SH delta volume bake...",
            Self::BillboardDirectScatterBake => "Billboard direct scatter bake...",
            Self::ChunkLightList => "Chunk light list bake...",
            Self::AtlasPreparation => "Atlas preparation...",
            Self::LightmapBake => "Lightmap bake...",
            Self::ShadowmaskAtlas => "Shadowmask atlas bake...",
            Self::AnimatedLightChunks => "Animated light chunks...",
            Self::AnimatedWeightMaps => "Animated light weight maps...",
            Self::SdfAtlasBake => "SDF atlas bake...",
            Self::TextureMips => "Texture mip bake...",
            Self::Packing => "Packing and writing...",
        }
    }
}

pub(crate) const ORDERED_STAGES: [StageId; 25] = [
    StageId::Parsing,
    StageId::DataScript,
    StageId::TextureValidation,
    StageId::Partitioning,
    StageId::Visibility,
    StageId::Geometry,
    StageId::BvhBuild,
    StageId::CellVisibility,
    StageId::NavMesh,
    StageId::ShBake,
    StageId::DeltaShBake,
    StageId::DirectShBake,
    StageId::AnimatedDirectShBake,
    StageId::EntityShadowLights,
    StageId::DirectShDeltaBake,
    StageId::BillboardDirectScatterBake,
    StageId::ChunkLightList,
    StageId::AtlasPreparation,
    StageId::LightmapBake,
    StageId::ShadowmaskAtlas,
    StageId::AnimatedLightChunks,
    StageId::AnimatedWeightMaps,
    StageId::SdfAtlasBake,
    StageId::TextureMips,
    StageId::Packing,
];

/// Return the ordered stage descriptors predicted for parsed map content.
///
/// Prediction is side-effect free and can run before the bake worker starts.
/// SDF presence intentionally uses the same content predicate as execution.
pub fn planned_stages(lights: &[map_data::MapLight]) -> Vec<StageDescriptor> {
    planned_stages_for_sdf(map_needs_sdf_atlas(lights))
}

pub(super) fn planned_stages_for_sdf(needs_sdf: bool) -> Vec<StageDescriptor> {
    ORDERED_STAGES
        .iter()
        .copied()
        .map(|id| StageDescriptor {
            id,
            label: id.label(),
            predicted_present: id != StageId::SdfAtlasBake || needs_sdf,
        })
        .collect()
}
