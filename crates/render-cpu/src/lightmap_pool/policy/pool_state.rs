// One pool's placements, allocator, reserved room and generation. The lightmap
// pool and the mask pool are two of these under one model.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap_pool::{PerPool, PoolDemand, PoolKind};
use postretro_level_loader::PoolReport;

use super::super::{BlockPool, Slot, place_all_resident};
use super::growth::first_generation;
use super::reservation::DemandRoom;

/// Marks an item absent from `resident`.
const NOT_RESIDENT: u32 = u32::MAX;

/// A pool knows items, not blocks: a lightmap pool item is a block id, and a
/// mask pool item is one plane of one block, so a block's two planes are told
/// apart everywhere a placement is. Absent planes have no item.
///
/// The pool texture always carries `layers() + 1` array layers: the last is
/// the spare a repack stages moves through, never a second pool. `layers()`
/// never passes the device limit the model was built with.
#[derive(Debug)]
pub struct PoolState {
    kind: PoolKind,
    edge: u32,
    alignment: u32,
    /// True extents by item, `(0, 0)` for a plane the block does not store;
    /// allocations round each up to `alignment`.
    extents: Vec<(u32, u32)>,
    /// Every item this pool can hold, ascending.
    items: Vec<u32>,
    pub(super) slots: Vec<Option<Slot>>,
    pub(super) resident: Vec<u32>,
    /// Index into `resident`, [`NOT_RESIDENT`] when absent.
    resident_pos: Vec<u32>,
    pub(super) allocator: BlockPool,
    /// Allocated texels per allocator layer, for band headroom.
    used_texels: Vec<u64>,
    pub(super) room: DemandRoom,
    pub(super) layers: u32,
    pub(super) cap_layers: u32,
    /// Layers every item placed at once fills: no growth reserves past it.
    pub(super) all_resident_layers: u32,
    /// Usable layers the device can hold beside the spare. Growth never
    /// plans past it; a pair that would need it is deferred.
    pub(super) max_layers: u32,
    pub(super) retiring: bool,
    pub(super) texture_allocations: u32,
}

impl PoolState {
    /// `None` for a zero-sized item or one larger than an `edge`² layer.
    pub(super) fn new(
        kind: PoolKind,
        demand: &PoolDemand,
        edge: u32,
        cap_layers: u32,
        max_layers: u32,
    ) -> Option<Self> {
        let items = demand.items(kind);
        let count = match kind {
            PoolKind::Lightmap => demand.block_count(),
            PoolKind::Mask => demand.block_count() * 2,
        };
        let mut extents = vec![(0, 0); count];
        for &item in &items {
            extents[item as usize] = demand.extents()[kind.block_of(item) as usize];
        }
        let alignment = demand.alignment();
        let present: Vec<(u32, u32)> = items.iter().map(|&i| extents[i as usize]).collect();
        let all_resident_layers = place_all_resident(&present, alignment, edge)?.layer_count;
        let layers = first_generation(cap_layers, all_resident_layers, max_layers);
        Some(Self {
            kind,
            edge,
            alignment,
            extents,
            items,
            slots: vec![None; count],
            resident: Vec::new(),
            resident_pos: vec![NOT_RESIDENT; count],
            allocator: BlockPool::new(edge, None),
            used_texels: vec![0; layers as usize],
            room: DemandRoom::new(count),
            layers,
            cap_layers,
            all_resident_layers,
            max_layers,
            retiring: false,
            texture_allocations: u32::from(all_resident_layers > 0),
        })
    }

    pub fn kind(&self) -> PoolKind {
        self.kind
    }

    /// Usable layers of the active generation; its texture has one more.
    pub fn layers(&self) -> u32 {
        self.layers
    }

    /// Array index of the spare layer in the active pool texture.
    pub fn spare_layer(&self) -> u32 {
        self.layers
    }

    /// Pool textures this pool has asked for: the first generation plus one
    /// per growth. A repack never adds one. None for a pool with nothing to
    /// hold.
    pub fn texture_allocations(&self) -> u32 {
        self.texture_allocations
    }

    /// Whether a grown-out generation still awaits submitted-work-done.
    pub fn retiring(&self) -> bool {
        self.retiring
    }

    /// Layers holding at least one item: one past the highest non-empty.
    pub fn occupied_layers(&self) -> u32 {
        self.allocator.extent() as u32
    }

    /// Resident items, in no particular order.
    pub fn resident_items(&self) -> &[u32] {
        &self.resident
    }

    pub(super) fn edge(&self) -> u32 {
        self.edge
    }

    pub(super) fn alignment(&self) -> u32 {
        self.alignment
    }

    pub(super) fn items(&self) -> &[u32] {
        &self.items
    }

    pub(super) fn slot(&self, item: u32) -> Option<Slot> {
        self.slots[item as usize]
    }

    pub(super) fn alloc_extent(&self, item: u32) -> (u32, u32) {
        let (width, height) = self.extents[item as usize];
        (
            width.next_multiple_of(self.alignment),
            height.next_multiple_of(self.alignment),
        )
    }

    /// Layers band blocks may use: the cap, bounded by the texture.
    pub(super) fn cap_eff(&self) -> u32 {
        self.cap_layers.min(self.layers)
    }

    /// First fit under the cap, then within the current layers.
    pub(super) fn allocate_in_layers(&mut self, width: u32, height: u32) -> Option<Slot> {
        let cap = self.cap_eff();
        let under_cap = self
            .allocator
            .allocate_within(width, height, Some(cap as usize));
        if under_cap.is_some() || self.layers <= cap {
            return under_cap;
        }
        let layers = self.layers as usize;
        self.allocator.allocate_within(width, height, Some(layers))
    }

    pub(super) fn attach(&mut self, item: u32, slot: Slot) {
        let at = item as usize;
        debug_assert!(self.slots[at].is_none(), "item {item} placed twice");
        self.slots[at] = Some(slot);
        self.resident_pos[at] = self.resident.len() as u32;
        self.resident.push(item);
        let layer = slot.layer as usize;
        if layer >= self.used_texels.len() {
            self.used_texels.resize(layer + 1, 0);
        }
        self.used_texels[layer] += u64::from(slot.width) * u64::from(slot.height);
    }

    pub(super) fn detach(&mut self, item: u32) -> Slot {
        let at = item as usize;
        let slot = self.slots[at].take().expect("detach a resident item");
        let pos = self.resident_pos[at] as usize;
        self.resident.swap_remove(pos);
        if let Some(&moved) = self.resident.get(pos) {
            self.resident_pos[moved as usize] = pos as u32;
        }
        self.resident_pos[at] = NOT_RESIDENT;
        self.used_texels[slot.layer as usize] -= u64::from(slot.width) * u64::from(slot.height);
        slot
    }

    /// Texels free under the cap, reserved room left out. A reserved pair the
    /// controller holds in hand is charged here and not again there: the
    /// plan's `room_reserved` tells it which.
    fn band_headroom_texels(&self) -> u64 {
        let layer_texels = u64::from(self.edge) * u64::from(self.edge);
        (0..self.cap_eff() as usize)
            .map(|layer| {
                let used = self.used_texels.get(layer).copied().unwrap_or(0);
                layer_texels - used - self.room.held_texels_in(layer)
            })
            .sum()
    }

    pub(super) fn report(&self, repacked: bool, grew: bool) -> PoolReport {
        PoolReport {
            layers: self.layers,
            band_headroom_texels: self.band_headroom_texels(),
            repacked,
            grew,
            retiring: self.retiring,
        }
    }

    #[cfg(test)]
    pub(super) fn capacities(&self) -> Vec<usize> {
        let mut capacities = vec![self.resident.capacity(), self.used_texels.capacity()];
        capacities.extend(self.room.capacities());
        capacities
    }
}

/// Both pools, for a model of `demand`. `None` as [`PoolState::new`].
pub(super) fn new_pools(
    demand: &PoolDemand,
    edge: u32,
    caps: PerPool<u32>,
    max_layers: u32,
) -> Option<PerPool<PoolState>> {
    Some(PerPool {
        lightmap: PoolState::new(PoolKind::Lightmap, demand, edge, caps.lightmap, max_layers)?,
        mask: PoolState::new(PoolKind::Mask, demand, edge, caps.mask, max_layers)?,
    })
}
