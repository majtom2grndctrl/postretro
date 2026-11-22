use crate::format::quake_map::QUAKE_PLAYER_START_CLASSNAMES;
use crate::map_data::MapData;
use crate::map_format::MapFormat;
use crate::parse::parse_map_file;
use log::Level;
use postretro_test_log_capture::LogCapture;

/// A sealed worldspawn box (entity 0) followed by `entities`, so the first one
/// passed is map entity 1.
fn map(entities: &[String]) -> String {
    let world = "{\n\
( 128 0 0 ) ( 128 1 0 ) ( 128 0 1 ) world_tex 0 0 0 1 1\n\
( 256 0 0 ) ( 256 0 1 ) ( 256 1 0 ) world_tex 0 0 0 1 1\n\
( 0 128 0 ) ( 0 128 1 ) ( 1 128 0 ) world_tex 0 0 0 1 1\n\
( 0 256 0 ) ( 1 256 0 ) ( 0 256 1 ) world_tex 0 0 0 1 1\n\
( 0 0 0 ) ( 1 0 0 ) ( 0 1 0 ) world_tex 0 0 0 1 1\n\
( 0 0 128 ) ( 0 1 128 ) ( 1 0 128 ) world_tex 0 0 0 1 1\n\
}";
    format!(
        "{{\n\"classname\" \"worldspawn\"\n{world}\n}}\n{}",
        entities.join("\n")
    )
}

fn point(classname: &str) -> String {
    format!("{{\n\"classname\" \"{classname}\"\n\"origin\" \"192 192 24\"\n}}")
}

fn parse(map_text: &str) -> anyhow::Result<MapData> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "postretro-player-starts-map-{}-{id}.map",
        std::process::id(),
    ));
    std::fs::write(&path, format!("{}\n", map_text.trim()))?;
    let result = parse_map_file(&path, MapFormat::IdTech2);
    let _ = std::fs::remove_file(&path);
    result
}

#[test]
fn quake_player_start_classnames_warn_naming_the_entity_and_player_spawn() {
    for classname in QUAKE_PLAYER_START_CLASSNAMES {
        let capture = LogCapture::start();
        parse(&map(&[point("reference_enemy"), point(classname)]))
            .expect("a Quake player start only warns");
        capture.assert_logged_once(
            Level::Warn,
            &format!("[Compiler] entity 2 '{classname}': Quake's player start is not read"),
        );
        capture.assert_logged_once(Level::Warn, "starts players only at a `player_spawn`");
    }
}

#[test]
fn map_without_player_spawn_warns_once_about_the_free_fly_camera() {
    let capture = LogCapture::start();
    parse(&map(&[
        point("info_player_start"),
        point("info_player_coop"),
    ]))
    .expect("a missing player_spawn only warns");
    capture.assert_logged_once(Level::Warn, "map has no `player_spawn`");
    capture.assert_logged_once(Level::Warn, "free-fly camera");

    capture.clear();
    parse(&map(&[])).expect("an empty map only warns");
    capture.assert_logged_once(Level::Warn, "map has no `player_spawn`");
}

#[test]
fn quake_co_op_start_suggests_coop_only() {
    let capture = LogCapture::start();
    parse(&map(&[
        point("info_player_coop"),
        point("info_player_start"),
    ]))
    .unwrap();
    capture.assert_logged_once(Level::Warn, "'info_player_start'");
    // Only the co-op start's warning carries the hint.
    capture.assert_logged_once(Level::Warn, "`player_spawn` with `coop_only` 1");
}

#[test]
fn quake_player_start_beside_player_spawn_still_warns_but_spawn_is_found() {
    let capture = LogCapture::start();
    parse(&map(&[point("player_spawn"), point("info_player_start")])).unwrap();
    capture.assert_logged_once(Level::Warn, "entity 2 'info_player_start'");
    capture.assert_not_logged(Level::Warn, "map has no `player_spawn`");
}

#[test]
fn map_with_player_spawn_has_no_player_start_warnings() {
    let capture = LogCapture::start();
    // Mod-registered classnames the compiler cannot know stay quiet.
    parse(&map(&[point("player_spawn"), point("reference_enemy")])).unwrap();
    capture.assert_not_logged(Level::Warn, "player_spawn");
    capture.assert_not_logged(Level::Warn, "player start");
    capture.assert_not_logged(Level::Warn, "reference_enemy");
}
