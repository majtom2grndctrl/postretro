// Level-installed seam entities and the host-side seam stage they bind to.
// See: context/lib/entity_model.md §7e

use glam::Vec3;
use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry, SeamComponent, Transform};
use postretro_level_format::seams::{SEAM_CLASSNAME, SEAM_SOURCE_ENTITY_KEY};
use postretro_level_loader::LevelWorld;
use postretro_sim::seam_stage::SeamStage;

/// One entity per seam, at its companion's origin with its tags, so scripts
/// find it by `getMapEntities("seam")` and arm or disarm it.
pub(crate) fn spawn_loaded_seams(registry: &mut EntityRegistry, world: &LevelWorld) {
    for (index, record) in world.seams.seams.iter().enumerate() {
        let companion = seam_companion(world, record.source_entity);
        if companion.is_none() {
            log::warn!(
                "[Loader] seam `{}` has no map-entity companion; it has no tags and sits at its \
                 first brush's centre",
                record.name
            );
        }
        let position = companion.map_or_else(
            || {
                record.brushes.first().map_or(Vec3::ZERO, |brush| {
                    (Vec3::from(brush.aabb_min) + Vec3::from(brush.aabb_max)) * 0.5
                })
            },
            |(_, entity)| Vec3::from(entity.origin),
        );
        let tags = companion.map_or(&[][..], |(_, entity)| entity.tags.as_slice());
        let Some(entity) = registry.try_spawn(
            Transform {
                position,
                ..Default::default()
            },
            tags,
        ) else {
            log::warn!(
                "[Loader] entity registry exhausted; dropping seam `{}`",
                record.name
            );
            break;
        };
        let _ = registry.set_component(
            entity,
            SeamComponent {
                index: index as u16,
                source_entity: record.source_entity,
                name: record.name.clone(),
                armed: record.enabled_on_spawn,
                on_arrive: record.on_arrive.clone(),
            },
        );
        if let Some((ordinal, _)) = companion {
            let _ = registry.set_component(
                entity,
                postretro_entities::components::map_ordinal::MapOrdinal(ordinal as u32),
            );
        }
    }
}

fn seam_companion(
    world: &LevelWorld,
    source_entity: u32,
) -> Option<(usize, &postretro_level_format::map_entity::MapEntityRecord)> {
    world.map_entities.iter().enumerate().find(|(_, entity)| {
        entity.classname == SEAM_CLASSNAME
            && entity.key_values.iter().any(|(key, value)| {
                key == SEAM_SOURCE_ENTITY_KEY && value.parse::<u32>().ok() == Some(source_entity)
            })
    })
}

/// The installed level's seam stage, bound to its seam entities. `inert`
/// marks a frontend backdrop, whose seams never activate.
pub(crate) fn install_seam_stage(
    registry: &EntityRegistry,
    world: &LevelWorld,
    inert: bool,
) -> SeamStage {
    let mut stage = SeamStage::install(&world.seams, inert);
    for (id, value) in registry.iter_with_kind(ComponentKind::Seam) {
        if let ComponentValue::Seam(seam) = value {
            stage.bind_entity(seam.index, id);
        }
    }
    stage
}
