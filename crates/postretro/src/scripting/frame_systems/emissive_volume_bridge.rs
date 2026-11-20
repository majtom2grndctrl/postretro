// Scripting ↔ renderer bridge for emissive volumes: entity registry → the
// forward shader's volume table, dirty-gated.
// See: context/lib/rendering_pipeline.md §7.3

use glam::{Quat, Vec3};

use postretro_entities::EmissiveVolumeComponent;
use postretro_entities::registry::{EntityId, EntityRegistry, Transform};
use postretro_level_format::emissive::EmissiveVolumeRecord;
use postretro_render_cpu::emissive_volume::{
    EmissiveTableUpload, EmissiveVolumeTable, volume_factor,
};

/// State carried across frames. Owned by the game layer so the renderer never
/// holds component data.
pub(crate) struct EmissiveVolumeBridge {
    /// Id-60 record index → `EntityId`, fixed at level load. Co-op binds a
    /// volume by this index.
    entity_ids: Vec<EntityId>,
    /// This frame's glow factor per volume, in record order. Sized at load;
    /// rewritten in place each frame.
    factors: Vec<[f32; 3]>,
    /// The CPU volume table and what the GPU copy is missing.
    table: EmissiveVolumeTable,
}

impl EmissiveVolumeBridge {
    pub(crate) fn new() -> Self {
        Self {
            entity_ids: Vec::new(),
            factors: Vec::new(),
            table: EmissiveVolumeTable::new(),
        }
    }

    /// The entity spawned for id-60 record `map_index`. Test-only until the
    /// co-op presentation binding reads it.
    #[cfg(test)]
    pub(crate) fn entity_for_map_index(&self, map_index: usize) -> Option<EntityId> {
        self.entity_ids.get(map_index).copied()
    }

    /// Spawn one entity per emissive volume record, in record order, at its
    /// no-script state: brightness 1, authored colour. An emptied volume
    /// (no member face) still gets its entity, so record indices stay
    /// aligned. Its transform sits at the record's origin, the co-op binding
    /// check; its tags are the record's.
    pub(crate) fn populate_from_level(
        &mut self,
        registry: &mut EntityRegistry,
        records: &[EmissiveVolumeRecord],
    ) {
        self.clear();
        self.entity_ids.reserve(records.len());
        for record in records {
            let transform = Transform {
                position: Vec3::from(record.origin),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            };
            let Some(id) = registry.try_spawn(transform, &record.tags) else {
                log::warn!(
                    "[EmissiveVolumeBridge] entity registry exhausted; dropping emissive volume \
                     (index {}). Later volumes in this level present their authored glow.",
                    self.entity_ids.len()
                );
                break;
            };
            // `set_component` only fails on a stale id — the id was just returned.
            let _ = registry.set_component(
                id,
                EmissiveVolumeComponent::authored(record.min, record.max, record.color),
            );
            self.entity_ids.push(id);
        }
        self.factors.resize(self.entity_ids.len(), [0.0; 3]);
    }

    /// Drop per-level state. The table pends every entry, so the next level's
    /// first upload replaces anything this one left on the GPU.
    pub(crate) fn clear(&mut self) {
        self.entity_ids.clear();
        self.factors.clear();
        self.table.reset();
    }

    /// Pack this frame's glow factor for every volume from its component.
    /// Runs after the script drain and before render, so a brightness or
    /// colour written this frame presents this frame. Allocation-free.
    pub(crate) fn update(&mut self, registry: &EntityRegistry) {
        for (factor, &id) in self.factors.iter_mut().zip(&self.entity_ids) {
            // A volume entity outlives no level; a missing component keeps
            // its last factor rather than inventing one.
            if let Ok(volume) = registry.get_component::<EmissiveVolumeComponent>(id) {
                *factor = volume_factor(volume.min, volume.max, volume.brightness, volume.color);
            }
        }
        // Seam for the photosensitivity flash limiter: it substitutes each
        // volume's presented factor in `self.factors` here, after the
        // scripted values and before the pack. Scripts and the wire keep the
        // component's values.
        self.table.pack(&self.factors);
    }

    /// The writes the GPU table is missing; empty runs on a frame where no
    /// volume changed.
    pub(crate) fn pending_upload(&mut self) -> EmissiveTableUpload<'_> {
        self.table.pending()
    }

    /// The renderer accepted every pending write.
    pub(crate) fn mark_uploaded(&mut self) {
        self.table.mark_uploaded();
    }
}

#[cfg(test)]
#[path = "emissive_volume_bridge_tests.rs"]
mod tests;
