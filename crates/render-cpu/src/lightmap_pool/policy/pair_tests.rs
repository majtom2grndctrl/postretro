// Atomic pairs across both pools: a block's halves install together or not at all, and leave together.

use postretro_level_format::lightmap_pool::PoolKind::{Lightmap, Mask};
use postretro_level_loader::{LightmapBlockClass, LightmapTarget};

use super::super::{
    BLOCK_FLAG_NO_MASK_POOL, BLOCK_FLAG_PLANE_A, BLOCK_FLAG_PLANE_B, BLOCK_FLAG_RESIDENT,
};
use super::gpu_mirror::GpuMirror;
use super::*;

const EDGE: u32 = 64;
const ALIGN: u32 = 4;
const A: [bool; 2] = [true, false];
const B: [bool; 2] = [false, true];
const BOTH: [bool; 2] = [true, true];
const NONE: [bool; 2] = [false, false];

fn target(block: u32, class: LightmapBlockClass, lead: u32) -> LightmapTarget {
    LightmapTarget { block, class, lead }
}

fn mandatory(block: u32) -> LightmapTarget {
    target(block, LightmapBlockClass::Mandatory, 0)
}

fn band(block: u32, lead: u32) -> LightmapTarget {
    target(block, LightmapBlockClass::Band, lead)
}

/// Both pools in earnest, each with its own cap, on a mirror of both textures.
struct Pair {
    model: LightmapPoolModel,
    gpu: GpuMirror,
    caps: PerPool<u32>,
}

impl Pair {
    fn new(blocks: &[((u32, u32), [bool; 2])], lightmap: u32, mask: u32) -> Self {
        let demand = PoolDemand::new(
            ALIGN,
            blocks.iter().map(|&(extent, _)| extent).collect(),
            Some(blocks.iter().map(|&(_, planes)| planes).collect()),
        );
        let caps = PerPool { lightmap, mask };
        let model = LightmapPoolModel::new(demand, EDGE, caps).unwrap();
        let gpu = GpuMirror::new(&model);
        Self { model, gpu, caps }
    }

    fn plan(&mut self, set: &[LightmapTarget], remove: &[u32], ready: &[u32]) -> &DrainPlan {
        self.model.plan_drain(DrainRequest {
            pool_cap_layers: self.caps,
            target_reset: None,
            target_set: set,
            target_remove: remove,
            ready,
        })
    }

    fn execute(&mut self) {
        self.gpu.execute(self.model.plan());
        self.gpu.assert_matches(&self.model);
    }

    fn drain(&mut self, set: &[LightmapTarget], remove: &[u32], ready: &[u32]) -> &DrainPlan {
        self.plan(set, remove, ready);
        self.execute();
        self.model.plan()
    }

    fn free_of(&self, kind: PoolKind) -> u64 {
        self.model.plan().report[kind].band_headroom_texels
    }

    fn assert_absent(&self, block: u32) {
        assert_eq!(self.model.placement(block), None, "block {block} lightmap");
        for plane in 0..2 {
            assert_eq!(
                self.model.plane_placement(block, plane),
                None,
                "block {block} plane {plane}"
            );
        }
    }
}

fn evicted_blocks(plan: &DrainPlan) -> Vec<u32> {
    plan.evicted.iter().map(|eviction| eviction.block).collect()
}

fn refused_blocks(plan: &DrainPlan) -> Vec<u32> {
    plan.refused.iter().map(|refusal| refusal.block).collect()
}

fn written_entries(plan: &DrainPlan) -> Vec<u32> {
    plan.table_writes
        .iter()
        .map(|write| write.index - 1)
        .collect()
}

const FULL: (u32, u32) = (64, 64);
const HALF: (u32, u32) = (64, 32);

// The lightmap half and plane A take their places, and plane B finds none.
#[test]
fn pair_short_in_either_pool_installs_nothing() {
    // The mask pool holds one layer: block 0 fills half, so block 1's plane A
    // fits and its plane B does not.
    let blocks = [(HALF, A), (HALF, BOTH)];
    let mut h = Pair::new(&blocks, 2, 1);
    assert_eq!(h.model.pool(Mask).layers(), 1);
    h.drain(&[mandatory(0)], &[], &[0]);
    let lightmap_room = h.free_of(Lightmap);
    let mask_room = h.free_of(Mask);

    // A band pair in that state is refused whole, its reasons named.
    let plan = h.drain(&[band(1, 1)], &[], &[1]);
    assert_eq!(refused_blocks(plan), vec![1]);
    assert_eq!(
        plan.refused[0].short,
        PerPool {
            lightmap: false,
            mask: true
        }
    );
    assert!(plan.uploads.is_empty() && plan.table_writes.is_empty());
    h.assert_absent(1);
    assert_eq!(h.free_of(Lightmap), lightmap_room, "every rect stays free");
    assert_eq!(h.free_of(Mask), mask_room);

    // A mandatory pair in that state must be seated, so it grows.
    let plan = h.drain(&[mandatory(1)], &[], &[1]);
    assert_eq!(plan.installed, vec![1]);
    assert!(plan.pools.mask.growth.is_some());
    assert!(plan.pools.lightmap.growth.is_none(), "only the short pool");
}

// The same pair the other way round: the mask pool has room, the lightmap pool
// none, and no plane stays placed.
#[test]
fn a_band_pair_short_of_the_lightmap_pool_leaves_no_plane_behind() {
    let blocks = [(FULL, NONE), (HALF, BOTH)];
    let mut h = Pair::new(&blocks, 1, 2);
    h.drain(&[mandatory(0)], &[], &[0]);
    let plan = h.drain(&[band(1, 1)], &[], &[1]);
    assert_eq!(refused_blocks(plan), vec![1]);
    assert!(plan.refused[0].short.lightmap && !plan.refused[0].short.mask);
    h.assert_absent(1);
    assert_eq!(h.free_of(Mask), u64::from(EDGE * EDGE), "both planes back");
}

// A pair turned back whole comes back whole: every rect it took is free again
// and it lands in one drain when its pool can take it.
#[test]
fn a_pair_that_cannot_land_retries_whole() {
    let blocks = [(FULL, A), (FULL, A), (FULL, B)];
    let mut h = Pair::new(&blocks, 3, 1);
    h.model.pools.mask.retiring = true;
    h.drain(&[mandatory(0)], &[], &[0]);
    let lightmap_room = h.free_of(Lightmap);
    let plan = h.drain(&[mandatory(1)], &[], &[1]);
    assert_eq!(plan.deferred, vec![1]);
    h.assert_absent(1);
    h.model.release_retirement(Mask);
    let plan = h.drain(&[], &[], &[1]);
    assert_eq!(plan.installed, vec![1]);
    h.assert_placed_whole(1);
    assert!(
        h.free_of(Lightmap) < lightmap_room,
        "its reserved room landed"
    );
}

impl Pair {
    fn assert_placed_whole(&self, block: u32) {
        assert!(self.model.is_resident(block));
        for (plane, stored) in self.model.planes(block).unwrap().into_iter().enumerate() {
            assert_eq!(
                self.model.plane_placement(block, plane).is_some(),
                stored,
                "block {block} plane {plane}"
            );
        }
    }
}

// Both halves need growth, and the mask pool is still retiring. Neither
// pool grows for the pair, whatever order its halves came in.
#[test]
fn pair_defers_whole_when_other_pool_retires() {
    let blocks = [(FULL, A), (FULL, A), (FULL, A), (FULL, A)];
    let mut h = Pair::new(&blocks, 1, 1);
    // Both pools grow on one drain, and the lightmap pool alone is released.
    let plan = h.drain(&[mandatory(0), mandatory(1)], &[], &[0, 1]);
    assert!(plan.pools.lightmap.growth.is_some() && plan.pools.mask.growth.is_some());
    h.model.release_retirement(Lightmap);
    let layers = (h.model.pool(Lightmap).layers(), h.model.pool(Mask).layers());

    // The growth reserved a third layer in each pool, and block 2 takes it.
    let plan = h.drain(&[mandatory(2), mandatory(3)], &[], &[2, 3]);
    assert_eq!(plan.installed, vec![2]);
    let (lightmap_grew, mask_grew) = (plan.pools.lightmap.growth, plan.pools.mask.growth);
    assert_eq!(
        (lightmap_grew, mask_grew),
        (None, None),
        "neither pool grows"
    );
    assert_eq!(plan.deferred, vec![3], "deferred whole");
    assert_eq!(written_entries(plan), vec![2]);
    h.assert_absent(3);
    assert_eq!(
        (h.model.pool(Lightmap).layers(), h.model.pool(Mask).layers()),
        layers
    );

    // Its retirement over, the same pair grows both pools and lands whole.
    h.model.release_retirement(Mask);
    let plan = h.drain(&[], &[], &[3]);
    assert_eq!(plan.installed, vec![3]);
    assert!(plan.pools.lightmap.growth.is_some() && plan.pools.mask.growth.is_some());
    h.assert_placed_whole(3);
}

// Growth waits on its own pool. A pair whose plane has room grows the
// lightmap pool however long the mask pool's generation takes to retire.
#[test]
fn growth_waits_only_on_its_own_pools_retirement() {
    let blocks = [(FULL, A), (FULL, A), (FULL, NONE), (FULL, NONE)];
    let mut h = Pair::new(&blocks, 1, 1);
    let plan = h.drain(&[mandatory(0), mandatory(1)], &[], &[0, 1]);
    assert!(plan.pools.mask.growth.is_some());
    h.model.release_retirement(Lightmap);
    assert!(h.model.pool(Mask).retiring());

    let plan = h.drain(&[mandatory(2), mandatory(3)], &[], &[2, 3]);
    assert_eq!(plan.installed, vec![2, 3]);
    assert!(plan.pools.lightmap.growth.is_some());
    assert_eq!(plan.pools.mask.growth, None);
    assert!(h.model.pool(Mask).retiring(), "the mask pool retires alone");
}

// Only the mask pool lacked room, and the victim leaves both.
#[test]
fn eviction_frees_both_halves() {
    let blocks = [(FULL, A), (HALF, A)];
    let mut h = Pair::new(&blocks, 2, 1);
    h.drain(&[band(0, 3)], &[], &[0]);
    let before = h.free_of(Lightmap);

    h.drain(&[mandatory(1)], &[], &[1]);
    h.assert_absent(0);
    h.assert_placed_whole(1);
    let half = 64 * 32;
    assert_eq!(
        h.free_of(Lightmap),
        before + 64 * 64 - half,
        "its lightmap rect"
    );
    let plan = h.model.plan();
    assert_eq!(plan.installed, vec![1]);
    assert_eq!(evicted_blocks(plan), vec![0]);
    assert!(plan.pools.lightmap.repack.is_none() && plan.pools.lightmap.growth.is_none());
    assert_eq!(
        written_entries(plan),
        vec![0, 1],
        "the victim's entry goes non-resident once"
    );
}

// Both ways. Band blocks 0 and 1 share the pools; the mandatory pair
// takes block 1's mask rect and none of block 0's.
#[test]
fn band_victim_returns_only_whole() {
    // Two half-layer planes fill the mask pool's one layer. Block 2's plane
    // needs a half layer: the farthest victim, block 1, makes the room, and
    // nearer block 0 is never asked.
    let blocks = [(HALF, A), (HALF, A), (HALF, A)];
    let mut h = Pair::new(&blocks, 2, 1);
    h.drain(&[band(0, 1), band(1, 2)], &[], &[0, 1]);
    let plan = h.drain(&[mandatory(2)], &[], &[2]);
    assert_eq!(evicted_blocks(plan), vec![1]);
    h.assert_absent(1);
    assert!(h.model.is_resident(0));

    // Now a victim that freed nothing the pair used. Block 1, farther off,
    // holds a small lightmap rect beside block 0's plane; block 3's plane
    // needs block 0's rect, so block 1 is walked first and goes back.
    let blocks = [(HALF, A), ((32, 32), NONE), (HALF, A), (HALF, A)];
    let mut h = Pair::new(&blocks, 2, 1);
    h.drain(&[mandatory(2), band(0, 1), band(1, 2)], &[], &[2, 0, 1]);
    let held = h.model.addresses(1);
    let plan = h.drain(&[mandatory(3)], &[], &[3]);
    assert_eq!(evicted_blocks(plan), vec![0], "the one whose room it took");
    assert_eq!(written_entries(plan), vec![0, 3], "nothing written for it");
    assert_eq!(h.model.addresses(1), held, "put back where it was");
    h.assert_absent(0);

    // A victim with one rect free and another taken stays out whole. Block 0
    // goes first and its plane's room seats the pair's plane. Block 1 then makes
    // the lightmap room, which leaves block 0's lightmap rect free.
    //
    // lightmap   layer 0  2 (48 high), 3     layer 1  4, 1 (32 high), 0
    // mask       layer 0  3, 4, 0, then 16 rows free
    let blocks = [
        ((32, 16), A),
        ((64, 32), NONE),
        ((64, 48), NONE),
        ((64, 16), A),
        ((64, 16), A),
        ((64, 32), A),
    ];
    let mut h = Pair::new(&blocks, 2, 1);
    let set = [
        band(0, 2),
        band(1, 1),
        mandatory(2),
        mandatory(3),
        mandatory(4),
    ];
    h.drain(&set, &[], &[0, 1, 2, 3, 4]);
    let lightmap_rect = h.model.placement(0).unwrap();
    assert_eq!((lightmap_rect.layer, lightmap_rect.y), (1, 48));
    let plan = h.drain(&[mandatory(5)], &[], &[5]);
    assert_eq!(evicted_blocks(plan), vec![0, 1]);
    assert_eq!(written_entries(plan), vec![0, 1, 5]);
    let pair = h.model.placement(5).unwrap();
    assert_eq!(
        (pair.layer, pair.y),
        (1, 16),
        "clear of block 0's lightmap rect"
    );
    h.assert_absent(0);
    h.assert_absent(1);
    h.assert_placed_whole(5);
}

#[test]
fn mask_placements_follow_plane_presence() {
    let blocks = [(HALF, NONE), (HALF, A), (HALF, B), (HALF, BOTH)];
    let mut h = Pair::new(&blocks, 2, 2);
    let set: Vec<_> = (0..4).map(mandatory).collect();
    let plan = h.drain(&set, &[], &[0, 1, 2, 3]);
    assert_eq!(plan.installed, vec![0, 1, 2, 3]);
    let planes_of = |upload: &BlockUpload| upload.planes.map(|plane| plane.is_some());
    let by_block: Vec<_> = plan
        .uploads
        .iter()
        .map(|u| (u.block, planes_of(u)))
        .collect();
    for (block, planes) in [(0, NONE), (1, A), (2, B), (3, BOTH)] {
        assert!(by_block.contains(&(block, planes)), "block {block}");
    }
    assert_eq!(
        h.model.pool(Mask).resident_items().len(),
        4,
        "a place per plane"
    );
    for (block, flags) in [
        (0, 0),
        (1, BLOCK_FLAG_PLANE_A),
        (2, BLOCK_FLAG_PLANE_B),
        (3, BLOCK_FLAG_PLANE_A | BLOCK_FLAG_PLANE_B),
    ] {
        let word = h.model.table_entry(block).to_words()[3] >> 24;
        assert_eq!(word, BLOCK_FLAG_RESIDENT | flags, "block {block}");
    }
}

// A drain that placed, evicted, repacked and grew in both pools
// leaves nothing of itself behind.
#[test]
fn abort_restores_both_pools() {
    let blocks = [(HALF, A), (HALF, A), (FULL, BOTH), (HALF, NONE)];
    let mut h = Pair::new(&blocks, 1, 1);
    h.drain(&[mandatory(0), band(1, 4)], &[], &[0, 1]);
    let before = (h.model.addresses(0), h.model.addresses(1));
    let table = h.model.table_bytes();

    h.plan(&[mandatory(2), mandatory(3)], &[], &[2, 3]);
    assert!(!h.model.plan().uploads.is_empty());
    assert!(h.model.plan().allocates_texture());
    h.model.abort_drain();

    assert_eq!((h.model.addresses(0), h.model.addresses(1)), before);
    assert_eq!(h.model.table_bytes(), table, "the table entries");
    h.assert_absent(2);
    h.assert_absent(3);
    for kind in PoolKind::ALL {
        let pool = h.model.pool(kind);
        assert_eq!((pool.layers(), pool.texture_allocations()), (1, 1));
        assert!(!pool.retiring());
    }
    assert_eq!(h.model.plan().table_writes, Vec::new());
    h.gpu.assert_matches(&h.model);

    // The same drain plans again: the abort restored its targets too.
    h.drain(&[mandatory(2), mandatory(3)], &[], &[2, 3]);
    h.assert_placed_whole(2);
}

#[test]
fn failed_install_frees_both_halves() {
    let blocks = [(HALF, BOTH), (HALF, A), (FULL, A)];
    let mut h = Pair::new(&blocks, 1, 1);
    h.drain(&[mandatory(0)], &[], &[0]);
    h.plan(&[mandatory(1), mandatory(2)], &[], &[1, 2]);
    let grew = h.model.plan().pools.mask.growth;
    assert!(grew.is_some());

    h.model.fail_install(1).unwrap();
    let plan = h.model.plan();
    assert_eq!(plan.failed, vec![1]);
    assert_eq!(plan.installed, vec![2]);
    assert_eq!(plan.pools.mask.growth, grew, "the drain's growth stands");
    h.assert_absent(1);
    h.assert_placed_whole(2);
    h.execute();
    assert_eq!(
        h.model.table_entry(1),
        BlockTableEntry::Missing {
            width: 64,
            height: 32
        }
    );
}

#[test]
fn new_generation_empties_both_pools() {
    let blocks = [(HALF, BOTH), (HALF, A), (HALF, NONE)];
    let mut h = Pair::new(&blocks, 2, 2);
    h.drain(&[mandatory(0), band(1, 1)], &[], &[0, 1]);
    assert_eq!(h.model.pool(Mask).resident_items().len(), 3);
    let layers = (h.model.pool(Lightmap).layers(), h.model.pool(Mask).layers());

    let batch = postretro_level_loader::LightmapDrainBatch {
        generation: 2,
        content_tag: [0; 32],
        pool_cap_layers: 2,
        target_reset: Some(Vec::new()),
        ..Default::default()
    };
    let plan = h.model.plan_batch_from_empty(&batch);
    assert!(plan.evicted.is_empty(), "not reported evicted");
    assert_eq!(written_entries(plan), vec![0, 1]);
    h.execute();
    assert!(h.model.pool(Lightmap).resident_items().is_empty());
    assert!(h.model.pool(Mask).resident_items().is_empty());
    assert_eq!(
        (h.model.pool(Lightmap).layers(), h.model.pool(Mask).layers()),
        layers,
        "the layers stand"
    );
}

/// Block 0 tops the mask pool's one layer above a band block and a gap, so a
/// plane at the block's own height fits only once the mask pool repacks. The
/// lightmap pool has room to spare and never repacks.
fn mask_pool_short_of_a_row() -> Pair {
    let blocks = [((64, 24), A), ((64, 16), A), ((64, 40), A), ((4, 4), A)];
    let mut h = Pair::new(&blocks, 4, 1);
    let plan = h.drain(&[mandatory(0), mandatory(1), band(3, 3)], &[], &[0, 1, 3]);
    assert_eq!(plan.installed, vec![0, 1, 3]);
    h
}

// A repack of the mask pool alone rewrites the entry of every block with
// a moved plane, and no other.
#[test]
fn mask_repack_rewrites_moved_entries_only() {
    let mut h = mask_pool_short_of_a_row();
    let lightmap = |h: &Pair| [1, 3].map(|block| h.model.placement(block));
    let before = lightmap(&h);
    let planes_before = [1, 3].map(|block| h.model.plane_placement(block, 0));
    let plan = h.plan(&[mandatory(2)], &[0], &[2]);
    assert!(plan.pools.mask.repack.is_some());
    assert!(plan.pools.lightmap.repack.is_none());
    assert!(plan.pools.lightmap.copies.is_empty());
    assert!(!plan.pools.mask.copies.is_empty());
    h.execute();
    assert_eq!(lightmap(&h), before, "the lightmap pool stood");
    let moved: Vec<u32> = [1, 3]
        .into_iter()
        .zip(planes_before)
        .filter(|&(block, was)| h.model.plane_placement(block, 0) != was)
        .map(|(block, _)| block)
        .collect();
    assert!(!moved.is_empty());
    let mut expected = moved;
    expected.extend([0, 2]);
    expected.sort_unstable();
    assert_eq!(written_entries(h.model.plan()), expected);
}

// One pool's repack moves nothing in the other, but a band block it drops
// leaves both.
#[test]
fn one_pool_repack_leaves_the_other() {
    // A band plane no repack can seat beside the demand.
    let blocks = [((64, 24), A), ((64, 16), A), ((64, 40), A), ((64, 16), A)];
    let mut h = Pair::new(&blocks, 4, 1);
    h.drain(&[mandatory(0), mandatory(1), band(3, 3)], &[], &[0, 1, 3]);
    let lightmap_at = h.model.placement(1);
    let plane_at = h.model.plane_placement(3, 0);
    assert!(plane_at.is_some());

    let plan = h.drain(&[mandatory(2)], &[0], &[2]);
    assert!(plan.pools.mask.repack.is_some());
    assert!(
        plan.pools.lightmap.repack.is_none(),
        "the lightmap pool stood"
    );
    assert_eq!(evicted_blocks(plan), vec![0, 3]);
    assert_eq!(
        written_entries(plan).iter().filter(|&&b| b == 3).count(),
        1,
        "its entry goes non-resident once"
    );
    h.assert_absent(3);
    assert_eq!(
        h.model.placement(1),
        lightmap_at,
        "every other placement stands"
    );

    // The lightmap pool's own repack drops a band block out of the mask pool.
    let blocks = [((64, 24), A), ((64, 16), A), ((64, 40), A), ((64, 16), A)];
    let mut h = Pair::new(&blocks, 1, 4);
    h.drain(&[mandatory(0), mandatory(1), band(3, 3)], &[], &[0, 1, 3]);
    let plane_at = h.model.plane_placement(1, 0);
    let plan = h.drain(&[mandatory(2)], &[0], &[2]);
    assert!(plan.pools.lightmap.repack.is_some());
    assert!(plan.pools.mask.repack.is_none());
    assert!(plan.pools.mask.copies.is_empty());
    h.assert_absent(3);
    assert_eq!(h.model.plane_placement(1, 0), plane_at);
}

// One pool repacks, the other grows, and each lands in its own
// generation with the other's texels untouched.
#[test]
fn repack_beside_growth_stands_in_one_plan() {
    // The mask pool repacks for block 2's row. The lightmap pool, full of
    // mandatory blocks, must grow for it.
    let blocks = [
        ((64, 24), A),
        ((64, 16), A),
        ((64, 40), A),
        ((64, 24), NONE),
    ];
    let mut h = Pair::new(&blocks, 1, 1);
    h.drain(&[mandatory(0), mandatory(1), mandatory(3)], &[], &[0, 1, 3]);
    assert_eq!(h.model.pool(Lightmap).layers(), 1);
    let plan = h.drain(&[mandatory(2)], &[0], &[2]);
    assert!(plan.pools.mask.repack.is_some(), "{:?}", plan.pools.mask);
    assert!(plan.pools.mask.growth.is_none());
    assert!(
        plan.pools.lightmap.growth.is_some(),
        "{:?}",
        plan.pools.lightmap
    );
    assert!(h.model.pool(Lightmap).retiring());
    assert!(
        !h.model.pool(Mask).retiring(),
        "only the grown pool retires"
    );
    assert_eq!(h.model.pool(Mask).texture_allocations(), 1);
}

// A block moved in both pools has one write with both
// addresses, and each pool's copies stay its own.
#[test]
fn both_pools_repack_with_one_write_per_block() {
    let blocks = [((64, 24), A), ((64, 16), A), ((64, 40), A), ((4, 4), A)];
    let mut h = Pair::new(&blocks, 1, 1);
    h.drain(&[mandatory(0), mandatory(1), band(3, 3)], &[], &[0, 1, 3]);
    let was = (h.model.placement(1), h.model.plane_placement(1, 0));
    let plan = h.drain(&[mandatory(2)], &[0], &[2]);
    assert!(plan.pools.lightmap.repack.is_some() && plan.pools.mask.repack.is_some());
    assert!(!plan.pools.lightmap.copies.is_empty() && !plan.pools.mask.copies.is_empty());
    let writes = written_entries(plan);
    let mut unique = writes.clone();
    unique.dedup();
    assert_eq!(writes, unique, "one write per block");
    assert!(h.model.placement(1) != was.0 && h.model.plane_placement(1, 0) != was.1);
    let entry = h
        .model
        .plan()
        .table_writes
        .iter()
        .find(|w| w.index == 2)
        .unwrap();
    assert_eq!(entry.entry, h.model.table_entry(1), "both new addresses");
}

// A repack rides the live generation while the pool's last one retires.
#[test]
fn repack_while_retiring_touches_live_generation() {
    for kind in PoolKind::ALL {
        let blocks = [((64, 64), A), ((64, 24), A), ((64, 16), A), ((64, 40), A)];
        let caps = match kind {
            Lightmap => (1, 8),
            Mask => (8, 1),
        };
        let mut h = Pair::new(&blocks, caps.0, caps.1);
        h.model.growth_sizing = growth::GrowthSizing::PerDrain;
        // Block 0 fills the first layer, so blocks 1 and 2 grow the pool and
        // stack in the second; then block 1 leaves two holes, neither of which
        // a 40-high half fits until the pool repacks.
        let set = [mandatory(0), mandatory(1), mandatory(2)];
        let plan = h.drain(&set, &[], &[0, 1, 2]);
        assert!(plan.pools[kind].growth.is_some());
        let layers = h.model.pool(kind).layers();
        assert_eq!(layers, 2);
        assert!(h.model.pool(kind).retiring());

        let plan = h.drain(&[mandatory(3)], &[1], &[3]);
        assert!(plan.pools[kind].repack.is_some(), "{:?}", plan.pools[kind]);
        assert_eq!(plan.pools[kind].growth, None);
        assert_eq!(plan.installed, vec![3]);
        assert!(h.model.pool(kind).retiring(), "it retires on");
        assert_eq!(h.model.pool(kind).layers(), layers);
        assert_eq!(h.model.pool(kind).texture_allocations(), 2);
    }
}

// A block's two planes, moved in one repack, stay told apart: the mirror
// holds a texel per plane and checks each against its own address.
#[test]
fn a_blocks_two_planes_stay_told_apart_through_a_repack() {
    let blocks = [((64, 16), BOTH), ((64, 8), BOTH), ((64, 24), BOTH)];
    let mut h = Pair::new(&blocks, 4, 1);
    h.drain(&[mandatory(0), mandatory(1)], &[], &[0, 1]);
    let before = [0, 1].map(|plane| h.model.plane_placement(1, plane));
    let plan = h.drain(&[mandatory(2)], &[0], &[2]);
    assert!(plan.pools.mask.repack.is_some(), "{:?}", plan.pools.mask);
    assert_ne!(
        [0, 1].map(|plane| h.model.plane_placement(1, plane)),
        before
    );
    assert_ne!(
        h.model.plane_placement(1, 0),
        h.model.plane_placement(1, 1),
        "one block's planes are two places"
    );
}

// Id 42 present and every plane absent places nothing and still has
// masks, unlike a level with no mask pool.
#[test]
fn a_level_of_absent_planes_has_masks_with_no_placement() {
    let mut h = Pair::new(&[(HALF, NONE), (HALF, NONE)], 1, 1);
    assert_eq!(h.model.pool(Mask).layers(), 0);
    assert_eq!(h.model.pool(Mask).texture_allocations(), 0);
    h.drain(&[mandatory(0), mandatory(1)], &[], &[0, 1]);
    let flags = h.model.table_entry(0).to_words()[3] >> 24;
    assert_eq!(flags, BLOCK_FLAG_RESIDENT, "reads zero");

    let demand = PoolDemand::lightmap_only(ALIGN, vec![HALF]);
    let mut model = LightmapPoolModel::new(demand, EDGE, PerPool::both(1)).unwrap();
    model.plan_drain(DrainRequest {
        pool_cap_layers: PerPool::both(1),
        target_set: &[mandatory(0)],
        ready: &[0],
        ..DrainRequest::default()
    });
    let flags = model.table_entry(0).to_words()[3] >> 24;
    assert_eq!(flags, BLOCK_FLAG_RESIDENT | BLOCK_FLAG_NO_MASK_POOL);
}

// A reservation in one pool lands with its pair, and a pair the other pool
// cannot take gives it back whole.
#[test]
fn room_held_in_one_pool_waits_for_the_other() {
    let blocks = [(FULL, A), (FULL, A), (HALF, NONE)];
    let mut h = Pair::new(&blocks, 2, 1);
    // Block 1 holds lightmap room and finds none in the mask pool.
    let plan = h.drain(&[mandatory(0), mandatory(1)], &[], &[0]);
    assert_eq!(plan.installed, vec![0]);
    assert_eq!(plan.pools.lightmap.room_reserved, vec![1]);
    assert!(plan.pools.mask.room_reserved.is_empty());

    // Its pair needs mask growth, which a retiring pool cannot give.
    h.model.pools.mask.retiring = true;
    let plan = h.drain(&[], &[], &[1]);
    assert_eq!(plan.deferred, vec![1]);
    assert!(
        plan.pools.lightmap.room_returned.is_empty(),
        "the room stands"
    );
    h.assert_absent(1);

    h.model.release_retirement(Mask);
    let plan = h.drain(&[], &[], &[1]);
    assert_eq!(plan.installed, vec![1]);
    assert_eq!(plan.pools.lightmap.room_returned, vec![1], "landed");
}

#[test]
fn the_steady_drain_of_two_pools_allocates_nothing() {
    let blocks = [(HALF, BOTH), (HALF, A), (HALF, B), (HALF, NONE)];
    let mut h = Pair::new(&blocks, 2, 2);
    let set: Vec<_> = (0..3).map(mandatory).collect();
    h.drain(&set, &[], &[0, 1, 2]);
    h.drain(&[], &[], &[]);
    let capacities = h.model.scratch_capacities();
    let passes = h.model.demand_passes;
    for _ in 0..4 {
        h.drain(&[], &[], &[]);
    }
    assert_eq!(h.model.scratch_capacities(), capacities);
    assert_eq!(h.model.demand_passes, passes);
}
