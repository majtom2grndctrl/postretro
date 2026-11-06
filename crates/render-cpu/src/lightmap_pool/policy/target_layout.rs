// A compaction's target layout: packed from scratch, then fitted to where blocks already sit.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// The from-scratch pack fixes which blocks share a layer and which share a
// shelf; everything else about the layout is free. Its layers are
// interchangeable, its shelves stack in any order with any gaps, and a shelf's
// blocks sit anywhere along it. Using that freedom keeps blocks where they
// are, and in a tight pool it decides whether a copy order exists at all: a
// large block with a new place in its own layer must fit the spare to get
// there, and one left in place never has to.

use std::cmp::Reverse;

use super::super::{BlockPool, Slot};
use super::LightmapPoolModel;

/// One shelf along a layer's height, or one block along a shelf's width.
#[derive(Debug, Clone, Copy)]
struct Item {
    /// The shelf's `y` or the block's index in the target, as packed.
    key: u32,
    size: u32,
    /// Where something already sitting here wants it.
    wanted: Option<u32>,
    at: Option<u32>,
}

/// Reused target buffers.
#[derive(Debug, Default)]
pub(super) struct TargetScratch {
    /// Every placed block in packing order, the order that replays the
    /// layout into the pool.
    pub slots: Vec<(u32, Slot)>,
    /// Texels of target layer `t` already in the `p`th chosen pool layer, at
    /// `t * n + p`.
    kept_texels: Vec<u64>,
    /// `(kept texels, target layer, index into the chosen pool layers)`, best
    /// match first.
    pairings: Vec<(u64, u32, u32)>,
    /// Index into the chosen pool layers for each target layer.
    layer_map: Vec<u32>,
    /// Indices into `slots`, by layer, then shelf.
    by_shelf: Vec<u32>,
    shelves: Vec<Item>,
    spans: Vec<Item>,
}

impl TargetScratch {
    #[cfg(test)]
    pub(super) fn capacities(&self) -> [usize; 7] {
        [
            self.slots.capacity(),
            self.kept_texels.capacity(),
            self.pairings.capacity(),
            self.layer_map.capacity(),
            self.by_shelf.capacity(),
            self.shelves.capacity(),
            self.spans.capacity(),
        ]
    }
}

impl LightmapPoolModel {
    /// Pack `keep`, and `band` as far as it fits under the cap, into as many
    /// layers as `pool_layers` names, then fit the result to those layers, on
    /// a scratch allocator: a target that does not fit or has no copy order
    /// never touches the pool. `pool_layers` ascends. False when a block of
    /// `keep` does not fit.
    pub(super) fn pack_target(
        &self,
        keep: &[u32],
        band: &[u32],
        pool_layers: &[u32],
        t: &mut TargetScratch,
    ) -> bool {
        let limit = Some(pool_layers.len());
        let under_cap = pool_layers
            .iter()
            .take_while(|&&layer| layer < self.cap_eff())
            .count();
        let mut pool = BlockPool::new(self.edge, None);
        t.slots.clear();
        for &block in keep {
            let (width, height) = self.alloc_extent(block);
            match pool.allocate_within(width, height, limit) {
                Some(slot) => t.slots.push((block, slot)),
                None => return false,
            }
        }
        for &block in band {
            let (width, height) = self.alloc_extent(block);
            if let Some(slot) = pool.allocate_within(width, height, Some(under_cap)) {
                t.slots.push((block, slot));
            }
        }
        self.relabel_layers(pool_layers, under_cap, t);
        self.reflow_layers(t);
        true
    }

    /// Give each target layer the chosen pool layer already holding most of
    /// its texels: greedily, then by swaps while any pair gains. Target
    /// layers under `under_cap` stay on pool layers under the cap, where band
    /// blocks must sit.
    fn relabel_layers(&self, pool_layers: &[u32], under_cap: usize, t: &mut TargetScratch) {
        let layers = pool_layers.len();
        t.kept_texels.clear();
        t.kept_texels.resize(layers * layers, 0);
        for &(block, slot) in &t.slots {
            let at =
                self.slots[block as usize].and_then(|at| pool_layers.binary_search(&at.layer).ok());
            if let Some(at) = at {
                let texels = u64::from(slot.width) * u64::from(slot.height);
                t.kept_texels[slot.layer as usize * layers + at] += texels;
            }
        }
        let same_side = |a: usize, b: usize| (a < under_cap) == (b < under_cap);
        t.pairings.clear();
        for target in 0..layers {
            for pool in (0..layers).filter(|&pool| same_side(target, pool)) {
                let kept = t.kept_texels[target * layers + pool];
                t.pairings.push((kept, target as u32, pool as u32));
            }
        }
        t.pairings
            .sort_unstable_by_key(|&(kept, target, pool)| (Reverse(kept), target, pool));
        t.layer_map.clear();
        t.layer_map.resize(layers, u32::MAX);
        for &(_, target, pool) in &t.pairings {
            if t.layer_map[target as usize] == u32::MAX && !t.layer_map.contains(&pool) {
                t.layer_map[target as usize] = pool;
            }
        }
        let mut improved = true;
        while improved {
            improved = false;
            for a in 0..layers {
                for b in a + 1..layers {
                    let (pa, pb) = (t.layer_map[a] as usize, t.layer_map[b] as usize);
                    let kept = |target: usize, pool: usize| t.kept_texels[target * layers + pool];
                    let gains = kept(a, pb) + kept(b, pa) > kept(a, pa) + kept(b, pb);
                    if gains && same_side(a, pb) && same_side(b, pa) {
                        t.layer_map.swap(a, b);
                        improved = true;
                    }
                }
            }
        }
        for (_, slot) in &mut t.slots {
            slot.layer = pool_layers[t.layer_map[slot.layer as usize] as usize];
        }
    }

    /// Slide each layer's shelves, and each shelf's blocks, toward where the
    /// blocks already in that layer sit. A shelf follows its largest such
    /// block; a shelf or block with none fills the gaps.
    fn reflow_layers(&self, t: &mut TargetScratch) {
        let mut by_shelf = std::mem::take(&mut t.by_shelf);
        by_shelf.clear();
        by_shelf.extend(0..t.slots.len() as u32);
        by_shelf.sort_unstable_by_key(|&at| {
            let slot = t.slots[at as usize].1;
            (slot.layer, slot.y, slot.x)
        });
        let mut start = 0;
        while start < by_shelf.len() {
            let layer_id = t.slots[by_shelf[start] as usize].1.layer;
            let len = by_shelf[start..]
                .iter()
                .take_while(|&&at| t.slots[at as usize].1.layer == layer_id)
                .count();
            self.reflow_layer(&by_shelf[start..start + len], t);
            start += len;
        }
        t.by_shelf = by_shelf;
    }

    fn reflow_layer(&self, layer: &[u32], t: &mut TargetScratch) {
        // Shelves are read before any is moved: the grouping below keys on
        // the `y` each slot was packed at.
        let packed_y: fn(&[(u32, Slot)], u32) -> u32 = |slots, at| slots[at as usize].1.y;
        t.shelves.clear();
        for shelf in layer.chunk_by(|&a, &b| packed_y(&t.slots, a) == packed_y(&t.slots, b)) {
            let item = self.shelf_item(shelf, &t.slots);
            t.shelves.push(item);
        }
        slide(&mut t.shelves, self.edge);
        t.shelves.sort_unstable_by_key(|shelf| shelf.key);
        let mut start = 0;
        for at in 0..t.shelves.len() {
            let shelf_y = t.shelves[at].key;
            let len = layer[start..]
                .iter()
                .take_while(|&&block| packed_y(&t.slots, block) == shelf_y)
                .count();
            let y = t.shelves[at].at.expect("slide places every item");
            t.spans.clear();
            for &index in &layer[start..start + len] {
                let (block, slot) = t.slots[index as usize];
                t.spans.push(Item {
                    key: index,
                    size: slot.width,
                    wanted: self.slots[block as usize]
                        .filter(|now| now.layer == slot.layer)
                        .map(|now| now.x),
                    at: None,
                });
            }
            slide(&mut t.spans, self.edge);
            for span in &t.spans {
                let slot = &mut t.slots[span.key as usize].1;
                slot.x = span.at.expect("slide places every item");
                slot.y = y;
            }
            start += len;
        }
    }

    fn shelf_item(&self, shelf: &[u32], slots: &[(u32, Slot)]) -> Item {
        let packed = |at: u32| slots[at as usize];
        let anchor = shelf
            .iter()
            .filter_map(|&at| {
                let (block, slot) = packed(at);
                let now = self.slots[block as usize].filter(|now| now.layer == slot.layer)?;
                Some((u64::from(slot.width) * u64::from(slot.height), now.y))
            })
            .max_by_key(|&(texels, y)| (texels, Reverse(y)));
        Item {
            key: packed(shelf[0]).1.y,
            size: shelf
                .iter()
                .map(|&at| packed(at).1.height)
                .max()
                .unwrap_or(0),
            wanted: anchor.map(|(_, y)| y),
            at: None,
        }
    }
}

/// Lay `items`, whose sizes sum to at most `edge`, along one axis: each
/// anchored item at or as near after its wanted place as the items before it
/// allow, and every other item in the gaps, first fit, then after the last.
fn slide(items: &mut [Item], edge: u32) {
    items.sort_unstable_by_key(|item| (item.wanted.is_none(), item.wanted, item.key));
    let mut slack = edge - items.iter().map(|item| item.size).sum::<u32>();
    let anchored = items
        .iter()
        .take_while(|item| item.wanted.is_some())
        .count();
    let (anchors, loose) = items.split_at_mut(anchored);
    let mut cursor = 0;
    for anchor in anchors {
        let wanted = anchor.wanted.expect("anchors sort first");
        for item in loose.iter_mut().filter(|item| item.at.is_none()) {
            if cursor + item.size <= wanted {
                item.at = Some(cursor);
                cursor += item.size;
            }
        }
        let gap = wanted.saturating_sub(cursor).min(slack);
        slack -= gap;
        cursor += gap;
        anchor.at = Some(cursor);
        cursor += anchor.size;
    }
    for item in loose.iter_mut().filter(|item| item.at.is_none()) {
        item.at = Some(cursor);
        cursor += item.size;
    }
}

#[cfg(test)]
mod tests {
    use super::{Item, slide};

    fn item(key: u32, size: u32, wanted: Option<u32>) -> Item {
        Item {
            key,
            size,
            wanted,
            at: None,
        }
    }

    fn placed(mut items: Vec<Item>, edge: u32) -> Vec<(u32, u32)> {
        slide(&mut items, edge);
        let mut placed: Vec<(u32, u32)> = items
            .iter()
            .map(|item| (item.key, item.at.unwrap()))
            .collect();
        placed.sort_unstable();
        placed
    }

    #[test]
    fn anchors_keep_their_place_and_loose_items_fill_the_gaps() {
        let items = vec![
            item(0, 20, Some(32)),
            item(1, 12, None),
            item(2, 8, None),
            item(3, 16, None),
        ];
        assert_eq!(placed(items, 72), vec![(0, 32), (1, 0), (2, 12), (3, 52)]);
    }

    #[test]
    fn an_anchor_gives_way_when_the_axis_has_no_slack_for_it() {
        let items = vec![item(0, 20, Some(32)), item(1, 20, None), item(2, 24, None)];
        assert_eq!(placed(items, 64), vec![(0, 20), (1, 0), (2, 40)]);
    }

    #[test]
    fn items_never_leave_the_axis_or_overlap() {
        let items = vec![
            item(0, 30, Some(60)),
            item(1, 30, Some(40)),
            item(2, 4, None),
        ];
        let laid = placed(items, 64);
        assert_eq!(laid, vec![(0, 34), (1, 4), (2, 0)]);
    }
}
