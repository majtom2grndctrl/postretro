// Installed teleporter geometry, authored poses and stable local entity bindings.
// See: context/lib/movement.md §6 · context/lib/entity_model.md §7

use glam::Vec3;
use postretro_collision::{ConvexLeaf, LeafPlane, TeleporterRegions};
use postretro_entities::EntityId;
use postretro_level_format::teleporters::{TeleporterRecord, TeleportersSection};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Default)]
pub struct Teleporters {
    pub regions: TeleporterRegions,
    records: Vec<TeleporterRecord>,
    entities: Vec<Option<EntityId>>,
    refused_warnings: Vec<AtomicBool>,
}

impl Teleporters {
    pub fn from_section(section: &TeleportersSection) -> Self {
        let regions = section
            .teleporters
            .iter()
            .map(|record| {
                record
                    .brushes
                    .iter()
                    .map(|brush| ConvexLeaf {
                        aabb_min: Vec3::from(brush.aabb_min),
                        aabb_max: Vec3::from(brush.aabb_max),
                        planes: brush
                            .planes
                            .iter()
                            .map(|plane| LeafPlane {
                                normal: Vec3::from(plane.normal),
                                distance: plane.distance,
                            })
                            .collect(),
                    })
                    .collect()
            })
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
    pub(crate) fn warn_refused(&self, index: usize) {
        if !self.refused_warnings[index].swap(true, Ordering::Relaxed) {
            log::warn!(
                "[Movement] teleporter entity {} arrival blocked; teleport refused",
                self.records[index].source_entity
            );
        }
    }
}
