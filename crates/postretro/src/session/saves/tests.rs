// Save slots at boot: migration from `state.json`, implicit slots, the stored
// selection and the `saves.*` rows.

use std::path::Path;

use postretro_entities::data_registry::{DEFAULT_ENEMY_FACTION_INDEX, PLAYER_FACTION_INDEX};
use postretro_entities::slot_table::SlotValue;
use postretro_foundation::Seat;
use postretro_net::wire::PlayerClaimId;
use postretro_scripting_core::runtime::SavesManifest;
use postretro_sim::scripting::save_files::{
    SaveDir, Selection, SlotFile, read_document, read_selection, write_document, write_selection,
};
use postretro_sim::scripting::state_persistence::{
    encode_player_claim_id, load_persisted_state, overlay_persisted_faction_sentiment,
    overlay_persisted_state,
};

use super::test_fixtures::*;
use crate::App;
use crate::startup::lifecycle::tests::script_ctx;

fn sentiment(app: &App) -> Option<f32> {
    script_ctx(app).faction_sentiment.borrow().get(
        PLAYER_FACTION_INDEX as f32,
        DEFAULT_ENEMY_FACTION_INDEX as f32,
    )
}

/// A version-4 `state.json`: a world global, a now run-scoped global, this
/// player's and another player's per-owner rows, and faction sentiment.
fn write_v4_state(data_dir: &Path) -> std::path::PathBuf {
    let path = data_dir.join(MOD_ID).join("state.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let player = encode_player_claim_id(&PlayerClaimId(PLAYER));
    let other = encode_player_claim_id(&PlayerClaimId(OTHER));
    std::fs::write(
        &path,
        format!(
            r#"{{"version":4,"slots":{{"k-score":42,"k-gen":5}},
                "per_owner":{{"k-xp":{{"{player}":7,"{other}":99}}}},
                "faction_sentiment":{{"@postretro.player":{{"@postretro.default-enemy":0.75}}}}}}"#
        ),
    )
    .unwrap();
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    path
}

const SAVES: Option<SavesManifest> = Some(SavesManifest {
    character_slots: 2,
    run_slots: 2,
});

#[test]
fn v4_state_migrates_to_character_and_world_once() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = write_v4_state(dir.path());
    // The values the pre-saves loader restores from this file (unchanged
    // since acd5eae8): overlay it on a fresh store the way boot did.
    let expected = app_with_store(dir.path());
    {
        let session = expected.session.as_ref().unwrap();
        let state = load_persisted_state(&legacy).unwrap().unwrap();
        let ctx = script_ctx(&expected);
        let warnings = overlay_persisted_state(
            &mut ctx.slot_table.borrow_mut(),
            &state,
            session.scripting.script_runtime.store_identity(),
            session.scripting.script_runtime.committed_store_slots(),
            Some(PLAYER),
            Seat(0),
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        let factions = ctx.data_registry.borrow();
        overlay_persisted_faction_sentiment(
            &mut ctx.faction_sentiment.borrow_mut(),
            &factions.factions,
            &state,
        );
    }
    assert_eq!(number(&expected, SCORE), Some(SlotValue::Number(42.0)));
    assert_eq!(xp(&expected), Some(SlotValue::Number(7.0)));
    assert_eq!(sentiment(&expected), Some(0.75));
    let legacy_bytes = std::fs::read(&legacy).unwrap();

    let mut app = app_with_store(dir.path());
    boot(&mut app, SAVES);

    assert_eq!(number(&app, SCORE), number(&expected, SCORE));
    assert_eq!(xp(&app), xp(&expected));
    // A mod with a `saves` block drops sentiment and run-scoped globals.
    assert_eq!(sentiment(&app), None);
    assert_eq!(number(&app, GENERATOR), Some(SlotValue::Number(0.0)));
    let saves_dir = SaveDir::new(dir.path(), MOD_ID);
    let world = read_document(&saves_dir.world_path()).unwrap().unwrap();
    assert!(
        !world.slots.contains_key("k-gen"),
        "never in the world file"
    );
    let character = read_document(&saves_dir.character_path(0))
        .unwrap()
        .unwrap();
    assert_eq!(
        character.seats[0].claim,
        encode_player_claim_id(&PlayerClaimId(PLAYER))
    );
    assert!(character.seats[0].loadout.is_empty());
    assert_eq!(read_document(&saves_dir.run_path(0)).unwrap(), None);
    let session = app.session.as_ref().unwrap();
    assert_eq!(
        session.saves.selection(),
        Selection {
            character: 0,
            run: -1
        }
    );
    assert_eq!(
        session.seat_table.as_ref().unwrap().carried_state(Seat(0)),
        None
    );
    assert_eq!(
        std::fs::read(&legacy).unwrap(),
        legacy_bytes,
        "the source stays"
    );

    // A second boot reads the new files and never the source again.
    std::fs::write(&legacy, br#"{"version":4,"slots":{"k-score":1000}}"#).unwrap();
    let mut again = app_with_store(dir.path());
    boot(&mut again, SAVES);
    assert_eq!(number(&again, SCORE), Some(SlotValue::Number(42.0)));
    assert_eq!(xp(&again), Some(SlotValue::Number(7.0)));
}

#[test]
fn migration_interrupted_after_each_write_converges() {
    let files = |dir: &Path| -> Vec<Option<Vec<u8>>> {
        let saves = SaveDir::new(dir, MOD_ID);
        [
            saves.character_path(0),
            saves.run_path(0),
            saves.selection_path(),
            saves.world_path(),
        ]
        .iter()
        .map(|path| std::fs::read(path).ok())
        .collect()
    };
    let reference = tempfile::tempdir().unwrap();
    write_v4_state(reference.path());
    boot(&mut app_with_store(reference.path()), None);
    let complete = files(reference.path());
    assert!(complete.iter().all(Option::is_some), "fixture: every file");

    // Writes land in this order; the world file, last, ends migration.
    for written in 1..=3 {
        let dir = tempfile::tempdir().unwrap();
        write_v4_state(dir.path());
        let saves = SaveDir::new(dir.path(), MOD_ID);
        for (path, bytes) in [
            saves.character_path(0),
            saves.run_path(0),
            saves.selection_path(),
        ]
        .iter()
        .zip(&complete)
        .take(written)
        {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes.as_ref().unwrap()).unwrap();
        }
        boot(&mut app_with_store(dir.path()), None);
        assert_eq!(files(dir.path()), complete, "killed after {written} writes");
    }
}

#[test]
fn mod_without_saves_block_gets_one_selected_active_run_holding_sentiment() {
    let dir = tempfile::tempdir().unwrap();
    write_v4_state(dir.path());
    let mut app = app_with_store(dir.path());
    boot(&mut app, None);

    let session = app.session.as_ref().unwrap();
    assert_eq!(
        session.saves.selection(),
        Selection {
            character: 0,
            run: 0
        }
    );
    assert!(session.saves.run_active());
    // The migrated run-scoped global and sentiment land in the implicit run.
    let run = read_document(&SaveDir::new(dir.path(), MOD_ID).run_path(0))
        .unwrap()
        .unwrap();
    assert!(run.slots.contains_key("k-gen"));
    assert_eq!(number(&app, GENERATOR), Some(SlotValue::Number(5.0)));
    assert_eq!(sentiment(&app), Some(0.75));
    assert_eq!(xp(&app), Some(SlotValue::Number(7.0)));

    // Without anything to migrate, the slots are created and selected.
    let fresh = tempfile::tempdir().unwrap();
    let mut app = app_with_store(fresh.path());
    boot(&mut app, None);
    let session = app.session.as_ref().unwrap();
    assert_eq!(
        session.saves.selection(),
        Selection {
            character: 0,
            run: 0
        }
    );
    assert!(session.saves.run_active());
    assert_eq!(session.saves.selected_character_id(), Some(PLAYER));
}

#[test]
fn selection_restores_from_save_dir_and_rejects_bad_slots() {
    let capture = postretro_test_log_capture::LogCapture::start();
    let dir = tempfile::tempdir().unwrap();
    let saves = SaveDir::new(dir.path(), MOD_ID);
    write_document(&saves.world_path(), &Default::default()).unwrap();
    let character = postretro_sim::scripting::save_files::LedgerDocument {
        seats: vec![postretro_sim::scripting::save_files::SeatRecord {
            claim: encode_player_claim_id(&PlayerClaimId(OTHER)),
            name: "Vex".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    write_document(&saves.character_path(0), &character).unwrap();
    std::fs::write(saves.character_path(1), b"{ not json").unwrap();

    write_selection(
        &saves.selection_path(),
        Selection {
            character: 0,
            run: 1,
        },
    )
    .unwrap();
    let mut app = app_with_store(dir.path());
    boot(&mut app, SAVES);
    let session = app.session.as_ref().unwrap();
    assert_eq!(
        session.saves.selection(),
        Selection {
            character: 0,
            run: -1
        }
    );
    assert_eq!(session.saves.selected_character_name(), "Vex");
    capture.assert_logged(log::Level::Warn, "stored run selection 1");

    for stored in [1, 5] {
        write_selection(
            &saves.selection_path(),
            Selection {
                character: stored,
                run: -1,
            },
        )
        .unwrap();
        let mut app = app_with_store(dir.path());
        boot(&mut app, SAVES);
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.saves.selection().character, -1, "slot {stored}");
        assert!(matches!(
            session.saves.character_slots()[1],
            SlotFile::Unreadable
        ));
    }
    // The selection lives in the mod's save directory, not player options.
    assert_eq!(read_selection(&saves.selection_path()).character, 5);
}

#[test]
fn saves_rows_match_declared_count() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_store(dir.path());
    boot(
        &mut app,
        Some(SavesManifest {
            character_slots: 3,
            run_slots: 2,
        }),
    );
    let table = script_ctx(&app).slot_table.clone();
    let table = table.borrow();
    let read = |name: String| table.get(&name).and_then(|slot| slot.value.clone());
    for row in 0..16 {
        assert_eq!(
            read(format!("saves.characters.{row}.present")),
            Some(SlotValue::Boolean(row < 3)),
            "character row {row}"
        );
        assert_eq!(
            read(format!("saves.runs.{row}.present")),
            Some(SlotValue::Boolean(row < 2)),
            "run row {row}"
        );
        assert_eq!(
            read(format!("saves.characters.{row}.occupied")),
            Some(SlotValue::Boolean(false))
        );
    }
    assert_eq!(
        read("saves.character".into()),
        Some(SlotValue::Number(-1.0))
    );
    assert_eq!(read("saves.run".into()), Some(SlotValue::Number(-1.0)));
}
