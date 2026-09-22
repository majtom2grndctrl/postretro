//! Id-50 source inventory, geometry, and cross-section validation.
//! See: context/plans/in-progress/sh-probe-streaming--cluster-residency/index.md

use super::*;

pub(super) fn validate_inputs(
    inputs: ClusterShPayloadsValidationInputs<'_>,
) -> Result<(), ClusterShPayloadsError> {
    inputs.directory.validate_structure().map_err(|error| {
        ClusterShPayloadsError::SourceMismatch(format!("id 49 is invalid: {error}"))
    })?;
    if inputs.base.tile_dimension != CLUSTER_SH_LOGICAL_TILE_DIMENSION
        || inputs.base.tile_border != CLUSTER_SH_LOGICAL_TILE_BORDER
    {
        return source_mismatch(
            "id 34 tile geometry is not the required 6x6/border-1 layout".into(),
        );
    }
    let probe_count = checked_product(inputs.base.grid_dimensions)?;
    let probe_count_usize = usize::try_from(probe_count)
        .map_err(|_| ClusterShPayloadsError::SizeOverflow("base probe count exceeds usize"))?;
    if inputs.base.probes.len() != probe_count_usize {
        return source_mismatch(format!(
            "id 34 has {} probe records, expected {probe_count}",
            inputs.base.probes.len()
        ));
    }
    if !matches!(
        inputs.base.irradiance_format,
        IRRADIANCE_FORMAT_BC6H | IRRADIANCE_FORMAT_RGBA16F
    ) {
        return source_mismatch("id 34 has an unknown irradiance format".into());
    }
    let expected_affinity = affinity_dimensions(inputs.base.grid_dimensions)?;
    let mut previous = None;
    let mut base_present = false;
    for source in inputs.sources {
        if !is_streamed_section(source.section_id()) {
            return source_mismatch(format!("source {} is not streamable", source.section_id()));
        }
        if previous.is_some_and(|id| id >= source.section_id()) {
            return source_mismatch("source metadata is not sorted and unique".into());
        }
        previous = Some(source.section_id());
        if source.section_id() == SectionId::OctahedralShVolume as u32 {
            base_present = matches!(source, ClusterShPayloadsSourceMetadata::Dense { .. });
        }
        match source {
            ClusterShPayloadsSourceMetadata::Dense {
                section_id,
                internal_version,
                irradiance_format,
            } => {
                if !matches!(
                    *section_id,
                    id if id == SectionId::OctahedralShVolume as u32
                        || id == SectionId::DirectShVolume as u32
                ) || !matches!(
                    *irradiance_format,
                    IRRADIANCE_FORMAT_BC6H | IRRADIANCE_FORMAT_RGBA16F
                ) {
                    return source_mismatch(format!("dense source {section_id} is invalid"));
                }
                if *internal_version != expected_internal_version(*section_id)? {
                    return source_mismatch(format!(
                        "dense source {section_id} has stale version {internal_version}"
                    ));
                }
                if *irradiance_format != inputs.base.irradiance_format {
                    return source_mismatch(format!(
                        "dense source {section_id} format disagrees with id 34"
                    ));
                }
            }
            ClusterShPayloadsSourceMetadata::Sparse {
                section_id,
                internal_version,
                affinity_dimensions,
                tile_dimension,
                tile_border,
                valid_probe_masks,
                cell_levels,
                affinity_offsets,
                affinity_lights,
            } => {
                if !matches!(
                    *section_id,
                    id if id == SectionId::DeltaShVolumes as u32
                        || id == SectionId::DirectShDeltaVolumes as u32
                        || id == SectionId::AnimatedDirectShDeltaVolumes as u32
                ) {
                    return source_mismatch(format!("sparse source {section_id} is invalid"));
                }
                if *internal_version != expected_internal_version(*section_id)? {
                    return source_mismatch(format!(
                        "sparse source {section_id} has stale version {internal_version}"
                    ));
                }
                if *affinity_dimensions != expected_affinity
                    || *tile_dimension != CLUSTER_SH_LOGICAL_TILE_DIMENSION
                    || *tile_border != CLUSTER_SH_LOGICAL_TILE_BORDER
                {
                    return source_mismatch(format!(
                        "sparse source {section_id} geometry disagrees with id 34"
                    ));
                }
                let cell_count = checked_product(*affinity_dimensions)?;
                let cell_count = usize::try_from(cell_count).map_err(|_| {
                    ClusterShPayloadsError::SizeOverflow("sparse affinity cell count exceeds usize")
                })?;
                let offset_count =
                    cell_count
                        .checked_add(1)
                        .ok_or(ClusterShPayloadsError::SizeOverflow(
                            "sparse affinity offset count",
                        ))?;
                let light_count = u32::try_from(affinity_lights.len()).map_err(|_| {
                    ClusterShPayloadsError::SizeOverflow("sparse affinity light count")
                })?;
                if valid_probe_masks.len() != cell_count
                    || cell_levels.len() != cell_count
                    || affinity_offsets.len() != offset_count
                    || affinity_offsets.first() != Some(&0)
                    || affinity_offsets.last().copied() != Some(light_count)
                    || affinity_offsets.windows(2).any(|pair| pair[0] > pair[1])
                {
                    return source_mismatch(format!(
                        "sparse source {section_id} CSR metadata is invalid"
                    ));
                }
                if cell_levels
                    .iter()
                    .any(|&level| Level::from_u8(level).is_none())
                {
                    return source_mismatch(format!(
                        "sparse source {section_id} has an invalid cell level"
                    ));
                }
                validate_storage_levels_against_delta(
                    inputs.base.grid_dimensions,
                    inputs.base.probes,
                    cell_levels,
                    affinity_offsets,
                )
                .map_err(|error| {
                    ClusterShPayloadsError::SourceMismatch(format!(
                        "sparse source {section_id} storage metadata disagrees with id 34: {error}"
                    ))
                })?;
            }
        }
    }
    if !base_present {
        return source_mismatch("id 50 requires id 34 in the source inventory".into());
    }
    validate_directory_source_resources(
        inputs.directory,
        inputs.sources,
        inputs.base.grid_dimensions,
        expected_affinity,
    )?;
    Ok(())
}

fn validate_directory_source_resources(
    directory: &ClusterDirectorySection,
    sources: &[ClusterShPayloadsSourceMetadata<'_>],
    grid_dimensions: [u32; 3],
    affinity_dimensions: [u32; 3],
) -> Result<(), ClusterShPayloadsError> {
    for resource in &directory.resources {
        if is_streamed_section(resource.section_id)
            && !sources
                .iter()
                .any(|source| source.section_id() == resource.section_id)
        {
            return source_mismatch(format!(
                "id 49 streamed resource {} is absent from the id-50 inventory",
                resource.section_id
            ));
        }
    }
    for source in sources {
        let resource = directory
            .resources
            .iter()
            .find(|resource| resource.section_id == source.section_id())
            .ok_or_else(|| {
                ClusterShPayloadsError::SourceMismatch(format!(
                    "id 49 lacks source resource {}",
                    source.section_id()
                ))
            })?;
        let (domain, dimensions) = match source.kind() {
            ClusterShPayloadsSourceKind::DenseBaseAtlas => {
                (ClusterResourceDomain::DenseProbe, grid_dimensions)
            }
            ClusterShPayloadsSourceKind::SparseAffinity => {
                (ClusterResourceDomain::AffinityCell, affinity_dimensions)
            }
        };
        if resource.domain != domain || resource.dimensions != dimensions {
            return source_mismatch(format!(
                "id 49 resource {} disagrees with source geometry",
                source.section_id()
            ));
        }
    }
    for source in sources {
        if source.section_id() == SectionId::OctahedralShVolume as u32
            || source.kind() != ClusterShPayloadsSourceKind::DenseBaseAtlas
        {
            continue;
        }
        for cluster_id in 0..directory.clusters.len() as u32 {
            if dense_indices(directory, cluster_id, source.section_id())?
                != dense_indices(directory, cluster_id, SectionId::OctahedralShVolume as u32)?
            {
                return source_mismatch(format!(
                    "id 49 dense source {} coverage differs from id 34 in cluster {cluster_id}",
                    source.section_id()
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_source_inventory(
    records: &[ClusterShPayloadsSourceRecord],
    sources: &[ClusterShPayloadsSourceMetadata<'_>],
) -> Result<(), ClusterShPayloadsError> {
    if records.len() != sources.len() {
        return source_mismatch(format!(
            "source record count {} disagrees with present source count {}",
            records.len(),
            sources.len()
        ));
    }
    for (record, source) in records.iter().zip(sources) {
        if record.section_id != source.section_id()
            || record.internal_version != source.internal_version()
            || record.kind != source.kind()
        {
            return source_mismatch(format!(
                "source record {} does not match present source inventory",
                record.section_id
            ));
        }
    }
    Ok(())
}
