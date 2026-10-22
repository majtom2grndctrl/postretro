// Engine-owned map ghost-link identity, arming state, shown member categories and script colour filter.
// See: context/lib/entity_model.md §7d (Ghost Links)

use postretro_foundation::GhostColorFilter;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GhostLinkComponent {
    /// Map-order index into the level's GhostLinks (id 56) records.
    pub index: u16,
    /// Raw `.map` entity index, as the PRL record keys it.
    pub source_entity: u32,
    pub armed: bool,
    /// Member categories the link shows, in the id-56 bit order
    /// (`postretro_level_format::ghost_links::GhostMembers`). `entities`
    /// depends only on `foundation`, so the column stores the validated bits.
    pub members: u8,
    /// The look fields `setColorFilter` has set, each with its script-time
    /// ramp. Host state: written at the host's tick start, replicated as set.
    #[serde(default)]
    pub filter: GhostColorFilter,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Component, ComponentKind, ComponentValue, EntityRegistry, Transform};

    #[test]
    fn ghost_link_registry_record_preserves_identity_and_mutable_state() {
        let mut registry = EntityRegistry::new();
        let entity = registry.spawn(Transform::default());
        let link = GhostLinkComponent {
            index: 3,
            source_entity: 42,
            armed: true,
            members: 0b1111,
            filter: GhostColorFilter::default(),
        };
        registry.set_component(entity, link).unwrap();
        let value = registry
            .get_component_value_mut(entity, ComponentKind::GhostLink)
            .unwrap();
        let ComponentValue::GhostLink(state) = value else {
            panic!("ghost-link column must hold ghost-link state");
        };
        state.armed = false;
        state.members = 0b0001;

        let stored = *registry
            .get_component::<GhostLinkComponent>(entity)
            .unwrap();
        assert_eq!((stored.index, stored.source_entity), (3, 42));
        assert!(!stored.armed);
        assert_eq!(stored.members, 0b0001);
        let serialized = serde_json::to_value(stored.into_value()).unwrap();
        assert_eq!(serialized["kind"], "ghost_link");
        let decoded: ComponentValue = serde_json::from_value(serialized).unwrap();
        assert_eq!(decoded.kind(), ComponentKind::GhostLink);
        assert_eq!(GhostLinkComponent::from_value(&decoded), Some(&stored));
    }
}
