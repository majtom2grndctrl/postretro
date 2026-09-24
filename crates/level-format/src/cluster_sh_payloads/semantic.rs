//! Metadata-derived closure, ownership, and index validation for id 50.
//! See: context/lib/build_pipeline.md §PRL section IDs.

use super::*;

#[derive(Debug, Clone)]
pub(super) struct ExpectedBlock {
    pub(super) section_id: u32,
    pub(super) kind: u32,
    pub(super) element_count: u32,
    pub(super) byte_len: u64,
    pub(super) irradiance_format: Option<u32>,
}

#[derive(Debug, Clone)]
pub(super) struct ExpectedProbePatch {
    pub(super) dense_index: u32,
    pub(super) word: u32,
    pub(super) mean_distance: u16,
    pub(super) mean_sq_distance: u16,
}

#[derive(Debug, Clone)]
pub(super) struct ExpectedSparseRow {
    pub(super) global_affinity_index: u32,
    pub(super) first_entry: u32,
    pub(super) entry_count: u32,
    pub(super) role: u32,
}

#[derive(Debug, Clone)]
pub(super) struct ExpectedSparseEntry {
    pub(super) light: u32,
    pub(super) first_tile_f16: u32,
    pub(super) tile_f16_count: u32,
}

#[derive(Debug, Clone)]
pub(super) struct ExpectedSparseBlock {
    pub(super) rows: Vec<ExpectedSparseRow>,
    pub(super) entries: Vec<ExpectedSparseEntry>,
    pub(super) tile_f16_count: u32,
}

#[derive(Debug, Clone)]
pub(super) struct ExpectedChunk {
    pub(super) stored_tile_count: u32,
    pub(super) dense_patch_count: u32,
    pub(super) affinity_patch_count: u32,
    pub(super) decoded_bytes: u64,
    pub(super) requested_resident_bytes: u64,
    pub(super) payload_len: u64,
    pub(super) blocks: Vec<ExpectedBlock>,
    pub(super) probe_patches: Vec<ExpectedProbePatch>,
    pub(super) sparse_blocks: BTreeMap<u32, ExpectedSparseBlock>,
}

pub(super) struct ValidationPlan<'a> {
    inputs: ClusterShPayloadsValidationInputs<'a>,
    prefix: StoredBrickPrefixSum,
    dense_nodes_per_cluster: Vec<BTreeSet<usize>>,
    node_owner: BTreeMap<usize, u32>,
}

impl<'a> ValidationPlan<'a> {
    pub(super) fn new(
        inputs: ClusterShPayloadsValidationInputs<'a>,
    ) -> Result<Self, ClusterShPayloadsError> {
        let affinity_dimensions = affinity_dimensions(inputs.base.grid_dimensions)?;
        let prefix = stored_prefix(inputs.base, affinity_dimensions)?;
        let mut dense_nodes_per_cluster = Vec::with_capacity(inputs.directory.clusters.len());
        for cluster_id in 0..inputs.directory.clusters.len() as u32 {
            let indices = dense_indices(
                inputs.directory,
                cluster_id,
                SectionId::OctahedralShVolume as u32,
            )?;
            validate_dense_node_closure(&indices, inputs.base, affinity_dimensions)?;
            let mut nodes = BTreeSet::new();
            for index in indices {
                if let Some(node) = node_for_probe(index, inputs.base, affinity_dimensions)? {
                    nodes.insert(node);
                }
            }
            dense_nodes_per_cluster.push(nodes);
        }
        let mut node_owner = BTreeMap::new();
        for (cluster_id, nodes) in dense_nodes_per_cluster.iter().enumerate() {
            for &node in nodes {
                node_owner.entry(node).or_insert(cluster_id as u32);
            }
        }
        Ok(Self {
            inputs,
            prefix,
            dense_nodes_per_cluster,
            node_owner,
        })
    }

    pub(super) fn expected_chunk(
        &self,
        cluster_id: u32,
    ) -> Result<ExpectedChunk, ClusterShPayloadsError> {
        let dense_indices = dense_indices(
            self.inputs.directory,
            cluster_id,
            SectionId::OctahedralShVolume as u32,
        )?;
        let dense_indices: Vec<u32> = dense_indices
            .into_iter()
            .filter(|index| self.inputs.base.probes[*index as usize].validity != 0)
            .collect();
        let nodes = &self.dense_nodes_per_cluster[cluster_id as usize];
        let mut node_base_ranks = BTreeMap::new();
        let mut stored_tile_count = 0u32;
        for &node in nodes {
            let range = self.prefix.bricks[node];
            node_base_ranks.insert(node, stored_tile_count);
            let next_stored_tile_count = stored_tile_count
                .checked_add(range.stored_tile_count)
                .ok_or(ClusterShPayloadsError::SizeOverflow(
                    "cluster stored tile count",
                ))?;
            let last_slot = next_stored_tile_count.checked_sub(1).ok_or_else(|| {
                ClusterShPayloadsError::InvalidData("stored node has zero tiles".into())
            })?;
            checked_probe_slot_rank(last_slot)?;
            stored_tile_count = next_stored_tile_count;
        }
        let dense_patch_count = u32::try_from(dense_indices.len())
            .map_err(|_| ClusterShPayloadsError::SizeOverflow("dense patch count"))?;
        let mut probe_patches = Vec::with_capacity(dense_indices.len());
        for &dense_index in &dense_indices {
            let probe = &self.inputs.base.probes[dense_index as usize];
            let node = node_for_probe(
                dense_index,
                self.inputs.base,
                affinity_dimensions(self.inputs.base.grid_dimensions)?,
            )?
            .ok_or_else(|| {
                ClusterShPayloadsError::InvalidData("valid probe has no stored node".into())
            })?;
            let rank = node_base_ranks[&node]
                .checked_add(local_probe_slot_offset(dense_index, self.inputs.base)?)
                .ok_or(ClusterShPayloadsError::SizeOverflow(
                    "chunk-local probe slot rank",
                ))?;
            checked_probe_slot_rank(rank)?;
            probe_patches.push(ExpectedProbePatch {
                dense_index,
                word: PROBE_INDIRECTION_VALID_BIT
                    | (u32::from(probe.density_level) & PROBE_INDIRECTION_LEVEL_MASK)
                    | (u32::from(probe.node_scale) << PROBE_INDIRECTION_SCALE_SHIFT)
                    | (rank << PROBE_INDIRECTION_SLOT_SHIFT),
                mean_distance: probe.mean_distance,
                mean_sq_distance: probe.mean_sq_distance,
            });
        }

        let mut sparse_rows = BTreeMap::new();
        let mut sparse_blocks = BTreeMap::new();
        let mut affinity_patch_count = 0u32;
        for source in self.inputs.sources {
            if source.kind() != ClusterShPayloadsSourceKind::SparseAffinity {
                continue;
            }
            let rows = affinity_indices(self.inputs.directory, cluster_id, source.section_id())?;
            affinity_patch_count = affinity_patch_count
                .checked_add(
                    u32::try_from(rows.len())
                        .map_err(|_| ClusterShPayloadsError::SizeOverflow("affinity row count"))?,
                )
                .ok_or(ClusterShPayloadsError::SizeOverflow("affinity patch count"))?;
            let ClusterShPayloadsSourceMetadata::Sparse {
                section_id,
                valid_probe_masks,
                cell_levels,
                affinity_offsets,
                affinity_lights,
                ..
            } = source
            else {
                unreachable!("sparse source kind has sparse metadata")
            };
            let mut expected_rows = Vec::with_capacity(rows.len());
            let mut expected_entries = Vec::new();
            let mut entry_cursor = 0u32;
            let mut tile_cursor = 0u32;
            for &row in &rows {
                let start = affinity_offsets[row as usize];
                let end = affinity_offsets[row as usize + 1];
                let entry_count = end - start;
                expected_rows.push(ExpectedSparseRow {
                    global_affinity_index: row,
                    first_entry: entry_cursor,
                    entry_count,
                    role: sparse_row_role(self.inputs.directory, cluster_id, *section_id, row)?
                        as u32,
                });
                let level = Level::from_u8(cell_levels[row as usize]).ok_or_else(|| {
                    ClusterShPayloadsError::SourceMismatch(format!(
                        "source {section_id} row {row} has invalid level"
                    ))
                })?;
                let tile_f16_count = u32::try_from(
                    stored_delta_tiles(level, valid_probe_masks[row as usize])
                        .checked_mul(delta_probe_f16_stride(CLUSTER_SH_LOGICAL_TILE_DIMENSION))
                        .ok_or(ClusterShPayloadsError::SizeOverflow(
                            "sparse entry tile count",
                        ))?,
                )
                .map_err(|_| {
                    ClusterShPayloadsError::SizeOverflow(
                        "sparse entry tile count exceeds u32 wire field",
                    )
                })?;
                for source_entry in start..end {
                    expected_entries.push(ExpectedSparseEntry {
                        light: affinity_lights[source_entry as usize],
                        first_tile_f16: tile_cursor,
                        tile_f16_count,
                    });
                    tile_cursor = tile_cursor
                        .checked_add(tile_f16_count)
                        .ok_or(ClusterShPayloadsError::SizeOverflow("sparse tile cursor"))?;
                }
                entry_cursor = entry_cursor
                    .checked_add(entry_count)
                    .ok_or(ClusterShPayloadsError::SizeOverflow("sparse entry cursor"))?;
            }
            sparse_blocks.insert(
                *section_id,
                ExpectedSparseBlock {
                    rows: expected_rows,
                    entries: expected_entries,
                    tile_f16_count: tile_cursor,
                },
            );
            sparse_rows.insert(source.section_id(), rows);
        }

        let mut blocks = Vec::new();
        if !nodes.is_empty() {
            blocks.push(ExpectedBlock {
                section_id: SectionId::OctahedralShVolume as u32,
                kind: BLOCK_KIND_PROBE_PATCHES,
                element_count: dense_patch_count,
                byte_len: u64::from(dense_patch_count)
                    .checked_mul(16)
                    .ok_or(ClusterShPayloadsError::SizeOverflow("probe patch bytes"))?,
                irradiance_format: None,
            });
            for source in self.inputs.sources {
                if source.kind() != ClusterShPayloadsSourceKind::DenseBaseAtlas {
                    continue;
                }
                let byte_len = isolated_atlas_block_len(stored_tile_count, dense_format(*source)?)?;
                blocks.push(ExpectedBlock {
                    section_id: source.section_id(),
                    kind: BLOCK_KIND_ISOLATED_ATLAS,
                    element_count: stored_tile_count,
                    byte_len,
                    irradiance_format: Some(dense_format(*source)?),
                });
            }
        }
        for source in self.inputs.sources {
            let ClusterShPayloadsSourceMetadata::Sparse {
                section_id,
                valid_probe_masks,
                cell_levels,
                affinity_offsets,
                ..
            } = source
            else {
                continue;
            };
            let rows = sparse_rows
                .get(section_id)
                .expect("all sparse rows recorded");
            if rows.is_empty() {
                continue;
            }
            let (entry_count, tile_f16_count) = sparse_counts(
                rows,
                valid_probe_masks,
                cell_levels,
                affinity_offsets,
                *section_id,
            )?;
            u32::try_from(entry_count).map_err(|_| {
                ClusterShPayloadsError::SizeOverflow("sparse entry count exceeds u32 wire field")
            })?;
            u32::try_from(tile_f16_count).map_err(|_| {
                ClusterShPayloadsError::SizeOverflow("sparse f16 count exceeds u32 wire field")
            })?;
            blocks.push(ExpectedBlock {
                section_id: *section_id,
                kind: BLOCK_KIND_SPARSE_ROWS,
                element_count: u32::try_from(rows.len())
                    .map_err(|_| ClusterShPayloadsError::SizeOverflow("sparse row count"))?,
                byte_len: sparse_block_len(rows.len(), entry_count, tile_f16_count)?,
                irradiance_format: None,
            });
        }
        blocks.sort_by_key(|block| (block.section_id, block.kind));
        let decoded_bytes = blocks.iter().try_fold(0u64, |total, block| {
            total
                .checked_add(block.byte_len)
                .ok_or(ClusterShPayloadsError::SizeOverflow("decoded block bytes"))
        })?;
        let payload_len = if blocks.is_empty() {
            0
        } else {
            (CLUSTER_SH_CHUNK_HEADER_SIZE as u64)
                .checked_add(
                    (blocks.len() as u64)
                        .checked_mul(CLUSTER_SH_BLOCK_RECORD_SIZE as u64)
                        .ok_or(ClusterShPayloadsError::SizeOverflow("block table bytes"))?,
                )
                .and_then(|value| value.checked_add(decoded_bytes))
                .ok_or(ClusterShPayloadsError::SizeOverflow("chunk payload bytes"))?
        };
        let requested_resident_bytes =
            self.requested_resident_bytes(cluster_id, nodes, &sparse_rows)?;
        Ok(ExpectedChunk {
            stored_tile_count,
            dense_patch_count,
            affinity_patch_count,
            decoded_bytes,
            requested_resident_bytes,
            payload_len,
            blocks,
            probe_patches,
            sparse_blocks,
        })
    }

    fn requested_resident_bytes(
        &self,
        cluster_id: u32,
        nodes: &BTreeSet<usize>,
        sparse_rows: &BTreeMap<u32, Vec<u32>>,
    ) -> Result<u64, ClusterShPayloadsError> {
        let mut total = 0u64;
        let owned_tiles = nodes.iter().try_fold(0u64, |count, node| {
            if self.node_owner[node] != cluster_id {
                return Ok(count);
            }
            count
                .checked_add(u64::from(self.prefix.bricks[*node].stored_tile_count))
                .ok_or(ClusterShPayloadsError::SizeOverflow("owned dense tiles"))
        })?;
        for source in self.inputs.sources {
            match source {
                ClusterShPayloadsSourceMetadata::Dense { .. } => {
                    total = total
                        .checked_add(
                            owned_tiles
                                .checked_mul(isolated_tile_bytes(dense_format(*source)?)?)
                                .ok_or(ClusterShPayloadsError::SizeOverflow(
                                    "dense resident bytes",
                                ))?,
                        )
                        .ok_or(ClusterShPayloadsError::SizeOverflow("resident bytes"))?;
                }
                ClusterShPayloadsSourceMetadata::Sparse {
                    section_id,
                    valid_probe_masks,
                    cell_levels,
                    affinity_offsets,
                    ..
                } => {
                    let rows = sparse_rows
                        .get(section_id)
                        .expect("all sparse rows recorded");
                    for &row in rows {
                        if sparse_row_role(self.inputs.directory, cluster_id, *section_id, row)?
                            != ClusterRangeRole::Owned
                        {
                            continue;
                        }
                        let start = affinity_offsets[row as usize];
                        let end = affinity_offsets[row as usize + 1];
                        let entry_count = u64::from(end - start);
                        let level = Level::from_u8(cell_levels[row as usize]).ok_or_else(|| {
                            ClusterShPayloadsError::SourceMismatch(format!(
                                "source {section_id} row {row} has invalid level {}",
                                cell_levels[row as usize]
                            ))
                        })?;
                        let tile_f16 = u64::try_from(stored_delta_tiles(
                            level,
                            valid_probe_masks[row as usize],
                        ))
                        .map_err(|_| ClusterShPayloadsError::SizeOverflow("sparse tile count"))?
                        .checked_mul(
                            u64::try_from(delta_probe_f16_stride(
                                CLUSTER_SH_LOGICAL_TILE_DIMENSION,
                            ))
                            .map_err(|_| ClusterShPayloadsError::SizeOverflow("delta stride"))?,
                        )
                        .and_then(|per_entry| per_entry.checked_mul(entry_count))
                        .ok_or(ClusterShPayloadsError::SizeOverflow(
                            "sparse resident tiles",
                        ))?;
                        total = total
                            .checked_add(entry_count.checked_mul(16).ok_or(
                                ClusterShPayloadsError::SizeOverflow("sparse resident entries"),
                            )?)
                            .and_then(|value| value.checked_add(tile_f16.checked_mul(2)?))
                            .ok_or(ClusterShPayloadsError::SizeOverflow("resident bytes"))?;
                    }
                }
            }
        }
        Ok(total)
    }
}

/// Recheck the id-49 generator's whole-node expansion so an id-50 chunk is
/// independently decodable even if its directory came from a stale producer.
fn validate_dense_node_closure(
    dense_indices: &[u32],
    base: ClusterShPayloadsBaseMetadata<'_>,
    affinity_dimensions: [u32; 3],
) -> Result<(), ClusterShPayloadsError> {
    let mut visited_nodes = BTreeSet::new();
    for &index in dense_indices {
        let probe = base
            .probes
            .get(usize::try_from(index).map_err(|_| {
                ClusterShPayloadsError::SizeOverflow("dense probe index exceeds usize")
            })?)
            .ok_or_else(|| {
                ClusterShPayloadsError::SourceMismatch(format!(
                    "id 49 dense probe {index} is outside id 34 metadata"
                ))
            })?;
        if probe.validity == 0 {
            continue;
        }
        let probe_coords = unflatten_probe(index, base.grid_dimensions)?;
        let brick_coords = probe_coords.map(|coordinate| coordinate / 4);
        let node_edge = 1u32
            .checked_shl(u32::from(probe.node_scale))
            .ok_or(ClusterShPayloadsError::SizeOverflow("dense node scale"))?;
        let node_origin = brick_coords.map(|coordinate| coordinate / node_edge * node_edge);
        if !visited_nodes.insert((node_origin, probe.node_scale)) {
            continue;
        }
        let node_end_brick = [
            node_origin[0]
                .checked_add(node_edge)
                .ok_or(ClusterShPayloadsError::SizeOverflow("dense node brick end"))?,
            node_origin[1]
                .checked_add(node_edge)
                .ok_or(ClusterShPayloadsError::SizeOverflow("dense node brick end"))?,
            node_origin[2]
                .checked_add(node_edge)
                .ok_or(ClusterShPayloadsError::SizeOverflow("dense node brick end"))?,
        ];
        if node_end_brick
            .iter()
            .zip(affinity_dimensions)
            .any(|(&end, dimension)| end > dimension)
        {
            return source_mismatch(format!(
                "id 34 node {node_origin:?}/scale {} exceeds affinity dimensions {affinity_dimensions:?}",
                probe.node_scale
            ));
        }
        let node_probe_origin = [
            node_origin[0]
                .checked_mul(4)
                .ok_or(ClusterShPayloadsError::SizeOverflow(
                    "dense node probe origin",
                ))?,
            node_origin[1]
                .checked_mul(4)
                .ok_or(ClusterShPayloadsError::SizeOverflow(
                    "dense node probe origin",
                ))?,
            node_origin[2]
                .checked_mul(4)
                .ok_or(ClusterShPayloadsError::SizeOverflow(
                    "dense node probe origin",
                ))?,
        ];
        let node_probe_end = std::array::from_fn(|axis| {
            u64::from(node_end_brick[axis])
                .checked_mul(4)
                .ok_or(ClusterShPayloadsError::SizeOverflow("dense node probe end"))
                .and_then(|end| {
                    u32::try_from(end.min(u64::from(base.grid_dimensions[axis])))
                        .map_err(|_| ClusterShPayloadsError::SizeOverflow("dense node probe end"))
                })
        });
        let [end_x, end_y, end_z] = node_probe_end;
        let node_probe_end = [end_x?, end_y?, end_z?];
        for z in node_probe_origin[2]..node_probe_end[2] {
            for y in node_probe_origin[1]..node_probe_end[1] {
                for x in node_probe_origin[0]..node_probe_end[0] {
                    let constituent = flatten([x, y, z], base.grid_dimensions)?;
                    if dense_indices.binary_search(&constituent).is_err() {
                        return source_mismatch(format!(
                            "id 49 dense range omits probe {constituent} from id 34 node {node_origin:?}/scale {}",
                            probe.node_scale
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

pub(super) fn validate_index_against_expected(
    cluster_id: u32,
    actual: &ClusterShPayloadsIndexRecord,
    expected: &ExpectedChunk,
) -> Result<(), ClusterShPayloadsError> {
    if actual.payload_len != expected.payload_len
        || actual.decoded_bytes != expected.decoded_bytes
        || actual.requested_resident_bytes != expected.requested_resident_bytes
        || actual.stored_tile_count != expected.stored_tile_count
        || actual.dense_patch_count != expected.dense_patch_count
        || actual.affinity_patch_count != expected.affinity_patch_count
    {
        return invalid(format!(
            "cluster {cluster_id} index counts disagree with id 49/source metadata"
        ));
    }
    Ok(())
}
