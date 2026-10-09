//! Block demand from the baked residency set, pins, and this frame's drawn cells.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::cell_residency_set::{CellResidencySetSection, ResidencyEntry};
use postretro_level_loader::{LightmapBlockClass, LightmapTarget};
use postretro_visibility::{VisibilityPath, VisibleCells};

use super::block_map::LevelBlockMap;
use crate::streaming::drain_budget::{DrainClass, DrainRank};

/// A targeted block's wire class and lead, and its rank on the shared drain
/// scale: visible → `Visible`, pinned → `Pinned`, mandatory by lead →
/// `Lead`, band → `Prefetch` at its cluster's authored priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockTarget {
    pub(crate) class: LightmapBlockClass,
    pub(crate) drain_class: DrainClass,
    pub(crate) lead: u32,
    pub(crate) priority: u32,
}

impl BlockTarget {
    const PINNED: Self = Self {
        class: LightmapBlockClass::Mandatory,
        drain_class: DrainClass::Pinned,
        lead: 0,
        priority: 0,
    };
    const VISIBLE: Self = Self {
        class: LightmapBlockClass::Visible,
        drain_class: DrainClass::Visible,
        lead: 0,
        priority: 0,
    };

    pub(crate) fn wire(self, block: u32) -> LightmapTarget {
        LightmapTarget {
            block,
            class: self.class,
            lead: self.lead,
        }
    }

    pub(crate) fn rank(self, block: u32) -> DrainRank {
        DrainRank::lightmap_block(self.drain_class, self.priority, self.lead, block)
    }

    /// The target of a block a non-portal frame draws while its pair is
    /// resident or on its way: mandatory only while demand names it
    /// mandatory, otherwise visible. The renderer never evicts either, so the
    /// drawn block is not freed under the frame that draws it, and a stale
    /// mandatory target never lingers in the mandatory gauge.
    pub(crate) fn held(demanded: Option<Self>) -> Self {
        match demanded {
            Some(target) if target.class == LightmapBlockClass::Mandatory => target,
            _ => Self::VISIBLE,
        }
    }
}

/// What the baked set says about a block from the current camera cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Baked {
    None,
    /// Lead within L.
    Mandatory(u32),
    /// L < lead <= the baked maximum.
    Band(u32),
}

#[derive(Debug, Clone, Copy)]
struct DemandSlot {
    baked: Baked,
    /// Drawn on the latest portal-walk frame.
    drawn: bool,
    drawn_epoch: u32,
    /// Drawn on the latest non-portal frame: the controller raises its target
    /// to at least visible while its pair is resident or on its way.
    held: bool,
    held_epoch: u32,
    /// Queued in `BlockDemand::dirty`.
    dirty: bool,
}

/// How a visibility path shapes this frame's demand. Only a portal walk adds
/// drawn cells to demand; every other path demands the camera cell's baked
/// set alone, and its drawn blocks are only held.
enum PathDemand {
    /// Baked set plus visible demand from the drawn cells.
    PortalWalk,
    /// Baked set; the frustum-culled drawn cells are held, never demanded.
    CameraSet,
    /// Solid or exterior camera cell: as `CameraSet` when the cell has a
    /// baked set; otherwise keep current demand and request nothing new.
    CameraSetOrHold,
    /// Empty world: no residency-set lookup, no change, no requests.
    Hold,
}

impl PathDemand {
    fn of(path: VisibilityPath) -> Self {
        match path {
            VisibilityPath::PrlPortal { .. } => Self::PortalWalk,
            VisibilityPath::PortalStepLimitFallback { .. } | VisibilityPath::NoPortalsFallback => {
                Self::CameraSet
            }
            VisibilityPath::SolidCellFallback | VisibilityPath::ExteriorCellFallback => {
                Self::CameraSetOrHold
            }
            VisibilityPath::EmptyWorldFallback => Self::Hold,
        }
    }
}

/// One frame's visibility, as lightmap demand reads it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DemandFrame<'a> {
    /// The level's id-51 set; the same level the controller was built for.
    pub(crate) residency_set: &'a CellResidencySetSection,
    pub(crate) camera_cell: u32,
    pub(crate) path: VisibilityPath,
    pub(crate) visible_cells: &'a VisibleCells,
}

impl DemandFrame<'_> {
    pub(crate) fn is_portal_walk(&self) -> bool {
        matches!(self.path, VisibilityPath::PrlPortal { .. })
    }

    /// Whether the frame draws cells whose visible misses count: every path
    /// but the empty world, which draws no cell.
    pub(crate) fn draws_cells(&self) -> bool {
        !matches!(self.path, VisibilityPath::EmptyWorldFallback)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DemandCounters {
    /// Times demand was rebuilt from the baked set (camera cell or L changed).
    pub(crate) baked_recomputes: u64,
    /// `CellResidencySetSection::entries_for` calls.
    pub(crate) residency_lookups: u64,
}

/// Per-block demand state plus the change list the controller consumes.
///
/// The baked part is recomputed only when the camera cell or lead L changes,
/// once per frame even if both change. Visible demand reuses its buffers:
/// a steady frame touches only this frame's drawn blocks and allocates
/// nothing.
#[derive(Debug)]
pub(crate) struct BlockDemand {
    slots: Vec<DemandSlot>,
    /// Blocks the current baked set names, to clear on the next recompute.
    baked_blocks: Vec<u32>,
    /// This portal frame's drawn blocks; the previous frame's while marking.
    drawn: Vec<u32>,
    drawn_scratch: Vec<u32>,
    /// This non-portal frame's drawn blocks; the previous frame's while
    /// marking.
    held: Vec<u32>,
    held_scratch: Vec<u32>,
    /// Whether the latest drawn set is `held` (a non-portal frame) rather
    /// than `drawn`.
    drawn_set_is_held: bool,
    /// Blocks whose demand may have changed since the controller last looked.
    dirty: Vec<u32>,
    /// Camera cell and lead the baked part was computed for.
    key: Option<(u32, u32)>,
    epoch: u32,
    held_epoch: u32,
    counters: DemandCounters,
}

impl BlockDemand {
    /// Pinned blocks start dirty: they are targets from the first frame.
    pub(crate) fn new(map: &LevelBlockMap) -> Self {
        let mut demand = Self {
            slots: vec![
                DemandSlot {
                    baked: Baked::None,
                    drawn: false,
                    drawn_epoch: 0,
                    held: false,
                    held_epoch: 0,
                    dirty: false,
                };
                map.block_count()
            ],
            baked_blocks: Vec::new(),
            drawn: Vec::new(),
            drawn_scratch: Vec::new(),
            held: Vec::new(),
            held_scratch: Vec::new(),
            drawn_set_is_held: false,
            dirty: Vec::new(),
            key: None,
            epoch: 0,
            held_epoch: 0,
            counters: DemandCounters::default(),
        };
        for &block in map.pinned_blocks() {
            demand.mark_dirty(block);
        }
        demand
    }

    /// Applies one frame. Returns whether the frame may issue new reads.
    ///
    /// A non-portal frame demands only the camera cell's baked set. Its drawn
    /// blocks are held instead: the controller raises a held block to at
    /// least visible while its pair is resident or on its way, so the
    /// renderer never frees a block the frame draws, and never reads one for
    /// it.
    pub(crate) fn update(
        &mut self,
        map: &LevelBlockMap,
        lead: u32,
        frame: DemandFrame<'_>,
    ) -> bool {
        let camera_cell = frame.camera_cell;
        match PathDemand::of(frame.path) {
            PathDemand::Hold => false,
            PathDemand::CameraSetOrHold => {
                if self.key != Some((camera_cell, lead)) {
                    let entries = self.lookup(frame.residency_set, camera_cell);
                    if entries.is_empty() {
                        // Current demand stands. Holds follow what this frame
                        // draws, and its visible misses are counted.
                        self.mark_held(map, frame.visible_cells);
                        return false;
                    }
                    self.recompute(map, camera_cell, lead, entries);
                }
                self.clear_drawn();
                self.mark_held(map, frame.visible_cells);
                true
            }
            PathDemand::CameraSet => {
                self.recompute_if_changed(map, frame.residency_set, camera_cell, lead);
                self.clear_drawn();
                self.mark_held(map, frame.visible_cells);
                true
            }
            PathDemand::PortalWalk => {
                self.recompute_if_changed(map, frame.residency_set, camera_cell, lead);
                self.clear_held();
                self.mark_drawn(map, frame.visible_cells);
                true
            }
        }
    }

    /// Capture's fixed view: the camera cell's baked set, plus every drawn
    /// cell's blocks as visible whatever the visibility path. An empty world
    /// looks up no residency set.
    pub(crate) fn update_capture_view(
        &mut self,
        map: &LevelBlockMap,
        lead: u32,
        frame: DemandFrame<'_>,
    ) {
        if frame.draws_cells() {
            self.recompute_if_changed(map, frame.residency_set, frame.camera_cell, lead);
        }
        self.clear_held();
        self.mark_drawn(map, frame.visible_cells);
    }

    /// Demand from `camera_cell`'s baked set and the pins alone, with no drawn
    /// cells: level install knows the spawn camera cell before any frame has
    /// walked its portals. An empty range leaves only the pins.
    #[cfg(test)]
    pub(crate) fn update_camera_set(
        &mut self,
        map: &LevelBlockMap,
        lead: u32,
        residency_set: &CellResidencySetSection,
        camera_cell: u32,
    ) {
        self.recompute_if_changed(map, residency_set, camera_cell, lead);
        self.clear_drawn();
        self.clear_held();
    }

    /// The block's current target, or `None` when nothing demands it.
    pub(crate) fn target(&self, map: &LevelBlockMap, block: u32) -> Option<BlockTarget> {
        let facts = map.facts(block);
        if facts.pinned {
            return Some(BlockTarget::PINNED);
        }
        let slot = &self.slots[block as usize];
        match slot.baked {
            Baked::Mandatory(lead) => Some(BlockTarget {
                class: LightmapBlockClass::Mandatory,
                drain_class: DrainClass::Lead,
                lead,
                priority: 0,
            }),
            _ if slot.drawn => Some(BlockTarget::VISIBLE),
            Baked::Band(lead) => Some(BlockTarget {
                class: LightmapBlockClass::Band,
                drain_class: DrainClass::Prefetch,
                lead,
                priority: u32::from(facts.priority),
            }),
            Baked::None => None,
        }
    }

    pub(crate) fn dirty_len(&self) -> usize {
        self.dirty.len()
    }

    pub(crate) fn dirty_at(&self, index: usize) -> u32 {
        self.dirty[index]
    }

    pub(crate) fn clear_dirty(&mut self) {
        for &block in &self.dirty {
            self.slots[block as usize].dirty = false;
        }
        self.dirty.clear();
    }

    /// Every block some demand names: pins, the baked set, and the drawn
    /// blocks. May repeat a block.
    pub(crate) fn demanded_blocks<'a>(
        &'a self,
        map: &'a LevelBlockMap,
    ) -> impl Iterator<Item = u32> + 'a {
        map.pinned_blocks()
            .iter()
            .chain(&self.baked_blocks)
            .chain(&self.drawn)
            .copied()
    }

    /// Blocks the latest frame drew: the portal walk's drawn blocks, or a
    /// non-portal frame's held ones.
    pub(crate) fn drawn_blocks(&self) -> &[u32] {
        if self.drawn_set_is_held {
            &self.held
        } else {
            &self.drawn
        }
    }

    /// Whether the latest non-portal frame drew `block`.
    pub(crate) fn is_held(&self, block: u32) -> bool {
        self.slots[block as usize].held
    }

    /// `block`'s pair came or went. A held block's target depends on whether
    /// it has a pair, so a held block is queued for the next retarget.
    pub(crate) fn recheck_if_held(&mut self, block: u32) {
        if self.slots[block as usize].held {
            self.mark_dirty(block);
        }
    }

    /// Whether the camera cell's baked set (lead L, the band) or the pins
    /// name `block`: all a non-portal frame may read.
    pub(crate) fn in_camera_set(&self, map: &LevelBlockMap, block: u32) -> bool {
        map.facts(block).pinned || self.slots[block as usize].baked != Baked::None
    }

    pub(crate) fn counters(&self) -> DemandCounters {
        self.counters
    }

    #[cfg(test)]
    pub(crate) fn buffer_capacities(&self) -> [usize; 6] {
        [
            self.baked_blocks.capacity(),
            self.drawn.capacity(),
            self.drawn_scratch.capacity(),
            self.held.capacity(),
            self.held_scratch.capacity(),
            self.dirty.capacity(),
        ]
    }

    fn lookup<'s>(
        &mut self,
        set: &'s CellResidencySetSection,
        camera_cell: u32,
    ) -> &'s [ResidencyEntry] {
        self.counters.residency_lookups += 1;
        set.entries_for(camera_cell as usize)
    }

    fn recompute_if_changed(
        &mut self,
        map: &LevelBlockMap,
        set: &CellResidencySetSection,
        camera_cell: u32,
        lead: u32,
    ) {
        if self.key != Some((camera_cell, lead)) {
            let entries = self.lookup(set, camera_cell);
            self.recompute(map, camera_cell, lead, entries);
        }
    }

    /// Replaces the baked part with `entries` split at `lead`. Blocks in
    /// either the old or the new set are marked dirty; the controller drops
    /// the ones whose target did not change.
    fn recompute(
        &mut self,
        map: &LevelBlockMap,
        camera_cell: u32,
        lead: u32,
        entries: &[ResidencyEntry],
    ) {
        self.counters.baked_recomputes += 1;
        for index in 0..self.baked_blocks.len() {
            let block = self.baked_blocks[index];
            self.slots[block as usize].baked = Baked::None;
            self.mark_dirty(block);
        }
        self.baked_blocks.clear();
        // A cell demands all of its blocks, which share its lead.
        for entry in entries {
            for block in map.blocks_of_cell(entry.cell_id) {
                self.slots[block as usize].baked = if entry.lead <= lead {
                    Baked::Mandatory(entry.lead)
                } else {
                    Baked::Band(entry.lead)
                };
                self.mark_dirty(block);
                self.baked_blocks.push(block);
            }
        }
        self.key = Some((camera_cell, lead));
    }

    fn mark_drawn(&mut self, map: &LevelBlockMap, visible_cells: &VisibleCells) {
        self.epoch = self.epoch.wrapping_add(1);
        self.drawn_set_is_held = false;
        std::mem::swap(&mut self.drawn, &mut self.drawn_scratch);
        self.drawn.clear();
        for block in drawn_blocks_of(map, visible_cells) {
            let slot = &mut self.slots[block as usize];
            slot.drawn_epoch = self.epoch;
            if !slot.drawn {
                slot.drawn = true;
                self.mark_dirty(block);
            }
            self.drawn.push(block);
        }
        for index in 0..self.drawn_scratch.len() {
            let block = self.drawn_scratch[index];
            let slot = &mut self.slots[block as usize];
            if slot.drawn && slot.drawn_epoch != self.epoch {
                slot.drawn = false;
                self.mark_dirty(block);
            }
        }
        self.drawn_scratch.clear();
    }

    fn clear_drawn(&mut self) {
        for index in 0..self.drawn.len() {
            let block = self.drawn[index];
            self.slots[block as usize].drawn = false;
            self.mark_dirty(block);
        }
        self.drawn.clear();
    }

    /// Records a non-portal frame's drawn blocks as held. A block that
    /// becomes held or stops being held is marked dirty, so the controller
    /// re-evaluates it: raises it while it has a pair, or releases it.
    fn mark_held(&mut self, map: &LevelBlockMap, visible_cells: &VisibleCells) {
        self.held_epoch = self.held_epoch.wrapping_add(1);
        self.drawn_set_is_held = true;
        std::mem::swap(&mut self.held, &mut self.held_scratch);
        self.held.clear();
        for block in drawn_blocks_of(map, visible_cells) {
            let slot = &mut self.slots[block as usize];
            slot.held_epoch = self.held_epoch;
            if !slot.held {
                slot.held = true;
                self.mark_dirty(block);
            }
            self.held.push(block);
        }
        for index in 0..self.held_scratch.len() {
            let block = self.held_scratch[index];
            let slot = &mut self.slots[block as usize];
            if slot.held && slot.held_epoch != self.held_epoch {
                slot.held = false;
                self.mark_dirty(block);
            }
        }
        self.held_scratch.clear();
    }

    /// Ends every hold: a portal walk decides visible demand itself.
    fn clear_held(&mut self) {
        for index in 0..self.held.len() {
            let block = self.held[index];
            let slot = &mut self.slots[block as usize];
            if slot.held {
                slot.held = false;
                self.mark_dirty(block);
            }
        }
        self.held.clear();
    }

    fn mark_dirty(&mut self, block: u32) {
        let slot = &mut self.slots[block as usize];
        if !slot.dirty {
            slot.dirty = true;
            self.dirty.push(block);
        }
    }
}

/// The blocks of the cells `visible_cells` draws; every block for `DrawAll`.
/// A cell without charts has no block; a cell may have several.
fn drawn_blocks_of<'a>(
    map: &'a LevelBlockMap,
    visible_cells: &'a VisibleCells,
) -> impl Iterator<Item = u32> + 'a {
    let (cells, every) = match visible_cells {
        VisibleCells::Culled(cells) => (cells.as_slice(), 0..0),
        VisibleCells::DrawAll => (&[][..], 0..map.block_count() as u32),
    };
    cells
        .iter()
        .flat_map(|&cell| map.blocks_of_cell(cell))
        .chain(every)
}
