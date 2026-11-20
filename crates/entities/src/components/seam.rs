// Engine-owned level-seam identity and host-only arming state.
// See: context/lib/entity_model.md §7e (Level Seams)

use serde::{Deserialize, Serialize};

/// A map-authored `level_seam`. `index` is the seam's record index in the
/// level's `Seams` section; `source_entity` is its id-29 companion index, which
/// orders seams in map order. Host-only: never replicated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeamComponent {
    pub index: u16,
    pub source_entity: u32,
    pub name: String,
    pub armed: bool,
    pub on_arrive: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Component, ComponentKind, ComponentValue, EntityRegistry, Transform};

    #[test]
    fn seam_registry_record_preserves_identity_and_mutable_arming() {
        let mut registry = EntityRegistry::new();
        let entity = registry.spawn(Transform::default());
        let seam = SeamComponent {
            index: 3,
            source_entity: 17,
            name: "airlock".into(),
            armed: true,
            on_arrive: "airlock.sealed".into(),
        };
        registry.set_component(entity, seam.clone()).unwrap();
        let value = registry
            .get_component_value_mut(entity, ComponentKind::Seam)
            .unwrap();
        let ComponentValue::Seam(state) = value else {
            panic!("seam column must hold seam state");
        };
        state.armed = false;

        let stored = registry.get_component::<SeamComponent>(entity).unwrap();
        assert_eq!(stored.index, seam.index);
        assert_eq!(stored.source_entity, seam.source_entity);
        assert_eq!(stored.name, seam.name);
        assert_eq!(stored.on_arrive, seam.on_arrive);
        assert!(!stored.armed);
        let serialized = serde_json::to_value(stored.clone().into_value()).unwrap();
        assert_eq!(serialized["kind"], "seam");
        let decoded: ComponentValue = serde_json::from_value(serialized).unwrap();
        assert_eq!(decoded.kind(), ComponentKind::Seam);
        assert_eq!(SeamComponent::from_value(&decoded), Some(stored));
        assert_eq!(ComponentKind::Seam as u16, 29);
    }
}
