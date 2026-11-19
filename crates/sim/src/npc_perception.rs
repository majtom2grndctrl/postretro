// Which cells players can perceive this tick, for the AI scheduler's tiers.
// The engine step locates each player pawn's cell and reads that cell's id-51
// residency set within the AI lead; the AI policy unions the sets, couples
// armed ghost links, and falls back to "every NPC perceived" when id 51 or a
// perceiver is missing. The scheduler's NPC cell table lives here too.
// See: context/lib/entity_model.md §7c, build_pipeline.md §PRL section IDs (51, 52)

use std::collections::HashMap;
use std::ops::Range;

use glam::Vec3;
use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform};
use postretro_level_format::cell_residency_set::{CellResidencySetSection, ResidencyEntry};
use postretro_level_format::cell_visibility::CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;
use postretro_level_loader::LevelWorld;

/// The AI lead when a map does not author `ai_wake_lead`, metres. Its own
/// constant, never streaming's runtime lead: retuning streaming moves no NPC's
/// tier. A dormant NPC at the fastest authored travel speed covers about 2.5 m
/// per 2 Hz step, well inside it.
pub const DEFAULT_AI_LEAD_METRES: f32 = 16.0;

/// The spatial reads the perception step makes against the loaded level.
pub trait LevelCells {
    /// The runtime cell containing `position`.
    fn locate_cell(&self, position: Vec3) -> usize;
    /// The level's id-51 residency relation, when it carries one.
    fn residency(&self) -> Option<&CellResidencySetSection>;
}

impl LevelCells for LevelWorld {
    fn locate_cell(&self, position: Vec3) -> usize {
        LevelWorld::locate_cell(self, position)
    }

    fn residency(&self) -> Option<&CellResidencySetSection> {
        self.cell_residency_set.as_ref()
    }
}

/// The level-side input to one tick's perception step.
pub struct PerceptionInput<'a> {
    pub level: &'a dyn LevelCells,
    pub state: &'a mut NpcPerception,
}

/// One ghost link's two cell lists, as ranges into [`GhostLinkCells::cells`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct GhostLinkPair {
    /// The link's id-56 record index, as `GhostLinkComponent::index` names it.
    link: u16,
    volume: Range<usize>,
    image: Range<usize>,
}

/// The cells under each ghost link's volume and under its image through the
/// link's seamless mapping, computed once at level load. While a link is
/// armed, a perceived cell on either side makes the other side perceived.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GhostLinkCells {
    pairs: Vec<GhostLinkPair>,
    cells: Vec<u32>,
}

impl GhostLinkCells {
    /// Every non-solid cell whose bounds overlap a link's brushes, and every
    /// one whose bounds overlap the brushes' image under the link's mapping.
    /// Bounds overlap is conservative: a wider set only keeps more NPCs
    /// perceived, the safe direction.
    pub fn build(level: &LevelWorld) -> Self {
        let mut built = Self::default();
        for (index, record) in level.ghost_links.links.iter().enumerate() {
            let Ok(link) = u16::try_from(index) else {
                break;
            };
            let mapping = postretro_foundation::movement::teleport::SeamlessMapping::new(
                Vec3::from(record.landmark_origin),
                record.landmark_yaw,
                Vec3::from(record.target_origin),
                record.target_yaw,
            );
            let boxes: Vec<(Vec3, Vec3)> = record
                .brushes
                .iter()
                .map(|brush| (Vec3::from(brush.aabb_min), Vec3::from(brush.aabb_max)))
                .collect();
            let images: Vec<(Vec3, Vec3)> = boxes
                .iter()
                .map(|&(min, max)| mapped_bounds(min, max, |p| mapping.point(p)))
                .collect();
            let cell_bounds = level
                .cells
                .iter()
                .map(|cell| (!cell.is_solid).then_some((cell.bounds_min, cell.bounds_max)));
            built.push(link, &boxes, &images, cell_bounds);
        }
        built
    }

    /// Record one link from its volume and image boxes against every cell's
    /// bounds (`None` for a solid cell). The test seam for synthetic levels.
    pub fn push(
        &mut self,
        link: u16,
        volume: &[(Vec3, Vec3)],
        image: &[(Vec3, Vec3)],
        cell_bounds: impl Iterator<Item = Option<(Vec3, Vec3)>> + Clone,
    ) {
        let volume = self.extend_overlapping(volume, cell_bounds.clone());
        let image = self.extend_overlapping(image, cell_bounds);
        self.pairs.push(GhostLinkPair {
            link,
            volume,
            image,
        });
    }

    /// Record one link from explicit cell lists. The test seam for synthetic
    /// levels whose cells have no bounds.
    pub fn push_cells(&mut self, link: u16, volume: &[u32], image: &[u32]) {
        let start = self.cells.len();
        self.cells.extend_from_slice(volume);
        let volume = start..self.cells.len();
        self.cells.extend_from_slice(image);
        let image = volume.end..self.cells.len();
        self.pairs.push(GhostLinkPair {
            link,
            volume,
            image,
        });
    }

    fn extend_overlapping(
        &mut self,
        boxes: &[(Vec3, Vec3)],
        cell_bounds: impl Iterator<Item = Option<(Vec3, Vec3)>>,
    ) -> Range<usize> {
        let start = self.cells.len();
        for (cell, bounds) in cell_bounds.enumerate() {
            let Some((cell_min, cell_max)) = bounds else {
                continue;
            };
            let overlaps = boxes
                .iter()
                .any(|&(min, max)| cell_min.cmple(max).all() && min.cmple(cell_max).all());
            if overlaps {
                self.cells.push(cell as u32);
            }
        }
        start..self.cells.len()
    }

    pub fn link_count(&self) -> usize {
        self.pairs.len()
    }

    /// Each link's record index and its volume and image cells, in link order.
    pub fn links(&self) -> impl Iterator<Item = (u16, &[u32], &[u32])> {
        self.pairs.iter().map(|pair| {
            (
                pair.link,
                &self.cells[pair.volume.clone()],
                &self.cells[pair.image.clone()],
            )
        })
    }
}

/// The axis-aligned bounds of a box's eight corners under `map`.
fn mapped_bounds(min: Vec3, max: Vec3, map: impl Fn(Vec3) -> Vec3) -> (Vec3, Vec3) {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for corner in 0..8u8 {
        let point = Vec3::new(
            if corner & 1 == 0 { min.x } else { max.x },
            if corner & 2 == 0 { min.y } else { max.y },
            if corner & 4 == 0 { min.z } else { max.z },
        );
        let mapped = map(point);
        lo = lo.min(mapped);
        hi = hi.max(mapped);
    }
    (lo, hi)
}

/// The AI lead in id-46 fixed-point units: the map's `ai_wake_lead` or the
/// default, capped at id 51's baked maximum. An authored lead above the cap is
/// warned about once, here, at level load.
pub fn resolve_ai_lead(authored_metres: Option<f32>, baked_max_lead: Option<u32>) -> u32 {
    let metres = authored_metres.unwrap_or(DEFAULT_AI_LEAD_METRES).max(0.0);
    let lead = (metres * CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE as f32).round() as u32;
    match baked_max_lead {
        Some(max) if lead > max => {
            if authored_metres.is_some() {
                log::warn!(
                    "[AI] worldspawn `ai_wake_lead` {metres} m is past this level's baked \
                     visibility lead; capped to it, {} m",
                    max as f32 / CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE as f32,
                );
            }
            max
        }
        _ => lead,
    }
}

/// Why this tick runs every NPC perceived, when it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerceiveAll {
    /// The level carries no id-51 relation, or one with no camera sets.
    NoResidencySet,
    /// No player pawn is in the registry this tick.
    NoPerceiver,
    /// A perceiver stands in a cell with no camera set (solid or exterior),
    /// so what it sees is unknown.
    PerceiverOutsideCameraCells,
}

/// The NPC cell table's entry: where the NPC was located and in which cell.
#[derive(Debug, Clone, Copy)]
struct NpcCell {
    position: Vec3,
    cell: u32,
    /// The step stamp that last read this entry, to prune departed NPCs.
    seen: u32,
}

/// The AI scheduler's perception state for the loaded level: the resolved AI
/// lead, ghost-link cells, this tick's perceived cells and each NPC's cell.
/// Built once per level load; [`Self::step`] reuses every buffer, so a tick
/// allocates nothing once the NPC table has seen each NPC.
#[derive(Debug, Clone)]
pub struct NpcPerception {
    lead: u32,
    ghost: GhostLinkCells,
    /// Per cell: the step stamp that last marked it perceived.
    perceived: Vec<u32>,
    /// Per ghost link: which sides this step couples (bit 0 volume → image,
    /// bit 1 image → volume).
    couples: Vec<u8>,
    stamp: u32,
    all: Option<PerceiveAll>,
    npc_cells: HashMap<EntityId, NpcCell>,
}

impl NpcPerception {
    /// Perception for a loaded level: its authored lead, id 51 and ghost links.
    pub fn for_level(level: &LevelWorld) -> Self {
        let residency = level.cell_residency_set.as_ref();
        Self::new(
            level.cells.len(),
            resolve_ai_lead(
                level.environment_volumes.ai_wake_lead,
                residency.map(|set| set.max_lead),
            ),
            GhostLinkCells::build(level),
        )
    }

    /// Perception over `cell_count` cells at an already resolved lead.
    pub fn new(cell_count: usize, lead: u32, ghost: GhostLinkCells) -> Self {
        Self {
            lead,
            couples: vec![0; ghost.link_count()],
            ghost,
            perceived: vec![0; cell_count],
            stamp: 0,
            all: Some(PerceiveAll::NoPerceiver),
            npc_cells: HashMap::new(),
        }
    }

    /// The resolved AI lead, id-46 fixed-point units.
    pub fn lead(&self) -> u32 {
        self.lead
    }

    /// Recompute this tick's perceived cells from every perceiver's position.
    /// Perceivers are the tick's player pawns, alive, dead or down. Runs after
    /// movement and teleports, so a pawn's cell is where it stands now.
    pub fn step(
        &mut self,
        level: &dyn LevelCells,
        registry: &EntityRegistry,
        perceivers: impl IntoIterator<Item = EntityId>,
    ) {
        self.advance_stamp();
        self.all = self.union(level, registry, perceivers);
        if self.all.is_none() {
            self.couple_ghost_links(registry);
        }
        self.prune_departed();
    }

    fn advance_stamp(&mut self) {
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == 0 {
            // Wrapped: no old mark may read as current.
            self.perceived.fill(0);
            for entry in self.npc_cells.values_mut() {
                entry.seen = 0;
            }
            self.stamp = 1;
        }
    }

    fn union(
        &mut self,
        level: &dyn LevelCells,
        registry: &EntityRegistry,
        perceivers: impl IntoIterator<Item = EntityId>,
    ) -> Option<PerceiveAll> {
        let Some(residency) = level.residency().filter(|set| set.has_camera_sets()) else {
            return Some(PerceiveAll::NoResidencySet);
        };
        let mut any = false;
        for pawn in perceivers {
            let Ok(transform) = registry.get_component::<Transform>(pawn) else {
                continue;
            };
            any = true;
            let cell = level.locate_cell(transform.position);
            let entries = residency.entries_for(cell);
            if entries.is_empty() {
                return Some(PerceiveAll::PerceiverOutsideCameraCells);
            }
            for entry in within_lead(entries, self.lead) {
                if let Some(mark) = self.perceived.get_mut(entry.cell_id as usize) {
                    *mark = self.stamp;
                }
            }
        }
        (!any).then_some(PerceiveAll::NoPerceiver)
    }

    /// Armed links couple their two sides, decided against the union before
    /// any coupling so link order never chains one link through another.
    fn couple_ghost_links(&mut self, registry: &EntityRegistry) {
        if self.ghost.pairs.is_empty() {
            return;
        }
        self.couples.fill(0);
        for (_, value) in registry.iter_with_kind(ComponentKind::GhostLink) {
            let ComponentValue::GhostLink(link) = value else {
                continue;
            };
            if !link.armed {
                continue;
            }
            for (slot, pair) in self.ghost.pairs.iter().enumerate() {
                if pair.link != link.index {
                    continue;
                }
                let seen = |range: &Range<usize>| {
                    self.ghost.cells[range.clone()]
                        .iter()
                        .any(|&cell| self.perceived[cell as usize] == self.stamp)
                };
                self.couples[slot] =
                    u8::from(seen(&pair.volume)) | (u8::from(seen(&pair.image)) << 1);
            }
        }
        for (slot, pair) in self.ghost.pairs.iter().enumerate() {
            let couples = self.couples[slot];
            if couples & 1 != 0 {
                for &cell in &self.ghost.cells[pair.image.clone()] {
                    self.perceived[cell as usize] = self.stamp;
                }
            }
            if couples & 2 != 0 {
                for &cell in &self.ghost.cells[pair.volume.clone()] {
                    self.perceived[cell as usize] = self.stamp;
                }
            }
        }
    }

    /// Drop table entries no lookup read during the previous step. Runs after
    /// this step's union, before this tick's lookups, so an NPC read every tick
    /// keeps its entry. `retain` keeps the table's capacity.
    fn prune_departed(&mut self) {
        let previous = self.stamp.wrapping_sub(1);
        self.npc_cells
            .retain(|_, entry| entry.seen == previous || entry.seen == self.stamp);
    }

    /// Why every NPC is perceived this tick, or `None` when the per-cell union
    /// decides.
    pub fn perceives_all(&self) -> Option<PerceiveAll> {
        self.all
    }

    /// Whether `cell` is in this tick's union.
    pub fn cell_perceived(&self, cell: usize) -> bool {
        self.all.is_some()
            || self
                .perceived
                .get(cell)
                .is_some_and(|&mark| mark == self.stamp)
    }

    /// Whether the NPC at `position` is perceived this tick. Re-locates it
    /// only when its position changed since the table last located it, so a
    /// spawn, a reused slot, and a script, mover or knockback move all read
    /// their new cell on the next lookup.
    pub fn npc_perceived(&mut self, level: &dyn LevelCells, npc: EntityId, position: Vec3) -> bool {
        if self.all.is_some() {
            return true;
        }
        let cell = self.npc_cell(level, npc, position);
        self.cell_perceived(cell as usize)
    }

    /// The NPC's cell, from the table or re-located.
    pub fn npc_cell(&mut self, level: &dyn LevelCells, npc: EntityId, position: Vec3) -> u32 {
        let stamp = self.stamp;
        match self.npc_cells.get_mut(&npc) {
            Some(entry) if entry.position == position => {
                entry.seen = stamp;
                entry.cell
            }
            Some(entry) => {
                entry.position = position;
                entry.cell = level.locate_cell(position) as u32;
                entry.seen = stamp;
                entry.cell
            }
            None => {
                let cell = level.locate_cell(position) as u32;
                self.npc_cells.insert(
                    npc,
                    NpcCell {
                        position,
                        cell,
                        seen: stamp,
                    },
                );
                cell
            }
        }
    }

    /// Entries in the NPC cell table.
    pub fn tracked_npcs(&self) -> usize {
        self.npc_cells.len()
    }
}

/// The prefix of a camera cell's (lead, cell)-sorted entries at `lead`.
fn within_lead(entries: &[ResidencyEntry], lead: u32) -> &[ResidencyEntry] {
    &entries[..entries.partition_point(|entry| entry.lead <= lead)]
}

#[cfg(test)]
#[path = "npc_perception/tests.rs"]
mod tests;
