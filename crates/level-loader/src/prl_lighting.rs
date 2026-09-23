// SH-family section bounds, decoding policy, and semantic validation.
// See: context/lib/build_pipeline.md §PRL Compilation

use postretro_level_format::animated_billboard_direct_scatter_delta_volumes::AnimatedBillboardDirectScatterDeltaVolumesSection;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::billboard_direct_scatter_volume::{
    BILLBOARD_DIRECT_SCATTER_RGBA_F16_COUNT, BILLBOARD_DIRECT_SCATTER_VALIDITY_ONE_F16,
    BillboardDirectScatterVolumeSection,
};
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::cluster_directory::ClusterDirectorySection;
use postretro_level_format::delta_sh_volumes::{AFFINITY_FACTOR, DeltaShVolumesSection};
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::lightmap::LightmapSection;
use postretro_level_format::sdf_atlas::SdfAtlasSection;
use postretro_level_format::sh_volume::{
    OctahedralShVolumeSection, validate_storage_levels_against_delta,
};
use postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection;
use postretro_level_format::{self as prl_format, SectionId};
use postretro_render_data::influence::LightInfluence;

use crate::prl::{LevelWorld, LightType, LightmapMode, MapLight, PrlLoadError, ShadowType};
#[cfg(test)]
use crate::prl_loader::MAX_DELTA_SECTION_BINDING_BYTES;
use crate::prl_loader::{section_validation, section_validation_from_error};
use crate::sh_stream::{ShStreamBaseMetadata, ShStreamSparseMetadata};

/// Lighting-related data assembled by the PRL loader before it is transferred
/// into the stable `LevelWorld` fields.
///
/// Keeping the transfer record here makes the legacy whole-section ownership
/// explicit without changing existing callers. A later streaming storage mode
/// can replace this boundary while the current public fields continue to
/// preserve their legacy contract.
pub(crate) struct LoadedLighting {
    pub(crate) lights: Vec<MapLight>,
    pub(crate) light_influences: Vec<LightInfluence>,
    pub(crate) sh_volume: Option<OctahedralShVolumeSection>,
    pub(crate) lightmap: Option<LightmapSection>,
    pub(crate) lightmap_mode: LightmapMode,
    pub(crate) sdf_atlas: Option<SdfAtlasSection>,
    pub(crate) chunk_light_list: Option<ChunkLightListSection>,
    pub(crate) animated_light_chunks: Option<AnimatedLightChunksSection>,
    pub(crate) animated_light_weight_maps: Option<AnimatedLightWeightMapsSection>,
    pub(crate) delta_sh_volumes: Option<DeltaShVolumesSection>,
    pub(crate) direct_sh_volume: Option<DirectShVolumeSection>,
    pub(crate) direct_sh_delta_volumes: Option<DirectShDeltaVolumesSection>,
    pub(crate) animated_direct_sh_delta_volumes: Option<AnimatedDirectShDeltaVolumesSection>,
    pub(crate) billboard_direct_scatter_volume: Option<BillboardDirectScatterVolumeSection>,
    pub(crate) animated_billboard_direct_scatter_delta_volumes:
        Option<AnimatedBillboardDirectScatterDeltaVolumesSection>,
    pub(crate) entity_shadow_lights: Vec<u32>,
    pub(crate) shadowmask_atlas: Option<ShadowmaskAtlasSection>,
    pub(crate) cluster_directory: Option<ClusterDirectorySection>,
}

impl Default for LoadedLighting {
    fn default() -> Self {
        Self {
            lights: Vec::new(),
            light_influences: Vec::new(),
            sh_volume: None,
            lightmap: None,
            lightmap_mode: LightmapMode::Shadowed,
            sdf_atlas: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
            delta_sh_volumes: None,
            direct_sh_volume: None,
            direct_sh_delta_volumes: None,
            animated_direct_sh_delta_volumes: None,
            billboard_direct_scatter_volume: None,
            animated_billboard_direct_scatter_delta_volumes: None,
            entity_shadow_lights: Vec::new(),
            shadowmask_atlas: None,
            cluster_directory: None,
        }
    }
}

/// Borrowed legacy lighting view. It is an additive access seam: existing
/// direct `LevelWorld` fields remain available until streaming owns their
/// storage, while new code can depend on one coherent lighting boundary.
#[derive(Clone, Copy)]
pub struct LevelWorldLighting<'a> {
    pub lights: &'a [MapLight],
    pub light_influences: &'a [LightInfluence],
    pub sh_volume: Option<&'a OctahedralShVolumeSection>,
    pub lightmap: Option<&'a LightmapSection>,
    pub lightmap_mode: LightmapMode,
    pub sdf_atlas: Option<&'a SdfAtlasSection>,
    pub chunk_light_list: Option<&'a ChunkLightListSection>,
    pub animated_light_chunks: Option<&'a AnimatedLightChunksSection>,
    pub animated_light_weight_maps: Option<&'a AnimatedLightWeightMapsSection>,
    pub delta_sh_volumes: Option<&'a DeltaShVolumesSection>,
    pub direct_sh_volume: Option<&'a DirectShVolumeSection>,
    pub direct_sh_delta_volumes: Option<&'a DirectShDeltaVolumesSection>,
    pub animated_direct_sh_delta_volumes: Option<&'a AnimatedDirectShDeltaVolumesSection>,
    pub billboard_direct_scatter_volume: Option<&'a BillboardDirectScatterVolumeSection>,
    pub animated_billboard_direct_scatter_delta_volumes:
        Option<&'a AnimatedBillboardDirectScatterDeltaVolumesSection>,
    pub entity_shadow_lights: &'a [u32],
    pub shadowmask_atlas: Option<&'a ShadowmaskAtlasSection>,
    pub cluster_directory: Option<&'a ClusterDirectorySection>,
    pub sh_storage: &'a crate::sh_stream::ShStorage,
}

impl LevelWorld {
    /// Explicit SH ownership. Streaming callers retain an immutable manifest;
    /// legacy callers retain the original whole section bodies.
    pub fn sh_storage(&self) -> &crate::sh_stream::ShStorage {
        &self.sh_storage
    }

    pub fn sh_stream_manifest(
        &self,
    ) -> Option<&std::sync::Arc<crate::sh_stream::ShStreamManifest>> {
        self.sh_storage.manifest()
    }

    pub fn sh_volume(&self) -> Option<&OctahedralShVolumeSection> {
        (!self.sh_storage.is_streaming())
            .then_some(self.sh_volume.as_ref())
            .flatten()
    }

    pub fn delta_sh_volumes(&self) -> Option<&DeltaShVolumesSection> {
        (!self.sh_storage.is_streaming())
            .then_some(self.delta_sh_volumes.as_ref())
            .flatten()
    }

    pub fn direct_sh_volume(&self) -> Option<&DirectShVolumeSection> {
        (!self.sh_storage.is_streaming())
            .then_some(self.direct_sh_volume.as_ref())
            .flatten()
    }

    pub fn direct_sh_delta_volumes(&self) -> Option<&DirectShDeltaVolumesSection> {
        (!self.sh_storage.is_streaming())
            .then_some(self.direct_sh_delta_volumes.as_ref())
            .flatten()
    }

    pub fn animated_direct_sh_delta_volumes(&self) -> Option<&AnimatedDirectShDeltaVolumesSection> {
        (!self.sh_storage.is_streaming())
            .then_some(self.animated_direct_sh_delta_volumes.as_ref())
            .flatten()
    }

    pub fn billboard_direct_scatter_volume(&self) -> Option<&BillboardDirectScatterVolumeSection> {
        self.billboard_direct_scatter_volume.as_ref()
    }

    pub fn animated_billboard_direct_scatter_delta_volumes(
        &self,
    ) -> Option<&AnimatedBillboardDirectScatterDeltaVolumesSection> {
        self.animated_billboard_direct_scatter_delta_volumes
            .as_ref()
    }

    pub fn entity_shadow_lights(&self) -> &[u32] {
        &self.entity_shadow_lights
    }

    pub fn shadowmask_atlas(&self) -> Option<&ShadowmaskAtlasSection> {
        self.shadowmask_atlas.as_ref()
    }

    pub fn cluster_directory(&self) -> Option<&ClusterDirectorySection> {
        self.cluster_directory.as_ref()
    }

    /// Global animation descriptors are metadata, not a streamed atlas body.
    pub fn animation_descriptors(
        &self,
    ) -> &[postretro_level_format::sh_volume::AnimationDescriptor] {
        match self.sh_stream_manifest() {
            Some(manifest) => &manifest.base().animation_descriptors,
            None => self
                .sh_volume
                .as_ref()
                .map_or(&[], |section| &section.animation_descriptors),
        }
    }

    /// Id-45's animation descriptor mapping remains in the streaming metadata
    /// projection even though its CSR/tile payload is chunk-streamed.
    pub fn animated_direct_descriptor_indices(&self) -> &[u32] {
        match self.sh_stream_manifest() {
            Some(manifest) => manifest
                .sources()
                .animated_direct_delta
                .as_ref()
                .map_or(&[], |metadata| &metadata.animation_descriptor_indices),
            None => self
                .animated_direct_sh_delta_volumes
                .as_ref()
                .map_or(&[], |section| &section.animation_descriptor_indices),
        }
    }

    /// Id-45's CSR light roster remains in the streaming metadata projection.
    /// Capture promotion uses roster positions, not the dense tile payload.
    pub fn animated_direct_affinity_lights(&self) -> &[u32] {
        match self.sh_stream_manifest() {
            Some(manifest) => manifest
                .sources()
                .animated_direct_delta
                .as_ref()
                .map_or(&[], |metadata| &metadata.affinity_lights),
            None => self
                .animated_direct_sh_delta_volumes
                .as_ref()
                .map_or(&[], |section| &section.affinity_lights),
        }
    }

    /// Returns every lighting/SH input through the future storage seam.
    ///
    /// Legacy callers can keep reading the existing fields during this
    /// mechanical split; the streaming migration will move them to this view.
    pub fn lighting(&self) -> LevelWorldLighting<'_> {
        LevelWorldLighting {
            lights: &self.lights,
            light_influences: &self.light_influences,
            sh_volume: self.sh_volume(),
            lightmap: self.lightmap.as_ref(),
            lightmap_mode: self.lightmap_mode,
            sdf_atlas: self.sdf_atlas.as_ref(),
            chunk_light_list: self.chunk_light_list.as_ref(),
            animated_light_chunks: self.animated_light_chunks.as_ref(),
            animated_light_weight_maps: self.animated_light_weight_maps.as_ref(),
            delta_sh_volumes: self.delta_sh_volumes(),
            direct_sh_volume: self.direct_sh_volume(),
            direct_sh_delta_volumes: self.direct_sh_delta_volumes(),
            animated_direct_sh_delta_volumes: self.animated_direct_sh_delta_volumes(),
            billboard_direct_scatter_volume: self.billboard_direct_scatter_volume(),
            animated_billboard_direct_scatter_delta_volumes: self
                .animated_billboard_direct_scatter_delta_volumes(),
            entity_shadow_lights: self.entity_shadow_lights(),
            shadowmask_atlas: self.shadowmask_atlas(),
            cluster_directory: self.cluster_directory(),
            sh_storage: self.sh_storage(),
        }
    }
}

/// A delta section's raw bytes after applying the storage-binding floor.
///
/// `OverBindingFloor` deliberately remains distinct from `Absent`: callers
/// use the former to log a precise degradation reason while preserving the
/// existing absence semantics for optional PRL sections.
pub(crate) enum BoundedDeltaSectionData<'a> {
    Absent,
    OverBindingFloor,
    Data(&'a [u8]),
}

/// Borrow an optional delta section after validating its container bounds, then
/// reject raw payloads that cannot fit a single runtime storage-buffer binding.
/// The borrow remains allocation-free, so no decoder table is allocated before
/// either structural validation or the binding-floor check.
#[cfg(test)]
pub(crate) fn read_bounded_delta_section_data<'a>(
    file_data: &'a [u8],
    meta: &prl_format::ContainerMeta,
    section_id: SectionId,
    section_name: &str,
) -> Result<BoundedDeltaSectionData<'a>, PrlLoadError> {
    read_bounded_delta_section_data_with_limit(
        file_data,
        meta,
        section_id,
        section_name,
        MAX_DELTA_SECTION_BINDING_BYTES,
    )
}

pub(crate) fn read_bounded_delta_section_data_with_limit<'a>(
    file_data: &'a [u8],
    meta: &prl_format::ContainerMeta,
    section_id: SectionId,
    section_name: &str,
    max_binding_bytes: u64,
) -> Result<BoundedDeltaSectionData<'a>, PrlLoadError> {
    let Some(data) = prl_format::section_data_from_bytes(file_data, meta, section_id as u32)?
    else {
        return Ok(BoundedDeltaSectionData::Absent);
    };
    if data.len() as u64 > max_binding_bytes {
        log::warn!(
            "[PRL] {section_name} raw payload is {} B, above the {} B storage-binding floor; disabling before decode",
            data.len(),
            max_binding_bytes,
        );
        return Ok(BoundedDeltaSectionData::OverBindingFloor);
    }
    Ok(BoundedDeltaSectionData::Data(data))
}

/// Read an optional scatter section without allowing a bad optional entry to
/// reject the map. Unlike core sections, billboard scatter selects an additive
/// optimization; a malformed container range must choose the legacy path.
pub(crate) fn read_soft_optional_scatter_section_data<'a>(
    file_data: &'a [u8],
    meta: &prl_format::ContainerMeta,
    section_id: SectionId,
    section_name: &str,
) -> Option<&'a [u8]> {
    match prl_format::section_data_from_bytes(file_data, meta, section_id as u32) {
        Ok(data) => data,
        Err(error) => {
            log::warn!(
                "[PRL] {section_name} has an invalid optional container entry; disabling billboard direct scatter: {error}"
            );
            None
        }
    }
}

pub(crate) enum BoundedScatterSectionData<'a> {
    Absent,
    OverPackCap,
    Data(&'a [u8]),
}

/// Apply section 48's encoded-size policy after validating container bounds
/// but before its dense decoder allocates descriptor, CSR, or delta vectors.
pub(crate) fn read_bounded_scatter_section_data_with_limit<'a>(
    file_data: &'a [u8],
    meta: &prl_format::ContainerMeta,
    max_encoded_bytes: u64,
) -> BoundedScatterSectionData<'a> {
    let Some(data) = read_soft_optional_scatter_section_data(
        file_data,
        meta,
        SectionId::AnimatedBillboardDirectScatterDeltaVolumes,
        "AnimatedBillboardDirectScatterDeltaVolumes",
    ) else {
        return BoundedScatterSectionData::Absent;
    };
    if data.len() as u64 > max_encoded_bytes {
        log::warn!(
            "[PRL] AnimatedBillboardDirectScatterDeltaVolumes is {} B, above the {} B encoded section cap; disabling billboard direct scatter before decode",
            data.len(),
            max_encoded_bytes,
        );
        return BoundedScatterSectionData::OverPackCap;
    }
    BoundedScatterSectionData::Data(data)
}

pub(crate) fn expected_affinity_dims(base_dims: [u32; 3], factor: u8) -> [u32; 3] {
    let f = factor as u32;
    [
        base_dims[0].div_ceil(f),
        base_dims[1].div_ceil(f),
        base_dims[2].div_ceil(f),
    ]
}

/// Validate a loaded DeltaShVolumes section against the engine's invariants.
/// `base` is the base OctahedralShVolume (id 34), or `None` if that section was
/// absent. Pure so the reject paths are unit-testable.
///
/// Rejects (clear typed error, no panic):
/// - `affinity_factor` != the engine's compiled-in `AFFINITY_FACTOR`,
/// - base ShVolume absent while a delta section is present,
/// - `affinity_dims` != `ceil(base_dims / affinity_factor)`,
/// - delta tile geometry differs from the base atlas tile geometry.
pub(crate) fn validate_delta_sh(
    section: &DeltaShVolumesSection,
    base: Option<&OctahedralShVolumeSection>,
) -> Result<(), PrlLoadError> {
    // affinity_factor is locked to the compose pass `@workgroup_size(4,4,4)`.
    if section.affinity_factor != AFFINITY_FACTOR {
        return Err(PrlLoadError::DeltaShAffinityFactorMismatch {
            found: section.affinity_factor,
            expected: AFFINITY_FACTOR,
        });
    }

    // The base grid's dims derive the expected affinity dims; the compose pass
    // cannot run without it.
    let Some(base) = base else {
        return Err(PrlLoadError::DeltaShMissingBaseVolume);
    };
    let base_dims = base.grid_dimensions;

    let expected = expected_affinity_dims(base_dims, AFFINITY_FACTOR);
    if section.affinity_dims != expected {
        return Err(PrlLoadError::DeltaShAffinityDimsMismatch {
            found: section.affinity_dims,
            base_dims,
            factor: AFFINITY_FACTOR as u32,
            expected,
        });
    }

    if section.tile_dimension != base.tile_dimension || section.tile_border != base.tile_border {
        return Err(PrlLoadError::DeltaShTileGeometryMismatch {
            found_dimension: section.tile_dimension,
            found_border: section.tile_border,
            base_dimension: base.tile_dimension,
            base_border: base.tile_border,
        });
    }

    let affinity_cell_count = section.affinity_cell_count();
    if section.valid_probe_masks.len() != affinity_cell_count {
        return Err(section_validation(
            "DeltaShVolumes",
            format!(
                "valid_probe_masks has length {}, expected {affinity_cell_count}",
                section.valid_probe_masks.len()
            ),
        ));
    }
    if base.probes.len() != base.total_probes() {
        return Err(section_validation(
            "DeltaShVolumes",
            format!(
                "OctahedralShVolume (id 34) has {} probe metadata records for {} grid probes",
                base.probes.len(),
                base.total_probes(),
            ),
        ));
    }
    for (cell, &stored_mask) in section.valid_probe_masks.iter().enumerate() {
        let expected_mask = valid_probe_mask_for_affinity_cell(base, section.affinity_dims, cell);
        if stored_mask != expected_mask {
            return Err(section_validation(
                "DeltaShVolumes",
                format!(
                    "valid_probe_masks[{cell}] {stored_mask:#018x} disagrees with OctahedralShVolume (id 34) validity {expected_mask:#018x}; recompile the .prl with the current `prl-build`"
                ),
            ));
        }
    }

    Ok(())
}

/// Validate the animated direct-SH delta section against the base SH layout and
/// its own CSR contract. Unlike promotion deltas (ID 41), this section has no
/// external selected-light namespace to cross-check: its light indices are
/// bounded by its own descriptor-index table.
pub(crate) fn validate_animated_direct_sh_delta(
    section: &AnimatedDirectShDeltaVolumesSection,
    base: Option<&OctahedralShVolumeSection>,
) -> Result<(), PrlLoadError> {
    const SECTION: &str = "AnimatedDirectShDeltaVolumes";

    if section.affinity_factor != AFFINITY_FACTOR {
        return Err(section_validation(
            SECTION,
            format!(
                "affinity_factor {} does not match runtime factor {AFFINITY_FACTOR}",
                section.affinity_factor
            ),
        ));
    }

    let Some(base) = base else {
        return Err(section_validation(
            SECTION,
            "section requires an OctahedralShVolume base grid",
        ));
    };
    let expected_dims = expected_affinity_dims(base.grid_dimensions, AFFINITY_FACTOR);
    if section.affinity_dims != expected_dims {
        return Err(section_validation(
            SECTION,
            format!(
                "affinity_dims {:?} do not match ceil(base grid {:?} / {AFFINITY_FACTOR}) = {expected_dims:?}",
                section.affinity_dims, base.grid_dimensions
            ),
        ));
    }
    if section.tile_dimension != base.tile_dimension || section.tile_border != base.tile_border {
        return Err(section_validation(
            SECTION,
            format!(
                "tile geometry {} + border {} does not match OctahedralShVolume {} + border {}",
                section.tile_dimension, section.tile_border, base.tile_dimension, base.tile_border,
            ),
        ));
    }

    let affinity_cell_count = section.affinity_cell_count();
    let expected_offsets_len = affinity_cell_count
        .checked_add(1)
        .ok_or_else(|| section_validation(SECTION, "affinity offset count overflows usize"))?;
    if section.affinity_offsets.len() != expected_offsets_len {
        return Err(section_validation(
            SECTION,
            format!(
                "affinity_offsets has length {}, expected {expected_offsets_len}",
                section.affinity_offsets.len()
            ),
        ));
    }
    if section.valid_probe_masks.len() != affinity_cell_count {
        return Err(section_validation(
            SECTION,
            format!(
                "valid_probe_masks has length {}, expected {affinity_cell_count}",
                section.valid_probe_masks.len()
            ),
        ));
    }
    if base.probes.len() != base.total_probes() {
        return Err(section_validation(
            SECTION,
            format!(
                "OctahedralShVolume (id 34) has {} probe metadata records for {} grid probes",
                base.probes.len(),
                base.total_probes(),
            ),
        ));
    }
    for (cell, &found) in section.valid_probe_masks.iter().enumerate() {
        let expected = valid_probe_mask_for_affinity_cell(base, section.affinity_dims, cell);
        if found != expected {
            return Err(PrlLoadError::AnimatedDirectShDeltaValidityMismatch {
                cell,
                found,
                expected,
            });
        }
    }
    if section.affinity_offsets.first().copied() != Some(0) {
        return Err(section_validation(
            SECTION,
            "affinity_offsets[0] must be 0 for CSR data",
        ));
    }
    for (index, offsets) in section.affinity_offsets.windows(2).enumerate() {
        if offsets[0] > offsets[1] {
            return Err(section_validation(
                SECTION,
                format!(
                    "affinity_offsets[{index}] ({}) > affinity_offsets[{}] ({}): offsets must be non-decreasing",
                    offsets[0],
                    index + 1,
                    offsets[1],
                ),
            ));
        }
    }
    let trailing_total = section
        .affinity_offsets
        .last()
        .copied()
        .expect("expected_offsets_len is always at least one");
    let light_count = u32::try_from(section.affinity_lights.len()).map_err(|_| {
        section_validation(SECTION, "affinity_lights length exceeds the u32 wire range")
    })?;
    if trailing_total != light_count {
        return Err(section_validation(
            SECTION,
            format!(
                "affinity_offsets trailing total {trailing_total} does not match affinity_lights length {light_count}"
            ),
        ));
    }
    for (entry, &light_index) in section.affinity_lights.iter().enumerate() {
        if light_index as usize >= section.animation_descriptor_indices.len() {
            return Err(section_validation(
                SECTION,
                format!(
                    "affinity_lights[{entry}] AnimatedBakedLights index {light_index} is out of range for {} descriptor index entries",
                    section.animation_descriptor_indices.len()
                ),
            ));
        }
    }
    let expected_subblock_len = section
        .expected_delta_subblock_f16_count()
        .ok_or_else(|| section_validation(SECTION, "delta_subblocks length overflows usize"))?;
    if section.delta_subblocks.len() != expected_subblock_len {
        return Err(section_validation(
            SECTION,
            format!(
                "delta_subblocks has length {}, expected {expected_subblock_len}",
                section.delta_subblocks.len()
            ),
        ));
    }

    Ok(())
}

/// Validate id 47 against the base octahedral SH grid. The scatter term is
/// intentionally normal-free, but positions and binary validity are shared so
/// the billboard sampler can use id-34's x-fastest probe addressing.
pub(crate) fn validate_billboard_direct_scatter_volume(
    section: &BillboardDirectScatterVolumeSection,
    base: Option<&OctahedralShVolumeSection>,
) -> Result<(), PrlLoadError> {
    const SECTION: &str = "BillboardDirectScatterVolume";
    let Some(base) = base else {
        return Err(section_validation(
            SECTION,
            "section requires an OctahedralShVolume base grid",
        ));
    };
    if section.grid_origin != base.grid_origin {
        return Err(section_validation(
            SECTION,
            format!(
                "grid_origin {:?} does not match OctahedralShVolume grid_origin {:?}",
                section.grid_origin, base.grid_origin
            ),
        ));
    }
    if section.cell_size != base.cell_size {
        return Err(section_validation(
            SECTION,
            format!(
                "cell_size {:?} does not match OctahedralShVolume cell_size {:?}",
                section.cell_size, base.cell_size
            ),
        ));
    }
    if section.grid_dimensions != base.grid_dimensions {
        return Err(section_validation(
            SECTION,
            format!(
                "grid_dimensions {:?} does not match OctahedralShVolume grid_dimensions {:?}",
                section.grid_dimensions, base.grid_dimensions
            ),
        ));
    }
    let expected_probe_count = base.total_probes();
    if base.probes.len() != expected_probe_count {
        return Err(section_validation(
            SECTION,
            format!(
                "OctahedralShVolume has {} probe metadata records for {expected_probe_count} grid probes",
                base.probes.len()
            ),
        ));
    }
    let expected_scatter_f16_count = expected_probe_count
        .checked_mul(BILLBOARD_DIRECT_SCATTER_RGBA_F16_COUNT)
        .ok_or_else(|| section_validation(SECTION, "scatter payload length overflows usize"))?;
    if section.scatter_rgba.len() != expected_scatter_f16_count {
        return Err(section_validation(
            SECTION,
            format!(
                "scatter_rgba length {}, expected {expected_scatter_f16_count}",
                section.scatter_rgba.len()
            ),
        ));
    }
    for (probe, expected_validity) in base.probes.iter().enumerate() {
        let found = section.scatter_rgba[probe * BILLBOARD_DIRECT_SCATTER_RGBA_F16_COUNT + 3];
        let expected = if expected_validity.validity == 0 {
            0
        } else {
            BILLBOARD_DIRECT_SCATTER_VALIDITY_ONE_F16
        };
        if found != expected {
            return Err(section_validation(
                SECTION,
                format!(
                    "scatter_rgba probe {probe} alpha {found:#06x} does not mirror OctahedralShVolume validity as {expected:#06x}"
                ),
            ));
        }
    }
    Ok(())
}

/// Streaming-compatible form of [`validate_billboard_direct_scatter_volume`].
/// It proves the whole-resident id-47 companion still shares id-34's global
/// probe addressing without materializing the id-34 compact atlas.
pub(crate) fn validate_billboard_direct_scatter_against_metadata(
    section: &BillboardDirectScatterVolumeSection,
    base: &ShStreamBaseMetadata,
) -> Result<(), PrlLoadError> {
    const SECTION: &str = "BillboardDirectScatterVolume";
    if section.grid_origin != base.grid_origin
        || section.cell_size != base.cell_size
        || section.grid_dimensions != base.grid_dimensions
    {
        return Err(section_validation(
            SECTION,
            "grid metadata does not match streamed OctahedralShVolume metadata",
        ));
    }
    let expected_probe_count = base.probes.len();
    let expected_scatter_f16_count = expected_probe_count
        .checked_mul(BILLBOARD_DIRECT_SCATTER_RGBA_F16_COUNT)
        .ok_or_else(|| section_validation(SECTION, "scatter payload length overflows usize"))?;
    if section.scatter_rgba.len() != expected_scatter_f16_count {
        return Err(section_validation(
            SECTION,
            format!(
                "scatter_rgba length {}, expected {expected_scatter_f16_count}",
                section.scatter_rgba.len(),
            ),
        ));
    }
    for (probe, expected_validity) in base.probes.iter().enumerate() {
        let found = section.scatter_rgba[probe * BILLBOARD_DIRECT_SCATTER_RGBA_F16_COUNT + 3];
        let expected = if expected_validity.validity == 0 {
            0
        } else {
            BILLBOARD_DIRECT_SCATTER_VALIDITY_ONE_F16
        };
        if found != expected {
            return Err(section_validation(
                SECTION,
                format!(
                    "scatter_rgba probe {probe} alpha {found:#06x} does not mirror streamed id-34 validity as {expected:#06x}"
                ),
            ));
        }
    }
    Ok(())
}

/// Validate id 48's duplicated animated descriptor map and CSR layout against
/// its authoritative id-45 sibling. Id 45 remains the owner of validity and
/// coarsening; id 48 has only dense 4×4×4 delta values.
pub(crate) fn validate_animated_billboard_direct_scatter_delta_volumes(
    section: &AnimatedBillboardDirectScatterDeltaVolumesSection,
    animated_direct: &AnimatedDirectShDeltaVolumesSection,
) -> Result<(), PrlLoadError> {
    const SECTION: &str = "AnimatedBillboardDirectScatterDeltaVolumes";
    if section.animation_descriptor_indices != animated_direct.animation_descriptor_indices {
        return Err(section_validation(
            SECTION,
            "animation_descriptor_indices do not match AnimatedDirectShDeltaVolumes (id 45)",
        ));
    }
    if section.affinity_factor != animated_direct.affinity_factor {
        return Err(section_validation(
            SECTION,
            format!(
                "affinity_factor {} does not match AnimatedDirectShDeltaVolumes factor {}",
                section.affinity_factor, animated_direct.affinity_factor
            ),
        ));
    }
    if section.affinity_dims != animated_direct.affinity_dims {
        return Err(section_validation(
            SECTION,
            format!(
                "affinity_dims {:?} do not match AnimatedDirectShDeltaVolumes dimensions {:?}",
                section.affinity_dims, animated_direct.affinity_dims
            ),
        ));
    }
    if section.affinity_offsets != animated_direct.affinity_offsets {
        return Err(section_validation(
            SECTION,
            "affinity_offsets do not match AnimatedDirectShDeltaVolumes (id 45)",
        ));
    }
    if section.affinity_lights != animated_direct.affinity_lights {
        return Err(section_validation(
            SECTION,
            "affinity_lights do not match AnimatedDirectShDeltaVolumes (id 45)",
        ));
    }
    let expected_delta_f16_count = section
        .expected_delta_f16_count()
        .ok_or_else(|| section_validation(SECTION, "dense delta payload length overflows usize"))?;
    if section.delta_rgba.len() != expected_delta_f16_count {
        return Err(section_validation(
            SECTION,
            format!(
                "delta_rgba length {}, expected {expected_delta_f16_count} (= {} CSR entries × 64 RGBA16F values)",
                section.delta_rgba.len(),
                section.affinity_lights.len(),
            ),
        ));
    }
    Ok(())
}

/// Projection-compatible id-45/id-48 validation. Id 48 remains whole
/// resident, while id 45's descriptor/CSR metadata is retained in id-50's
/// validated streaming source projection.
pub(crate) fn validate_animated_billboard_direct_scatter_against_metadata(
    section: &AnimatedBillboardDirectScatterDeltaVolumesSection,
    animated_direct: &ShStreamSparseMetadata,
) -> Result<(), PrlLoadError> {
    const SECTION: &str = "AnimatedBillboardDirectScatterDeltaVolumes";
    if section.affinity_factor != AFFINITY_FACTOR {
        return Err(section_validation(
            SECTION,
            format!(
                "affinity_factor {} does not match streamed AnimatedDirectShDeltaVolumes factor {AFFINITY_FACTOR}",
                section.affinity_factor,
            ),
        ));
    }
    if section.animation_descriptor_indices != animated_direct.animation_descriptor_indices {
        return Err(section_validation(
            SECTION,
            "animation_descriptor_indices do not match streamed AnimatedDirectShDeltaVolumes metadata",
        ));
    }
    if section.affinity_dims != animated_direct.affinity_dims {
        return Err(section_validation(
            SECTION,
            "affinity_dims do not match streamed AnimatedDirectShDeltaVolumes metadata",
        ));
    }
    if section.affinity_offsets != animated_direct.affinity_offsets {
        return Err(section_validation(
            SECTION,
            "affinity_offsets do not match streamed AnimatedDirectShDeltaVolumes metadata",
        ));
    }
    if section.affinity_lights != animated_direct.affinity_lights {
        return Err(section_validation(
            SECTION,
            "affinity_lights do not match streamed AnimatedDirectShDeltaVolumes metadata",
        ));
    }
    let expected_delta_f16_count = section
        .expected_delta_f16_count()
        .ok_or_else(|| section_validation(SECTION, "dense delta payload length overflows usize"))?;
    if section.delta_rgba.len() != expected_delta_f16_count {
        return Err(section_validation(
            SECTION,
            format!(
                "delta_rgba length {}, expected {expected_delta_f16_count}",
                section.delta_rgba.len(),
            ),
        ));
    }
    Ok(())
}

pub(crate) fn validate_direct_sh_layout(
    section: &DirectShVolumeSection,
    base: &OctahedralShVolumeSection,
) -> Result<(), PrlLoadError> {
    if section.grid_origin != base.grid_origin {
        return Err(section_validation(
            "DirectShVolume",
            format!(
                "grid_origin {:?} does not match OctahedralShVolume grid_origin {:?}",
                section.grid_origin, base.grid_origin
            ),
        ));
    }
    if section.cell_size != base.cell_size {
        return Err(section_validation(
            "DirectShVolume",
            format!(
                "cell_size {:?} does not match OctahedralShVolume cell_size {:?}",
                section.cell_size, base.cell_size
            ),
        ));
    }
    if section.grid_dimensions != base.grid_dimensions {
        return Err(section_validation(
            "DirectShVolume",
            format!(
                "grid_dimensions {:?} does not match OctahedralShVolume grid_dimensions {:?}",
                section.grid_dimensions, base.grid_dimensions
            ),
        ));
    }
    if section.tile_dimension != base.tile_dimension {
        return Err(section_validation(
            "DirectShVolume",
            format!(
                "tile_dimension {} does not match OctahedralShVolume tile_dimension {}",
                section.tile_dimension, base.tile_dimension
            ),
        ));
    }
    if section.tile_border != base.tile_border {
        return Err(section_validation(
            "DirectShVolume",
            format!(
                "tile_border {} does not match OctahedralShVolume tile_border {}",
                section.tile_border, base.tile_border
            ),
        ));
    }
    if section.atlas_dimensions != base.atlas_dimensions {
        return Err(section_validation(
            "DirectShVolume",
            format!(
                "atlas_dimensions {:?} does not match OctahedralShVolume atlas_dimensions {:?}",
                section.atlas_dimensions, base.atlas_dimensions
            ),
        ));
    }
    if section.atlas_tiles_per_row != base.atlas_tiles_per_row {
        return Err(section_validation(
            "DirectShVolume",
            format!(
                "atlas_tiles_per_row {} does not match OctahedralShVolume atlas_tiles_per_row {}",
                section.atlas_tiles_per_row, base.atlas_tiles_per_row
            ),
        ));
    }
    if section.layer_count != base.layer_count {
        return Err(section_validation(
            "DirectShVolume",
            format!(
                "layer_count {} does not match OctahedralShVolume layer_count {}",
                section.layer_count, base.layer_count
            ),
        ));
    }
    if section.tiles_per_layer != base.tiles_per_layer {
        return Err(section_validation(
            "DirectShVolume",
            format!(
                "tiles_per_layer {} does not match OctahedralShVolume tiles_per_layer {}",
                section.tiles_per_layer, base.tiles_per_layer
            ),
        ));
    }
    if section.irradiance_format != base.irradiance_format {
        return Err(section_validation(
            "DirectShVolume",
            format!(
                "irradiance_format {} does not match OctahedralShVolume irradiance_format {}",
                section.irradiance_format, base.irradiance_format
            ),
        ));
    }

    Ok(())
}

/// Enforce I2 for every present, grid-matched delta section before renderer
/// resources are created. The metadata-derived base storage may be finer than
/// a delta entry, never coarser; accepting a violation would make a later
/// stored-slot compose reconstruct from data that was never emitted.
pub(crate) fn validate_storage_ceiling_for_delta(
    section_name: &'static str,
    base: &OctahedralShVolumeSection,
    cell_levels: &[u8],
    affinity_offsets: &[u32],
) -> Result<(), PrlLoadError> {
    validate_storage_levels_against_delta(
        base.grid_dimensions,
        &base.probes,
        cell_levels,
        affinity_offsets,
    )
    .map_err(|error| section_validation_from_error(section_name, error))
}

/// I2 applies to every parsed delta section whose affinity grid names this
/// id-34 grid, even if another optional direct-light contract later chooses to
/// disable that section. Keep the grid test separate so legacy soft-failure
/// handling for unrelated optional-section defects remains unchanged.
pub(crate) fn delta_grid_matches_base(
    base: &OctahedralShVolumeSection,
    affinity_factor: u8,
    affinity_dims: [u32; 3],
) -> bool {
    affinity_factor == AFFINITY_FACTOR
        && affinity_dims == expected_affinity_dims(base.grid_dimensions, AFFINITY_FACTOR)
}

pub(crate) fn validate_direct_sh_delta(
    section: &DirectShDeltaVolumesSection,
    direct: &DirectShVolumeSection,
    base: &OctahedralShVolumeSection,
    selected_light_count: usize,
) -> Result<(), PrlLoadError> {
    if section.affinity_factor != AFFINITY_FACTOR {
        return Err(PrlLoadError::DirectShDeltaAffinityFactorMismatch {
            found: section.affinity_factor,
            expected: AFFINITY_FACTOR,
        });
    }

    let base_dims = direct.grid_dimensions;
    let expected = expected_affinity_dims(base_dims, AFFINITY_FACTOR);
    if section.affinity_dims != expected {
        return Err(PrlLoadError::DirectShDeltaAffinityDimsMismatch {
            found: section.affinity_dims,
            base_dims,
            factor: AFFINITY_FACTOR as u32,
            expected,
        });
    }

    if section.tile_dimension != direct.tile_dimension || section.tile_border != direct.tile_border
    {
        return Err(PrlLoadError::DirectShDeltaTileGeometryMismatch {
            found_dimension: section.tile_dimension,
            found_border: section.tile_border,
            base_dimension: direct.tile_dimension,
            base_border: direct.tile_border,
        });
    }

    let affinity_cell_count = section.affinity_cell_count();
    if section.valid_probe_masks.len() != affinity_cell_count {
        return Err(section_validation(
            "DirectShDeltaVolumes",
            format!(
                "valid_probe_masks has length {}, expected {affinity_cell_count}",
                section.valid_probe_masks.len()
            ),
        ));
    }
    if base.probes.len() != base.total_probes() {
        return Err(section_validation(
            "DirectShDeltaVolumes",
            format!(
                "OctahedralShVolume has {} probe metadata records for {} grid probes",
                base.probes.len(),
                base.total_probes(),
            ),
        ));
    }
    for (cell, &stored_mask) in section.valid_probe_masks.iter().enumerate() {
        let expected_mask = valid_probe_mask_for_affinity_cell(base, section.affinity_dims, cell);
        if stored_mask != expected_mask {
            return Err(section_validation(
                "DirectShDeltaVolumes",
                format!(
                    "valid_probe_masks[{cell}] {stored_mask:#018x} disagrees with OctahedralShVolume (id 34) validity {expected_mask:#018x}; recompile the .prl with the current `prl-build`"
                ),
            ));
        }
    }

    for (entry, &selection_index) in section.affinity_lights.iter().enumerate() {
        if selection_index as usize >= selected_light_count {
            return Err(section_validation(
                "DirectShDeltaVolumes",
                format!(
                    "affinity_lights[{entry}] selection index {selection_index} out of range for {selected_light_count} selected light(s)"
                ),
            ));
        }
    }

    let mut seen_selection_indices = vec![false; selected_light_count];
    for &selection_index in &section.affinity_lights {
        seen_selection_indices[selection_index as usize] = true;
    }
    if let Some(missing_index) = seen_selection_indices
        .iter()
        .position(|&has_delta| !has_delta)
    {
        return Err(section_validation(
            "DirectShDeltaVolumes",
            format!(
                "missing usable delta entry for selected light index {missing_index} of {selected_light_count}"
            ),
        ));
    }

    Ok(())
}

pub(crate) fn valid_probe_mask_for_affinity_cell(
    base: &OctahedralShVolumeSection,
    affinity_dims: [u32; 3],
    cell_index: usize,
) -> u64 {
    let cell_index = cell_index as u32;
    let cell_x = cell_index % affinity_dims[0];
    let cell_y = (cell_index / affinity_dims[0]) % affinity_dims[1];
    let cell_z = cell_index / (affinity_dims[0] * affinity_dims[1]);
    let factor = AFFINITY_FACTOR as u32;
    let mut mask = 0u64;
    for local_z in 0..factor {
        for local_y in 0..factor {
            for local_x in 0..factor {
                let probe = [
                    cell_x * factor + local_x,
                    cell_y * factor + local_y,
                    cell_z * factor + local_z,
                ];
                if probe[0] >= base.grid_dimensions[0]
                    || probe[1] >= base.grid_dimensions[1]
                    || probe[2] >= base.grid_dimensions[2]
                {
                    continue;
                }
                let probe_index = probe[0] as usize
                    + probe[1] as usize * base.grid_dimensions[0] as usize
                    + probe[2] as usize
                        * base.grid_dimensions[0] as usize
                        * base.grid_dimensions[1] as usize;
                if base.probes[probe_index].validity != 0 {
                    let local = local_x + local_y * factor + local_z * factor * factor;
                    mask |= 1u64 << local;
                }
            }
        }
    }
    mask
}

pub(crate) fn validate_entity_shadow_light_selection(
    selected_light_indices: &[u32],
    lights: &[MapLight],
) -> Result<(), PrlLoadError> {
    for &index in selected_light_indices {
        let Some(light) = lights.get(index as usize) else {
            return Err(section_validation(
                "EntityShadowLights",
                format!(
                    "light index {index} exceeds level light count {}",
                    lights.len()
                ),
            ));
        };
        if light.is_dynamic {
            return Err(section_validation(
                "EntityShadowLights",
                format!("light index {index} references a dynamic-tier light"),
            ));
        }
        if light.light_type == LightType::Directional {
            return Err(section_validation(
                "EntityShadowLights",
                format!("light index {index} references a directional light"),
            ));
        }
        if light.shadow_type != ShadowType::StaticLightMap {
            return Err(section_validation(
                "EntityShadowLights",
                format!("light index {index} is not a static_light_map direct contributor"),
            ));
        }
        if light.animated_slot.is_some() {
            return Err(section_validation(
                "EntityShadowLights",
                format!("light index {index} references an animated static light"),
            ));
        }
    }
    Ok(())
}
