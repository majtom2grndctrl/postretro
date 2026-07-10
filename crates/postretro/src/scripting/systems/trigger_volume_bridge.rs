//! Level-load bridge for invisible trigger-volume AABBs. Tick evaluation lives in E17-C Task 3.

use glam::{Quat, Vec3};
use postretro_entities::{
    EntityId, EntityRegistry, MoverCommand, Transform, TriggerActivation, TriggerFireMode,
    TriggerVolumeComponent,
};
use postretro_level_format::trigger_volumes::TriggerVolumeRecord;
use std::collections::HashMap;

pub(crate) struct TriggerVolumeBridge {
    aabbs: HashMap<EntityId, (Vec3, Vec3)>,
}

impl TriggerVolumeBridge {
    pub(crate) fn new() -> Self {
        Self {
            aabbs: HashMap::new(),
        }
    }
    pub(crate) fn clear(&mut self) {
        self.aabbs.clear();
    }
    pub(crate) fn aabb(&self, id: EntityId) -> Option<(Vec3, Vec3)> {
        self.aabbs.get(&id).copied()
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
                    command,
                    fire_mode,
                    record.rearm_ms,
                    record.enabled_on_spawn,
                ),
            );
            self.aabbs.insert(id, (min, max));
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
