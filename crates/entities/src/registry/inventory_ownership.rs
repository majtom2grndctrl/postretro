//! Inventory remains canonical; this reverse index serves world queries without
//! scanning every holder. Only checked component replacement changes membership.

use super::{EntityId, EntityRegistry, Inventory, RegistryError};

impl EntityRegistry {
    /// The live holder and slot of an inventory item, including holstered slots.
    /// Full generation-bearing IDs prevent a recycled slot inheriting ownership.
    #[must_use]
    pub fn inventory_owner(&self, item: EntityId) -> Option<(EntityId, usize)> {
        let owner = self.inventory_owners.get(&item).copied()?;
        (self.exists(item) && self.exists(owner.0)).then_some(owner)
    }

    #[must_use]
    pub fn is_inventory_owned(&self, item: EntityId) -> bool {
        self.inventory_owner(item).is_some()
    }

    /// Validate the whole replacement before touching either representation.
    /// Stale forward IDs remain for the existing inventory normalization pass.
    pub(super) fn replace_inventory_ownership(
        &mut self,
        holder: EntityId,
        inventory: &Inventory,
    ) -> Result<(), RegistryError> {
        for (slot, item) in inventory.wieldables.iter().enumerate() {
            let Some(item) = item.filter(|item| self.exists(*item)) else {
                continue;
            };
            if inventory.wieldables[..slot].contains(&Some(item)) {
                return Err(RegistryError::InventoryOwnershipConflict { item, holder });
            }
            if let Some((owner, _)) = self.inventory_owner(item)
                && owner != holder
            {
                return Err(RegistryError::InventoryOwnershipConflict {
                    item,
                    holder: owner,
                });
            }
        }

        self.clear_inventory_ownership(holder);
        for (slot, item) in inventory.wieldables.iter().enumerate() {
            if let Some(item) = item.filter(|item| self.exists(*item)) {
                self.inventory_owners.insert(item, (holder, slot));
            }
        }
        Ok(())
    }

    pub(super) fn clear_inventory_ownership(&mut self, holder: EntityId) {
        let items = self
            .get_component::<Inventory>(holder)
            .map(|inventory| inventory.wieldables)
            .unwrap_or_default();
        for item in items.into_iter().flatten() {
            if self
                .inventory_owners
                .get(&item)
                .is_some_and(|(owner, _)| *owner == holder)
            {
                self.inventory_owners.remove(&item);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{ComponentKind, ComponentValue, Transform};

    fn inventory(item: EntityId, slot: usize) -> Inventory {
        let mut inventory = Inventory::default();
        inventory.wieldables[slot] = Some(item);
        inventory
    }

    #[test]
    fn inventory_ownership_replacements_are_atomic_and_include_holstered_slots() {
        let mut registry = EntityRegistry::new();
        let a = registry.spawn(Transform::default());
        let b = registry.spawn(Transform::default());
        let item = registry.spawn(Transform::default());
        registry.set_component(a, inventory(item, 7)).unwrap();
        registry.set_component(b, Inventory::default()).unwrap();
        assert_eq!(registry.inventory_owner(item), Some((a, 7)));

        let before = registry.get_component::<Inventory>(a).unwrap().clone();
        assert!(matches!(
            registry.set_component_value(b, ComponentValue::Inventory(inventory(item, 1))),
            Err(RegistryError::InventoryOwnershipConflict { .. })
        ));
        assert_eq!(registry.get_component::<Inventory>(a).unwrap(), &before);
        assert_eq!(
            registry.get_component::<Inventory>(b).unwrap(),
            &Inventory::default()
        );
        assert_eq!(registry.inventory_owner(item), Some((a, 7)));

        let mut duplicate = before.clone();
        duplicate.wieldables[2] = Some(item);
        assert!(registry.set_component(a, duplicate).is_err());
        assert_eq!(registry.get_component::<Inventory>(a).unwrap(), &before);
        registry.set_component(a, inventory(item, 2)).unwrap();
        assert_eq!(registry.inventory_owner(item), Some((a, 2)));

        // Replacing/removing inventory releases membership; only holder destruction
        // retires its current auxiliaries, so drop can reuse the same live item.
        registry
            .remove_component_kind(a, ComponentKind::Inventory)
            .unwrap();
        assert!(!registry.is_inventory_owned(item));
        assert!(registry.exists(item));
        registry.set_component(b, inventory(item, 0)).unwrap();
        registry.remove_component::<Inventory>(b).unwrap();
        assert!(!registry.is_inventory_owned(item));
        assert!(registry.exists(item));
    }

    #[test]
    fn inventory_ownership_despawn_follows_current_membership_and_generation() {
        let mut registry = EntityRegistry::new();
        let a = registry.spawn(Transform::default());
        let b = registry.spawn(Transform::default());
        let former = registry.spawn(Transform::default());
        let later_pickup = registry.spawn(Transform::default());
        registry.set_component(a, inventory(former, 0)).unwrap();
        registry
            .set_component(a, inventory(later_pickup, 4))
            .unwrap();
        registry.set_component(b, inventory(former, 1)).unwrap();
        registry.despawn(a).unwrap();
        assert!(!registry.exists(later_pickup));
        assert!(registry.exists(former));
        assert_eq!(registry.inventory_owner(former), Some((b, 1)));
        assert!(!registry.is_inventory_owned(later_pickup));

        registry.despawn(former).unwrap();
        let replacement = registry.spawn(Transform::default());
        assert_ne!(former, replacement);
        assert!(!registry.is_inventory_owned(replacement));
        // Existing stale-slot normalization owns selection repair.
        assert_eq!(
            registry.get_component::<Inventory>(b).unwrap().wieldables[1],
            Some(former)
        );
        registry.clear_for_level_unload();
        assert!(registry.inventory_owners.is_empty());
    }

    #[test]
    fn inventory_ownership_mutable_bypasses_are_refused() {
        let mut registry = EntityRegistry::new();
        let holder = registry.spawn(Transform::default());
        registry
            .set_component(holder, Inventory::default())
            .unwrap();
        assert!(matches!(
            registry.get_component_value_mut(holder, ComponentKind::Inventory),
            Err(RegistryError::InventoryRequiresReplacement)
        ));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            registry
                .for_each_with_kind_mut(ComponentKind::Inventory, |_, _| panic!("must not visit"));
        }));
        assert!(result.is_err());
    }

    #[test]
    fn inventory_ownership_cyclic_holder_retirement_terminates() {
        let mut registry = EntityRegistry::new();
        let a = registry.spawn(Transform::default());
        let b = registry.spawn(Transform::default());
        registry.set_component(a, inventory(b, 0)).unwrap();
        registry.set_component(b, inventory(a, 0)).unwrap();
        registry.despawn(a).unwrap();
        assert!(!registry.exists(a));
        assert!(!registry.exists(b));
        assert!(registry.inventory_owners.is_empty());

        let self_owner = registry.spawn(Transform::default());
        registry
            .set_component(self_owner, inventory(self_owner, 0))
            .unwrap();
        registry.despawn(self_owner).unwrap();
        assert!(!registry.exists(self_owner));
        assert!(registry.inventory_owners.is_empty());
    }
}
