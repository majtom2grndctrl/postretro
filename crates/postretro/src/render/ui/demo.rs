// UI registry constants and shipped fallback descriptor tests.
// See: context/lib/ui.md

/// Registry name the pause menu is registered + pushed under (M13 Goal F, Task
/// 5). The App registers the descriptor at boot (from `pauseMenu.json` via
/// `tree_asset::register_tree_from_disk`) and pushes/pops it via `push_named` on
/// `nav.menu`.
pub(crate) const PAUSE_MENU_NAME: &str = "pauseMenu";

/// Registry name for the engine fallback frontend menu. Mods may declare any
/// registered tree as `frontend.menuTree`; this name is the no-mod fallback.
pub(crate) const FRONTEND_MENU_NAME: &str = "frontendMenu";

/// Read a committed UI descriptor JSON anchored to the repo root (NOT runtime
/// cwd, so it passes under `cargo test`, which runs from the crate dir). Mirrors
/// the `tree_asset`/keyboard precedent: `CARGO_MANIFEST_DIR` + `../..` reaches the
/// workspace root, then `content/base/ui/<name>`. Test-only — the engine loads
/// these via the cwd-relative `tree_asset` path at boot.
#[cfg(test)]
fn load_ui_fixture(name: &str) -> super::descriptor::AnchoredTree {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("content/base/ui")
        .join(name);
    let bytes = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("fixture '{}' exists: {e}", path.display()));
    serde_json::from_str(&bytes)
        .unwrap_or_else(|e| panic!("fixture '{}' deserializes: {e}", path.display()))
}

/// The shipped HUD descriptor (`content/base/ui/hud.json`). The HUD is now
/// JSON-authored; the demo's behavioral tests (here and `demo_ui_gate_test`) load
/// it from the source of truth rather than a hand-assembled builder.
#[cfg(test)]
pub(crate) fn build_demo_descriptor() -> super::descriptor::AnchoredTree {
    load_ui_fixture("hud.json")
}

/// The shipped pause-menu descriptor (`content/base/ui/pauseMenu.json`).
#[cfg(test)]
pub(crate) fn build_pause_menu_descriptor() -> super::descriptor::AnchoredTree {
    load_ui_fixture("pauseMenu.json")
}

/// The shipped frontend-menu descriptor (`content/base/ui/frontendMenu.json`).
#[cfg(test)]
pub(crate) fn build_frontend_menu_descriptor() -> super::descriptor::AnchoredTree {
    load_ui_fixture("frontendMenu.json")
}

#[cfg(test)]
mod tests {
    use super::super::descriptor::{CaptureMode, Widget};
    use super::{
        build_demo_descriptor, build_frontend_menu_descriptor, build_pause_menu_descriptor,
    };

    const FALLBACK_HUD_MARKER: &str = "FALLBACK HUD HP --";
    const FALLBACK_HUD_FORMAT: &str = "FALLBACK HUD HP {}";

    /// The engine HUD asset is now a minimal fallback. The production HUD is
    /// registered by mod content; this marker must stay fallback-only so shadowing
    /// tests can prove the mod `hud` replaced it.
    #[test]
    fn fallback_hud_descriptor_carries_the_fallback_only_marker() {
        let tree = build_demo_descriptor();
        let Widget::VStack(col) = &tree.root else {
            panic!("fallback HUD root is a vstack column");
        };
        assert_eq!(col.children.len(), 1, "fallback HUD has one health row");

        let Widget::Text(health) = &col.children[0] else {
            panic!("fallback row is the health text");
        };
        assert_eq!(health.content, FALLBACK_HUD_MARKER);
        assert_eq!(
            health.bind.as_ref().and_then(|b| b.source.slot()),
            Some("player.health"),
        );
        assert_eq!(
            health.bind.as_ref().and_then(|b| b.format.as_deref()),
            Some(FALLBACK_HUD_FORMAT),
        );
    }

    /// The fallback stays intentionally smaller than the production HUD: it only
    /// carries the health text needed when no mod HUD is registered.
    #[test]
    fn fallback_hud_descriptor_omits_demo_only_surfaces() {
        let tree = build_demo_descriptor();
        let json = serde_json::to_string(&tree).expect("fallback HUD serializes");
        for removed in [
            "player.ammo",
            "AMMO",
            "intro.flashColor",
            "SCREEN.FLASH",
            "screen.flash",
        ] {
            assert!(
                !json.contains(removed),
                "fallback HUD must not carry legacy HUD surface {removed:?}: {json}",
            );
        }
        assert!(
            !json.contains(r#""kind":"bar""#),
            "fallback HUD is text-only; the production mod HUD owns the bar"
        );
    }

    /// The engine pause menu is a script-independent fallback. Production mods
    /// shadow it with an SDK-authored menu; this asset only needs to capture input
    /// and explain the engine-owned close policy.
    #[test]
    fn fallback_pause_menu_is_capturing_text_only() {
        let tree = build_pause_menu_descriptor();
        assert_eq!(
            tree.capture_mode,
            CaptureMode::Capture,
            "the pause menu captures input (gates player controls, releases cursor)",
        );
        assert_eq!(
            tree.initial_focus.as_deref(),
            None,
            "the fallback has no focusable controls",
        );

        let Widget::VStack(col) = &tree.root else {
            panic!("pause menu root is a vstack column");
        };
        assert_eq!(col.children.len(), 2);

        let Widget::Text(title) = &col.children[0] else {
            panic!("first row is the title text");
        };
        assert_eq!(title.content, "PAUSED");
        assert_eq!(
            title.bind.as_ref().and_then(|b| b.source.slot()),
            None,
            "fallback title is not bound to state",
        );

        let Widget::Text(instruction) = &col.children[1] else {
            panic!("second row is the resume instruction text");
        };
        assert_eq!(instruction.content, "PRESS ESC OR B TO RESUME");
        assert_eq!(
            instruction.bind.as_ref().and_then(|b| b.source.slot()),
            None,
            "fallback instruction is not bound to state",
        );
    }

    /// The fallback must not depend on mod stores, named reactions, reserved
    /// button actions, text entry, or input-mode readouts. Those surfaces are
    /// exercised by production SDK-authored trees and generic keyboard coverage.
    #[test]
    fn fallback_pause_menu_omits_removed_demo_surfaces() {
        let tree = build_pause_menu_descriptor();
        let json = serde_json::to_string(&tree).expect("pause fallback serializes");
        for removed in [
            "resumePauseMenu",
            "openTextEntry",
            "ui.closeDialog",
            "ui.commitTextEntry",
            "audio.master",
            "input.mode",
            "ui.textEntry",
            "pauseVolume",
            "pauseOpenTextEntry",
            "pauseResume",
        ] {
            assert!(
                !json.contains(removed),
                "fallback pause menu must not carry demo surface {removed:?}: {json}",
            );
        }
        assert!(
            !json.contains(r#""kind":"button""#) && !json.contains(r#""kind":"slider""#),
            "fallback pause menu is text-only; the production mod owns controls",
        );
    }

    /// The frontend fallback is the no-mod/no-map boot surface. It must be a
    /// capturing modal so it uses the same control suppression and cursor release
    /// path as mod-authored frontend menus.
    #[test]
    fn fallback_frontend_menu_is_capturing_text_only() {
        let tree = build_frontend_menu_descriptor();
        assert_eq!(
            tree.capture_mode,
            CaptureMode::Capture,
            "the frontend fallback captures input through the modal-stack path",
        );
        assert_eq!(
            tree.initial_focus.as_deref(),
            None,
            "Task 4 fallback has no focusable controls",
        );

        let Widget::VStack(col) = &tree.root else {
            panic!("frontend fallback root is a vstack column");
        };
        assert_eq!(col.children.len(), 2);

        let Widget::Text(title) = &col.children[0] else {
            panic!("first row is the title text");
        };
        assert_eq!(title.content, "POSTRETRO");

        let Widget::Text(instruction) = &col.children[1] else {
            panic!("second row is the status text");
        };
        assert_eq!(instruction.content, "NO MOD FRONTEND REGISTERED");
    }

    /// The `nav.menu` toggle pushes/pops the registered pause menu through the
    /// modal stack (the exact sequence `App::toggle_pause_menu` runs): a first
    /// toggle pushes the capturing menu (gameplay → menu), a second pops it back
    /// (menu → gameplay). Pins that the registered descriptor captures and that
    /// the registry name matches what the App pushes.
    #[test]
    fn nav_menu_toggle_pushes_then_pops_the_pause_menu() {
        use crate::render::ui::modal_stack::ModalStack;

        let mut stack = ModalStack::new();
        stack.registry_mut().register(
            super::PAUSE_MENU_NAME,
            build_pause_menu_descriptor(),
            crate::render::ui::modal_stack::ScopeTier::Engine,
            false,
        );

        // No capturing tree up: gameplay keeps input.
        assert_eq!(stack.top_capture_mode(), CaptureMode::Passthrough);
        assert_ne!(stack.active_name(), Some(super::PAUSE_MENU_NAME));

        // First `nav.menu`: push the pause menu (it captures → menu focus).
        stack.push_named(super::PAUSE_MENU_NAME, None);
        assert_eq!(stack.active_name(), Some(super::PAUSE_MENU_NAME));
        assert_eq!(
            stack.top_capture_mode(),
            CaptureMode::Capture,
            "the pushed pause menu captures input",
        );

        // Second `nav.menu`: the menu is the top tree, so it pops back to gameplay.
        stack.pop();
        assert_ne!(stack.active_name(), Some(super::PAUSE_MENU_NAME));
        assert_eq!(stack.top_capture_mode(), CaptureMode::Passthrough);
    }

    /// The fallback pause menu has no controls; Escape / gamepad B / Start close
    /// it through App policy, not an authored button or named reaction. The
    /// renderer→focus-engine export therefore has no focusable rects.
    #[test]
    fn fallback_pause_menu_exports_no_focusable_controls() {
        use crate::render::ui::theme::UiTheme;
        use crate::render::ui::tree::{ImageSizes, UiTree};
        use postretro_entities::SlotValue;
        use std::collections::HashMap;

        let tree = build_pause_menu_descriptor();
        let theme = UiTheme::engine_default();
        let mut ui = UiTree::from_descriptor(&tree, &theme);
        let mut font_system = crate::render::ui::text::build_font_system();
        let images = ImageSizes::new();
        let slots: HashMap<String, SlotValue> = HashMap::new();
        let cells = crate::render::ui::tree::CellValues::new();
        // Lay out + export the focus rects exactly as the renderer does each frame.
        ui.build_draw_data([1280, 720], &mut font_system, &images, &slots);
        let rects = ui.export_focus_rects(&tree, [1280, 720], &slots, &cells);

        assert!(
            rects.rects.is_empty(),
            "fallback pause menu should not export focusable widgets",
        );
        assert!(
            rects.groups.is_empty(),
            "fallback pause menu should not open focus groups",
        );
    }
}
