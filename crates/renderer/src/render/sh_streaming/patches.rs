//! Canonical dense-patch rewrites over the shared slot pool.

use super::*;

impl ShResidencyState {
    pub(super) fn install_patches(
        &mut self,
        journal: &mut InstallJournal,
        cluster_id: u32,
        chunk: &DecodedClusterShPayload,
    ) -> Result<Vec<InstalledProbe>, ShResidencyDrainError> {
        let Some(block) = chunk
            .blocks
            .iter()
            .find(|block| block.kind == PROBE_PATCH_BLOCK)
        else {
            return Err(malformed(cluster_id, "chunk has no probe-patch block"));
        };
        let bytes = chunk.block_bytes(block);
        if bytes.len() != block.element_count as usize * 16 {
            return Err(malformed(cluster_id, "probe-patch block length is invalid"));
        }
        let mut patched = Vec::new();
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
            let Some(owner) = self.dense_owner.get(dense as usize).copied().flatten() else {
                return Err(ShResidencyDrainError::MissingDenseOwner {
                    cluster_id,
                    dense_index: dense,
                });
            };
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
            let node_owner = self.node_owner.get(&node).copied().ok_or(
                ShResidencyDrainError::MissingDenseOwner {
                    cluster_id,
                    dense_index: dense,
                },
            )?;
            if node_owner != cluster_id && !self.installed.contains_key(&node_owner) {
                return Err(malformed(cluster_id, "node owner was not installed first"));
            }
            if owner != cluster_id {
                continue;
            }
            let slot = self.node_slots.get(&node).copied().ok_or(malformed(
                cluster_id,
                "canonical node slot was not allocated",
            ))?;
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
                    "node-local stored rank exceeds canonical live slot range",
                ));
            }
            let slot = slot
                .start
                .checked_add(local_slot)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            let rewritten = rewrite_slot(word, slot)?;
            self.journal_compose_word(journal, cluster_id, dense, rewritten)?;
            patched.push(InstalledProbe {
                dense,
                mean_distance,
                mean_sq_distance,
            });
        }
        Ok(patched)
    }

    /// Translate the id-50 closure-relative stored-tile ranks into the live
    /// canonical node ranges. The source rank is intentionally revalidated
    /// here rather than trusted from the loader: it must be one contiguous,
    /// globally-prefix-ordered closure, and a node may only contribute tile
    /// uploads from its canonical writer.
    pub(super) fn local_to_live_slots(
        &self,
        cluster_id: u32,
        chunk: &DecodedClusterShPayload,
    ) -> Result<BTreeMap<u32, u32>, ShResidencyDrainError> {
        let patch_block = chunk
            .blocks
            .iter()
            .find(|block| block.kind == PROBE_PATCH_BLOCK)
            .ok_or(malformed(cluster_id, "chunk has no probe-patch block"))?;
        let bytes = chunk.block_bytes(patch_block);
        if bytes.len() != patch_block.element_count as usize * 16 {
            return Err(malformed(cluster_id, "probe-patch block length is invalid"));
        }
        let isolated = chunk
            .blocks
            .iter()
            .find(|block| {
                block.kind == ISOLATED_ATLAS_BLOCK && block.section_id == INDIRECT_BASE_ID
            })
            .ok_or(malformed(cluster_id, "chunk has no id-34 isolated atlas"))?;

        let mut closure_bases = BTreeMap::<StoredNode, u32>::new();
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
            let layout = self.node_layouts.get(&node).ok_or(malformed(
                cluster_id,
                "canonical node lacks id-34 prefix metadata",
            ))?;
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

        let mut ordered: Vec<_> = closure_bases.into_iter().collect();
        ordered.sort_by_key(|(node, _)| {
            self.node_layouts
                .get(node)
                .map(|layout| layout.global_base_slot)
                .unwrap_or(u32::MAX)
        });
        let mut expected_base = 0u32;
        let mut local_to_live = BTreeMap::new();
        for (node, source_base) in ordered {
            let layout = self.node_layouts.get(&node).ok_or(malformed(
                cluster_id,
                "canonical node lacks id-34 prefix metadata",
            ))?;
            if source_base != expected_base {
                return Err(malformed(
                    cluster_id,
                    "id-50 node closure is not globally prefix-ordered",
                ));
            }
            expected_base = expected_base
                .checked_add(layout.tile_count)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            // Halo chunks carry this source closure so their probe words can
            // be verified, but only its canonical owner ever uploads the
            // shared physical tiles.
            if self.node_owner.get(&node).copied() != Some(cluster_id) {
                continue;
            }
            let live = self.node_slots.get(&node).copied().ok_or(malformed(
                cluster_id,
                "canonical node slot was not allocated",
            ))?;
            if live.len != layout.tile_count {
                return Err(malformed(
                    cluster_id,
                    "canonical live node range has wrong length",
                ));
            }
            for local in 0..layout.tile_count {
                local_to_live.insert(
                    source_base
                        .checked_add(local)
                        .ok_or(ShResidencyDrainError::SlotOverflow)?,
                    live.start
                        .checked_add(local)
                        .ok_or(ShResidencyDrainError::SlotOverflow)?,
                );
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
