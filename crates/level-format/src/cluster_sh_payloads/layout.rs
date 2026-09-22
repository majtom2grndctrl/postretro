//! Checked shared address, atlas, sparse-row, and source helpers for id 50.
//! See: context/plans/in-progress/sh-probe-streaming--cluster-residency/index.md

use super::*;

pub(super) struct SparseSource<'a> {
    pub(super) valid_probe_masks: &'a [u64],
    pub(super) cell_levels: &'a [u8],
    pub(super) affinity_offsets: &'a [u32],
    pub(super) affinity_lights: &'a [u32],
}

pub(super) fn sparse_source_from_inputs<'a>(
    sources: &'a [ClusterShPayloadsSourceMetadata<'a>],
    section_id: u32,
) -> Result<SparseSource<'a>, ClusterShPayloadsError> {
    let source = sources
        .iter()
        .find(|source| source.section_id() == section_id)
        .ok_or_else(|| {
            ClusterShPayloadsError::SourceMismatch(format!("missing sparse source {section_id}"))
        })?;
    match source {
        ClusterShPayloadsSourceMetadata::Sparse {
            valid_probe_masks,
            cell_levels,
            affinity_offsets,
            affinity_lights,
            ..
        } => Ok(SparseSource {
            valid_probe_masks,
            cell_levels,
            affinity_offsets,
            affinity_lights,
        }),
        ClusterShPayloadsSourceMetadata::Dense { .. } => {
            source_mismatch(format!("source {section_id} is not sparse"))
        }
    }
}

pub(super) fn dense_source<'a>(
    sources: &'a [ClusterShPayloadsSourceMetadata<'a>],
    section_id: u32,
) -> Result<ClusterShPayloadsSourceMetadata<'a>, ClusterShPayloadsError> {
    let source = sources
        .iter()
        .find(|source| source.section_id() == section_id)
        .copied()
        .ok_or_else(|| {
            ClusterShPayloadsError::SourceMismatch(format!("missing dense source {section_id}"))
        })?;
    if source.kind() != ClusterShPayloadsSourceKind::DenseBaseAtlas {
        return source_mismatch(format!("source {section_id} is not dense"));
    }
    Ok(source)
}

pub(super) fn dense_format(
    source: ClusterShPayloadsSourceMetadata<'_>,
) -> Result<u32, ClusterShPayloadsError> {
    match source {
        ClusterShPayloadsSourceMetadata::Dense {
            irradiance_format, ..
        } => Ok(irradiance_format),
        ClusterShPayloadsSourceMetadata::Sparse { section_id, .. } => {
            source_mismatch(format!("source {section_id} is not dense"))
        }
    }
}

pub(super) fn isolated_layout(
    slot_count: u32,
) -> Result<IrradianceAtlasArrayLayout, ClusterShPayloadsError> {
    cluster_sh_isolated_atlas_array_layout(slot_count).ok_or_else(|| {
        ClusterShPayloadsError::InvalidData("isolated atlas layout exceeds format cap".into())
    })
}

pub(super) fn isolated_tile_bytes(format: u32) -> Result<u64, ClusterShPayloadsError> {
    match format {
        IRRADIANCE_FORMAT_BC6H => Ok(64),
        IRRADIANCE_FORMAT_RGBA16F => Ok(512),
        _ => invalid("unknown isolated atlas format".into()),
    }
}

pub(super) fn isolated_atlas_payload_len(
    slot_count: u32,
    format: u32,
) -> Result<u64, ClusterShPayloadsError> {
    let layout = isolated_layout(slot_count)?;
    match format {
        IRRADIANCE_FORMAT_BC6H => u64::from(layout.layer_count)
            .checked_mul(u64::from(layout.atlas_width / 4))
            .and_then(|value| value.checked_mul(u64::from(layout.atlas_height / 4)))
            .and_then(|value| value.checked_mul(16))
            .ok_or(ClusterShPayloadsError::SizeOverflow(
                "BC6H isolated atlas bytes",
            )),
        IRRADIANCE_FORMAT_RGBA16F => u64::from(layout.layer_count)
            .checked_mul(u64::from(layout.atlas_width))
            .and_then(|value| value.checked_mul(u64::from(layout.atlas_height)))
            .and_then(|value| value.checked_mul(8))
            .ok_or(ClusterShPayloadsError::SizeOverflow(
                "RGBA16F isolated atlas bytes",
            )),
        _ => invalid("unknown isolated atlas format".into()),
    }
}

pub(super) fn isolated_atlas_block_len(
    slot_count: u32,
    format: u32,
) -> Result<u64, ClusterShPayloadsError> {
    20u64
        .checked_add(isolated_atlas_payload_len(slot_count, format)?)
        .ok_or(ClusterShPayloadsError::SizeOverflow(
            "isolated atlas block bytes",
        ))
}

pub(super) fn sparse_counts(
    rows: &[u32],
    valid_probe_masks: &[u64],
    cell_levels: &[u8],
    offsets: &[u32],
    section_id: u32,
) -> Result<(usize, usize), ClusterShPayloadsError> {
    let mut entries = 0usize;
    let mut tiles = 0usize;
    for &row in rows {
        let start = *offsets.get(row as usize).ok_or_else(|| {
            ClusterShPayloadsError::SourceMismatch(format!(
                "source {section_id} row {row} lacks CSR offset"
            ))
        })?;
        let end = *offsets.get(row as usize + 1).ok_or_else(|| {
            ClusterShPayloadsError::SourceMismatch(format!(
                "source {section_id} row {row} lacks CSR end"
            ))
        })?;
        let level = Level::from_u8(*cell_levels.get(row as usize).ok_or_else(|| {
            ClusterShPayloadsError::SourceMismatch(format!(
                "source {section_id} row {row} lacks level"
            ))
        })?)
        .ok_or_else(|| {
            ClusterShPayloadsError::SourceMismatch(format!(
                "source {section_id} row {row} has invalid level"
            ))
        })?;
        let stored = stored_delta_tiles(
            level,
            *valid_probe_masks.get(row as usize).ok_or_else(|| {
                ClusterShPayloadsError::SourceMismatch(format!(
                    "source {section_id} row {row} lacks validity mask"
                ))
            })?,
        )
        .checked_mul(delta_probe_f16_stride(CLUSTER_SH_LOGICAL_TILE_DIMENSION))
        .ok_or(ClusterShPayloadsError::SizeOverflow(
            "sparse per-entry f16 count",
        ))?;
        let count = usize::try_from(end - start).map_err(|_| {
            ClusterShPayloadsError::SizeOverflow("sparse entry count exceeds usize")
        })?;
        entries = entries
            .checked_add(count)
            .ok_or(ClusterShPayloadsError::SizeOverflow("sparse entry count"))?;
        tiles = tiles
            .checked_add(
                count
                    .checked_mul(stored)
                    .ok_or(ClusterShPayloadsError::SizeOverflow("sparse f16 count"))?,
            )
            .ok_or(ClusterShPayloadsError::SizeOverflow("sparse f16 count"))?;
    }
    Ok((entries, tiles))
}

pub(super) fn sparse_block_len(
    row_count: usize,
    entry_count: usize,
    tile_f16_count: usize,
) -> Result<u64, ClusterShPayloadsError> {
    let rows = u64::try_from(row_count)
        .map_err(|_| ClusterShPayloadsError::SizeOverflow("sparse row count exceeds u64"))?;
    let entries = u64::try_from(entry_count)
        .map_err(|_| ClusterShPayloadsError::SizeOverflow("sparse entry count exceeds u64"))?;
    let tiles = u64::try_from(tile_f16_count)
        .map_err(|_| ClusterShPayloadsError::SizeOverflow("sparse f16 count exceeds u64"))?;
    16u64
        .checked_add(
            rows.checked_mul(16)
                .ok_or(ClusterShPayloadsError::SizeOverflow("sparse row bytes"))?,
        )
        .and_then(|value| value.checked_add(entries.checked_mul(16)?))
        .and_then(|value| value.checked_add(tiles.checked_mul(2)?))
        .ok_or(ClusterShPayloadsError::SizeOverflow("sparse block bytes"))
}

pub(super) fn stored_prefix(
    base: ClusterShPayloadsBaseMetadata<'_>,
    affinity_dimensions: [u32; 3],
) -> Result<StoredBrickPrefixSum, ClusterShPayloadsError> {
    let prefix = validate_probe_metadata(base.grid_dimensions, base.probes).map_err(|error| {
        ClusterShPayloadsError::SourceMismatch(format!("id 34 metadata is invalid: {error}"))
    })?;
    if prefix.bricks.len()
        != usize::try_from(checked_product(affinity_dimensions)?)
            .map_err(|_| ClusterShPayloadsError::SizeOverflow("affinity brick count"))?
    {
        return source_mismatch(
            "id 34 stored-node prefix disagrees with affinity dimensions".into(),
        );
    }
    Ok(prefix)
}

pub(super) fn node_for_probe(
    index: u32,
    base: ClusterShPayloadsBaseMetadata<'_>,
    affinity_dimensions: [u32; 3],
) -> Result<Option<usize>, ClusterShPayloadsError> {
    let probe = base.probes.get(index as usize).ok_or_else(|| {
        ClusterShPayloadsError::SourceMismatch(format!("dense probe {index} is out of range"))
    })?;
    if probe.validity == 0 {
        return Ok(None);
    }
    let coords = unflatten_probe(index, base.grid_dimensions)?;
    let brick = [coords[0] / 4, coords[1] / 4, coords[2] / 4];
    let edge = 1u32
        .checked_shl(u32::from(probe.node_scale))
        .ok_or(ClusterShPayloadsError::SizeOverflow("node scale"))?;
    let origin = [
        brick[0] / edge * edge,
        brick[1] / edge * edge,
        brick[2] / edge * edge,
    ];
    Ok(Some(flatten(origin, affinity_dimensions)? as usize))
}

/// Return a valid L0 probe's compact ordinal within its own brick. L1 and L2
/// address their shared node base directly; L0 follows the renderer's compact
/// valid-probe order.
pub(super) fn local_probe_slot_offset(
    index: u32,
    base: ClusterShPayloadsBaseMetadata<'_>,
) -> Result<u32, ClusterShPayloadsError> {
    let probe =
        base.probes
            .get(usize::try_from(index).map_err(|_| {
                ClusterShPayloadsError::SizeOverflow("dense probe index exceeds usize")
            })?)
            .ok_or_else(|| {
                ClusterShPayloadsError::SourceMismatch(format!(
                    "dense probe {index} is outside id 34 metadata"
                ))
            })?;
    let level = Level::from_u8(probe.density_level).ok_or_else(|| {
        ClusterShPayloadsError::SourceMismatch(format!(
            "dense probe {index} has invalid density level {}",
            probe.density_level
        ))
    })?;
    if level != Level::L0 {
        return Ok(0);
    }
    let coords = unflatten_probe(index, base.grid_dimensions)?;
    let origin = coords.map(|coordinate| coordinate / 4 * 4);
    let end = [
        origin[0]
            .checked_add(4)
            .ok_or(ClusterShPayloadsError::SizeOverflow("L0 brick end"))?
            .min(base.grid_dimensions[0]),
        origin[1]
            .checked_add(4)
            .ok_or(ClusterShPayloadsError::SizeOverflow("L0 brick end"))?
            .min(base.grid_dimensions[1]),
        origin[2]
            .checked_add(4)
            .ok_or(ClusterShPayloadsError::SizeOverflow("L0 brick end"))?
            .min(base.grid_dimensions[2]),
    ];
    let mut ordinal = 0u32;
    for z in origin[2]..end[2] {
        for y in origin[1]..end[1] {
            for x in origin[0]..end[0] {
                let candidate = flatten([x, y, z], base.grid_dimensions)?;
                if candidate == index {
                    return Ok(ordinal);
                }
                let candidate_probe = base
                    .probes
                    .get(usize::try_from(candidate).map_err(|_| {
                        ClusterShPayloadsError::SizeOverflow("dense probe index exceeds usize")
                    })?)
                    .ok_or_else(|| {
                        ClusterShPayloadsError::SourceMismatch(format!(
                            "dense probe {candidate} is outside id 34 metadata"
                        ))
                    })?;
                if candidate_probe.validity != 0 {
                    ordinal =
                        ordinal
                            .checked_add(1)
                            .ok_or(ClusterShPayloadsError::SizeOverflow(
                                "L0 probe compact ordinal",
                            ))?;
                }
            }
        }
    }
    Err(ClusterShPayloadsError::InvalidData(format!(
        "dense probe {index} is absent from its L0 brick"
    )))
}

pub(super) fn dense_indices(
    directory: &ClusterDirectorySection,
    cluster_id: u32,
    section_id: u32,
) -> Result<Vec<u32>, ClusterShPayloadsError> {
    let cluster = directory.clusters.get(cluster_id as usize).ok_or_else(|| {
        ClusterShPayloadsError::SourceMismatch(format!("id 49 lacks cluster {cluster_id}"))
    })?;
    let resource_index = directory
        .resources
        .iter()
        .position(|resource| resource.section_id == section_id)
        .ok_or_else(|| {
            ClusterShPayloadsError::SourceMismatch(format!("id 49 lacks resource {section_id}"))
        })?;
    if directory.resources[resource_index].domain != ClusterResourceDomain::DenseProbe {
        return source_mismatch(format!("id 49 resource {section_id} is not dense"));
    }
    let mut indices = Vec::new();
    let begin = cluster.range_start as usize;
    let end = begin + cluster.range_count as usize;
    for range in directory.ranges.get(begin..end).ok_or_else(|| {
        ClusterShPayloadsError::SourceMismatch("id 49 cluster range slice is invalid".into())
    })? {
        if range.resource_index as usize != resource_index {
            continue;
        }
        if range.role != ClusterRangeRole::Dense {
            return source_mismatch(format!(
                "id 49 dense range for {section_id} has affinity role"
            ));
        }
        let range_end =
            range
                .start
                .checked_add(range.count)
                .ok_or(ClusterShPayloadsError::SizeOverflow(
                    "dense directory range",
                ))?;
        indices.extend(range.start..range_end);
    }
    Ok(indices)
}

pub(super) fn affinity_indices(
    directory: &ClusterDirectorySection,
    cluster_id: u32,
    section_id: u32,
) -> Result<Vec<u32>, ClusterShPayloadsError> {
    let cluster = directory.clusters.get(cluster_id as usize).ok_or_else(|| {
        ClusterShPayloadsError::SourceMismatch(format!("id 49 lacks cluster {cluster_id}"))
    })?;
    let Some(resource_index) = directory
        .resources
        .iter()
        .position(|resource| resource.section_id == section_id)
    else {
        return source_mismatch(format!("id 49 lacks resource {section_id}"));
    };
    if directory.resources[resource_index].domain != ClusterResourceDomain::AffinityCell {
        return source_mismatch(format!("id 49 resource {section_id} is not sparse"));
    }
    let mut rows = Vec::new();
    let begin = cluster.range_start as usize;
    let end = begin + cluster.range_count as usize;
    for range in directory.ranges.get(begin..end).ok_or_else(|| {
        ClusterShPayloadsError::SourceMismatch("id 49 cluster range slice is invalid".into())
    })? {
        if range.resource_index as usize != resource_index {
            continue;
        }
        let range_end =
            range
                .start
                .checked_add(range.count)
                .ok_or(ClusterShPayloadsError::SizeOverflow(
                    "sparse directory range",
                ))?;
        rows.extend(range.start..range_end);
    }
    Ok(rows)
}

pub(super) fn sparse_row_role(
    directory: &ClusterDirectorySection,
    cluster_id: u32,
    section_id: u32,
    row: u32,
) -> Result<ClusterRangeRole, ClusterShPayloadsError> {
    let cluster = directory.clusters.get(cluster_id as usize).ok_or_else(|| {
        ClusterShPayloadsError::SourceMismatch(format!("id 49 lacks cluster {cluster_id}"))
    })?;
    let resource_index = directory
        .resources
        .iter()
        .position(|resource| resource.section_id == section_id)
        .ok_or_else(|| {
            ClusterShPayloadsError::SourceMismatch(format!("id 49 lacks resource {section_id}"))
        })?;
    let begin = cluster.range_start as usize;
    let end = begin + cluster.range_count as usize;
    directory.ranges[begin..end]
        .iter()
        .find(|range| {
            range.resource_index as usize == resource_index
                && range.start <= row
                && row < range.start.saturating_add(range.count)
        })
        .map(|range| range.role)
        .ok_or_else(|| {
            ClusterShPayloadsError::SourceMismatch(format!(
                "id 49 cluster {cluster_id} does not cover source {section_id} row {row}"
            ))
        })
}

pub(super) fn affinity_dimensions(grid: [u32; 3]) -> Result<[u32; 3], ClusterShPayloadsError> {
    if grid == [0, 0, 0] {
        return Ok(grid);
    }
    if grid.contains(&0) {
        return source_mismatch(format!("id 34 has mixed-zero grid dimensions {grid:?}"));
    }
    Ok(grid.map(|dimension| dimension.div_ceil(4)))
}

pub(super) fn checked_product(dimensions: [u32; 3]) -> Result<u64, ClusterShPayloadsError> {
    u64::from(dimensions[0])
        .checked_mul(u64::from(dimensions[1]))
        .and_then(|value| value.checked_mul(u64::from(dimensions[2])))
        .ok_or(ClusterShPayloadsError::SizeOverflow("dimension product"))
}

pub(super) fn flatten(
    coords: [u32; 3],
    dimensions: [u32; 3],
) -> Result<u32, ClusterShPayloadsError> {
    if coords
        .into_iter()
        .zip(dimensions)
        .any(|(coord, dimension)| coord >= dimension)
    {
        return Err(ClusterShPayloadsError::RangeOutOfBounds(format!(
            "coordinates {coords:?} exceed dimensions {dimensions:?}"
        )));
    }
    let value = u64::from(coords[0])
        .checked_add(
            u64::from(dimensions[0])
                .checked_mul(u64::from(coords[1]))
                .ok_or(ClusterShPayloadsError::SizeOverflow("flatten y"))?,
        )
        .and_then(|value| {
            u64::from(dimensions[0])
                .checked_mul(u64::from(dimensions[1]))
                .and_then(|plane| plane.checked_mul(u64::from(coords[2])))
                .and_then(|z| value.checked_add(z))
        })
        .ok_or(ClusterShPayloadsError::SizeOverflow("flatten z"))?;
    u32::try_from(value).map_err(|_| ClusterShPayloadsError::SizeOverflow("flatten exceeds u32"))
}

pub(super) fn unflatten(
    index: u32,
    dimensions: [u32; 3],
) -> Result<[u32; 3], ClusterShPayloadsError> {
    let count = checked_product(dimensions)?;
    if u64::from(index) >= count || dimensions.contains(&0) {
        return Err(ClusterShPayloadsError::RangeOutOfBounds(format!(
            "index {index} exceeds dimensions {dimensions:?}"
        )));
    }
    let xy = dimensions[0]
        .checked_mul(dimensions[1])
        .ok_or(ClusterShPayloadsError::SizeOverflow("unflatten plane"))?;
    let z = index / xy;
    let remainder = index % xy;
    Ok([remainder % dimensions[0], remainder / dimensions[0], z])
}

pub(super) fn unflatten_probe(
    index: u32,
    dimensions: [u32; 3],
) -> Result<[u32; 3], ClusterShPayloadsError> {
    unflatten(index, dimensions)
}

pub(super) fn expected_internal_version(section_id: u32) -> Result<u32, ClusterShPayloadsError> {
    match section_id {
        id if id == SectionId::DeltaShVolumes as u32 => Ok(u32::from(DELTA_SH_VOLUMES_VERSION)),
        id if id == SectionId::OctahedralShVolume as u32 => Ok(SH_VOLUME_VERSION),
        id if id == SectionId::DirectShVolume as u32 => Ok(DIRECT_SH_VOLUME_VERSION),
        id if id == SectionId::DirectShDeltaVolumes as u32 => {
            Ok(u32::from(DIRECT_SH_DELTA_VOLUMES_VERSION))
        }
        id if id == SectionId::AnimatedDirectShDeltaVolumes as u32 => {
            Ok(u32::from(ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION))
        }
        _ => source_mismatch(format!("section {section_id} is not an id-50 source")),
    }
}

pub(super) fn checked_probe_slot_rank(rank: u32) -> Result<u32, ClusterShPayloadsError> {
    if rank > PROBE_INDIRECTION_MAX_SLOT {
        return Err(ClusterShPayloadsError::RangeOutOfBounds(format!(
            "chunk-local stored slot rank {rank} exceeds the {}-bit indirection field",
            u32::BITS - PROBE_INDIRECTION_SLOT_SHIFT
        )));
    }
    Ok(rank)
}

pub(super) fn is_streamed_section(section_id: u32) -> bool {
    matches!(
        section_id,
        id if id == SectionId::DeltaShVolumes as u32
            || id == SectionId::OctahedralShVolume as u32
            || id == SectionId::DirectShVolume as u32
            || id == SectionId::DirectShDeltaVolumes as u32
            || id == SectionId::AnimatedDirectShDeltaVolumes as u32
    )
}
