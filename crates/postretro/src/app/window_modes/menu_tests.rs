// Shipped menu activation through the live working-copy and policy adapters.
// See: context/lib/player_options.md §7

use super::policy_tests::{FakeBackend, mode};
use super::{dispatch_display_mode_action, projection, request_mode, service};
use crate::App;
use crate::options::{PlayerOptions, WindowMode};
use crate::startup::lifecycle::tests::test_app;
use postretro_level_format::data_script::DataScriptSection;
use postretro_ui::demo::DISPLAY_MODE_CONFIRM_NAME;
use postretro_ui::descriptor::AnchoredTree;
use postretro_ui::modal_stack::ScopeTier;
use postretro_ui::theme::UiTheme;
use postretro_ui::tree::{FocusRectOwner, ImageSizes, UiTree};
use std::path::Path;
use std::time::Instant;

const DEVICE: [u32; 2] = [1280, 720];

fn install_shipped_menu(app: &mut App) {
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev/scripts/frontend-menu.ts");
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("window-mode-menu.ts");
    // User-script bundling strips bare imports in favor of the runtime SDK
    // prelude. Keep the shipped declarations in the entry itself so an absolute
    // filesystem import cannot be mistaken for a bare SDK import.
    std::fs::write(
        &entry,
        format!(
            "{}\nexport function setupLevel() {{ return {{ uiTrees: [optionsMenu], reactions: frontendReactions }}; }}\n",
            include_str!("../../../../../content/dev/scripts/frontend-menu.ts")
        ),
    )
    .unwrap();
    let session = app.session.as_mut().unwrap();
    let manifest = session.scripting.script_runtime.run_data_script(
        &DataScriptSection {
            compiled_bytes: postretro_script_compiler::bundle_entry(&entry)
                .expect("the shipped frontend menu bundles")
                .into_bytes(),
            source_path: entry.to_string_lossy().into_owned(),
        },
        source.parent().unwrap(),
    );
    session
        .modal_stack
        .register_script_trees(manifest.ui_trees, ScopeTier::Mod);
    session
        .scripting
        .script_ctx
        .data_registry
        .borrow_mut()
        .populate_level(manifest.reactions, manifest.crossings, &[]);
    crate::scripting::reactions::system_commands::register_system_reaction_primitives(
        &mut session.scripting.system_registry,
    );
    let confirm: AnchoredTree = serde_json::from_str(include_str!(
        "../../../../../core/ui/displayModeConfirm.json"
    ))
    .unwrap();
    session.modal_stack.registry_mut().register(
        DISPLAY_MODE_CONFIRM_NAME,
        confirm,
        ScopeTier::Engine,
        false,
    );
    session
        .modal_stack
        .push_named(crate::options::OPTIONS_MENU_TREE_NAME, None);
    app.seed_options_menu_slots();
}

/// Use the production focused-control lookup, reserved-action parser, and
/// named-reaction drain. Only the window backend differs from the live App.
fn activate(app: &mut App, backend: &mut FakeBackend, id: &str, now: Instant) {
    let rects = app
        .session
        .as_ref()
        .unwrap()
        .ui_focus_rects
        .as_ref()
        .unwrap();
    assert!(
        rects
            .rects
            .iter()
            .any(|rect| rect.id == id && !rect.disabled),
        "the shipped active tree must expose {id}"
    );
    app.fire_focused_button_activation_with_display_mode(Some(id), |app, action| {
        let change = dispatch_display_mode_action(
            &mut app.window_modes.controller,
            app.window_modes.confirm_instance,
            app.session.as_mut().unwrap(),
            backend,
            action,
            now,
        );
        app.finish_window_change(change);
    });
    app.dispatch_system_commands();
}

/// Run the actual options bridge and modal-close save ordering, supplying the
/// same request/service adapters with a deterministic window backend.
fn update_options(app: &mut App, backend: &mut FakeBackend, dt: f32, now: Instant) {
    let was_open = app.options_menu_is_top();
    app.update_player_options_with_window_modes(dt, was_open, |app, mode| {
        if let Some(mode) = mode {
            let change = request_mode(
                &mut app.window_modes.controller,
                app.session.as_mut().unwrap(),
                backend,
                mode,
                now,
            );
            app.finish_window_change(change);
        }
        let change = service(
            &mut app.window_modes.controller,
            app.window_modes.confirm_instance,
            app.session.as_ref().unwrap(),
            backend,
            now,
        );
        app.finish_window_change(change);
    });
}

/// Snapshot the session's real slots and local cells, evaluate the visible
/// shipped tree, and export the focus rectangles consumed by activation.
fn draw(app: &mut App) -> Vec<String> {
    let session = app.session.as_mut().unwrap();
    let snapshot = App::build_ui_read_snapshot(
        &session.modal_stack,
        &mut session.presentation_cells,
        &session.scripting.script_ctx.slot_table.borrow(),
        app.script_time,
        session.ui_input_mode,
        app.ui_focused_id.clone(),
        false,
    );
    let top = snapshot.trees.last().expect("a shipped modal is active");
    let mut ui = UiTree::from_descriptor(&top.descriptor, &UiTheme::engine_default());
    let drawn = ui.build_draw_data_retained(
        DEVICE,
        &mut session.font_system,
        &ImageSizes::new(),
        &snapshot.slot_values,
        &snapshot.cell_values,
        0.0,
    );
    let mut rects = ui.export_focus_rects(
        &top.descriptor,
        DEVICE,
        &snapshot.slot_values,
        &snapshot.cell_values,
    );
    rects.owner = Some(FocusRectOwner {
        name: top.name.clone(),
        tier: top.tier,
    });
    session.ui_focus_rects = Some(rects);
    drawn.texts.into_iter().map(|text| text.content).collect()
}

fn assert_live_mode(texts: &[String], width: u32, hz: u32) {
    for expected in [format!("{width}x"), "720".into(), format!(" @ {hz} Hz")] {
        assert!(
            texts.contains(&expected),
            "missing {expected:?} in {texts:?}"
        );
    }
}

#[test]
fn shipped_menu_activations_drive_policy_bridge_bindings_and_saved_choice() {
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    let old = mode(1280, 60000, "current");
    let new = mode(1920, 144000, "current");
    let mut backend = FakeBackend::default();
    *backend.modes.borrow_mut() = vec![old.clone(), new.clone()];
    let now = Instant::now();
    {
        let session = app.session.as_mut().unwrap();
        session.settings_path = Some(path.clone());
        session.player_options.set_display_mode(old.clone());
        session.player_options.save(&path).unwrap();
        app.window_modes
            .controller
            .boot(&mut backend, &session.player_options, now);
    }
    install_shipped_menu(&mut app);
    projection::project(
        &app.window_modes.controller,
        &mut app
            .session
            .as_ref()
            .unwrap()
            .scripting
            .script_ctx
            .slot_table
            .borrow_mut(),
        now,
    );

    draw(&mut app);
    activate(&mut app, &mut backend, "optionsTabGraphics", now);
    update_options(&mut app, &mut backend, 0.0, now);
    assert_live_mode(&draw(&mut app), 1280, 60);

    activate(&mut app, &mut backend, "displayModeNext", now);
    update_options(&mut app, &mut backend, 0.0, now);
    assert_live_mode(&draw(&mut app), 1920, 144);
    assert_eq!(
        app.session.as_ref().unwrap().player_options.display_mode,
        Some(new)
    );
    activate(&mut app, &mut backend, "displayModePrev", now);
    update_options(&mut app, &mut backend, 0.0, now);
    assert_live_mode(&draw(&mut app), 1280, 60);
    assert_eq!(
        app.session.as_ref().unwrap().player_options.display_mode,
        Some(old.clone())
    );
    assert!(
        backend.requests.is_empty(),
        "windowed picks do not apply exclusive modes"
    );

    // The row's real named reaction writes the working copy; the bridge then
    // requests the picked exclusive tuple and opens the engine-owned confirm.
    activate(&mut app, &mut backend, "optionsExclusive", now);
    assert!(matches!(
        app.session.as_ref().unwrap().scripting.script_ctx.slot_table.borrow()
            .get("options.windowMode").unwrap().value.as_ref(),
        Some(postretro_entities::SlotValue::Enum(value)) if value == "exclusive"
    ));
    assert_eq!(
        app.session.as_ref().unwrap().player_options.window_mode,
        WindowMode::Windowed
    );
    update_options(&mut app, &mut backend, 0.0, now);
    assert!(app.window_modes.controller.pending.is_some());
    assert!(app.display_mode_confirm_is_top());
    assert_eq!(
        app.session.as_ref().unwrap().player_options.window_mode,
        WindowMode::Windowed
    );
    assert_eq!(backend.requests.len(), 1);
    assert_eq!(backend.requests[0].mode, WindowMode::Exclusive);
    assert_eq!(backend.requests[0].display, Some(old.clone()));
    assert_eq!(PlayerOptions::load(&path).window_mode, WindowMode::Windowed);

    draw(&mut app);
    activate(&mut app, &mut backend, "displayModeKeep", now);
    update_options(&mut app, &mut backend, 0.3, now);
    assert!(app.window_modes.controller.pending.is_none());
    assert!(app.options_menu_is_top());
    assert_live_mode(&draw(&mut app), 1280, 60);
    assert_eq!(
        backend.requests.len(),
        1,
        "keep/reseed must not reapply the mode"
    );
    let saved = PlayerOptions::load(&path);
    assert_eq!(saved.window_mode, WindowMode::Exclusive);
    assert_eq!(saved.display_mode, Some(old));
}
