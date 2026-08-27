// Wieldable inventory composition and carried-loadout restore.
// See: context/lib/scripting.md

use std::collections::HashSet;

use super::MapEntity;
use super::data_archetype::{find_descriptor, spawn_descriptor_instance};
use postretro_entities::AmmoReserve;
use postretro_entities::components::inventory::{Inventory, WIELDABLE_SLOT_CAPACITY};
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::components::touchable::TouchableComponent;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::provenance::DescriptorSpawnPath;
use postretro_entities::registry::{EntityId, EntityRegistry};
use postretro_scripting_core::data_descriptors::{EntityTypeDescriptor, WeaponResource};

use crate::netcode::CarriedState;

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

fn restore_carried_reserve(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    carried_reserve: &AmmoReserve,
) {
    let mut restored = registry
        .get_component::<AmmoReserve>(pawn)
        .cloned()
        .unwrap_or_default();
    for (ammo_type, amount) in carried_reserve.balances() {
        restored.set_exact(ammo_type, amount);
    }
    let _ = registry.set_component(pawn, restored);
}

/// Materialize one independent wieldable instance for each authored loadout
/// entry and attach their ordered ownership to the pawn. This is the only
/// composition path for a fresh inventory; live slot fills route through
/// [`acquire_wieldable_at`]. Descriptors hold names, while the inventory holds
/// live entity ids and no input-selection state.
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
        restore_carried_reserve(registry, pawn, &carried_loadout.reserve);
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
        let _ = registry.remove_component::<MeshComponent>(weapon_id);
        let _ = registry.remove_component::<TouchableComponent>(weapon_id);

        if carried_loadout.is_none()
            && let Some(WeaponResource::Ammo(ammo)) = weapon.resource.as_ref()
            && seeded_ammo_types.insert(ammo.ammo_type.clone())
        {
            seed_weapon_reserve(registry, pawn, weapon_descriptor);
        }
    }

    let first_populated_slot = || {
        inventory
            .wieldables
            .iter()
            .position(Option::is_some)
            .unwrap_or(0)
    };
    inventory.active_slot = carried_loadout
        .map(|loadout| loadout.active_slot)
        .filter(|slot| inventory.wieldables.get(*slot).is_some_and(Option::is_some))
        .unwrap_or_else(first_populated_slot);

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

/// Fill one named inventory slot with a live wieldable instance.
///
/// This mutation deliberately owns no acquisition policy. Callers decide
/// whether the item should be taken; this chokepoint only preserves inventory
/// ownership and its visual invariant.
pub(crate) fn acquire_wieldable_at(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    slot: usize,
    item: EntityId,
) -> bool {
    if slot >= WIELDABLE_SLOT_CAPACITY || registry.get_component::<WeaponComponent>(item).is_err() {
        return false;
    }

    let Ok(mut inventory) = registry.get_component::<Inventory>(pawn).cloned() else {
        return false;
    };
    if inventory.wieldables[slot].is_some() {
        return false;
    }

    let inventory_was_empty = inventory.wieldables.iter().all(Option::is_none);
    inventory.wieldables[slot] = Some(item);
    if inventory_was_empty {
        inventory.active_slot = slot;
    }
    if registry.set_component(pawn, inventory).is_err() {
        return false;
    }
    let _ = registry.remove_component::<MeshComponent>(item);
    let _ = registry.remove_component::<TouchableComponent>(item);
    true
}

/// Fill the lowest available inventory slot with a live wieldable instance.
pub(crate) fn acquire_wieldable(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    item: EntityId,
) -> Option<usize> {
    let inventory = registry.get_component::<Inventory>(pawn).ok()?;
    let slot = inventory.wieldables.iter().position(Option::is_none)?;
    acquire_wieldable_at(registry, pawn, slot, item).then_some(slot)
}

/// Release a slot's wieldable and repair selection state around the vacancy.
pub(crate) fn release_wieldable(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    slot: usize,
) -> Option<EntityId> {
    let mut inventory = registry.get_component::<Inventory>(pawn).cloned().ok()?;
    let item = inventory.wieldables.get_mut(slot)?.take()?;

    inventory.active_slot = inventory
        .wieldables
        .iter()
        .position(Option::is_some)
        .unwrap_or(0);
    if inventory.switch_target == Some(slot) {
        inventory.switch_target = None;
    }
    if inventory.switch_origin == Some(slot) {
        inventory.switch_origin = None;
    }
    let _ = registry.set_component(pawn, inventory);
    Some(item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    use postretro_entities::registry::Transform;
    use postretro_scripting_core::data_descriptors::{
        AmmoResource, FireMode, MeshDescriptor, ReloadStyle, ResolutionMode, TouchMode,
        TouchableDescriptor, WeaponDescriptor, WeaponResource,
    };

    fn pawn_descriptor() -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some("player".to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    fn weapon_descriptor(
        canonical_name: &str,
        ammo_type: &str,
        magazine: u32,
        reserve: u32,
    ) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(canonical_name.to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: Some(WeaponDescriptor {
                damage: 10.0,
                pellet_count: 1,
                spread_degrees: 0.0,
                range: 20.0,
                cooldown_ms: 100.0,
                fire_mode: FireMode::Semi,
                resolution: ResolutionMode::Hitscan,
                projectile: None,
                credit_source: None,
                third_person_model: None,
                viewmodel: None,
                placement: None,
                muzzle_offset: None,
                resource: Some(WeaponResource::Ammo(AmmoResource {
                    ammo_type: ammo_type.to_string(),
                    magazine,
                    cost_per_shot: 1,
                    reserve,
                    reload_ms: 1000,
                    reload_style: ReloadStyle::Magazine,
                })),
                lower_ms: 0,
                raise_ms: 0,
                block_during_reload: None,
            }),
            touchable: None,
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

    fn wieldable(registry: &mut EntityRegistry) -> EntityId {
        let descriptor = weapon_descriptor("test_wieldable", "cells", 6, 20);
        let weapon = registry.spawn(Transform::default());
        registry
            .set_component(
                weapon,
                WeaponComponent::from_descriptor(descriptor.weapon.as_ref().unwrap()),
            )
            .unwrap();
        weapon
    }

    #[test]
    fn acquire_fills_the_requested_free_slot_without_resetting_selection_or_reserve() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let retained = wieldable(&mut registry);
        let item = wieldable(&mut registry);
        let mut inventory = Inventory::default();
        inventory.wieldables[1] = Some(retained);
        inventory.active_slot = 1;
        inventory.switch_target = Some(1);
        inventory.switch_origin = Some(1);
        registry.set_component(pawn, inventory).unwrap();
        registry
            .set_component(
                item,
                MeshComponent::stateless("models/item.gltf".to_string()),
            )
            .unwrap();
        registry
            .set_component(
                item,
                TouchableComponent {
                    mode: TouchMode::Auto,
                    radius: 1.0,
                },
            )
            .unwrap();
        let mut reserve = AmmoReserve::default();
        reserve.set_exact("cells", 13);
        registry.set_component(pawn, reserve).unwrap();

        assert!(acquire_wieldable_at(&mut registry, pawn, 4, item));

        let inventory = registry.get_component::<Inventory>(pawn).unwrap();
        assert_eq!(inventory.wieldables[4], Some(item));
        assert_eq!(inventory.active_slot, 1);
        assert_eq!(inventory.switch_target, Some(1));
        assert_eq!(inventory.switch_origin, Some(1));
        assert!(registry.get_component::<MeshComponent>(item).is_err());
        assert!(
            registry.get_component::<TouchableComponent>(item).is_err(),
            "inventory ownership must remove world-item membership"
        );
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("cells"),
            13
        );
    }

    #[test]
    fn acquire_uses_lowest_free_slot_and_selects_it_only_for_an_empty_inventory() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        registry.set_component(pawn, Inventory::default()).unwrap();
        let first = wieldable(&mut registry);
        let second = wieldable(&mut registry);

        assert_eq!(acquire_wieldable(&mut registry, pawn, first), Some(0));
        assert_eq!(acquire_wieldable(&mut registry, pawn, second), Some(1));
        let inventory = registry.get_component::<Inventory>(pawn).unwrap();
        assert_eq!(inventory.active_slot, 0);
        assert_eq!(inventory.wieldables[..2], [Some(first), Some(second)]);
    }

    #[test]
    fn acquire_refuses_occupied_out_of_range_and_weaponless_items() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        registry.set_component(pawn, Inventory::default()).unwrap();
        let weapon = wieldable(&mut registry);
        let bare = registry.spawn(Transform::default());

        assert!(!acquire_wieldable_at(
            &mut registry,
            pawn,
            WIELDABLE_SLOT_CAPACITY,
            weapon
        ));
        assert!(!acquire_wieldable_at(&mut registry, pawn, 0, bare));
        assert!(acquire_wieldable_at(&mut registry, pawn, 0, weapon));
        let other_weapon = wieldable(&mut registry);
        assert!(!acquire_wieldable_at(&mut registry, pawn, 0, other_weapon));
    }

    #[test]
    fn release_reselects_lowest_slot_and_clears_references_to_the_released_slot() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let first = wieldable(&mut registry);
        let released = wieldable(&mut registry);
        let mut inventory = Inventory::default();
        inventory.wieldables[1] = Some(first);
        inventory.wieldables[3] = Some(released);
        inventory.active_slot = 3;
        inventory.switch_target = Some(3);
        inventory.switch_origin = Some(3);
        registry.set_component(pawn, inventory).unwrap();

        assert_eq!(release_wieldable(&mut registry, pawn, 3), Some(released));

        let inventory = registry.get_component::<Inventory>(pawn).unwrap();
        assert_eq!(inventory.wieldables[3], None);
        assert_eq!(inventory.active_slot, 1);
        assert_eq!(inventory.switch_target, None);
        assert_eq!(inventory.switch_origin, None);
    }

    #[test]
    fn release_last_wieldable_leaves_slot_zero_active() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let item = wieldable(&mut registry);
        let mut inventory = Inventory::default();
        inventory.wieldables[2] = Some(item);
        inventory.active_slot = 2;
        registry.set_component(pawn, inventory).unwrap();

        assert_eq!(release_wieldable(&mut registry, pawn, 2), Some(item));
        let inventory = registry.get_component::<Inventory>(pawn).unwrap();
        assert_eq!(inventory.active_slot, 0);
        assert_eq!(inventory.wieldables, [None; WIELDABLE_SLOT_CAPACITY]);
    }

    // Regression: authored loadout composition left touchable weapons registered
    // as world items at their stale player-spawn transform.
    #[test]
    fn authored_loadout_strips_world_item_components_from_held_wieldable() {
        let mut pawn_descriptor = pawn_descriptor();
        pawn_descriptor.inventory = Some(postretro_entities::InventoryDescriptor {
            loadout: vec!["droppable_pistol".to_string()],
        });
        let mut weapon_descriptor = weapon_descriptor("droppable_pistol", "cells", 6, 20);
        weapon_descriptor.touchable = Some(TouchableDescriptor {
            mode: TouchMode::Auto,
            radius: 1.0,
        });
        weapon_descriptor.mesh = Some(MeshDescriptor {
            model: "models/pistol_world.gltf".to_string(),
            shadow_only: false,
            attachments: Default::default(),
            shadow_bias_scale: 1.0,
            animations: Default::default(),
            default_state: None,
            locomotion: None,
        });
        let descriptors = [pawn_descriptor.clone(), weapon_descriptor];
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());

        let _ = compose_wieldable_inventory(
            &mut registry,
            pawn,
            &pawn_descriptor,
            &placement(),
            &descriptors,
            None,
        );

        let held = registry
            .get_component::<Inventory>(pawn)
            .unwrap()
            .wieldables[0]
            .expect("authored loadout materializes");
        assert!(registry.get_component::<MeshComponent>(held).is_err());
        assert!(
            registry.get_component::<TouchableComponent>(held).is_err(),
            "held loadout instance is not a host world item"
        );
    }

    #[test]
    fn inventory_slot_fills_are_limited_to_composition_and_acquisition_chokepoints() {
        let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let crates_dir = workspace_root.join("crates");
        let mut fill_sites = Vec::new();
        collect_slot_fill_sites(&crates_dir, &crates_dir, &mut fill_sites);
        fill_sites.sort();

        assert_eq!(
            fill_sites,
            vec![
                PathBuf::from("postretro/src/scripting/builtins/wieldable_inventory.rs"),
                PathBuf::from("postretro/src/scripting/builtins/wieldable_inventory.rs"),
            ],
            "live inventory slot fills must route through composition or acquire_wieldable_at"
        );
    }

    fn collect_slot_fill_sites(root: &Path, path: &Path, fill_sites: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };

        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                collect_slot_fill_sites(root, &path, fill_sites);
            } else if path.extension().is_some_and(|extension| extension == "rs")
                && !is_test_source_file(root, &path)
            {
                let Ok(source) = fs::read_to_string(&path) else {
                    continue;
                };
                let production_source = mask_test_only_blocks(&source);
                let relative_path = path.strip_prefix(root).map(PathBuf::from).unwrap_or(path);
                for _ in 0..inventory_slot_fill_count(&production_source) {
                    fill_sites.push(relative_path.clone());
                }
            }
        }
    }

    fn is_test_source_file(root: &Path, path: &Path) -> bool {
        let Ok(relative_path) = path.strip_prefix(root) else {
            return false;
        };
        let is_in_integration_tests = relative_path
            .components()
            .any(|component| component.as_os_str() == "tests");
        let is_test_harness = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| {
                stem.ends_with("_test")
                    || stem.ends_with("_tests")
                    || stem.ends_with("_test_fixtures")
                    || stem.ends_with("_harness")
            });

        is_in_integration_tests || is_test_harness
    }

    fn inventory_slot_fill_count(source: &str) -> usize {
        let mut count = 0;
        let mut rest = source;
        while let Some(offset) = rest.find(".wieldables[") {
            let after_open = &rest[offset + ".wieldables[".len()..];
            let Some(close) = after_open.find(']') else {
                break;
            };
            let after_slot = after_open[close + 1..].trim_start();
            if after_slot.starts_with("= Some(") {
                count += 1;
            }
            rest = after_open;
        }
        count
    }

    fn mask_test_only_blocks(source: &str) -> String {
        let mut masked = mask_comments_and_string_literals(source);
        let mut search_start = 0;

        while let Some(attribute_offset) = masked[search_start..].find("#[cfg(test)]") {
            let attribute_start = search_start + attribute_offset;
            let Some(body_start) = masked[attribute_start..].find('{') else {
                break;
            };
            let body_start = attribute_start + body_start;
            let Some(body_end) = matching_brace_end(&masked, body_start) else {
                break;
            };
            masked.replace_range(
                attribute_start..body_end,
                &" ".repeat(body_end - attribute_start),
            );
            search_start = body_end;
        }

        masked
    }

    fn matching_brace_end(source: &str, opening_brace: usize) -> Option<usize> {
        let mut depth = 0usize;
        for (offset, byte) in source[opening_brace..].bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(opening_brace + offset + 1);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn mask_comments_and_string_literals(source: &str) -> String {
        let mut masked = String::with_capacity(source.len());
        let mut chars = source.char_indices().peekable();
        let mut block_comment_depth = 0usize;
        let mut in_line_comment = false;
        let mut in_string = false;
        let mut string_escape = false;
        let mut raw_string_hashes: Option<usize> = None;
        let mut character_literal_closing_offset: Option<usize> = None;

        while let Some((index, ch)) = chars.next() {
            let next = chars.peek().map(|(_, next)| *next);
            if in_line_comment {
                if ch == '\n' {
                    in_line_comment = false;
                    masked.push('\n');
                } else {
                    push_masked_char(&mut masked, ch);
                }
                continue;
            }
            if block_comment_depth > 0 {
                if ch == '/' && next == Some('*') {
                    block_comment_depth += 1;
                    push_masked_char(&mut masked, ch);
                    if let Some((_, next_ch)) = chars.next() {
                        push_masked_char(&mut masked, next_ch);
                    }
                } else if ch == '*' && next == Some('/') {
                    block_comment_depth -= 1;
                    push_masked_char(&mut masked, ch);
                    if let Some((_, next_ch)) = chars.next() {
                        push_masked_char(&mut masked, next_ch);
                    }
                } else if ch == '\n' {
                    masked.push('\n');
                } else {
                    push_masked_char(&mut masked, ch);
                }
                continue;
            }
            if let Some(hash_count) = raw_string_hashes {
                if raw_string_closes(source, index, hash_count) {
                    raw_string_hashes = None;
                    push_masked_char(&mut masked, ch);
                    for _ in 0..hash_count {
                        if let Some((_, next_ch)) = chars.next() {
                            push_masked_char(&mut masked, next_ch);
                        }
                    }
                } else if ch == '\n' {
                    masked.push('\n');
                } else {
                    push_masked_char(&mut masked, ch);
                }
                continue;
            }
            if let Some(end) = character_literal_closing_offset {
                push_masked_char(&mut masked, ch);
                if index == end {
                    character_literal_closing_offset = None;
                }
                continue;
            }
            if in_string {
                if ch == '\n' {
                    masked.push('\n');
                } else {
                    push_masked_char(&mut masked, ch);
                }
                if string_escape {
                    string_escape = false;
                } else if ch == '\\' {
                    string_escape = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            if ch == '/' && next == Some('/') {
                in_line_comment = true;
                push_masked_char(&mut masked, ch);
                if let Some((_, next_ch)) = chars.next() {
                    push_masked_char(&mut masked, next_ch);
                }
            } else if ch == '/' && next == Some('*') {
                block_comment_depth = 1;
                push_masked_char(&mut masked, ch);
                if let Some((_, next_ch)) = chars.next() {
                    push_masked_char(&mut masked, next_ch);
                }
            } else if let Some(hash_count) = raw_string_start(source, index) {
                raw_string_hashes = Some(hash_count);
                push_masked_char(&mut masked, ch);
                for _ in 0..hash_count {
                    if let Some((_, next_ch)) = chars.next() {
                        push_masked_char(&mut masked, next_ch);
                    }
                }
                if let Some((_, next_ch)) = chars.next() {
                    push_masked_char(&mut masked, next_ch);
                }
            } else if ch == '"' {
                in_string = true;
                string_escape = false;
                push_masked_char(&mut masked, ch);
            } else if ch == '\''
                && let Some(end) = character_literal_end(source, index)
            {
                character_literal_closing_offset = Some(end);
                push_masked_char(&mut masked, ch);
            } else {
                masked.push(ch);
            }
        }
        masked
    }

    fn push_masked_char(masked: &mut String, ch: char) {
        for _ in 0..ch.len_utf8() {
            masked.push(' ');
        }
    }

    fn raw_string_start(source: &str, index: usize) -> Option<usize> {
        let rest = source.get(index..)?;
        let mut chars = rest.chars();
        if chars.next()? != 'r' {
            return None;
        }
        let mut hash_count = 0usize;
        for ch in chars {
            match ch {
                '#' => hash_count += 1,
                '"' => return Some(hash_count),
                _ => return None,
            }
        }
        None
    }

    fn raw_string_closes(source: &str, index: usize, hash_count: usize) -> bool {
        let Some(rest) = source.get(index..) else {
            return false;
        };
        rest.starts_with('"') && rest[1..].starts_with(&"#".repeat(hash_count))
    }

    fn character_literal_end(source: &str, index: usize) -> Option<usize> {
        let rest = source.get(index..)?;
        let mut chars = rest.char_indices();
        if chars.next()?.1 != '\'' {
            return None;
        }
        let (_, first) = chars.next()?;
        if first == '\\' {
            for (offset, ch) in chars {
                if ch == '\'' {
                    return Some(index + offset);
                }
                if ch == '\n' {
                    return None;
                }
            }
            return None;
        }
        let (offset, closing) = chars.next()?;
        (closing == '\'').then_some(index + offset)
    }

    #[test]
    fn carried_loadout_overrides_defaults_and_restores_exact_weapon_state() {
        let pawn_descriptor = pawn_descriptor();
        let descriptors = [
            pawn_descriptor.clone(),
            weapon_descriptor("pistol", "shells", 8, 50),
            weapon_descriptor("launcher", "rockets", 4, 12),
        ];
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let mut carried = CarriedState {
            active_slot: 2,
            ..Default::default()
        };
        carried.wieldables[1] = Some("pistol".to_string());
        carried.wieldables[2] = Some("launcher".to_string());
        carried.magazines[1] = Some(3);
        carried.magazines[2] = Some(3);
        carried.reserve.set_exact("shells", 11);
        carried.reserve.set_exact("rockets", 0);
        carried.reserve.set_exact("cells", 23);

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
        let pistol = inventory.wieldables[1].expect("carried slot materializes pistol");
        let launcher = inventory.wieldables[2].expect("carried slot materializes launcher");
        assert_eq!(active, Some(launcher));
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(pistol)
                .expect("pistol component materialized")
                .magazine,
            3
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(launcher)
                .expect("launcher component materialized")
                .magazine,
            3
        );
        let reserve = registry
            .get_component::<AmmoReserve>(pawn)
            .expect("carried reserve materialized");
        assert_eq!(reserve.available("shells"), 11);
        assert_eq!(reserve.available("rockets"), 0);
        assert_eq!(reserve.available("cells"), 23);
        assert!(
            reserve
                .balances()
                .any(|(ammo_type, amount)| ammo_type == "rockets" && amount == 0),
            "zero carried balances remain explicitly represented"
        );
    }

    // Regression: an active index absent from the new loadout spawned empty-handed.
    #[test]
    fn carried_missing_active_slot_falls_back_to_first_materialized_weapon() {
        let pawn_descriptor = pawn_descriptor();
        let descriptors = [
            pawn_descriptor.clone(),
            weapon_descriptor("pistol", "shells", 8, 50),
        ];
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let mut carried = CarriedState {
            active_slot: 2,
            ..Default::default()
        };
        carried.wieldables[1] = Some("pistol".to_string());
        carried.wieldables[2] = Some("missing_weapon".to_string());

        let active = compose_wieldable_inventory(
            &mut registry,
            pawn,
            &pawn_descriptor,
            &placement(),
            &descriptors,
            Some(&carried),
        );

        let inventory = registry.get_component::<Inventory>(pawn).unwrap();
        assert_eq!(inventory.active_slot, 1);
        assert_eq!(active, inventory.wieldables[1]);
    }
}
