//! Proof that chunks agree on the storage they share.
//!
//! A shared node or row is stored once at runtime, so every chunk that carries
//! it must carry the same bytes (see context/lib/rendering_pipeline.md §4
//! Bounded SH pool). BC6H is encoded per chunk, which is why agreement is
//! checked rather than assumed.
//! Hashes only: chunks are spooled one at a time, and no payload stays here.
//! See: context/lib/build_pipeline.md §PRL section IDs (SH mandatory bound)

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use super::DenseNode;
use super::wire::{BLOCK_KIND_ISOLATED_ATLAS, EncodedBlock, EncodedSparse};

#[derive(Clone, Copy)]
struct FirstSeen {
    cluster: u32,
    hash: blake3::Hash,
}

/// Every shared node and row seen so far, across a level's chunks.
#[derive(Default)]
pub(super) struct SharedBytesCheck {
    nodes: HashMap<usize, FirstSeen>,
    /// Keyed by `(section_id, row)`.
    rows: HashMap<(u32, u32), FirstSeen>,
}

impl SharedBytesCheck {
    /// Check each of a chunk's dense nodes across every atlas in `blocks`.
    pub(super) fn record_nodes(
        &mut self,
        cluster: u32,
        nodes: &[DenseNode],
        blocks: &[EncodedBlock],
    ) -> anyhow::Result<()> {
        for node in nodes {
            let mut hasher = blake3::Hasher::new();
            for block in blocks
                .iter()
                .filter(|block| block.kind == BLOCK_KIND_ISOLATED_ATLAS)
            {
                hasher.update(&block.section_id.to_le_bytes());
                block.hash_node_tiles(node, &mut hasher)?;
            }
            let seen = FirstSeen {
                cluster,
                hash: hasher.finalize(),
            };
            record(&mut self.nodes, node.index, seen, || {
                format!("dense node {}", node.index)
            })?;
        }
        Ok(())
    }

    /// Check each row of one family's block in a chunk.
    pub(super) fn record_rows(
        &mut self,
        cluster: u32,
        sparse: &EncodedSparse,
    ) -> anyhow::Result<()> {
        for (&row, &hash) in sparse.rows.iter().zip(&sparse.row_hashes) {
            let seen = FirstSeen { cluster, hash };
            record(&mut self.rows, (sparse.section_id, row), seen, || {
                format!("sparse row {row} of section {}", sparse.section_id)
            })?;
        }
        Ok(())
    }
}

fn record<K: std::hash::Hash + Eq>(
    seen: &mut HashMap<K, FirstSeen>,
    key: K,
    current: FirstSeen,
    describe: impl FnOnce() -> String,
) -> anyhow::Result<()> {
    match seen.entry(key) {
        Entry::Vacant(slot) => {
            slot.insert(current);
            Ok(())
        }
        Entry::Occupied(first) if first.get().hash == current.hash => Ok(()),
        Entry::Occupied(first) => anyhow::bail!(
            "id-50 clusters {} and {} store different bytes for shared {}; the first resident \
             cluster's copy would serve both, so this is a compiler bug, not a map problem",
            first.get().cluster,
            current.cluster,
            describe(),
        ),
    }
}
