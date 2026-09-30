// Streamed lightmap pool placement policy: per-level pool model and per-drain plans.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

mod journal;
mod placement;
mod plan;
mod repack;

use postretro_level_loader::{LightmapBlockClass, LightmapPoolReport, LightmapTarget};

use super::{BlockPlacement, BlockPool, BlockTableEntry, Slot, block_table_bytes};
use journal::{JournalOp, Touched};
pub use plan::{
    BlockUpload, DrainPlan, EvictionReason, NotAPlannedUpload, PlannedEviction, PoolCopy,
    PoolGrowth, TableWrite,
};

/// One drain's inputs, as the controller's `LightmapDrainBatch` carries them,
/// minus payload bytes. The caller validates the batch
/// (`LightmapDrainBatch::validate_contract`) before planning: block ids are
/// in range, deltas sorted and disjoint, ready ids unique.
#[derive(Debug, Clone, Copy, Default)]
pub struct DrainRequest<'a> {
    pub pool_cap_layers: u32,
    pub target_reset: Option<&'a [LightmapTarget]>,
    pub target_set: &'a [LightmapTarget],
    pub target_remove: &'a [u32],
    /// Block ids of the pairs the shared drain budget admitted.
    pub ready: &'a [u32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Target {
    class: LightmapBlockClass,
    lead: u32,
}

impl Target {
    fn is_band(self) -> bool {
        self.class == LightmapBlockClass::Band
    }
}

/// Reused per-drain buffers. Owned by the model so a steady drain allocates
/// nothing.
#[derive(Debug, Default)]
struct Scratch {
    ready: Vec<u32>,
    never_refused: Vec<u32>,
    band_ready: Vec<u32>,
    /// Resident band blocks, nearest lead first; eviction pops the farthest.
    victims: Vec<u32>,
    victims_built: bool,
    over_cap: Vec<u32>,
    /// Extents deferred at the device layer limit this drain.
    device_limited: Vec<(u32, u32)>,
    /// `(block, highest layer + 1 it may land in)`.
    repack_keep: Vec<(u32, u32)>,
    repack_band: Vec<(u32, u32)>,
    repack_residents: Vec<u32>,
    moves: Vec<PoolCopy>,
}

/// The renderer's model of one level's streamed lightmap pool: which block
/// sits where, the pool's layer count, and the one retiring generation.
///
/// The pool texture always carries `layers() + 1` array layers: the last is
/// the spare a repack stages same-layer moves through, never a second pool.
/// `layers()` never passes the device limit the model was built with.
/// Each drain produces a [`DrainPlan`] the GPU layer executes in order; the
/// model has already applied it, and journals every mutation so the GPU
/// layer can [`fail_install`](Self::fail_install) one pair or
/// [`abort_drain`](Self::abort_drain) the whole drain.
#[derive(Debug)]
pub struct LightmapPoolModel {
    edge: u32,
    alignment: u32,
    /// True block extents by block id; allocations round each up to
    /// `alignment`.
    extents: Vec<(u32, u32)>,
    targets: Vec<Option<Target>>,
    slots: Vec<Option<Slot>>,
    resident: Vec<u32>,
    /// Index into `resident`, `u32::MAX` when absent.
    resident_pos: Vec<u32>,
    pool: BlockPool,
    /// Allocated texels per allocator layer, for band headroom.
    used_texels: Vec<u64>,
    layers: u32,
    cap_layers: u32,
    /// Usable layers the device can hold beside the spare. Growth never
    /// plans past it; a pair that would need it is deferred.
    max_layers: u32,
    retiring: bool,
    texture_allocations: u32,
    journal: Vec<JournalOp>,
    drain: u64,
    touch_stamp: Vec<u64>,
    touch_index: Vec<u32>,
    touched: Vec<Touched>,
    scratch: Scratch,
    plan: DrainPlan,
}

impl LightmapPoolModel {
    /// A model for a level's blocks, nothing resident, with no device layer
    /// limit. The first generation holds `min(cap_layers, L)` layers, where
    /// `L` is what the level needs all-resident: the pool never
    /// pre-allocates past what every block together would fill. `None` for a
    /// zero-sized block or one larger than an `edge`² layer; the loader
    /// rejects both.
    pub fn new(
        extents: Vec<(u32, u32)>,
        alignment: u32,
        edge: u32,
        cap_layers: u32,
    ) -> Option<Self> {
        Self::with_layer_limit(extents, alignment, edge, cap_layers, u32::MAX)
    }

    /// [`new`](Self::new) bounded by the device: the active generation never
    /// holds more than `max_layers` usable layers (the device's
    /// `maxTextureArrayLayers` minus the spare). A mandatory or visible pair
    /// that would need a layer past it is deferred, a counted miss that,
    /// unlike one while a generation retires, lasts until the resident set
    /// shrinks.
    pub fn with_layer_limit(
        extents: Vec<(u32, u32)>,
        alignment: u32,
        edge: u32,
        cap_layers: u32,
        max_layers: u32,
    ) -> Option<Self> {
        let alignment = alignment.max(1);
        let ceiling = super::place_all_resident(&extents, alignment, edge)?.layer_count;
        let layers = cap_layers.min(ceiling).min(max_layers);
        let count = extents.len();
        Some(Self {
            edge,
            alignment,
            extents,
            targets: vec![None; count],
            slots: vec![None; count],
            resident: Vec::new(),
            resident_pos: vec![u32::MAX; count],
            pool: BlockPool::new(edge, None),
            used_texels: vec![0; layers as usize],
            layers,
            cap_layers,
            max_layers,
            retiring: false,
            texture_allocations: 1,
            journal: Vec::new(),
            drain: 0,
            touch_stamp: vec![0; count],
            touch_index: vec![0; count],
            touched: Vec::new(),
            scratch: Scratch::default(),
            plan: DrainPlan::default(),
        })
    }

    /// Usable layers of the active generation; its texture has one more.
    pub fn layers(&self) -> u32 {
        self.layers
    }

    /// Array index of the spare layer in the active pool texture.
    pub fn spare_layer(&self) -> u32 {
        self.layers
    }

    /// Pool textures this model has asked for: the first generation plus one
    /// per growth. A repack never adds one.
    pub fn texture_allocations(&self) -> u32 {
        self.texture_allocations
    }

    /// Whether a grown-out generation still awaits submitted-work-done.
    pub fn retiring(&self) -> bool {
        self.retiring
    }

    /// The GPU layer reports the retiring generation's submitted work done
    /// and drops it. Growth is possible again from the next drain.
    pub fn release_retirement(&mut self) {
        self.retiring = false;
    }

    /// Layers holding at least one block: one past the highest non-empty.
    pub fn occupied_layers(&self) -> u32 {
        self.pool.extent() as u32
    }

    pub fn block_count(&self) -> u32 {
        self.extents.len() as u32
    }

    pub fn is_resident(&self, block: u32) -> bool {
        self.slots[block as usize].is_some()
    }

    pub fn placement(&self, block: u32) -> Option<BlockPlacement> {
        self.slots[block as usize].map(placement_of)
    }

    /// Resident block ids, in no particular order.
    pub fn resident_blocks(&self) -> &[u32] {
        &self.resident
    }

    /// Table entry `block + 1` as the model holds it now.
    pub fn table_entry(&self, block: u32) -> BlockTableEntry {
        let (width, height) = self.extents[block as usize];
        match self.slots[block as usize] {
            Some(slot) => BlockTableEntry::Resident {
                placement: placement_of(slot),
                width,
                height,
            },
            None => BlockTableEntry::Missing { width, height },
        }
    }

    /// The whole table, for the pool's creation at level install. Later
    /// drains write only [`DrainPlan::table_writes`].
    pub fn table_bytes(&self) -> Vec<u8> {
        let placements: Vec<Option<BlockPlacement>> = self
            .slots
            .iter()
            .map(|slot| slot.map(placement_of))
            .collect();
        block_table_bytes(&self.extents, &placements)
    }

    /// The last drain's plan, updated by `fail_install` and cleared by
    /// `abort_drain`.
    pub fn plan(&self) -> &DrainPlan {
        &self.plan
    }

    fn alloc_extent(&self, block: u32) -> (u32, u32) {
        let (width, height) = self.extents[block as usize];
        (
            width.next_multiple_of(self.alignment),
            height.next_multiple_of(self.alignment),
        )
    }

    /// Layers band blocks may use: the cap, bounded by the texture.
    fn cap_eff(&self) -> u32 {
        self.cap_layers.min(self.layers)
    }

    fn target(&self, block: u32) -> Option<Target> {
        self.targets[block as usize]
    }

    fn is_band(&self, block: u32) -> bool {
        self.target(block).is_some_and(Target::is_band)
    }

    fn band_headroom_texels(&self) -> u64 {
        let layer_texels = u64::from(self.edge) * u64::from(self.edge);
        (0..self.cap_eff() as usize)
            .map(|layer| layer_texels - self.used_texels.get(layer).copied().unwrap_or(0))
            .sum()
    }

    fn report(&self, repacked: bool, grew: bool) -> LightmapPoolReport {
        LightmapPoolReport {
            layers: self.layers,
            band_headroom_texels: self.band_headroom_texels(),
            repacked,
            grew,
            retiring: self.retiring,
        }
    }

    /// Capacities of every per-drain buffer, for the steady-drain allocation
    /// proof.
    #[cfg(test)]
    fn scratch_capacities(&self) -> Vec<usize> {
        let s = &self.scratch;
        let p = &self.plan;
        vec![
            self.resident.capacity(),
            self.used_texels.capacity(),
            self.journal.capacity(),
            self.touched.capacity(),
            s.ready.capacity(),
            s.never_refused.capacity(),
            s.band_ready.capacity(),
            s.victims.capacity(),
            s.over_cap.capacity(),
            s.device_limited.capacity(),
            s.repack_keep.capacity(),
            s.repack_band.capacity(),
            s.repack_residents.capacity(),
            s.moves.capacity(),
            p.copies.capacity(),
            p.uploads.capacity(),
            p.table_writes.capacity(),
            p.installed.capacity(),
            p.refused.capacity(),
            p.deferred.capacity(),
            p.failed.capacity(),
            p.evicted.capacity(),
        ]
    }
}

fn placement_of(slot: Slot) -> BlockPlacement {
    BlockPlacement {
        layer: slot.layer,
        x: slot.x,
        y: slot.y,
    }
}

#[cfg(test)]
mod gpu_mirror;
#[cfg(test)]
mod proptests;
#[cfg(test)]
mod tests;
