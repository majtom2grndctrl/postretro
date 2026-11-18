//! A host's seats serialize as one durable ledger document and restore whole.
use std::collections::{BTreeMap, BTreeSet};

use super::*;
use postretro_entities::slot_table::{
    ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotTable, SlotType, SlotValue,
};
use postretro_net::wire::{ConnectClaim, JoinSeedValue, PlayerClaimId};
use postretro_scripting_core::store_identity::StoreIdentityLedger;
use postretro_sim::scripting::save_files::{LedgerDocument, LoadoutRecord, SeatRecord};
use postretro_sim::scripting::state_persistence::{
    apply_join_seed, collect_seat_per_owner_values, decode_player_claim_id,
    encode_player_claim_id,
};

const XP: &str = "hero.xp";
const PERK: &str = "hero.perk";

fn per_owner_slot(slot_type: SlotType, default: SlotValue) -> SlotRecord {
    SlotRecord::new(SlotSchema {
        slot_type,
        default: Some(default),
        range: None,
        persist: true,
        readonly: false,
        ownership: SlotOwnership::Mod,
        network: ReplicationScope::None,
        per_owner: true,
        accumulate: None,
    })
}

fn store() -> (SlotTable, StoreIdentityLedger, BTreeSet<String>) {
    let mut table = SlotTable::new();
    table
        .insert(XP.into(), per_owner_slot(SlotType::Number, SlotValue::Number(0.0)))
        .unwrap();
    table
        .insert(
            PERK.into(),
            per_owner_slot(SlotType::String, SlotValue::String(String::new())),
        )
        .unwrap();
    let mut identity = StoreIdentityLedger::empty();
    identity.slots.insert(XP.into(), "kxp".into());
    identity.slots.insert(PERK.into(), "kperk".into());
    (table, identity, BTreeSet::from([XP.into(), PERK.into()]))
}

fn full_carry(weapon: &str, health: f32) -> CarriedState {
    let mut carried = CarriedState {
        health_current: Some(health),
        life_state: Some("alive".into()),
        active_slot: 4,
        layers: vec![("shield".into(), 12.5)],
        ..CarriedState::default()
    };
    carried.reserve.set_exact("shells", 7);
    carried.wieldables[4] = Some(weapon.into());
    carried.magazines[4] = Some(3);
    carried
}

/// The ledger's seat records, in seat order, for the seats listed.
fn document(
    seats: &SeatTable,
    table: &SlotTable,
    identity: &StoreIdentityLedger,
    committed: &BTreeSet<String>,
    listed: &[Seat],
) -> LedgerDocument {
    LedgerDocument {
        seats: listed
            .iter()
            .map(|&seat| {
                let (per_owner, warnings) =
                    collect_seat_per_owner_values(table, Some(identity), committed, seat);
                assert!(warnings.is_empty(), "{warnings:?}");
                let claim = seats.claim(seat).expect("every listed seat has a claim");
                SeatRecord {
                    claim: encode_player_claim_id(&claim.player_id),
                    name: claim.display_name.clone(),
                    per_owner,
                    loadout: LoadoutRecord::from_carried(seats.carried_state(seat)),
                }
            })
            .collect(),
        ..LedgerDocument::default()
    }
}

#[test]
fn ledger_document_restores_every_durable_seat_field_and_omits_live_state() {
    let (mut table, identity, committed) = store();
    let mut seats = SeatTable::from_test_session_id([3; 16]);
    let mut listed = Vec::new();
    for (client, (name, weapon)) in [("Vex", "pistol"), ("Ash", "launcher"), ("Kip", "rifle")]
        .into_iter()
        .enumerate()
    {
        let client = client as u64 + 10;
        let claim = ConnectClaim {
            player_id: PlayerClaimId([client as u8; 16]),
            display_name: name.into(),
        };
        let seat = seats.admit_or_reclaim(client, Some(claim), false).unwrap().seat;
        seats.set_carried_state(seat, Some(full_carry(weapon, 20.0 + client as f32)));
        let xp = table.get_mut(XP).unwrap();
        xp.set_per_seat_value(seat, SlotValue::Number(client as f32 * 100.0));
        let perk = table.get_mut(PERK).unwrap();
        perk.set_per_seat_value(seat, SlotValue::String(format!("{name}-perk")));
        listed.push(seat);
    }
    // Live state the document must not carry: a held seat and a placement.
    seats.placement_assignments.insert(listed[0], 2);
    seats.hold_disconnected_client(&mut postretro_entities::EntityRegistry::new(), 11);

    let written = document(&seats, &table, &identity, &committed, &listed);
    let bytes = written.to_vec().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for record in json["seats"].as_array().unwrap() {
        let keys: BTreeSet<&str> = record.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, BTreeSet::from(["claim", "name", "perOwner", "loadout"]));
    }

    let read = LedgerDocument::from_slice(&bytes).unwrap();
    let (mut fresh_table, _, _) = store();
    let mut fresh = SeatTable::from_test_session_id([4; 16]);
    let mut restored = Vec::new();
    for record in &read.seats {
        let mut warnings = Vec::new();
        let seat = fresh
            .restore_durable_seat(
                Some(ConnectClaim {
                    player_id: decode_player_claim_id(&record.claim).unwrap(),
                    display_name: record.name.clone(),
                }),
                record.loadout.to_carried(&mut warnings),
            )
            .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let seed: BTreeMap<String, JoinSeedValue> = record
            .per_owner
            .iter()
            .map(|(key, value)| (key.clone(), value.clone().try_into().unwrap()))
            .collect();
        let outcome = apply_join_seed(&mut fresh_table, Some(&identity), &committed, seat, seed);
        assert_eq!(outcome.applied, 2, "{:?}", outcome.warnings);
        assert_eq!(fresh.client_for_seat(seat), None, "no connection restores");
        assert_eq!(fresh.placement_for_seat(seat), None, "no placement restores");
        restored.push(seat);
    }

    assert_eq!(
        document(&fresh, &fresh_table, &identity, &committed, &restored),
        written
    );
}
