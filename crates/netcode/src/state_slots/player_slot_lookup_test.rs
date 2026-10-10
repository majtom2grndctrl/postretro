// The shared per-pawn lookup, driven by the engine-state catalog: every slot
// the catalog marks per-player has a per-pawn source, and the host reads and
// replicates each pawn's own value.
// See: context/lib/scripting.md §5 (Per-player engine slots)

use std::collections::HashMap;

use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::components::wieldable_state::WieldableState;
use postretro_entities::data_descriptors::{HealthDescriptor, WeaponDescriptor};
use postretro_entities::engine_state_catalog::{EngineStateValueType, engine_state_catalog};
use postretro_entities::{AmmoReserve, EntityId, EntityRegistry, SlotTable, SlotValue, Transform};
use postretro_net::state_slots::WireSlotValue;
use postretro_scripting_core::player_slots::{PlayerSlot, player_slot_value};

use super::{HostStateReplication, ReplicatedSlotIdentity, ReplicatedSlotSchema};
use crate::netcode::command_queue::{MovementOwners, WeaponOwners};

const CLIENT_A: u64 = 1;
const CLIENT_B: u64 = 2;

/// One pawn's component state: health plus an optional active weapon in slot
/// 0 and an ammo reserve.
struct PawnState {
    health: (f32, f32),
    weapon: Option<WeaponComponent>,
    reserve: u32,
}

impl PawnState {
    fn healthy(current: f32, max: f32) -> Self {
        Self {
            health: (current, max),
            weapon: None,
            reserve: 0,
        }
    }

    fn wielding(weapon: WeaponComponent) -> Self {
        Self {
            health: (100.0, 100.0),
            weapon: Some(weapon),
            reserve: 0,
        }
    }
}

fn weapon(resource: serde_json::Value) -> WeaponComponent {
    let descriptor: WeaponDescriptor = serde_json::from_value(serde_json::json!({
        "damage": 10.0,
        "range": 64.0,
        "primary": { "trigger": "hold", "recoveryMs": 100.0, "steps": [{ "kind": "shot" }] },
        "resolution": "hitscan",
        "resource": resource,
    }))
    .unwrap();
    WeaponComponent::from_descriptor(&descriptor)
}

fn ammo_weapon(magazine: u32) -> WeaponComponent {
    let mut component = weapon(serde_json::json!({
        "kind": "ammo", "type": "rounds", "magazine": 8, "reserve": 0, "reloadMs": 800
    }));
    component.magazine = magazine;
    component
}

fn heat_weapon(heat: f32, overheat_at: f32, overheated: bool) -> WeaponComponent {
    let mut component = weapon(serde_json::json!({
        "kind": "heat", "heatPerShot": 10.0, "overheatAt": overheat_at, "coolPerSecond": 20.0
    }));
    let live = component.heat.as_mut().unwrap();
    live.heat = heat;
    live.overheated = overheated;
    component
}

fn cell_weapon(charge: f32, capacity: f32) -> WeaponComponent {
    let mut component = weapon(serde_json::json!({
        "kind": "cell", "capacity": capacity, "costPerShot": 4.0, "regenPerSecond": 8.0
    }));
    component.cell.as_mut().unwrap().charge = charge;
    component
}

fn reloading(remaining_ms: u32) -> WeaponComponent {
    let mut component = ammo_weapon(2);
    component.state = WieldableState::Reloading;
    component.state_remaining_ms = remaining_ms;
    component.state_total_ms = 800;
    component
}

fn cooling(cooldown_ms: f32) -> WeaponComponent {
    let mut component = ammo_weapon(8);
    component.cooldown_remaining_ms = cooldown_ms;
    component
}

/// Two pawn states that differ in `slot`'s value. Exhaustive, so a slot added
/// to the lookup cannot skip this test.
fn differing_states(slot: PlayerSlot) -> [PawnState; 2] {
    match slot {
        PlayerSlot::Health => [
            PawnState::healthy(80.0, 100.0),
            PawnState::healthy(40.0, 100.0),
        ],
        PlayerSlot::MaxHealth => [
            PawnState::healthy(50.0, 100.0),
            PawnState::healthy(50.0, 60.0),
        ],
        PlayerSlot::Ammo => [
            PawnState::wielding(ammo_weapon(6)),
            PawnState::wielding(ammo_weapon(3)),
        ],
        PlayerSlot::AmmoReserve => [
            PawnState {
                reserve: 30,
                ..PawnState::wielding(ammo_weapon(8))
            },
            PawnState {
                reserve: 10,
                ..PawnState::wielding(ammo_weapon(8))
            },
        ],
        PlayerSlot::Heat => [
            PawnState::wielding(heat_weapon(30.0, 80.0, false)),
            PawnState::wielding(heat_weapon(50.0, 80.0, false)),
        ],
        PlayerSlot::OverheatAt => [
            PawnState::wielding(heat_weapon(10.0, 80.0, false)),
            PawnState::wielding(heat_weapon(10.0, 60.0, false)),
        ],
        PlayerSlot::Overheated => [
            PawnState::wielding(heat_weapon(80.0, 80.0, true)),
            PawnState::wielding(heat_weapon(10.0, 80.0, false)),
        ],
        PlayerSlot::Cell => [
            PawnState::wielding(cell_weapon(12.0, 40.0)),
            PawnState::wielding(cell_weapon(20.0, 40.0)),
        ],
        PlayerSlot::CellCapacity => [
            PawnState::wielding(cell_weapon(10.0, 40.0)),
            PawnState::wielding(cell_weapon(10.0, 30.0)),
        ],
        PlayerSlot::ReloadActive => [
            PawnState::wielding(reloading(400)),
            PawnState::wielding(ammo_weapon(8)),
        ],
        PlayerSlot::ReloadProgress => [
            PawnState::wielding(reloading(600)),
            PawnState::wielding(reloading(200)),
        ],
        PlayerSlot::WeaponCooldownMs => [
            PawnState::wielding(cooling(42.0)),
            PawnState::wielding(cooling(10.0)),
        ],
    }
}

fn spawn(registry: &mut EntityRegistry, state: PawnState) -> EntityId {
    let pawn = registry.spawn(Transform::default());
    let mut health = HealthComponent::from_descriptor(&HealthDescriptor {
        max: state.health.1,
        hitbox: None,
        zone_multipliers: HashMap::new(),
    });
    health.current = state.health.0;
    registry.set_component(pawn, health).unwrap();
    let mut reserve = AmmoReserve::new();
    reserve.credit("rounds", state.reserve);
    registry.set_component(pawn, reserve).unwrap();
    if let Some(weapon) = state.weapon {
        let weapon_id = registry.spawn(Transform::default());
        registry.set_component(weapon_id, weapon).unwrap();
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(weapon_id);
        inventory.active_slot = 0;
        registry.set_component(pawn, inventory).unwrap();
    }
    pawn
}

/// The scalar a replicated owner-private record carries: plain for health,
/// the value half of `[slot, value]` for weapon slots.
fn wire_scalar(value: &WireSlotValue) -> f32 {
    match value {
        WireSlotValue::Number(number) => *number,
        WireSlotValue::Array(pair) if pair.len() == 2 => pair[1],
        other => panic!("unexpected owner-private wire value {other:?}"),
    }
}

fn scalar(value: &SlotValue) -> f32 {
    match value {
        SlotValue::Number(number) => *number,
        SlotValue::Boolean(flag) => f32::from(u8::from(*flag)),
        other => panic!("unexpected per-player value {other:?}"),
    }
}

#[test]
fn every_catalog_per_player_slot_has_a_per_pawn_source_of_its_type() {
    let catalog = engine_state_catalog().expect("catalog builds");
    let marked: Vec<_> = catalog
        .entries()
        .iter()
        .filter(|entry| entry.is_per_player())
        .collect();
    for entry in &marked {
        let slot = PlayerSlot::from_name(entry.wire_name).unwrap_or_else(|| {
            panic!(
                "`{}` is marked per-player but the lookup has no per-pawn source",
                entry.wire_name
            )
        });
        let expected = match entry.value_type {
            EngineStateValueType::Number => postretro_foundation::IrType::Number,
            EngineStateValueType::Boolean => postretro_foundation::IrType::Bool,
            other => panic!("`{}` has non-projectable type {other:?}", entry.wire_name),
        };
        assert_eq!(slot.ir_type(), expected, "`{}` type", entry.wire_name);
    }
    for slot in PlayerSlot::ALL {
        assert!(
            marked.iter().any(|entry| entry.wire_name == slot.name()),
            "lookup slot `{}` is not marked per-player in the catalog",
            slot.name()
        );
    }
}

#[test]
fn two_pawns_read_and_replicate_their_own_value_for_every_per_player_slot() {
    let catalog = engine_state_catalog().expect("catalog builds");
    let host_table = SlotTable::new();
    let identity = ReplicatedSlotIdentity::default();
    let schema = ReplicatedSlotSchema::build(&host_table, &identity);

    for entry in catalog
        .entries()
        .iter()
        .filter(|entry| entry.is_per_player())
    {
        let slot = PlayerSlot::from_name(entry.wire_name).expect("source checked above");
        let mut registry = EntityRegistry::new();
        let mut owners = MovementOwners::new();
        let [state_a, state_b] = differing_states(slot);
        let pawn_a = spawn(&mut registry, state_a);
        let pawn_b = spawn(&mut registry, state_b);
        owners.set(pawn_a, CLIENT_A);
        owners.set(pawn_b, CLIENT_B);

        let value_a = player_slot_value(&registry, slot, pawn_a)
            .unwrap_or_else(|| panic!("`{}` has a value for pawn A", slot.name()));
        let value_b = player_slot_value(&registry, slot, pawn_b)
            .unwrap_or_else(|| panic!("`{}` has a value for pawn B", slot.name()));
        assert_ne!(
            value_a,
            value_b,
            "`{}` reads per pawn on the host",
            slot.name()
        );

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        host.register_client(CLIENT_B);
        host.ingest_frame(
            &host_table,
            &identity,
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        let id = schema
            .id_for(slot.name())
            .unwrap_or_else(|| panic!("`{}` replicates", slot.name()));
        for (client, value) in [(CLIENT_A, &value_a), (CLIENT_B, &value_b)] {
            let records = host.produce_for_client(client, 0).unwrap();
            let record = records
                .iter()
                .find(|record| record.slot_id == id.0)
                .unwrap_or_else(|| panic!("`{}` reaches client {client}", slot.name()));
            assert_eq!(
                wire_scalar(&record.value),
                scalar(value),
                "client {client}'s `{}` snapshot carries its own pawn's value",
                slot.name()
            );
        }
    }
}

#[test]
fn a_pawn_without_a_source_reads_absent_never_the_host_value() {
    let mut registry = EntityRegistry::new();
    let bare = registry.spawn(Transform::default());
    for slot in PlayerSlot::ALL {
        assert_eq!(
            player_slot_value(&registry, *slot, bare),
            None,
            "`{}` is absent for a pawn with no source",
            slot.name()
        );
    }

    // Regression: a pawn without `HealthComponent` replicated the host's own
    // global `player.health` to its owner.
    let mut host_table = SlotTable::new();
    host_table.get_mut("player.health").unwrap().value = Some(SlotValue::Number(77.0));
    let identity = ReplicatedSlotIdentity::default();
    let schema = ReplicatedSlotSchema::build(&host_table, &identity);
    let mut owners = MovementOwners::new();
    owners.set(bare, CLIENT_A);
    let mut host = HostStateReplication::new();
    host.register_client(CLIENT_A);
    host.ingest_frame(
        &host_table,
        &identity,
        &registry,
        &owners,
        &WeaponOwners::new(),
    );
    let id = schema.id_for("player.health").unwrap();
    let records = host.produce_for_client(CLIENT_A, 0).unwrap();
    assert!(
        records.iter().all(|record| record.slot_id != id.0),
        "no health record reaches the owner of a pawn without health"
    );
}
