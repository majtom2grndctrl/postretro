use crate::map_data::MapData;
use crate::map_format::MapFormat;
use crate::parse::parse_map_file;
use postretro_level_format::ghost_links::{
    GHOST_LINK_SOURCE_ENTITY_KEY, GhostLinksSection, GhostMembers,
};

fn box_brush(min: [i32; 3], max: [i32; 3], texture: &str) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        r#"{{
( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {texture} 0 0 0 1 1
( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {texture} 0 0 0 1 1
( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {texture} 0 0 0 1 1
( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {texture} 0 0 0 1 1
( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {texture} 0 0 0 1 1
( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {texture} 0 0 0 1 1
}}"#
    )
}

/// One entity. `properties` are extra `"key" "value"` lines; no blank line,
/// which would make the `.map` parser drop the remaining entities.
fn entity(classname: &str, properties: &str, brushes: &[String]) -> String {
    let mut lines = vec!["{".to_string(), format!("\"classname\" \"{classname}\"")];
    lines.extend(properties.lines().map(str::to_owned));
    lines.extend(brushes.iter().cloned());
    lines.push("}".to_string());
    lines.join("\n")
}

/// A worldspawn box (entity 0) followed by `entities`, so the first one passed
/// is map entity 1.
fn map(entities: &[String]) -> String {
    let world = box_brush([128, 128, 0], [256, 256, 128], "world_tex");
    format!(
        "{{\n\"classname\" \"worldspawn\"\n{world}\n}}\n{}",
        entities.join("\n")
    )
}

fn parse(map_text: &str) -> anyhow::Result<MapData> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "postretro-ghost-link-map-{}-{id}.map",
        std::process::id(),
    ));
    std::fs::write(&path, format!("{}\n", map_text.trim()))?;
    let result = parse_map_file(&path, MapFormat::IdTech2);
    let _ = std::fs::remove_file(&path);
    result
}

fn destination(name: &str, origin: &str, angles: &str) -> String {
    entity(
        "teleport_destination",
        &format!("\"name\" \"{name}\"\n\"origin\" \"{origin}\"\n\"angles\" \"{angles}\""),
        &[],
    )
}

fn link(extra: &str) -> String {
    entity(
        "ghost_link",
        extra,
        &[
            box_brush([0, 0, 0], [32, 32, 64], "ghost_tex"),
            box_brush([64, 0, 0], [96, 32, 64], "ghost_tex"),
        ],
    )
}

/// Link (entity 1) first, so both destinations are forward references.
fn fixture(extra: &str) -> String {
    map(&[
        link(extra),
        destination("copy_a", "0 0 0", "0 0 0"),
        destination("copy_b", "512 0 0", "0 90 0"),
    ])
}

fn error(map_text: &str) -> String {
    parse(map_text)
        .map(|_| ())
        .expect_err("authoring must fail")
        .to_string()
}

#[test]
fn ghost_link_parser_round_trips_records_tags_members_and_style_through_the_section() {
    let map = parse(&fixture(
        "\"landmark\" \"copy_a\"\n\"target\" \"copy_b\"\n\"enabled_on_spawn\" \"0\"\n\
         \"ghost_members\" \"movers players movers\"\n\"ghost_tags\" \"barrier lift barrier\"\n\
         \"ghost_style\" \"echo\"\n\"_tags\" \"vault_echo east\"",
    ))
    .unwrap();
    assert_eq!(
        map.brush_volumes.len(),
        1,
        "link brushes are peeled before BSP; only the world box remains"
    );
    assert_eq!(map.map_entities.len(), 1, "destinations emit no map entity");
    let companion = &map.map_entities[0];
    assert_eq!(companion.classname, "ghost_link");
    assert_eq!(companion.tags, ["vault_echo", "east"]);
    assert!(
        companion
            .key_values
            .contains(&(GHOST_LINK_SOURCE_ENTITY_KEY.to_owned(), "1".to_owned()))
    );
    assert!(
        companion
            .key_values
            .contains(&("ghost_style".to_owned(), "echo".to_owned()))
    );
    let unit = 0.0254f64;
    // Union of x 0..96, y 0..32, z 0..64 (Quake), centred, in engine axes.
    let centre = glam::DVec3::new(-16.0 * unit, 32.0 * unit, -48.0 * unit);
    assert!(
        companion.origin.abs_diff_eq(centre, 1e-6),
        "{:?}",
        companion.origin
    );

    let decoded = GhostLinksSection::from_bytes(&map.ghost_links.to_bytes())
        .expect("compiled section decodes");
    assert_eq!(decoded, map.ghost_links);
    let link = &decoded.links[0];
    assert_eq!(link.source_entity, 1);
    assert!(!link.enabled_on_spawn);
    assert_eq!(link.members, GhostMembers::PLAYERS | GhostMembers::MOVERS);
    assert_eq!(link.style, "echo");
    assert_eq!(link.ghost_tags, ["barrier", "lift"]);
    assert_eq!(link.brushes.len(), 2);
    assert_eq!(link.landmark_origin, [0.0; 3]);
    assert_eq!(link.landmark_yaw, 0.0);
    let unit = 0.0254f32;
    assert_eq!(link.target_origin, [0.0, 0.0, -512.0 * unit]);
    assert_eq!(link.target_yaw, 90.0f32.to_radians());
}

#[test]
fn ghost_link_parser_defaults_to_all_categories_armed_floor_style_and_no_tags() {
    for members in ["", "\n\"ghost_members\" \"  \""] {
        let map = parse(&fixture(&format!(
            "\"landmark\" \"copy_a\"\n\"target\" \"copy_b\"{members}"
        )))
        .unwrap();
        let decoded = GhostLinksSection::from_bytes(&map.ghost_links.to_bytes()).unwrap();
        let link = &decoded.links[0];
        assert_eq!(link.members, GhostMembers::all(), "members `{members}`");
        assert!(link.enabled_on_spawn);
        assert!(link.style.is_empty());
        assert!(link.ghost_tags.is_empty());
    }
}

#[test]
fn ghost_link_parser_rejects_invalid_authoring_naming_the_link() {
    let label = "entity 1 'ghost_link'";
    for (extra, expected) in [
        ("\"target\" \"copy_b\"", "requires landmark"),
        ("\"landmark\" \"copy_a\"", "requires non-blank `target`"),
        (
            "\"landmark\" \"copy_a\"\n\"target\" \"nowhere\"",
            "unknown target `nowhere`",
        ),
        (
            "\"landmark\" \"nowhere\"\n\"target\" \"copy_b\"",
            "unknown landmark `nowhere`",
        ),
        (
            "\"landmark\" \"copy_a\"\n\"target\" \"copy_a\"",
            "landmark must differ from target",
        ),
        (
            "\"landmark\" \"copy_a\"\n\"target\" \"copy_b\"\n\"enabled_on_spawn\" \"2\"",
            "enabled_on_spawn must be 0/1, got `2`",
        ),
        (
            "\"landmark\" \"copy_a\"\n\"target\" \"copy_b\"\n\"ghost_members\" \"players ghosts\"",
            "unknown ghost_members category `ghosts`",
        ),
    ] {
        let error = error(&fixture(extra));
        assert!(error.contains(&format!("{label} {expected}")), "{error}");
    }
    let brushless = map(&[
        entity(
            "ghost_link",
            "\"landmark\" \"copy_a\"\n\"target\" \"copy_b\"",
            &[],
        ),
        destination("copy_a", "0 0 0", "0 0 0"),
        destination("copy_b", "512 0 0", "0 0 0"),
    ]);
    let error = error(&brushless);
    assert!(
        error.contains(&format!("{label} has no brushes")),
        "{error}"
    );
}

#[test]
fn ghost_link_and_teleporter_resolve_against_one_destination_table() {
    let gate = entity(
        "trigger_teleport",
        "\"mode\" \"seamless\"\n\"landmark\" \"copy_a\"\n\"target\" \"copy_b\"",
        &[box_brush([0, 64, 0], [32, 96, 64], "teleport_tex")],
    );
    let map = parse(&map(&[
        link("\"landmark\" \"copy_a\"\n\"target\" \"copy_b\""),
        gate,
        destination("copy_a", "0 0 0", "0 0 0"),
        destination("copy_b", "512 0 0", "0 90 0"),
    ]))
    .unwrap();
    let link = &map.ghost_links.links[0];
    let teleporter = &map.teleporters.teleporters[0];
    assert_eq!(link.target_origin, teleporter.destination_origin);
    assert_eq!(link.target_yaw, teleporter.destination_yaw);
    assert_eq!(link.landmark_origin, teleporter.landmark_origin);
    // Each region keeps its own companion, keyed by its own source entity.
    let classnames: Vec<_> = map
        .map_entities
        .iter()
        .map(|e| e.classname.as_str())
        .collect();
    assert_eq!(classnames, ["ghost_link", "trigger_teleport"]);
}
