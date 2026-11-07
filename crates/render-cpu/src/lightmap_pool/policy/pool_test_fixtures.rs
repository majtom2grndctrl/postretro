// Shared fixtures of the pool policy tests: a model driven on the GPU mirror.

use postretro_level_loader::{LightmapBlockClass, LightmapTarget};

use super::gpu_mirror::GpuMirror;
use super::*;

pub(super) const EDGE: u32 = 64;
pub(super) const ALIGN: u32 = 4;

/// The pool that stays out of the suite's way.
const OTHER: PoolKind = match POOL {
    PoolKind::Lightmap => PoolKind::Mask,
    PoolKind::Mask => PoolKind::Lightmap,
};

/// Plane A on even blocks, plane B on odd ones.
pub(super) fn plane_of(block: u32) -> usize {
    (block % 2) as usize
}

fn item_in(kind: PoolKind, block: u32) -> u32 {
    match kind {
        PoolKind::Lightmap => block,
        PoolKind::Mask => mask_item(block, plane_of(block)),
    }
}

/// `block`'s item in the pool under test.
pub(super) fn item(block: u32) -> u32 {
    item_in(POOL, block)
}

fn blocks_of(items: &[u32]) -> Vec<u32> {
    items.iter().map(|&item| POOL.block_of(item)).collect()
}

/// Blocks that newly hold room in the pool under test.
pub(super) fn reserved(plan: &DrainPlan) -> Vec<u32> {
    blocks_of(&plan.pools[POOL].room_reserved)
}

pub(super) fn returned(plan: &DrainPlan) -> Vec<u32> {
    blocks_of(&plan.pools[POOL].room_returned)
}

pub(super) fn refused(plan: &DrainPlan) -> Vec<u32> {
    plan.refused.iter().map(|refusal| refusal.block).collect()
}

/// Where an upload's half in the pool under test lands.
pub(super) fn upload_at(upload: &BlockUpload) -> BlockPlacement {
    match POOL {
        PoolKind::Lightmap => upload.placement,
        PoolKind::Mask => upload.planes[plane_of(upload.block)].unwrap(),
    }
}

/// The pool a suite exercises, read through its block ids.
pub(super) trait PoolUnderTest {
    fn at(&self, block: u32) -> Option<BlockPlacement>;
    fn slot_of(&self, block: u32) -> Option<Slot>;
}

impl PoolUnderTest for LightmapPoolModel {
    fn at(&self, block: u32) -> Option<BlockPlacement> {
        self.slot_of(block).map(placement_of)
    }

    fn slot_of(&self, block: u32) -> Option<Slot> {
        self.pools[POOL].slot(item(block))
    }
}

/// `cap` for the pool under test; the other pool is never capped.
pub(super) fn caps(cap: u32) -> PerPool<u32> {
    let mut caps = PerPool::both(u32::MAX);
    caps[POOL] = cap;
    caps
}

/// A model of one-plane blocks with `cap` and `max_layers` on the pool under
/// test, and a layer per block in the other. `None` as the model's own `new`.
pub(super) fn test_model(
    extents: Vec<(u32, u32)>,
    alignment: u32,
    edge: u32,
    cap: u32,
    max_layers: u32,
) -> Option<LightmapPoolModel> {
    let planes = (0..extents.len() as u32)
        .map(|block| std::array::from_fn(|plane| plane == plane_of(block)))
        .collect();
    let blocks = extents.len() as u32;
    let demand = PoolDemand::new(alignment, extents, Some(planes));
    let mut model = LightmapPoolModel::with_layer_limit(demand, edge, caps(cap), u32::MAX)?;
    model.pools[POOL].max_layers = max_layers;
    model.pools[POOL].layers = model.pools[POOL].layers.min(max_layers);
    model.pools[OTHER].layers = blocks;
    Some(model)
}

/// Seat `block`'s half in the other pool, for a layout built by hand in the
/// pool under test.
pub(super) fn place_other_half(model: &mut LightmapPoolModel, block: u32) {
    let at = item_in(OTHER, block);
    let (width, height) = model.pools[OTHER].alloc_extent(at);
    let slot = model.pools[OTHER]
        .allocator
        .allocate(width, height)
        .expect("the other pool has a layer per block");
    model.place(OTHER, at, slot);
}

pub(super) fn target(block: u32, class: LightmapBlockClass, lead: u32) -> LightmapTarget {
    LightmapTarget { block, class, lead }
}

pub(super) fn mandatory(block: u32) -> LightmapTarget {
    target(block, LightmapBlockClass::Mandatory, 0)
}

pub(super) fn visible(block: u32) -> LightmapTarget {
    target(block, LightmapBlockClass::Visible, 0)
}

pub(super) fn band(block: u32, lead: u32) -> LightmapTarget {
    target(block, LightmapBlockClass::Band, lead)
}

/// A model plus the GPU mirror every plan executes on.
pub(super) struct Harness {
    pub model: LightmapPoolModel,
    pub gpu: GpuMirror,
    pub cap: u32,
}

impl Harness {
    pub(super) fn new(extents: &[(u32, u32)], cap: u32) -> Self {
        Self::with_layer_limit(extents, cap, u32::MAX)
    }

    pub(super) fn with_layer_limit(extents: &[(u32, u32)], cap: u32, max_layers: u32) -> Self {
        let model = test_model(extents.to_vec(), ALIGN, EDGE, cap, max_layers).unwrap();
        let gpu = GpuMirror::new(&model);
        Self { model, gpu, cap }
    }

    /// Plan without executing.
    pub(super) fn plan(
        &mut self,
        set: &[LightmapTarget],
        remove: &[u32],
        ready: &[u32],
    ) -> &DrainPlan {
        self.model.plan_drain(DrainRequest {
            pool_cap_layers: caps(self.cap),
            target_reset: None,
            target_set: set,
            target_remove: remove,
            ready,
        })
    }

    pub(super) fn execute(&mut self) {
        self.gpu.execute(self.model.plan());
        self.gpu.assert_matches(&self.model);
    }

    pub(super) fn drain(
        &mut self,
        set: &[LightmapTarget],
        remove: &[u32],
        ready: &[u32],
    ) -> &DrainPlan {
        self.plan(set, remove, ready);
        self.execute();
        self.model.plan()
    }

    pub(super) fn evicted(&self) -> Vec<(u32, EvictionReason)> {
        self.model
            .plan()
            .evicted
            .iter()
            .map(|e| (e.block, e.reason))
            .collect()
    }
}

pub(super) fn assert_disjoint_within_layers(model: &LightmapPoolModel) {
    let rects: Vec<(u32, Slot)> = model
        .resident_blocks()
        .iter()
        .map(|&block| (block, model.slot_of(block).unwrap()))
        .collect();
    for (i, &(a, sa)) in rects.iter().enumerate() {
        assert!(
            sa.layer < model.pool(POOL).layers(),
            "block {a} past the active layers"
        );
        assert!(sa.x + sa.width <= EDGE && sa.y + sa.height <= EDGE);
        assert_eq!((sa.x % ALIGN, sa.y % ALIGN), (0, 0), "block {a} unaligned");
        for &(b, sb) in &rects[i + 1..] {
            let apart = sa.layer != sb.layer
                || sa.x + sa.width <= sb.x
                || sb.x + sb.width <= sa.x
                || sa.y + sa.height <= sb.y
                || sb.y + sb.height <= sa.y;
            assert!(apart, "blocks {a} and {b} overlap: {sa:?} {sb:?}");
        }
    }
}
