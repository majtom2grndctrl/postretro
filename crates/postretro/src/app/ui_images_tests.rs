// Mod UI image decode, the eager / loading-only split, and its warnings.
// See: context/lib/ui.md §5

use super::*;
use postretro_scripting_core::ui::descriptor::AnchoredTree;
use postretro_test_log_capture::LogCapture;

/// A mod root holding one 3×2 PNG at `ui/good.png` and one file that is not
/// a PNG at `ui/broken.png`.
fn mod_root() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("ui")).unwrap();
    image::RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255]))
        .save(root.path().join("ui/good.png"))
        .unwrap();
    std::fs::write(root.path().join("ui/broken.png"), b"not a png").unwrap();
    root
}

fn images(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, path)| (key.to_string(), path.to_string()))
        .collect()
}

/// A registered tree drawing `background` (if any) beneath an image of each
/// of `assets`.
pub(crate) fn tree(name: &str, background: Option<&str>, assets: &[&str]) -> RegisteredUiTree {
    let children: Vec<String> = assets
        .iter()
        .map(|asset| format!(r#"{{ "kind": "image", "asset": "{asset}" }}"#))
        .collect();
    let background = background
        .map(|image| format!(r#""background": {{ "image": "{image}" }},"#))
        .unwrap_or_default();
    let json = format!(
        r#"{{ "anchor": "center", "offset": [0.0, 0.0], {background}
             "root": {{ "kind": "vstack", "gap": 0.0, "padding": 0.0, "align": "start",
                        "children": [{}] }} }}"#,
        children.join(",")
    );
    let tree: AnchoredTree = serde_json::from_str(&json).expect("test tree parses");
    RegisteredUiTree {
        name: name.to_string(),
        tree,
        always_on: false,
        hide_below: false,
    }
}

fn template(asset: &str) -> PresentationTemplate {
    serde_json::from_str(&format!(
        r#"{{ "id": "hit", "lifetimeMs": 500,
             "root": {{ "kind": "image", "asset": "{asset}" }},
             "motion": {{ "rise": 0.0, "easing": "linear" }},
             "fade": {{ "startMs": 0 }}, "spawnScatter": {{ "radius": 0.0 }} }}"#
    ))
    .expect("test template parses")
}

pub(crate) fn map(id: &str, loading_tree: &[&str]) -> ModMapEntry {
    ModMapEntry {
        id: id.to_string(),
        path: format!("maps/{id}.prl"),
        name: id.to_string(),
        tags: Vec::new(),
        loading_tree: loading_tree.iter().map(|name| name.to_string()).collect(),
    }
}

fn committed(
    entries: &[(&str, &str)],
    trees: &[RegisteredUiTree],
    templates: &[PresentationTemplate],
    maps: &[ModMapEntry],
    mod_pool: &[&str],
) -> ModUiImages {
    let mod_pool: Vec<String> = mod_pool.iter().map(|name| name.to_string()).collect();
    let mut state = ModUiImages::default();
    state.commit(
        images(entries),
        ManifestImageRefs::from_manifest(trees, templates, maps, &mod_pool),
    );
    state
}

#[test]
fn ui_images_decode_good_entries_and_warn_naming_the_bad_ones() {
    let root = mod_root();
    let capture = LogCapture::start();
    let decoded = decode_mod_ui_images(
        root.path(),
        &images(&[
            ("art/good", "ui/good.png"),
            ("art/missing", "ui/missing.png"),
            ("art/broken", "ui/broken.png"),
        ]),
    );

    assert_eq!(decoded.len(), 1, "only the decodable entry loads");
    assert_eq!(decoded[0].key, "art/good");
    assert_eq!((decoded[0].width, decoded[0].height), (3, 2));
    assert_eq!(decoded[0].rgba.len(), 3 * 2 * 4);
    capture.assert_logged_once(log::Level::Warn, "uiImages.art/missing");
    capture.assert_logged_once(log::Level::Warn, "uiImages.art/broken");
}

/// The manifest parse already drops `engine/` names; the loader refuses
/// them again, so no mod entry can replace an engine image.
#[test]
fn ui_images_never_load_an_engine_prefixed_key() {
    let root = mod_root();
    let capture = LogCapture::start();
    let decoded = decode_mod_ui_images(
        root.path(),
        &images(&[
            (SPLASH_LOGO_IMAGE, "ui/good.png"),
            ("art/good", "ui/good.png"),
        ]),
    );
    let keys: Vec<&str> = decoded.iter().map(|image| image.key.as_str()).collect();
    assert_eq!(keys, ["art/good"]);
    capture.assert_logged_once(log::Level::Warn, "reserved `engine/` prefix");
}

#[test]
fn ui_images_shadowed_by_glyph_art_warn_once_per_key() {
    let mod_keys: HashSet<String> = ["ui/glyphs/kbm/space", "art/logo"]
        .into_iter()
        .map(String::from)
        .collect();
    let glyph_keys: HashSet<String> = ["ui/glyphs/kbm/space", "ui/glyphs/kbm/e"]
        .into_iter()
        .map(String::from)
        .collect();
    let mut warned = HashSet::new();
    assert_eq!(
        glyph_collisions(&mod_keys, &glyph_keys, &mut warned),
        ["ui/glyphs/kbm/space"]
    );
    assert!(
        glyph_collisions(&mod_keys, &glyph_keys, &mut warned).is_empty(),
        "a reload that keeps the collision does not warn again"
    );
}

#[test]
fn ui_images_reload_when_the_map_or_staged_generation_changes() {
    let mut state = ModUiImages::default();
    assert!(!state.is_current(None), "nothing has loaded yet");
    state.commit(images(&[("art/a", "ui/a.png")]), Default::default());
    state.loaded = Some((state.eager.clone(), None));
    assert!(state.is_current(None));
    assert!(!state.is_current(Some(1)), "a committed reload reloads");
    state.commit(images(&[("art/b", "ui/b.png")]), Default::default());
    assert!(!state.is_current(None), "a changed map reloads");
    state.forget_uploads();
    assert!(state.loaded.is_none() && !state.engine_loaded);
}

/// Every loading-candidate source defers an image only it names, whether as a
/// background or as an `Image` asset.
#[test]
fn ui_images_referenced_only_by_a_loading_candidate_are_deferred() {
    let state = committed(
        &[
            ("shots/e1m1", "ui/e1m1.png"),
            ("shots/plain", "ui/plain.png"),
            ("shots/fallback", "ui/fallback.png"),
            ("art/logo", "ui/logo.png"),
        ],
        &[
            tree("load.e1m1", Some("shots/e1m1"), &[]),
            tree("load.plain", None, &["shots/plain"]),
            tree(LOADING_SCREEN_NAME, Some("shots/fallback"), &[]),
            tree("hud", None, &["art/logo"]),
        ],
        &[],
        &[map("e1m1", &["load.e1m1"])],
        &["load.plain"],
    );

    for key in ["shots/e1m1", "shots/plain", "shots/fallback"] {
        assert!(state.deferred_path(key).is_some(), "{key} is loading-only");
        assert!(
            !state.eager.contains_key(key),
            "{key} does not load eagerly"
        );
    }
    assert_eq!(state.deferred_path("shots/e1m1"), Some("ui/e1m1.png"));
    assert_eq!(
        state.eager.keys().collect::<Vec<_>>(),
        ["art/logo"],
        "an image only a non-loading tree names stays eager"
    );
}

#[test]
fn ui_images_also_named_by_a_non_loading_tree_or_template_stay_eager() {
    let state = committed(
        &[
            ("shots/menu", "ui/menu.png"),
            ("art/hit", "ui/hit.png"),
            ("shots/unused", "ui/unused.png"),
        ],
        &[
            tree("load.e1m1", Some("shots/menu"), &["art/hit"]),
            tree("frontend", Some("shots/menu"), &[]),
        ],
        &[template("art/hit")],
        &[map("e1m1", &["load.e1m1"])],
        &[],
    );

    assert_eq!(
        state.deferred_path("shots/menu"),
        None,
        "the menu draws it too"
    );
    assert_eq!(
        state.deferred_path("art/hit"),
        None,
        "a template draws it too"
    );
    assert_eq!(
        state.deferred_path("shots/unused"),
        None,
        "an image no loading tree names is not loading-only"
    );
    assert_eq!(state.eager.len(), 3, "every entry loads eagerly");
}

#[test]
fn ui_images_unknown_tree_background_warns_once_per_commit_naming_the_tree() {
    let mut state = committed(
        &[("shots/e1m1", "ui/e1m1.png")],
        &[
            tree("load.e1m1", Some("shots/e1m1"), &[]),
            tree("load.typo", Some("shots/e1mi"), &[]),
            tree("load.engine", Some(SPLASH_LOGO_IMAGE), &[]),
            tree("load.glyph", Some("ui/glyphs/kbm/space"), &[]),
        ],
        &[],
        &[],
        &[],
    );
    let glyph_keys: HashSet<String> = ["ui/glyphs/kbm/space".to_string()].into();
    let capture = LogCapture::start();

    state.warn_unknown_backgrounds(&glyph_keys);
    state.warn_unknown_backgrounds(&glyph_keys);

    capture.assert_logged_once(
        log::Level::Warn,
        "tree `load.typo` background names `shots/e1mi`",
    );
    for tree in ["load.e1m1", "load.engine", "load.glyph"] {
        capture.assert_not_logged(log::Level::Warn, &format!("tree `{tree}`"));
    }
}

#[test]
fn ui_images_loading_only_key_shadowed_by_glyph_art_warns() {
    let mut state = committed(
        &[("ui/glyphs/kbm/space", "ui/space.png")],
        &[tree("load.e1m1", Some("ui/glyphs/kbm/space"), &[])],
        &[],
        &[map("e1m1", &["load.e1m1"])],
        &[],
    );
    let capture = LogCapture::start();
    state.warn_glyph_collisions(&["ui/glyphs/kbm/space".to_string()].into());
    capture.assert_logged_once(
        log::Level::Warn,
        "uiImages.ui/glyphs/kbm/space shares its key",
    );
}
