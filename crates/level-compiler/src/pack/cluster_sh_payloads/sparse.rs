//! Sparse affinity-row slicing for compiler-produced id-50 chunks.
//! See: context/plans/in-progress/sh-probe-streaming--cluster-residency/index.md

use postretro_level_format::SectionId;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::cluster_directory::{
    ClusterDirectorySection, ClusterRangeRole, ClusterResourceDomain,
};
use postretro_level_format::cluster_sh_payloads::CLUSTER_SH_LOGICAL_TILE_DIMENSION;
use postretro_level_format::delta_sh_volumes::{DeltaShVolumesSection, delta_probe_f16_stride};
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::sh_reconstruct::{Level, stored_delta_tiles};

use super::{EncodedSparse, push_u32};

pub(super) struct SparseSources<'a> {
    sources: Vec<SparseSource<'a>>,
}

impl<'a> SparseSources<'a> {
    pub(super) fn from_sections(
        delta: Option<&'a DeltaShVolumesSection>,
        direct: Option<&'a DirectShDeltaVolumesSection>,
        animated: Option<&'a AnimatedDirectShDeltaVolumesSection>,
    ) -> anyhow::Result<Self> {
        let mut sources = Vec::new();
        if let Some(section) = delta {
            sources.push(SparseSource::new(
                SectionId::DeltaShVolumes as u32,
                &section.valid_probe_masks,
                &section.cell_levels,
                &section.affinity_offsets,
                &section.affinity_lights,
                &section.delta_subblocks,
            )?);
        }
        if let Some(section) = direct {
            sources.push(SparseSource::new(
                SectionId::DirectShDeltaVolumes as u32,
                &section.valid_probe_masks,
                &section.cell_levels,
                &section.affinity_offsets,
                &section.affinity_lights,
                &section.delta_subblocks,
            )?);
        }
        if let Some(section) = animated {
            sources.push(SparseSource::new(
                SectionId::AnimatedDirectShDeltaVolumes as u32,
                &section.valid_probe_masks,
                &section.cell_levels,
                &section.affinity_offsets,
                &section.affinity_lights,
                &section.delta_subblocks,
            )?);
        }
        Ok(Self { sources })
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &SparseSource<'a>> {
        self.sources.iter()
    }

    pub(super) fn find(&self, section_id: u32) -> anyhow::Result<&SparseSource<'a>> {
        self.sources
            .iter()
            .find(|source| source.section_id == section_id)
            .ok_or_else(|| anyhow::anyhow!("id-50 lacks sparse source {section_id}"))
    }
}

pub(super) struct SparseSource<'a> {
    pub(super) section_id: u32,
    valid_probe_masks: &'a [u64],
    cell_levels: &'a [u8],
    affinity_offsets: &'a [u32],
    affinity_lights: &'a [u32],
    delta_subblocks: &'a [u16],
    entry_tile_offsets: Vec<usize>,
}

impl<'a> SparseSource<'a> {
    fn new(
        section_id: u32,
        valid_probe_masks: &'a [u64],
        cell_levels: &'a [u8],
        affinity_offsets: &'a [u32],
        affinity_lights: &'a [u32],
        delta_subblocks: &'a [u16],
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            affinity_offsets.len()
                == valid_probe_masks
                    .len()
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!(
                        "id-50 sparse affinity offset count overflow"
                    ))?,
            "id-50 sparse source {section_id} has mismatched CSR offsets"
        );
        anyhow::ensure!(
            cell_levels.len() == valid_probe_masks.len(),
            "id-50 sparse source {section_id} has mismatched cell levels"
        );
        let mut entry_tile_offsets = Vec::with_capacity(affinity_lights.len() + 1);
        let mut cursor = 0usize;
        entry_tile_offsets.push(cursor);
        for row in 0..valid_probe_masks.len() {
            let start = usize::try_from(affinity_offsets[row])?;
            let end = usize::try_from(affinity_offsets[row + 1])?;
            anyhow::ensure!(
                start <= end && end <= affinity_lights.len(),
                "id-50 sparse source {section_id} has invalid CSR range"
            );
            let tiles = stored_delta_tiles(
                Level::from_u8(cell_levels[row]).ok_or_else(|| {
                    anyhow::anyhow!("id-50 sparse source {section_id} has invalid level")
                })?,
                valid_probe_masks[row],
            )
            .checked_mul(delta_probe_f16_stride(CLUSTER_SH_LOGICAL_TILE_DIMENSION))
            .ok_or_else(|| {
                anyhow::anyhow!("id-50 sparse source {section_id} tile count overflow")
            })?;
            for _ in start..end {
                cursor = cursor.checked_add(tiles).ok_or_else(|| {
                    anyhow::anyhow!("id-50 sparse source {section_id} payload overflow")
                })?;
                entry_tile_offsets.push(cursor);
            }
        }
        anyhow::ensure!(
            entry_tile_offsets.len() == affinity_lights.len() + 1
                && cursor == delta_subblocks.len(),
            "id-50 sparse source {section_id} payload does not match CSR metadata"
        );
        Ok(Self {
            section_id,
            valid_probe_masks,
            cell_levels,
            affinity_offsets,
            affinity_lights,
            delta_subblocks,
            entry_tile_offsets,
        })
    }

    pub(super) fn entry_range(&self, row: u32) -> anyhow::Result<std::ops::Range<usize>> {
        let row = usize::try_from(row)?;
        let start = usize::try_from(
            *self
                .affinity_offsets
                .get(row)
                .ok_or_else(|| anyhow::anyhow!("id-50 sparse row exceeds CSR offsets"))?,
        )?;
        let end = usize::try_from(
            *self
                .affinity_offsets
                .get(row + 1)
                .ok_or_else(|| anyhow::anyhow!("id-50 sparse row lacks CSR end"))?,
        )?;
        Ok(start..end)
    }

    pub(super) fn entry_count(&self, row: u32) -> anyhow::Result<u32> {
        let range = self.entry_range(row)?;
        u32::try_from(range.end - range.start).map_err(Into::into)
    }

    pub(super) fn tile_count(&self, row: u32) -> anyhow::Result<u32> {
        let row = usize::try_from(row)?;
        let level = Level::from_u8(
            *self
                .cell_levels
                .get(row)
                .ok_or_else(|| anyhow::anyhow!("id-50 sparse row lacks level"))?,
        )
        .ok_or_else(|| anyhow::anyhow!("id-50 sparse row has invalid level"))?;
        u32::try_from(
            stored_delta_tiles(level, self.valid_probe_masks[row])
                .checked_mul(delta_probe_f16_stride(CLUSTER_SH_LOGICAL_TILE_DIMENSION))
                .ok_or_else(|| anyhow::anyhow!("id-50 sparse row tile count overflow"))?,
        )
        .map_err(Into::into)
    }

    fn tile_range(&self, row: u32) -> anyhow::Result<Vec<&'a [u16]>> {
        self.entry_range(row)?
            .map(|entry| {
                let range = self.entry_tile_offsets[entry]..self.entry_tile_offsets[entry + 1];
                self.delta_subblocks
                    .get(range)
                    .ok_or_else(|| anyhow::anyhow!("id-50 sparse entry tile range exceeds payload"))
            })
            .collect()
    }

    pub(super) fn counts(&self, rows: &[u32]) -> anyhow::Result<(usize, usize)> {
        let mut entries = 0usize;
        let mut tiles = 0usize;
        for row in rows {
            entries = entries
                .checked_add(usize::try_from(self.entry_count(*row)?)?)
                .ok_or_else(|| anyhow::anyhow!("id-50 sparse entry count overflow"))?;
            tiles = tiles
                .checked_add(
                    usize::try_from(self.tile_count(*row)?)?
                        .checked_mul(usize::try_from(self.entry_count(*row)?)?)
                        .ok_or_else(|| anyhow::anyhow!("id-50 sparse tile count overflow"))?,
                )
                .ok_or_else(|| anyhow::anyhow!("id-50 sparse tile count overflow"))?;
        }
        Ok((entries, tiles))
    }
}

pub(super) fn encode_sparse_block(
    directory: &ClusterDirectorySection,
    cluster_id: u32,
    source: &SparseSource<'_>,
    rows: &[u32],
) -> anyhow::Result<EncodedSparse> {
    let (entry_count, tile_f16_count) = source.counts(rows)?;
    let mut body = Vec::new();
    push_u32(&mut body, u32::try_from(rows.len())?);
    push_u32(&mut body, u32::try_from(entry_count)?);
    push_u32(&mut body, u32::try_from(tile_f16_count)?);
    push_u32(&mut body, 0);
    let mut entry_cursor = 0u32;
    for row in rows {
        let count = source.entry_count(*row)?;
        push_u32(&mut body, *row);
        push_u32(&mut body, entry_cursor);
        push_u32(&mut body, count);
        push_u32(
            &mut body,
            sparse_row_role(directory, cluster_id, source.section_id, *row)? as u32,
        );
        entry_cursor = entry_cursor
            .checked_add(count)
            .ok_or_else(|| anyhow::anyhow!("id-50 sparse entry cursor overflow"))?;
    }
    let mut tile_cursor = 0u32;
    for row in rows {
        let tile_count = source.tile_count(*row)?;
        for entry in source.entry_range(*row)? {
            push_u32(&mut body, source.affinity_lights[entry]);
            push_u32(&mut body, tile_cursor);
            push_u32(&mut body, tile_count);
            push_u32(&mut body, 0);
            tile_cursor = tile_cursor
                .checked_add(tile_count)
                .ok_or_else(|| anyhow::anyhow!("id-50 sparse tile cursor overflow"))?;
        }
    }
    for row in rows {
        for entry_tiles in source.tile_range(*row)? {
            for value in entry_tiles {
                body.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
    Ok(EncodedSparse {
        section_id: source.section_id,
        rows: rows.to_vec(),
        body,
    })
}

pub(super) fn affinity_indices(
    directory: &ClusterDirectorySection,
    cluster_id: u32,
    section_id: u32,
) -> anyhow::Result<Vec<u32>> {
    let cluster = directory
        .clusters
        .get(cluster_id as usize)
        .ok_or_else(|| anyhow::anyhow!("id-50 cluster {cluster_id} is outside id 49"))?;
    let resource_index = directory
        .resources
        .iter()
        .position(|resource| resource.section_id == section_id)
        .ok_or_else(|| anyhow::anyhow!("id 49 lacks sparse source {section_id}"))?;
    anyhow::ensure!(
        directory.resources[resource_index].domain == ClusterResourceDomain::AffinityCell,
        "id 49 source {section_id} does not use affinity-cell addressing"
    );
    let start = usize::try_from(cluster.range_start)?;
    let end = start
        .checked_add(usize::try_from(cluster.range_count)?)
        .ok_or_else(|| anyhow::anyhow!("id-50 sparse range slice overflow"))?;
    let mut result = Vec::new();
    for range in directory
        .ranges
        .get(start..end)
        .ok_or_else(|| anyhow::anyhow!("id-50 sparse range slice exceeds id 49"))?
    {
        if range.resource_index as usize != resource_index {
            continue;
        }
        let range_end = range
            .start
            .checked_add(range.count)
            .ok_or_else(|| anyhow::anyhow!("id-50 sparse range endpoint overflow"))?;
        result.extend(range.start..range_end);
    }
    Ok(result)
}

pub(super) fn sparse_row_role(
    directory: &ClusterDirectorySection,
    cluster_id: u32,
    section_id: u32,
    row: u32,
) -> anyhow::Result<ClusterRangeRole> {
    let cluster = directory
        .clusters
        .get(cluster_id as usize)
        .ok_or_else(|| anyhow::anyhow!("id-50 cluster {cluster_id} is outside id 49"))?;
    let resource_index = directory
        .resources
        .iter()
        .position(|resource| resource.section_id == section_id)
        .ok_or_else(|| anyhow::anyhow!("id 49 lacks sparse source {section_id}"))?;
    let start = usize::try_from(cluster.range_start)?;
    let end = start
        .checked_add(usize::try_from(cluster.range_count)?)
        .ok_or_else(|| anyhow::anyhow!("id-50 sparse role range overflow"))?;
    directory
        .ranges
        .get(start..end)
        .ok_or_else(|| anyhow::anyhow!("id-50 sparse role range exceeds id 49"))?
        .iter()
        .find(|range| {
            range.resource_index as usize == resource_index
                && range.start <= row
                && row < range.start.saturating_add(range.count)
        })
        .map(|range| range.role)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "id-50 cluster {cluster_id} does not cover row {row} of source {section_id}"
            )
        })
}
