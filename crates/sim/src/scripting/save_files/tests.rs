use std::collections::BTreeMap;

use postretro_combat_model::CarriedState;
use postretro_entities::AmmoReserve;

use super::*;
use crate::scripting::state_persistence::PersistedValue;

fn full_carried() -> CarriedState {
    let mut reserve = AmmoReserve::new();
    reserve.set_exact("shells", 13);
    reserve.set_exact("rockets", 0);
    let mut carried = CarriedState {
        health_current: Some(41.5),
        life_state: Some("downed".into()),
        reserve,
        active_slot: 2,
        layers: vec![("shield".into(), 30.25), ("armor".into(), 5.0)],
        ..CarriedState::default()
    };
    carried.wieldables[0] = Some("pistol".into());
    carried.wieldables[2] = Some("rocket_launcher".into());
    carried.magazines[0] = Some(4);
    carried.magazines[2] = Some(1);
    carried
}

fn seat(carried: &CarriedState) -> SeatRecord {
    SeatRecord {
        claim: "0123456789abcdef0123456789abcdef".into(),
        name: "Vex".into(),
        per_owner: BTreeMap::from([("kxp".into(), PersistedValue::Number(12.0))]),
        loadout: LoadoutRecord::from_carried(Some(carried)),
    }
}

#[test]
fn character_file_round_trips_every_loadout_field() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("characters/0.json");
    let carried = full_carried();
    let document = LedgerDocument {
        seats: vec![seat(&carried)],
        ..LedgerDocument::default()
    };

    write_document(&path, &document).unwrap();
    let read = read_document(&path).unwrap().expect("the file exists");

    assert_eq!(read, document);
    let mut warnings = Vec::new();
    assert_eq!(
        read.seats[0].loadout.to_carried(&mut warnings),
        Some(carried)
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn run_file_round_trips_level_checkpoint_slots_and_sentiment() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runs/1.json");
    let document = LedgerDocument {
        level: Some(LevelRecord {
            catalog_id: "campaign-test".into(),
            checkpoint: Some("bridge".into()),
        }),
        run: Some(RunRecord {
            character: "00112233445566778899aabbccddeeff".into(),
            saved_at: 1_700_000_000,
        }),
        slots: BTreeMap::from([("kgen".into(), PersistedValue::Boolean(true))]),
        faction_sentiment: BTreeMap::from([(
            "@postretro.player".into(),
            BTreeMap::from([("gangers".into(), -0.5)]),
        )]),
        ..LedgerDocument::default()
    };

    write_document(&path, &document).unwrap();

    assert_eq!(read_document(&path).unwrap(), Some(document));
}

#[test]
fn empty_loadout_restores_no_carry() {
    let mut warnings = Vec::new();
    assert_eq!(LoadoutRecord::default().to_carried(&mut warnings), None);
    assert!(LoadoutRecord::from_carried(None).is_empty());
}

#[test]
fn unknown_ledger_section_is_ignored_and_rewritten_byte_for_byte() {
    let section = r#"{"maps":{"e1m2":[1, 2.50, "x"]},  "note":"kept"}"#;
    let source = format!(r#"{{"version":1,"slots":{{"kworld":3}},"revisited":{section}}}"#);

    let document = LedgerDocument::from_slice(source.as_bytes()).unwrap();
    assert_eq!(
        document.unknown_sections().collect::<Vec<_>>(),
        ["revisited"]
    );
    assert_eq!(
        document.slots.get("kworld"),
        Some(&PersistedValue::Number(3.0))
    );

    let rewritten = String::from_utf8(document.to_vec().unwrap()).unwrap();
    assert!(
        rewritten.contains(&format!(r#""revisited": {section}"#)),
        "the unknown section keeps its bytes: {rewritten}"
    );
    assert_eq!(
        LedgerDocument::from_slice(rewritten.as_bytes()).unwrap(),
        document
    );
}

#[test]
fn unsupported_or_unparsable_documents_are_errors() {
    for (source, unsupported) in [
        (r#"{"version":999}"#, true),
        (r#"{"version":0}"#, true),
        (r#"{"version":1,"seats":"not a list"}"#, false),
        ("not json", false),
        (r#"{"slots":{}}"#, false),
    ] {
        let error = LedgerDocument::from_slice(source.as_bytes()).unwrap_err();
        assert_eq!(
            matches!(error, SaveFileError::UnsupportedVersion(_)),
            unsupported,
            "{source}: {error}"
        );
    }
}

#[test]
fn character_file_past_capacity_loads_in_range_part() {
    let mut record = LoadoutRecord::from_carried(Some(&full_carried()));
    record.wieldables.resize(12, None);
    record.wieldables.push(Some("railgun".into()));
    record.magazines.resize(11, None);
    record.magazines.push(Some(9));
    record.active_slot = 12;

    let mut warnings = Vec::new();
    let carried = record.to_carried(&mut warnings).unwrap();

    assert_eq!(carried.wieldables[0].as_deref(), Some("pistol"));
    assert_eq!(carried.wieldables[2].as_deref(), Some("rocket_launcher"));
    assert_eq!(carried.magazines[2], Some(1));
    assert_eq!(carried.active_slot, 0);
    assert_eq!(warnings.len(), 3, "{warnings:?}");
}

#[test]
fn save_killed_before_rename_keeps_previous_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("world.json");
    let before = LedgerDocument {
        slots: BTreeMap::from([("kworld".into(), PersistedValue::Number(1.0))]),
        ..LedgerDocument::default()
    };
    write_document(&path, &before).unwrap();
    let bytes = std::fs::read(&path).unwrap();

    let after = LedgerDocument {
        slots: BTreeMap::from([("kworld".into(), PersistedValue::Number(2.0))]),
        ..LedgerDocument::default()
    };
    // The process dies after the temporary file is written.
    drop(stage_document(&path, &after).unwrap());

    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(read_document(&path).unwrap(), Some(before));
}

fn world(value: f64) -> LedgerDocument {
    LedgerDocument {
        slots: BTreeMap::from([("kworld".into(), PersistedValue::Number(value))]),
        ..LedgerDocument::default()
    }
}

#[test]
fn slot_files_live_under_the_mod_directory() {
    let dir = SaveDir::new(std::path::Path::new("/data"), "dev");
    assert_eq!(
        dir.character_path(2),
        std::path::Path::new("/data/dev/characters/2.json")
    );
    assert_eq!(
        dir.run_path(0),
        std::path::Path::new("/data/dev/runs/0.json")
    );
    assert_eq!(
        dir.world_path(),
        std::path::Path::new("/data/dev/world.json")
    );
    assert_eq!(
        dir.selection_path(),
        std::path::Path::new("/data/dev/selection.json")
    );
}

#[test]
fn unreadable_slot_reads_occupied_and_not_readable() {
    let dir = tempfile::tempdir().unwrap();
    let saves = SaveDir::new(dir.path(), "dev");
    let future = saves.character_path(0);
    std::fs::create_dir_all(future.parent().unwrap()).unwrap();
    std::fs::write(&future, br#"{"version":999,"seats":[]}"#).unwrap();
    let garbled = saves.character_path(1);
    std::fs::write(&garbled, b"{ not json").unwrap();
    let older = saves.run_path(0);
    write_document(&older, &world(3.0)).unwrap();

    for path in [&future, &garbled] {
        let slot = read_slot(path);
        assert_eq!(slot, SlotFile::Unreadable);
        assert!(slot.is_occupied());
        assert!(slot.document().is_none());
    }
    assert_eq!(read_slot(&saves.character_path(2)), SlotFile::Empty);
    assert_eq!(read_slot(&older), SlotFile::Occupied(world(3.0)));

    delete_slot(&future).unwrap();
    assert_eq!(read_slot(&future), SlotFile::Empty);
}

#[test]
fn slots_are_independent_and_delete_touches_one_file() {
    let dir = tempfile::tempdir().unwrap();
    let saves = SaveDir::new(dir.path(), "dev");
    let paths = [
        saves.character_path(0),
        saves.character_path(1),
        saves.run_path(0),
        saves.run_path(1),
    ];
    for (index, path) in paths.iter().enumerate() {
        write_document(path, &world(index as f64)).unwrap();
    }
    let before: Vec<Vec<u8>> = paths
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();

    write_document(&paths[1], &world(10.0)).unwrap();
    delete_slot(&paths[2]).unwrap();

    assert_eq!(std::fs::read(&paths[0]).unwrap(), before[0]);
    assert_eq!(std::fs::read(&paths[3]).unwrap(), before[3]);
    assert_eq!(read_slot(&paths[2]), SlotFile::Empty);
    assert_eq!(read_slot(&paths[1]), SlotFile::Occupied(world(10.0)));
}

#[test]
fn selection_round_trips_and_defaults_to_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = SaveDir::new(dir.path(), "dev").selection_path();
    assert_eq!(read_selection(&path), Selection::default());

    let selection = Selection {
        character: 2,
        run: -1,
    };
    write_selection(&path, selection).unwrap();
    assert_eq!(read_selection(&path), selection);

    std::fs::write(&path, b"garbage").unwrap();
    assert_eq!(read_selection(&path), Selection::default());
}
