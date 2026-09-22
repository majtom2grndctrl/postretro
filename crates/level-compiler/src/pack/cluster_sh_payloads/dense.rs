//! Dense-node closure, ownership, and probe-patch assembly for id-50 chunks.
//! See: context/plans/in-progress/sh-probe-streaming--cluster-residency/index.md

use std::collections::{BTreeMap, BTreeSet};

use postretro_level_format::SectionId;
use postretro_level_format::cluster_directory::{
    ClusterDirectorySection, ClusterRangeRole, ClusterResourceDomain,
};
use postretro_level_format::cluster_sh_payloads::{
    CLUSTER_SH_LOGICAL_TILE_DIMENSION, ClusterShPayloadsBaseMetadata,
    ClusterShPayloadsSourceMetadata, ClusterShPayloadsValidationInputs,
};
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use postretro_level_format::sh_reconstruct::{Level, StoredBrickPrefixSum};
use postretro_level_format::sh_volume::{
    OctahedralShProbe, OctahedralShVolumeSection, validate_probe_metadata,
};

use super::atlas::DenseAtlasGeometry;
use super::sparse::{SparseSources, affinity_indices, encode_sparse_block};
use super::{BLOCK_KIND_PROBE_PATCHES, EncodedBlock, EncodedSparse, encode_chunk_wire, push_u32};

pub(super) const PROBE_INDIRECTION_LEVEL_MASK: u32 = 0x0000_0003;
const PROBE_INDIRECTION_VALID_BIT: u32 = 0x0000_0004;
pub(super) const PROBE_INDIRECTION_SCALE_SHIFT: u32 = 3;
pub(super) const PROBE_INDIRECTION_SLOT_SHIFT: u32 = 5;

#[derive(Clone)]
pub(super) struct DenseNode {
    pub(super) index: usize,
    pub(super) global_base: u32,
    pub(super) local_base: u32,
    pub(super) stored_tile_count: u32,
}

pub(super) struct EncodedChunk {
    pub(super) bytes: Vec<u8>,
    pub(super) decoded_bytes: u64,
    pub(super) requested_resident_bytes: u64,
    pub(super) stored_tile_count: u32,
    pub(super) dense_patch_count: u32,
    pub(super) affinity_patch_count: u32,
}

pub(super) struct ChunkPlan<'a> {
    inputs: ClusterShPayloadsValidationInputs<'a>,
    prefix: StoredBrickPrefixSum,
    pub(super) affinity_dimensions: [u32; 3],
    dense_nodes: Vec<BTreeSet<usize>>,
    dense_node_owner: BTreeMap<usize, u32>,
    sparse_sources: SparseSources<'a>,
}

impl<'a> ChunkPlan<'a> {
    pub(super) fn new(
        inputs: ClusterShPayloadsValidationInputs<'a>,
        sparse_sources: SparseSources<'a>,
    ) -> anyhow::Result<Self> {
        let affinity_dimensions = inputs
            .base
            .grid_dimensions
            .map(|dimension| dimension.div_ceil(4));
        let prefix = validate_probe_metadata(inputs.base.grid_dimensions, inputs.base.probes)
            .map_err(|error| {
                anyhow::anyhow!("id 34 metadata cannot form an id-50 prefix: {error}")
            })?;
        anyhow::ensure!(
            prefix.affinity_dimensions == affinity_dimensions,
            "id-50 affinity dimensions disagree with id-34 stored-node prefix"
        );
        let mut dense_nodes = Vec::with_capacity(inputs.directory.clusters.len());
        for cluster_id in 0..u32::try_from(inputs.directory.clusters.len())? {
            let indices = dense_indices(inputs.directory, cluster_id)?;
            let mut nodes = BTreeSet::new();
            for index in indices {
                if inputs.base.probes[index as usize].validity != 0 {
                    nodes.insert(node_for_probe(index, inputs.base, affinity_dimensions)?);
                }
            }
            dense_nodes.push(nodes);
        }
        let mut dense_node_owner = BTreeMap::new();
        for (cluster_id, nodes) in dense_nodes.iter().enumerate() {
            for node in nodes {
                dense_node_owner
                    .entry(*node)
                    .or_insert(u32::try_from(cluster_id)?);
            }
        }
        Ok(Self {
            inputs,
            prefix,
            affinity_dimensions,
            dense_nodes,
            dense_node_owner,
            sparse_sources,
        })
    }

    pub(super) fn encode_chunk(
        &self,
        cluster_id: u32,
        indirect_source: &OctahedralShVolumeSection,
        direct_source: Option<&DirectShVolumeSection>,
    ) -> anyhow::Result<EncodedChunk> {
        let dense_indices = dense_indices(self.inputs.directory, cluster_id)?
            .into_iter()
            .filter(|index| self.inputs.base.probes[*index as usize].validity != 0)
            .collect::<Vec<_>>();
        let nodes = self
            .dense_nodes
            .get(cluster_id as usize)
            .ok_or_else(|| anyhow::anyhow!("cluster {cluster_id} exceeds dense-node plan"))?;
        let dense_nodes = self.local_dense_nodes(nodes)?;
        let stored_tile_count = dense_nodes.iter().try_fold(0u32, |total, node| {
            total
                .checked_add(node.stored_tile_count)
                .ok_or_else(|| anyhow::anyhow!("id-50 stored tile count overflow"))
        })?;
        let dense_patch_count = u32::try_from(dense_indices.len())?;

        let mut sparse_chunks = Vec::new();
        for source in self.sparse_sources.iter() {
            let rows = affinity_indices(self.inputs.directory, cluster_id, source.section_id)?;
            if !rows.is_empty() {
                sparse_chunks.push(encode_sparse_block(
                    self.inputs.directory,
                    cluster_id,
                    source,
                    &rows,
                )?);
            }
        }
        let affinity_patch_count = sparse_chunks.iter().try_fold(0u32, |total, sparse| {
            total
                .checked_add(u32::try_from(sparse.rows.len())?)
                .ok_or_else(|| anyhow::anyhow!("id-50 affinity patch count overflow"))
        })?;

        let mut blocks = Vec::new();
        if !dense_nodes.is_empty() {
            blocks.push(EncodedBlock::probe_patches(
                &dense_indices,
                self.inputs.base.probes,
                self.inputs.base.grid_dimensions,
                self.affinity_dimensions,
                &dense_nodes,
            )?);
            blocks.push(EncodedBlock::isolated_atlas(
                SectionId::OctahedralShVolume as u32,
                self.inputs.base.irradiance_format,
                stored_tile_count,
                indirect_source.compact_atlas.as_slice(),
                DenseAtlasGeometry::from_octahedral(indirect_source),
                &dense_nodes,
            )?);
            if let Some(direct_source) = direct_source {
                let direct_format =
                    dense_format(self.inputs.sources, SectionId::DirectShVolume as u32)?;
                blocks.push(EncodedBlock::isolated_atlas(
                    SectionId::DirectShVolume as u32,
                    direct_format,
                    stored_tile_count,
                    direct_source.atlas.as_slice(),
                    DenseAtlasGeometry::from_direct(direct_source),
                    &dense_nodes,
                )?);
            }
        }
        blocks.extend(sparse_chunks.into_iter().map(EncodedSparse::into_block));
        blocks.sort_by_key(|block| (block.section_id, block.kind));
        let decoded_bytes = blocks.iter().try_fold(0u64, |total, block| {
            total
                .checked_add(u64::try_from(block.body.len())?)
                .ok_or_else(|| anyhow::anyhow!("id-50 decoded block byte count overflow"))
        })?;
        let requested_resident_bytes = self.requested_resident_bytes(cluster_id, nodes)?;
        let bytes = encode_chunk_wire(cluster_id, &blocks)?;
        Ok(EncodedChunk {
            bytes,
            decoded_bytes,
            requested_resident_bytes,
            stored_tile_count,
            dense_patch_count,
            affinity_patch_count,
        })
    }

    fn local_dense_nodes(&self, nodes: &BTreeSet<usize>) -> anyhow::Result<Vec<DenseNode>> {
        let mut local_base = 0u32;
        let mut result = Vec::with_capacity(nodes.len());
        for index in nodes {
            let range = self.prefix.bricks.get(*index).ok_or_else(|| {
                anyhow::anyhow!("id-50 node {index} is outside id-34 stored-node prefix")
            })?;
            anyhow::ensure!(
                range.stored_tile_count != 0,
                "id-50 node {index} has no stored tiles"
            );
            result.push(DenseNode {
                index: *index,
                global_base: range.base_slot,
                local_base,
                stored_tile_count: range.stored_tile_count,
            });
            local_base = local_base
                .checked_add(range.stored_tile_count)
                .ok_or_else(|| anyhow::anyhow!("id-50 local stored-slot rank overflow"))?;
        }
        Ok(result)
    }

    fn requested_resident_bytes(
        &self,
        cluster_id: u32,
        nodes: &BTreeSet<usize>,
    ) -> anyhow::Result<u64> {
        let owned_tiles = nodes.iter().try_fold(0u64, |total, node| {
            if self.dense_node_owner[node] != cluster_id {
                return Ok(total);
            }
            total
                .checked_add(u64::from(self.prefix.bricks[*node].stored_tile_count))
                .ok_or_else(|| anyhow::anyhow!("id-50 owned dense tile count overflow"))
        })?;
        let mut total = 0u64;
        for source in self.inputs.sources {
            match source {
                ClusterShPayloadsSourceMetadata::Dense {
                    irradiance_format, ..
                } => {
                    total = total
                        .checked_add(
                            owned_tiles
                                .checked_mul(isolated_tile_bytes(*irradiance_format)?)
                                .ok_or_else(|| {
                                    anyhow::anyhow!("id-50 dense resident bytes overflow")
                                })?,
                        )
                        .ok_or_else(|| anyhow::anyhow!("id-50 resident bytes overflow"))?;
                }
                ClusterShPayloadsSourceMetadata::Sparse { section_id, .. } => {
                    let source = self.sparse_sources.find(*section_id)?;
                    for row in affinity_indices(self.inputs.directory, cluster_id, *section_id)? {
                        if super::sparse::sparse_row_role(
                            self.inputs.directory,
                            cluster_id,
                            *section_id,
                            row,
                        )? != ClusterRangeRole::Owned
                        {
                            continue;
                        }
                        let entry_count = u64::from(source.entry_count(row)?);
                        let tile_bytes = u64::from(source.tile_count(row)?)
                            .checked_mul(entry_count)
                            .and_then(|count| count.checked_mul(2))
                            .ok_or_else(|| {
                                anyhow::anyhow!("id-50 sparse resident tile bytes overflow")
                            })?;
                        total = total
                            .checked_add(
                                entry_count
                                    .checked_mul(16)
                                    .and_then(|bytes| bytes.checked_add(tile_bytes))
                                    .ok_or_else(|| {
                                        anyhow::anyhow!("id-50 sparse resident bytes overflow")
                                    })?,
                            )
                            .ok_or_else(|| anyhow::anyhow!("id-50 resident bytes overflow"))?;
                    }
                }
            }
        }
        Ok(total)
    }
}

impl EncodedBlock {
    fn probe_patches(
        indices: &[u32],
        probes: &[OctahedralShProbe],
        grid_dimensions: [u32; 3],
        affinity_dimensions: [u32; 3],
        nodes: &[DenseNode],
    ) -> anyhow::Result<Self> {
        let node_ranks = nodes
            .iter()
            .map(|node| (node.index, node.local_base))
            .collect::<BTreeMap<_, _>>();
        let mut body = Vec::with_capacity(indices.len() * 16);
        for index in indices {
            let probe = probes
                .get(*index as usize)
                .ok_or_else(|| anyhow::anyhow!("id-50 dense probe {index} is out of range"))?;
            let node = node_for_probe(
                *index,
                ClusterShPayloadsBaseMetadata {
                    grid_dimensions,
                    tile_dimension: CLUSTER_SH_LOGICAL_TILE_DIMENSION,
                    tile_border: 1,
                    irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
                    probes,
                },
                affinity_dimensions,
            )?;
            let rank = node_ranks[&node]
                .checked_add(local_probe_slot_offset(*index, probes, grid_dimensions)?)
                .ok_or_else(|| anyhow::anyhow!("id-50 probe slot rank overflow"))?;
            push_u32(&mut body, *index);
            push_u32(
                &mut body,
                PROBE_INDIRECTION_VALID_BIT
                    | (u32::from(probe.density_level) & PROBE_INDIRECTION_LEVEL_MASK)
                    | (u32::from(probe.node_scale) << PROBE_INDIRECTION_SCALE_SHIFT)
                    | (rank << PROBE_INDIRECTION_SLOT_SHIFT),
            );
            body.extend_from_slice(&probe.mean_distance.to_le_bytes());
            body.extend_from_slice(&probe.mean_sq_distance.to_le_bytes());
            push_u32(&mut body, 0);
        }
        Ok(Self {
            section_id: SectionId::OctahedralShVolume as u32,
            kind: BLOCK_KIND_PROBE_PATCHES,
            element_count: u32::try_from(indices.len())?,
            body,
        })
    }
}

fn dense_indices(directory: &ClusterDirectorySection, cluster_id: u32) -> anyhow::Result<Vec<u32>> {
    let cluster = directory
        .clusters
        .get(cluster_id as usize)
        .ok_or_else(|| anyhow::anyhow!("id-50 cluster {cluster_id} is outside id 49"))?;
    let resource_index = directory
        .resources
        .iter()
        .position(|resource| resource.section_id == SectionId::OctahedralShVolume as u32)
        .ok_or_else(|| anyhow::anyhow!("id 49 lacks id-34 dense resource"))?;
    anyhow::ensure!(
        directory.resources[resource_index].domain == ClusterResourceDomain::DenseProbe,
        "id 49 id-34 resource does not use dense probe addressing"
    );
    let start = usize::try_from(cluster.range_start)?;
    let end = start
        .checked_add(usize::try_from(cluster.range_count)?)
        .ok_or_else(|| anyhow::anyhow!("id-50 dense range slice overflow"))?;
    let mut result = Vec::new();
    for range in directory
        .ranges
        .get(start..end)
        .ok_or_else(|| anyhow::anyhow!("id-50 dense range slice exceeds id 49"))?
    {
        if range.resource_index as usize != resource_index {
            continue;
        }
        anyhow::ensure!(
            range.role == ClusterRangeRole::Dense,
            "id-50 id-34 range has an affinity role"
        );
        let range_end = range
            .start
            .checked_add(range.count)
            .ok_or_else(|| anyhow::anyhow!("id-50 dense range endpoint overflow"))?;
        result.extend(range.start..range_end);
    }
    Ok(result)
}

fn node_for_probe(
    index: u32,
    base: ClusterShPayloadsBaseMetadata<'_>,
    affinity_dimensions: [u32; 3],
) -> anyhow::Result<usize> {
    let probe = base
        .probes
        .get(index as usize)
        .ok_or_else(|| anyhow::anyhow!("id-50 dense probe {index} exceeds id-34 metadata"))?;
    let coords = unflatten(index, base.grid_dimensions)?;
    let brick = coords.map(|coordinate| coordinate / 4);
    let edge = 1u32
        .checked_shl(u32::from(probe.node_scale))
        .ok_or_else(|| anyhow::anyhow!("id-50 node scale overflow"))?;
    let origin = [
        brick[0] / edge * edge,
        brick[1] / edge * edge,
        brick[2] / edge * edge,
    ];
    Ok(usize::try_from(flatten(origin, affinity_dimensions)?)?)
}

fn local_probe_slot_offset(
    index: u32,
    probes: &[OctahedralShProbe],
    dimensions: [u32; 3],
) -> anyhow::Result<u32> {
    let probe = probes
        .get(index as usize)
        .ok_or_else(|| anyhow::anyhow!("id-50 dense probe {index} exceeds id-34 metadata"))?;
    if probe.density_level != Level::L0.to_u8() {
        return Ok(0);
    }
    let coords = unflatten(index, dimensions)?;
    let origin = coords.map(|coordinate| coordinate / 4 * 4);
    let end = [
        origin[0]
            .checked_add(4)
            .ok_or_else(|| anyhow::anyhow!("id-50 L0 brick endpoint overflow"))?
            .min(dimensions[0]),
        origin[1]
            .checked_add(4)
            .ok_or_else(|| anyhow::anyhow!("id-50 L0 brick endpoint overflow"))?
            .min(dimensions[1]),
        origin[2]
            .checked_add(4)
            .ok_or_else(|| anyhow::anyhow!("id-50 L0 brick endpoint overflow"))?
            .min(dimensions[2]),
    ];
    let mut rank = 0u32;
    for z in origin[2]..end[2] {
        for y in origin[1]..end[1] {
            for x in origin[0]..end[0] {
                let candidate = flatten([x, y, z], dimensions)?;
                if candidate == index {
                    return Ok(rank);
                }
                if probes[candidate as usize].validity != 0 {
                    rank = rank
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("id-50 L0 compact rank overflow"))?;
                }
            }
        }
    }
    anyhow::bail!("id-50 dense probe {index} is absent from its L0 brick")
}

fn dense_format(
    sources: &[ClusterShPayloadsSourceMetadata<'_>],
    section_id: u32,
) -> anyhow::Result<u32> {
    let source = sources
        .iter()
        .find(|source| source.section_id() == section_id)
        .ok_or_else(|| anyhow::anyhow!("id-50 lacks dense source {section_id}"))?;
    match source {
        ClusterShPayloadsSourceMetadata::Dense {
            irradiance_format, ..
        } => Ok(*irradiance_format),
        ClusterShPayloadsSourceMetadata::Sparse { .. } => {
            anyhow::bail!("id-50 source {section_id} is not dense")
        }
    }
}

fn isolated_tile_bytes(format: u32) -> anyhow::Result<u64> {
    match format {
        IRRADIANCE_FORMAT_BC6H => Ok(64),
        IRRADIANCE_FORMAT_RGBA16F => Ok(512),
        _ => anyhow::bail!("id-50 dense source uses unknown irradiance format {format}"),
    }
}

fn flatten(coords: [u32; 3], dimensions: [u32; 3]) -> anyhow::Result<u32> {
    anyhow::ensure!(
        coords
            .into_iter()
            .zip(dimensions)
            .all(|(coordinate, dimension)| coordinate < dimension),
        "id-50 coordinates {coords:?} exceed dimensions {dimensions:?}"
    );
    u32::try_from(
        u64::from(coords[0])
            .checked_add(
                u64::from(dimensions[0])
                    .checked_mul(u64::from(coords[1]))
                    .ok_or_else(|| anyhow::anyhow!("id-50 flatten y overflow"))?,
            )
            .and_then(|value| {
                u64::from(dimensions[0])
                    .checked_mul(u64::from(dimensions[1]))
                    .and_then(|plane| plane.checked_mul(u64::from(coords[2])))
                    .and_then(|z| value.checked_add(z))
            })
            .ok_or_else(|| anyhow::anyhow!("id-50 flatten z overflow"))?,
    )
    .map_err(Into::into)
}

fn unflatten(index: u32, dimensions: [u32; 3]) -> anyhow::Result<[u32; 3]> {
    let count = u64::from(dimensions[0])
        .checked_mul(u64::from(dimensions[1]))
        .and_then(|value| value.checked_mul(u64::from(dimensions[2])))
        .ok_or_else(|| anyhow::anyhow!("id-50 unflatten dimension product overflow"))?;
    anyhow::ensure!(
        u64::from(index) < count && !dimensions.contains(&0),
        "id-50 index {index} exceeds dimensions {dimensions:?}"
    );
    let xy = dimensions[0]
        .checked_mul(dimensions[1])
        .ok_or_else(|| anyhow::anyhow!("id-50 unflatten plane overflow"))?;
    let z = index / xy;
    let remainder = index % xy;
    Ok([remainder % dimensions[0], remainder / dimensions[0], z])
}
