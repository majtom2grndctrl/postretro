//! A freeing rectangle allocator for cell blocks, shaped like the `etagere`
//! crate's shelf allocator: each pool layer is a stack of horizontal shelves,
//! each shelf a row of spans. Freeing merges neighbouring free spans, and a
//! shelf left empty merges with empty neighbours, so space is reusable.
//!
//! The bake's `MaxRects` cannot free; this stands in for the runtime
//! allocator in the fragmentation simulation, without a crate dependency.

/// An allocated rectangle. `owner` is unique per allocation for the pool's
/// lifetime, so a slot kept after its block was freed cannot free whatever
/// the allocator later placed at the same origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Slot {
    pub layer: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub owner: u64,
}

/// A free that names no live allocation: stale, doubled, or mis-sized.
/// Refused without touching the layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StaleFree;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Owner {
    id: u64,
    /// Allocated height; a shelf can be taller than what it holds.
    height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    x: u32,
    width: u32,
    owner: Option<Owner>,
}

impl Span {
    fn used(&self) -> bool {
        self.owner.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Shelf {
    y: u32,
    height: u32,
    /// Contiguous, ascending by `x`, covering the layer width.
    spans: Vec<Span>,
}

impl Shelf {
    fn is_empty(&self) -> bool {
        self.spans.len() == 1 && !self.spans[0].used()
    }

    /// First free span at least `width` wide.
    fn free_span(&self, width: u32) -> Option<usize> {
        self.spans
            .iter()
            .position(|s| !s.used() && s.width >= width)
    }
}

/// One pool layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShelfLayer {
    width: u32,
    /// Contiguous, ascending by `y`, covering the layer height.
    shelves: Vec<Shelf>,
    allocations: usize,
}

impl ShelfLayer {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            shelves: vec![Self::empty_shelf(0, height, width)],
            allocations: 0,
        }
    }

    fn empty_shelf(y: u32, height: u32, width: u32) -> Shelf {
        Shelf {
            y,
            height,
            spans: vec![Span {
                x: 0,
                width,
                owner: None,
            }],
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.allocations == 0
    }

    /// Place `width × height` for `owner`, returning its top-left. Prefers
    /// an occupied shelf that wastes at most half the item's height, then
    /// splits the tightest empty shelf, then any occupied shelf that fits.
    /// Refuses a zero-width or zero-height request.
    pub(crate) fn allocate(&mut self, width: u32, height: u32, owner: u64) -> Option<(u32, u32)> {
        if width == 0 || height == 0 {
            return None;
        }
        let occupied = |slack: u32| {
            self.shelves
                .iter()
                .enumerate()
                .filter(|(_, s)| !s.is_empty() && s.height >= height && s.height - height <= slack)
                .filter_map(|(i, s)| s.free_span(width).map(|span| (s.height, i, span)))
                .min()
        };
        let chosen = occupied(height / 2)
            .map(|(_, shelf, span)| (shelf, span))
            .or_else(|| {
                self.shelves
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.is_empty() && s.height >= height && self.width >= width)
                    .min_by_key(|(i, s)| (s.height, *i))
                    .map(|(i, _)| (i, 0))
            })
            .or_else(|| occupied(u32::MAX).map(|(_, shelf, span)| (shelf, span)))?;
        let (shelf_index, span_index) = chosen;
        if self.shelves[shelf_index].is_empty() && self.shelves[shelf_index].height > height {
            let shelf = &mut self.shelves[shelf_index];
            let rest = Self::empty_shelf(shelf.y + height, shelf.height - height, self.width);
            shelf.height = height;
            self.shelves.insert(shelf_index + 1, rest);
        }
        let shelf = &mut self.shelves[shelf_index];
        let span = shelf.spans[span_index];
        shelf.spans[span_index] = Span {
            x: span.x,
            width,
            owner: Some(Owner { id: owner, height }),
        };
        if span.width > width {
            shelf.spans.insert(
                span_index + 1,
                Span {
                    x: span.x + width,
                    width: span.width - width,
                    owner: None,
                },
            );
        }
        self.allocations += 1;
        Some((span.x, shelf.y))
    }

    /// Free `owner`'s `width × height` allocation at `(x, y)`. Any mismatch
    /// (no such span, another owner, a different size) is refused.
    pub(crate) fn free(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        owner: u64,
    ) -> Result<(), StaleFree> {
        let shelf_index = self
            .shelves
            .iter()
            .position(|s| s.y == y)
            .ok_or(StaleFree)?;
        let shelf = &mut self.shelves[shelf_index];
        let expected = Some(Owner { id: owner, height });
        let mut span_index = shelf
            .spans
            .iter()
            .position(|s| s.x == x && s.width == width && s.owner == expected)
            .ok_or(StaleFree)?;
        shelf.spans[span_index].owner = None;
        self.allocations -= 1;
        if span_index + 1 < shelf.spans.len() && !shelf.spans[span_index + 1].used() {
            let next = shelf.spans.remove(span_index + 1);
            shelf.spans[span_index].width += next.width;
        }
        if span_index > 0 && !shelf.spans[span_index - 1].used() {
            let this = shelf.spans.remove(span_index);
            span_index -= 1;
            shelf.spans[span_index].width += this.width;
        }
        if !shelf.is_empty() {
            return Ok(());
        }
        let mut index = shelf_index;
        if index + 1 < self.shelves.len() && self.shelves[index + 1].is_empty() {
            let next = self.shelves.remove(index + 1);
            self.shelves[index].height += next.height;
        }
        if index > 0 && self.shelves[index - 1].is_empty() {
            let this = self.shelves.remove(index);
            index -= 1;
            self.shelves[index].height += this.height;
        }
        Ok(())
    }
}

/// Square pool layers, optionally capped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockPool {
    edge: u32,
    layers: Vec<ShelfLayer>,
    max_layers: Option<usize>,
    /// Never reset, `clear` included, so no slot outlives its owner id.
    next_owner: u64,
}

impl BlockPool {
    pub(crate) fn new(edge: u32, max_layers: Option<usize>) -> Self {
        Self {
            edge,
            layers: Vec::new(),
            max_layers,
            next_owner: 0,
        }
    }

    /// First fit by layer index, opening a layer when none fits and the cap
    /// allows. Refuses a zero-width or zero-height request.
    pub(crate) fn allocate(&mut self, width: u32, height: u32) -> Option<Slot> {
        self.allocate_within(width, height, self.max_layers)
    }

    /// `allocate` bounded to layers below `limit` instead of the pool cap:
    /// a pool that grew past a soft cap keeps placing optional blocks under it.
    pub(crate) fn allocate_within(
        &mut self,
        width: u32,
        height: u32,
        limit: Option<usize>,
    ) -> Option<Slot> {
        if width == 0 || height == 0 || width > self.edge || height > self.edge {
            return None;
        }
        let searchable = limit.map_or(self.layers.len(), |max| max.min(self.layers.len()));
        let owner = self.next_owner;
        let slot = |layer: usize, (x, y): (u32, u32)| Slot {
            layer: layer as u32,
            x,
            y,
            width,
            height,
            owner,
        };
        let placed = self.layers[..searchable]
            .iter_mut()
            .enumerate()
            .find_map(|(index, layer)| {
                layer
                    .allocate(width, height, owner)
                    .map(|at| slot(index, at))
            });
        let placed = match placed {
            Some(placed) => placed,
            None => {
                if limit.is_some_and(|max| self.layers.len() >= max) {
                    return None;
                }
                let mut layer = ShelfLayer::new(self.edge, self.edge);
                let at = layer
                    .allocate(width, height, owner)
                    .expect("a block no larger than the edge fits an empty layer");
                self.layers.push(layer);
                slot(self.layers.len() - 1, at)
            }
        };
        self.next_owner += 1;
        Some(placed)
    }

    /// Free `slot`, refusing one that no longer names its live allocation.
    pub(crate) fn free(&mut self, slot: Slot) -> Result<(), StaleFree> {
        self.layers
            .get_mut(slot.layer as usize)
            .ok_or(StaleFree)?
            .free(slot.x, slot.y, slot.width, slot.height, slot.owner)
    }

    /// Layers a pool must hold right now: one past the highest non-empty one.
    pub(crate) fn extent(&self) -> usize {
        self.layers
            .iter()
            .rposition(|layer| !layer.is_empty())
            .map_or(0, |last| last + 1)
    }

    pub(crate) fn clear(&mut self) {
        self.layers.clear();
    }
}
