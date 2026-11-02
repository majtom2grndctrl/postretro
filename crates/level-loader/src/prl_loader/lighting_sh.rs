// SH companion sections: SH deltas, direct SH, billboard scatter, entity shadow lights.
// See: context/lib/build_pipeline.md §PRL Compilation

use postretro_level_format::SectionId;
use postretro_level_format::animated_billboard_direct_scatter_delta_volumes::AnimatedBillboardDirectScatterDeltaVolumesSection;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::billboard_direct_scatter_volume::BillboardDirectScatterVolumeSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;

use super::lighting::LightingDecode;
use super::{
    BoundedDeltaSectionData, BoundedScatterSectionData, delta_grid_matches_base,
    read_bounded_delta_section_data_with_limit, read_bounded_scatter_section_data_with_limit,
    read_soft_optional_scatter_section_data, section_validation, section_validation_from_error,
    validate_animated_billboard_direct_scatter_against_metadata,
    validate_animated_billboard_direct_scatter_delta_volumes, validate_animated_direct_sh_delta,
    validate_billboard_direct_scatter_against_metadata, validate_billboard_direct_scatter_volume,
    validate_delta_sh, validate_direct_sh_delta, validate_direct_sh_layout,
    validate_entity_shadow_light_selection, validate_storage_ceiling_for_delta,
};
use crate::PrlLoadError;
use crate::prl_container::PrlContainer;
use crate::sh_stream::ShStreamManifest;

fn validate_streamed_direct_delta_selection(
    metadata: &crate::sh_stream::ShStreamSparseMetadata,
    selected_light_count: usize,
) -> Result<(), PrlLoadError> {
    let mut seen = vec![false; selected_light_count];
    for &selection in &metadata.affinity_lights {
        let selection = usize::try_from(selection).map_err(|_| {
            section_validation("DirectShDeltaVolumes", "selection index exceeds usize")
        })?;
        let Some(seen) = seen.get_mut(selection) else {
            return Err(section_validation(
                "DirectShDeltaVolumes",
                format!(
                    "affinity light selection {selection} out of range for {selected_light_count} selected light(s)"
                ),
            ));
        };
        *seen = true;
    }
    if let Some(missing) = seen.iter().position(|&has_delta| !has_delta) {
        return Err(section_validation(
            "DirectShDeltaVolumes",
            format!("missing usable delta entry for selected light index {missing}"),
        ));
    }
    Ok(())
}

/// SH companion sections as decoded, before parse-end validation derives
/// their runtime form.
pub(super) struct ShCompanions {
    pub(super) delta_sh_volumes: Option<DeltaShVolumesSection>,
    pub(super) parsed_animated_direct_sh_delta_volumes: Option<AnimatedDirectShDeltaVolumesSection>,
    pub(super) parsed_billboard_direct_scatter_volume: Option<BillboardDirectScatterVolumeSection>,
    pub(super) parsed_animated_billboard_direct_scatter_delta_volumes:
        Option<AnimatedBillboardDirectScatterDeltaVolumesSection>,
    pub(super) scatter_pair_is_usable: bool,
    pub(super) direct_sh_volume: Option<DirectShVolumeSection>,
    pub(super) parsed_entity_shadow_lights: Option<EntityShadowLightsSection>,
    pub(super) entity_shadow_lights: Vec<u32>,
    pub(super) direct_sh_delta_volumes: Option<DirectShDeltaVolumesSection>,
}

pub(super) fn decode_sh_companions(
    container: &PrlContainer,
    streaming: Option<&ShStreamManifest>,
    max_delta_section_binding_bytes: u64,
    max_scatter_section_bytes: u64,
    lighting: &LightingDecode,
) -> Result<ShCompanions, PrlLoadError> {
    let meta = container.metadata();
    let read_section = |section: SectionId| container.read_section(section as u32);
    let LightingDecode {
        lights, sh_volume, ..
    } = lighting;

    // Optional — absent → SH compose pass falls back to base→total copy.
    let delta_sh_volumes: Option<DeltaShVolumesSection> = if streaming.is_some() {
        // Id 27's body is cluster-streamed. Manifest construction has retained
        // and cross-validated the CSR projection but intentionally no tiles.
        None
    } else {
        match read_bounded_delta_section_data_with_limit(
            container,
            SectionId::DeltaShVolumes,
            "DeltaShVolumes",
            max_delta_section_binding_bytes,
        )? {
            BoundedDeltaSectionData::Data(data) => {
                let section = DeltaShVolumesSection::from_bytes(&data)?;

                // Validation (mirrors the section-version reject path): a mismatched
                // bake must fail the load with a clear error rather than feed the
                // compose pass garbage. `sh_volume` (id 34) was loaded above.
                validate_delta_sh(&section, sh_volume.as_ref())?;
                validate_storage_ceiling_for_delta(
                    "DeltaShVolumes",
                    sh_volume.as_ref().expect("id-34 is required before id-27"),
                    &section.cell_levels,
                    &section.affinity_offsets,
                )?;

                log::info!(
                    "[PRL] DeltaShVolumes: {} animated light(s), affinity grid {}×{}×{} \
                     ({} CSR entr(y/ies), {} delta subblock halves)",
                    section.animation_descriptor_indices.len(),
                    section.affinity_dims[0],
                    section.affinity_dims[1],
                    section.affinity_dims[2],
                    section.affinity_lights.len(),
                    section.delta_subblocks.len(),
                );
                Some(section)
            }
            BoundedDeltaSectionData::Absent | BoundedDeltaSectionData::OverBindingFloor => None,
        }
    };

    // Optional — malformed animated-direct deltas disable only this additive
    // term. An id-45/id-34 valid-probe descriptor disagreement is different:
    // it would make compact payload offsets address the wrong tiles, so reject
    // the complete load before any renderer buffers are built.
    let parsed_animated_direct_sh_delta_volumes: Option<AnimatedDirectShDeltaVolumesSection> =
        if streaming.is_some() {
            // Id 45 is cluster-streamed. Id 48 validates against the retained
            // metadata projection below.
            None
        } else {
            match read_bounded_delta_section_data_with_limit(
                container,
                SectionId::AnimatedDirectShDeltaVolumes,
                "AnimatedDirectShDeltaVolumes",
                max_delta_section_binding_bytes,
            )? {
                BoundedDeltaSectionData::Data(data) => {
                    match AnimatedDirectShDeltaVolumesSection::from_bytes(&data) {
                        Ok(section) => {
                            let base = sh_volume.as_ref().expect("id-34 is required before id-45");
                            if delta_grid_matches_base(
                                base,
                                section.affinity_factor,
                                section.affinity_dims,
                            ) {
                                validate_storage_ceiling_for_delta(
                                    "AnimatedDirectShDeltaVolumes",
                                    base,
                                    &section.cell_levels,
                                    &section.affinity_offsets,
                                )?;
                            }
                            match validate_animated_direct_sh_delta(&section, sh_volume.as_ref()) {
                                Ok(()) => {
                                    log::info!(
                                        "[PRL] AnimatedDirectShDeltaVolumes: {} animated light(s), affinity grid {}×{}×{} ({} CSR entr(y/ies), {} delta subblock halves)",
                                        section.animation_descriptor_indices.len(),
                                        section.affinity_dims[0],
                                        section.affinity_dims[1],
                                        section.affinity_dims[2],
                                        section.affinity_lights.len(),
                                        section.delta_subblocks.len(),
                                    );
                                    Some(section)
                                }
                                Err(
                                    err @ PrlLoadError::AnimatedDirectShDeltaValidityMismatch {
                                        ..
                                    },
                                ) => {
                                    return Err(err);
                                }
                                Err(err) => {
                                    log::warn!(
                                        "[PRL] AnimatedDirectShDeltaVolumes unusable; disabling animated direct SH: {err}"
                                    );
                                    None
                                }
                            }
                        }
                        Err(err) => {
                            log::warn!(
                                "[PRL] AnimatedDirectShDeltaVolumes malformed; disabling animated direct SH: {err}"
                            );
                            None
                        }
                    }
                }
                BoundedDeltaSectionData::Absent | BoundedDeltaSectionData::OverBindingFloor => None,
            }
        };

    // Optional normal-free direct scatter for billboard lighting. This is an
    // optimization with a legacy direct-light fallback, so all parse and
    // cross-validation failures deliberately clear it instead of rejecting a
    // map that otherwise loads.
    let parsed_billboard_direct_scatter_volume: Option<BillboardDirectScatterVolumeSection> =
        if let Some(manifest) = streaming {
            match read_section(SectionId::BillboardDirectScatterVolume)? {
                Some(data) => match BillboardDirectScatterVolumeSection::from_bytes(&data) {
                    Ok(section) => match validate_billboard_direct_scatter_against_metadata(
                        &section,
                        manifest.base(),
                    ) {
                        Ok(()) => Some(section),
                        Err(error) => {
                            log::warn!(
                                "[PRL] BillboardDirectScatterVolume unusable; disabling billboard direct scatter: {error}"
                            );
                            None
                        }
                    },
                    Err(error) => {
                        log::warn!(
                            "[PRL] BillboardDirectScatterVolume malformed; disabling billboard direct scatter: {error}"
                        );
                        None
                    }
                },
                None => None,
            }
        } else {
            match read_soft_optional_scatter_section_data(
                container,
                SectionId::BillboardDirectScatterVolume,
                "BillboardDirectScatterVolume",
            ) {
                Some(data) => match BillboardDirectScatterVolumeSection::from_bytes(&data) {
                    Ok(section) => {
                        match validate_billboard_direct_scatter_volume(&section, sh_volume.as_ref())
                        {
                            Ok(()) => {
                                log::info!(
                                    "[PRL] BillboardDirectScatterVolume: {}×{}×{} grid ({} RGBA16F scatter halves)",
                                    section.grid_dimensions[0],
                                    section.grid_dimensions[1],
                                    section.grid_dimensions[2],
                                    section.scatter_rgba.len(),
                                );
                                Some(section)
                            }
                            Err(error) => {
                                log::warn!(
                                    "[PRL] BillboardDirectScatterVolume unusable; disabling billboard direct scatter: {error}"
                                );
                                None
                            }
                        }
                    }
                    Err(error) => {
                        log::warn!(
                            "[PRL] BillboardDirectScatterVolume malformed; disabling billboard direct scatter: {error}"
                        );
                        None
                    }
                },
                None => None,
            }
        };

    let id45_present = container.has_section(SectionId::AnimatedDirectShDeltaVolumes as u32);
    let id48_present =
        container.has_section(SectionId::AnimatedBillboardDirectScatterDeltaVolumes as u32);
    let parsed_animated_billboard_direct_scatter_delta_volumes: Option<
        AnimatedBillboardDirectScatterDeltaVolumesSection,
    > = if let Some(manifest) = streaming {
        match manifest.sources().animated_direct_delta.as_ref() {
            None => None,
            Some(animated_direct) => {
                if let Some(entry) =
                    meta.find_section(SectionId::AnimatedBillboardDirectScatterDeltaVolumes as u32)
                    && entry.size > max_scatter_section_bytes
                {
                    log::warn!(
                        "[PRL] AnimatedBillboardDirectScatterDeltaVolumes raw payload is {} B, above the {} B pack cap; disabling billboard direct scatter",
                        entry.size,
                        max_scatter_section_bytes,
                    );
                    container.settle_section(
                        SectionId::AnimatedBillboardDirectScatterDeltaVolumes as u32,
                        0,
                    );
                    None
                } else {
                    match read_section(SectionId::AnimatedBillboardDirectScatterDeltaVolumes)? {
                        Some(data) => match AnimatedBillboardDirectScatterDeltaVolumesSection::from_bytes(&data) {
                            Ok(section) => match validate_animated_billboard_direct_scatter_against_metadata(
                                &section,
                                animated_direct,
                            ) {
                                Ok(()) => Some(section),
                                Err(error) => {
                                    log::warn!(
                                        "[PRL] AnimatedBillboardDirectScatterDeltaVolumes unusable; disabling billboard direct scatter: {error}"
                                    );
                                    None
                                }
                            },
                            Err(error) => {
                                log::warn!(
                                    "[PRL] AnimatedBillboardDirectScatterDeltaVolumes malformed; disabling billboard direct scatter: {error}"
                                );
                                None
                            }
                        },
                        None => None,
                    }
                }
            }
        }
    } else {
        match parsed_animated_direct_sh_delta_volumes.as_ref() {
            Some(animated_direct) => match read_bounded_scatter_section_data_with_limit(
                container,
                max_scatter_section_bytes,
            ) {
                BoundedScatterSectionData::Data(data) => {
                    match AnimatedBillboardDirectScatterDeltaVolumesSection::from_bytes(&data) {
                        Ok(section) => {
                            match validate_animated_billboard_direct_scatter_delta_volumes(
                                &section,
                                animated_direct,
                            ) {
                                Ok(()) => Some(section),
                                Err(error) => {
                                    log::warn!(
                                        "[PRL] AnimatedBillboardDirectScatterDeltaVolumes unusable; disabling billboard direct scatter: {error}"
                                    );
                                    None
                                }
                            }
                        }
                        Err(error) => {
                            log::warn!(
                                "[PRL] AnimatedBillboardDirectScatterDeltaVolumes malformed; disabling billboard direct scatter: {error}"
                            );
                            None
                        }
                    }
                }
                BoundedScatterSectionData::Absent | BoundedScatterSectionData::OverPackCap => None,
            },
            None => None,
        }
    };

    // The id-45 layout is authoritative whenever present. A static-only map
    // needs only id 47; an animated map needs both the usable id-45 layout and
    // its id-48 dense companion. Id 48 without id 45 is likewise an invalid
    // pair. Keep both fields absent so downstream code cannot accidentally mix
    // scatter with the legacy animated-direct path.
    let scatter_pair_is_usable = match (id45_present, id48_present) {
        (false, false) => true,
        (true, true) => parsed_animated_billboard_direct_scatter_delta_volumes.is_some(),
        (true, false) => false,
        (false, true) => false,
    };
    if parsed_billboard_direct_scatter_volume.is_some() && !scatter_pair_is_usable {
        let reason = match (id45_present, id48_present) {
            (true, false) => {
                "AnimatedDirectShDeltaVolumes (id 45) is present without AnimatedBillboardDirectScatterDeltaVolumes (id 48)"
            }
            (false, true) => {
                "AnimatedBillboardDirectScatterDeltaVolumes (id 48) is present without AnimatedDirectShDeltaVolumes (id 45)"
            }
            (true, true) => "the id-45/id-48 animated scatter pair is unusable",
            (false, false) => "the static-only scatter pair is unexpectedly unusable",
        };
        log::warn!("[PRL] {reason}; disabling billboard direct scatter");
    }

    // Optional — absent when the map has no static direct SH/static lights.
    // Dynamic objects fall back to indirect-only.
    let direct_sh_volume: Option<DirectShVolumeSection> = match streaming {
        Some(_) => None,
        None => match read_section(SectionId::DirectShVolume)? {
            Some(data) => {
                // id-35 is a stored-set sibling of required id-34. A present stale,
                // malformed, geometry-mismatched, or length-mismatched payload has
                // no safe downgrade: it would address a different stored slot map.
                let section = DirectShVolumeSection::from_bytes(&data)
                    .map_err(|error| section_validation_from_error("DirectShVolume", error))?;
                let base = sh_volume.as_ref().expect("id-34 is required before id-35");
                validate_direct_sh_layout(&section, base)?;
                log::info!(
                    "[PRL] DirectShVolume: {}×{}×{} grid ({} probes, {}×{} stored atlas, {} atlas layer(s), tile {} + border {}, {} tile(s)/row, {} tile(s)/layer, format {}, {} atlas byte(s))",
                    section.grid_dimensions[0],
                    section.grid_dimensions[1],
                    section.grid_dimensions[2],
                    section.total_probes(),
                    section.atlas_dimensions[0],
                    section.atlas_dimensions[1],
                    section.layer_count,
                    section.tile_dimension,
                    section.tile_border,
                    section.atlas_tiles_per_row,
                    section.tiles_per_layer,
                    section.irradiance_format,
                    section.atlas.len(),
                );
                Some(section)
            }
            None => None,
        },
    };

    let has_direct_sh = streaming.map_or(direct_sh_volume.is_some(), |manifest| {
        manifest.sources().direct.is_some()
    });
    let parsed_entity_shadow_lights: Option<EntityShadowLightsSection> = match read_section(
        SectionId::EntityShadowLights,
    )? {
        // A corrupt/tampered EntityShadowLights section degrades to empty (warn +
        // clear), mirroring the sibling DirectShDeltaVolumes path below — presence
        // without a usable selection means "no promotion", not a bricked load.
        // Older PRLs (absent section) already load fine via the `None` arm. The
        // outer `?` still propagates a structurally broken section table.
        Some(data) => match EntityShadowLightsSection::from_bytes(&data) {
            Ok(section) => {
                if !has_direct_sh {
                    if !section.light_indices.is_empty() {
                        log::warn!(
                            "[PRL] EntityShadowLights present without DirectShVolume; ignoring {} selected light(s)",
                            section.light_indices.len()
                        );
                    }
                    None
                } else if let Err(err) =
                    validate_entity_shadow_light_selection(&section.light_indices, lights)
                {
                    log::warn!(
                        "[PRL] EntityShadowLights invalid selection; clearing {} selected static light(s): {err}",
                        section.light_indices.len()
                    );
                    None
                } else {
                    log::info!(
                        "[PRL] EntityShadowLights: {} selected static light(s)",
                        section.light_indices.len()
                    );
                    Some(section)
                }
            }
            Err(err) => {
                log::warn!(
                    "[PRL] EntityShadowLights malformed; treating as empty (no promotion): {err}"
                );
                None
            }
        },
        None => None,
    };
    let mut entity_shadow_lights = parsed_entity_shadow_lights
        .as_ref()
        .map_or_else(Vec::new, |section| section.light_indices.clone());

    let direct_sh_delta_volumes: Option<DirectShDeltaVolumesSection> = if let Some(manifest) =
        streaming
    {
        match manifest.sources().direct_delta.as_ref() {
            Some(metadata) if !entity_shadow_lights.is_empty() => {
                if let Err(error) =
                    validate_streamed_direct_delta_selection(metadata, entity_shadow_lights.len())
                {
                    log::warn!(
                        "[PRL] streamed DirectShDeltaVolumes metadata unusable for EntityShadowLights; clearing {} selected static light(s): {error}",
                        entity_shadow_lights.len(),
                    );
                    entity_shadow_lights.clear();
                }
                None
            }
            Some(_) => None,
            None => {
                if !entity_shadow_lights.is_empty() {
                    log::warn!(
                        "[PRL] EntityShadowLights present without streamed DirectShDeltaVolumes; clearing {} selected static light(s)",
                        entity_shadow_lights.len(),
                    );
                    entity_shadow_lights.clear();
                }
                None
            }
        }
    } else {
        match read_bounded_delta_section_data_with_limit(
            container,
            SectionId::DirectShDeltaVolumes,
            "DirectShDeltaVolumes",
            max_delta_section_binding_bytes,
        )? {
            BoundedDeltaSectionData::Data(data) => {
                match DirectShDeltaVolumesSection::from_bytes(&data) {
                    Ok(section) => {
                        let base = sh_volume.as_ref().expect("id-34 is required before id-41");
                        if delta_grid_matches_base(
                            base,
                            section.affinity_factor,
                            section.affinity_dims,
                        ) {
                            validate_storage_ceiling_for_delta(
                                "DirectShDeltaVolumes",
                                base,
                                &section.cell_levels,
                                &section.affinity_offsets,
                            )?;
                        }
                        if entity_shadow_lights.is_empty() {
                            log::warn!(
                                "[PRL] DirectShDeltaVolumes present without selected EntityShadowLights; ignoring section"
                            );
                            None
                        } else {
                            if let (Some(direct), Some(base)) =
                                (direct_sh_volume.as_ref(), sh_volume.as_ref())
                            {
                                match validate_direct_sh_delta(
                                    &section,
                                    direct,
                                    base,
                                    entity_shadow_lights.len(),
                                ) {
                                    Ok(()) => {
                                        log::info!(
                                            "[PRL] DirectShDeltaVolumes: {} selected-light CSR entr(y/ies), affinity grid {}×{}×{} ({} delta subblock halves)",
                                            section.affinity_lights.len(),
                                            section.affinity_dims[0],
                                            section.affinity_dims[1],
                                            section.affinity_dims[2],
                                            section.delta_subblocks.len(),
                                        );
                                        Some(section)
                                    }
                                    Err(err) => {
                                        log::warn!(
                                            "[PRL] DirectShDeltaVolumes unusable for EntityShadowLights; clearing {} selected static light(s): {err}",
                                            entity_shadow_lights.len()
                                        );
                                        entity_shadow_lights.clear();
                                        None
                                    }
                                }
                            } else {
                                log::warn!(
                                    "[PRL] DirectShDeltaVolumes present without DirectShVolume or OctahedralShVolume; clearing {} selected static light(s)",
                                    entity_shadow_lights.len()
                                );
                                entity_shadow_lights.clear();
                                None
                            }
                        }
                    }
                    Err(err) => {
                        log::warn!(
                            "[PRL] DirectShDeltaVolumes malformed; clearing {} selected static light(s): {err}",
                            entity_shadow_lights.len()
                        );
                        entity_shadow_lights.clear();
                        None
                    }
                }
            }
            BoundedDeltaSectionData::OverBindingFloor => {
                if !entity_shadow_lights.is_empty() {
                    log::warn!(
                        "[PRL] DirectShDeltaVolumes exceeded the storage-binding floor; clearing {} selected static light(s)",
                        entity_shadow_lights.len()
                    );
                    entity_shadow_lights.clear();
                }
                None
            }
            BoundedDeltaSectionData::Absent => {
                if !entity_shadow_lights.is_empty() {
                    log::warn!(
                        "[PRL] EntityShadowLights present without DirectShDeltaVolumes; clearing {} selected static light(s)",
                        entity_shadow_lights.len()
                    );
                    entity_shadow_lights.clear();
                }
                None
            }
        }
    };

    Ok(ShCompanions {
        delta_sh_volumes,
        parsed_animated_direct_sh_delta_volumes,
        parsed_billboard_direct_scatter_volume,
        parsed_animated_billboard_direct_scatter_delta_volumes,
        scatter_pair_is_usable,
        direct_sh_volume,
        parsed_entity_shadow_lights,
        entity_shadow_lights,
        direct_sh_delta_volumes,
    })
}
