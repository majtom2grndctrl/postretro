// Scripting ↔ renderer bridge for emissive volumes: entity registry → the
// forward shader's volume table, dirty-gated.
// See: context/lib/rendering_pipeline.md §7.3

use glam::{Quat, Vec3};

use postretro_entities::registry::{EntityId, EntityRegistry, Transform};
use postretro_entities::{ComponentKind, ComponentValue, EmissiveVolumeComponent};
use postretro_level_format::emissive::EmissiveVolumeRecord;
use postretro_render_cpu::emissive_flash::{EmissiveFlashClamp, EmissiveFlashSource};
use postretro_render_cpu::emissive_volume::{
    EmissiveTableUpload, EmissiveVolumeTable, VolumeGlow, volume_factor,
};
use postretro_render_cpu::flash_limiter::LimiterFrameUniform;

#[path = "emissive_volume_anim.rs"]
mod anim;

/// State carried across frames. Owned by the game layer so the renderer never
/// holds component data.
pub(crate) struct EmissiveVolumeBridge {
    /// Id-60 record index → `EntityId`, fixed at level load. Co-op binds a
    /// volume by this index.
    entity_ids: Vec<EntityId>,
    /// Each record's authored origin (its brushes' union AABB centre), in
    /// record order: the co-op binding's 1 cm check. Kept here rather than read
    /// off the entity transform, so nothing that moves the entity moves it.
    origins: Vec<Vec3>,
    /// Each volume's scripted glow, in record order: the component's values,
    /// sampled this frame. Sized at load; rewritten in place each frame.
    scripted: Vec<VolumeGlow>,
    /// This frame's presented glow factor per volume, in record order: the
    /// scripted glow through the flash limiter. Sized at load.
    factors: Vec<[f32; 3]>,
    /// The photosensitivity limiter over every volume. Presentation only:
    /// components, presentation capture and the wire keep scripted values.
    flash: EmissiveFlashClamp,
    /// The CPU volume table and what the GPU copy is missing.
    table: EmissiveVolumeTable,
    /// Brightness animation start times, in record order.
    clock: anim::EmissiveAnimClock,
}

impl EmissiveVolumeBridge {
    pub(crate) fn new() -> Self {
        Self {
            entity_ids: Vec::new(),
            origins: Vec::new(),
            scripted: Vec::new(),
            factors: Vec::new(),
            flash: EmissiveFlashClamp::default(),
            table: EmissiveVolumeTable::new(),
            clock: anim::EmissiveAnimClock::new(),
        }
    }

    /// The entity spawned for id-60 record `map_index`: the co-op binding's
    /// identity.
    pub(crate) fn entity_for_map_index(&self, map_index: usize) -> Option<EntityId> {
        self.entity_ids.get(map_index).copied()
    }

    /// The id-60 record index `entity` was spawned for.
    pub(crate) fn map_index_for_entity(&self, entity: EntityId) -> Option<usize> {
        self.entity_ids.iter().position(|id| *id == entity)
    }

    /// The authored origin of the record `entity` was spawned for.
    pub(crate) fn map_origin_for_entity(&self, entity: EntityId) -> Option<Vec3> {
        self.origins
            .get(self.map_index_for_entity(entity)?)
            .copied()
    }

    /// Spawn one entity per emissive volume record, in record order, at its
    /// no-script state: brightness 1, authored colour. An emptied volume
    /// (no member face) still gets its entity, so record indices stay
    /// aligned. Its transform sits at the record's origin, the co-op binding
    /// check; its tags are the record's.
    ///
    /// `flash_sources` is what the flash limiter judges each record by
    /// ([`EmissiveFlashSource::for_volumes`]), in record order. Every clamp
    /// starts fresh.
    ///
    /// If the entity registry fills, the record that found it full and every
    /// later one get no entity: no script reaches them and the limiter skips
    /// them. Their table entries pack once at the authored no-script glow
    /// (brightness 1, authored colour), the state the bake shaded them in,
    /// and hold it for the level.
    pub(crate) fn populate_from_level(
        &mut self,
        registry: &mut EntityRegistry,
        records: &[EmissiveVolumeRecord],
        flash_sources: &[EmissiveFlashSource],
    ) {
        debug_assert_eq!(records.len(), flash_sources.len());
        self.clear();
        self.entity_ids.reserve(records.len());
        self.origins.reserve(records.len());
        for record in records {
            let transform = Transform {
                position: Vec3::from(record.origin),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            };
            let Some(id) = registry.try_spawn(transform, &record.tags) else {
                let index = self.entity_ids.len();
                log::warn!(
                    "[EmissiveVolumeBridge] entity registry exhausted; dropping emissive volumes \
                     {index}..{} ({} of {}). They hold their authored glow; scripts cannot \
                     reach them.",
                    records.len(),
                    records.len() - index,
                    records.len()
                );
                break;
            };
            // `set_component` only fails on a stale id — the id was just returned.
            let _ = registry.set_component(
                id,
                EmissiveVolumeComponent::authored(record.min, record.max, record.color),
            );
            self.entity_ids.push(id);
            self.origins.push(transform.position);
            self.scripted.push(VolumeGlow::scripted(
                record.min,
                record.max,
                1.0,
                record.color,
            ));
        }
        let count = self.entity_ids.len();
        self.factors.resize(count, [0.0; 3]);
        // Volumes the registry had no room for: `clear` left their entries at
        // the no-volume identity, and the frame pack only reaches the first
        // `count`, so pack each one's authored glow here, once.
        for (index, record) in records.iter().enumerate().skip(count) {
            self.table.pack_volume(
                index,
                volume_factor(record.min, record.max, 1.0, record.color),
            );
        }
        // Only spawned volumes are limited; `count` stops short of
        // `records.len()` (one source per record) when the registry filled.
        self.flash.install(&flash_sources[..count]);
        self.clock.reset(count);
    }

    /// Start every animation the level install left running (its `levelLoad`
    /// installs) at script time 0, so the first running frame samples each
    /// from its first sample, as the fog and light bridges start theirs.
    pub(crate) fn start_installed_animations(&mut self, registry: &EntityRegistry) {
        for (index, &id) in self.entity_ids.iter().enumerate() {
            if let Ok(volume) = registry.get_component::<EmissiveVolumeComponent>(id) {
                self.clock.seed(index, volume, 0.0);
            }
        }
    }

    /// Seed `id`'s animation run from a replicated record, after co-op apply
    /// has written the component and before this frame's [`Self::update`], so
    /// the host's start survives the same-frame tick.
    ///
    /// `start` is the run's start on this machine's script clock (seconds),
    /// rebased from the host's. With no host start, a running slot keeps its
    /// local start and adopts the component's run; a new run starts lazily at
    /// the next update. A record with no animation drops the slot. Returns
    /// false for a non-finite start or an entity this bridge does not own.
    pub(crate) fn seed_replicated_state(
        &mut self,
        registry: &EntityRegistry,
        id: EntityId,
        start: Option<f64>,
    ) -> bool {
        if start.is_some_and(|value| !value.is_finite() || !(value * 1000.0).is_finite()) {
            return false;
        }
        let Some(index) = self.map_index_for_entity(id) else {
            return false;
        };
        let Ok(volume) = registry.get_component::<EmissiveVolumeComponent>(id) else {
            return false;
        };
        match start {
            Some(start) => self.clock.seed(index, volume, start * 1000.0),
            None => self.clock.adopt(index, volume),
        }
        true
    }

    /// Drop per-level state. The table pends every entry, so the next level's
    /// first upload replaces anything this one left on the GPU; the flash
    /// limiter keeps no history across levels.
    pub(crate) fn clear(&mut self) {
        self.entity_ids.clear();
        self.origins.clear();
        self.scripted.clear();
        self.factors.clear();
        self.flash.install(&[]);
        self.table.reset();
        self.clock.reset(0);
    }

    /// Sample every running brightness animation at `script_time_seconds`
    /// (the freeze-aware script clock) into its component, limit every
    /// volume's glow through the photosensitivity limiter, then pack this
    /// frame's presented factors. Runs after the script drain and before
    /// render, so a brightness or colour written this frame presents this
    /// frame. Allocation-free unless a new animation run starts.
    ///
    /// `limiter` is this frame's limiter input (presented-frame time and the
    /// fail-safe enable flag), or `None` for a frame outside the limiter,
    /// which packs scripted glow and leaves its history untouched. The
    /// components keep the scripted values: only the packed table sees the
    /// limit. Settle the frame with [`Self::finish_presentation`].
    pub(crate) fn update(
        &mut self,
        registry: &mut EntityRegistry,
        script_time_seconds: f64,
        limiter: Option<&LimiterFrameUniform>,
    ) {
        let now_ms = script_time_seconds * 1000.0;
        for (index, (scripted, &id)) in self.scripted.iter_mut().zip(&self.entity_ids).enumerate() {
            // A volume entity outlives no level; a missing component keeps
            // its last scripted glow rather than inventing one.
            let Ok(ComponentValue::EmissiveVolume(volume)) =
                registry.get_component_value_mut(id, ComponentKind::EmissiveVolume)
            else {
                continue;
            };
            self.clock.advance(index, volume, now_ms);
            *scripted =
                VolumeGlow::scripted(volume.min, volume.max, volume.brightness, volume.color);
        }
        // Every volume, in view or not: the limiter judges the source.
        self.flash
            .present(limiter, &self.scripted, &mut self.factors);
        self.table.pack(&self.factors);
    }

    /// Settle this frame's flash limiter once the renderer knows whether the
    /// frame presented: one that presented nothing leaves every clamp where
    /// the frame began and carries its time into the next frame.
    pub(crate) fn finish_presentation(&mut self, presented: bool) {
        self.flash.finish_presentation(presented);
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
