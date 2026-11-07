// Copy order for a compaction: target layers built one at a time, through one layer kept empty.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// Works on one pool's blocks, wholly on the CPU, before any copy records: it
// simulates the layout hop by hop, so its copy list is valid by construction.
// The target's layers are not tied to pool layers. The spare starts empty, so
// the first target layer builds there: every block lands in one pass, with no
// cycle to break, wherever it came from. Then the least-full open layer parks
// its blocks in free space elsewhere, and that layer, now empty, takes the next
// target layer. A target layer that finds no empty layer builds over the open
// layer with the fewest texels in its way, parking only those. Each hop crosses
// layers, as WebGPU requires. A target layer left on the spare's index moves to
// the one pool layer left open, so the spare ends empty, where the texture
// keeps it. Nothing here allocates texture memory: the spare layer is the whole
// staging budget.
//
// Area never stops this. The array has a layer more than the target fills, so
// free space always adds up to a whole layer, and a layer's blocks always fit
// it by area. Fragmentation can: a block can find no hole. `Err` means that
// happened, not that no order exists.

use std::cmp::Reverse;

use super::super::BlockPlacement;
use super::plan::PoolCopy;

/// One block a compaction plans around.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BlockMove {
    /// Where its texels are. `None` for a block still to upload: it needs its
    /// target rect clear, and no copy.
    pub src: Option<BlockPlacement>,
    /// Its rect in the target, whose `layer` numbers the target's layers, not
    /// the pool's.
    pub dst: BlockPlacement,
    pub width: u32,
    pub height: u32,
}

/// What a compaction plans over.
#[derive(Debug, Clone, Copy)]
pub(super) struct CopyOrderInput<'a> {
    /// Every surviving block. Blocks being dropped are not in it, so their
    /// rects count as free.
    pub moves: &'a [BlockMove],
    /// The pool's usable layers, and the target's. The spare is the array
    /// index past them, and must be empty.
    pub layers: u32,
    pub edge: u32,
}

/// No order through the one spare was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct NoCopyOrder;

/// What one planned order took, for diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyOrderStats {
    /// Blocks with a new place, whatever it took to get them there.
    pub moved_blocks: u32,
    /// Hops into free space, out of a layer that had to clear.
    pub parked: u32,
    /// Target layers built over a layer that could not be emptied first.
    pub built_over: u32,
}

/// One simulated hop, kept so a failed step can be undone.
#[derive(Debug, Clone, Copy)]
struct Hop {
    block: u32,
    from: Option<BlockPlacement>,
    parked: bool,
}

/// Reused buffers, so repeated compactions keep their capacity.
#[derive(Debug, Default)]
pub(super) struct CopyOrderScratch {
    at: Vec<Option<BlockPlacement>>,
    /// Block indices per array layer, the spare last.
    occupants: Vec<Vec<u32>>,
    /// Texels held per array layer.
    used: Vec<u64>,
    /// Layers no target layer has claimed yet.
    open: Vec<bool>,
    /// Block indices per target layer.
    by_target: Vec<Vec<u32>>,
    /// Target layers not built yet, tallest first.
    todo: Vec<u32>,
    /// Blocks of the target layer being built, not yet landed.
    landing: Vec<u32>,
    /// `(sort key, tie break, layer)` of the layers a step can use.
    ranked: Vec<(u64, Reverse<u64>, u32)>,
    /// Blocks a step must clear, largest last.
    clearing: Vec<u32>,
    /// `(texels held, layer)`, the fullest first, for one park.
    fullest: Vec<(Reverse<u64>, u32)>,
    /// Candidate `y`s of one free-space search, and the `(start, end)` of what
    /// crosses one of them.
    rows: Vec<u32>,
    spans: Vec<(u32, u32)>,
    hops: Vec<Hop>,
    /// The pool layer each target layer landed on.
    pub homes: Vec<u32>,
}

impl CopyOrderScratch {
    #[cfg(test)]
    pub(super) fn capacities(&self) -> [usize; 14] {
        [
            self.at.capacity(),
            self.occupants.capacity(),
            self.used.capacity(),
            self.open.capacity(),
            self.by_target.capacity(),
            self.todo.capacity(),
            self.landing.capacity(),
            self.ranked.capacity(),
            self.clearing.capacity(),
            self.fullest.capacity(),
            self.rows.capacity(),
            self.spans.capacity(),
            self.hops.capacity(),
            self.homes.capacity(),
        ]
    }
}

/// The order target layers build in. Tallest first clears the blocks hardest
/// to park; a pool that still jams, usually on a wide short block near the end,
/// nearly always clears the other way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuildOrder {
    TallestFirst,
    ShortestFirst,
}

/// Least-full layers tried for clearing after each target layer: past the
/// first few, a layer that cannot clear only costs the frame.
const LAYERS_TRIED_FOR_CLEARING: usize = 4;

/// Free-space search an order may spend per block, in candidate rows squared,
/// before it gives up: planning runs on the frame thread, and a pool that jams
/// retries every target layer on every open layer. It is checked only between
/// the candidate layers a target layer tries, never inside a step, so an order
/// can overrun it: a landing that spends it runs to its end, and clearing a
/// layer after a build never checks it. Measured on the hallway's block mix, a
/// tighter budget starts losing orders that exist.
const WORK_PER_BLOCK: usize = 8192;

/// Append to `copies` an order that takes every block to its target rect and
/// leaves `scratch.homes` naming each target layer's pool layer, or leave
/// `copies` as it was and report that none was found.
pub(super) fn plan_copy_order(
    input: CopyOrderInput<'_>,
    scratch: &mut CopyOrderScratch,
    copies: &mut Vec<PoolCopy>,
) -> Result<CopyOrderStats, NoCopyOrder> {
    let start = copies.len();
    for build_order in [BuildOrder::TallestFirst, BuildOrder::ShortestFirst] {
        let mut order = Order {
            input,
            s: &mut *scratch,
            copies: &mut *copies,
            built_over: 0,
            work_left: WORK_PER_BLOCK * input.moves.len(),
        };
        order.begin(build_order);
        if order.run() {
            return Ok(order.stats());
        }
        copies.truncate(start);
    }
    Err(NoCopyOrder)
}

struct Order<'a> {
    input: CopyOrderInput<'a>,
    s: &'a mut CopyOrderScratch,
    copies: &'a mut Vec<PoolCopy>,
    built_over: u32,
    work_left: usize,
}

impl Order<'_> {
    fn begin(&mut self, build_order: BuildOrder) {
        let CopyOrderInput { moves, layers, .. } = self.input;
        let targets = layers as usize;
        let array_layers = targets + 1;
        let s = &mut *self.s;
        s.at.clear();
        s.at.extend(moves.iter().map(|m| m.src));
        s.occupants.iter_mut().for_each(Vec::clear);
        s.occupants
            .resize_with(array_layers.max(s.occupants.len()), Vec::new);
        s.used.clear();
        s.used.resize(array_layers, 0);
        s.open.clear();
        s.open.resize(array_layers, true);
        s.by_target.iter_mut().for_each(Vec::clear);
        s.by_target
            .resize_with(targets.max(s.by_target.len()), Vec::new);
        s.homes.clear();
        s.homes.resize(targets, u32::MAX);
        s.hops.clear();
        for (block, m) in moves.iter().enumerate() {
            if let Some(src) = m.src {
                s.occupants[src.layer as usize].push(block as u32);
                s.used[src.layer as usize] += texels(m);
            }
            s.by_target[m.dst.layer as usize].push(block as u32);
        }
        s.todo.clear();
        s.todo
            .extend((0..targets as u32).filter(|&target| !s.by_target[target as usize].is_empty()));
        if build_order == BuildOrder::ShortestFirst {
            s.todo.reverse();
        }
    }

    fn run(&mut self) -> bool {
        while !self.s.todo.is_empty() {
            let Some(next) = (0..self.s.todo.len()).find(|&at| self.build(self.s.todo[at])) else {
                return false;
            };
            self.s.todo.remove(next);
            self.clear_a_layer();
        }
        self.vacate_the_spare();
        true
    }

    /// Build `target` on the open layer with the fewest texels in its way,
    /// the one holding most of it in place among equals. A layer with nothing
    /// in the way parks nothing, so it searches no free space and cannot fail:
    /// it builds even on a spent budget.
    fn build(&mut self, target: u32) -> bool {
        self.rank_homes(target);
        for at in 0..self.s.ranked.len() {
            let (in_the_way, _, layer) = self.s.ranked[at];
            if self.work_left == 0 && in_the_way > 0 {
                return false;
            }
            let mark = self.s.hops.len();
            if self.land_on(target, layer) {
                self.s.open[layer as usize] = false;
                self.s.homes[target as usize] = layer;
                self.built_over += u32::from(in_the_way > 0);
                return true;
            }
            self.undo_to(mark);
        }
        false
    }

    fn rank_homes(&mut self, target: u32) {
        let mut ranked = std::mem::take(&mut self.s.ranked);
        ranked.clear();
        for layer in 0..=self.input.layers {
            if !self.s.open[layer as usize] {
                continue;
            }
            let (mut in_the_way, mut kept) = (0, 0);
            for &other in &self.s.occupants[layer as usize] {
                let texels = texels(&self.input.moves[other as usize]);
                if self.is_landed(other, target, layer) {
                    kept += texels;
                } else if self.lies_on_target(other, target) {
                    in_the_way += texels;
                }
            }
            ranked.push((in_the_way, Reverse(kept), layer));
        }
        ranked.sort_unstable();
        self.s.ranked = ranked;
    }

    fn is_landed(&self, block: u32, target: u32, layer: u32) -> bool {
        let dst = self.input.moves[block as usize].dst;
        dst.layer == target && self.s.at[block as usize] == Some(BlockPlacement { layer, ..dst })
    }

    fn lies_on_target(&self, block: u32, target: u32) -> bool {
        self.s.by_target[target as usize]
            .iter()
            .any(|&other| other == block || self.covers_dst(block, other))
    }

    /// `block`, where it sits now, lies on `other`'s target rect.
    fn covers_dst(&self, block: u32, other: u32) -> bool {
        let moves = self.input.moves;
        let (b, o) = (&moves[block as usize], &moves[other as usize]);
        let place = self.s.at[block as usize].expect("a block on a layer has a place");
        other != block && overlap(place, b, o.dst, o)
    }

    /// Land every block of `target` on `layer`: at each turn the one with the
    /// fewest texels in its way, the largest among equals, parking what is.
    fn land_on(&mut self, target: u32, layer: u32) -> bool {
        let mut landing = std::mem::take(&mut self.s.landing);
        landing.clear();
        landing.extend(
            self.s.by_target[target as usize]
                .iter()
                .copied()
                .filter(|&block| !self.is_landed(block, target, layer)),
        );
        let mut landed = true;
        while !landing.is_empty() {
            let next = (0..landing.len())
                .min_by_key(|&at| {
                    let block = landing[at];
                    (
                        self.texels_in_the_way(block, layer),
                        Reverse(texels(&self.input.moves[block as usize])),
                        block,
                    )
                })
                .expect("not empty");
            let block = landing.swap_remove(next);
            if !self.clear_for(block, layer) {
                landed = false;
                break;
            }
            let dst = self.input.moves[block as usize].dst;
            self.hop(block, BlockPlacement { layer, ..dst }, false);
        }
        self.s.landing = landing;
        landed
    }

    fn texels_in_the_way(&self, block: u32, layer: u32) -> u64 {
        let own = self.s.at[block as usize].filter(|at| at.layer == layer);
        let block_texels = own.map_or(0, |_| texels(&self.input.moves[block as usize]));
        self.s.occupants[layer as usize]
            .iter()
            .filter(|&&other| self.covers_dst(other, block))
            .map(|&other| texels(&self.input.moves[other as usize]))
            .sum::<u64>()
            + block_texels
    }

    /// Park whatever lies on `block`'s rect in `layer`, and `block` itself if
    /// it sits in `layer` already: a copy cannot stay within one layer.
    fn clear_for(&mut self, block: u32, layer: u32) -> bool {
        let mut clearing = std::mem::take(&mut self.s.clearing);
        clearing.clear();
        clearing.extend(
            self.s.occupants[layer as usize]
                .iter()
                .copied()
                .filter(|&other| other == block || self.covers_dst(other, block)),
        );
        let parked = self.park_all(&mut clearing, layer);
        self.s.clearing = clearing;
        parked
    }

    /// Keep a layer empty for the next target layer, if one can clear.
    fn clear_a_layer(&mut self) {
        if self.s.todo.is_empty() {
            return;
        }
        let mut ranked = std::mem::take(&mut self.s.ranked);
        ranked.clear();
        ranked.extend(
            (0..=self.input.layers)
                .filter(|&layer| self.s.open[layer as usize])
                .map(|layer| (self.s.used[layer as usize], Reverse(0), layer)),
        );
        if ranked.iter().any(|&(used, _, _)| used == 0) {
            self.s.ranked = ranked;
            return;
        }
        ranked.sort_unstable();
        ranked.truncate(LAYERS_TRIED_FOR_CLEARING);
        for &(_, _, layer) in &ranked {
            let mark = self.s.hops.len();
            let mut clearing = std::mem::take(&mut self.s.clearing);
            clearing.clear();
            clearing.extend_from_slice(&self.s.occupants[layer as usize]);
            let cleared = self.park_all(&mut clearing, layer);
            self.s.clearing = clearing;
            if cleared {
                break;
            }
            self.undo_to(mark);
        }
        self.s.ranked = ranked;
    }

    /// Park `blocks`, all in `layer`, largest first: a small block fits the
    /// holes a large one leaves behind, and not the other way round.
    fn park_all(&mut self, blocks: &mut [u32], layer: u32) -> bool {
        let moves = self.input.moves;
        blocks.sort_unstable_by_key(|&block| (Reverse(texels(&moves[block as usize])), block));
        blocks.iter().all(|&block| self.park(block, layer))
    }

    /// Hop `block` out of `from` into the fullest layer with a hole for it:
    /// the emptier layers keep their room in one piece.
    fn park(&mut self, block: u32, from: u32) -> bool {
        let mut fullest = std::mem::take(&mut self.s.fullest);
        fullest.clear();
        fullest.extend(
            (0..=self.input.layers)
                .filter(|&layer| layer != from)
                .map(|layer| (Reverse(self.s.used[layer as usize]), layer)),
        );
        fullest.sort_unstable();
        let spot = fullest
            .iter()
            .find_map(|&(_, layer)| self.free_spot(block, layer));
        self.s.fullest = fullest;
        match spot {
            Some(spot) => {
                self.hop(block, spot, true);
                true
            }
            None => false,
        }
    }

    /// Topmost, then leftmost, free place for `block` in `layer`. A free rect
    /// slides up until it touches something, so the top and the far edges of
    /// what is in the layer hold every row worth testing, and they stay aligned
    /// because every edge in the pool is.
    fn free_spot(&mut self, block: u32, layer: u32) -> Option<BlockPlacement> {
        let m = self.input.moves[block as usize];
        let edge = self.input.edge;
        if self.s.used[layer as usize] + texels(&m) > u64::from(edge) * u64::from(edge) {
            return None;
        }
        let mut rows = std::mem::take(&mut self.s.rows);
        rows.clear();
        rows.push(0);
        rows.extend(self.placed(layer).map(|(at, o)| at.y + o.height));
        rows.sort_unstable();
        rows.dedup();
        self.work_left = self.work_left.saturating_sub(rows.len().pow(2));
        let spot = rows
            .iter()
            .take_while(|&&y| y + m.height <= edge)
            .find_map(|&y| {
                self.free_x_in_row(&m, layer, y)
                    .map(|x| BlockPlacement { layer, x, y })
            });
        self.s.rows = rows;
        spot
    }

    /// Leftmost `x` where `m` fits between what crosses the row at `y`.
    fn free_x_in_row(&mut self, m: &BlockMove, layer: u32, y: u32) -> Option<u32> {
        let mut spans = std::mem::take(&mut self.s.spans);
        spans.clear();
        spans.extend(
            self.placed(layer)
                .filter(|(at, o)| at.y < y + m.height && y < at.y + o.height)
                .map(|(at, o)| (at.x, at.x + o.width)),
        );
        spans.sort_unstable();
        let mut x = 0;
        for &(start, end) in &spans {
            if x + m.width <= start {
                break;
            }
            x = x.max(end);
        }
        self.s.spans = spans;
        (x + m.width <= self.input.edge).then_some(x)
    }

    fn placed(&self, layer: u32) -> impl Iterator<Item = (BlockPlacement, &BlockMove)> + '_ {
        self.s.occupants[layer as usize].iter().map(|&other| {
            let at = self.s.at[other as usize].expect("an occupant has a place");
            (at, &self.input.moves[other as usize])
        })
    }

    /// A target layer left on the spare's index moves to the pool layer no
    /// target layer claimed, at the same coordinates.
    fn vacate_the_spare(&mut self) {
        let spare = self.input.layers;
        let Some(target) = self.s.homes.iter().position(|&home| home == spare) else {
            return;
        };
        let free = (0..spare)
            .find(|&layer| self.s.open[layer as usize])
            .expect("a target layer on the spare leaves a pool layer unclaimed");
        for at in 0..self.s.by_target[target].len() {
            let block = self.s.by_target[target][at];
            let dst = self.input.moves[block as usize].dst;
            self.hop(block, BlockPlacement { layer: free, ..dst }, false);
        }
        self.s.homes[target] = free;
    }

    fn hop(&mut self, block: u32, to: BlockPlacement, parked: bool) {
        let m = self.input.moves[block as usize];
        let from = self.s.at[block as usize];
        if let Some(from) = from {
            debug_assert_ne!(from.layer, to.layer, "a copy within one subresource");
            self.leave(block, from.layer);
        }
        // A block with no texels yet takes its place without a copy.
        if let (Some(from), true) = (from, m.src.is_some()) {
            self.copies.push(PoolCopy {
                src: from,
                dst: to,
                width: m.width,
                height: m.height,
            });
        }
        self.enter(block, to);
        self.s.hops.push(Hop {
            block,
            from,
            parked,
        });
    }

    fn undo_to(&mut self, mark: usize) {
        while self.s.hops.len() > mark {
            let hop = self.s.hops.pop().expect("above the mark");
            let now = self.s.at[hop.block as usize].expect("a hop left a place");
            self.leave(hop.block, now.layer);
            if self.input.moves[hop.block as usize].src.is_some() {
                self.copies.pop();
            }
            match hop.from {
                Some(from) => self.enter(hop.block, from),
                None => self.s.at[hop.block as usize] = None,
            }
        }
    }

    fn leave(&mut self, block: u32, layer: u32) {
        let occupants = &mut self.s.occupants[layer as usize];
        let was = occupants
            .iter()
            .position(|&other| other == block)
            .expect("a block sits in its layer's occupants");
        occupants.swap_remove(was);
        self.s.used[layer as usize] -= texels(&self.input.moves[block as usize]);
    }

    fn enter(&mut self, block: u32, at: BlockPlacement) {
        self.s.occupants[at.layer as usize].push(block);
        self.s.used[at.layer as usize] += texels(&self.input.moves[block as usize]);
        self.s.at[block as usize] = Some(at);
    }

    fn stats(&self) -> CopyOrderStats {
        let moves = self.input.moves;
        let moved = (0..moves.len())
            .filter(|&block| moves[block].src.is_some() && moves[block].src != self.s.at[block]);
        CopyOrderStats {
            moved_blocks: moved.count() as u32,
            parked: self.s.hops.iter().filter(|hop| hop.parked).count() as u32,
            built_over: self.built_over,
        }
    }
}

fn texels(m: &BlockMove) -> u64 {
    u64::from(m.width) * u64::from(m.height)
}

/// Whether rects of `a`'s and `b`'s extents overlap at these places, in one
/// layer.
fn overlap(at_a: BlockPlacement, a: &BlockMove, at_b: BlockPlacement, b: &BlockMove) -> bool {
    at_a.x < at_b.x + b.width
        && at_b.x < at_a.x + a.width
        && at_a.y < at_b.y + b.height
        && at_b.y < at_a.y + a.height
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-layer pool whose block moves within its layer: the target layer
    /// builds on the empty spare, with nothing in its way, then moves off it.
    /// A budget spent before that build, as a step that emptied a layer can
    /// spend it, must not refuse a build that searches nothing.
    #[test]
    fn a_target_layer_with_nothing_in_its_way_builds_on_a_spent_budget() {
        let at = |layer, x, y| BlockPlacement { layer, x, y };
        let moves = [BlockMove {
            src: Some(at(0, 0, 0)),
            dst: at(0, 4, 0),
            width: 4,
            height: 4,
        }];
        let input = CopyOrderInput {
            moves: &moves,
            layers: 1,
            edge: 64,
        };
        let mut scratch = CopyOrderScratch::default();
        let mut copies = Vec::new();
        let mut order = Order {
            input,
            s: &mut scratch,
            copies: &mut copies,
            built_over: 0,
            work_left: 0,
        };
        order.begin(BuildOrder::TallestFirst);
        assert!(order.run(), "a build with nothing in the way cannot fail");
        let copy = |src, dst| PoolCopy {
            src,
            dst,
            width: 4,
            height: 4,
        };
        assert_eq!(
            copies,
            [
                copy(at(0, 0, 0), at(1, 4, 0)),
                copy(at(1, 4, 0), at(0, 4, 0)),
            ]
        );
        assert_eq!(scratch.homes, [0]);
    }
}
