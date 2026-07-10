use crate::map_data::MapTriggerVolume;
use postretro_level_format::trigger_volumes::{TriggerVolumeRecord, TriggerVolumesSection};

pub fn encode_trigger_volumes_section(
    triggers: &[MapTriggerVolume],
) -> Option<TriggerVolumesSection> {
    (!triggers.is_empty()).then(|| TriggerVolumesSection {
        triggers: triggers
            .iter()
            .map(|t| TriggerVolumeRecord {
                name: t.name.clone(),
                tags: t.tags.clone(),
                aabb_min: t.aabb_min,
                aabb_max: t.aabb_max,
                activation: t.activation,
                target_tag: t.target_tag.clone(),
                command: t.command,
                command_arg: t.command_arg.clone(),
                fire_mode: t.fire_mode,
                rearm_ms: t.rearm_ms,
                enabled_on_spawn: t.enabled_on_spawn,
            })
            .collect(),
    })
}
