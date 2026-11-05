//! What an install, or the whole mandatory tier, needs from the pools.
//! See: context/lib/rendering_pipeline.md §4 (Bounded SH pool)

use super::allocator::reserved_tile_f16;
use super::*;
use postretro_render_cpu::sh_residency::sparse_row_demand;

#[derive(Debug, Clone, Copy)]
struct RowDemand {
    section_id: u32,
    row: u32,
    entries: u32,
    tile_f16: u32,
}

/// Storage named once each, whoever else names it. Cluster bytes do not add
/// up under shared references, so this is a unique union, resident or not.
#[derive(Debug, Default)]
pub(super) struct PoolDemand {
    nodes: Vec<(StoredNode, u32)>,
    rows: Vec<RowDemand>,
}

/// Each pool's address high-water once a demand has placed: first-fit against
/// the free ranges held right now, so fragmentation is already in the figure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PoolPlacement {
    pub(super) dense_slots: u32,
    pub(super) sparse: SparseCapacityFloors,
}

impl ShResidencyState {
    /// What this chunk names, in the order its install would allocate.
    pub(super) fn chunk_demand(
        &self,
        cluster_id: u32,
        chunk: &DecodedClusterShPayload,
    ) -> Result<PoolDemand, ShResidencyDrainError> {
        let nodes = if self.validate_isolated_atlases(cluster_id, chunk)? {
            self.chunk_node_set(cluster_id, chunk)?
        } else {
            BTreeSet::new()
        };
        let rows = self
            .collect_sparse_rows(cluster_id, chunk)?
            .into_iter()
            .map(|(section_id, row)| RowDemand {
                section_id,
                row: row.row,
                entries: row.entry_count,
                tile_f16: row.tile_f16_count,
            })
            .collect();
        Ok(PoolDemand {
            nodes: self.with_tile_counts(cluster_id, nodes)?,
            rows,
        })
    }

    /// The unique union of what `clusters` name, priced from id 49 alone: a
    /// mandatory target usually has no chunk in hand yet. This walks every
    /// cluster's probes, so it belongs on the growth path and nowhere hotter.
    pub(super) fn mandatory_demand(
        &self,
        clusters: &[u32],
    ) -> Result<PoolDemand, ShResidencyDrainError> {
        let mut nodes = BTreeSet::new();
        let mut rows = BTreeMap::new();
        for &cluster_id in clusters {
            let storage = self
                .cluster_storage
                .get(cluster_id as usize)
                .ok_or(ShResidencyDrainError::TargetOutOfRange(cluster_id))?;
            for probes in &storage.dense {
                // Consecutive probes mostly repeat the previous node.
                let mut last = None;
                for &node in self.dense_node[probes.start as usize..probes.end as usize]
                    .iter()
                    .flatten()
                {
                    if last != Some(node) {
                        nodes.insert(node);
                        last = Some(node);
                    }
                }
            }
            for (section_id, section_rows) in &storage.sparse {
                for row in section_rows.clone() {
                    if let std::collections::btree_map::Entry::Vacant(entry) =
                        rows.entry((*section_id, row))
                    {
                        entry.insert(sparse_row_demand(
                            &self.source_metadata,
                            cluster_id,
                            *section_id,
                            row,
                        )?);
                    }
                }
            }
        }
        Ok(PoolDemand {
            nodes: self.with_tile_counts(0, nodes)?,
            rows: rows
                .into_iter()
                .map(|((section_id, row), (entries, tile_f16))| RowDemand {
                    section_id,
                    row,
                    entries,
                    tile_f16,
                })
                .collect(),
        })
    }

    fn with_tile_counts(
        &self,
        cluster_id: u32,
        nodes: BTreeSet<StoredNode>,
    ) -> Result<Vec<(StoredNode, u32)>, ShResidencyDrainError> {
        nodes
            .into_iter()
            .map(|node| {
                let layout = self
                    .node_layouts
                    .get(&node)
                    .ok_or(malformed(cluster_id, "node lacks id-34 prefix metadata"))?;
                Ok((node, layout.tile_count))
            })
            .collect()
    }

    /// Where `demand` would land, read against the mirrors as they stand.
    /// Storage already live costs nothing, and a free that took shared
    /// storage away costs it again, so this stays exact across evictions.
    pub(super) fn place(
        &self,
        demand: &PoolDemand,
    ) -> Result<PoolPlacement, ShResidencyDrainError> {
        let mut dense = self.dense_slots.clone();
        for (node, tiles) in &demand.nodes {
            if !self.node_slots.contains_key(node) {
                dense.allocate_undoable(*tiles)?;
            }
        }
        let mut sparse: BTreeMap<u32, (FirstFitRanges, FirstFitRanges)> = self
            .sparse_pools
            .iter()
            .map(|(&section_id, pool)| (section_id, (pool.entries.clone(), pool.tiles.clone())))
            .collect();
        for row in &demand.rows {
            if self
                .storage_refs
                .holds_sparse_row(&(row.section_id, row.row))
            {
                continue;
            }
            let (entries, tiles) =
                sparse
                    .get_mut(&row.section_id)
                    .ok_or(ShResidencyDrainError::MalformedChunk {
                        cluster_id: 0,
                        reason: "sparse source disappeared during placement",
                    })?;
            entries.allocate_undoable(row.entries)?;
            tiles.allocate_undoable(reserved_tile_f16(row.tile_f16)?)?;
        }
        Ok(PoolPlacement {
            dense_slots: dense.capacity,
            sparse: sparse
                .into_iter()
                .map(|(section_id, (entries, tiles))| {
                    (section_id, (entries.capacity, tiles.capacity))
                })
                .collect(),
        })
    }

    /// Whether the pools as allocated now hold `placement`. A state with no
    /// GPU pools has no physical capacity to run out of.
    pub(super) fn holds(&self, placement: &PoolPlacement) -> bool {
        let Some(gpu) = self.gpu.as_ref() else {
            return true;
        };
        placement.dense_slots <= gpu.shape.slots
            && placement
                .sparse
                .iter()
                .all(|(&section_id, &(entries, tiles))| {
                    entries <= gpu.sparse_entry_capacity(section_id)
                        && tiles <= gpu.sparse_tile_f16_capacity(section_id)
                })
    }

    /// Refuse a staged install whose provisional ranges ran past the pools.
    /// `FirstFitRanges::capacity` is only an address high-water and may exceed
    /// the GPU shape after fragmentation, so no upload may address it unchecked.
    pub(super) fn check_pool_room(&self) -> Result<(), ShResidencyDrainError> {
        let Some(gpu) = self.gpu.as_ref() else {
            return Ok(());
        };
        let room = self.dense_slots.capacity <= gpu.shape.slots
            && self.sparse_pools.iter().all(|(&section_id, pool)| {
                pool.entries.capacity <= gpu.sparse_entry_capacity(section_id)
                    && pool.tiles.capacity <= gpu.sparse_tile_f16_capacity(section_id)
            });
        if room {
            Ok(())
        } else {
            Err(ShResidencyDrainError::PoolShortfall)
        }
    }
}
