// A carried loadout across the wire: conversions both ways, and the host's
// check of a joiner's loadout against its own mod.
// See: context/lib/networking.md §Session-state ledger · §Slot lifecycle

use postretro_combat_model::CarriedState;
use postretro_entities::EntityTypeDescriptor;
use postretro_entities::components::inventory::WIELDABLE_SLOT_CAPACITY;
use postretro_foundation::WeaponResource;
use postretro_foundation::defense::DefenseVocabulary;
use postretro_net::wire::CarriedLoadoutWire;
use postretro_sim::scripting::builtins::data_archetype::find_descriptor;
use postretro_sim::scripting::builtins::player_spawn::spawn_descriptor;
use postretro_sim::scripting::map_entity::MapEntity;
use postretro_sim::scripting::save_files::{LayerRecord, LoadoutRecord};

/// Hundredths, rounded. Rust's float-to-int cast saturates, and NaN casts to 0.
fn to_centi(value: f32) -> i32 {
    (value * 100.0).round() as i32
}

fn from_centi(value: i32) -> f32 {
    value as f32 / 100.0
}

/// The wire mirror of a character file's loadout record.
#[must_use]
pub fn loadout_record_to_wire(record: &LoadoutRecord) -> CarriedLoadoutWire {
    CarriedLoadoutWire {
        health_centi: record.health.map(to_centi),
        life_state: record.life_state.clone(),
        reserve: record
            .reserve
            .iter()
            .map(|(ammo, amount)| (ammo.clone(), *amount))
            .collect(),
        wieldables: record.wieldables.clone(),
        magazines: record.magazines.clone(),
        active_slot: u32::try_from(record.active_slot).unwrap_or(u32::MAX),
        layers: record
            .layers
            .iter()
            .map(|layer| (layer.name.clone(), to_centi(layer.value)))
            .collect(),
    }
}

/// The loadout record a wire loadout names, before any check. Duplicate ammo
/// names keep the last balance.
#[must_use]
pub fn loadout_record_from_wire(wire: &CarriedLoadoutWire) -> LoadoutRecord {
    LoadoutRecord {
        health: wire.health_centi.map(from_centi),
        life_state: wire.life_state.clone(),
        reserve: wire.reserve.iter().cloned().collect(),
        wieldables: wire.wieldables.clone(),
        magazines: wire.magazines.clone(),
        active_slot: usize::try_from(wire.active_slot).unwrap_or(usize::MAX),
        layers: wire
            .layers
            .iter()
            .map(|(name, value)| LayerRecord {
                name: name.clone(),
                value: from_centi(*value),
            })
            .collect(),
    }
}

/// A seat's carry as it crosses the wire. `None` is a seat with no carry.
#[must_use]
pub fn carried_to_wire(carried: Option<&CarriedState>) -> Option<CarriedLoadoutWire> {
    carried.map(|carried| loadout_record_to_wire(&LoadoutRecord::from_carried(Some(carried))))
}

/// The carry a wire loadout restores, unchecked against any mod: a client
/// reading the loadout its own host sent. Entries past this build's wieldable
/// capacity drop with a warning; an empty loadout is no carry, as an empty
/// character file is.
#[must_use]
pub fn carried_from_wire(
    wire: &CarriedLoadoutWire,
    warnings: &mut Vec<String>,
) -> Option<CarriedState> {
    loadout_record_from_wire(wire).to_carried(warnings)
}

/// What a host's mod defines, for checking a joiner's loadout against it.
///
/// - A weapon is defined when a descriptor with that canonical name has a
///   weapon component.
/// - An ammo type is defined when some weapon descriptor's ammo resource names
///   it. Ammo grants are reaction steps whose pool name is free text, so they
///   define nothing: a reserve no weapon of this mod spends is dropped.
/// - A defense layer is defined when the mod registers it.
/// - The player's maximum health and initial life state come from the
///   descriptor a joiner spawns as: the first joinable `player_spawn`'s
///   `entity_class`, else `player`.
pub struct HostGear<'a> {
    descriptors: &'a [EntityTypeDescriptor],
    layers: &'a [String],
    player: Option<&'a EntityTypeDescriptor>,
}

impl<'a> HostGear<'a> {
    /// The host's gear vocabulary. `spawn_points` are the installed level's;
    /// with none (a host at its frontend), the `player` descriptor stands in.
    #[must_use]
    pub fn new(
        descriptors: &'a [EntityTypeDescriptor],
        defense: &'a DefenseVocabulary,
        spawn_points: &[MapEntity],
    ) -> Self {
        let player = spawn_points
            .iter()
            .find_map(|spawn| spawn_descriptor(spawn, descriptors))
            .or_else(|| find_descriptor(descriptors, "player"));
        Self {
            descriptors,
            layers: &defense.layers,
            player,
        }
    }

    /// The magazine size of a weapon this mod defines: its ammo resource's
    /// magazine, or 0 for a heat or cell weapon. `None` when the mod does not
    /// define the weapon.
    fn weapon_magazine(&self, name: &str) -> Option<u32> {
        let weapon = find_descriptor(self.descriptors, name)?.weapon.as_ref()?;
        Some(match weapon.resource.as_ref() {
            Some(WeaponResource::Ammo(ammo)) => ammo.magazine,
            _ => 0,
        })
    }

    fn defines_ammo(&self, ammo_type: &str) -> bool {
        self.descriptors.iter().any(|descriptor| {
            matches!(
                descriptor.weapon.as_ref().and_then(|weapon| weapon.resource.as_ref()),
                Some(WeaponResource::Ammo(ammo)) if ammo.ammo_type == ammo_type
            )
        })
    }

    fn defines_layer(&self, layer: &str) -> bool {
        self.layers.iter().any(|registered| registered == layer)
    }

    fn player_health_max(&self) -> Option<f32> {
        self.player?.health.as_ref().map(|health| health.max)
    }

    fn initial_life_state(&self) -> Option<&str> {
        self.player?
            .life_states
            .as_ref()
            .map(|life_states| life_states.initial.as_str())
    }
}

/// Check a joining character's loadout against the host's mod, returning the
/// carry its seat takes. Each drop or clamp pushes a warning naming what and
/// why; a loadout this mod fully defines restores whole.
///
/// Weapons, ammo types and defense layers this mod does not define drop. A
/// magazine above its weapon's size, and health above the player's maximum,
/// clamp. A life state other than the initial one arrives initial. An active
/// slot naming no kept weapon selects slot 0.
#[must_use]
pub fn check_join_loadout(
    wire: &CarriedLoadoutWire,
    gear: &HostGear<'_>,
    warnings: &mut Vec<String>,
) -> Option<CarriedState> {
    let mut record = loadout_record_from_wire(wire);

    record.reserve.retain(|ammo, _| {
        let defined = gear.defines_ammo(ammo);
        if !defined {
            warnings.push(format!(
                "dropped ammo `{ammo}`: no weapon of this mod uses that ammo type"
            ));
        }
        defined
    });

    for (slot, weapon) in record.wieldables.iter_mut().enumerate() {
        let Some(name) = weapon.as_deref() else {
            continue;
        };
        if gear.weapon_magazine(name).is_none() {
            warnings.push(format!(
                "dropped weapon `{name}` in slot {slot}: this mod defines no such weapon"
            ));
            *weapon = None;
        }
    }
    for (slot, magazine) in record.magazines.iter_mut().enumerate() {
        let Some(count) = *magazine else {
            continue;
        };
        let weapon = record.wieldables.get(slot).cloned().flatten();
        let Some(size) = weapon
            .as_deref()
            .and_then(|name| gear.weapon_magazine(name))
        else {
            // A dropped weapon's own warning covers its magazine; a magazine
            // in a slot that never held a weapon is malformed on its own.
            if wire.wieldables.get(slot).is_none_or(Option::is_none) {
                warnings.push(format!(
                    "dropped the magazine in slot {slot}: the slot holds no weapon"
                ));
            }
            *magazine = None;
            continue;
        };
        if count > size {
            warnings.push(format!(
                "clamped weapon `{}` magazine in slot {slot} from {count} to its magazine size {size}",
                weapon.as_deref().unwrap_or_default()
            ));
            *magazine = Some(size);
        }
    }

    if let Some(health) = record.health {
        match gear.player_health_max() {
            Some(max) if health > max => {
                warnings.push(format!(
                    "clamped health from {health} to the player's maximum {max}"
                ));
                record.health = Some(max);
            }
            Some(_) => {}
            None => {
                warnings.push(format!(
                    "dropped health {health}: this mod's player has no health"
                ));
                record.health = None;
            }
        }
    }

    if let Some(life_state) = record.life_state.as_deref()
        && Some(life_state) != gear.initial_life_state()
    {
        warnings.push(format!(
            "life state `{life_state}` arrives as the initial state: a joining character never carries another"
        ));
        record.life_state = None;
    }

    record.layers.retain(|layer| {
        let defined = gear.defines_layer(&layer.name);
        if !defined {
            warnings.push(format!(
                "dropped defense layer `{}`: this mod registers no such layer",
                layer.name
            ));
        }
        defined
    });

    let active = record.active_slot;
    if active != 0
        && active < WIELDABLE_SLOT_CAPACITY
        && record.wieldables.get(active).is_none_or(Option::is_none)
    {
        warnings.push(format!(
            "active slot {active} holds no kept weapon; selecting slot 0"
        ));
        record.active_slot = 0;
    }

    // Capacity: entries past this build's wieldable slots drop, and an active
    // slot past them selects slot 0, each with its own warning.
    record.to_carried(warnings)
}

#[cfg(test)]
#[path = "loadout_wire_tests.rs"]
mod tests;
