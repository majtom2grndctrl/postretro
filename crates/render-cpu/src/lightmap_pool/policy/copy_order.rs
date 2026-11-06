// Copy order for a compaction: moves as a dependency graph, staged through the pool's one spare.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// Works on one pool's blocks, wholly on the CPU, before any copy records: it
// simulates the layout hop by hop, so its copy list is valid by construction.
// A move runs once its destination is clear. WebGPU copies only between
// distinct subresources, so a block with a new place in its own layer takes
// two hops. When every move waits on another, a blocker stages in the spare to
// break the cycle; blockers bound for one layer share the spare at their
// destination coordinates, and a block the spare cannot hold parks in free
// pool space. Nothing here allocates texture memory: the spare layer is the
// whole staging budget.
//
// This is a search, not a proof. Nothing shows an order always exists through
// one spare, and the search can miss one that does: `Err` means none was
// found. `compaction_proptests` pins how far that holds on small generated
// pools; a real pool of a few hundred large blocks near 90% full is where it
// has been seen to fail, and `compaction_fixtures` keeps that graph.

use super::super::BlockPlacement;
use super::plan::PoolCopy;

/// One surviving block of the pool: where its texels are, and where the
/// target layout wants them. `src == dst` holds a block in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BlockMove {
    pub src: BlockPlacement,
    pub dst: BlockPlacement,
    pub width: u32,
    pub height: u32,
}

/// No order through the one spare was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct NoCopyOrder;

/// What one planned order took, for diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyOrderStats {
    /// Blocks with a new place, whatever it took to get them there.
    pub moved_blocks: u32,
    /// Blockers staged to break a dependency cycle.
    pub cycles_staged: u32,
    /// Hops into free pool space because the spare could not hold the block.
    pub parked: u32,
    /// Most blocks the spare held at once.
    pub peak_spare_blocks: u32,
    /// Orders tried before one reached the target.
    pub attempts: u32,
}

/// Reused buffers, so repeated compactions keep their capacity.
#[derive(Debug, Default)]
pub(super) struct CopyOrderScratch {
    at: Vec<BlockPlacement>,
    /// Block indices per array layer, the spare last.
    occupants: Vec<Vec<u32>>,
    waiting: Vec<u32>,
    /// Blocks still bound for each layer, however far along.
    arrivals: Vec<Vec<u32>>,
    /// Per block: how much staging it frees. [`RANKED_OUT`] for one with
    /// nowhere to go this round.
    rank: Vec<u32>,
    /// Per layer: texels in the way of its arrivals, and whether any wait.
    layer_cost: Vec<(u64, bool)>,
}

impl CopyOrderScratch {
    #[cfg(test)]
    pub(super) fn capacities(&self) -> [usize; 6] {
        [
            self.at.capacity(),
            self.occupants.capacity(),
            self.waiting.capacity(),
            self.arrivals.capacity(),
            self.rank.capacity(),
            self.layer_cost.capacity(),
        ]
    }
}

/// Hops an attempt may take without landing a block before it gives up: the
/// stop for a search that only circles.
const STALL_HOPS: usize = 128;

/// Orders tried for a move graph of a few large blocks, where each costs
/// next to nothing and the first is likeliest to fail.
const MAX_ATTEMPTS: usize = 256;

/// Bounds a whole search, against the square of the moving blocks since an
/// attempt costs about that: a graph with no order costs the frame about what
/// a few orders do, however many blocks it has.
const ATTEMPT_BUDGET: usize = 1 << 16;

const FEWEST_ATTEMPTS: usize = 4;

/// Blockers tried before an attempt gives up on breaking a cycle: when the
/// best few have nowhere to go, the rest rarely do, and each costs a search of
/// every layer.
const BLOCKERS_PER_BREAK: usize = 32;

const RANKED_OUT: u32 = u32::MAX;

/// A blocker in the focus layer outranks any count of moves it frees.
const FOCUS_RANK: u32 = 1 << 16;

/// Append to `copies` an order that takes every block of `moves` from `src`
/// to `dst`, or leave it as it was and report that none was found.
///
/// `moves` lists every surviving block, held ones included: each is a rect a
/// hop must not land on. Blocks being dropped or uploaded later are not in it,
/// so their rects count as free. `spare` is the staging layer's array index,
/// one past the pool's usable layers, and must be empty. `spare_edge` is how
/// much of it, from the top left, a block may stage in: the pool passes
/// `edge`, and a test narrows it to reach the failure path. `first_order_only`
/// stops after one attempt, for a caller with a cheaper target to try next.
pub(super) fn plan_copy_order(
    moves: &[BlockMove],
    spare: u32,
    edge: u32,
    spare_edge: u32,
    first_order_only: bool,
    scratch: &mut CopyOrderScratch,
    copies: &mut Vec<PoolCopy>,
) -> Result<CopyOrderStats, NoCopyOrder> {
    let start = copies.len();
    let moving = moves.iter().filter(|m| m.src != m.dst).count();
    let attempts = if first_order_only {
        1
    } else {
        (ATTEMPT_BUDGET / moving.max(1).pow(2)).clamp(FEWEST_ATTEMPTS, MAX_ATTEMPTS)
    };
    for attempt in 0..attempts as u32 {
        let mut search = Search {
            moves,
            spare,
            edge,
            spare_edge,
            attempt,
            scratch: &mut *scratch,
            copies: &mut *copies,
            stats: CopyOrderStats::default(),
        };
        search.begin();
        if search.run() {
            return Ok(CopyOrderStats {
                attempts: attempt + 1,
                ..search.stats
            });
        }
        copies.truncate(start);
    }
    Err(NoCopyOrder)
}

struct Search<'a> {
    moves: &'a [BlockMove],
    spare: u32,
    edge: u32,
    spare_edge: u32,
    attempt: u32,
    scratch: &'a mut CopyOrderScratch,
    copies: &'a mut Vec<PoolCopy>,
    stats: CopyOrderStats,
}

impl Search<'_> {
    fn begin(&mut self) {
        let layers = self.spare as usize + 1;
        let s = &mut *self.scratch;
        s.at.clear();
        s.at.extend(self.moves.iter().map(|m| m.src));
        s.occupants.iter_mut().for_each(Vec::clear);
        s.arrivals.iter_mut().for_each(Vec::clear);
        if s.occupants.len() < layers {
            s.occupants.resize_with(layers, Vec::new);
            s.arrivals.resize_with(layers, Vec::new);
        }
        s.waiting.clear();
        s.rank.clear();
        s.rank.resize(self.moves.len(), 0);
        for (i, m) in self.moves.iter().enumerate() {
            s.occupants[m.src.layer as usize].push(i as u32);
            if m.src != m.dst {
                s.waiting.push(i as u32);
                s.arrivals[m.dst.layer as usize].push(i as u32);
            }
        }
        self.stats.moved_blocks = s.waiting.len() as u32;
    }

    fn run(&mut self) -> bool {
        let mut fewest_waiting = self.scratch.waiting.len();
        let mut landed_at = self.copies.len();
        loop {
            self.run_clear_moves();
            let waiting = self.scratch.waiting.len();
            if waiting == 0 {
                return true;
            }
            if waiting < fewest_waiting {
                fewest_waiting = waiting;
                landed_at = self.copies.len();
            }
            if self.copies.len() - landed_at > STALL_HOPS {
                return false;
            }
            if !self.hop_a_same_layer_block() && !self.break_a_cycle() {
                return false;
            }
        }
    }

    /// Run every move whose destination is clear, until none is.
    fn run_clear_moves(&mut self) {
        loop {
            let mut progressed = false;
            let mut at = 0;
            while at < self.scratch.waiting.len() {
                let block = self.scratch.waiting[at];
                let dst = self.moves[block as usize].dst;
                if self.scratch.at[block as usize].layer != dst.layer && self.clear(block, dst) {
                    self.hop(block, dst);
                    self.scratch.waiting.swap_remove(at);
                    progressed = true;
                } else {
                    at += 1;
                }
            }
            if !progressed {
                return;
            }
        }
    }

    /// A block whose destination in its own layer is clear needs only the
    /// hop out: the next clear-move pass lands it.
    fn hop_a_same_layer_block(&mut self) -> bool {
        for at in 0..self.scratch.waiting.len() {
            let block = self.scratch.waiting[at];
            let dst = self.moves[block as usize].dst;
            if self.scratch.at[block as usize].layer == dst.layer && self.clear(block, dst) {
                return self.stage_or_park(block);
            }
        }
        false
    }

    /// Every remaining move waits on another: stage a blocker. Even attempts
    /// clear one layer at a time, so the spare is not left holding part of
    /// several layers; odd ones rank every blocker by the moves it frees.
    fn break_a_cycle(&mut self) -> bool {
        self.rank_sole_blockers();
        if self.attempt.is_multiple_of(2) {
            self.rank_focus_layer();
        }
        for _ in 0..BLOCKERS_PER_BREAK {
            let Some(block) = self.best_blocker() else {
                return false;
            };
            if self.stage_or_park(block) {
                self.stats.cycles_staged += 1;
                return true;
            }
            self.scratch.rank[block as usize] = RANKED_OUT;
        }
        false
    }

    /// Rank each blocker by the moves it alone holds up.
    fn rank_sole_blockers(&mut self) {
        self.scratch.rank.fill(0);
        for at in 0..self.scratch.waiting.len() {
            let block = self.scratch.waiting[at];
            let dst = self.moves[block as usize].dst;
            if let Some(sole) = self.sole_blocker(block, dst) {
                self.scratch.rank[sole as usize] += 1;
            }
        }
    }

    fn sole_blocker(&self, block: u32, dst: BlockPlacement) -> Option<u32> {
        let mut blockers = self.blockers(block, dst);
        blockers.next().filter(|_| blockers.next().is_none())
    }

    /// Raise every blocker of the layer with the fewest texels in the way of
    /// its arrivals: the cheapest layer to finish.
    fn rank_focus_layer(&mut self) {
        let layers = self.spare as usize;
        self.scratch.layer_cost.clear();
        self.scratch.layer_cost.resize(layers, (0, false));
        for at in 0..self.scratch.waiting.len() {
            let block = self.scratch.waiting[at];
            let dst = self.moves[block as usize].dst;
            let cost: u64 = self
                .blockers(block, dst)
                .map(|other| self.area(other))
                .sum();
            let entry = &mut self.scratch.layer_cost[dst.layer as usize];
            *entry = (entry.0 + cost, true);
        }
        let costs = &self.scratch.layer_cost;
        let focus = (0..layers as u32)
            .filter(|&layer| costs[layer as usize].1)
            .min_by_key(|&layer| (costs[layer as usize].0, self.noise(layer)));
        let Some(focus) = focus else {
            return;
        };
        for at in 0..self.scratch.waiting.len() {
            let block = self.scratch.waiting[at];
            let dst = self.moves[block as usize].dst;
            if dst.layer != focus {
                continue;
            }
            for k in 0..self.scratch.occupants[focus as usize].len() {
                let other = self.scratch.occupants[focus as usize][k];
                if self.covers(other, block, dst) {
                    let rank = &mut self.scratch.rank[other as usize];
                    *rank = rank.saturating_add(FOCUS_RANK);
                }
            }
        }
    }

    /// The waiting block outside the spare to stage next: the first attempt
    /// takes the highest rank, smallest first among equals, so the spare
    /// keeps room for the next one; later attempts draw among equals.
    fn best_blocker(&self) -> Option<u32> {
        let s = &*self.scratch;
        s.waiting
            .iter()
            .copied()
            .filter(|&block| {
                s.rank[block as usize] != RANKED_OUT && s.at[block as usize].layer != self.spare
            })
            .max_by_key(|&block| {
                let rank = s.rank[block as usize];
                if self.attempt < 2 {
                    (rank, u64::MAX - self.area(block), u64::from(block))
                } else {
                    (rank / FOCUS_RANK, u64::from(rank > 0), self.noise(block))
                }
            })
    }

    /// A rank that differs per block and per attempt, the same on every run.
    fn noise(&self, key: u32) -> u64 {
        let seed = (u64::from(self.attempt) << 32 | u64::from(key)).wrapping_add(1);
        let mixed = (seed ^ (seed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        let mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        mixed ^ (mixed >> 31)
    }

    /// Take `block` out of the way: into the spare packed toward the top left,
    /// which leaves its room in one piece, or, every other pair of attempts,
    /// at its destination's coordinates, where blocks bound for one layer can
    /// never collide; failing that, into free space of another pool layer.
    fn stage_or_park(&mut self, block: u32) -> bool {
        let m = self.moves[block as usize];
        let wanted = BlockPlacement {
            layer: self.spare,
            ..m.dst
        };
        let fits = m.dst.x + m.width <= self.spare_edge && m.dst.y + m.height <= self.spare_edge;
        if fits && (self.attempt / 2) % 2 == 1 && self.clear(block, wanted) {
            self.hop(block, wanted);
            return true;
        }
        if let Some(spot) = self.free_spot(block, self.spare, self.spare_edge, false) {
            self.hop(block, spot);
            return true;
        }
        // A block parked on a waiting destination only moves the blockage:
        // the search then circles. Land on one only with no other room.
        let here = self.scratch.at[block as usize].layer;
        for clear_of_arrivals in [true, false] {
            for layer in (0..self.spare).filter(|&layer| layer != here) {
                if let Some(spot) = self.free_spot(block, layer, self.edge, clear_of_arrivals) {
                    self.hop(block, spot);
                    self.stats.parked += 1;
                    return true;
                }
            }
        }
        false
    }

    /// First free place for `block` in `layer`, within `edge`, and off every
    /// waiting destination if `clear_of_arrivals`. A free rect slides up and
    /// left until it touches something, so the origin and the far edges of
    /// what it must avoid hold every candidate worth testing, and they stay
    /// aligned because every edge in the pool is.
    fn free_spot(
        &self,
        block: u32,
        layer: u32,
        edge: u32,
        clear_of_arrivals: bool,
    ) -> Option<BlockPlacement> {
        let m = &self.moves[block as usize];
        let s = &*self.scratch;
        let occupants = &s.occupants[layer as usize];
        let taken: u64 = occupants.iter().map(|&other| self.area(other)).sum();
        if taken + self.area(block) > u64::from(edge) * u64::from(edge) {
            return None;
        }
        let arrivals: &[u32] = if clear_of_arrivals {
            &s.arrivals[layer as usize]
        } else {
            &[]
        };
        let edges = |size: u32, far_at: fn(&BlockPlacement, &BlockMove) -> u32| {
            let held = occupants
                .iter()
                .map(move |&other| far_at(&s.at[other as usize], &self.moves[other as usize]));
            let bound = arrivals.iter().map(move |&other| {
                let o = &self.moves[other as usize];
                far_at(&o.dst, o)
            });
            std::iter::once(0)
                .chain(held)
                .chain(bound)
                .filter(move |at| at + size <= edge)
        };
        let is_free = |spot: BlockPlacement| {
            self.clear(block, spot)
                && !arrivals.iter().any(|&other| {
                    let o = &self.moves[other as usize];
                    other != block && s.at[other as usize] != o.dst && overlap(spot, m, o.dst, o)
                })
        };
        edges(m.height, |at, o| at.y + o.height).find_map(|y| {
            edges(m.width, |at, o| at.x + o.width)
                .map(|x| BlockPlacement { layer, x, y })
                .find(|&spot| is_free(spot))
        })
    }

    /// No block but `block` itself covers `block`'s rect at `place`.
    fn clear(&self, block: u32, place: BlockPlacement) -> bool {
        self.blockers(block, place).next().is_none()
    }

    fn blockers(&self, block: u32, place: BlockPlacement) -> impl Iterator<Item = u32> + '_ {
        self.scratch.occupants[place.layer as usize]
            .iter()
            .copied()
            .filter(move |&other| self.covers(other, block, place))
    }

    /// `other` sits on `block`'s rect at `place`.
    fn covers(&self, other: u32, block: u32, place: BlockPlacement) -> bool {
        let m = &self.moves[block as usize];
        let o = &self.moves[other as usize];
        other != block && overlap(place, m, self.scratch.at[other as usize], o)
    }

    fn area(&self, block: u32) -> u64 {
        let m = &self.moves[block as usize];
        u64::from(m.width) * u64::from(m.height)
    }

    fn hop(&mut self, block: u32, to: BlockPlacement) {
        let m = self.moves[block as usize];
        let from = self.scratch.at[block as usize];
        debug_assert_ne!(from.layer, to.layer, "a copy within one subresource");
        self.copies.push(PoolCopy {
            src: from,
            dst: to,
            width: m.width,
            height: m.height,
        });
        let occupants = &mut self.scratch.occupants;
        let was = occupants[from.layer as usize]
            .iter()
            .position(|&other| other == block)
            .expect("a block sits in its layer's occupants");
        occupants[from.layer as usize].swap_remove(was);
        occupants[to.layer as usize].push(block);
        self.scratch.at[block as usize] = to;
        let in_spare = occupants[self.spare as usize].len() as u32;
        self.stats.peak_spare_blocks = self.stats.peak_spare_blocks.max(in_spare);
    }
}

/// Whether rects of `a`'s and `b`'s extents overlap at these places, in one
/// layer.
fn overlap(at_a: BlockPlacement, a: &BlockMove, at_b: BlockPlacement, b: &BlockMove) -> bool {
    at_a.x < at_b.x + b.width
        && at_b.x < at_a.x + a.width
        && at_a.y < at_b.y + b.height
        && at_b.y < at_a.y + a.height
}
