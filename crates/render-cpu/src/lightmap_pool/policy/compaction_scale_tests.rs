// Compaction at real scale: the hallway's block sizes in a pool of its size, on histories.
//
// The small generated pools of `compaction_proptests` never reach what a real
// pool does: hundreds of blocks, a few of them most of a layer, near the most
// a tallest-first pack can hold. The texel mirror would not fit a 2048² pool,
// so these tests run each plan on rects: every copy must read a whole block,
// land on free space and cross layers.

use std::cell::Cell;
use std::collections::HashMap;

use super::pool_test_fixtures::*;
use super::*;

const SCALE_EDGE: u32 = 2048;
const SCALE_ALIGN: u32 = 4;

/// One block per arrival of the mix, drawn with replacement: enough for a
/// history to refill any pool here many times over.
const BLOCKS_PER_LAYER: usize = 40;

/// The hallway's block extents as `width height count` lines.
fn hallway_mix() -> Vec<(u32, u32)> {
    let text = include_str!("compaction_fixtures/hallway_block_extents.txt");
    let mut numbers = text.split_whitespace().map(|n| n.parse::<u32>().unwrap());
    let mut mix = Vec::new();
    while let (Some(width), Some(height), Some(count)) =
        (numbers.next(), numbers.next(), numbers.next())
    {
        mix.extend(std::iter::repeat_n((width, height), count as usize));
    }
    mix
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound as u64) as usize
    }
}

/// Where each block's rect sits, as the GPU layer would leave it.
#[derive(Default)]
struct RectMirror {
    blocks: HashMap<u32, (BlockPlacement, u32, u32)>,
}

impl RectMirror {
    fn execute(&mut self, plan: &DrainPlan) {
        for eviction in &plan.evicted {
            self.blocks.remove(&eviction.block);
        }
        for copy in &plan.copies {
            assert_ne!(copy.src.layer, copy.dst.layer, "a copy within one layer");
            let block = self
                .blocks
                .iter()
                .find(|(_, rect)| **rect == (copy.src, copy.width, copy.height))
                .map(|(&block, _)| block)
                .unwrap_or_else(|| panic!("no block to read at {copy:?}"));
            self.land(block, copy.dst, copy.width, copy.height);
        }
        for upload in &plan.uploads {
            let (w, h) = (
                upload.width.next_multiple_of(SCALE_ALIGN),
                upload.height.next_multiple_of(SCALE_ALIGN),
            );
            self.land(upload.block, upload.placement, w, h);
        }
    }

    fn land(&mut self, block: u32, at: BlockPlacement, width: u32, height: u32) {
        for (&other, &(o, w, h)) in &self.blocks {
            let apart = other == block
                || o.layer != at.layer
                || o.x + w <= at.x
                || at.x + width <= o.x
                || o.y + h <= at.y
                || at.y + height <= o.y;
            assert!(apart, "block {block} lands on block {other} at {at:?}");
        }
        self.blocks.insert(block, (at, width, height));
    }

    fn assert_matches(&self, model: &LightmapPoolModel) {
        assert_eq!(self.blocks.len(), model.resident_blocks().len());
        for &block in model.resident_blocks() {
            assert_eq!(
                self.blocks.get(&block).map(|&(at, _, _)| at),
                model.placement(block),
                "block {block}"
            );
        }
    }
}

/// Layers a from-scratch pack of `set` fills, if it fits at all.
fn scratch_layers(extents: &[(u32, u32)], set: &[u32]) -> u32 {
    let mut demand: Vec<(u32, u32)> = set.iter().map(|&b| extents[b as usize]).collect();
    demand.sort_unstable_by_key(|&(w, h)| (std::cmp::Reverse(h), std::cmp::Reverse(w)));
    crate::lightmap_pool::place_all_resident(&demand, SCALE_ALIGN, SCALE_EDGE)
        .map_or(u32::MAX, |pool| pool.layer_count)
}

/// `(compactions, rollbacks)` for one cell of a tally.
type Counts = Cell<(u64, u64)>;

/// Fill bands, as the last four: under 85%, 85 to 90, 90 to 95, and above.
fn fill_band(percent: u64) -> usize {
    (percent.saturating_sub(80) as usize / 5).min(3)
}

#[derive(Debug, Default)]
struct Tally {
    histories: Cell<u64>,
    drains: Cell<u64>,
    compactions: Cell<u64>,
    /// By layers the demand left spare (0, 1, 2, 3 and more), then by the
    /// demand's fill of the whole pool.
    by_pool_fill: [[Counts; 4]; 4],
    /// The same, by the demand's fill of the layers a from-scratch pack uses:
    /// how hard the packing itself was, whatever the spare layers add.
    by_packed_fill: [[Counts; 4]; 4],
    most_copies: Cell<u64>,
    /// Longest drain that compacted, and longest that rolled back.
    most_planning_micros: Cell<u64>,
    most_rollback_micros: Cell<u64>,
}

impl Tally {
    /// Rollbacks whose demand left at least `spare` layers spare.
    fn rollbacks_with_spare(&self, spare: usize) -> u64 {
        self.by_pool_fill[spare..]
            .iter()
            .flatten()
            .map(|cell| cell.get().1)
            .sum()
    }

    fn compactions_with_spare(&self, spare: usize) -> u64 {
        self.by_pool_fill[spare..]
            .iter()
            .flatten()
            .map(|cell| cell.get().0)
            .sum()
    }

    fn print(&self, layers: u32, label: &str) {
        let flat = |matrix: &[[Counts; 4]; 4]| {
            matrix
                .iter()
                .flatten()
                .map(|cell| format!("{}/{}", cell.get().0, cell.get().1))
                .collect::<Vec<_>>()
                .join(" ")
        };
        println!(
            "real-scale compaction, {layers} layers, {label}: {} histories, {} drains, {} \
             compactions, {} rollbacks ({} with a layer spare), at most {} copies, {} µs planning \
             a drain, {} µs on a rollback\n  TALLY layers={layers} label={label} \
             pool_fill[spare 0..3+ x fill <85/85/90/95] = {} | packed_fill = {}",
            self.histories.get(),
            self.drains.get(),
            self.compactions.get(),
            self.rollbacks_with_spare(0),
            self.rollbacks_with_spare(1),
            self.most_copies.get(),
            self.most_planning_micros.get(),
            self.most_rollback_micros.get(),
            flat(&self.by_pool_fill),
            flat(&self.by_packed_fill),
        );
    }
}

fn add_to(cell: &Counts, rolled_back: bool) {
    let (compactions, rollbacks) = cell.get();
    cell.set((compactions + 1, rollbacks + u64::from(rolled_back)));
}

/// How a history chooses its demand.
#[derive(Clone, Copy)]
struct Mode {
    /// Layers the demand must leave spare. At 0 the demand may fill the pool,
    /// which rarely reaches a layer spare. Above it, arrivals are kept only
    /// while the whole demand still packs into `layers - spare` layers, so a
    /// compaction at that spare is a hard one: dense, not a half-empty pool.
    spare: u32,
}

/// One history: departures a few at a time, arrivals up to what still packs,
/// and band blocks in whatever gaps they find. A rollback ends it, the pool
/// having grown.
fn run_history(seed: u64, layers: u32, steps: usize, mode: Mode, tally: &Tally) {
    let pack_limit = layers - mode.spare;
    let mix = hallway_mix();
    let mut rng = Rng::new(seed);
    let extents: Vec<(u32, u32)> = (0..BLOCKS_PER_LAYER * layers as usize)
        .map(|_| mix[rng.below(mix.len())])
        .collect();
    let mut model =
        LightmapPoolModel::new(extents.clone(), SCALE_ALIGN, SCALE_EDGE, layers).unwrap();
    assert_eq!(model.layers(), layers);
    let mut gpu = RectMirror::default();
    let mut demand: Vec<u32> = Vec::new();
    let layer_texels = u64::from(SCALE_EDGE).pow(2);

    for _ in 0..steps {
        let mut remove = Vec::new();
        // A dense mode departs less and tries more arrivals, to sit at the limit.
        let (leaving, trying) = if mode.spare == 0 {
            (rng.below(8), 1 + rng.below(12))
        } else {
            (rng.below(5), 4 + rng.below(30))
        };
        for _ in 0..leaving {
            if !demand.is_empty() {
                remove.push(demand.swap_remove(rng.below(demand.len())));
            }
        }
        let mut set = Vec::new();
        let mut ready = Vec::new();
        for _ in 0..trying {
            let block = rng.below(extents.len()) as u32;
            let free = !demand.contains(&block) && !remove.contains(&block);
            if free && !set.iter().any(|t: &LightmapTarget| t.block == block) {
                demand.push(block);
                if scratch_layers(&extents, &demand) <= pack_limit {
                    set.push(mandatory(block));
                    if !model.is_resident(block) {
                        ready.push(block);
                    }
                } else {
                    demand.pop();
                }
            }
        }
        for _ in 0..rng.below(3) {
            let block = rng.below(extents.len()) as u32;
            let free = !demand.contains(&block) && !remove.contains(&block);
            if free && !set.iter().any(|t: &LightmapTarget| t.block == block) {
                set.push(band(block, rng.below(8) as u32));
                if !model.is_resident(block) {
                    ready.push(block);
                }
            }
        }
        let start = std::time::Instant::now();
        let plan = model.plan_drain(DrainRequest {
            pool_cap_layers: layers,
            target_reset: None,
            target_set: &set,
            target_remove: &remove,
            ready: &ready,
        });
        let micros = start.elapsed().as_micros() as u64;
        tally.drains.set(tally.drains.get() + 1);
        let compacted = matches!(plan.repack, Some(RepackKind::Compaction(_)));
        let rolled_back = plan.compaction_rolled_back;
        if compacted || rolled_back {
            tally.compactions.set(tally.compactions.get() + 1);
            let packed = scratch_layers(&extents, &demand);
            let spare = (layers - packed) as usize;
            let texels: u64 = demand
                .iter()
                .map(|&b| u64::from(extents[b as usize].0) * u64::from(extents[b as usize].1))
                .sum();
            let pool_fill = texels * 100 / (u64::from(layers) * layer_texels);
            let packed_fill = texels * 100 / (u64::from(packed.max(1)) * layer_texels);
            let row = spare.min(3);
            add_to(&tally.by_pool_fill[row][fill_band(pool_fill)], rolled_back);
            add_to(
                &tally.by_packed_fill[row][fill_band(packed_fill)],
                rolled_back,
            );
            tally
                .most_copies
                .set(tally.most_copies.get().max(plan.copies.len() as u64));
            let longest = if rolled_back {
                &tally.most_rollback_micros
            } else {
                &tally.most_planning_micros
            };
            longest.set(longest.get().max(micros));
        }
        if rolled_back {
            break;
        }
        assert_eq!(plan.growth, None, "the demand packs, so it needs no growth");
        gpu.execute(plan);
        gpu.assert_matches(&model);
        assert_eq!(model.texture_allocations(), 1, "no second pool");
    }
    tally.histories.set(tally.histories.get() + 1);
}

#[test]
fn compaction_places_the_hallway_mix_at_its_real_scale() {
    let tally = Tally::default();
    for seed in 1..=6 {
        run_history(seed, 27, 200, Mode { spare: 0 }, &tally);
    }
    tally.print(27, "routine, any spare");
    // A generator that stops reaching compaction proves nothing.
    assert!(
        tally.compactions.get() >= 100,
        "only {} compactions",
        tally.compactions.get()
    );
    // A pool with no layer spare may miss and take logged growth: reported
    // above, not asserted. A layer spare must not.
    assert_eq!(tally.rollbacks_with_spare(1), 0);

    // Dense demand with exactly one layer spare, in a small pool.
    let dense = Tally::default();
    for seed in 1..=3 {
        run_history(seed, 12, 100, Mode { spare: 1 }, &dense);
    }
    dense.print(12, "routine, one spare");
    assert!(
        dense.compactions_with_spare(1) >= 20,
        "only {} compactions with a layer spare",
        dense.compactions_with_spare(1)
    );
    assert_eq!(dense.rollbacks_with_spare(1), 0);
}

/// The evidence for the fit claim at scale: what fraction of compactions roll
/// back, by spare layers and by how tight the demand is. It reports, and fails
/// only on a miss with a layer spare, which is the claim. A pool with none
/// does miss now and then, and that is reported.
#[test]
#[ignore = "real-scale hunt; set POSTRETRO_COMPACTION_SCALE_HISTORIES"]
fn compaction_hunt_at_real_scale() {
    let env = |name: &str, default: u64| {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
    };
    let histories = env("POSTRETRO_COMPACTION_SCALE_HISTORIES", 100);
    let first = env("POSTRETRO_COMPACTION_SCALE_SEED", 1000);
    let spare = env("POSTRETRO_COMPACTION_SCALE_SPARE", 0) as u32;
    let only_layers = env("POSTRETRO_COMPACTION_SCALE_LAYERS", 0) as u32;
    let steps = env("POSTRETRO_COMPACTION_SCALE_STEPS", 400) as usize;
    for layers in [12, 27, 34] {
        if only_layers != 0 && only_layers != layers {
            continue;
        }
        let tally = Tally::default();
        for seed in 0..histories {
            run_history(first + seed, layers, steps, Mode { spare }, &tally);
        }
        tally.print(layers, &format!("hunt seed {first} spare {spare}"));
        assert_eq!(
            tally.rollbacks_with_spare(1),
            0,
            "a compaction missed with a layer spare: seed range {first}.., {layers} layers"
        );
    }
}

// The pool of a far-point tour at 27 layers when a 1908×800 block arrived: 247
// residents, 89.8% full, and no block's shelf with room for it.
#[test]
fn the_hallway_tour_at_its_27_layer_bound_compacts_without_growth() {
    let text = include_str!("compaction_fixtures/hallway_tour_27_layers.txt");
    let mut numbers = text.split_whitespace().map(|n| n.parse::<u32>().unwrap());
    let mut next = || numbers.next().unwrap();
    let (edge, layers, cap, residents, pending) = (next(), next(), next(), next(), next());
    assert_eq!(edge, SCALE_EDGE);
    let mut placed: Vec<(u32, u32, u32, u32, u32, bool)> = (0..residents)
        .map(|_| {
            let (_id, layer, x, y, w, h, band, _lead) = (
                next(),
                next(),
                next(),
                next(),
                next(),
                next(),
                next(),
                next(),
            );
            (layer, x, y, w, h, band != 0)
        })
        .collect();
    // Shelves replay top down, each from its tallest block.
    placed.sort_unstable_by_key(|&(layer, x, y, _, h, _)| (layer, y, std::cmp::Reverse(h), x));
    let mut extents: Vec<(u32, u32)> = placed.iter().map(|p| (p.3, p.4)).collect();
    let arrivals: Vec<u32> = (0..pending)
        .map(|at| {
            let (_id, w, h) = (next(), next(), next());
            extents.push((w, h));
            residents + at
        })
        .collect();
    let mut model = LightmapPoolModel::new(extents, SCALE_ALIGN, edge, cap).unwrap();
    assert_eq!(model.layers(), layers);
    let targets: Vec<LightmapTarget> = (0..residents)
        .map(|block| {
            if placed[block as usize].5 {
                band(block, 1)
            } else {
                mandatory(block)
            }
        })
        .collect();
    model.plan_drain(DrainRequest {
        pool_cap_layers: cap,
        target_reset: None,
        target_set: &targets,
        target_remove: &[],
        ready: &[],
    });
    // The dumped layout is the pool's, so nothing holds room in it yet.
    model.return_all_room();
    let mut gpu = RectMirror::default();
    for (block, &(layer, x, y, w, h, _)) in placed.iter().enumerate() {
        let slot = model.pool.allocate_at(layer, x, y, w, h).unwrap();
        model.place(block as u32, slot);
        gpu.land(block as u32, placement_of(slot), w, h);
    }

    let set: Vec<LightmapTarget> = arrivals.iter().map(|&block| mandatory(block)).collect();
    let plan = model.plan_drain(DrainRequest {
        pool_cap_layers: cap,
        target_reset: None,
        target_set: &set,
        target_remove: &[],
        ready: &arrivals,
    });
    assert!(matches!(plan.repack, Some(RepackKind::Compaction(_))));
    assert!(!plan.compaction_rolled_back);
    assert_eq!(plan.growth, None);
    assert_eq!(plan.installed, arrivals);
    gpu.execute(plan);
    gpu.assert_matches(&model);
}
