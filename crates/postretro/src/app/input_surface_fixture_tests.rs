// The dev mod's input scripting-surface fixture, run as a level data
// script in both runtimes: a Glyph row, a scrolling level list, and a menu
// entry to the engine controls panel.
// See: docs/scripting-reference.md

use std::path::{Path, PathBuf};

use postretro_level_format::data_script::DataScriptSection;
use postretro_scripting_core::data_descriptors::RegisteredUiTree;
use postretro_ui::descriptor::Widget;
use postretro_ui::modal_stack::ScopeTier;
use postretro_ui::tree::{FocusRect, FocusRectList, FocusRectOwner, NodeInteraction};

use crate::App;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/scripts")
        .join(name)
}

fn run_fixtures(app: &App) -> (Vec<RegisteredUiTree>, Vec<RegisteredUiTree>) {
    let ts = fixture("input-surface-fixture.ts");
    let luau = fixture("input-surface-fixture.luau");
    let root = ts.parent().unwrap();
    let runtime = &app.session.as_ref().unwrap().scripting.script_runtime;
    let run = |section: DataScriptSection| runtime.run_data_script(&section, root).ui_trees;
    let ts_trees = run(DataScriptSection {
        compiled_bytes: postretro_script_compiler::bundle_entry(&ts)
            .expect("the TS fixture bundles through scripts-build")
            .into_bytes(),
        source_path: ts.to_string_lossy().into_owned(),
    });
    let luau_trees = run(DataScriptSection {
        compiled_bytes: std::fs::read(&luau).expect("the Luau fixture reads"),
        source_path: luau.to_string_lossy().into_owned(),
    });
    (ts_trees, luau_trees)
}

fn children(widget: &Widget) -> &[Widget] {
    match widget {
        Widget::VStack(container) | Widget::HStack(container) => &container.children,
        Widget::Grid(grid) => &grid.children,
        _ => &[],
    }
}

#[test]
fn both_sdks_author_the_same_input_surface() {
    let app = crate::app::controls_panel::tests::test_app();
    let (ts, luau) = run_fixtures(&app);
    assert_eq!(ts.len(), 1, "the TS fixture registers its tree");
    assert_eq!(ts, luau, "TypeScript and Luau author identical descriptors");

    let root = &ts[0].tree.root;
    let row = &children(root)[0];
    assert!(matches!(&children(row)[0], Widget::Glyph(glyph) if glyph.command == "nav_confirm"));
    let Widget::VStack(list) = &children(root)[1] else {
        panic!("the level list is a vstack");
    };
    assert_eq!(
        list.scroll.as_ref().map(|scroll| scroll.max_height),
        Some(320.0)
    );
    assert_eq!(list.children.len(), 10);
}

#[test]
fn the_fixture_glyph_resolves_and_its_controls_entry_opens_the_panel() {
    let mut app = crate::app::controls_panel::tests::test_app();
    let (ts, _) = run_fixtures(&app);
    let tree = ts.into_iter().next().unwrap();

    // The glyph resolves in the frame's snapshot: no art is loaded, so it
    // draws the confirm input's name on the default keyboard family.
    let mut snapshot = postretro_ui::UiReadSnapshot::with_trees(
        vec![postretro_ui::UiTreeEntry {
            name: tree.name.clone(),
            tier: ScopeTier::Level,
            descriptor: tree.tree.clone(),
            capture_mode: tree.tree.capture_mode,
            on_commit: None,
        }],
        Default::default(),
        Default::default(),
        0.0,
        None,
    );
    crate::app::glyph_art::resolve_snapshot_glyphs(&mut snapshot, app.session.as_ref().unwrap());
    let row = &children(&snapshot.trees[0].descriptor.root)[0];
    match &children(row)[0] {
        Widget::Text(text) => assert_eq!(text.content, "ENTER"),
        other => panic!("the glyph draws the input name, got {other:?}"),
    }

    // The CONTROLS entry opens the engine panel.
    let session = app.session.as_mut().unwrap();
    session
        .modal_stack
        .push(tree.name.clone(), tree.tree.clone());
    session.ui_focus_rects = Some(FocusRectList {
        rects: vec![FocusRect {
            id: "controls".into(),
            rect: [0.0, 0.0, 100.0, 20.0],
            z: 0,
            group: None,
            neighbors: Default::default(),
            interaction: Some(NodeInteraction::Button {
                on_press: postretro_ui::actions::OPEN_CONTROLS_ACTION.into(),
                repeat_on_hold: None,
            }),
            selected: None,
            checked: None,
            disabled: false,
            tablist: None,
            clip: None,
        }],
        owner: Some(FocusRectOwner {
            name: tree.name.clone(),
            tier: ScopeTier::Level,
        }),
        ..Default::default()
    });
    app.fire_focused_button_activation(Some("controls"));
    assert_eq!(
        app.session.as_ref().unwrap().modal_stack.active_name(),
        Some(postretro_ui::demo::CONTROLS_PANEL_NAME)
    );
}
