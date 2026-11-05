//! Dense-patch rewrites over the shared slot pool.
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use super::*;

/// One stored node's tiles: `len` consecutive closure-local slots of an
/// id-50 isolated atlas that land on `len` consecutive live pool slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SlotRun {
    pub(super) local: u32,
    pub(super) live: u32,
    pub(super) len: u32,
}

/// The nodes a chunk's probe patches name, in `StoredNode` order.
#[derive(Debug, Default)]
pub(super) struct ChunkNodes {
    pub(super) referenced: Vec<StoredNode>,
    /// The nodes nothing held until this install, whose tiles it uploads.
    pub(super) first_written: BTreeSet<StoredNode>,
}

/// A chunk's probes after rewriting their addresses.
#[derive(Debug, Default)]
pub(super) struct ChunkPatches {
    /// Every probe the chunk names; each one holds a word reference.
    pub(super) probes: Vec<InstalledProbe>,
    /// The dense indices whose words this install wrote first.
    pub(super) first_written: Vec<u32>,
}

/// A chunk's probe-patch block, length-checked.
fn probe_patch_bytes(
    cluster_id: u32,
    chunk: &DecodedClusterShPayload,
) -> Result<&[u8], ShResidencyDrainError> {
    let block = chunk
        .blocks
        .iter()
        .find(|block| block.kind == PROBE_PATCH_BLOCK)
        .ok_or(malformed(cluster_id, "chunk has no probe-patch block"))?;
    let bytes = chunk.block_bytes(block);
    if bytes.len() != block.element_count as usize * 16 {
        return Err(malformed(cluster_id, "probe-patch block length is invalid"));
    }
    Ok(bytes)
}

impl ShResidencyState {
    /// Reference every node the chunk's patches name, allocating the ones
    /// nothing holds. Node identity comes from id-34 metadata, so any chunk
    /// naming a node can write it.
    pub(super) fn reference_chunk_nodes(
        &mut self,
        journal: &mut InstallJournal,
        cluster_id: u32,
        chunk: &DecodedClusterShPayload,
    ) -> Result<ChunkNodes, ShResidencyDrainError> {
        let bytes = probe_patch_bytes(cluster_id, chunk)?;
        let mut nodes = BTreeSet::new();
        // A node covers a brick of probes, so consecutive patches mostly
        // repeat it.
        let mut last_node = None;
        for offset in (0..bytes.len()).step_by(16) {
            let dense =
                u32_at(bytes, offset).ok_or(malformed(cluster_id, "truncated probe patch"))?;
            let node = self
                .dense_node
                .get(dense as usize)
                .copied()
                .flatten()
                .ok_or(malformed(cluster_id, "patch names an invalid dense probe"))?;
            if last_node != Some(node) {
                nodes.insert(node);
                last_node = Some(node);
            }
        }
        let mut result = ChunkNodes {
            referenced: Vec::with_capacity(nodes.len()),
            first_written: BTreeSet::new(),
        };
        for node in nodes {
            let tile_count = self
                .node_layouts
                .get(&node)
                .ok_or(malformed(cluster_id, "node lacks id-34 prefix metadata"))?
                .tile_count;
            if self.journal_reference_node(journal, node, tile_count)? {
                result.first_written.insert(node);
            }
            result.referenced.push(node);
        }
        Ok(result)
    }

    /// Rewrite and reference every probe the chunk names. Each word is fully
    /// determined by id-34 metadata and its node's live slot, so a word another
    /// cluster already wrote stays as it is.
    pub(super) fn install_patches(
        &mut self,
        journal: &mut InstallJournal,
        cluster_id: u32,
        chunk: &DecodedClusterShPayload,
    ) -> Result<ChunkPatches, ShResidencyDrainError> {
        let bytes = probe_patch_bytes(cluster_id, chunk)?;
        let mut patches = ChunkPatches {
            probes: Vec::with_capacity(bytes.len() / 16),
            first_written: Vec::new(),
        };
        // Consecutive patches mostly share a node, so reuse the last node's
        // live range instead of searching the node map once per probe.
        let mut last_node: Option<(StoredNode, PoolRange)> = None;
        for offset in (0..bytes.len()).step_by(16) {
            let dense =
                u32_at(bytes, offset).ok_or(malformed(cluster_id, "truncated probe patch"))?;
            let word =
                u32_at(bytes, offset + 4).ok_or(malformed(cluster_id, "truncated probe word"))?;
            let mean_distance = bytes
                .get(offset + 8..offset + 10)
                .map(|value| u16::from_le_bytes(value.try_into().expect("two-byte slice")))
                .ok_or(malformed(cluster_id, "truncated probe mean distance"))?;
            let mean_sq_distance = bytes
                .get(offset + 10..offset + 12)
                .map(|value| u16::from_le_bytes(value.try_into().expect("two-byte slice")))
                .ok_or(malformed(
                    cluster_id,
                    "truncated probe mean-square distance",
                ))?;
            let Some(node) = self.dense_node.get(dense as usize).copied().flatten() else {
                return Err(malformed(cluster_id, "patch names an invalid dense probe"));
            };
            if word & PROBE_INDIRECTION_VALID_BIT == 0
                || word & PROBE_INDIRECTION_LEVEL_MASK != u32::from(node.level)
                || (word & PROBE_INDIRECTION_SCALE_MASK) >> PROBE_INDIRECTION_SCALE_SHIFT
                    != u32::from(node.scale)
            {
                return Err(malformed(
                    cluster_id,
                    "probe patch flags disagree with retained id-34 node metadata",
                ));
            }
            let slot = match last_node {
                Some((cached, slot)) if cached == node => slot,
                _ => {
                    let slot = self
                        .node_slots
                        .get(&node)
                        .copied()
                        .ok_or(malformed(cluster_id, "stored node slot was not allocated"))?;
                    last_node = Some((node, slot));
                    slot
                }
            };
            let local_slot = self
                .dense_node_local_slot
                .get(dense as usize)
                .copied()
                .flatten()
                .ok_or(malformed(
                    cluster_id,
                    "dense probe lacks a node-local stored rank",
                ))?;
            let wire_rank = word >> PROBE_INDIRECTION_SLOT_SHIFT;
            if wire_rank < local_slot {
                return Err(malformed(
                    cluster_id,
                    "probe patch rank precedes its node-local stored rank",
                ));
            }
            if local_slot >= slot.len {
                return Err(malformed(
                    cluster_id,
                    "node-local stored rank exceeds live slot range",
                ));
            }
            let slot = slot
                .start
                .checked_add(local_slot)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            let rewritten = rewrite_slot(word, slot)?;
            if self.journal_reference_word(journal, dense)? {
                self.journal_compose_word(journal, dense, rewritten)?;
                patches.first_written.push(dense);
            } else {
                debug_assert_eq!(
                    self.compose_words[dense as usize], rewritten,
                    "a shared probe word follows its node's live slot"
                );
            }
            patches.probes.push(InstalledProbe {
                dense,
                mean_distance,
                mean_sq_distance,
            });
        }
        Ok(patches)
    }

    /// Translate the id-50 closure-relative stored-tile ranks into live node
    /// ranges. The source rank is intentionally revalidated here rather than
    /// trusted from the loader: it must be one contiguous, globally-prefix-
    /// ordered closure, and only `first_written` nodes contribute tile
    /// uploads, because a live node already holds these bytes.
    pub(super) fn local_to_live_slots(
        &self,
        cluster_id: u32,
        chunk: &DecodedClusterShPayload,
        first_written: &BTreeSet<StoredNode>,
    ) -> Result<Vec<SlotRun>, ShResidencyDrainError> {
        let bytes = probe_patch_bytes(cluster_id, chunk)?;
        let isolated = chunk
            .blocks
            .iter()
            .find(|block| {
                block.kind == ISOLATED_ATLAS_BLOCK && block.section_id == INDIRECT_BASE_ID
            })
            .ok_or(malformed(cluster_id, "chunk has no id-34 isolated atlas"))?;

        let mut closure_bases = BTreeMap::<StoredNode, u32>::new();
        // A repeat of the previous patch's node and base was already checked
        // and recorded; skip the map work for it.
        let mut last_node: Option<(StoredNode, u32)> = None;
        for offset in (0..bytes.len()).step_by(16) {
            let dense =
                u32_at(bytes, offset).ok_or(malformed(cluster_id, "truncated probe patch"))?;
            let word =
                u32_at(bytes, offset + 4).ok_or(malformed(cluster_id, "truncated probe word"))?;
            let node = self
                .dense_node
                .get(dense as usize)
                .copied()
                .flatten()
                .ok_or(malformed(cluster_id, "patch names an invalid dense probe"))?;
            let local = self
                .dense_node_local_slot
                .get(dense as usize)
                .copied()
                .flatten()
                .ok_or(malformed(
                    cluster_id,
                    "dense probe lacks a node-local stored rank",
                ))?;
            let rank = word >> PROBE_INDIRECTION_SLOT_SHIFT;
            let base = rank.checked_sub(local).ok_or(malformed(
                cluster_id,
                "probe patch rank precedes node-local rank",
            ))?;
            if last_node == Some((node, base)) {
                continue;
            }
            last_node = Some((node, base));
            let layout = self
                .node_layouts
                .get(&node)
                .ok_or(malformed(cluster_id, "node lacks id-34 prefix metadata"))?;
            if base
                .checked_add(layout.tile_count)
                .is_none_or(|end| end > isolated.element_count)
            {
                return Err(malformed(
                    cluster_id,
                    "probe patch closure rank exceeds id-34 isolated atlas",
                ));
            }
            match closure_bases.entry(node) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(base);
                }
                std::collections::btree_map::Entry::Occupied(entry) if *entry.get() == base => {}
                std::collections::btree_map::Entry::Occupied(_) => {
                    return Err(malformed(
                        cluster_id,
                        "one stored node has inconsistent closure-local ranks",
                    ));
                }
            }
        }

        let mut ordered: Vec<_> = closure_bases
            .into_iter()
            .map(|(node, base)| {
                let global_base = self
                    .node_layouts
                    .get(&node)
                    .map_or(u32::MAX, |layout| layout.global_base_slot);
                (global_base, node, base)
            })
            .collect();
        ordered.sort_by_key(|&(global_base, _, _)| global_base);
        let mut expected_base = 0u32;
        let mut local_to_live = Vec::new();
        for (_, node, source_base) in ordered {
            let layout = self
                .node_layouts
                .get(&node)
                .ok_or(malformed(cluster_id, "node lacks id-34 prefix metadata"))?;
            if source_base != expected_base {
                return Err(malformed(
                    cluster_id,
                    "id-50 node closure is not globally prefix-ordered",
                ));
            }
            expected_base = expected_base
                .checked_add(layout.tile_count)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            // Every chunk carries its whole closure so its probe words can be
            // verified, but a node's tiles upload only with its first writer.
            if !first_written.contains(&node) {
                continue;
            }
            let live = self
                .node_slots
                .get(&node)
                .copied()
                .ok_or(malformed(cluster_id, "stored node slot was not allocated"))?;
            if live.len != layout.tile_count {
                return Err(malformed(cluster_id, "live node range has wrong length"));
            }
            if layout.tile_count != 0 {
                local_to_live.push(SlotRun {
                    local: source_base,
                    live: live.start,
                    len: layout.tile_count,
                });
            }
        }
        if expected_base != isolated.element_count {
            return Err(malformed(
                cluster_id,
                "id-50 isolated atlas contains slots outside the node closure",
            ));
        }
        Ok(local_to_live)
    }
}
