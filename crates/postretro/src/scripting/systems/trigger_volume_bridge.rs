//! Level-load bridge for trigger-volume AABBs and authored event names.
//! See: context/lib/build_pipeline.md §Entity resolution

use glam::{Quat, Vec3};
use postretro_entities::{
    EntityId, EntityRegistry, MoverCommand, Transform, TriggerActivation, TriggerFireMode,
    TriggerVolumeComponent,
};
use postretro_level_format::trigger_volumes::TriggerVolumeRecord;
use std::collections::HashMap;

pub(crate) struct TriggerVolumeBridge {
    aabbs: HashMap<EntityId, (Vec3, Vec3)>,
    #[cfg(any(test, feature = "dev-tools"))]
    names: HashMap<EntityId, String>,
}

impl TriggerVolumeBridge {
    pub(crate) fn new() -> Self {
        Self {
            aabbs: HashMap::new(),
            #[cfg(any(test, feature = "dev-tools"))]
            names: HashMap::new(),
        }
    }
    pub(crate) fn clear(&mut self) {
        self.aabbs.clear();
        #[cfg(any(test, feature = "dev-tools"))]
        self.names.clear();
    }
    pub(crate) fn aabb(&self, id: EntityId) -> Option<(Vec3, Vec3)> {
        self.aabbs.get(&id).copied()
    }
    #[cfg(any(test, feature = "dev-tools"))]
    pub(crate) fn name(&self, id: EntityId) -> Option<&str> {
        self.names.get(&id).map(String::as_str)
    }
    pub(crate) fn populate_from_level(
        &mut self,
        registry: &mut EntityRegistry,
        records: &[TriggerVolumeRecord],
    ) {
        self.clear();
        for record in records {
            let min = Vec3::from(record.aabb_min);
            let max = Vec3::from(record.aabb_max);
            let Some(id) = registry.try_spawn(
                Transform {
                    position: (min + max) * 0.5,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                &record.tags,
            ) else {
                log::warn!(
                    "[TriggerVolumeBridge] entity registry exhausted; dropping trigger `{}`",
                    record.name
                );
                break;
            };
            let activation = match record.activation {
                0 => TriggerActivation::Touch,
                1 => TriggerActivation::Use,
                _ => unreachable!("format validates activation"),
            };
            let command = match record.command {
                0 => MoverCommand::Start,
                1 => MoverCommand::Stop,
                2 => MoverCommand::Reverse,
                3 => MoverCommand::GoToPathNode(record.command_arg.clone()),
                _ => unreachable!("format validates command"),
            };
            let fire_mode = match record.fire_mode {
                0 => TriggerFireMode::Once,
                1 => TriggerFireMode::Multiple,
                _ => unreachable!("format validates fire mode"),
            };
            let _ = registry.set_component(
                id,
                TriggerVolumeComponent::new(
                    activation,
                    record.target_tag.clone(),
                    record.on_fire.clone(),
                    record.on_exit.clone(),
                    command,
                    fire_mode,
                    record.rearm_ms,
                    record.enabled_on_spawn,
                ),
            );
            self.aabbs.insert(id, (min, max));
            #[cfg(any(test, feature = "dev-tools"))]
            self.names.insert(id, record.name.clone());
        }
    }
    #[cfg(test)]
    pub(crate) fn count(&self) -> usize {
        self.aabbs.len()
    }

    #[cfg(test)]
    pub(crate) fn insert_for_test(&mut self, id: EntityId, min: Vec3, max: Vec3) {
        self.aabbs.insert(id, (min, max));
    }
}

impl Default for TriggerVolumeBridge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_records_preserve_trigger_event_names_on_components() {
        let record = TriggerVolumeRecord {
            name: "lift_plate".into(),
            tags: vec!["lift".into()],
            aabb_min: [-1.0, 0.0, -1.0],
            aabb_max: [1.0, 2.0, 1.0],
            activation: 0,
            target_tag: "lift".into(),
            command: 0,
            command_arg: String::new(),
            fire_mode: 0,
            rearm_ms: 0.0,
            enabled_on_spawn: true,
            on_fire: "open_lift".into(),
            on_exit: "close_lift".into(),
        };
        let mut registry = EntityRegistry::new();
        let mut bridge = TriggerVolumeBridge::new();

        bridge.populate_from_level(&mut registry, &[record]);

        assert_eq!(bridge.count(), 1);
        let id = *bridge.aabbs.keys().next().expect("trigger entity spawned");
        let component = registry
            .get_component::<TriggerVolumeComponent>(id)
            .expect("trigger component attached");
        assert_eq!(component.on_fire, "open_lift");
        assert_eq!(component.on_exit, "close_lift");
        assert_eq!(bridge.name(id), Some("lift_plate"));
    }
}
