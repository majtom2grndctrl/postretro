//! Teleporter authoring errors, the yaw-only warning and id-51 cache invalidation,
//! all through the real `prl-build` binary with its stderr captured.
//! See: context/lib/build_pipeline.md §Compiler pipeline

use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use postretro_level_format::teleporters::{TeleportMode, TeleportersSection};
use postretro_level_format::{SectionId, read_container, read_section_data};

const TEXTURE: &str = "50-free-textures/concrete_stone_021";

struct TempBuildDir(PathBuf);

impl TempBuildDir {
    fn new() -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "postretro-teleporters-compile-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(path.join("maps")).expect("create isolated compiler directory");
        Self(path)
    }
}

impl Drop for TempBuildDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn box_brush(min: [i32; 3], max: [i32; 3]) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        "{{\n\
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {TEXTURE} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {TEXTURE} 0 0 0 1 1\n\
         }}"
    )
}

/// Two sealed rooms joined by a doorway at x = 0. Entity 1 is the player spawn;
/// `extra` entities follow, so the first one is entity 2.
fn two_room_map(extra: &[String]) -> String {
    let brushes = [
        box_brush([-272, -144, -16], [272, 144, 0]),
        box_brush([-272, -144, 128], [272, 144, 144]),
        box_brush([-272, -144, 0], [-256, 144, 128]),
        box_brush([256, -144, 0], [272, 144, 128]),
        box_brush([-256, -144, 0], [256, -128, 128]),
        box_brush([-256, 128, 0], [256, 144, 128]),
        box_brush([-16, -128, 0], [16, -32, 128]),
        box_brush([-16, 32, 0], [16, 128, 128]),
        box_brush([-16, -32, 96], [16, 32, 128]),
    ];
    let mut entities = vec![
        format!(
            "{{\n\"classname\" \"worldspawn\"\n{}\n}}",
            brushes.join("\n")
        ),
        "{\n\"classname\" \"player_spawn\"\n\"origin\" \"-128 0 32\"\n}".to_string(),
    ];
    entities.extend(extra.iter().cloned());
    entities.join("\n")
}

fn destination(name: &str, origin: &str, angles: &str) -> String {
    format!(
        "{{\n\"classname\" \"teleport_destination\"\n\"name\" \"{name}\"\n\"origin\" \"{origin}\"\n\"angles\" \"{angles}\"\n}}"
    )
}

fn gate(props: &str, min: [i32; 3], max: [i32; 3]) -> String {
    format!(
        "{{\n\"classname\" \"trigger_teleport\"\n{props}\n{}\n}}",
        box_brush(min, max)
    )
}

fn west_gate(props: &str) -> String {
    gate(props, [-200, -64, 0], [-168, 64, 64])
}

fn run(
    temp: &TempBuildDir,
    name: &str,
    map_text: &str,
    cache: Option<&PathBuf>,
    log: &str,
) -> (Output, PathBuf) {
    let input = temp.0.join("maps").join(format!("{name}.map"));
    let output_path = temp.0.join(format!("{name}.prl"));
    std::fs::write(&input, format!("{map_text}\n")).expect("write fixture map");
    let mut command = Command::new(env!("CARGO_BIN_EXE_prl-build"));
    command
        .env("RUST_LOG", log)
        .arg(&input)
        .arg("-o")
        .arg(&output_path)
        .arg("--no-tui")
        .arg("--sh-probe-spacing")
        .arg("4")
        .arg("--lightmap-density")
        .arg("0.25");
    match cache {
        Some(dir) => command.arg("--cache-dir").arg(dir),
        None => command.arg("--no-cache"),
    };
    (command.output().expect("spawn prl-build"), output_path)
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn logs(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        stderr(output)
    )
}

fn teleporters(prl: &PathBuf) -> TeleportersSection {
    let mut cursor = std::io::Cursor::new(std::fs::read(prl).expect("read PRL"));
    let metadata = read_container(&mut cursor).expect("container");
    let payload = read_section_data(&mut cursor, &metadata, SectionId::Teleporters as u32)
        .expect("read teleporters")
        .expect("teleporters section is written when authored");
    TeleportersSection::from_bytes(&payload).expect("teleporters decode")
}

#[test]
fn teleport_invalid_authoring_fails_prl_build_naming_entity() {
    let cases: [(&str, Vec<String>, &str); 7] = [
        (
            "unknown destination",
            vec![west_gate("\"target\" \"missing\"")],
            "entity 2 'trigger_teleport'",
        ),
        (
            "duplicate destination",
            vec![
                destination("out", "128 0 32", "0 0 0"),
                destination("out", "160 0 32", "0 0 0"),
            ],
            "entity 3 'teleport_destination'",
        ),
        (
            "seamless without landmark",
            vec![
                west_gate("\"target\" \"out\"\n\"mode\" \"seamless\""),
                destination("out", "128 0 32", "0 0 0"),
            ],
            "entity 2 'trigger_teleport'",
        ),
        (
            "classic with landmark",
            vec![
                west_gate("\"target\" \"out\"\n\"landmark\" \"in\""),
                destination("out", "128 0 32", "0 0 0"),
                destination("in", "-128 0 32", "0 0 0"),
            ],
            "entity 2 'trigger_teleport'",
        ),
        (
            "landmark equals target",
            vec![
                west_gate("\"target\" \"out\"\n\"mode\" \"seamless\"\n\"landmark\" \"out\""),
                destination("out", "128 0 32", "0 0 0"),
            ],
            "entity 2 'trigger_teleport'",
        ),
        (
            "unknown landmark",
            vec![
                west_gate("\"target\" \"out\"\n\"mode\" \"seamless\"\n\"landmark\" \"nope\""),
                destination("out", "128 0 32", "0 0 0"),
            ],
            "entity 2 'trigger_teleport'",
        ),
        (
            "unknown mode",
            vec![
                west_gate("\"target\" \"out\"\n\"mode\" \"strange\""),
                destination("out", "128 0 32", "0 0 0"),
            ],
            "entity 2 'trigger_teleport'",
        ),
    ];
    for (case, entities, entity) in cases {
        let temp = TempBuildDir::new();
        let (output, _) = run(&temp, "bad", &two_room_map(&entities), None, "warn");
        assert!(
            !output.status.success(),
            "{case} must fail\n{}",
            logs(&output)
        );
        assert!(
            stderr(&output).contains(entity),
            "{case}: stderr must name `{entity}`\n{}",
            logs(&output)
        );
    }
}

#[test]
fn teleport_destination_pitch_roll_warns_through_prl_build_and_uses_yaw_only() {
    let temp = TempBuildDir::new();
    let map = two_room_map(&[
        west_gate("\"target\" \"out\""),
        destination("out", "128 0 32", "12 90 -2"),
    ]);
    let (output, prl) = run(&temp, "yaw", &map, None, "warn");
    assert!(output.status.success(), "{}", logs(&output));
    let text = stderr(&output);
    assert_eq!(
        text.matches("pitch and roll are ignored").count(),
        1,
        "{}",
        logs(&output)
    );
    assert!(text.contains("entity 3 'teleport_destination'"), "{text}");
    let section = teleporters(&prl);
    assert_eq!(section.teleporters[0].destination_yaw, 90.0f32.to_radians());
    assert_eq!(section.teleporters[0].mode, TeleportMode::Classic);
}

fn residency_cache_state(output: &Output) -> Option<&'static str> {
    let text = stderr(output);
    if text.contains("[cache] cell_residency_set hit") {
        Some("hit")
    } else if text.contains("[cache] cell_residency_set miss") {
        Some("miss")
    } else {
        None
    }
}

#[test]
fn teleport_residency_cache_rebakes_id51_on_pose_shape_and_mode_edits() {
    let temp = TempBuildDir::new();
    let cache = temp.0.join("cache");
    let classic = |dest: &str, source: ([i32; 3], [i32; 3])| {
        two_room_map(&[
            gate("\"target\" \"out\"", source.0, source.1),
            destination("out", dest, "0 0 0"),
        ])
    };
    let seamless = |dest: &str, landmark: &str| {
        two_room_map(&[
            west_gate("\"target\" \"out\"\n\"mode\" \"seamless\"\n\"landmark\" \"in\""),
            destination("out", dest, "0 0 0"),
            destination("in", landmark, "0 0 0"),
        ])
    };
    let base_source = ([-200, -64, 0], [-168, 64, 64]);
    let base = classic("128 0 32", base_source);
    let seamless_base = seamless("128 0 32", "-128 0 32");
    let (cold, _) = run(&temp, "base", &base, Some(&cache), "info");
    assert!(cold.status.success(), "{}", logs(&cold));
    assert_eq!(
        residency_cache_state(&cold),
        Some("miss"),
        "{}",
        logs(&cold)
    );
    let (warm, _) = run(&temp, "base", &base, Some(&cache), "info");
    assert_eq!(residency_cache_state(&warm), Some("hit"), "{}", logs(&warm));
    let (seamless_cold, _) = run(&temp, "base", &seamless_base, Some(&cache), "info");
    assert_eq!(
        residency_cache_state(&seamless_cold),
        Some("miss"),
        "mode change"
    );
    let edits = [
        (
            "destination moved within its room",
            classic("160 0 32", base_source),
            base.as_str(),
        ),
        (
            "source brush reshaped",
            classic("128 0 32", ([-200, -64, 0], [-168, 64, 80])),
            base.as_str(),
        ),
        (
            "landmark moved within its room",
            seamless("128 0 32", "-160 0 32"),
            seamless_base.as_str(),
        ),
        (
            "destination moved under seamless",
            seamless("160 0 32", "-128 0 32"),
            seamless_base.as_str(),
        ),
    ];
    for (edit, edited, baseline) in edits {
        // Re-warm the baseline so the edit is judged against a populated cache.
        let (rewarm, _) = run(&temp, "base", baseline, Some(&cache), "info");
        assert_eq!(
            residency_cache_state(&rewarm),
            Some("hit"),
            "{edit}: baseline"
        );
        let (first, _) = run(&temp, "base", &edited, Some(&cache), "info");
        assert!(first.status.success(), "{edit}\n{}", logs(&first));
        assert_eq!(
            residency_cache_state(&first),
            Some("miss"),
            "{edit}\n{}",
            logs(&first)
        );
        let (second, _) = run(&temp, "base", &edited, Some(&cache), "info");
        assert_eq!(
            residency_cache_state(&second),
            Some("hit"),
            "{edit}: then warm"
        );
    }
}
