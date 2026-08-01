use std::collections::HashSet;

use super::MapEntity;
use super::data_archetype::{find_descriptor, spawn_descriptor_instance};
use postretro_entities::AmmoReserve;
use postretro_entities::components::inventory::{Inventory, WIELDABLE_SLOT_CAPACITY};
use postretro_entities::provenance::DescriptorSpawnPath;
use postretro_entities::registry::{EntityId, EntityRegistry};
use postretro_scripting_core::data_descriptors::{EntityTypeDescriptor, WeaponResource};

fn seed_weapon_reserve(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    weapon_descriptor: &EntityTypeDescriptor,
) {
    let Some(WeaponResource::Ammo(ammo)) = weapon_descriptor
        .weapon
        .as_ref()
        .and_then(|weapon| weapon.resource.as_ref())
    else {
        return;
    };

    let mut reserve = registry
        .get_component::<AmmoReserve>(pawn)
        .cloned()
        .unwrap_or_default();
    reserve.credit(&ammo.ammo_type, ammo.reserve);
    let _ = registry.set_component(pawn, reserve);
}

/// Materialize one independent wieldable instance for each authored loadout
/// entry and attach their ordered ownership to the pawn. This is the only
/// composition path: descriptors hold names, while the inventory holds live
/// entity ids and no input-selection state.
pub(super) fn compose_wieldable_inventory(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    pawn_descriptor: &EntityTypeDescriptor,
    placement: &MapEntity,
    descriptors: &[EntityTypeDescriptor],
) -> Option<EntityId> {
    let inventory = Inventory::default();
    let Some(loadout) = pawn_descriptor
        .inventory
        .as_ref()
        .map(|inventory| &inventory.loadout)
    else {
        let _ = registry.set_component(pawn, inventory);
        return None;
    };

    compose_wieldable_inventory_slots(
        registry,
        pawn,
        placement,
        descriptors,
        loadout
            .iter()
            .take(WIELDABLE_SLOT_CAPACITY)
            .enumerate()
            .map(|(slot, canonical_name)| (slot, Some(canonical_name.as_str()))),
    )
}

/// Compose a local inventory from explicit slot identities. The connected-client
/// tuning path uses this only when Control arrives before its pawn baseline: the
/// host has already resolved the slot layout, including interior empty slots.
pub(crate) fn compose_wieldable_inventory_from_slots(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    placement: &MapEntity,
    descriptors: &[EntityTypeDescriptor],
    slots: &[Option<String>; WIELDABLE_SLOT_CAPACITY],
) -> Option<EntityId> {
    compose_wieldable_inventory_slots(
        registry,
        pawn,
        placement,
        descriptors,
        slots
            .iter()
            .enumerate()
            .map(|(slot, canonical_name)| (slot, canonical_name.as_deref())),
    )
}

fn compose_wieldable_inventory_slots<'a>(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    placement: &MapEntity,
    descriptors: &[EntityTypeDescriptor],
    slots: impl IntoIterator<Item = (usize, Option<&'a str>)>,
) -> Option<EntityId> {
    let mut inventory = Inventory::default();
    let mut seeded_ammo_types = HashSet::new();

    for (slot, canonical_name) in slots {
        let Some(canonical_name) = canonical_name else {
            continue;
        };
        let Some(weapon_descriptor) = find_descriptor(descriptors, canonical_name) else {
            log::warn!(
                "[Loader] {}: inventory loadout `{canonical_name}` not registered; slot {slot} stays empty",
                placement.diagnostic_origin(),
            );
            continue;
        };
        let Some(weapon) = weapon_descriptor.weapon.as_ref() else {
            log::warn!(
                "[Loader] {}: inventory loadout `{canonical_name}` has no weapon component; slot {slot} stays empty",
                placement.diagnostic_origin(),
            );
            continue;
        };
        let weapon_entity = MapEntity {
            classname: canonical_name.to_string(),
            origin: placement.origin,
            angles: placement.angles,
            key_values: Default::default(),
            tags: vec![],
        };
        let Some(weapon_id) = spawn_descriptor_instance(
            registry,
            weapon_descriptor,
            &weapon_entity,
            true,
            DescriptorSpawnPath::DefaultWeapon,
            None,
        ) else {
            log::warn!(
                "[Loader] {}: entity registry exhausted; dropping inventory loadout `{canonical_name}`",
                placement.diagnostic_origin(),
            );
            continue;
        };
        let _ = registry.set_map_kvps(weapon_id, Default::default());
        inventory.wieldables[slot] = Some(weapon_id);

        if let Some(WeaponResource::Ammo(ammo)) = weapon.resource.as_ref()
            && seeded_ammo_types.insert(ammo.ammo_type.clone())
        {
            seed_weapon_reserve(registry, pawn, weapon_descriptor);
        }
    }

    let active = inventory.wieldables.iter().position(Option::is_some);
    if let Some(active_slot) = active {
        inventory.active_slot = active_slot;
    }
    let active_weapon = inventory.active_wieldable();
    let _ = registry.set_component(pawn, inventory);
    active_weapon
}
