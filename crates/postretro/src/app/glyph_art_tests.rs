// Glyph tests: per-frame resolution into image, text or nothing, following the
// device family and rebinding; and the mod art loader.
// See: context/lib/ui.md §4

use gilrs::Button;
use postretro_ui::descriptor::{AnchoredTree, Widget};
use postretro_ui::modal_stack::ScopeTier;

use super::*;
use crate::input::{AuthorLayer, Command, DeviceClass, GlyphDirs, PhysicalInput, PlayerLayer};
use crate::startup::lifecycle::tests::test_app;

fn glyph_tree() -> AnchoredTree {
    serde_json::from_value(serde_json::json!({
        "anchor": "center",
        "offset": [0.0, 0.0],
        "root": {
            "kind": "hstack", "gap": 8.0, "padding": 0.0, "align": "center",
            "children": [
                { "kind": "glyph", "command": "nav_confirm", "id": "confirmGlyph" },
                { "kind": "glyph", "command": "dash" },
                { "kind": "text", "content": "SELECT", "fontSize": 16, "color": "ok" },
            ],
        },
    }))
    .unwrap()
}

fn app_with_glyphs() -> App {
    let mut app = test_app();
    let session = app.session.as_mut().unwrap();
    session.bindings.set_author_layer(AuthorLayer {
        glyphs: GlyphDirs {
            keyboard_mouse: Some("ui/glyphs/kbm".into()),
            xbox: Some("ui/glyphs/xbox".into()),
            playstation: Some("ui/glyphs/ps".into()),
            nintendo: None,
        },
        ..AuthorLayer::default()
    });
    session.glyph_art.keys = ["ui/glyphs/xbox/south", "ui/glyphs/xbox/west", "ui/glyphs/ps/south"]
        .into_iter()
        .map(str::to_string)
        .collect();
    app.refresh_effective_bindings();
    app
}

/// The resolved children of the test tree's row.
fn resolve(app: &App) -> Vec<Widget> {
    let mut snapshot = postretro_ui::UiReadSnapshot::with_trees(
        vec![postretro_ui::UiTreeEntry {
            name: "row".into(),
            tier: ScopeTier::Mod,
            descriptor: glyph_tree(),
            capture_mode: postretro_ui::descriptor::CaptureMode::Passthrough,
            on_commit: None,
        }],
        Default::default(),
        Default::default(),
        0.0,
        None,
    );
    resolve_snapshot_glyphs(&mut snapshot, app.session.as_ref().unwrap());
    let Widget::HStack(row) = &snapshot.trees[0].descriptor.root else {
        panic!("row is an hstack");
    };
    row.children.clone()
}

fn image_asset(widget: &Widget) -> Option<&str> {
    match widget {
        Widget::Image(image) => Some(image.asset.as_str()),
        _ => None,
    }
}

fn set_family(app: &mut App, vendor: Option<u16>) {
    let tracker = &mut app.session.as_mut().unwrap().device_family;
    tracker.note_pad(vendor);
    tracker.end_frame();
}

#[test]
fn a_glyph_draws_the_familys_art_and_an_irrelevant_command_draws_nothing() {
    let mut app = app_with_glyphs();
    set_family(&mut app, Some(0x045E));
    let children = resolve(&app);
    assert_eq!(image_asset(&children[0]), Some("ui/glyphs/xbox/south"));
    let Widget::Image(image) = &children[0] else {
        unreachable!()
    };
    assert_eq!(image.id.as_deref(), Some("confirmGlyph"));
    assert!(
        matches!(children[1], Widget::Spacer(_)),
        "dash is irrelevant without a dash descriptor"
    );
    assert!(matches!(children[2], Widget::Text(_)));

    set_family(&mut app, Some(0x054C));
    assert_eq!(image_asset(&resolve(&app)[0]), Some("ui/glyphs/ps/south"));
}

#[test]
fn missing_art_draws_the_input_name() {
    let mut app = app_with_glyphs();
    // Keyboard-and-mouse art is declared but none loaded.
    app.session
        .as_mut()
        .unwrap()
        .device_family
        .note_keyboard_mouse();
    app.session.as_mut().unwrap().device_family.end_frame();
    match &resolve(&app)[0] {
        Widget::Text(text) => assert_eq!(text.content, "ENTER"),
        other => panic!("expected the input name, got {other:?}"),
    }
}

#[test]
fn rebinding_confirm_changes_its_glyph_on_the_next_frame() {
    let mut app = app_with_glyphs();
    set_family(&mut app, None);
    assert_eq!(image_asset(&resolve(&app)[0]), Some("ui/glyphs/xbox/south"));
    let mut player = PlayerLayer::default();
    player.rows.insert(
        (Command::NavConfirm, DeviceClass::Gamepad),
        vec![Some(PhysicalInput::GamepadButton(Button::West))],
    );
    app.session.as_mut().unwrap().bindings.set_player_layer(player);
    app.refresh_effective_bindings();
    assert_eq!(image_asset(&resolve(&app)[0]), Some("ui/glyphs/xbox/west"));
}

#[test]
fn glyph_directories_stay_inside_the_mod() {
    assert!(is_mod_relative("ui/glyphs/xbox"));
    assert!(is_mod_relative("./ui/glyphs"));
    assert!(!is_mod_relative("../other-mod/glyphs"));
    assert!(!is_mod_relative("ui/../../glyphs"));
    assert!(!is_mod_relative("/abs/glyphs"));
    assert!(read_glyph_dir(Path::new("."), "../escape").is_empty());
}

#[test]
fn the_dev_mods_art_decodes_under_input_keyed_names() {
    let mod_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
    let images = read_glyph_dir(&mod_root, "ui/glyphs/xbox");
    let south = images
        .iter()
        .find(|(key, ..)| key == "ui/glyphs/xbox/south")
        .expect("the dev mod ships xbox south art");
    assert_eq!((south.2, south.3), (48, 48));
    assert_eq!(south.1.len(), 48 * 48 * 4);
    for family in ["kbm", "xbox", "ps", "nx"] {
        assert!(
            !read_glyph_dir(&mod_root, &format!("ui/glyphs/{family}")).is_empty(),
            "{family} art ships"
        );
    }
}
