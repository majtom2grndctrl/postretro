// Engine-owned map teleporter identity, arming state and presentation metadata.
// See: context/lib/entity_model.md §7 (Environment Volumes)

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TeleporterComponent {
    pub index: u16,
    pub source_entity: u32,
    pub armed: bool,
    pub arrival_sound: String,
    pub on_teleport: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Component, ComponentKind, ComponentValue, EntityRegistry, Transform};

    #[test]
    fn teleporter_registry_record_preserves_identity_and_mutable_arming() {
        let mut registry = EntityRegistry::new();
        let entity = registry.spawn(Transform::default());
        let teleporter = TeleporterComponent {
            index: 7,
            source_entity: 42,
            armed: true,
            arrival_sound: "sfx/teleport".into(),
            on_teleport: "gate.used".into(),
        };
        registry.set_component(entity, teleporter.clone()).unwrap();
        let value = registry
            .get_component_value_mut(entity, ComponentKind::Teleporter)
            .unwrap();
        let ComponentValue::Teleporter(state) = value else {
            panic!("teleporter column must hold teleporter state");
        };
        state.armed = false;

        let stored = registry
            .get_component::<TeleporterComponent>(entity)
            .unwrap();
        assert_eq!(stored.index, teleporter.index);
        assert_eq!(stored.source_entity, teleporter.source_entity);
        assert!(!stored.armed);
        let serialized = serde_json::to_value(stored.clone().into_value()).unwrap();
        assert_eq!(serialized["kind"], "teleporter");
        let decoded: ComponentValue = serde_json::from_value(serialized).unwrap();
        assert_eq!(decoded.kind(), ComponentKind::Teleporter);
        assert_eq!(TeleporterComponent::from_value(&decoded), Some(stored));
        assert_eq!(ComponentKind::Teleporter as u16, 24);
    }
}
