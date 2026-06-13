// M13 demo gameplay HUD + pause-menu descriptors, now JSON-authored. The HUD
// (`content/base/ui/hud.json`) and pause menu (`content/base/ui/pauseMenu.json`)
// ship as on-disk `AnchoredTree` descriptors loaded through the shared
// `tree_asset` load-and-register path at boot — there is no hand-assembled tree in
// engine Rust for either screen. `main.rs` resolves the HUD by name from the
// registry as the always-on bottom passthrough layer; the pause menu pushes/pops
// by `PAUSE_MENU_NAME`.
//
// What remains here is the `PAUSE_MENU_NAME` registry constant the App pushes
// under, plus the demo's behavioral tests, which load the shipped JSON and assert
// the wiring (bound slots, focus chain, capture mode) against the source of truth.
//
// See: context/lib/scripting.md §3 (defineStore / DefinitionOnly) ·
//      context/lib/ui.md

/// Registry name the pause menu is registered + pushed under (M13 Goal F, Task
/// 5). The App registers the descriptor at boot (from `pauseMenu.json` via
/// `tree_asset::register_tree_from_disk`) and pushes/pops it via `push_named` on
/// `nav.menu`.
pub(crate) const PAUSE_MENU_NAME: &str = "pauseMenu";

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

#[cfg(test)]
mod tests {
    use super::super::descriptor::{
        CaptureMode, ColorValue, Easing, PanelTween, TextTween, Widget,
    };
    use super::{build_demo_descriptor, build_pause_menu_descriptor};

    /// Pause-menu widget ids the focus-chain tests assert against. These mirror the
    /// ids authored in `pauseMenu.json`; the tests load the JSON and check the
    /// wiring resolves to these nodes.
    const PAUSE_RESUME_ID: &str = "pauseResume";
    const PAUSE_VOLUME_ID: &str = "pauseVolume";
    const PAUSE_TEXT_ENTRY_ID: &str = "pauseOpenTextEntry";

    /// The pause-menu reaction names the buttons carry in `pauseMenu.json`.
    const PAUSE_TEXT_ENTRY_REACTION: &str = "openTextEntry";
    const PAUSE_RESUME_REACTION: &str = "resumePauseMenu";

    /// Authored values the HUD JSON pins; asserted to stay in sync with the source.
    const HEALTH_TWEEN_MS: f32 = 1200.0;
    const FLASH_TWEEN_MS: f32 = 150.0;
    const FLASH_FALLBACK_FILL: [f32; 4] = [0.0, 0.65, 0.75, 1.0];
    const SCREEN_FLASH_FALLBACK_FILL: [f32; 4] = [0.0, 0.0, 0.0, 0.0];
    const HEALTH_BAR_MAX: f32 = 100.0;
    const HEALTH_CRITICAL_UP_TO: f32 = 0.2;
    const HEALTH_WARNING_UP_TO: f32 = 0.5;

    /// The slider's authored range/step in `pauseMenu.json`.
    const VOLUME_MIN: f32 = 0.0;
    const VOLUME_MAX: f32 = 1.0;
    const VOLUME_STEP: f32 = 0.1;

    /// The demo descriptor binds the three expected slots: `player.health` and
    /// `player.ammo` on text nodes, `intro.flashColor` on a panel fill. This pins
    /// the wiring at the descriptor level; the gate test drives it through layout.
    #[test]
    fn demo_descriptor_binds_the_three_slots() {
        let tree = build_demo_descriptor();
        let Widget::VStack(col) = &tree.root else {
            panic!("demo root is a vstack column");
        };
        // health, ammo, flash swatch, health bar (styleRanges), screen.flash swatch
        assert_eq!(col.children.len(), 5, "five HUD rows");

        let Widget::Text(health) = &col.children[0] else {
            panic!("first row is the health text");
        };
        assert_eq!(
            health.bind.as_ref().map(|b| b.slot.as_str()),
            Some("player.health"),
        );
        assert_eq!(
            health.bind.as_ref().and_then(|b| b.format.as_deref()),
            Some("HP {}"),
        );
        // The health bind tweens the first-resolve count-up: from 0 over 1.2s,
        // easeOut.
        assert_eq!(
            health.bind.as_ref().and_then(|b| b.tween.clone()),
            Some(TextTween {
                duration_ms: HEALTH_TWEEN_MS,
                easing: Easing::EaseOut,
                from: Some(0.0),
            }),
            "health bind carries the 0→100 first-resolve count-up tween",
        );

        let Widget::Text(ammo) = &col.children[1] else {
            panic!("second row is the ammo text");
        };
        assert_eq!(
            ammo.bind.as_ref().map(|b| b.slot.as_str()),
            Some("player.ammo"),
        );
        assert_eq!(
            ammo.bind.as_ref().and_then(|b| b.format.as_deref()),
            Some("AMMO {}"),
        );

        let Widget::Grid(swatch) = &col.children[2] else {
            panic!("third row is the swatch grid");
        };
        let Widget::Panel(panel) = &swatch.children[0] else {
            panic!("swatch grid's first cell is the bound flash panel");
        };
        assert_eq!(
            panel.bind.as_ref().map(|b| b.slot.as_str()),
            Some("intro.flashColor"),
        );
        assert_eq!(
            panel.fill,
            ColorValue::Literal(FLASH_FALLBACK_FILL),
            "panel keeps a literal fallback fill",
        );
        // The swatch panel eases each proxy toggle (150ms easeInOut, no `from`).
        assert_eq!(
            panel.bind.as_ref().and_then(|b| b.tween.clone()),
            Some(PanelTween {
                duration_ms: FLASH_TWEEN_MS,
                easing: Easing::EaseInOut,
                from: None,
            }),
            "swatch panel carries the toggle-smoothing tween (no `from`)",
        );
    }

    /// The styleRanges health bar (the M13 Goal E demo bar): a `panel` bound to
    /// the numeric `player.health` slot carrying a three-band styleRanges map.
    /// This is the fourth HUD row (after the flash swatch) so the swatch stays
    /// the first-emitted quad for the gate tests.
    #[test]
    fn demo_descriptor_carries_a_styleranges_health_bar() {
        let tree = build_demo_descriptor();
        let Widget::VStack(col) = &tree.root else {
            panic!("demo root is a vstack column");
        };
        let Widget::Grid(bar) = &col.children[3] else {
            panic!("fourth row is the health-bar grid");
        };
        let Widget::Panel(bar_panel) = &bar.children[0] else {
            panic!("health-bar grid's first cell is the bound bar panel");
        };
        assert_eq!(
            bar_panel.bind.as_ref().map(|b| b.slot.as_str()),
            Some("player.health"),
            "health bar binds the numeric player.health slot",
        );
        let ranges = bar_panel
            .style_ranges
            .as_ref()
            .expect("health bar carries a styleRanges map");
        assert_eq!(ranges.max, HEALTH_BAR_MAX);
        assert_eq!(ranges.entries.len(), 3, "critical / warning / ok bands");
        assert_eq!(ranges.entries[0].up_to, Some(HEALTH_CRITICAL_UP_TO));
        assert_eq!(ranges.entries[1].up_to, Some(HEALTH_WARNING_UP_TO));
        assert_eq!(ranges.entries[2].up_to, None, "ok is the trailing default");
    }

    /// The pause menu (M13 Goal F, Task 5): a centered capturing modal with a
    /// Resume button and an `audio.master`-bound volume slider that captures
    /// left/right nav, plus a `text` bound to `input.mode`. Focus starts on Resume.
    #[test]
    fn pause_menu_is_a_capturing_modal_with_button_and_volume_slider() {
        let tree = build_pause_menu_descriptor();
        assert_eq!(
            tree.capture_mode,
            CaptureMode::Capture,
            "the pause menu captures input (freezes gameplay, releases cursor)",
        );
        assert_eq!(
            tree.initial_focus.as_deref(),
            Some(PAUSE_RESUME_ID),
            "focus starts on the Resume button",
        );

        let Widget::VStack(col) = &tree.root else {
            panic!("pause menu root is a vstack column");
        };
        // title, input.mode readout, resume button, volume slider, ui.textEntry
        // readout, open-text-entry button
        assert_eq!(col.children.len(), 6);

        let Widget::Text(mode) = &col.children[1] else {
            panic!("second row is the input.mode readout text");
        };
        assert_eq!(
            mode.bind.as_ref().map(|b| b.slot.as_str()),
            Some("input.mode"),
            "the readout binds the engine-owned input.mode slot",
        );

        let Widget::Button(resume) = &col.children[2] else {
            panic!("third row is the Resume button");
        };
        assert_eq!(resume.id, PAUSE_RESUME_ID);
        assert_eq!(resume.on_press, PAUSE_RESUME_REACTION);

        let Widget::Slider(volume) = &col.children[3] else {
            panic!("fourth row is the volume slider");
        };
        assert_eq!(volume.id, PAUSE_VOLUME_ID);
        assert_eq!(volume.bind.slot, "audio.master");
        assert_eq!(
            volume.captures_nav,
            vec!["nav.left".to_string(), "nav.right".to_string()],
            "the slider captures left/right nav to step volume",
        );
        assert_eq!(volume.min, VOLUME_MIN);
        assert_eq!(volume.max, VOLUME_MAX);
        assert_eq!(volume.step, VOLUME_STEP);
    }

    /// The text-entry demo widgets (M13 Text-Entry, Task 4): a `text` row binding
    /// `ui.textEntry` DIRECTLY (so the live entry shows here) and a button whose
    /// `onPress` opens the on-screen keyboard via the `openTextEntry` reaction.
    #[test]
    fn pause_menu_demos_direct_text_entry_binding_and_an_open_button() {
        let tree = build_pause_menu_descriptor();
        let Widget::VStack(col) = &tree.root else {
            panic!("pause menu root is a vstack column");
        };

        let Widget::Text(entry) = &col.children[4] else {
            panic!("fifth row is the ui.textEntry readout text");
        };
        assert_eq!(
            entry.bind.as_ref().map(|b| b.slot.as_str()),
            Some("ui.textEntry"),
            "the readout binds the engine-owned ui.textEntry slot directly (no copyState)",
        );

        let Widget::Button(open) = &col.children[5] else {
            panic!("sixth row is the open-text-entry button");
        };
        assert_eq!(open.id, PAUSE_TEXT_ENTRY_ID);
        assert_eq!(
            open.on_press, PAUSE_TEXT_ENTRY_REACTION,
            "the button fires the openTextEntry reaction (showDialog keyboard)",
        );
    }

    /// Regression (M13 Text-Entry): a backspace edit acts ONLY on the bound
    /// `ui.textEntry` value and never touches the static "ENTER TEXT" opener label
    /// or the readout's `"ENTRY {}"` format prefix. This pins the reported bug —
    /// "backspace removes characters from the Enter Text label" — to stay fixed.
    ///
    /// Drives the real edit path (`apply_text_edit` against a live `ScriptCtx`)
    /// alongside the readout's drawn-string composition (`resolve_text`-equivalent:
    /// format prefix + bound value), asserting:
    /// - the slot value edits as a pure FIFO/char-pop of what was typed,
    /// - the opener button's `label` and the readout's `content` + `format` are
    ///   never mutated by an edit (they live on separate nodes, distinct from the
    ///   slot the edit targets),
    /// - the readout's drawn string always keeps its `"ENTRY "` prefix — backspace
    ///   shortens only the value tail, never the prefix.
    #[test]
    fn backspace_edits_only_the_bound_value_never_the_label_or_format() {
        use crate::scripting::ctx::ScriptCtx;
        use crate::scripting::primitives::store::{TextEdit, apply_text_edit, read_store_slot};
        use crate::scripting::slot_table::SlotValue;

        // The displayed readout string is the format with `{}` replaced by the
        // current `ui.textEntry` value — the same composition `tree::resolve_text`
        // performs for a bound text node (format present, single placeholder).
        fn drawn_readout(format: &str, value: &str) -> String {
            format.replacen("{}", value, 1)
        }

        let tree = build_pause_menu_descriptor();
        let Widget::VStack(col) = &tree.root else {
            panic!("pause menu root is a vstack column");
        };

        // Snapshot the immutable authored strings BEFORE any edit.
        let Widget::Text(readout) = &col.children[4] else {
            panic!("fifth row is the ui.textEntry readout text");
        };
        let readout_content = readout.content.clone();
        let readout_format = readout
            .bind
            .as_ref()
            .and_then(|b| b.format.clone())
            .expect("readout binds with a format");
        let readout_slot = readout
            .bind
            .as_ref()
            .map(|b| b.slot.clone())
            .expect("readout binds a slot");

        let Widget::Button(opener) = &col.children[5] else {
            panic!("sixth row is the open-text-entry button");
        };
        let opener_label = opener.label.clone();

        // Preconditions: the label and readout-format are the strings a careless
        // edit could eat into; they are NOT the slot the edit targets.
        assert_eq!(opener_label, "ENTER TEXT");
        assert_eq!(readout_format, "ENTRY {}");
        assert_eq!(readout_slot, "ui.textEntry");
        assert_ne!(
            readout_slot, "input.mode",
            "the readout binds the text-entry slot, not an unrelated one",
        );

        // Drive the real edit path against a live store: type, then backspace.
        let ctx = ScriptCtx::new();
        for ch in ["a", "b", "c"] {
            apply_text_edit(&ctx, &readout_slot, TextEdit::Append(ch)).unwrap();
        }
        let SlotValue::String(typed) = read_store_slot(&ctx, &readout_slot).unwrap() else {
            panic!("ui.textEntry is a string slot");
        };
        assert_eq!(typed, "abc", "appends land on the bound value");
        // The readout draws the value behind its untouched "ENTRY " prefix.
        assert_eq!(drawn_readout(&readout_format, &typed), "ENTRY abc");

        apply_text_edit(&ctx, &readout_slot, TextEdit::Backspace).unwrap();
        let SlotValue::String(after) = read_store_slot(&ctx, &readout_slot).unwrap() else {
            panic!("ui.textEntry is a string slot");
        };
        // Backspace shortened ONLY the value tail.
        assert_eq!(after, "ab", "backspace pops one char off the bound value");
        // The drawn readout keeps its "ENTRY " prefix; only the value tail shrank.
        let drawn = drawn_readout(&readout_format, &after);
        assert_eq!(drawn, "ENTRY ab");
        assert!(
            drawn.starts_with("ENTRY "),
            "backspace never eats into the format prefix",
        );

        // Backspace to empty, then once more on empty (no-op, no underflow): the
        // value bottoms out at "" and the prefix is still intact — it can never be
        // consumed.
        apply_text_edit(&ctx, &readout_slot, TextEdit::Backspace).unwrap();
        apply_text_edit(&ctx, &readout_slot, TextEdit::Backspace).unwrap();
        apply_text_edit(&ctx, &readout_slot, TextEdit::Backspace).unwrap();
        let SlotValue::String(emptied) = read_store_slot(&ctx, &readout_slot).unwrap() else {
            panic!("ui.textEntry is a string slot");
        };
        assert_eq!(emptied, "", "backspace floors at empty, never negative");
        assert_eq!(
            drawn_readout(&readout_format, &emptied),
            "ENTRY ",
            "an empty value still renders the full, intact prefix",
        );

        // The authored descriptor strings are unchanged by the whole edit sequence
        // — the edit only ever touched the slot, never the label/format nodes.
        let tree_after = build_pause_menu_descriptor();
        let Widget::VStack(col_after) = &tree_after.root else {
            panic!("pause menu root is a vstack column");
        };
        let Widget::Button(opener_after) = &col_after.children[5] else {
            panic!("sixth row is the open-text-entry button");
        };
        assert_eq!(
            opener_after.label, opener_label,
            "the ENTER TEXT opener label is immutable across edits",
        );
        let Widget::Text(readout_after) = &col_after.children[4] else {
            panic!("fifth row is the readout");
        };
        assert_eq!(
            readout_after.content, readout_content,
            "the readout's literal content fallback is immutable across edits",
        );
        assert_eq!(
            readout_after.bind.as_ref().and_then(|b| b.format.clone()),
            Some(readout_format),
            "the readout's format prefix is immutable across edits",
        );
    }

    /// The `nav.menu` toggle pushes/pops the registered pause menu through the
    /// modal stack (the exact sequence `App::toggle_pause_menu` runs): a first
    /// toggle pushes the capturing menu (gameplay → menu), a second pops it back
    /// (menu → gameplay). Pins that the registered descriptor captures and that
    /// the registry name matches what the App pushes.
    #[test]
    fn nav_menu_toggle_pushes_then_pops_the_pause_menu() {
        use crate::input::UiCaptureMode;
        use crate::render::ui::modal_stack::ModalStack;

        let mut stack = ModalStack::new();
        stack
            .registry_mut()
            .register(super::PAUSE_MENU_NAME, build_pause_menu_descriptor());

        // No capturing tree up: gameplay keeps input.
        assert_eq!(stack.top_capture_mode(), UiCaptureMode::Passthrough);
        assert_ne!(stack.active_name(), Some(super::PAUSE_MENU_NAME));

        // First `nav.menu`: push the pause menu (it captures → menu focus).
        stack.push_named(super::PAUSE_MENU_NAME, None);
        assert_eq!(stack.active_name(), Some(super::PAUSE_MENU_NAME));
        assert_eq!(
            stack.top_capture_mode(),
            UiCaptureMode::Capture,
            "the pushed pause menu captures input",
        );

        // Second `nav.menu`: the menu is the top tree, so it pops back to gameplay.
        stack.pop();
        assert_ne!(stack.active_name(), Some(super::PAUSE_MENU_NAME));
        assert_eq!(stack.top_capture_mode(), UiCaptureMode::Passthrough);
    }

    /// The `screen.flash` swatch (fifth HUD row): a panel bound to the engine-
    /// owned `screen.flash` surface, rendering the flash-decay state's output.
    #[test]
    fn demo_descriptor_binds_screen_flash_surface() {
        let tree = build_demo_descriptor();
        let Widget::VStack(col) = &tree.root else {
            panic!("demo root is a vstack column");
        };
        let Widget::Grid(grid) = &col.children[4] else {
            panic!("fifth row is the screen.flash grid");
        };
        let Widget::Panel(panel) = &grid.children[0] else {
            panic!("screen.flash grid's first cell is the bound panel");
        };
        assert_eq!(
            panel.bind.as_ref().map(|b| b.slot.as_str()),
            Some("screen.flash"),
            "the panel binds the engine-owned screen.flash surface",
        );
        assert_eq!(
            panel.fill,
            ColorValue::Literal(SCREEN_FLASH_FALLBACK_FILL),
            "screen.flash swatch falls back to transparent at rest",
        );
    }

    /// End-to-end gamepad navigability of the pause menu (regression fix for the
    /// pause-menu gamepad-nav review finding): the root's linear focus policy must
    /// open a `FocusGroup` so directional nav moves focus
    /// between the interactive widgets. Loads the descriptor, exports its focus
    /// rects through the SAME path the renderer→focus-engine seam uses
    /// (`UiTree::export_focus_rects`), then drives `UiFocusEngine` with `Nav(Down)`
    /// / `Nav(Up)` and asserts focus walks Resume → volume slider → Enter-Text and
    /// back. Regression: a `focus: None` root opened no group, so `move_focus`
    /// early-returned and the menu was un-navigable by D-pad/stick.
    #[test]
    fn pause_menu_gamepad_nav_walks_resume_slider_enter_text_and_wraps() {
        use crate::input::{InputMode, NavIntent, UiFocusEngine};
        use crate::render::ui::theme::UiTheme;
        use crate::render::ui::tree::{ImageSizes, UiTree};
        use crate::scripting::slot_table::SlotValue;
        use std::collections::HashMap;

        let tree = build_pause_menu_descriptor();
        let theme = UiTheme::engine_default();
        let mut ui = UiTree::from_descriptor(&tree, &theme);
        let mut font_system = crate::render::ui::text::build_font_system();
        let images = ImageSizes::new();
        let slots: HashMap<String, SlotValue> = HashMap::new();
        // Lay out + export the focus rects exactly as the renderer does each frame.
        ui.build_draw_data([1280, 720], &mut font_system, &images, &slots);
        let rects = ui.export_focus_rects(&tree, [1280, 720]);

        // The interactive widgets export as focusable in tree order under one group.
        let ids: Vec<&str> = rects.rects.iter().map(|r| r.id.as_str()).collect();
        assert!(
            ids.contains(&PAUSE_RESUME_ID)
                && ids.contains(&PAUSE_VOLUME_ID)
                && ids.contains(&PAUSE_TEXT_ENTRY_ID),
            "Resume, volume slider, and Enter-Text all export as focusable nodes",
        );
        assert!(
            !rects.groups.is_empty(),
            "the linear focus policy opens a FocusGroup (the un-navigable bug had none)",
        );

        let mut fe = UiFocusEngine::new();
        let drive = |fe: &mut UiFocusEngine, intent: Option<NavIntent>| {
            let intents: Vec<NavIntent> = intent.into_iter().collect();
            fe.tick(
                Some(super::PAUSE_MENU_NAME),
                Some(&rects),
                &intents,
                None,
                &[],
                InputMode::Focus,
                0.0,
            )
            .focused
        };

        // Initial focus is Resume (the tree's initialFocus).
        assert_eq!(drive(&mut fe, None).as_deref(), Some(PAUSE_RESUME_ID));
        // Down: Resume → volume slider.
        assert_eq!(
            drive(&mut fe, Some(NavIntent::Down)).as_deref(),
            Some(PAUSE_VOLUME_ID),
            "down moves Resume → volume slider",
        );
        // Down: volume slider → Enter-Text.
        assert_eq!(
            drive(&mut fe, Some(NavIntent::Down)).as_deref(),
            Some(PAUSE_TEXT_ENTRY_ID),
            "down moves volume slider → Enter-Text",
        );
        // Down again wraps the chain: Enter-Text → Resume.
        assert_eq!(
            drive(&mut fe, Some(NavIntent::Down)).as_deref(),
            Some(PAUSE_RESUME_ID),
            "down wraps Enter-Text → Resume",
        );
        // Up walks the chain back the other way: Resume → Enter-Text (wrap up).
        assert_eq!(
            drive(&mut fe, Some(NavIntent::Up)).as_deref(),
            Some(PAUSE_TEXT_ENTRY_ID),
            "up wraps Resume → Enter-Text",
        );
        assert_eq!(
            drive(&mut fe, Some(NavIntent::Up)).as_deref(),
            Some(PAUSE_VOLUME_ID),
            "up moves Enter-Text → volume slider",
        );
        assert_eq!(
            drive(&mut fe, Some(NavIntent::Up)).as_deref(),
            Some(PAUSE_RESUME_ID),
            "up moves volume slider → Resume",
        );
    }
}
