use crate::map_data::MapData;
use crate::map_format::MapFormat;
use crate::parse::parse_map_file;
use log::Level;
use postretro_level_format::seams::{
    SEAM_CLASSNAME, SEAM_SOURCE_ENTITY_KEY, SeamActivation, SeamMode, SeamRecord,
};
use postretro_test_log_capture::LogCapture;

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
        "postretro-level-seam-map-{}-{id}.map",
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

fn player_spawn() -> String {
    entity(
        "player_spawn",
        "\"origin\" \"192 192 24\"\n\"angles\" \"0 0 0\"",
        &[],
    )
}

/// A two-brush seam: Quake union x 0..96, y 0..32, z 0..64.
fn seam(extra: &str) -> String {
    entity(
        SEAM_CLASSNAME,
        extra,
        &[
            box_brush([0, 0, 0], [32, 32, 64], "seam_tex"),
            box_brush([64, 0, 0], [96, 32, 64], "seam_tex"),
        ],
    )
}

/// Seam (entity 1) first, so the landmark is a forward reference.
fn fixture(extra: &str) -> String {
    map(&[
        seam(extra),
        destination("arrive", "256 128 64", "0 90 0"),
        player_spawn(),
    ])
}

fn only_seam(map: &MapData) -> &SeamRecord {
    assert_eq!(map.seams.seams.len(), 1, "one seam");
    &map.seams.seams[0]
}

const UNIT: f32 = 0.0254;

/// The two-brush seam's union AABB centre in engine metres: Quake (48, 16, 32).
fn bounds_centre() -> [f32; 3] {
    [-16.0 * UNIT, 32.0 * UNIT, -48.0 * UNIT]
}

fn assert_close(actual: [f32; 3], expected: [f32; 3]) {
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() < 1e-5, "{actual:?} vs {expected:?}");
    }
}

#[test]
fn seam_parser_resolves_authored_landmark_tags_companion_and_peels_brushes() {
    let parsed = parse(&fixture(
        "\"name\" \"pair_door\"\n\"target\" \"seam-pair-b\"\n\"activation\" \"use\"\n\
         \"mode\" \"classic\"\n\"landmark\" \"arrive\"\n\"enabled_on_spawn\" \"0\"\n\
         \"on_arrive\" \"seam.arrived\"\n\"_tags\" \"airlock exit\"",
    ))
    .unwrap();
    let record = only_seam(&parsed);
    assert_eq!(record.source_entity, 1);
    assert_eq!(record.name, "pair_door");
    assert_eq!(record.target, "seam-pair-b");
    assert_eq!(record.activation, SeamActivation::Use);
    assert_eq!(record.mode, SeamMode::Classic);
    assert!(!record.enabled_on_spawn);
    assert_eq!(record.brushes.len(), 2);
    assert_eq!(record.on_arrive, "seam.arrived");
    assert_eq!(
        record.landmark_origin,
        [-128.0 * UNIT, 64.0 * UNIT, -256.0 * UNIT],
        "the authored destination's origin"
    );
    assert_eq!(record.landmark_yaw, 90.0f32.to_radians());

    let companion = parsed
        .map_entities
        .iter()
        .find(|e| e.classname == SEAM_CLASSNAME)
        .expect("each seam writes an id-29 companion");
    assert_eq!(companion.tags, ["airlock", "exit"]);
    for key_value in [(SEAM_SOURCE_ENTITY_KEY, "1"), ("name", "pair_door")] {
        assert!(
            companion
                .key_values
                .contains(&(key_value.0.to_owned(), key_value.1.to_owned())),
            "{key_value:?} in {:?}",
            companion.key_values
        );
    }
    assert_close(companion.origin.as_vec3().to_array(), bounds_centre());

    // Peeled before BSP: no world brush, trigger, environment volume or
    // teleporter comes from the seam's brushes.
    let base = parse(&map(&[player_spawn()])).unwrap();
    assert_eq!(parsed.brush_volumes.len(), base.brush_volumes.len());
    assert!(parsed.trigger_volumes.is_empty());
    assert!(parsed.environment_volumes.is_empty());
    assert!(parsed.teleporters.teleporters.is_empty());
}

#[test]
fn seam_without_landmark_uses_bounds_centre_at_yaw_zero_and_defaults() {
    let parsed = parse(&fixture("\"name\" \"door\"\n\"target\" \"next\"")).unwrap();
    let record = only_seam(&parsed);
    assert_close(record.landmark_origin, bounds_centre());
    assert_eq!(record.landmark_yaw, 0.0);
    assert_eq!(record.activation, SeamActivation::Touch);
    assert_eq!(record.mode, SeamMode::Classic);
    assert!(record.enabled_on_spawn);
    assert_eq!(record.on_arrive, "");
}

#[test]
fn seam_blank_optional_keys_take_their_defaults() {
    let absent = parse(&fixture("\"name\" \"door\"\n\"target\" \"next\"")).unwrap();
    for blank in ["", "  "] {
        let cleared = parse(&fixture(&format!(
            "\"name\" \"door\"\n\"target\" \"next\"\n\"activation\" \"{blank}\"\n\
             \"mode\" \"{blank}\"\n\"landmark\" \"{blank}\"\n\
             \"enabled_on_spawn\" \"{blank}\"\n\"on_arrive\" \"{blank}\"\n\"_tags\" \"{blank}\""
        )))
        .unwrap_or_else(|e| panic!("cleared keys must fall back to defaults: {e}"));
        assert_eq!(only_seam(&cleared), only_seam(&absent));
    }
}

#[test]
fn seam_parser_rejects_invalid_authoring_naming_entity() {
    for (extra, message) in [
        ("\"target\" \"next\"", "requires non-blank `name`"),
        (
            "\"name\" \" \"\n\"target\" \"next\"",
            "requires non-blank `name`",
        ),
        ("\"name\" \"door\"", "requires non-blank `target`"),
        (
            "\"name\" \"door\"\n\"target\" \"\"",
            "requires non-blank `target`",
        ),
        (
            "\"name\" \"door\"\n\"target\" \"next\"\n\"activation\" \"shove\"",
            "unknown activation `shove`",
        ),
        (
            "\"name\" \"door\"\n\"target\" \"next\"\n\"mode\" \"warp\"",
            "unknown mode `warp`",
        ),
        (
            "\"name\" \"door\"\n\"target\" \"next\"\n\"enabled_on_spawn\" \"2\"",
            "enabled_on_spawn must be 0/1",
        ),
        (
            "\"name\" \"door\"\n\"target\" \"next\"\n\"landmark\" \"missing\"",
            "unknown landmark `missing`",
        ),
    ] {
        let error = parse(&fixture(extra))
            .map(|_| ())
            .expect_err("authoring must fail")
            .to_string();
        assert!(
            error.contains(&format!("entity 1 '{SEAM_CLASSNAME}' {message}")),
            "{error}"
        );
    }

    let duplicate = map(&[
        seam("\"name\" \"door\"\n\"target\" \"next\""),
        seam("\"name\" \"door\"\n\"target\" \"other\""),
        player_spawn(),
    ]);
    let error = parse(&duplicate).err().unwrap().to_string();
    assert!(
        error.contains("entity 2 'level_seam' duplicate seam name `door`"),
        "{error}"
    );

    let point = map(&[
        entity(
            SEAM_CLASSNAME,
            "\"name\" \"door\"\n\"target\" \"next\"",
            &[],
        ),
        player_spawn(),
    ]);
    let error = parse(&point).err().unwrap().to_string();
    assert!(
        error.contains("entity 1 'level_seam' has no brushes"),
        "{error}"
    );
}

#[test]
fn seam_in_a_map_without_player_spawn_warns_once() {
    let capture = LogCapture::start();
    let map_text = map(&[
        seam("\"name\" \"door\"\n\"target\" \"next\""),
        seam("\"name\" \"hatch\"\n\"target\" \"next\""),
    ]);
    parse(&map_text).expect("a missing player_spawn only warns");
    capture.assert_logged_once(Level::Warn, "no player_spawn");
    capture.assert_logged_once(Level::Warn, "[Compiler] entity 1 'level_seam' `door`");

    capture.clear();
    parse(&fixture("\"name\" \"door\"\n\"target\" \"next\"")).unwrap();
    parse(&map(&[])).unwrap();
    capture.assert_not_logged(Level::Warn, "no player_spawn");
}

#[test]
fn seamless_seam_compiles_as_written_without_warning() {
    let capture = LogCapture::start();
    let parsed = parse(&fixture(
        "\"name\" \"door\"\n\"target\" \"next\"\n\"mode\" \"seamless\"",
    ))
    .unwrap();
    assert_eq!(only_seam(&parsed).mode, SeamMode::Seamless);
    capture.assert_not_logged(Level::Warn, "level_seam");
    capture.assert_not_logged(Level::Warn, "seamless");
}

/// One `level_seam` property as `postretro.fgd` declares it.
struct FgdProperty {
    key: String,
    default: String,
    choices: Vec<String>,
}

/// The quoted and bare fields after `key(type)`, up to a choices `=`.
fn fgd_fields(rest: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut chars = rest.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            '=' => break,
            ':' | ' ' | '\t' => {
                chars.next();
            }
            '"' => {
                chars.next();
                fields.push(chars.by_ref().take_while(|&c| c != '"').collect());
            }
            _ => {
                let mut field = String::new();
                while let Some(&c) = chars.peek() {
                    if matches!(c, ' ' | '\t' | ':' | '=') {
                        break;
                    }
                    field.push(c);
                    chars.next();
                }
                fields.push(field);
            }
        }
    }
    fields
}

fn fgd_level_seam() -> Vec<FgdProperty> {
    let fgd_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sdk/TrenchBroom/postretro.fgd"
    );
    let fgd = std::fs::read_to_string(fgd_path).expect("read committed postretro.fgd");
    assert!(
        fgd.contains(&format!(
            "@SolidClass\n    color(240 160 40)\n    = {SEAM_CLASSNAME} :"
        )),
        "`level_seam` is a brush entity"
    );
    let body = fgd
        .split_once(&format!("= {SEAM_CLASSNAME} :"))
        .and_then(|(_, rest)| rest.split_once("\n]"))
        .map(|(body, _)| body.to_owned())
        .expect("postretro.fgd declares a `level_seam` class with a closed body");
    let mut properties: Vec<FgdProperty> = Vec::new();
    let mut in_choices = false;
    for line in body.lines().map(str::trim) {
        if line == "[" {
            in_choices = !properties.is_empty();
        } else if line == "]" {
            in_choices = false;
        } else if in_choices {
            let option = fgd_fields(line).into_iter().next().expect("a choice value");
            properties.last_mut().unwrap().choices.push(option);
        } else if let Some((key, rest)) = line.split_once('(')
            && !key.is_empty()
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            let (_, rest) = rest.split_once(')').expect("a property type");
            let fields = fgd_fields(rest);
            properties.push(FgdProperty {
                key: key.to_owned(),
                default: fields.get(1).cloned().unwrap_or_default(),
                choices: Vec::new(),
            });
        }
    }
    properties
}

// AC "FGD defaults equal the compiler's, by test": a seam authored with every
// FGD default compiles to the same record as one that omits every optional
// key, and every FGD choice compiles.
#[test]
fn fgd_level_seam_defaults_equal_the_compilers() {
    let properties = fgd_level_seam();
    let keys: Vec<&str> = properties.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "name",
            "target",
            "activation",
            "mode",
            "landmark",
            "enabled_on_spawn",
            "on_arrive",
            "_tags"
        ]
    );
    let required = |key: &str| match key {
        "name" => Some("door"),
        "target" => Some("next"),
        _ => None,
    };
    let authored: Vec<String> = properties
        .iter()
        .map(|p| {
            let value = match required(p.key.as_str()) {
                Some(value) => value,
                None => p.default.as_str(),
            };
            format!("\"{}\" \"{value}\"", p.key)
        })
        .collect();
    let with_defaults = parse(&fixture(&authored.join("\n"))).unwrap();
    let absent = parse(&fixture("\"name\" \"door\"\n\"target\" \"next\"")).unwrap();
    assert_eq!(only_seam(&with_defaults), only_seam(&absent));
    for property in properties
        .iter()
        .filter(|p| required(p.key.as_str()).is_some())
    {
        assert_eq!(property.default, "", "`{}` has no default", property.key);
    }

    let choices: Vec<(&str, Vec<&str>)> = properties
        .iter()
        .filter(|p| !p.choices.is_empty())
        .map(|p| {
            (
                p.key.as_str(),
                p.choices.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        choices,
        [
            ("activation", vec!["touch", "use"]),
            ("mode", vec!["classic", "seamless"]),
            ("enabled_on_spawn", vec!["0", "1"]),
        ]
    );
    for (key, options) in choices {
        for option in options {
            parse(&fixture(&format!(
                "\"name\" \"door\"\n\"target\" \"next\"\n\"{key}\" \"{option}\""
            )))
            .unwrap_or_else(|e| panic!("FGD choice {key} `{option}` must compile: {e}"));
        }
    }
}
