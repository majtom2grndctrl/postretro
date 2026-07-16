//! Built-in classname handler for `entity_spawner` map entities.
//!
//! Descriptor validation happens after all classname dispatch, once the active
//! level descriptor table is available. This handler only preserves authored
//! configuration in a serde-friendly ECS component.

use glam::Vec3;

use super::MapEntity;
use postretro_entities::components::spawner::SpawnerComponent;
use postretro_entities::registry::{EntityId, EntityRegistry, Transform};

/// FGD `classname` this handler binds to.
pub(crate) const CLASSNAME: &str = "entity_spawner";

fn archetype_from_entity(entity: &MapEntity) -> String {
    match entity.key_values.get("archetype") {
        Some(value) if !value.is_empty() => value.clone(),
        Some(_) => {
            log::warn!(
                "[Loader] {}: key `archetype` is empty; entity_spawner will spawn nothing",
                entity.diagnostic_origin()
            );
            String::new()
        }
        None => {
            log::warn!(
                "[Loader] {}: required key `archetype` is absent; entity_spawner will spawn nothing",
                entity.diagnostic_origin()
            );
            String::new()
        }
    }
}

fn count_from_entity(entity: &MapEntity) -> u32 {
    let raw = entity.key_values.get("count").map(String::as_str);
    match raw.and_then(|value| value.parse::<u32>().ok()) {
        Some(count) if count > 0 => count,
        _ => {
            let reason = match raw {
                None => "is absent",
                Some("") => "is empty",
                Some("0") => "is zero",
                Some(_) => "is malformed or zero",
            };
            log::warn!(
                "[Loader] {}: key `count` {reason}; entity_spawner will spawn nothing",
                entity.diagnostic_origin()
            );
            0
        }
    }
}

/// Spawn an inert, unresolved spawner configuration. Its tags are attached at
/// `try_spawn`, and the common dispatch layer writes the raw KVP table.
pub(crate) fn handle(entity: &MapEntity, registry: &mut EntityRegistry) -> Option<EntityId> {
    let transform = Transform {
        position: entity.origin,
        rotation: entity.rotation_quat(),
        scale: Vec3::ONE,
    };
    let id = registry.try_spawn(transform, &entity.tags).or_else(|| {
        log::warn!(
            "[Loader] {}: entity registry exhausted; dropping entity_spawner",
            entity.diagnostic_origin()
        );
        None
    })?;
    let _ = registry.set_component(
        id,
        SpawnerComponent {
            archetype_name: archetype_from_entity(entity),
            count: count_from_entity(entity),
            resolved: false,
        },
    );
    Some(id)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn entity_with_kvps(pairs: &[(&str, &str)]) -> MapEntity {
        let key_values = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<HashMap<_, _>>();
        MapEntity {
            classname: CLASSNAME.to_string(),
            origin: Vec3::new(1.0, 2.0, 3.0),
            angles: Vec3::ZERO,
            key_values,
            tags: vec!["closet".to_string()],
        }
    }

    #[test]
    fn handler_preserves_configuration_and_tags() {
        let mut registry = EntityRegistry::new();
        let id = handle(
            &entity_with_kvps(&[("archetype", "cultist"), ("count", "3")]),
            &mut registry,
        )
        .expect("spawner should fit");

        assert_eq!(
            registry.get_component::<SpawnerComponent>(id).unwrap(),
            &SpawnerComponent {
                archetype_name: "cultist".into(),
                count: 3,
                resolved: false,
            }
        );
        assert_eq!(registry.get_tags(id).unwrap(), &["closet"]);
    }

    #[test]
    fn missing_or_invalid_count_is_inert() {
        for kvps in [vec![], vec![("count", "0")], vec![("count", "bad")]] {
            let mut registry = EntityRegistry::new();
            let id = handle(&entity_with_kvps(&kvps), &mut registry).expect("spawner should fit");
            assert_eq!(
                registry
                    .get_component::<SpawnerComponent>(id)
                    .unwrap()
                    .count,
                0
            );
        }
    }
}
