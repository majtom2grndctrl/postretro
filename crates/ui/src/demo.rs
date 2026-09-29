// UI registry constants and shipped fallback descriptor tests.
// See: context/lib/ui.md

/// Registry name the pause menu is registered + pushed under (M13 Goal F, Task
/// 5). The App registers the descriptor at boot (from `pauseMenu.json` via
/// `tree_asset::register_tree_from_disk`) and pushes/pops it via `push_named` on
/// `nav.menu`.
pub const PAUSE_MENU_NAME: &str = "pauseMenu";

/// Registry name for the engine fallback frontend menu. Mods may declare any
/// registered tree as `frontend.menuTree`; this name is the no-mod fallback.
pub const FRONTEND_MENU_NAME: &str = "frontendMenu";

/// Reserved, non-shadowable registry name of the engine accessibility panel
/// (`core/ui/accessibilityPanel.json`). A mod- or level-scope registration
/// under it is rejected.
pub const ACCESSIBILITY_PANEL_NAME: &str = "accessibilityPanel";

/// Read a committed UI descriptor JSON anchored to the repo root (NOT runtime
/// cwd, so it passes under `cargo test`, which runs from the crate dir). Mirrors
/// the `tree_asset`/keyboard precedent: `CARGO_MANIFEST_DIR` + `../..` reaches the
/// workspace root, then the workspace's own `core/`. Test-only — the engine
/// resolves its root from argv at boot.
#[cfg(any(test, feature = "test-fixtures"))]
fn load_ui_fixture(name: &str) -> super::descriptor::AnchoredTree {
    let path = super::core_root::CoreRoot::at(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("core"),
    )
    .ui_asset_path(name);
    let bytes = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("fixture '{}' exists: {e}", path.display()));
    serde_json::from_str(&bytes)
        .unwrap_or_else(|e| panic!("fixture '{}' deserializes: {e}", path.display()))
}

/// The shipped HUD descriptor (`core/ui/hud.json`). The HUD is now
/// JSON-authored; the demo's behavioral tests (here and `demo_ui_gate_test`) load
/// it from the source of truth rather than a hand-assembled builder.
#[cfg(any(test, feature = "test-fixtures"))]
pub fn build_demo_descriptor() -> super::descriptor::AnchoredTree {
    load_ui_fixture("hud.json")
}

/// The shipped pause-menu descriptor (`core/ui/pauseMenu.json`). Only
/// this crate's own tests consume it (its `test-fixtures` siblings are used
/// cross-crate; this one is not), so it stays `#[cfg(test)]`-only.
#[cfg(test)]
pub(crate) fn build_pause_menu_descriptor() -> super::descriptor::AnchoredTree {
    load_ui_fixture("pauseMenu.json")
}

/// The shipped engine accessibility panel (`core/ui/accessibilityPanel.json`).
#[cfg(any(test, feature = "test-fixtures"))]
pub fn build_accessibility_panel_descriptor() -> super::descriptor::AnchoredTree {
    load_ui_fixture("accessibilityPanel.json")
}

/// The shipped frontend-menu descriptor (`core/ui/frontendMenu.json`).
#[cfg(any(test, feature = "test-fixtures"))]
pub fn build_frontend_menu_descriptor() -> super::descriptor::AnchoredTree {
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
    const OPEN_SEATS_FORMAT: &str = "OPEN SEATS {}";

    /// The engine HUD asset is now a minimal fallback. The production HUD is
    /// registered by mod content; this marker must stay fallback-only so shadowing
    /// tests can prove the mod `hud` replaced it.
    #[test]
    fn fallback_hud_descriptor_carries_the_fallback_only_marker() {
        let tree = build_demo_descriptor();
        let Widget::VStack(col) = &tree.root else {
            panic!("fallback HUD root is a vstack column");
        };
        let [health_row, open_seats_row] = col.children.as_slice() else {
            panic!("fallback HUD has health and open-seat rows");
        };

        let Widget::Text(health) = health_row else {
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

        let Widget::Text(open_seats) = open_seats_row else {
            panic!("second fallback row is the open-seat status");
        };
        assert_eq!(
            open_seats.bind.as_ref().and_then(|b| b.source.slot()),
            Some("session.openSeats"),
        );
        assert_eq!(
            open_seats.bind.as_ref().and_then(|b| b.format.as_deref()),
            Some(OPEN_SEATS_FORMAT),
        );
    }

    /// The fallback stays intentionally smaller than the production HUD: it only
    /// carries engine-owned health and open-seat status when no mod HUD is
    /// registered.
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
    /// A fallback menu's one control: the entry to the engine accessibility
    /// panel, which needs no mod-registered reaction.
    fn assert_only_control_is_the_accessibility_entry(widget: &Widget, id: &str) {
        let Widget::Button(button) = widget else {
            panic!("the last row is the accessibility entry button");
        };
        assert_eq!(button.id, id);
        assert_eq!(button.label.as_deref(), Some("ACCESSIBILITY"));
        assert_eq!(button.on_press, crate::actions::OPEN_ACCESSIBILITY_ACTION);
    }

    #[test]
    fn fallback_pause_menu_is_capturing_with_only_an_accessibility_entry() {
        let tree = build_pause_menu_descriptor();
        assert_eq!(
            tree.capture_mode,
            CaptureMode::Capture,
            "the pause menu captures input (gates player controls, releases cursor)",
        );
        assert_eq!(tree.initial_focus.as_deref(), Some("pauseAccessibility"));

        let Widget::VStack(col) = &tree.root else {
            panic!("pause menu root is a vstack column");
        };
        assert_eq!(col.children.len(), 3);
        assert_only_control_is_the_accessibility_entry(&col.children[2], "pauseAccessibility");

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
        assert_eq!(
            json.matches(r#""kind":"button""#).count(),
            1,
            "the accessibility entry is the fallback's only button; the production mod owns controls",
        );
        assert!(!json.contains(r#""kind":"slider""#));
    }

    /// The frontend fallback is the no-mod/no-map boot surface. It must be a
    /// capturing modal so it uses the same control suppression and cursor release
    /// path as mod-authored frontend menus.
    #[test]
    fn fallback_frontend_menu_is_capturing_with_only_an_accessibility_entry() {
        let tree = build_frontend_menu_descriptor();
        assert_eq!(
            tree.capture_mode,
            CaptureMode::Capture,
            "the frontend fallback captures input through the modal-stack path",
        );
        assert_eq!(tree.initial_focus.as_deref(), Some("frontendAccessibility"));

        let Widget::VStack(col) = &tree.root else {
            panic!("frontend fallback root is a vstack column");
        };
        assert_eq!(col.children.len(), 3);
        assert_only_control_is_the_accessibility_entry(&col.children[2], "frontendAccessibility");

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
        use crate::modal_stack::ModalStack;

        let mut stack = ModalStack::new();
        stack.registry_mut().register(
            super::PAUSE_MENU_NAME,
            build_pause_menu_descriptor(),
            crate::modal_stack::ScopeTier::Engine,
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

    /// The fallback pause menu's one control is the accessibility entry; Escape /
    /// gamepad B / Start close it through App policy, not an authored button or
    /// named reaction.
    #[test]
    fn fallback_pause_menu_exports_only_the_accessibility_entry() {
        use crate::theme::UiTheme;
        use crate::tree::{ImageSizes, UiTree};
        use postretro_entities::SlotValue;
        use std::collections::HashMap;

        let tree = build_pause_menu_descriptor();
        let theme = UiTheme::engine_default();
        let mut ui = UiTree::from_descriptor(&tree, &theme);
        let mut font_system = crate::text::build_font_system();
        let images = ImageSizes::new();
        let slots: HashMap<String, SlotValue> = HashMap::new();
        let cells = crate::tree::CellValues::new();
        // Lay out + export the focus rects exactly as the renderer does each frame.
        ui.build_draw_data([1280, 720], &mut font_system, &images, &slots);
        let rects = ui.export_focus_rects(&tree, [1280, 720], &slots, &cells);

        let ids: Vec<&str> = rects.rects.iter().map(|rect| rect.id.as_str()).collect();
        assert_eq!(ids, ["pauseAccessibility"]);
    }

    fn visit<'a>(widget: &'a Widget, out: &mut Vec<&'a Widget>) {
        out.push(widget);
        let children: &[Widget] = match widget {
            Widget::VStack(c) | Widget::HStack(c) => &c.children,
            Widget::Grid(g) => &g.children,
            _ => &[],
        };
        for child in children {
            visit(child, out);
        }
    }

    /// The panel works under a mod that registers no reactions: every control
    /// fires a reserved `ui.*` action or is an engine-routed slider on a
    /// readonly `accessibility.*` slot, and each carries a name.
    #[test]
    fn accessibility_panel_controls_are_reserved_actions_with_names() {
        let tree = super::build_accessibility_panel_descriptor();
        assert_eq!(tree.capture_mode, CaptureMode::Capture);
        let mut widgets = Vec::new();
        visit(&tree.root, &mut widgets);
        let mut controls = 0;
        for widget in widgets {
            match widget {
                Widget::Button(button) => {
                    controls += 1;
                    assert!(
                        button.on_press.starts_with("ui.accessibility.")
                            || button.on_press == crate::actions::CLOSE_DIALOG_ACTION,
                        "{} fires {}",
                        button.id,
                        button.on_press
                    );
                    assert!(button.label.is_some() || button.labelled_by.is_some());
                }
                Widget::Slider(slider) => {
                    controls += 1;
                    let slot = slider
                        .bind
                        .source
                        .slot()
                        .expect("panel sliders bind a slot");
                    assert!(slot.starts_with("accessibility."), "{slot}");
                    assert!(slider.label.is_some() || slider.labelled_by.is_some());
                }
                _ => {}
            }
        }
        assert!(controls >= 10, "every field plus a close button");
    }

    fn visible_panel_texts(slots: &[(&str, bool)]) -> Vec<String> {
        use crate::theme::UiTheme;
        use crate::tree::{ImageSizes, UiTree};
        use postretro_entities::SlotValue;
        use std::collections::HashMap;

        let tree = super::build_accessibility_panel_descriptor();
        let mut ui = UiTree::from_descriptor(&tree, &UiTheme::engine_default());
        let mut fs = crate::text::build_font_system();
        let slots: HashMap<String, SlotValue> = slots
            .iter()
            .map(|(name, value)| (name.to_string(), SlotValue::Boolean(*value)))
            .collect();
        ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &ImageSizes::new(),
            &slots,
            &crate::tree::CellValues::new(),
            0.0,
        )
        .texts
        .into_iter()
        .map(|text| text.content)
        .collect()
    }

    /// S3: with reduce motion unset and the OS reporting on, the control reads
    /// "System (On)"; after one cycle step it reads "On".
    #[test]
    fn reduce_motion_control_reads_system_on_then_on() {
        let following = visible_panel_texts(&[
            ("accessibility.reduceMotion", true),
            ("accessibility.reduceMotionFollowsSystem", true),
        ]);
        assert!(
            following.iter().any(|t| t == "SYSTEM (ON)"),
            "{following:?}"
        );
        assert!(!following.iter().any(|t| t == "SYSTEM (OFF)"));

        let player_set = visible_panel_texts(&[
            ("accessibility.reduceMotion", true),
            ("accessibility.reduceMotionFollowsSystem", false),
        ]);
        assert!(
            !player_set.iter().any(|t| t.starts_with("SYSTEM")),
            "{player_set:?}"
        );
        assert!(player_set.iter().any(|t| t == "ON"));
    }
}
