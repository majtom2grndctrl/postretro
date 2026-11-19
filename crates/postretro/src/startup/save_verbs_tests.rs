// The save verbs at the app drain: the frontend gate, characters, runs,
// Continue and New game, and what each leaves on disk and in the store.
// See: context/lib/scripting.md §5 (Durable State Store)

use postretro_combat_model::CarriedState;
use postretro_entities::SystemReactionCommand;
use postretro_entities::reactions::system_commands::{FireOrigin, MachineSource};
use postretro_entities::slot_table::SlotValue;
use postretro_foundation::{ModMapEntry, Seat};
use postretro_scripting_core::runtime::SavesManifest;
use postretro_sim::scripting::save_files::{
    LedgerDocument, SaveDir, Selection, SlotFile, read_document, read_selection,
};

use crate::App;
use crate::session::saves::test_fixtures::{
    GENERATOR, MOD_ID, SCORE, app_with_store, boot, number, present_frontend, xp,
};
use crate::startup::lifecycle::tests::script_ctx;

const SAVES: Option<SavesManifest> = Some(SavesManifest {
    character_slots: 2,
    run_slots: 2,
});

fn origin() -> FireOrigin {
    FireOrigin::EveryMachine(MachineSource::OwnState)
}

fn src() -> String {
    "named:test".into()
}

fn select(index: usize) -> SystemReactionCommand {
    SystemReactionCommand::SelectCharacter {
        index,
        dispatch_source: src(),
        origin: origin(),
    }
}
fn create(index: usize) -> SystemReactionCommand {
    SystemReactionCommand::CreateCharacter {
        index,
        dispatch_source: src(),
        origin: origin(),
    }
}
fn delete_character(index: usize) -> SystemReactionCommand {
    SystemReactionCommand::DeleteCharacter {
        index,
        dispatch_source: src(),
        origin: origin(),
    }
}
fn continue_run(index: usize) -> SystemReactionCommand {
    SystemReactionCommand::ContinueRun {
        index,
        dispatch_source: src(),
        origin: origin(),
    }
}
fn new_run(index: usize, map: &str) -> SystemReactionCommand {
    SystemReactionCommand::NewRun {
        index,
        map: map.into(),
        dispatch_source: src(),
        origin: origin(),
    }
}
fn delete_run(index: usize) -> SystemReactionCommand {
    SystemReactionCommand::DeleteRun {
        index,
        dispatch_source: src(),
        origin: origin(),
    }
}

/// A booted mod with a `saves` block of two characters and two runs, its
/// catalog holding `next` and `second`, standing at its frontend.
fn frontend(dir: &std::path::Path) -> App {
    let mut app = app_with_store(dir);
    boot(&mut app, SAVES);
    {
        let ctx = script_ctx(&app);
        let mut data = ctx.data_registry.borrow_mut();
        for id in ["next", "second"] {
            data.maps.push(ModMapEntry {
                id: id.into(),
                path: format!("maps/{id}-never-read.prl"),
                name: id.to_uppercase(),
                tags: Vec::new(),
                loading_tree: Vec::new(),
            });
        }
    }
    present_frontend(&mut app);
    assert!(app.at_frontend());
    app
}

fn name_entry(app: &App, name: &str) {
    script_ctx(app)
        .slot_table
        .borrow_mut()
        .get_mut("ui.textEntry")
        .unwrap()
        .write_value(Some(SlotValue::String(name.into())));
}

/// Create a character named `name` in `index`, selecting it.
fn character(app: &mut App, index: usize, name: &str) {
    name_entry(app, name);
    app.handle_save_verb(create(index));
    assert_eq!(
        app.session.as_ref().unwrap().saves.selection().character,
        index as i32
    );
}

fn set(app: &App, name: &str, value: f32) {
    script_ctx(app)
        .slot_table
        .borrow_mut()
        .get_mut(name)
        .unwrap()
        .write_value(Some(SlotValue::Number(value)));
}

fn set_xp(app: &App, value: f32) {
    script_ctx(app)
        .slot_table
        .borrow_mut()
        .get_mut(crate::session::saves::test_fixtures::XP)
        .unwrap()
        .set_per_seat_value(Seat(0), SlotValue::Number(value));
}

fn carry(app: &App) -> Option<CarriedState> {
    app.session
        .as_ref()
        .unwrap()
        .seat_table
        .as_ref()
        .unwrap()
        .carried_state(Seat(0))
        .cloned()
}

fn set_carry(app: &mut App, shells: u32) {
    let mut carried = CarriedState::default();
    carried.reserve.set_exact("shells", shells);
    app.session
        .as_mut()
        .unwrap()
        .seat_table
        .as_mut()
        .unwrap()
        .set_carried_state(Seat(0), Some(carried));
}

/// Save the selected character and world, as a save point does.
fn save(app: &mut App) {
    let session = app.session.as_mut().unwrap();
    let _ = crate::session::progress_save::save_world_and_character(session);
}

/// Back at the frontend after a run's level was queued: the queued load
/// begins under the menu, then the frontend entry ends it.
fn back_to_frontend(app: &mut App) {
    app.navigate_to_frontend();
    app.drain_level_requests();
    app.drain_level_requests();
    assert!(app.at_frontend());
}

fn saves(dir: &std::path::Path) -> SaveDir {
    SaveDir::new(dir, MOD_ID)
}

fn run_file(dir: &std::path::Path, index: usize) -> Option<LedgerDocument> {
    read_document(&saves(dir).run_path(index)).unwrap()
}

fn selection(app: &App) -> Selection {
    app.session.as_ref().unwrap().saves.selection()
}

#[test]
fn create_character_refuses_occupied_slot() {
    let capture = postretro_test_log_capture::LogCapture::start();
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    let before = std::fs::read(saves(dir.path()).character_path(0)).unwrap();

    name_entry(&app, "Ash");
    app.handle_save_verb(create(0));

    capture.assert_logged(log::Level::Warn, "createCharacter refused");
    assert_eq!(
        std::fs::read(saves(dir.path()).character_path(0)).unwrap(),
        before
    );
}

#[test]
fn select_character_replaces_per_owner_values() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    set_xp(&app, 5.0);
    save(&mut app);
    character(&mut app, 1, "Ash");
    assert_eq!(
        xp(&app),
        Some(SlotValue::Number(0.0)),
        "B starts at the default"
    );
    set_xp(&app, 2.0);
    save(&mut app);

    app.handle_save_verb(select(0));
    assert_eq!(xp(&app), Some(SlotValue::Number(5.0)));
    app.handle_save_verb(select(1));
    assert_eq!(xp(&app), Some(SlotValue::Number(2.0)));
}

#[test]
fn deleting_selection_stores_minus_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    app.handle_save_verb(new_run(1, "next"));
    back_to_frontend(&mut app);
    assert_eq!(
        selection(&app),
        Selection {
            character: 0,
            run: 1
        }
    );

    app.handle_save_verb(delete_run(1));
    assert_eq!(selection(&app).run, -1);
    app.handle_save_verb(delete_character(0));
    assert_eq!(selection(&app), Selection::default());
    assert_eq!(
        read_selection(&saves(dir.path()).selection_path()),
        Selection::default()
    );
    let table = script_ctx(&app).slot_table.clone();
    assert_eq!(
        table.borrow().get("saves.character").unwrap().value,
        Some(SlotValue::Number(-1.0))
    );

    let mut later = app_with_store(dir.path());
    boot(&mut later, SAVES);
    assert_eq!(selection(&later), Selection::default());
}

#[test]
fn deleted_selection_spawns_empty_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    set_carry(&mut app, 9);
    save(&mut app);

    app.handle_save_verb(delete_character(0));

    assert_eq!(carry(&app), None);
    save(&mut app);
    assert!(!saves(dir.path()).character_path(0).exists());
}

#[test]
fn new_run_slot_ownership() {
    let capture = postretro_test_log_capture::LogCapture::start();
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    app.handle_save_verb(new_run(0, "next"));
    back_to_frontend(&mut app);
    character(&mut app, 1, "Ash");
    let before = std::fs::read(saves(dir.path()).run_path(0)).unwrap();

    app.handle_save_verb(new_run(0, "second"));
    capture.assert_logged(log::Level::Warn, "another character's run");
    assert_eq!(
        std::fs::read(saves(dir.path()).run_path(0)).unwrap(),
        before
    );

    app.handle_save_verb(new_run(1, "second"));
    assert_eq!(
        run_file(dir.path(), 1).unwrap().level.unwrap().catalog_id,
        "second"
    );
    back_to_frontend(&mut app);
    // Into the selected character's own run: permitted.
    app.handle_save_verb(new_run(1, "next"));
    assert_eq!(
        run_file(dir.path(), 1).unwrap().level.unwrap().catalog_id,
        "next"
    );
}

#[test]
fn new_run_resets_run_scope_and_keeps_world_and_character() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    set(&app, SCORE, 7.0);
    set(&app, GENERATOR, 5.0);
    set_xp(&app, 3.0);
    set_carry(&mut app, 9);
    save(&mut app);

    app.handle_save_verb(new_run(0, "next"));

    assert_eq!(number(&app, SCORE), Some(SlotValue::Number(7.0)));
    assert_eq!(number(&app, GENERATOR), Some(SlotValue::Number(0.0)));
    assert_eq!(xp(&app), Some(SlotValue::Number(3.0)));
    assert_eq!(carry(&app).unwrap().reserve.available("shells"), 9);
    let run = run_file(dir.path(), 0).expect("written before its load begins");
    assert_eq!(run.level.unwrap().catalog_id, "next");
    assert!(
        !run.slots.contains_key("k-gen")
            || run.slots["k-gen"]
                == postretro_sim::scripting::state_persistence::PersistedValue::Number(0.0)
    );
    assert!(app.level_load_in_flight() || !app.level_requests.is_empty());
}

#[test]
fn continue_installs_run_with_its_character() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    set_carry(&mut app, 9);
    save(&mut app);
    app.handle_save_verb(new_run(0, "next"));
    set(&app, GENERATOR, 5.0);
    // The run reaches `second` with the generator on.
    app.drain_level_requests();
    app.enqueue_level_request(super::LevelRequest::Load(super::LevelSource::Catalog(
        "second".into(),
    )));
    app.drain_level_requests();
    back_to_frontend(&mut app);
    character(&mut app, 1, "Ash");
    assert_eq!(number(&app, GENERATOR), Some(SlotValue::Number(0.0)));

    app.handle_save_verb(continue_run(0));

    assert_eq!(
        selection(&app),
        Selection {
            character: 0,
            run: 0
        }
    );
    assert_eq!(number(&app, GENERATOR), Some(SlotValue::Number(5.0)));
    assert_eq!(carry(&app).unwrap().reserve.available("shells"), 9);
    assert!(
        app.level_requests.iter().any(|request| matches!(
            request,
            super::LevelRequest::Load(super::LevelSource::Catalog(id)) if id == "second"
        )),
        "Continue queues the run's level"
    );
}

#[test]
fn continue_refuses_orphaned_or_unknown_level_runs() {
    let capture = postretro_test_log_capture::LogCapture::start();
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    app.handle_save_verb(new_run(0, "next"));
    back_to_frontend(&mut app);
    app.handle_save_verb(delete_character(0));
    let before = selection(&app);

    app.handle_save_verb(continue_run(0));
    capture.assert_logged(log::Level::Warn, "character was deleted");
    assert_eq!(selection(&app), before, "refused, changing nothing");
    // A new character in the same slot is a different character.
    character(&mut app, 0, "Vex again");
    let before = selection(&app);
    app.handle_save_verb(continue_run(0));
    assert!(app.at_frontend(), "still refused");
    assert_eq!(selection(&app), before);

    // A run whose level the mod no longer defines.
    app.handle_save_verb(new_run(1, "second"));
    back_to_frontend(&mut app);
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .maps
        .retain(|entry| entry.id != "second");
    app.handle_save_verb(continue_run(1));
    capture.assert_logged(log::Level::Warn, "not in this mod's catalog");
    assert!(app.at_frontend());

    app.handle_save_verb(delete_run(0));
    assert_eq!(run_file(dir.path(), 0), None, "deleting it is permitted");
}

#[test]
fn verb_after_queued_run_load_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    app.handle_save_verb(new_run(0, "next"));
    back_to_frontend(&mut app);
    character(&mut app, 1, "Ash");

    // Continue selects Vex, the run's character; the select queued after it
    // in the same reaction is refused.
    app.handle_save_verb(continue_run(0));
    app.handle_save_verb(select(1));
    assert_eq!(
        selection(&app).character,
        0,
        "the select after Continue changed nothing"
    );

    back_to_frontend(&mut app);
    app.handle_save_verb(new_run(0, "second"));
    app.handle_save_verb(delete_run(0));
    assert!(
        run_file(dir.path(), 0).is_some(),
        "the delete after newRun changed nothing"
    );
}

#[test]
fn save_verbs_gate_by_scene() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    // A played level is installed: refused.
    app.session.as_mut().unwrap().modal_stack.clear_pushed();
    assert!(!app.at_frontend());
    name_entry(&app, "Vex");
    app.handle_save_verb(create(0));
    assert!(matches!(
        app.session.as_ref().unwrap().saves.character_slots()[0],
        SlotFile::Empty
    ));
    // At the frontend: permitted.
    app.present_frontend_menu();
    app.handle_save_verb(create(0));
    assert!(saves(dir.path()).character_path(0).exists());
}

#[test]
fn no_selection_blocks_session_entry_and_new_run() {
    let capture = postretro_test_log_capture::LogCapture::start();
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());

    app.handle_save_verb(new_run(0, "next"));
    capture.assert_logged(log::Level::Warn, "no character is selected");
    assert!(!app.host_from_frontend(0));
    capture.assert_logged(log::Level::Warn, "host refused: select a character first");
    assert!(!app.join_from_frontend("127.0.0.1:9".parse().unwrap()));
    capture.assert_logged(log::Level::Warn, "join refused: select a character first");

    character(&mut app, 0, "Vex");
    assert!(app.host_from_frontend(0), "permitted once selected");
}

#[test]
fn hosting_frontend_verb_gate() {
    let capture = postretro_test_log_capture::LogCapture::start();
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 1, "Ash");
    app.handle_save_verb(new_run(1, "second"));
    back_to_frontend(&mut app);
    character(&mut app, 0, "Vex");
    app.handle_save_verb(new_run(0, "next"));
    back_to_frontend(&mut app);
    assert!(app.host_from_frontend(0));

    app.handle_save_verb(select(1));
    app.handle_save_verb(delete_run(1));
    app.handle_save_verb(continue_run(1));
    capture.assert_logged(log::Level::Warn, "selectCharacter refused");
    capture.assert_logged(log::Level::Warn, "deleteRun refused");
    capture.assert_logged(log::Level::Warn, "belongs to another character");
    assert_eq!(
        selection(&app),
        Selection {
            character: 0,
            run: 0
        }
    );

    app.handle_save_verb(continue_run(0));
    assert!(
        !app.at_frontend(),
        "the host's own Continue starts its level"
    );
}

#[test]
fn store_reload_resets_mod_slots_and_keeps_engine_slots() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    set(&app, SCORE, 7.0);
    save(&mut app);
    // A non-persisted mod slot and an engine slot written during play.
    script_ctx(&app)
        .slot_table
        .borrow_mut()
        .insert_namespace(
            "scratch",
            vec![("temp".into(), {
                let mut record = crate::session::saves::test_fixtures::persisted(false);
                record.schema.persist = false;
                record
            })],
        )
        .unwrap();
    set(&app, "scratch.temp", 4.0);
    name_entry(&app, "typed");

    app.handle_save_verb(new_run(0, "next"));

    assert_eq!(number(&app, "scratch.temp"), Some(SlotValue::Number(0.0)));
    assert_eq!(number(&app, SCORE), Some(SlotValue::Number(7.0)));
    assert_eq!(
        number(&app, "ui.textEntry"),
        Some(SlotValue::String("typed".into()))
    );
}

#[test]
fn selecting_another_character_never_takes_prior_gear() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    set_carry(&mut app, 9);
    save(&mut app);
    let a_before = std::fs::read(saves(dir.path()).character_path(0)).unwrap();
    character(&mut app, 1, "Ash");
    app.handle_save_verb(new_run(1, "next"));
    back_to_frontend(&mut app);
    app.handle_save_verb(select(0));
    assert_eq!(carry(&app).unwrap().reserve.available("shells"), 9);

    app.handle_save_verb(continue_run(1));

    assert_eq!(carry(&app), None, "B spawns empty");
    assert_eq!(
        std::fs::read(saves(dir.path()).character_path(0)).unwrap(),
        a_before
    );
}

#[test]
fn failed_load_still_continues_at_destination() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    app.handle_save_verb(new_run(0, "next"));
    // The level start names `second`, whose file never loads.
    app.drain_level_requests();
    app.enqueue_level_request(super::LevelRequest::Load(super::LevelSource::Catalog(
        "second".into(),
    )));
    app.drain_level_requests();
    back_to_frontend(&mut app);

    app.handle_save_verb(continue_run(0));
    assert!(app.level_requests.iter().any(|request| matches!(
        request,
        super::LevelRequest::Load(super::LevelSource::Catalog(id)) if id == "second"
    )));
}

/// Seat 0's pawn in an installed level holding `shells`; `played` false is
/// the frontend backdrop.
fn install_pawn(app: &mut App, played: bool, shells: u32) {
    use postretro_entities::components::health::HealthComponent;
    use postretro_entities::data_descriptors::HealthDescriptor;
    use postretro_entities::{AmmoReserve, Transform};
    app.boot_state = super::BootState::Running;
    app.backdrop_installed = !played;
    let ctx = script_ctx(app);
    let session = app.session.as_mut().unwrap();
    let mut registry = ctx.registry.borrow_mut();
    let pawn = registry.spawn(Transform::default());
    registry
        .set_component(
            pawn,
            HealthComponent::from_descriptor(&HealthDescriptor {
                max: 100.0,
                ..Default::default()
            }),
        )
        .unwrap();
    let mut reserve = AmmoReserve::new();
    reserve.set_exact("shells", shells);
    registry.set_component(pawn, reserve).unwrap();
    let seats = session.seat_table.as_mut().unwrap();
    seats.bind_pawn(&mut registry, Seat(0), pawn);
    session.session_players.begin_level(
        played,
        Some(seats),
        None,
        &registry,
        &mut ctx.slot_table.borrow_mut(),
    );
}

#[test]
fn new_character_carries_gear_and_second_starts_empty() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    // The dev mod's backdrop spawns a pawn bound to seat 0 under the menu.
    install_pawn(&mut app, false, 50);
    character(&mut app, 0, "Vex");
    app.handle_save_verb(new_run(0, "next"));
    app.drain_level_requests();
    assert_eq!(carry(&app), None, "the backdrop's pawn is never harvested");

    // The first level: a pickup, then the level start of the second.
    install_pawn(&mut app, true, 9);
    app.enqueue_level_request(super::LevelRequest::Load(super::LevelSource::Catalog(
        "second".into(),
    )));
    app.drain_level_requests();
    assert_eq!(carry(&app).unwrap().reserve.available("shells"), 9);
    let saved = read_document(&saves(dir.path()).character_path(0))
        .unwrap()
        .unwrap();
    assert_eq!(saved.seats[0].loadout.reserve.get("shells"), Some(&9));

    back_to_frontend(&mut app);
    character(&mut app, 1, "Ash");
    app.handle_save_verb(new_run(1, "next"));
    assert_eq!(carry(&app), None, "a second new character starts with none");
}
