use std::collections::HashSet;

use super::MapEntity;
use super::data_archetype::{find_descriptor, spawn_descriptor_instance};
use postretro_entities::AmmoReserve;
use postretro_entities::components::inventory::{Inventory, WIELDABLE_SLOT_CAPACITY};
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::provenance::DescriptorSpawnPath;
use postretro_entities::registry::{EntityId, EntityRegistry};
use postretro_scripting_core::data_descriptors::{EntityTypeDescriptor, WeaponResource};

use crate::netcode::CarriedState;

fn seed_weapon_reserve(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    weapon_descriptor: &EntityTypeDescriptor,
    carried_reserve: Option<&AmmoReserve>,
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
    if let Some(carried_reserve) = carried_reserve {
        reserve.set_exact(&ammo.ammo_type, carried_reserve.available(&ammo.ammo_type));
    } else {
        reserve.credit(&ammo.ammo_type, ammo.reserve);
    }
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
    carried_loadout: Option<&CarriedState>,
) -> Option<EntityId> {
    let authored_loadout = pawn_descriptor
        .inventory
        .as_ref()
        .map(|inventory| &inventory.loadout);
    if carried_loadout.is_none() && authored_loadout.is_none() {
        let inventory = Inventory::default();
        let _ = registry.set_component(pawn, inventory);
        return None;
    }

    if let Some(carried_loadout) = carried_loadout {
        compose_wieldable_inventory_slots(
            registry,
            pawn,
            placement,
            descriptors,
            carried_loadout
                .wieldables
                .iter()
                .enumerate()
                .map(|(slot, canonical_name)| (slot, canonical_name.as_deref())),
            Some(carried_loadout),
        )
    } else {
        let loadout = authored_loadout.expect("handled by no-loadout return above");
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
            None,
        )
    }
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
    carried_loadout: Option<&CarriedState>,
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
        carried_loadout,
    )
}

fn compose_wieldable_inventory_slots<'a>(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    placement: &MapEntity,
    descriptors: &[EntityTypeDescriptor],
    slots: impl IntoIterator<Item = (usize, Option<&'a str>)>,
    carried_loadout: Option<&CarriedState>,
) -> Option<EntityId> {
    let mut inventory = Inventory::default();
    let mut seeded_ammo_types = HashSet::new();

    if let Some(carried_loadout) = carried_loadout {
        let _ = registry.set_component(pawn, carried_loadout.reserve.clone());
    }

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
            seed_weapon_reserve(
                registry,
                pawn,
                weapon_descriptor,
                carried_loadout.map(|loadout| &loadout.reserve),
            );
        }
    }

    inventory.active_slot = carried_loadout.map_or_else(
        || {
            inventory
                .wieldables
                .iter()
                .position(Option::is_some)
                .unwrap_or(0)
        },
        |loadout| loadout.active_slot,
    );

    if let Some(carried_loadout) = carried_loadout {
        for (slot, weapon_id) in inventory.wieldables.iter().enumerate() {
            let (Some(weapon_id), Some(magazine)) = (weapon_id, carried_loadout.magazines[slot])
            else {
                continue;
            };
            let Ok(mut weapon) = registry
                .get_component::<WeaponComponent>(*weapon_id)
                .cloned()
            else {
                continue;
            };
            weapon.magazine = magazine;
            let _ = registry.set_component(*weapon_id, weapon);
        }
    }

    let active_weapon = inventory.active_wieldable();
    let _ = registry.set_component(pawn, inventory);
    active_weapon
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::registry::Transform;
    use postretro_scripting_core::data_descriptors::{
        AmmoResource, FireMode, ReloadStyle, ResolutionMode, WeaponDescriptor, WeaponResource,
    };

    fn pawn_descriptor() -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some("player".to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    fn weapon_descriptor() -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some("pistol".to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: Some(WeaponDescriptor {
                damage: 10.0,
                range: 20.0,
                cooldown_ms: 100.0,
                fire_mode: FireMode::Semi,
                resolution: ResolutionMode::Hitscan,
                credit_source: None,
                third_person_model: None,
                viewmodel: None,
                resource: Some(WeaponResource::Ammo(AmmoResource {
                    ammo_type: "shells".to_string(),
                    magazine: 8,
                    cost_per_shot: 1,
                    reserve: 50,
                    reload_ms: 1000,
                    reload_style: ReloadStyle::Magazine,
                })),
                lower_ms: 0,
                raise_ms: 0,
                block_during_reload: None,
            }),
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    fn placement() -> MapEntity {
        MapEntity {
            classname: "player_spawn".to_string(),
            origin: glam::Vec3::ZERO,
            angles: glam::Vec3::ZERO,
            key_values: Default::default(),
            tags: vec![],
        }
    }

    #[test]
    fn carried_loadout_overrides_defaults_and_restores_exact_weapon_state() {
        let pawn_descriptor = pawn_descriptor();
        let weapon_descriptor = weapon_descriptor();
        let descriptors = [pawn_descriptor.clone(), weapon_descriptor];
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let mut carried = CarriedState {
            active_slot: 2,
            ..Default::default()
        };
        carried.wieldables[2] = Some("pistol".to_string());
        carried.magazines[2] = Some(3);
        carried.reserve.set_exact("shells", 11);
        carried.reserve.set_exact("rockets", 0);

        let active = compose_wieldable_inventory(
            &mut registry,
            pawn,
            &pawn_descriptor,
            &placement(),
            &descriptors,
            Some(&carried),
        );

        let inventory = registry
            .get_component::<Inventory>(pawn)
            .expect("carried loadout materializes an inventory");
        assert_eq!(inventory.active_slot, 2);
        let weapon = inventory.wieldables[2].expect("carried slot materializes pistol");
        assert_eq!(active, Some(weapon));
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .expect("weapon component materialized")
                .magazine,
            3
        );
        let reserve = registry
            .get_component::<AmmoReserve>(pawn)
            .expect("carried reserve materialized");
        assert_eq!(reserve.available("shells"), 11);
        assert_eq!(reserve.available("rockets"), 0);
    }
}
