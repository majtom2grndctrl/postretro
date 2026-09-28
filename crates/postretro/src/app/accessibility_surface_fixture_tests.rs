// The dev mod's accessibility scripting-surface fixture, run as a level data
// script in both runtimes, then drawn and pressed through the engine.
// See: context/lib/ui.md §4.1

use std::path::{Path, PathBuf};

use postretro_level_format::data_script::DataScriptSection;
use postretro_scripting_core::data_descriptors::RegisteredUiTree;
use postretro_ui::demo::{ACCESSIBILITY_PANEL_NAME, build_accessibility_panel_descriptor};
use postretro_ui::modal_stack::ScopeTier;
use postretro_ui::theme::UiTheme;
use postretro_ui::tree::{CellValues, FocusRectOwner, ImageSizes, NodeInteraction, UiTree};

use crate::App;
use crate::options::OsPreferences;
use crate::startup::lifecycle::tests::test_app;

const FIXTURE_TREE: &str = "accessibilitySurfaceFixture";
const DEVICE: [u32; 2] = [1280, 720];

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/scripts")
        .join(name)
}

/// Each authoring's `setupLevel` trees, run the way level load runs them.
fn run_fixtures(app: &App) -> (Vec<RegisteredUiTree>, Vec<RegisteredUiTree>) {
    let ts = fixture("accessibility-surface-fixture.ts");
    let luau = fixture("accessibility-surface-fixture.luau");
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

struct Drawn {
    texts: Vec<String>,
}

impl Drawn {
    fn shows(&self, content: &str) -> bool {
        self.texts.iter().any(|t| t == content)
    }
}

/// One frame: the options update projects the resolution, the tree draws
/// against the session's slot snapshot, and its focus export (owned by the
/// fixture at level tier) becomes the one the next press resolves against.
fn frame(app: &mut App, tree: &RegisteredUiTree) -> Drawn {
    app.update_player_options(0.0, false);
    let session = app.session.as_mut().unwrap();
    let slots = App::build_ui_slot_snapshot(&session.scripting.script_ctx.slot_table.borrow());
    let mut ui = UiTree::from_descriptor(&tree.tree, &UiTheme::engine_default());
    let draw = ui.build_draw_data_retained(
        DEVICE,
        &mut session.font_system,
        &ImageSizes::new(),
        &slots,
        &CellValues::new(),
        0.0,
    );
    let mut rects = ui.export_focus_rects(&tree.tree, DEVICE, &slots, &CellValues::new());
    rects.owner = Some(FocusRectOwner {
        name: FIXTURE_TREE.to_string(),
        tier: ScopeTier::Level,
    });
    session.ui_focus_rects = Some(rects);
    Drawn {
        texts: draw.texts.into_iter().map(|t| t.content).collect(),
    }
}

#[test]
fn the_scripting_surface_fixture_runs_in_both_runtimes_and_drives_the_engine() {
    let mut app = test_app();
    let (ts, luau) = run_fixtures(&app);
    assert_eq!(ts, luau, "both authorings produce the same trees");
    let [tree] = ts.as_slice() else {
        panic!("the fixture returns one tree, got {}", ts.len());
    };
    assert_eq!(tree.name, FIXTURE_TREE);

    // The OS reports reduced motion and the player never set the field.
    {
        let session = app.session.as_mut().unwrap();
        let registry = session.modal_stack.registry_mut();
        registry.register(
            ACCESSIBILITY_PANEL_NAME,
            build_accessibility_panel_descriptor(),
            ScopeTier::Engine,
            false,
        );
        registry.register(FIXTURE_TREE, tree.tree.clone(), ScopeTier::Level, false);
        session.modal_stack.push_named(FIXTURE_TREE, None);
        session.options_bridge.set_os_preferences(OsPreferences {
            reduce_motion: Some(true),
        });
        session.options_bridge.seed_accessibility(
            &mut session.scripting.script_ctx.slot_table.borrow_mut(),
            &session.player_options,
        );
    }

    let drawn = frame(&mut app, tree);
    assert!(drawn.shows("MOTION REDUCED"), "{:?}", drawn.texts);
    assert!(drawn.shows("FOLLOWING SYSTEM"), "{:?}", drawn.texts);

    // The mod slider binds the working copy on the ordinary setState path.
    let rects = app
        .session
        .as_ref()
        .unwrap()
        .ui_focus_rects
        .clone()
        .unwrap();
    let slider = rects.rects.iter().find(|r| r.id == "shake").unwrap();
    assert!(
        matches!(
            &slider.interaction,
            Some(NodeInteraction::Slider { slot, .. }) if slot == "options.screenShakeScale"
        ),
        "{:?}",
        slider.interaction
    );

    // One cycle step from System: On, player-set, no longer following.
    app.fire_focused_button_activation(Some("reduceMotion"));
    let drawn = frame(&mut app, tree);
    assert_eq!(
        app.session
            .as_ref()
            .unwrap()
            .player_options
            .accessibility
            .reduce_motion,
        Some(true)
    );
    assert!(drawn.shows("MOTION REDUCED"), "{:?}", drawn.texts);
    assert!(!drawn.shows("FOLLOWING SYSTEM"), "{:?}", drawn.texts);

    // The next step: Off, which the resolved slot follows.
    app.fire_focused_button_activation(Some("reduceMotion"));
    let drawn = frame(&mut app, tree);
    assert!(!drawn.shows("MOTION REDUCED"), "{:?}", drawn.texts);
    assert!(!drawn.shows("FOLLOWING SYSTEM"), "{:?}", drawn.texts);

    // A numeric field action on a mod button steps that field.
    app.fire_focused_button_activation(Some("shakeDown"));
    let shake = app
        .session
        .as_ref()
        .unwrap()
        .player_options
        .accessibility
        .screen_shake_scale;
    assert!(shake < 1.0, "screen shake stepped down, got {shake}");

    // The menu entry opens the engine panel over the fixture.
    app.fire_focused_button_activation(Some("openA11y"));
    let stack = &app.session.as_ref().unwrap().modal_stack;
    assert_eq!(stack.active_name(), Some(ACCESSIBILITY_PANEL_NAME));
}

/// The manual strobe fixture (X1) evaluates to a complete manifest, so a
/// surface change that breaks it fails here rather than on hardware.
#[test]
fn the_strobe_fixture_evaluates() {
    let app = test_app();
    let runtime = &app.session.as_ref().unwrap().scripting.script_runtime;
    let run = |name: &str| {
        let path = fixture(name);
        let section = DataScriptSection {
            compiled_bytes: postretro_script_compiler::bundle_entry(&path)
                .expect("the fixture bundles through scripts-build")
                .into_bytes(),
            source_path: path.to_string_lossy().into_owned(),
        };
        runtime.run_data_script(&section, path.parent().unwrap())
    };

    let strobe = run("a11y-strobe-test.ts");
    let pads: Vec<&str> = strobe
        .trigger_events
        .iter()
        .map(|t| t.tag.as_str())
        .collect();
    assert_eq!(
        pads,
        [
            "strobe_white",
            "strobe_red",
            "strobe_small_panel",
            "strobe_large_panel",
            "strobe_light_square",
            "strobe_light_sine",
        ]
    );
    for pad in &strobe.trigger_events {
        assert!(
            strobe.reactions.iter().any(|r| pad.fire.contains(&r.name)),
            "pad `{}` fires a declared reaction",
            pad.tag
        );
    }
    let trees: Vec<&str> = strobe.ui_trees.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(trees, ["a11y.strobe.smallPanel", "a11y.strobe.largePanel"]);
}
