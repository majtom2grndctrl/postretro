use super::*;
use log::Level;
use postretro_test_log_capture::LogCapture;

const PATH: &str = "mod/materials.toml";

fn parse(text: &str) -> MaterialsToml {
    MaterialsToml::parse(PathBuf::from(PATH), text)
}

fn emissive(parsed: &MaterialsToml, name: &str) -> Option<f32> {
    parsed
        .entry(&normalize_map_texture_name(name))
        .and_then(|entry| entry.emissive)
}

#[test]
fn texture_tables_set_emissive_silently() {
    let capture = LogCapture::start();
    let parsed = parse(
        "# Glow strengths\n\
         \"inline/sign\" = { emissive = 0 }\n\
         [\"neon/glow_panel\"]\n\
         emissive = 4.0\n\
         \n\
         [\"signs/bar\"]\n\
         emissive = 2\n",
    );
    assert_eq!(emissive(&parsed, "neon/glow_panel"), Some(4.0));
    assert_eq!(
        emissive(&parsed, "signs/bar"),
        Some(2.0),
        "an integer is a number"
    );
    assert_eq!(emissive(&parsed, "inline/sign"), Some(0.0), "0 is valid");
    assert_eq!(parsed.entry("signs/bar").unwrap().line, 6);
    capture.assert_nothing_above(Level::Info);
}

#[test]
fn an_absent_file_is_silent_and_holds_no_entry() {
    let capture = LogCapture::start();
    let root = tempfile::tempdir().unwrap();
    let parsed = MaterialsToml::load(root.path());
    assert!(parsed.entry("signs/bar").is_none());
    assert_eq!(parsed.path(), root.path().join(MATERIALS_TOML));
    capture.assert_nothing_above(Level::Info);
}

#[test]
fn load_reads_the_file_under_the_content_root() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join(MATERIALS_TOML),
        "[\"signs/bar\"]\nemissive = 3\n",
    )
    .unwrap();
    assert_eq!(
        emissive(&MaterialsToml::load(root.path()), "signs/bar"),
        Some(3.0)
    );
}

// Pin P53: a syntax error leaves every texture at its default.
#[test]
fn a_syntax_error_warns_naming_file_and_line_and_drops_every_entry() {
    let capture = LogCapture::start();
    let parsed = parse("[\"signs/bar\"]\nemissive = 4\n\n[\"broken\"\nemissive = 2\n");
    assert!(parsed.entry("signs/bar").is_none(), "no entry survives");
    capture.assert_logged_once(Level::Warn, "mod/materials.toml:4: syntax error:");
    capture.assert_logged_once(
        Level::Warn,
        "every texture takes its default emissive strength",
    );
}

// Pin P54: byte-identical headers are a TOML syntax error.
#[test]
fn a_repeated_identical_header_is_a_syntax_error() {
    let capture = LogCapture::start();
    let parsed = parse("[\"signs/bar\"]\nemissive = 4\n[\"signs/bar\"]\nemissive = 2\n");
    assert!(parsed.entry("signs/bar").is_none());
    capture.assert_logged_once(Level::Warn, "mod/materials.toml:3: syntax error:");
}

#[test]
fn an_unknown_key_warns_naming_file_and_line_while_the_tables_emissive_applies() {
    let capture = LogCapture::start();
    let parsed = parse("[\"signs/bar\"]\nglow = 9\nemissive = 4\n");
    assert_eq!(emissive(&parsed, "signs/bar"), Some(4.0));
    capture.assert_logged_once(
        Level::Warn,
        "mod/materials.toml:2: unknown key 'glow' in [\"signs/bar\"]; ignored",
    );
}

#[test]
fn a_non_number_value_warns_naming_file_and_line_and_reads_the_default() {
    let capture = LogCapture::start();
    let parsed = parse("[\"signs/bar\"]\n\nemissive = \"bright\"\n");
    assert!(parsed.entry("signs/bar").is_some());
    assert_eq!(
        emissive(&parsed, "signs/bar"),
        None,
        "the texture's default"
    );
    capture.assert_logged_once(
        Level::Warn,
        "mod/materials.toml:3: [\"signs/bar\"] emissive must be a number, found a string",
    );
}

#[test]
fn a_top_level_value_that_is_not_a_table_warns_and_is_ignored() {
    let capture = LogCapture::start();
    let parsed = parse("emissive = 4\n[\"signs/bar\"]\nemissive = 2\n");
    assert_eq!(emissive(&parsed, "signs/bar"), Some(2.0));
    capture.assert_logged_once(
        Level::Warn,
        "mod/materials.toml:1: top-level key 'emissive' is not a texture table; ignored",
    );
}

#[test]
fn bad_values_are_fixed_and_each_warns_by_key_name() {
    let capture = LogCapture::start();
    let parsed = parse(
        "[\"a\"]\nemissive = -2\n\
         [\"b\"]\nemissive = nan\n\
         [\"c\"]\nemissive = inf\n\
         [\"d\"]\nemissive = 1000\n\
         [\"e\"]\nemissive = 64\n\
         [\"f\"]\nemissive = -0.0\n",
    );
    assert_eq!(emissive(&parsed, "a"), Some(0.0), "negative reads 0");
    assert_eq!(emissive(&parsed, "b"), None, "non-finite reads the default");
    assert_eq!(emissive(&parsed, "c"), None, "non-finite reads the default");
    assert_eq!(
        emissive(&parsed, "d"),
        Some(EMISSIVE_VALUE_CEILING),
        "past the ceiling reads the ceiling"
    );
    assert_eq!(
        emissive(&parsed, "e"),
        Some(64.0),
        "the ceiling itself is valid"
    );
    assert_eq!(
        emissive(&parsed, "f").map(f32::to_bits),
        Some(0.0f32.to_bits()),
        "-0 stores as 0"
    );
    capture.assert_logged_once(
        Level::Warn,
        "mod/materials.toml:2: [\"a\"] emissive -2 is negative; reading 0",
    );
    capture.assert_logged_once(
        Level::Warn,
        "mod/materials.toml:4: [\"b\"] emissive NaN is not finite",
    );
    capture.assert_logged_once(
        Level::Warn,
        "mod/materials.toml:6: [\"c\"] emissive inf is not finite",
    );
    capture.assert_logged_once(
        Level::Warn,
        "mod/materials.toml:8: [\"d\"] emissive 1000 is above the ceiling 64; reading 64",
    );
    capture.assert_not_logged(Level::Warn, "[\"e\"]");
    capture.assert_not_logged(Level::Warn, "[\"f\"]");
}

// Pin P54: two keys that normalize alike warn once, naming both lines; the
// later wins.
#[test]
fn keys_that_normalize_alike_warn_once_naming_both_lines_and_the_later_wins() {
    let capture = LogCapture::start();
    let parsed = parse(
        "[\"signs/bar\"]\nemissive = 4\n\
         [\"Signs/Bar\"]\nemissive = 2\n",
    );
    assert_eq!(emissive(&parsed, "signs/bar"), Some(2.0));
    capture.assert_logged_once(
        Level::Warn,
        "mod/materials.toml: [\"signs/bar\"] (line 1) and [\"Signs/Bar\"] (line 3) name the same \
         texture 'signs/bar'; the later, line 3, wins",
    );

    // Lexical key order is not file order: the later line still wins.
    let capture_root = parse(
        "[\"textures/signs/bar\"]\nemissive = 3\n\
         [\"signs/bar\"]\nemissive = 5\n",
    );
    assert_eq!(emissive(&capture_root, "textures/signs/bar"), Some(5.0));
    capture.assert_logged_once(
        Level::Warn,
        "[\"textures/signs/bar\"] (line 1) and [\"signs/bar\"] (line 3) name the same texture",
    );
}
