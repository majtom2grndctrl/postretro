// Pools sized once from the compiled bound: a walk within it, a pool at the
// all-resident layers, band blocks in every layer, and what a growth reserves.

use postretro_level_format::lightmap::LightmapBlockPayload;
use postretro_level_format::lightmap_pool::{
    LIGHTMAP_POOL_MARGIN, first_generation_layers, place_all_resident,
};
use postretro_level_loader::{LightmapDrainBatch, LightmapTarget, PreparedLightmapBlock};

use super::pool_test_fixtures::*;
use super::*;

/// A small deterministic generator, so a fixture's blocks never drift.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, bound: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) % u64::from(bound)) as u32
    }

    /// A block extent on the 4-texel grid: 8 to 44 on a side.
    fn extent(&mut self) -> (u32, u32) {
        (8 + 4 * self.below(10), 8 + 4 * self.below(10))
    }
}

/// Layers the shared packing order fills for `blocks`.
fn pack_layers(extents: &[(u32, u32)], blocks: impl Iterator<Item = u32>) -> u32 {
    let set: Vec<(u32, u32)> = blocks.map(|block| extents[block as usize]).collect();
    place_all_resident(&set, ALIGN, EDGE).unwrap().layer_count
}

/// Cells in a row, each owning `BLOCKS_PER_CELL` blocks. A camera cell's
/// mandatory set reaches `REACH` cells either way; the next cells are its band.
struct Corridor {
    extents: Vec<(u32, u32)>,
}

impl Corridor {
    const CELLS: u32 = 24;
    const BLOCKS_PER_CELL: u32 = 4;
    const REACH: u32 = 2;

    fn new(seed: u64) -> Self {
        let mut lcg = Lcg(seed);
        let blocks = Self::CELLS * Self::BLOCKS_PER_CELL;
        Self {
            extents: (0..blocks).map(|_| lcg.extent()).collect(),
        }
    }

    fn cells_within(reach: u32, camera: u32) -> std::ops::RangeInclusive<u32> {
        camera.saturating_sub(reach)..=(camera + reach).min(Self::CELLS - 1)
    }

    fn blocks_of(cells: std::ops::RangeInclusive<u32>) -> impl Iterator<Item = u32> {
        cells.flat_map(|cell| {
            let first = cell * Self::BLOCKS_PER_CELL;
            first..first + Self::BLOCKS_PER_CELL
        })
    }

    fn mandatory(camera: u32) -> impl Iterator<Item = u32> {
        Self::blocks_of(Self::cells_within(Self::REACH, camera))
    }

    /// The compiled bound: the worst mandatory pack of any camera cell.
    fn bound(&self) -> u32 {
        (0..Self::CELLS)
            .map(|camera| pack_layers(&self.extents, Self::mandatory(camera)))
            .max()
            .unwrap()
    }

    /// `camera`'s targets: its mandatory set, then a band one cell further.
    fn targets(&self, camera: u32) -> Vec<LightmapTarget> {
        let mut targets: Vec<LightmapTarget> = Self::blocks_of(Self::cells_within(
            Self::REACH + 1,
            camera,
        ))
        .map(|block| band(block, 1))
        .collect();
        for block in Self::mandatory(camera) {
            let at = targets.iter().position(|t| t.block == block).unwrap();
            targets[at] = mandatory(block);
        }
        targets
    }
}

/// Walk `steps` random single-cell moves through `corridor`, every step's pairs
/// ready in its drain and each retirement released before the next, and count
/// the growths.
fn walk_growths(corridor: &Corridor, seed: u64, steps: u32, h: &mut Harness) -> u32 {
    let mut lcg = Lcg(seed);
    let mut camera = 0;
    let mut targets: Vec<LightmapTarget> = Vec::new();
    let mut growths = 0;
    for _ in 0..steps {
        let next = corridor.targets(camera);
        let set: Vec<LightmapTarget> = next
            .iter()
            .filter(|t| !targets.contains(t))
            .copied()
            .collect();
        let remove: Vec<u32> = targets
            .iter()
            .map(|t| t.block)
            .filter(|&block| !next.iter().any(|t| t.block == block))
            .collect();
        let ready: Vec<u32> = next
            .iter()
            .map(|t| t.block)
            .filter(|&block| !h.model.is_resident(block))
            .collect();
        let plan = h.drain(&set, &remove, &ready);
        assert!(plan.deferred.is_empty(), "every pair places: {:?}", plan.deferred);
        growths += u32::from(plan.pools[POOL].growth.is_some());
        h.model.release_retirement(POOL);
        targets = next;
        camera = match lcg.below(2) {
            0 => camera.saturating_sub(1),
            _ => (camera + 1).min(Corridor::CELLS - 1),
        };
    }
    growths
}

// The pool at the bound with no margin: every set the walk demands packs into
// it, so compaction alone holds it. The same walk on a downward-only repack, as
// the pool ran before compaction, grows: that is what the fixture is for.
#[test]
fn walk_within_bound_never_grows() {
    let corridor = Corridor::new(0x5eed);
    let bound = corridor.bound();

    let mut h = Harness::new(&corridor.extents, bound);
    assert_eq!(walk_growths(&corridor, 7, 600, &mut h), 0);
    assert_eq!(h.model.pool(POOL).layers(), bound);
    assert_eq!(h.model.pool(POOL).texture_allocations(), 1);

    let mut h = Harness::new(&corridor.extents, bound);
    h.model.downward_only = true;
    let growths = walk_growths(&corridor, 7, 600, &mut h);
    assert!(growths > 0, "the fixture must grow a downward-only pool");
    eprintln!("{POOL:?}: bound {bound}, {growths} growth(s) downward-only");
}

/// A whole level on the shelf: `count` blocks of mixed size.
fn mixed_level(seed: u64, count: u32) -> Vec<(u32, u32)> {
    let mut lcg = Lcg(seed);
    (0..count).map(|_| lcg.extent()).collect()
}

/// A pool at `extents`' all-resident layers, as a level whose worst cell is the
/// whole of it sizes one.
fn at_the_all_resident_cap(extents: &[(u32, u32)]) -> Harness {
    let all_resident = pack_layers(extents, 0..extents.len() as u32);
    Harness::new(extents, first_generation_layers(all_resident, all_resident))
}

fn assert_holds_every_block_without_growth(h: &Harness, extents: &[(u32, u32)], how: &str) {
    assert_eq!(
        h.model.resident_blocks().len(),
        extents.len(),
        "{how}: every block"
    );
    assert_eq!(h.model.pool(POOL).texture_allocations(), 1, "{how}: no growth");
    assert_disjoint_within_layers(&h.model);
}

// R24: this set packs into one layer in block-id order and two tallest first.
// The pool counts the bound's order, so it holds it however the blocks come.
// The mixed level is wholly mandatory from a single cell: every target lands at
// once, and its pairs arrive over any number of drains in any order.
#[test]
fn all_resident_cap_holds_every_block() {
    let r24 = [(56, 16), (28, 44), (24, 20)];
    assert_eq!(pack_layers(&r24, 0..3), 2, "tallest first");
    let mixed = mixed_level(0xca9, 120);
    for extents in [&r24[..], &mixed] {
        let count = extents.len() as u32;
        let set: Vec<LightmapTarget> = (0..count).map(mandatory).collect();

        let mut h = at_the_all_resident_cap(extents);
        let ready: Vec<u32> = (0..count).collect();
        h.drain(&set, &[], &ready);
        assert_holds_every_block_without_growth(&h, extents, "one drain");

        let mut by_height: Vec<u32> = (0..count).collect();
        by_height.sort_unstable_by_key(|&block| extents[block as usize].1);
        for order in [by_height.clone(), by_height.into_iter().rev().collect()] {
            let mut h = at_the_all_resident_cap(extents);
            h.drain(&set, &[], &[]);
            for pair in order.chunks(2) {
                let plan = h.drain(&[], &[], pair);
                assert!(plan.deferred.is_empty(), "{:?}", plan.deferred);
            }
            assert_holds_every_block_without_growth(&h, extents, "pairs over drains");
        }
    }
}

// The same pool reached cell by cell: each block turns mandatory with its pair
// in hand, so nothing reserved its room and the last arrivals repack. Band
// pairs stay out of this: they never repack, and R33 leaves them refused.
#[test]
fn all_resident_cap_holds_a_demand_that_widens_one_block_at_a_time() {
    let r24 = [(56, 16), (28, 44), (24, 20)];
    let mixed = mixed_level(0xca9, 120);
    for extents in [&r24[..], &mixed] {
        let mut h = at_the_all_resident_cap(extents);
        for block in 0..extents.len() as u32 {
            h.drain(&[mandatory(block)], &[], &[block]);
        }
        assert_holds_every_block_without_growth(&h, extents, "block by block");
    }
}

/// A controller's batch of band pairs over `blocks`, first drain of its
/// generation.
fn band_batch(generation: u64, blocks: std::ops::Range<u32>) -> LightmapDrainBatch {
    LightmapDrainBatch {
        generation,
        content_tag: [3; 32],
        target_reset: Some(blocks.clone().map(|block| band(block, 1)).collect()),
        ready: blocks
            .map(|block| PreparedLightmapBlock {
                generation,
                content_tag: [3; 32],
                block,
                payload: LightmapBlockPayload::default(),
            })
            .collect(),
        ..LightmapDrainBatch::default()
    }
}

// R25: a pool sized past the old fifteen-layer default is one pool. A band pair lands in
// its top layer under the first controller, stays there, and lands there again
// under a controller that replaced it, after a growth as before one.
#[test]
fn band_uses_every_layer_under_any_controller() {
    const LAYERS: u32 = 20;
    let mut h = Harness::new(&[(64, 64); LAYERS as usize + 2], LAYERS);

    let plan = h.model.plan_batch(&band_batch(1, 0..LAYERS));
    assert_eq!(plan.installed.len(), LAYERS as usize, "{:?}", plan.refused);
    let top_layer = |h: &Harness| {
        (0..h.model.block_count())
            .filter_map(|block| h.model.at(block))
            .map(|at| at.layer)
            .max()
    };
    h.execute();
    assert_eq!(top_layer(&h), Some(LAYERS - 1));

    // Nothing moves a band block out of a high layer while it stays a target.
    let plan = h.drain(&[], &[], &[]);
    assert!(plan.evicted.is_empty(), "{:?}", plan.evicted);

    // Mandatory demand evicts band blocks, then grows past what they gave up.
    let set: Vec<LightmapTarget> = (0..LAYERS + 2).map(mandatory).collect();
    let ready: Vec<u32> = (LAYERS..LAYERS + 2).collect();
    let plan = h.drain(&set, &[], &ready);
    let grown = plan.pools[POOL].growth.expect("two blocks past the pool");
    assert_eq!(grown.from_layers, LAYERS);
    h.model.release_retirement(POOL);

    // A band block in a grown layer stays there as its target drops to band.
    let set: Vec<LightmapTarget> = (0..LAYERS + 2).map(|block| band(block, 1)).collect();
    let plan = h.drain(&set, &[], &[]);
    assert!(plan.evicted.is_empty(), "{:?}", plan.evicted);
    assert_eq!(top_layer(&h), Some(grown.to_layers - 1));

    // A replacement controller's first batch starts from an empty pool whose
    // size stands, and every layer is open to it.
    let plan = h.model.plan_batch_from_empty(&band_batch(2, 0..LAYERS + 2));
    assert_eq!(plan.installed.len(), LAYERS as usize + 2, "{:?}", plan.refused);
    assert!(plan.evicted.is_empty(), "{:?}", plan.evicted);
    h.execute();
    assert_eq!(top_layer(&h), Some(grown.to_layers - 1));
    assert_eq!(h.model.pool(POOL).layers(), grown.to_layers);
}

// One growth covers the whole targeted demand, resident or not, at the margin
// the first generation uses: the only headroom a pool has.
#[test]
fn growth_reserves_demand_times_the_shared_margin() {
    let grow = |blocks: u32, targeted: u32, limit: u32| {
        let mut h = Harness::with_layer_limit(&vec![(64, 64); blocks as usize], 1, limit);
        let set: Vec<LightmapTarget> = (0..targeted).map(mandatory).collect();
        let plan = h.drain(&set, &[], &[0, 1]);
        plan.pools[POOL].growth.expect("two full layers past one").to_layers
    };
    let margin = LIGHTMAP_POOL_MARGIN.of(8);
    assert_eq!(margin, 10, "eight layers of demand, 5/4");
    assert_eq!(grow(16, 8, u32::MAX), margin, "demand times the margin");
    assert_eq!(grow(9, 8, u32::MAX), 9, "capped at all-resident");
    assert_eq!(grow(16, 8, 9), 9, "capped at the device limit");
    assert_eq!(grow(16, 2, u32::MAX), 3, "rounded up, from the demand alone");

    let growth = include_str!("growth.rs").to_lowercase();
    assert!(
        !growth.contains("headroom"),
        "no growth headroom remains beside the shared margin"
    );
}
