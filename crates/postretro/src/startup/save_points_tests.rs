// Save points through the production level-request drain: a level start
// writes the run, then the character, then the world, after the harvest and
// before the next level installs; a backdrop's pawn is never harvested; an
// exit mid-level saves the gear held now.
// See: context/lib/scripting.md §5 (Durable State Store)

use postretro_entities::components::health::HealthComponent;
use postretro_entities::data_descriptors::HealthDescriptor;
use postretro_entities::{AmmoReserve, Transform};
use postretro_foundation::{ModMapEntry, Seat};
use postretro_sim::scripting::save_files::{LedgerDocument, LoadoutRecord, SaveDir, read_document};

use super::{LevelRequest, LevelSource};
use crate::App;
use crate::session::saves::test_fixtures::{MOD_ID, app_with_store, boot};
use crate::startup::lifecycle::tests::script_ctx;

fn saves_dir(dir: &std::path::Path) -> SaveDir {
    SaveDir::new(dir, MOD_ID)
}

/// A booted mod without a `saves` block (one active run) whose catalog
/// holds `next`, a map that is never read in these tests.
fn booted(dir: &std::path::Path) -> App {
    let mut app = app_with_store(dir);
    boot(&mut app, None);
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .maps
        .push(ModMapEntry {
            id: "next".into(),
            path: "maps/never-read.prl".into(),
            name: "Next".into(),
            tags: Vec::new(),
            loading_tree: Vec::new(),
        });
    app
}

/// Seat 0's pawn in an installed level holding `health` and `shells`.
/// `played` false installs it as a frontend backdrop.
fn install(app: &mut App, played: bool, health: f32, shells: u32) {
    app.boot_state = super::BootState::Running;
    let ctx = script_ctx(app);
    let session = app.session.as_mut().unwrap();
    let mut registry = ctx.registry.borrow_mut();
    let pawn = registry.spawn(Transform::default());
    let mut component = HealthComponent::from_descriptor(&HealthDescriptor {
        max: 100.0,
        ..Default::default()
    });
    component.current = health;
    registry.set_component(pawn, component).unwrap();
    let mut reserve = AmmoReserve::new();
    reserve.set_exact("shells", shells);
    registry.set_component(pawn, reserve).unwrap();
    let seats = session.seat_table.as_mut().unwrap();
    seats.bind_pawn(&mut registry, Seat(0), pawn);
    app.backdrop_installed = !played;
    session.session_players.begin_level(
        played,
        Some(seats),
        None,
        &registry,
        &mut ctx.slot_table.borrow_mut(),
    );
}

fn character(dir: &std::path::Path) -> LoadoutRecord {
    read_document(&saves_dir(dir).character_path(0))
        .unwrap()
        .expect("the character file exists")
        .seats[0]
        .loadout
        .clone()
}

fn run(dir: &std::path::Path) -> LedgerDocument {
    read_document(&saves_dir(dir).run_path(0))
        .unwrap()
        .expect("the run file exists")
}

fn load(app: &mut App, source: LevelSource) {
    app.enqueue_level_request(LevelRequest::Load(source));
    app.drain_level_requests();
}

#[test]
fn level_start_writes_run_then_character_before_install() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = booted(dir.path());
    install(&mut app, true, 40.0, 9);

    load(&mut app, LevelSource::Catalog("next".into()));

    let level = run(dir.path()).level.expect("the run names a level");
    assert_eq!(level.catalog_id, "next");
    assert_eq!(level.checkpoint, None);
    let loadout = character(dir.path());
    assert_eq!(loadout.health, Some(40.0));
    assert_eq!(loadout.reserve.get("shells"), Some(&9));
    assert!(
        app.level_load_in_flight() && !app.level_is_installed_state(),
        "the saves land before the destination installs"
    );
}

#[test]
fn crash_between_level_start_writes_never_duplicates_gear() {
    // The character write fails: the run names the destination and the
    // character keeps its earlier gear.
    let dir = tempfile::tempdir().unwrap();
    let mut app = booted(dir.path());
    let before = character(dir.path());
    install(&mut app, true, 40.0, 9);
    let blocker = saves_dir(dir.path())
        .character_path(0)
        .with_extension("json.tmp");
    std::fs::create_dir_all(&blocker).unwrap();
    load(&mut app, LevelSource::Catalog("next".into()));
    assert_eq!(run(dir.path()).level.unwrap().catalog_id, "next");
    assert_eq!(character(dir.path()), before);

    // The run write fails: the character is not written either, so the
    // earlier level never pairs with the later gear.
    let dir = tempfile::tempdir().unwrap();
    let mut app = booted(dir.path());
    let run_before = std::fs::read(saves_dir(dir.path()).run_path(0)).unwrap();
    install(&mut app, true, 40.0, 9);
    let blocker = saves_dir(dir.path()).run_path(0).with_extension("json.tmp");
    std::fs::create_dir_all(&blocker).unwrap();
    load(&mut app, LevelSource::Catalog("next".into()));
    assert_eq!(
        std::fs::read(saves_dir(dir.path()).run_path(0)).unwrap(),
        run_before
    );
    assert!(character(dir.path()).is_empty());
}

#[test]
fn downed_character_saves_initial_life_state_at_full_health() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = booted(dir.path());
    install(&mut app, true, 0.0, 3);
    {
        // A non-initial state the harvest recorded.
        let session = app.session.as_mut().unwrap();
        let seats = session.seat_table.as_mut().unwrap();
        let mut carried = seats.carried_state(Seat(0)).cloned().unwrap_or_default();
        carried.life_state = Some("down".into());
        seats.set_carried_state(Seat(0), Some(carried));
    }

    load(&mut app, LevelSource::Catalog("next".into()));

    let loadout = character(dir.path());
    assert_eq!(loadout.life_state, None, "initial");
    assert_eq!(loadout.health, None, "full on restore");
    assert_eq!(loadout.reserve.get("shells"), Some(&3), "gear kept");
}

#[test]
fn dropped_app_loses_only_play_since_last_save_point() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = booted(dir.path());
    install(&mut app, true, 40.0, 9);
    load(&mut app, LevelSource::Catalog("next".into()));
    // Play after the level start, then the process dies without its exit.
    install(&mut app, true, 12.0, 30);
    drop(app);

    let mut later = app_with_store(dir.path());
    boot(&mut later, None);
    let carried = later
        .session
        .as_ref()
        .unwrap()
        .seat_table
        .as_ref()
        .unwrap()
        .carried_state(Seat(0))
        .cloned()
        .expect("seat 0 carries the saved character");
    assert_eq!(carried.health_current, Some(40.0));
    assert_eq!(carried.reserve.available("shells"), 9);
}

#[test]
fn path_load_leaves_run_file_untouched_and_writes_character() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = booted(dir.path());
    let run_before = std::fs::read(saves_dir(dir.path()).run_path(0)).unwrap();
    install(&mut app, true, 40.0, 9);

    load(
        &mut app,
        LevelSource::Path(std::path::PathBuf::from("maps/never-read.prl")),
    );

    assert_eq!(
        std::fs::read(saves_dir(dir.path()).run_path(0)).unwrap(),
        run_before
    );
    assert_eq!(character(dir.path()).health, Some(40.0));
}

#[test]
fn backdrop_pawn_is_never_harvested() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = booted(dir.path());
    crate::session::saves::test_fixtures::present_frontend(&mut app);
    assert!(app.frontend_menu_is_present());
    install(&mut app, false, 40.0, 9);

    // A played load from the frontend clears the menu, as `loadLevel` does.
    app.session.as_mut().unwrap().modal_stack.clear_pushed();
    load(&mut app, LevelSource::Catalog("next".into()));

    let session = app.session.as_ref().unwrap();
    assert_eq!(
        session.seat_table.as_ref().unwrap().carried_state(Seat(0)),
        None
    );
    assert!(character(dir.path()).is_empty());
}

#[test]
fn exit_mid_level_saves_the_gear_held_now() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = booted(dir.path());
    install(&mut app, true, 40.0, 9);

    app.leave_end_and_save_for_exit();

    let loadout = character(dir.path());
    assert_eq!(loadout.health, Some(40.0));
    assert_eq!(loadout.reserve.get("shells"), Some(&9));
}
