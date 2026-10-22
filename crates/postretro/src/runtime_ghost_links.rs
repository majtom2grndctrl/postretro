// Level-installed ghost-link entities: one per id-56 record, at its companion.
// See: context/lib/entity_model.md §7d · context/lib/build_pipeline.md §PRL section IDs

use glam::Vec3;
use postretro_entities::components::map_ordinal::MapOrdinal;
use postretro_entities::{EntityRegistry, GhostLinkComponent, Transform};
use postretro_level_format::ghost_links::{GHOST_LINK_CLASSNAME, GHOST_LINK_SOURCE_ENTITY_KEY};
use postretro_level_format::map_entity::MapEntityRecord;
use postretro_level_loader::LevelWorld;

/// Spawn one engine-owned entity per loaded link, armed and showing the
/// categories its record starts with. Nothing here replicates or evaluates
/// membership; this is the installed identity later stages bind to.
pub(crate) fn spawn_loaded_ghost_links(registry: &mut EntityRegistry, world: &LevelWorld) {
    for (index, record) in world.ghost_links.links.iter().enumerate() {
        // The decoder caps links at the u16 index space.
        let index = u16::try_from(index).expect("ghost link index fits u16");
        let companion = ghost_link_companion(world, record.source_entity);
        if companion.is_none() {
            // The compiler always writes one; a PRL without it still loads, but the
            // link has no tags for scripts to find and no authored origin.
            log::warn!(
                "[Loader] ghost link entity {} has no map-entity companion; it has no tags \
                 and sits at its first brush's centre",
                record.source_entity
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
                "[Loader] entity registry exhausted; dropping ghost link entity {}",
                record.source_entity
            );
            break;
        };
        let _ = registry.set_component(
            entity,
            GhostLinkComponent {
                index,
                source_entity: record.source_entity,
                armed: record.enabled_on_spawn,
                members: record.members.bits(),
            },
        );
        if let Some((ordinal, _)) = companion {
            let ordinal = u32::try_from(ordinal).expect("PRL map-entity ordinal fits u32");
            let _ = registry.set_component(entity, MapOrdinal(ordinal));
        }
    }
}

/// The id-29 record carrying a link's tags and authored keys, with its map
/// ordinal. The compiler writes one per link, keyed by source entity.
fn ghost_link_companion(
    world: &LevelWorld,
    source_entity: u32,
) -> Option<(usize, &MapEntityRecord)> {
    world.map_entities.iter().enumerate().find(|(_, entity)| {
        entity.classname == GHOST_LINK_CLASSNAME
            && entity.key_values.iter().any(|(key, value)| {
                key == GHOST_LINK_SOURCE_ENTITY_KEY
                    && value.parse::<u32>().ok() == Some(source_entity)
            })
    })
}

#[cfg(test)]
mod tests;
