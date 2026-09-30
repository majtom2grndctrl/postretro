use super::*;
use crate::netcode::command_queue::{MovementOwners, WeaponOwners};
use crate::state_slots::{
    ClientStateApply, HostStateReplication, ReplicatedSlotIdentity, ReplicatedSlotSchema,
    compute_fingerprint,
};
use postretro_entities::components::inventory::Inventory;
use postretro_entities::data_descriptors::WeaponDescriptor;
use postretro_entities::{EntityId, EntityRegistry, ReplicationScope, SlotTable, Transform};
use postretro_net::state_slots::{
    RawStateSlotRecord, STATE_RECORD_KIND_FULL_BASELINE, SlotValueType,
};

const CLIENT_A: u64 = 1;
const CLIENT_B: u64 = 2;
const CLIENT_C: u64 = 3;

const RESOURCE_SLOTS: [&str; 5] = [
    HEAT_SLOT,
    OVERHEAT_AT_SLOT,
    OVERHEATED_SLOT,
    CELL_SLOT,
    CELL_CAPACITY_SLOT,
];

/// Engine-catalog slots need no mod identity ledger.
fn identity() -> ReplicatedSlotIdentity<'static> {
    ReplicatedSlotIdentity::default()
}

fn resource_weapon(resource: serde_json::Value) -> WeaponComponent {
    let descriptor: WeaponDescriptor = serde_json::from_value(serde_json::json!({
        "damage": 10.0,
        "range": 64.0,
        "fireRateMs": 100.0,
        "fireMode": "auto",
        "resolution": "hitscan",
        "resource": resource,
    }))
    .unwrap();
    WeaponComponent::from_descriptor(&descriptor)
}

fn heat_weapon(heat: f32, overheated: bool) -> WeaponComponent {
    let mut weapon = resource_weapon(serde_json::json!({
        "kind": "heat", "heatPerShot": 10.0, "overheatAt": 80.0, "coolPerSecond": 20.0
    }));
    let live = weapon.heat.as_mut().unwrap();
    live.heat = heat;
    live.overheated = overheated;
    weapon
}

fn cell_weapon(charge: f32) -> WeaponComponent {
    let mut weapon = resource_weapon(serde_json::json!({
        "kind": "cell", "capacity": 40.0, "costPerShot": 4.0, "regenPerSecond": 8.0
    }));
    weapon.cell.as_mut().unwrap().charge = charge;
    weapon
}

/// Spawn a pawn owned by `client` wielding `weapon` in host slot `slot`.
fn add_owned_pawn(
    registry: &mut EntityRegistry,
    owners: &mut MovementOwners,
    client: u64,
    slot: usize,
    weapon: WeaponComponent,
) -> (EntityId, EntityId) {
    let pawn = registry.spawn(Transform::default());
    let weapon_id = registry.spawn(Transform::default());
    registry.set_component(weapon_id, weapon).unwrap();
    let mut inventory = Inventory::default();
    inventory.wieldables[slot] = Some(weapon_id);
    inventory.active_slot = slot;
    registry.set_component(pawn, inventory).unwrap();
    owners.set(pawn, client);
    (pawn, weapon_id)
}

fn record_value<'a>(
    schema: &ReplicatedSlotSchema,
    records: &'a [RawStateSlotRecord],
    name: &str,
) -> Option<&'a WireSlotValue> {
    let id = schema
        .id_for(name)
        .unwrap_or_else(|| panic!("{name} replicated"));
    records
        .iter()
        .find(|record| record.slot_id == id.0)
        .map(|record| &record.value)
}

fn slot(table: &SlotTable, name: &str) -> Option<SlotValue> {
    table.get(name).unwrap().value.clone()
}

/// The client's heat and cell store values after one applied snapshot.
struct Applied([(&'static str, Option<SlotValue>); 5]);

fn applied_slot(applied: &Applied, name: &str) -> Option<SlotValue> {
    applied
        .0
        .iter()
        .find(|(slot, _)| *slot == name)
        .and_then(|(_, value)| value.clone())
}

fn sample<T>(slot: usize, value: T) -> Option<SlotSample<T>> {
    Some(SlotSample { slot, value })
}

fn present(slot: usize, value: f32) -> WireSlotValue {
    WireSlotValue::Array(vec![slot as f32, value])
}

fn absent(slot: usize) -> WireSlotValue {
    WireSlotValue::Array(vec![slot as f32])
}

#[test]
fn heat_and_cell_slots_use_slot_correlated_wire_shapes_and_the_kind_stays_local() {
    let table = SlotTable::new();
    let schema = ReplicatedSlotSchema::build(&table, &identity());
    for name in RESOURCE_SLOTS {
        let entry = schema
            .entries()
            .iter()
            .find(|entry| entry.name == name)
            .unwrap_or_else(|| panic!("{name} is replicated"));
        let expected = if name == OVERHEATED_SLOT {
            ReplicatedWireShape::WieldableSlotBoolean
        } else {
            ReplicatedWireShape::WieldableSlotOptionalNumber
        };
        assert_eq!(entry.wire_shape, expected, "{name}");
        assert_eq!(entry.scope, ReplicationScope::OwnerPrivatePlayer, "{name}");
        let descriptor = entry.to_net_descriptor();
        assert_eq!(descriptor.value_type, SlotValueType::Array, "{name}");
        assert_eq!(descriptor.range, None, "{name}");
    }
    assert_eq!(
        schema.id_for("player.weaponResource"),
        None,
        "every role publishes its own kind; a replicated one would be the host's"
    );

    // A peer built before these shapes, which read the values as plain slots
    // or replicated the kind, never matches this schema.
    let mut plain = schema.entries().to_vec();
    for entry in &mut plain {
        if RESOURCE_SLOTS.contains(&entry.name.as_str()) {
            entry.wire_shape = ReplicatedWireShape::Plain;
        }
    }
    assert_ne!(compute_fingerprint(&plain), *schema.fingerprint());
    let mut replicated_kind = SlotTable::new();
    replicated_kind
        .get_mut("player.weaponResource")
        .unwrap()
        .schema
        .network = ReplicationScope::OwnerPrivatePlayer;
    assert_ne!(
        ReplicatedSlotSchema::build(&replicated_kind, &identity()).fingerprint(),
        schema.fingerprint()
    );
}

#[test]
fn heat_and_cell_values_are_sourced_per_pawn_never_from_the_host_table() {
    let mut host_table = SlotTable::new();
    // The host player's own HUD values must never reach a remote owner.
    for name in [HEAT_SLOT, OVERHEAT_AT_SLOT, CELL_SLOT, CELL_CAPACITY_SLOT] {
        host_table.get_mut(name).unwrap().value = Some(SlotValue::Number(999.0));
    }
    host_table.get_mut(OVERHEATED_SLOT).unwrap().value = Some(SlotValue::Boolean(true));

    // A holds a latched heat gun in slot 0, B an unlatched one in slot 1, and
    // C a cell gun in slot 2.
    let mut registry = EntityRegistry::new();
    let mut owners = MovementOwners::new();
    add_owned_pawn(
        &mut registry,
        &mut owners,
        CLIENT_A,
        0,
        heat_weapon(30.0, true),
    );
    add_owned_pawn(
        &mut registry,
        &mut owners,
        CLIENT_B,
        1,
        heat_weapon(50.0, false),
    );
    add_owned_pawn(&mut registry, &mut owners, CLIENT_C, 2, cell_weapon(12.0));

    let schema = ReplicatedSlotSchema::build(&host_table, &identity());
    let mut host = HostStateReplication::new();
    for client in [CLIENT_A, CLIENT_B, CLIENT_C] {
        host.register_client(client);
    }
    let fingerprint = host.fingerprint(&host_table, &identity());
    host.ingest_frame(
        &host_table,
        &identity(),
        &registry,
        &owners,
        &WeaponOwners::new(),
    );

    // Wire order follows RESOURCE_SLOTS.
    let expected_wire = [
        (
            CLIENT_A,
            [
                present(0, 30.0),
                present(0, 80.0),
                present(0, 1.0),
                absent(0),
                absent(0),
            ],
        ),
        (
            CLIENT_B,
            [
                present(1, 50.0),
                present(1, 80.0),
                present(1, 0.0),
                absent(1),
                absent(1),
            ],
        ),
        (
            CLIENT_C,
            [
                absent(2),
                absent(2),
                present(2, 0.0),
                present(2, 12.0),
                present(2, 40.0),
            ],
        ),
    ];
    for (client, expected) in expected_wire {
        let records = host.produce_for_client(client, 0).unwrap();
        for (name, wire) in RESOURCE_SLOTS.into_iter().zip(expected) {
            assert_eq!(
                record_value(&schema, &records, name),
                Some(&wire),
                "client {client}: {name}"
            );
        }

        let mut client_table = SlotTable::new();
        let mut apply = ClientStateApply::new();
        let outcome =
            apply.apply_snapshot_state(&mut client_table, &identity(), 0, &fingerprint, &records);
        assert_eq!(
            outcome.slot_baselines.len(),
            records.len(),
            "client {client}"
        );
        let projection = *apply.weapon_projection();
        let (heat, overheat_at, overheated, cell, capacity, host_slot) = match client {
            CLIENT_A => (Some(30.0), Some(80.0), true, None, None, 0),
            CLIENT_B => (Some(50.0), Some(80.0), false, None, None, 1),
            _ => (None, None, false, Some(12.0), Some(40.0), 2),
        };
        for (name, value) in [
            (HEAT_SLOT, heat),
            (OVERHEAT_AT_SLOT, overheat_at),
            (CELL_SLOT, cell),
            (CELL_CAPACITY_SLOT, capacity),
        ] {
            assert_eq!(
                slot(&client_table, name),
                value.map(SlotValue::Number),
                "client {client}: {name}"
            );
        }
        assert_eq!(
            slot(&client_table, OVERHEATED_SLOT),
            Some(SlotValue::Boolean(overheated)),
            "client {client}"
        );
        assert_eq!(projection.heat, sample(host_slot, heat));
        assert_eq!(projection.overheat_at, sample(host_slot, overheat_at));
        assert_eq!(projection.overheated, sample(host_slot, overheated));
        assert_eq!(projection.cell, sample(host_slot, cell));
        assert_eq!(projection.cell_capacity, sample(host_slot, capacity));
    }
}

#[test]
fn a_kind_switch_clears_the_outgoing_values_on_the_client() {
    let table = SlotTable::new();
    let schema = ReplicatedSlotSchema::build(&table, &identity());
    let mut registry = EntityRegistry::new();
    let mut owners = MovementOwners::new();
    let (pawn, _) = add_owned_pawn(
        &mut registry,
        &mut owners,
        CLIENT_A,
        0,
        heat_weapon(80.0, true),
    );
    let mut host = HostStateReplication::new();
    host.register_client(CLIENT_A);
    let fingerprint = host.fingerprint(&table, &identity());
    let mut client_table = SlotTable::new();
    let mut client = ClientStateApply::new();
    let mut step = |registry: &EntityRegistry, sequence: u32| {
        host.ingest_frame(
            &table,
            &identity(),
            registry,
            &owners,
            &WeaponOwners::new(),
        );
        let records = host.produce_for_client(CLIENT_A, sequence).unwrap();
        let outcome = client.apply_snapshot_state(
            &mut client_table,
            &identity(),
            sequence,
            &fingerprint,
            &records,
        );
        assert_eq!(outcome.slot_baselines.len(), records.len());
        host.apply_ack(CLIENT_A, sequence, &outcome.slot_baselines, None);
        let applied = Applied(
            RESOURCE_SLOTS.map(|name| (name, client_table.get(name).unwrap().value.clone())),
        );
        (records, *client.weapon_projection(), applied)
    };
    let (_, projection, applied) = step(&registry, 0);
    assert_eq!(projection.overheated, sample(0, true));
    assert_eq!(applied_slot(&applied, HEAT_SLOT), Some(SlotValue::Number(80.0)));

    // The host switches to a cell gun in slot 1.
    let cell_id = registry.spawn(Transform::default());
    registry.set_component(cell_id, cell_weapon(20.0)).unwrap();
    let mut inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
    inventory.wieldables[1] = Some(cell_id);
    inventory.active_slot = 1;
    registry.set_component(pawn, inventory).unwrap();
    let (records, projection, applied) = step(&registry, 1);
    assert_eq!(record_value(&schema, &records, HEAT_SLOT), Some(&absent(1)));
    assert_eq!(
        record_value(&schema, &records, OVERHEATED_SLOT),
        Some(&present(1, 0.0)),
        "the latch reads false for a weapon without heat"
    );
    assert_eq!(applied_slot(&applied, HEAT_SLOT), None, "cleared, not left at 80");
    assert_eq!(applied_slot(&applied, OVERHEAT_AT_SLOT), None);
    assert_eq!(
        applied_slot(&applied, OVERHEATED_SLOT),
        Some(SlotValue::Boolean(false))
    );
    assert_eq!(applied_slot(&applied, CELL_SLOT), Some(SlotValue::Number(20.0)));
    assert_eq!(projection.heat, sample(1, None));
    assert_eq!(projection.overheated, sample(1, false));
    assert_eq!(projection.cell, sample(1, Some(20.0)));

    // An acknowledged absence is not resent.
    let (steady, _, _) = step(&registry, 2);
    assert_eq!(record_value(&schema, &steady, HEAT_SLOT), None);
}

#[test]
fn no_live_active_weapon_sends_no_heat_or_cell_value() {
    let weapon = heat_weapon(10.0, false);
    let empty_slot = ResourceSlotProjection::of(Some(1), None);
    let no_inventory = ResourceSlotProjection::of(None, Some(&weapon));
    for name in RESOURCE_SLOTS {
        assert_eq!(empty_slot.wire_sample(name), Some(None), "{name}");
        assert_eq!(no_inventory.wire_sample(name), Some(None), "{name}");
    }
    assert_eq!(
        empty_slot.wire_sample("player.ammo"),
        None,
        "not a resource slot"
    );

    // The same through the host: the pawn's active slot holds nothing.
    let table = SlotTable::new();
    let mut registry = EntityRegistry::new();
    let mut owners = MovementOwners::new();
    let (pawn, _) = add_owned_pawn(&mut registry, &mut owners, CLIENT_A, 0, weapon);
    let mut inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
    inventory.active_slot = 1;
    registry.set_component(pawn, inventory).unwrap();
    let schema = ReplicatedSlotSchema::build(&table, &identity());
    let mut host = HostStateReplication::new();
    host.register_client(CLIENT_A);
    host.ingest_frame(
        &table,
        &identity(),
        &registry,
        &owners,
        &WeaponOwners::new(),
    );
    let records = host.produce_for_client(CLIENT_A, 0).unwrap_or_default();
    for name in RESOURCE_SLOTS {
        assert_eq!(record_value(&schema, &records, name), None, "{name}");
    }
}

#[test]
fn a_malformed_heat_or_cell_sample_rejects_the_batch() {
    let table = SlotTable::new();
    let schema = ReplicatedSlotSchema::build(&table, &identity());
    for (name, value) in [
        // The latch has no absence: a weapon without heat reads false.
        (OVERHEATED_SLOT, absent(0)),
        (OVERHEATED_SLOT, present(0, 0.5)),
        (HEAT_SLOT, WireSlotValue::Number(10.0)),
        (CELL_SLOT, present(0, -1.0)),
    ] {
        let mut client_table = SlotTable::new();
        let record = RawStateSlotRecord {
            slot_id: schema.id_for(name).unwrap().0,
            kind: STATE_RECORD_KIND_FULL_BASELINE,
            has_baseline_ref: false,
            baseline_ref: 0,
            baseline_id: 1,
            value: value.clone(),
        };
        let outcome = ClientStateApply::new().apply_snapshot_state(
            &mut client_table,
            &identity(),
            0,
            schema.fingerprint(),
            &[record],
        );
        assert!(
            outcome.slot_baselines.is_empty(),
            "{name} {value:?} is rejected"
        );
    }
}
