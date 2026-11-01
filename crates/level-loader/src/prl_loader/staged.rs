// Staged PRL parse: the monolithic decode order, handed out in five ordered
// groups as each group's data becomes final, then one late verdict.
// See: context/lib/boot_sequence.md §2 · context/lib/build_pipeline.md §PRL Compilation

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use postretro_level_format::ContainerMeta;
use postretro_level_format::SectionId;
use postretro_level_format::animated_billboard_direct_scatter_delta_volumes::AnimatedBillboardDirectScatterDeltaVolumesSection;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::billboard_direct_scatter_volume::BillboardDirectScatterVolumeSection;
use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::cluster_directory::ClusterDirectorySection;
use postretro_level_format::cluster_sh_payloads::ClusterShPayloadsSection;
use postretro_level_format::data_script::DataScriptSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::fog_volumes::FogVolumeRecord;
use postretro_level_format::ghost_links::GhostLinksSection;
use postretro_level_format::lightmap::LightmapBlockIndex;
use postretro_level_format::map_entity::MapEntityRecord;
use postretro_level_format::navmesh::NavMeshSection;
use postretro_level_format::reflection_probes::ReflectionProbesSection;
use postretro_level_format::sdf_atlas::SdfAtlasSection;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::shadowmask_atlas::{FaceOwners, ShadowmaskBlockIndex};
use postretro_level_format::teleporters::TeleportersSection;
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_level_format::trigger_volumes::TriggerVolumeRecord;
use postretro_render_data::geometry::{BvhTree, WorldVertex};
use postretro_render_data::influence::LightInfluence;

use super::spatial::SpatialDecode;
use super::{
    cell_draw_index, cluster_directory, entities, finalize_lighting, geometry, lighting,
    lighting_sh, log_loaded, spatial, validate_reflection_probes, validation,
};
use crate::lightmap_stream::{LightmapStorage, LightmapStreamingMode};
use crate::prl_container::PrlContainer;
use crate::prl_file::PrlReadCounters;
use crate::prl_lightmap::GpuLightingPayloads;
use crate::sh_stream::{ShStorage, ShStreamBaseMetadata, ShStreamManifest, ShStreamSourceMetadata};
use crate::{
    CellData, CellDrawIndex, CellLocatorChild, CellLocatorNodeData, CellVisibility, FaceMeta,
    KinematicGeometry, LevelWorld, LightmapMode, MapLight, PortalData, PrlLoadError,
};

/// A cooperative cancel flag the staged parse polls between stages, never
/// inside a read. The engine load pool's token implements it; `AtomicBool`
/// does too.
pub trait LoadCancel {
    fn is_cancelled(&self) -> bool;
}

impl LoadCancel for AtomicBool {
    fn is_cancelled(&self) -> bool {
        self.load(Ordering::Acquire)
    }
}

impl<T: LoadCancel + ?Sized> LoadCancel for &T {
    fn is_cancelled(&self) -> bool {
        (**self).is_cancelled()
    }
}

impl<T: LoadCancel + ?Sized> LoadCancel for Arc<T> {
    fn is_cancelled(&self) -> bool {
        (**self).is_cancelled()
    }
}

/// How a staged parse ended when it did not fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagedParseOutcome {
    /// Every hand-out went out and the late verdict accepted the level.
    Complete,
    /// The cancel flag was seen at a stage boundary. No hand-out followed it;
    /// the ones already sent belong to a load that will not commit.
    Cancelled,
}

/// The five hand-outs, in the only order a parse sends them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HandOutStage {
    ShManifest = 1,
    Geometry = 2,
    Spatial = 3,
    Entities = 4,
    Final = 5,
}

/// One group of decoded level data, final when sent.
///
/// The parse keeps a reference to hand-outs 2–4 for its own later stages, so
/// those travel by `Arc`; [`assemble`] takes the data back without a copy once
/// no consumer still holds a clone.
///
/// # Before the verdict
///
/// Hand-outs 1–4 go out before the late checks run, and the parse can still
/// reject the level after them; [`HandOut::Final`] arrives only with an
/// accepting verdict. Until then a consumer may rely on everything a group's
/// own decode validates, and on:
///
/// - hand-out 3 agreeing with hand-out 2: every face's cell exists, every BVH
///   leaf's index range lies inside the index buffer and names an existing
///   cell, and every drawable cell has a drawing leaf;
/// - hand-out 4's cell draw index being validated against hand-out 3's BVH
///   and cells, its push forces and probe bindings against its environment
///   volumes, and its kinematic geometry being final (unaddressable sealed
///   portal ids and invalid carried-light links already dropped).
///
/// A consumer must not rely on, and must not index or size by without a
/// bounds check:
///
/// - a non-empty geometry having BVH leaves, or a zero-leaf BVH having no
///   drawable cell (the cross-section check);
/// - `fog_volumes.len()` being within `MAX_FOG_VOLUMES`;
/// - a hand-out 2 vertex's `lightmap_block` or `animated_block` naming a block
///   inside the lightmap or animated-block table: the lighting decode checks
///   them after hand-outs 2 and 3;
/// - a hand-out 3 cell's `portal_ref_start` / `portal_ref_count` range lying
///   inside the portal-ref array: the parse-end portal check validates it;
/// - a reflection probe's `roster_index` falling inside the animated-light
///   roster, or the roster tables being the identity;
/// - the SH cluster directory's members naming real cells, or any cluster
///   topology: hand-out 1 exposes neither (pin P19);
/// - the level loading at all: work built from hand-outs 1–4 must be
///   discardable on a rejection.
///
/// The portal graph, the fog cell masks, the lights and every lighting
/// section ride hand-out 5 because parse-end validation or a parse-end
/// rewrite makes them final (pin P25).
#[derive(Debug)]
pub enum HandOut {
    ShManifest(ShManifestHandOut),
    Geometry(Arc<GeometryHandOut>),
    Spatial(Arc<SpatialHandOut>),
    Entities(Arc<EntitiesHandOut>),
    Final(Box<FinalHandOut>),
}

impl HandOut {
    pub fn stage(&self) -> HandOutStage {
        match self {
            HandOut::ShManifest(_) => HandOutStage::ShManifest,
            HandOut::Geometry(_) => HandOutStage::Geometry,
            HandOut::Spatial(_) => HandOutStage::Spatial,
            HandOut::Entities(_) => HandOutStage::Entities,
            HandOut::Final(_) => HandOutStage::Final,
        }
    }
}

/// Hand-out 1: the SH stream manifest's metadata, sent before the parse reads
/// its first section. `None` when the level loads its SH resident (no id 50,
/// or SH streaming off).
#[derive(Debug, Clone)]
pub struct ShManifestHandOut {
    pub manifest: Option<ShManifestMetadata>,
}

/// The validated SH stream manifest without its cluster topology, which the
/// parse installs only after the cluster directory's semantic validation. The
/// whole manifest, topology installed, arrives in hand-out 5's
/// [`FinalLighting::sh_storage`].
#[derive(Debug, Clone)]
pub struct ShManifestMetadata(Arc<ShStreamManifest>);

impl ShManifestMetadata {
    pub fn cluster_count(&self) -> u32 {
        self.0.cluster_count()
    }

    pub fn payloads(&self) -> &ClusterShPayloadsSection {
        self.0.payloads()
    }

    pub fn base(&self) -> &ShStreamBaseMetadata {
        self.0.base()
    }

    pub fn sources(&self) -> &ShStreamSourceMetadata {
        self.0.sources()
    }

    /// The id-49 structure the manifest was read with: its clusters, ranges
    /// and resources. Before the verdict its members are not yet checked
    /// against the cell table (see [`HandOut`]); the cluster topology is not
    /// here at all.
    pub fn cluster_directory(&self) -> &ClusterDirectorySection {
        self.0.cluster_directory()
    }

    pub fn content_tag(&self) -> [u8; 32] {
        self.0.content_tag()
    }

    pub fn container(&self) -> &ContainerMeta {
        self.0.container()
    }

    pub fn diagnostic_path(&self) -> &std::path::Path {
        self.0.diagnostic_path()
    }

    /// Whether `manifest` is the one this metadata was handed out from.
    pub fn is_metadata_of(&self, manifest: &Arc<ShStreamManifest>) -> bool {
        Arc::ptr_eq(&self.0, manifest)
    }

    /// The whole manifest, for tests that check what a consumer of hand-out
    /// 1 could not yet see.
    #[cfg(test)]
    pub(crate) fn manifest_for_test(&self) -> &Arc<ShStreamManifest> {
        &self.0
    }
}

/// Hand-out 2: geometry, texture names and texture cache keys: what texture
/// prep needs first.
#[derive(Debug, Clone)]
pub struct GeometryHandOut {
    pub vertices: Vec<WorldVertex>,
    pub indices: Vec<u32>,
    pub face_meta: Vec<FaceMeta>,
    pub texture_names: Vec<String>,
    pub texture_cache_keys: TextureCacheKeysSection,
}

/// Hand-out 3: BVH, cells, visibility, residency set and locator. The portal
/// graph (portals, `has_portals`, `cell_portal_refs`) rides hand-out 5: its
/// adjacency against the cells is a parse-end check.
#[derive(Debug, Clone)]
pub struct SpatialHandOut {
    pub bvh: BvhTree,
    pub cells: Vec<CellData>,
    pub cell_visibility: Option<CellVisibility>,
    pub cell_residency_set: Option<CellResidencySetSection>,
    pub cell_locator_root: CellLocatorChild,
    pub cell_locator_nodes: Vec<CellLocatorNodeData>,
}

/// Hand-out 4: the entity group and the cell draw index. The fog cell masks
/// ride hand-out 5: the parse-end cross-section check validates them.
#[derive(Debug, Clone)]
pub struct EntitiesHandOut {
    pub data_script: Option<DataScriptSection>,
    pub map_entities: Vec<MapEntityRecord>,
    pub kinematic_geometry: KinematicGeometry,
    pub trigger_volumes: Vec<TriggerVolumeRecord>,
    pub teleporters: TeleportersSection,
    pub ghost_links: GhostLinksSection,
    pub environment_volumes: EnvironmentVolumesSection,
    pub environment_push_forces: Vec<f32>,
    pub reflection_probes: Option<ReflectionProbesSection>,
    pub fog_volumes: Vec<FogVolumeRecord>,
    pub fog_pixel_scale: u32,
    pub navmesh: Option<NavMeshSection>,
    pub cell_draw_index: Option<CellDrawIndex>,
}

/// Every lighting field after the parse-end lighting step: the shadowmask
/// drop and GPU payload split, the finalized lightmap storage, and the SH
/// storage with its cluster topology installed. Field meanings are
/// `LevelWorld`'s.
#[derive(Debug)]
pub struct FinalLighting {
    pub lights: Vec<MapLight>,
    pub light_influences: Vec<LightInfluence>,
    pub sh_volume: Option<OctahedralShVolumeSection>,
    pub sh_storage: ShStorage,
    pub lightmap: Option<LightmapBlockIndex>,
    pub lightmap_mode: LightmapMode,
    pub sdf_atlas: Option<SdfAtlasSection>,
    pub chunk_light_list: Option<ChunkLightListSection>,
    pub animated_light_chunks: Option<AnimatedLightChunksSection>,
    pub animated_light_weight_maps: Option<AnimatedLightWeightMapsSection>,
    pub delta_sh_volumes: Option<DeltaShVolumesSection>,
    pub direct_sh_volume: Option<DirectShVolumeSection>,
    pub direct_sh_delta_volumes: Option<DirectShDeltaVolumesSection>,
    pub animated_direct_sh_delta_volumes: Option<AnimatedDirectShDeltaVolumesSection>,
    pub billboard_direct_scatter_volume: Option<BillboardDirectScatterVolumeSection>,
    pub animated_billboard_direct_scatter_delta_volumes:
        Option<AnimatedBillboardDirectScatterDeltaVolumesSection>,
    pub entity_shadow_lights: Vec<u32>,
    pub shadowmask_atlas: Option<ShadowmaskBlockIndex>,
    pub vertex_spec_owners: Vec<FaceOwners>,
    pub gpu_lighting_payloads: GpuLightingPayloads,
    pub lightmap_storage: LightmapStorage,
    pub cluster_directory: Option<ClusterDirectorySection>,
}

/// Hand-out 5, sent only once the late verdict accepts the level: every
/// section a parse-end check validates or a parse-end step rewrites.
#[derive(Debug)]
pub struct FinalHandOut {
    pub portals: Vec<PortalData>,
    pub has_portals: bool,
    pub cell_portal_refs: Vec<u32>,
    pub fog_cell_masks: Option<Vec<u32>>,
    pub lighting: FinalLighting,
    pub prl_read_counters: Option<Arc<PrlReadCounters>>,
}

/// The parse runs on the engine load pool and its hand-outs are consumed on
/// other pool tasks.
const _: () = {
    const fn assert_send<T: Send>() {}
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send::<HandOut>();
    assert_send_sync::<GeometryHandOut>();
    assert_send_sync::<SpatialHandOut>();
    assert_send_sync::<EntitiesHandOut>();
    assert_send::<FinalHandOut>();
    assert_send::<ShManifestHandOut>();
};

/// Decodes the container in the monolithic order and hands out five ordered
/// groups (see [`HandOut`]), then the late verdict as the return value.
///
/// `cancel` is polled at every stage boundary; once seen, the parse sends
/// nothing more and returns [`StagedParseOutcome::Cancelled`]. A rejection
/// returns the error the monolithic parse returns for the same file, after
/// whichever hand-outs had already gone out.
pub(crate) fn load_prl_staged_from_container(
    container: PrlContainer,
    max_delta_section_binding_bytes: u64,
    max_scatter_section_bytes: u64,
    stream_manifest: Option<Arc<ShStreamManifest>>,
    lightmap_requested: LightmapStreamingMode,
    cancel: &(impl LoadCancel + ?Sized),
    sink: &mut dyn FnMut(HandOut),
) -> Result<StagedParseOutcome, PrlLoadError> {
    macro_rules! stage_boundary {
        () => {
            if cancel.is_cancelled() {
                return Ok(StagedParseOutcome::Cancelled);
            }
        };
    }
    let streaming = stream_manifest.as_deref();

    // Hand-out 1, before the first section read.
    stage_boundary!();
    sink(HandOut::ShManifest(ShManifestHandOut {
        manifest: stream_manifest.clone().map(ShManifestMetadata),
    }));

    stage_boundary!();
    let parsed_cluster_directory =
        cluster_directory::read_cluster_directory_structure(&container, streaming)?;
    stage_boundary!();
    let geometry = Arc::new(geometry::decode_geometry(&container)?);
    stage_boundary!();
    sink(HandOut::Geometry(geometry.clone()));

    stage_boundary!();
    let SpatialDecode {
        sections,
        validation_inputs,
        cell_portal_refs,
        portal_data,
    } = spatial::decode_spatial(&container, &geometry)?;
    let spatial = Arc::new(sections);
    stage_boundary!();
    sink(HandOut::Spatial(spatial.clone()));

    // Lighting decodes before the entities, as in the file order, but rides
    // hand-out 5: the parse-end split rewrites it.
    stage_boundary!();
    let lighting = lighting::decode_lighting(
        &container,
        streaming,
        lightmap_requested,
        &geometry,
        &spatial,
        portal_data.as_deref(),
    )?;
    stage_boundary!();
    let sh = lighting_sh::decode_sh_companions(
        &container,
        streaming,
        max_delta_section_binding_bytes,
        max_scatter_section_bytes,
        &lighting,
    )?;
    stage_boundary!();
    let mut entity_decode = entities::decode_entities(
        &container,
        &lighting.lights,
        portal_data.as_ref().map(Vec::len),
    )?;
    stage_boundary!();
    let cell_draw_index_data = container.read_section(SectionId::CellDrawIndex as u32)?;
    let fog_cell_masks = entity_decode.fog_cell_masks.take();
    // The cell draw index needs only the BVH and cells, so it decodes before
    // hand-out 4 rather than after the cross-section check as in the
    // monolithic order. A rejection keeps that order's precedence: a failing
    // cross-section check is the error the file reports.
    let cell_draw_index = match cell_draw_index::decode_cell_draw_index(
        cell_draw_index_data,
        &spatial.bvh,
        &spatial.cells,
    ) {
        Ok(index) => index,
        Err(error) => {
            validation::validate_cross_sections(
                &geometry,
                &spatial,
                &validation_inputs,
                &lighting.lights,
                entity_decode.fog_volumes.len(),
                fog_cell_masks,
            )?;
            return Err(error);
        }
    };
    let entities = Arc::new(entity_decode.into_hand_out(cell_draw_index));
    stage_boundary!();
    sink(HandOut::Entities(entities.clone()));

    // Late validation: one verdict, in the monolithic order.
    stage_boundary!();
    let fog_cell_masks = validation::validate_cross_sections(
        &geometry,
        &spatial,
        &validation_inputs,
        &lighting.lights,
        entities.fog_volumes.len(),
        fog_cell_masks,
    )?;
    stage_boundary!();
    let cluster_directory = cluster_directory::validate_cluster_directory(
        &container,
        streaming,
        parsed_cluster_directory,
        &validation_inputs,
        &lighting.sh_volume,
        &sh,
    )?;
    let sh = validation::finalize_sh_companions(sh);
    stage_boundary!();
    let (portals, has_portals) =
        validation::validate_portals(&spatial.cells, &cell_portal_refs, portal_data)?;
    log_loaded(&geometry, &spatial, portals.len());
    stage_boundary!();
    let lighting = finalize_lighting(
        &container,
        stream_manifest,
        lighting,
        sh,
        cluster_directory,
        &geometry,
        &spatial,
    )?;
    validate_reflection_probes(entities.reflection_probes.as_ref(), &lighting)?;

    stage_boundary!();
    sink(HandOut::Final(Box::new(FinalHandOut {
        portals,
        has_portals,
        cell_portal_refs,
        fog_cell_masks,
        lighting,
        prl_read_counters: container.read_counters().cloned(),
    })));
    Ok(StagedParseOutcome::Complete)
}

/// All five hand-outs of one accepted parse.
#[derive(Debug)]
pub struct CompleteHandOuts {
    pub sh_manifest: ShManifestHandOut,
    pub geometry: Arc<GeometryHandOut>,
    pub spatial: Arc<SpatialHandOut>,
    pub entities: Arc<EntitiesHandOut>,
    pub final_hand_out: Box<FinalHandOut>,
}

/// Gathers one parse's hand-outs in order, for a consumer that wants the
/// whole level at the end.
#[derive(Debug, Default)]
pub struct HandOutCollector {
    sh_manifest: Option<ShManifestHandOut>,
    geometry: Option<Arc<GeometryHandOut>>,
    spatial: Option<Arc<SpatialHandOut>>,
    entities: Option<Arc<EntitiesHandOut>>,
    final_hand_out: Option<Box<FinalHandOut>>,
    last_stage: Option<HandOutStage>,
}

impl HandOutCollector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Accept the next hand-out. Panics if it arrives out of order: a parse
    /// sends each stage once, in [`HandOutStage`] order.
    pub fn accept(&mut self, hand_out: HandOut) {
        let stage = hand_out.stage();
        assert!(
            self.last_stage
                .map_or(stage == HandOutStage::ShManifest, |last| {
                    stage as u8 == last as u8 + 1
                }),
            "hand-out {stage:?} arrived after {:?}",
            self.last_stage
        );
        self.last_stage = Some(stage);
        match hand_out {
            HandOut::ShManifest(hand_out) => self.sh_manifest = Some(hand_out),
            HandOut::Geometry(hand_out) => self.geometry = Some(hand_out),
            HandOut::Spatial(hand_out) => self.spatial = Some(hand_out),
            HandOut::Entities(hand_out) => self.entities = Some(hand_out),
            HandOut::Final(hand_out) => self.final_hand_out = Some(hand_out),
        }
    }

    /// The last stage accepted so far.
    pub fn last_stage(&self) -> Option<HandOutStage> {
        self.last_stage
    }

    /// All five hand-outs, once the final one has arrived.
    pub fn complete(self) -> Option<CompleteHandOuts> {
        Some(CompleteHandOuts {
            sh_manifest: self.sh_manifest?,
            geometry: self.geometry?,
            spatial: self.spatial?,
            entities: self.entities?,
            final_hand_out: self.final_hand_out?,
        })
    }
}

/// Takes a hand-out's data back from its `Arc`, copying only when a consumer
/// still holds a clone.
fn unshare<T: Clone>(shared: Arc<T>) -> T {
    Arc::try_unwrap(shared).unwrap_or_else(|shared| (*shared).clone())
}

/// Assembles the `LevelWorld` from one accepted parse's hand-outs: a pure
/// rearrangement, no decoding or validation.
pub fn assemble(hand_outs: CompleteHandOuts) -> LevelWorld {
    let CompleteHandOuts {
        sh_manifest,
        geometry,
        spatial,
        entities,
        final_hand_out,
    } = hand_outs;
    let GeometryHandOut {
        vertices,
        indices,
        face_meta,
        texture_names,
        texture_cache_keys,
    } = unshare(geometry);
    let SpatialHandOut {
        bvh,
        cells,
        cell_visibility,
        cell_residency_set,
        cell_locator_root,
        cell_locator_nodes,
    } = unshare(spatial);
    let EntitiesHandOut {
        data_script,
        map_entities,
        kinematic_geometry,
        trigger_volumes,
        teleporters,
        ghost_links,
        environment_volumes,
        environment_push_forces,
        reflection_probes,
        fog_volumes,
        fog_pixel_scale,
        navmesh,
        cell_draw_index,
    } = unshare(entities);
    let FinalHandOut {
        portals,
        has_portals,
        cell_portal_refs,
        fog_cell_masks,
        lighting,
        prl_read_counters,
    } = *final_hand_out;
    let FinalLighting {
        lights,
        light_influences,
        sh_volume,
        sh_storage,
        lightmap,
        lightmap_mode,
        sdf_atlas,
        chunk_light_list,
        animated_light_chunks,
        animated_light_weight_maps,
        delta_sh_volumes,
        direct_sh_volume,
        direct_sh_delta_volumes,
        animated_direct_sh_delta_volumes,
        billboard_direct_scatter_volume,
        animated_billboard_direct_scatter_delta_volumes,
        entity_shadow_lights,
        shadowmask_atlas,
        vertex_spec_owners,
        gpu_lighting_payloads,
        lightmap_storage,
        cluster_directory,
    } = lighting;
    debug_assert_eq!(
        sh_manifest.manifest.is_some(),
        sh_storage.is_streaming(),
        "hand-out 1 carries metadata exactly when the level streams SH"
    );
    debug_assert!(
        sh_manifest
            .manifest
            .as_ref()
            .zip(sh_storage.manifest())
            .is_none_or(|(metadata, manifest)| metadata.is_metadata_of(manifest)),
        "hand-out 1's metadata is hand-out 5's manifest"
    );

    LevelWorld {
        vertices,
        indices,
        face_meta,
        cells,
        cell_portal_refs,
        cell_locator_root,
        cell_locator_nodes,
        portals,
        has_portals,
        portal_planes: Default::default(),
        cell_visibility,
        texture_names,
        texture_cache_keys,
        bvh,
        lights,
        light_influences,
        sh_volume,
        sh_storage,
        lightmap,
        lightmap_mode,
        sdf_atlas,
        chunk_light_list,
        animated_light_chunks,
        animated_light_weight_maps,
        delta_sh_volumes,
        direct_sh_volume,
        direct_sh_delta_volumes,
        animated_direct_sh_delta_volumes,
        billboard_direct_scatter_volume,
        animated_billboard_direct_scatter_delta_volumes,
        entity_shadow_lights,
        shadowmask_atlas,
        vertex_spec_owners,
        gpu_lighting_payloads,
        lightmap_storage,
        data_script,
        map_entities,
        kinematic_geometry,
        trigger_volumes,
        teleporters,
        ghost_links,
        fog_volumes,
        fog_pixel_scale,
        environment_volumes,
        environment_push_forces,
        reflection_probes,
        fog_cell_masks,
        navmesh,
        cell_draw_index,
        cluster_directory,
        cell_residency_set,
        prl_read_counters,
    }
}
