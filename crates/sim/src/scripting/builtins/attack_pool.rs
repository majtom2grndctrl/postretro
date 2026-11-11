//! Built-in classname handler for `ai_attack_pool`: a placement-free logic entity naming its members' budget profile.
//! See: context/lib/build_pipeline.md §Custom FGD · context/lib/entity_model.md §7c (Attack pools)

use glam::Vec3;

use super::MapEntity;
use postretro_entities::components::attack_pool::AttackPoolComponent;
use postretro_entities::registry::{EntityId, EntityRegistry, Transform};

/// FGD `classname` this handler binds to.
pub const CLASSNAME: &str = "ai_attack_pool";

fn trimmed(entity: &MapEntity, key: &str) -> String {
    entity
        .key_values
        .get(key)
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

/// Spawn the pool, stopped, at its origin with its `_tags`. Where it sits does
/// not matter; triggers and scripts reach it by tag. The compiler rejects an
/// empty `member_tag`, and an empty `budget_profile` means `default`.
pub fn handle(entity: &MapEntity, registry: &mut EntityRegistry) -> Option<EntityId> {
    let transform = Transform {
        position: entity.origin,
        rotation: entity.rotation_quat(),
        scale: Vec3::ONE,
    };
    let id = registry.try_spawn(transform, &entity.tags).or_else(|| {
        log::warn!(
            "[Loader] {}: entity registry exhausted; dropping ai_attack_pool",
            entity.diagnostic_origin()
        );
        None
    })?;
    let component = AttackPoolComponent::new(
        trimmed(entity, "member_tag"),
        trimmed(entity, "budget_profile"),
    );
    let _ = registry.set_component(id, component);
    Some(id)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    // The two authored KVPs land on a stopped pool; `_tags` addresses it.
    #[test]
    fn ai_attack_pool_kvps_spawn_a_stopped_pool_with_its_tags() {
        let entity = MapEntity {
            map_ordinal: None,
            classname: CLASSNAME.to_string(),
            origin: Vec3::new(1.0, 2.0, 3.0),
            angles: Vec3::ZERO,
            key_values: [("member_tag", " arena_guard "), ("budget_profile", "arena")]
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect::<HashMap<_, _>>(),
            tags: vec!["arena".to_string()],
        };
        let mut registry = EntityRegistry::new();
        let id = handle(&entity, &mut registry).expect("the pool spawns");
        assert_eq!(
            registry.get_component::<AttackPoolComponent>(id).unwrap(),
            &AttackPoolComponent::new("arena_guard".into(), "arena".into())
        );
        assert_eq!(registry.get_tags(id).unwrap(), ["arena".to_string()]);
    }
}
