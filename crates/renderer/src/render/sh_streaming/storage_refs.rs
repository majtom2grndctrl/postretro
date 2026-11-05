//! Storage lifetime counts for shared streamed SH data.
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use super::*;

/// How many installed clusters reference each dense node, probe word and
/// sparse row. The first reference writes the storage and the last frees it.
/// Only install and eviction change these; compose membership is a separate
/// question, answered by `RowRefTable` over what is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StorageRefs {
    nodes: BTreeMap<StoredNode, u32>,
    /// Indexed by dense probe, so a chunk's patch walk never searches.
    words: Vec<u32>,
    sparse_rows: BTreeMap<(u32, u32), u32>,
}

impl StorageRefs {
    pub(super) fn with_probe_count(probes: usize) -> Self {
        Self {
            nodes: BTreeMap::new(),
            words: vec![0; probes],
            sparse_rows: BTreeMap::new(),
        }
    }

    pub(super) fn clear(&mut self) {
        self.nodes.clear();
        self.words.fill(0);
        self.sparse_rows.clear();
    }

    pub(super) fn add_node(&mut self, node: StoredNode) -> Result<(), ShResidencyDrainError> {
        add_ref(self.nodes.entry(node).or_insert(0)).map(|_| ())
    }

    /// Returns whether that was the last reference.
    pub(super) fn release_node(
        &mut self,
        node: &StoredNode,
    ) -> Result<bool, ShResidencyDrainError> {
        release_keyed_ref(&mut self.nodes, node)
    }

    /// Returns whether this is the word's first reference.
    pub(super) fn add_word(&mut self, dense: u32) -> Result<bool, ShResidencyDrainError> {
        let count = self
            .words
            .get_mut(dense as usize)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        add_ref(count)
    }

    /// Returns whether that was the last reference.
    pub(super) fn release_word(&mut self, dense: u32) -> Result<bool, ShResidencyDrainError> {
        let count = self
            .words
            .get_mut(dense as usize)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        *count = count
            .checked_sub(1)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        Ok(*count == 0)
    }

    /// Returns whether this is the row's first reference.
    pub(super) fn add_sparse_row(
        &mut self,
        key: (u32, u32),
    ) -> Result<bool, ShResidencyDrainError> {
        add_ref(self.sparse_rows.entry(key).or_insert(0))
    }

    /// Returns whether that was the last reference.
    pub(super) fn release_sparse_row(
        &mut self,
        key: &(u32, u32),
    ) -> Result<bool, ShResidencyDrainError> {
        release_keyed_ref(&mut self.sparse_rows, key)
    }

    #[cfg(test)]
    pub(super) fn node_refs(&self, node: &StoredNode) -> u32 {
        self.nodes.get(node).copied().unwrap_or(0)
    }

    #[cfg(test)]
    pub(super) fn word_refs(&self, dense: u32) -> u32 {
        self.words[dense as usize]
    }

    #[cfg(test)]
    pub(super) fn sparse_row_refs(&self, key: &(u32, u32)) -> u32 {
        self.sparse_rows.get(key).copied().unwrap_or(0)
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.sparse_rows.is_empty() && self.words.iter().all(|&n| n == 0)
    }
}

fn add_ref(count: &mut u32) -> Result<bool, ShResidencyDrainError> {
    let first = *count == 0;
    *count = count
        .checked_add(1)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    Ok(first)
}

fn release_keyed_ref<K: Ord>(
    refs: &mut BTreeMap<K, u32>,
    key: &K,
) -> Result<bool, ShResidencyDrainError> {
    let count = refs
        .get_mut(key)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    *count -= 1;
    let last = *count == 0;
    if last {
        refs.remove(key);
    }
    Ok(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NODE: StoredNode = StoredNode {
        brick_origin: [1, 2, 3],
        scale: 0,
        level: 0,
    };

    #[test]
    fn a_reference_count_cannot_underflow() {
        let mut refs = StorageRefs::with_probe_count(1);
        assert_eq!(
            refs.release_node(&NODE),
            Err(ShResidencyDrainError::SlotOverflow)
        );
        assert_eq!(
            refs.release_word(0),
            Err(ShResidencyDrainError::SlotOverflow)
        );
        assert_eq!(
            refs.release_sparse_row(&(27, 0)),
            Err(ShResidencyDrainError::SlotOverflow)
        );
    }

    #[test]
    fn first_and_last_are_reported_once_per_lifetime() {
        let mut refs = StorageRefs::with_probe_count(1);
        assert_eq!(refs.add_word(0), Ok(true));
        assert_eq!(refs.add_word(0), Ok(false));
        assert_eq!(refs.release_word(0), Ok(false));
        assert_eq!(refs.release_word(0), Ok(true));

        assert_eq!(refs.add_sparse_row((27, 4)), Ok(true));
        assert_eq!(refs.add_sparse_row((27, 4)), Ok(false));
        assert_eq!(refs.release_sparse_row(&(27, 4)), Ok(false));
        assert_eq!(refs.release_sparse_row(&(27, 4)), Ok(true));

        refs.add_node(NODE).unwrap();
        refs.add_node(NODE).unwrap();
        assert_eq!(refs.release_node(&NODE), Ok(false));
        assert_eq!(refs.release_node(&NODE), Ok(true));
        assert!(refs.is_empty());
    }
}
