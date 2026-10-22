// Installed teleporter geometry, authored poses and stable local entity bindings.
// See: context/lib/movement.md §6 · context/lib/entity_model.md §7

use crate::environment_volumes::convex_leaf;
use postretro_collision::TeleporterRegions;
use postretro_entities::EntityId;
use postretro_level_format::teleporters::{TeleporterRecord, TeleportersSection};
use std::sync::atomic::{AtomicBool, Ordering};

/// A level's installed teleporters, all indexed by map order. The static
/// collision world owns them, so host movement, client prediction and reconcile
/// replay read one copy through the same tick.
///
/// Geometry and authored records are fixed at install; only the entity
/// bindings are written after it. Armed state lives on the bound entities'
/// `TeleporterComponent`, which scripts change on the host and replication
/// carries to clients.
#[derive(Debug, Default)]
pub struct Teleporters {
    /// Indexed brush unions for the post-step body-point test and arrival latches.
    pub regions: TeleporterRegions,
    /// Authored poses, mode, cue and spawn state, as loaded from the PRL section.
    records: Vec<TeleporterRecord>,
    /// The installed entity holding each teleporter's armed state. Bound once
    /// at install to the existing entity; receiving replication never spawns
    /// a second one.
    entities: Vec<Option<EntityId>>,
    /// One blocked-arrival warning per teleporter. Movement holds the world by
    /// shared reference, so the flag needs interior mutability; atomics keep
    /// the world `Sync`, and relaxed ordering suffices for a log-once flag.
    refused_warnings: Vec<AtomicBool>,
}

impl Teleporters {
    pub fn from_section(section: &TeleportersSection) -> Self {
        let regions = section
            .teleporters
            .iter()
            .map(|record| record.brushes.iter().map(convex_leaf).collect())
            .collect();
        Self {
            regions: TeleporterRegions::build(regions),
            records: section.teleporters.clone(),
            entities: vec![None; section.teleporters.len()],
            refused_warnings: (0..section.teleporters.len())
                .map(|_| AtomicBool::new(false))
                .collect(),
        }
    }

    pub fn records(&self) -> &[TeleporterRecord] {
        &self.records
    }
    pub fn entity(&self, index: usize) -> Option<EntityId> {
        self.entities.get(index).copied().flatten()
    }
    pub fn bind_entity(&mut self, index: usize, entity: EntityId) {
        if let Some(binding) = self.entities.get_mut(index) {
            *binding = Some(entity);
        }
    }
    /// Whether teleporter `index` is armed this tick. A teleporter with no bound
    /// entity, or one whose component is gone, reports its authored
    /// `enabled_on_spawn`: the state its installed component starts in, so a
    /// fixture or tool that installs no entities still sees the map as
    /// authored. An unknown index is disarmed.
    pub fn armed(&self, registry: &postretro_entities::EntityRegistry, index: usize) -> bool {
        self.entity(index)
            .and_then(|id| {
                registry
                    .get_component::<postretro_entities::TeleporterComponent>(id)
                    .ok()
            })
            .map_or_else(
                || {
                    self.records
                        .get(index)
                        .is_some_and(|record| record.enabled_on_spawn)
                },
                |record| record.armed,
            )
    }
    /// Warn once per teleporter that a blocked arrival refused its teleport.
    pub(crate) fn warn_refused(&self, index: usize) {
        if !self.refused_warnings[index].swap(true, Ordering::Relaxed) {
            log::warn!(
                "[Movement] teleporter entity {} arrival blocked; teleport refused",
                self.records[index].source_entity
            );
        }
    }
}
