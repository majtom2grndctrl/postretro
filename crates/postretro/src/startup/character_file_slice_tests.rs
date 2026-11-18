// A carried loadout harvested in one process restores whole in a later one
// through a character file: the premise the character and run saves rest on.
// See: context/lib/scripting.md §5 (Durable State Store)

use std::collections::HashMap;

use glam::Vec3;
use postretro_combat_model::CarriedState;
use postretro_foundation::Seat;
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_sim::scripting::builtins::data_archetype::spawn_from_player_starts_with_carried_loadout;
use postretro_sim::scripting::map_entity::MapEntity;
use postretro_sim::scripting::save_files::{
    LedgerDocument, LoadoutRecord, SeatRecord, read_document, write_document,
};

use crate::coop_fixture_tests::initialize_dev_mod;
use crate::scripting::primitives::register_all;
use crate::startup::lifecycle::tests::{script_ctx, test_app};

fn placement() -> MapEntity {
    MapEntity {
        map_ordinal: None,
        classname: "player_spawn".into(),
        origin: Vec3::ZERO,
        angles: Vec3::ZERO,
        key_values: HashMap::new(),
        tags: Vec::new(),
    }
}

/// A fresh process: the dev mod initialized with a local seat table.
fn process() -> crate::App {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    let mut primitives = PrimitiveRegistry::new();
    register_all(&mut primitives, ctx.clone());
    initialize_dev_mod(&mut app, &primitives);
    ctx.data_registry.borrow_mut().recompose(&[]);
    ctx.registry.borrow_mut().clear_for_level_unload();
    app.session
        .as_mut()
        .unwrap()
        .seat_table
        .get_or_insert_with(crate::netcode::SeatTable::local_only);
    app
}

/// Install seat 0's pawn with its carry, bind it, then harvest it as a level
/// unload does, returning what seat 0 now carries.
fn install_and_harvest(app: &mut crate::App) -> CarriedState {
    let ctx = script_ctx(app);
    let descriptors = ctx.data_registry.borrow().entities.clone();
    let life_state_names = ctx.data_registry.borrow().life_state_names.clone();
    let seats = app.session.as_mut().unwrap().seat_table.as_mut().unwrap();
    let mut registry = ctx.registry.borrow_mut();
    let pawn = spawn_from_player_starts_with_carried_loadout(
        &[placement()],
        &descriptors,
        &mut registry,
        None,
        seats.carried_state(Seat(0)),
    )
    .expect("the dev mod's player spawns")
    .pawn;
    seats.bind_pawn(&mut registry, Seat(0), pawn);
    seats.harvest_bound_pawns(&registry, &life_state_names);
    registry.clear_for_level_unload();
    seats.clear_pawn_bindings_for_level_unload(&mut registry);
    seats
        .carried_state(Seat(0))
        .cloned()
        .expect("the harvest leaves seat 0 a carry")
}

#[test]
fn harvested_carry_restores_whole_in_a_later_process_through_a_character_file() {
    let mut first = process();
    let weapons: Vec<String> = script_ctx(&first)
        .data_registry
        .borrow()
        .entities
        .iter()
        .filter(|descriptor| descriptor.weapon.is_some())
        .filter_map(|descriptor| descriptor.canonical_name.clone())
        .take(2)
        .collect();
    assert_eq!(weapons.len(), 2, "fixture: the dev mod declares two weapons");
    let mut seeded = CarriedState {
        health_current: Some(37.0),
        active_slot: 3,
        ..CarriedState::default()
    };
    seeded.wieldables[1] = Some(weapons[0].clone());
    seeded.wieldables[3] = Some(weapons[1].clone());
    seeded.magazines[1] = Some(1);
    seeded.magazines[3] = Some(1);
    seeded.reserve.set_exact("shells", 19);
    seeded.reserve.set_exact("cells", 0);
    first
        .session
        .as_mut()
        .unwrap()
        .seat_table
        .as_mut()
        .unwrap()
        .set_carried_state(Seat(0), Some(seeded));
    let harvested = install_and_harvest(&mut first);
    assert_eq!(harvested.wieldables[3].as_deref(), Some(weapons[1].as_str()));
    assert_eq!(harvested.active_slot, 3);
    drop(first);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("characters/0.json");
    write_document(
        &path,
        &LedgerDocument {
            seats: vec![SeatRecord {
                claim: "0123456789abcdef0123456789abcdef".into(),
                name: "Vex".into(),
                loadout: LoadoutRecord::from_carried(Some(&harvested)),
                ..SeatRecord::default()
            }],
            ..LedgerDocument::default()
        },
    )
    .unwrap();

    let mut later = process();
    let document = read_document(&path).unwrap().expect("the character file exists");
    let mut warnings = Vec::new();
    let restored = document.seats[0].loadout.to_carried(&mut warnings);
    assert!(warnings.is_empty(), "{warnings:?}");
    later
        .session
        .as_mut()
        .unwrap()
        .seat_table
        .as_mut()
        .unwrap()
        .set_carried_state(Seat(0), restored);

    assert_eq!(install_and_harvest(&mut later), harvested);
}
