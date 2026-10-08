//! Stored-node keyed table laid out over the affinity-brick grid.
//!
//! A level has one stored node per brick origin in practice, so a flat array
//! indexed by the origin's affinity index answers a lookup in one probe and
//! one comparison. A key the array cannot hold exactly (an origin outside the
//! grid, or a second node at an origin another scale or level already holds)
//! lands in an ordered overflow map, so the table answers every query
//! identically to a `BTreeMap<StoredNode, V>` for any input.
//!
//! Iteration order is unspecified. Code that needs `StoredNode` order sorts
//! the keys it collects.

use std::collections::BTreeMap;
use std::ops::Index;

use super::ownership::StoredNode;

#[derive(Debug)]
pub(super) struct NodeMap<V> {
    /// Affinity-brick extent of the array, or zero when the grid was too
    /// large for the probe count to justify it (a malformed manifest).
    affinity_dims: [u32; 3],
    /// `(scale, level, value)` of the node stored at each origin index.
    primary: Vec<Option<(u8, u8, V)>>,
    overflow: BTreeMap<StoredNode, V>,
    len: usize,
}

impl<V> Default for NodeMap<V> {
    fn default() -> Self {
        Self {
            affinity_dims: [0; 3],
            primary: Vec::new(),
            overflow: BTreeMap::new(),
            len: 0,
        }
    }
}

impl<V> NodeMap<V> {
    /// Size the array to the 4x4x4 brick grid over `grid_dimensions`. A grid
    /// with more bricks than probes cannot be well-formed; it keeps every key
    /// in the overflow map instead of allocating from an untrusted extent.
    pub(super) fn for_grid(grid_dimensions: [u32; 3], probe_count: usize) -> Self {
        let affinity_dims = grid_dimensions.map(|axis| axis.div_ceil(4));
        let brick_count = affinity_dims.iter().try_fold(1usize, |count, &axis| {
            count.checked_mul(usize::try_from(axis).ok()?)
        });
        match brick_count {
            Some(count) if count <= probe_count.max(1) => Self {
                affinity_dims,
                primary: std::iter::repeat_with(|| None).take(count).collect(),
                overflow: BTreeMap::new(),
                len: 0,
            },
            _ => Self::default(),
        }
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.len
    }

    fn slot(&self, node: &StoredNode) -> Option<usize> {
        let [width, height, depth] = self.affinity_dims;
        let [x, y, z] = node.brick_origin;
        if x >= width || y >= height || z >= depth {
            return None;
        }
        Some(x as usize + width as usize * (y as usize + height as usize * z as usize))
    }

    pub(super) fn get(&self, node: &StoredNode) -> Option<&V> {
        if let Some(slot) = self.slot(node)
            && let Some((scale, level, value)) = &self.primary[slot]
            && *scale == node.scale
            && *level == node.level
        {
            return Some(value);
        }
        if self.overflow.is_empty() {
            return None;
        }
        self.overflow.get(node)
    }

    /// Insert `value` under `node`, or run `merge` on the value already
    /// there. The table never removes entries, so an empty array cell means
    /// no overflow key shares its origin.
    pub(super) fn insert_or_update(
        &mut self,
        node: StoredNode,
        value: V,
        merge: impl FnOnce(&mut V),
    ) {
        if let Some(slot) = self.slot(&node) {
            let cell = &mut self.primary[slot];
            if let Some((scale, level, existing)) = cell {
                if *scale == node.scale && *level == node.level {
                    merge(existing);
                    return;
                }
            } else {
                *cell = Some((node.scale, node.level, value));
                self.len += 1;
                return;
            }
        }
        match self.overflow.entry(node) {
            std::collections::btree_map::Entry::Occupied(mut entry) => merge(entry.get_mut()),
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(value);
                self.len += 1;
            }
        }
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &V> {
        self.primary
            .iter()
            .filter_map(|entry| entry.as_ref().map(|(_, _, value)| value))
            .chain(self.overflow.values())
    }

    /// Every entry, in unspecified order.
    pub(super) fn iter(&self) -> impl Iterator<Item = (StoredNode, &V)> {
        let [width, height, _] = self.affinity_dims;
        let (width, height) = (width as usize, height as usize);
        let primary = self
            .primary
            .iter()
            .enumerate()
            .filter_map(move |(index, entry)| {
                let (scale, level, value) = entry.as_ref()?;
                let node = StoredNode {
                    brick_origin: [
                        (index % width) as u32,
                        (index / width % height) as u32,
                        (index / (width * height)) as u32,
                    ],
                    scale: *scale,
                    level: *level,
                };
                Some((node, value))
            });
        primary.chain(self.overflow.iter().map(|(node, value)| (*node, value)))
    }
}

impl<V> Index<&StoredNode> for NodeMap<V> {
    type Output = V;

    fn index(&self, node: &StoredNode) -> &V {
        self.get(node).expect("stored node is present in the table")
    }
}
