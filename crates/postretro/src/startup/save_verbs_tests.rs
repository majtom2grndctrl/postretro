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
use crate::session::role_state::SessionRole;
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
        name: None,
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

// The name a reaction captured when it fired wins over the entry as the drain
// finds it: a later step of the same body may have emptied it.
#[test]
fn create_character_takes_the_name_captured_at_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    name_entry(&app, "");
    app.handle_save_verb(SystemReactionCommand::CreateCharacter {
        index: 0,
        name: Some("Vex".into()),
        dispatch_source: src(),
        origin: origin(),
    });
    assert_eq!(
        app.session
            .as_ref()
            .unwrap()
            .saves
            .selected_character_name(),
        "Vex"
    );

    // A captured empty name is a name, not a missing capture: the slot's
    // later text does not replace it.
    name_entry(&app, "Ash");
    app.handle_save_verb(SystemReactionCommand::CreateCharacter {
        index: 1,
        name: Some(String::new()),
        dispatch_source: src(),
        origin: origin(),
    });
    assert_eq!(selection(&app).character, 1);
    assert_eq!(
        app.session
            .as_ref()
            .unwrap()
            .saves
            .selected_character_name(),
        ""
    );
}

// Play-test regression: the host's own row and roster entry read `Player 1`
// whatever its character was named, because seat 0 asserts no claim.
fn seat_zero_name(app: &App) -> String {
    app.session
        .as_ref()
        .unwrap()
        .seat_table
        .as_ref()
        .expect("the frontend fixture holds a seat ledger")
        .display_name(postretro_foundation::Seat(0))
        .to_owned()
}

#[test]
fn seat_zero_is_named_for_the_selected_character() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    assert_eq!(seat_zero_name(&app), "Vex");
    character(&mut app, 1, "Ash");
    assert_eq!(seat_zero_name(&app), "Ash");
    app.handle_save_verb(select(0));
    assert_eq!(seat_zero_name(&app), "Vex");
    app.handle_save_verb(delete_character(0));
    assert_eq!(seat_zero_name(&app), "", "no character selected");
}

// The common case: a relaunch with last session's character still selected
// names seat 0 from the boot restore alone, before any verb runs.
#[test]
fn seat_zero_is_named_at_boot_for_the_saved_selection() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut app = frontend(dir.path());
        character(&mut app, 0, "Vex");
        character(&mut app, 1, "Wren");
    }
    let app = frontend(dir.path());
    assert_eq!(selection(&app).character, 1);
    assert_eq!(seat_zero_name(&app), "Wren");
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
    app.enqueue_level_request(crate::startup::LevelRequest::Load(
        crate::startup::LevelSource::Catalog("second".into()),
    ));
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
            crate::startup::LevelRequest::Load(crate::startup::LevelSource::Catalog(id)) if id == "second"
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
    app.enqueue_level_request(crate::startup::LevelRequest::Load(
        crate::startup::LevelSource::Catalog("second".into()),
    ));
    app.drain_level_requests();
    back_to_frontend(&mut app);

    app.handle_save_verb(continue_run(0));
    assert!(app.level_requests.iter().any(|request| matches!(
        request,
        crate::startup::LevelRequest::Load(crate::startup::LevelSource::Catalog(id)) if id == "second"
    )));
}

/// Seat 0's pawn in an installed level holding `shells`; `played` false is
/// the frontend backdrop.
fn install_pawn(app: &mut App, played: bool, shells: u32) {
    use postretro_entities::components::health::HealthComponent;
    use postretro_entities::data_descriptors::HealthDescriptor;
    use postretro_entities::{AmmoReserve, Transform};
    app.boot_state = crate::startup::BootState::Running;
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
    app.enqueue_level_request(crate::startup::LevelRequest::Load(
        crate::startup::LevelSource::Catalog("second".into()),
    ));
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

fn checkpoint(name: &str) -> SystemReactionCommand {
    SystemReactionCommand::Checkpoint {
        name: name.into(),
        dispatch_source: src(),
        origin: origin(),
    }
}

/// `next`'s spawns: two at the level start, two at checkpoint `bridge`.
fn spawn_points() -> Vec<postretro_sim::scripting::map_entity::MapEntity> {
    let mut points = super::spawn_pose_handoff_fixtures::spawn_points(4);
    for point in &mut points[2..] {
        point
            .key_values
            .insert("checkpoint".into(), "bridge".into());
    }
    points
}

/// A run of Vex's in progress at `next`, with `next`'s spawns installed at
/// its level-start set.
fn run_in_next(dir: &std::path::Path) -> App {
    let mut app = frontend(dir);
    script_ctx(&app).data_registry.borrow_mut().entities =
        super::spawn_pose_handoff_fixtures::descriptors();
    character(&mut app, 0, "Vex");
    app.handle_save_verb(new_run(0, "next"));
    app.drain_level_requests();
    played_next(&mut app);
    app
}

/// `next` installed and played, at its level-start set.
fn played_next(app: &mut App) {
    install_pawn(app, true, 9);
    app.active_level_source = Some(crate::startup::LevelSource::Catalog("next".into()));
    app.level_spawn_points = spawn_points();
    let checkpoint = app
        .session
        .as_ref()
        .unwrap()
        .saves
        .spawn_checkpoint()
        .map(str::to_owned);
    app.apply_spawn_checkpoint(checkpoint);
}

fn run_level(dir: &std::path::Path) -> (String, Option<String>) {
    let level = run_file(dir, 0).unwrap().level.unwrap();
    (level.catalog_id, level.checkpoint)
}

fn active_keys(app: &App) -> Vec<Option<String>> {
    app.host_spawn_points
        .iter()
        .map(|spawn| spawn.key_values.get("checkpoint").cloned())
        .collect()
}

#[test]
fn checkpoint_saves_and_continue_arrives_in_its_set() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = run_in_next(dir.path());
    assert_eq!(active_keys(&app), [None, None]);

    app.handle_save_verb(checkpoint("bridge"));

    assert_eq!(
        run_level(dir.path()),
        ("next".into(), Some("bridge".into()))
    );
    assert!(saves(dir.path()).character_path(0).exists());
    assert_eq!(
        active_keys(&app),
        [Some("bridge".into()), Some("bridge".into())]
    );
    // Respawn and late joiners choose within the checkpoint's set.
    let ctx = script_ctx(&app);
    let published = ctx.respawn_placements.borrow();
    assert_eq!(
        published
            .map_order
            .iter()
            .map(|placement| placement.map_index)
            .collect::<Vec<_>>(),
        [0, 1],
        "indices into the active set: the two bridge spawns"
    );
    drop(published);

    // Continue arrives at `bridge`, and keeps it at its own level start (P11).
    back_to_frontend(&mut app);
    app.handle_save_verb(continue_run(0));
    app.drain_level_requests();
    assert_eq!(
        app.session.as_ref().unwrap().saves.spawn_checkpoint(),
        Some("bridge")
    );
    assert_eq!(
        run_level(dir.path()),
        ("next".into(), Some("bridge".into()))
    );
    played_next(&mut app);
    assert_eq!(
        active_keys(&app),
        [Some("bridge".into()), Some("bridge".into())]
    );

    // Quitting and Continue again with no save point between: `bridge` again.
    back_to_frontend(&mut app);
    app.handle_save_verb(continue_run(0));
    app.drain_level_requests();
    assert_eq!(
        app.session.as_ref().unwrap().saves.spawn_checkpoint(),
        Some("bridge")
    );
}

#[test]
fn unknown_checkpoint_falls_back_and_level_start_clears() {
    let capture = postretro_test_log_capture::LogCapture::start();
    let dir = tempfile::tempdir().unwrap();
    let mut app = run_in_next(dir.path());

    app.handle_save_verb(checkpoint("vault"));
    capture.assert_logged(log::Level::Warn, "checkpoint `vault` names no player_spawn");
    assert_eq!(active_keys(&app), [None, None]);

    app.handle_save_verb(checkpoint("bridge"));
    app.enqueue_level_request(crate::startup::LevelRequest::Load(
        crate::startup::LevelSource::Catalog("second".into()),
    ));
    app.drain_level_requests();
    assert_eq!(run_level(dir.path()), ("second".into(), None));
    assert_eq!(app.session.as_ref().unwrap().saves.spawn_checkpoint(), None);
}

#[test]
fn checkpoint_scope_by_run_and_scene() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = run_in_next(dir.path());
    // Outside an active run: only the character saves.
    back_to_frontend(&mut app);
    let run_before = std::fs::read(saves(dir.path()).run_path(0)).unwrap();
    app.session.as_mut().unwrap().modal_stack.clear_pushed();
    played_next(&mut app);
    app.handle_save_verb(checkpoint("bridge"));
    assert_eq!(
        std::fs::read(saves(dir.path()).run_path(0)).unwrap(),
        run_before
    );
    let saved = read_document(&saves(dir.path()).character_path(0))
        .unwrap()
        .unwrap();
    assert_eq!(saved.seats[0].loadout.reserve.get("shells"), Some(&9));

    // In the frontend backdrop: nothing.
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    install_pawn(&mut app, false, 9);
    let before = std::fs::read(saves(dir.path()).character_path(0)).unwrap();
    app.handle_save_verb(checkpoint("bridge"));
    assert_eq!(
        std::fs::read(saves(dir.path()).character_path(0)).unwrap(),
        before
    );
}

#[test]
fn checkpoint_and_load_level_in_one_reaction_resume_destination() {
    for checkpoint_first in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = run_in_next(dir.path());
        let load = || {
            crate::startup::LevelRequest::Load(crate::startup::LevelSource::Catalog(
                "second".into(),
            ))
        };
        if checkpoint_first {
            app.handle_save_verb(checkpoint("bridge"));
            app.enqueue_level_request(load());
        } else {
            app.enqueue_level_request(load());
            app.handle_save_verb(checkpoint("bridge"));
        }
        app.drain_level_requests();
        assert_eq!(
            run_level(dir.path()),
            ("second".into(), None),
            "checkpoint first: {checkpoint_first}"
        );
    }
}

#[test]
fn later_checkpoint_in_one_tick_wins() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = run_in_next(dir.path());
    app.level_spawn_points[0]
        .key_values
        .insert("checkpoint".into(), "gate".into());
    app.handle_save_verb(checkpoint("gate"));
    app.handle_save_verb(checkpoint("bridge"));
    assert_eq!(
        run_level(dir.path()),
        ("next".into(), Some("bridge".into()))
    );
    assert_eq!(
        active_keys(&app),
        [Some("bridge".into()), Some("bridge".into())]
    );
}

// V3 and N8: a guest admitted at the host's lobby keeps its per-owner values
// and loadout through the host's Continue, its final character returns them
// unchanged, and the host's save files never hold them.
#[test]
fn lobby_guest_keeps_values_through_host_continue() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    set_xp(&app, 2.0);
    save(&mut app);
    app.handle_save_verb(new_run(0, "next"));
    back_to_frontend(&mut app);
    assert!(app.host_from_frontend(0));
    let guest = {
        let session = app.session.as_mut().unwrap();
        let seats = session.seat_table.as_mut().unwrap();
        let claim = postretro_net::wire::ConnectClaim {
            player_id: postretro_net::wire::PlayerClaimId([0x42; 16]),
            display_name: "Guest".into(),
        };
        let seat = seats.admit_or_reclaim(41, Some(claim), false).unwrap().seat;
        let mut carried = CarriedState::default();
        carried.reserve.set_exact("shells", 4);
        seats.set_carried_state(seat, Some(carried));
        seat
    };
    script_ctx(&app)
        .slot_table
        .borrow_mut()
        .get_mut(crate::session::saves::test_fixtures::XP)
        .unwrap()
        .set_per_seat_value(guest, SlotValue::Number(9.0));

    app.handle_save_verb(continue_run(0));
    app.drain_level_requests();

    let session = app.session.as_ref().unwrap();
    let guest_carry = session
        .seat_table
        .as_ref()
        .unwrap()
        .carried_state(guest)
        .cloned();
    assert_eq!(guest_carry.unwrap().reserve.available("shells"), 4);
    // What its final character reads: the seat's live value, unchanged.
    assert_eq!(
        script_ctx(&app)
            .slot_table
            .borrow()
            .get(crate::session::saves::test_fixtures::XP)
            .unwrap()
            .per_seat_value(guest)
            .cloned(),
        Some(SlotValue::Number(9.0))
    );
    assert_eq!(
        xp(&app),
        Some(SlotValue::Number(2.0)),
        "the host's own value"
    );

    // The host's files hold only its own character.
    for path in [
        saves(dir.path()).character_path(0),
        saves(dir.path()).world_path(),
        saves(dir.path()).run_path(0),
    ] {
        let bytes = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
        assert!(!bytes.contains(&"42".repeat(16)), "{}", path.display());
        assert!(!bytes.contains("Guest"), "{}", path.display());
        assert!(
            !bytes.contains("9.0") && !bytes.contains(": 9"),
            "{}",
            path.display()
        );
    }
}

/// Load catalog `map` from the frontend as `loadLevel` does, then stand in
/// it with seat 0 holding `shells`.
fn load_and_play(app: &mut App, map: &str, shells: u32) {
    app.session.as_mut().unwrap().modal_stack.clear_pushed();
    app.enqueue_level_request(crate::startup::LevelRequest::Load(
        crate::startup::LevelSource::Catalog(map.into()),
    ));
    app.drain_level_requests();
    install_pawn(app, true, shells);
}

// R11: a world global set in a `loadLevel` level outside any run survives
// the return to the frontend and a selection.
#[test]
fn world_global_outside_run_survives_frontend_and_select() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    load_and_play(&mut app, "next", 1);
    set(&app, SCORE, 7.0);

    back_to_frontend(&mut app);
    app.handle_save_verb(select(0));

    assert_eq!(number(&app, SCORE), Some(SlotValue::Number(7.0)));
}

// R13: a world global written in a run survives the frontend, each reload
// verb and a restart.
#[test]
fn world_global_in_run_survives_verbs_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    app.handle_save_verb(new_run(0, "next"));
    app.drain_level_requests();
    install_pawn(&mut app, true, 1);
    set(&app, SCORE, 8.0);
    back_to_frontend(&mut app);

    app.handle_save_verb(select(0));
    assert_eq!(number(&app, SCORE), Some(SlotValue::Number(8.0)), "select");
    app.handle_save_verb(continue_run(0));
    assert_eq!(
        number(&app, SCORE),
        Some(SlotValue::Number(8.0)),
        "continue"
    );
    back_to_frontend(&mut app);
    app.handle_save_verb(new_run(1, "second"));
    assert_eq!(number(&app, SCORE), Some(SlotValue::Number(8.0)), "newRun");

    let mut later = app_with_store(dir.path());
    boot(&mut later, SAVES);
    later.session.as_mut().unwrap();
    crate::session::saves::test_fixtures::present_frontend(&mut later);
    later.handle_save_verb(select(0));
    assert_eq!(
        number(&later, SCORE),
        Some(SlotValue::Number(8.0)),
        "restart"
    );
}

// S6: after Continue and a return to the frontend, a dev-picker load and its
// level start leave the run file untouched and write the character.
#[test]
fn dev_picker_load_after_continue_leaves_run_file_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    app.handle_save_verb(new_run(0, "next"));
    back_to_frontend(&mut app);
    app.handle_save_verb(continue_run(0));
    back_to_frontend(&mut app);
    let run_before = std::fs::read(saves(dir.path()).run_path(0)).unwrap();

    load_and_play(&mut app, "second", 13);
    app.enqueue_level_request(crate::startup::LevelRequest::Load(
        crate::startup::LevelSource::Catalog("next".into()),
    ));
    app.drain_level_requests();

    assert_eq!(
        std::fs::read(saves(dir.path()).run_path(0)).unwrap(),
        run_before
    );
    let saved = read_document(&saves(dir.path()).character_path(0))
        .unwrap()
        .unwrap();
    assert_eq!(saved.seats[0].loadout.reserve.get("shells"), Some(&13));
}

// R19: a mod without a `saves` block keeps per-owner and global progress and
// faction sentiment across a restart, its one run active throughout, also
// after the frontend and another level.
#[test]
fn mod_without_saves_block_keeps_progress_and_sentiment() {
    use postretro_entities::data_registry::{DEFAULT_ENEMY_FACTION_INDEX, PLAYER_FACTION_INDEX};
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_store(dir.path());
    boot(&mut app, None);
    present_frontend(&mut app);
    for id in ["next", "second"] {
        script_ctx(&app)
            .data_registry
            .borrow_mut()
            .maps
            .push(ModMapEntry {
                id: id.into(),
                path: format!("maps/{id}-never-read.prl"),
                name: id.into(),
                tags: Vec::new(),
                loading_tree: Vec::new(),
            });
    }
    load_and_play(&mut app, "next", 1);
    set(&app, SCORE, 3.0);
    set(&app, GENERATOR, 4.0);
    set_xp(&app, 5.0);
    script_ctx(&app)
        .faction_sentiment
        .borrow_mut()
        .set(PLAYER_FACTION_INDEX, DEFAULT_ENEMY_FACTION_INDEX, -0.5, 0.0)
        .unwrap();
    back_to_frontend(&mut app);
    assert!(
        app.session.as_ref().unwrap().saves.run_active(),
        "after the frontend"
    );
    load_and_play(&mut app, "second", 1);
    app.enqueue_level_request(crate::startup::LevelRequest::Load(
        crate::startup::LevelSource::Catalog("next".into()),
    ));
    app.drain_level_requests();
    drop(app);

    let mut later = app_with_store(dir.path());
    boot(&mut later, None);
    assert!(later.session.as_ref().unwrap().saves.run_active());
    assert_eq!(number(&later, SCORE), Some(SlotValue::Number(3.0)));
    assert_eq!(number(&later, GENERATOR), Some(SlotValue::Number(4.0)));
    assert_eq!(xp(&later), Some(SlotValue::Number(5.0)));
    assert_eq!(
        script_ctx(&later)
            .faction_sentiment
            .borrow()
            .get(PLAYER_FACTION_INDEX, DEFAULT_ENEMY_FACTION_INDEX),
        Some(-0.5)
    );
}

struct NoExit;

impl crate::startup::LoopExit for NoExit {
    fn exit(&self) {
        panic!("a runtime load failure never exits the loop");
    }
}

// Review fix: a Continue or newRun whose level fails to load lands on the
// frontend, which ends the run and frees the verbs.
#[test]
fn failed_run_load_ends_the_run_and_frees_the_verbs() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    app.handle_save_verb(new_run(0, "next"));
    app.drain_level_requests();
    let run_before = std::fs::read(saves(dir.path()).run_path(0)).unwrap();

    app.finish_level_failure("prl parse failed".to_string(), &NoExit);
    assert!(
        app.at_frontend(),
        "the menu comes back after a failed run load"
    );

    let session = app.session.as_ref().unwrap();
    assert!(!session.saves.run_active());
    assert!(!session.saves.run_load_queued());
    app.handle_save_verb(select(0));
    // A dev-picker load afterwards leaves the run file alone.
    load_and_play(&mut app, "second", 1);
    app.enqueue_level_request(crate::startup::LevelRequest::Load(
        crate::startup::LevelSource::Catalog("next".into()),
    ));
    app.drain_level_requests();
    assert_eq!(
        std::fs::read(saves(dir.path()).run_path(0)).unwrap(),
        run_before
    );
}

// Review fix: a return to the frontend queued earlier in the same drain does
// not end the run newRun starts.
#[test]
fn queued_return_to_frontend_does_not_end_a_new_run() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    app.navigate_to_frontend();

    app.handle_save_verb(new_run(0, "next"));
    app.drain_level_requests();

    assert!(app.session.as_ref().unwrap().saves.run_active());
}

// Review fix: a world slot written at the frontend survives the reload a
// verb runs; a run-scoped one resets with the new run.
#[test]
fn frontend_world_write_survives_new_run() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    set(&app, SCORE, 6.0);
    set(&app, GENERATOR, 2.0);

    app.handle_save_verb(new_run(0, "next"));

    assert_eq!(number(&app, SCORE), Some(SlotValue::Number(6.0)));
    assert_eq!(number(&app, GENERATOR), Some(SlotValue::Number(0.0)));
}

// Review fix: selecting a character refreshes the per-owner value UI binds
// read, not only seat 0's.
#[test]
fn select_character_refreshes_the_bound_per_owner_value() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    set_xp(&app, 5.0);
    save(&mut app);
    character(&mut app, 1, "Ash");

    app.handle_save_verb(select(0));

    assert_eq!(
        number(&app, crate::session::saves::test_fixtures::XP),
        Some(SlotValue::Number(5.0))
    );
}

/// A booted mod without a `saves` block at its frontend, with `next` in its
/// catalog.
fn implicit_frontend(dir: &std::path::Path) -> App {
    let mut app = app_with_store(dir);
    boot(&mut app, None);
    present_frontend(&mut app);
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .maps
        .push(ModMapEntry {
            id: "next".into(),
            path: "maps/next-never-read.prl".into(),
            name: "next".into(),
            tags: Vec::new(),
            loading_tree: Vec::new(),
        });
    app
}

// Review fixes for a mod without a `saves` block: selecting its character
// keeps the implicit run's state, and deleting its run lets level starts
// still save the character.
#[test]
fn implicit_mod_select_keeps_run_state_and_delete_run_still_saves() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = implicit_frontend(dir.path());
    set(&app, GENERATOR, 4.0);
    load_and_play(&mut app, "next", 1);
    app.enqueue_level_request(crate::startup::LevelRequest::Load(
        crate::startup::LevelSource::Catalog("next".into()),
    ));
    app.drain_level_requests();
    back_to_frontend(&mut app);

    app.handle_save_verb(select(0));
    assert_eq!(number(&app, GENERATOR), Some(SlotValue::Number(4.0)));

    app.handle_save_verb(delete_run(0));
    assert!(!app.session.as_ref().unwrap().saves.run_active());
    load_and_play(&mut app, "next", 23);
    app.enqueue_level_request(crate::startup::LevelRequest::Load(
        crate::startup::LevelSource::Catalog("next".into()),
    ));
    app.drain_level_requests();
    let saved = read_document(&saves(dir.path()).character_path(0))
        .unwrap()
        .unwrap();
    assert_eq!(saved.seats[0].loadout.reserve.get("shells"), Some(&23));
}

// C1 co-op and C3: once a checkpoint fires, late joiners take different
// spawns of its set.
#[test]
fn late_joiners_spread_over_the_active_checkpoint_set() {
    use super::spawn_pose_handoff_fixtures::Relay;
    let points = spawn_points();
    let mut relay = Relay::host();
    relay.install(points[..2].to_vec());
    relay.host_reveal();
    relay.app.level_spawn_points = points.clone();
    relay.app.apply_spawn_checkpoint(Some("bridge".into()));

    let mut placed = Vec::new();
    for client in [21, 22] {
        relay.admit(&[client]);
        relay.reveal(client);
        let pawn = relay.await_pawn(client);
        placed.push(relay.transform(pawn).position);
    }

    let bridge = super::spawn_pose_handoff_fixtures::standing_on(&points[2..], 0);
    let bridge_second = super::spawn_pose_handoff_fixtures::standing_on(&points[2..], 1);
    assert!(
        placed.iter().all(|position| {
            position.distance(bridge) < 0.01 || position.distance(bridge_second) < 0.01
        }),
        "{placed:?}"
    );
    assert!(
        placed[0].distance(placed[1]) > 0.01,
        "each joiner its own spawn"
    );
}

/// Put a `--connect` boot's client endpoint on `app`, as boot flags do before
/// the mod's saves load.
fn boot_as_client(app: &mut App) {
    let session = app.session.as_mut().unwrap();
    let entry = crate::session::net_entry::enter_role(
        &crate::netcode::NetRole::Connect {
            addr: "127.0.0.1:9".parse().unwrap(),
        },
        session.player_options.player_id.map(|id| (id, "")),
        &mut session.scripting,
        &mut session.seat_table,
        getrandom::fill,
        || None,
        "",
    );
    session.net_endpoint = entry.endpoint;
    session.boot_connect = Some(("127.0.0.1:9".parse().unwrap(), String::new()));
    session.boot_role_pending = true;
    assert_eq!(app.session_role(), SessionRole::Client);
}

// Regression: a client that booted connected kept a store snapshot without
// the implicit run, so its leave restored defaults that the next level start
// saved over the run.
#[test]
fn connected_boot_keeps_the_implicit_run_for_its_leave() {
    let dir = tempfile::tempdir().unwrap();
    let mut first = implicit_frontend(dir.path());
    load_and_play(&mut first, "next", 1);
    set(&first, GENERATOR, 4.0);
    first.enqueue_level_request(crate::startup::LevelRequest::Load(
        crate::startup::LevelSource::Catalog("next".into()),
    ));
    first.drain_level_requests();
    drop(first);

    let mut app = app_with_store(dir.path());
    boot_as_client(&mut app);
    boot(&mut app, None);

    let retained = app
        .session
        .as_ref()
        .unwrap()
        .persisted_state
        .clone()
        .unwrap();
    assert_eq!(
        retained.slots().get("k-gen"),
        Some(&postretro_sim::scripting::state_persistence::PersistedValue::Number(4.0))
    );
}

// Review fix: a `--connect` boot of a mod with a `saves` block and no
// selected character leaves, as an in-app join would have been refused.
#[test]
fn boot_connect_without_a_selected_character_leaves() {
    let capture = postretro_test_log_capture::LogCapture::start();
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_store(dir.path());
    boot_as_client(&mut app);
    boot(&mut app, SAVES);

    app.reconcile_boot_role_with_saves();

    capture.assert_logged(log::Level::Warn, "--connect refused");
    assert_eq!(app.session_role(), SessionRole::SinglePlayer);
}

// Review round 3: a suspend while a run's level plays resumes at the
// frontend: the character saves what it held, the run ends, and the verbs
// run with no backdrop.
#[test]
fn suspend_during_a_played_run_level_saves_and_frees_the_verbs() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    app.handle_save_verb(new_run(0, "next"));
    app.drain_level_requests();
    install_pawn(&mut app, true, 17);
    {
        // The suspend's own harvest, then its reset.
        let session = app.session.as_mut().unwrap();
        let ctx = session.scripting.script_ctx.clone();
        session
            .seat_table
            .as_mut()
            .unwrap()
            .harvest_bound_pawns(&ctx.registry.borrow(), &[]);
    }
    app.reset_boot_state_after_suspend();
    app.boot_state = crate::startup::BootState::Frontend;
    present_frontend(&mut app);

    assert!(app.at_frontend());
    assert!(!app.session.as_ref().unwrap().saves.run_active());
    let saved = read_document(&saves(dir.path()).character_path(0))
        .unwrap()
        .unwrap();
    assert_eq!(saved.seats[0].loadout.reserve.get("shells"), Some(&17));
    app.handle_save_verb(select(0));
    assert_eq!(carry(&app).unwrap().reserve.available("shells"), 17);
}

// Review round 2: a suspend before a run's level loads resumes at the
// frontend, which ends the run and frees the verbs.
#[test]
fn suspend_resume_ends_a_queued_run() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    app.handle_save_verb(new_run(0, "next"));

    app.reset_boot_state_after_suspend();

    let session = app.session.as_ref().unwrap();
    assert!(!session.saves.run_active());
    assert!(!session.saves.run_load_queued());
}

// Review round 2: a frontend entry whose character write failed keeps the
// gear the unload harvested, for the next save point to retry.
#[test]
fn frontend_entry_keeps_harvested_gear_when_the_character_write_fails() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    load_and_play(&mut app, "next", 31);
    let blocker = saves(dir.path())
        .character_path(0)
        .with_extension("json.tmp");
    std::fs::create_dir_all(&blocker).unwrap();

    back_to_frontend(&mut app);

    assert_eq!(carry(&app).unwrap().reserve.available("shells"), 31);
}

// Review round 4: a second suspend at the resumed frontend never re-harvests
// the pawn the first suspend left bound into another character.
#[test]
fn second_suspend_never_reharvests_a_stale_pawn_into_another_character() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    app.handle_save_verb(new_run(0, "next"));
    app.drain_level_requests();
    install_pawn(&mut app, true, 9);
    app.suspend_platform();
    let saved = read_document(&saves(dir.path()).character_path(0))
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.seats[0].loadout.reserve.get("shells"),
        Some(&9),
        "the suspend harvests and saves the played level's gear"
    );
    app.boot_state = crate::startup::BootState::Frontend;
    present_frontend(&mut app);
    character(&mut app, 1, "Ash");
    assert_eq!(carry(&app), None);

    app.suspend_platform();

    assert_eq!(carry(&app), None, "Ash never takes Vex's gear");
}

// F4: a slot whose file this build cannot read stays occupied; selecting it,
// continuing it and starting a new run in it are refused, changing nothing,
// and deleting it is permitted.
#[test]
fn verbs_on_an_unreadable_slot_are_refused_and_delete_is_permitted() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = frontend(dir.path());
    character(&mut app, 0, "Vex");
    let unreadable_character = saves(dir.path()).character_path(1);
    let unreadable_run = saves(dir.path()).run_path(1);
    std::fs::create_dir_all(unreadable_run.parent().unwrap()).unwrap();
    std::fs::write(&unreadable_character, br#"{"version":999}"#).unwrap();
    std::fs::write(&unreadable_run, b"{ not json").unwrap();
    // Reload the slots as boot does.
    drop(app);
    let mut app = app_with_store(dir.path());
    boot(&mut app, SAVES);
    present_frontend(&mut app);
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .maps
        .push(ModMapEntry {
            id: "next".into(),
            path: "maps/next-never-read.prl".into(),
            name: "next".into(),
            tags: Vec::new(),
            loading_tree: Vec::new(),
        });
    app.handle_save_verb(select(0));
    let selection_before = selection(&app);

    app.handle_save_verb(select(1));
    app.handle_save_verb(continue_run(1));
    app.handle_save_verb(new_run(1, "next"));

    assert_eq!(selection(&app), selection_before);
    assert_eq!(
        std::fs::read(&unreadable_character).unwrap(),
        br#"{"version":999}"#
    );
    assert_eq!(std::fs::read(&unreadable_run).unwrap(), b"{ not json");
    assert!(app.at_frontend());

    app.handle_save_verb(delete_character(1));
    app.handle_save_verb(delete_run(1));
    assert!(!unreadable_character.exists());
    assert!(!unreadable_run.exists());
}
