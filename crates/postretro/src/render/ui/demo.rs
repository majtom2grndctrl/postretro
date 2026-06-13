// M13 demo gameplay HUD descriptor: the hardcoded Rust description of the
// state-binding demo, authored in 1280x720 logical-reference space and laid out
// through the retained gameplay `UiTree` (`UiPass::layout_gameplay_tree`). It is
// the FIRST gameplay UI producer — `main.rs` publishes this tree on the
// once-per-frame read snapshot, so the renderer drives it through the
// subscriber-aware retained path.
//
// This is a demo, not a HUD design: three bound nodes prove the binding seam
// end-to-end. Two `text` nodes bind `player.health` / `player.ammo` (Number →
// formatted string); one `panel` binds `intro.flashColor` (length-4 linear RGBA
// array → fill). The descriptor is structurally identical every frame, so the
// retained tree reuses it and only the bound VALUES drive the diff (text-content
// change relays out; panel-fill change is appearance-only).
//
// See: context/lib/scripting.md §3 (defineStore / DefinitionOnly) ·
//      context/lib/ui.md

use super::descriptor::{
    Align, AnchoredTree, ButtonWidget, CaptureMode, ColorValue, ContainerWidget, Easing,
    GridWidget, PanelBind, PanelTween, PanelWidget, SliderBind, SliderWidget, SpacingValue,
    TextBind, TextTween, TextWidget, Widget,
};
use super::layout::Anchor;
use super::style_ranges::{StyleEntry, StyleRanges};

/// HUD text color token. The readouts (`player.health` / `player.ammo`) show a
/// nominal at-rest state, so the `ok` token (the theme's green) reads as a
/// healthy readout — `critical` (hot red) would imply a danger/low state. This
/// resolves against the active theme at build time, exercising token resolution
/// on a live screen (not just fixtures). The swatch label below uses the same
/// readout color.
const HUD_TEXT_COLOR_TOKEN: &str = "ok";

/// HUD text size, logical-reference px.
const HUD_FONT_SIZE: f32 = 28.0;

/// Literal fallback fill for the flash panel, linear RGBA. Used when the
/// `intro.flashColor` slot is absent or malformed — the same solid intro color
/// the Task 2 proxy holds at rest (`[0.0, 0.65, 0.75, 1.0]`), so the swatch reads
/// correctly even before the proxy's first write lands.
const FLASH_FALLBACK_FILL: [f32; 4] = [0.0, 0.65, 0.75, 1.0];

/// The flash swatch's label, shown beside the bound color block. A bare `panel`
/// has no intrinsic size (it fills its flex/grid slot), so it cannot establish a
/// box on its own. The swatch is a 2-column grid pairing the bound panel with
/// this measured label: the grid's `1fr` tracks give the panel cell a real width
/// (the grid content-sizes to the label), and grid items stretch to fill their
/// cell in both axes, so the panel block reads as a visible swatch the height of
/// the label row.
const SWATCH_LABEL: &str = "FLASH";

/// The `screen.flash` swatch's label. The engine-owned `screen.flash` surface
/// is the published flash slot a drained `flashScreen` system reaction feeds
/// (via the App-side flash-decay state, decaying to transparent). The demo
/// binds it on a panel here to render the decay end-to-end. Making the flash
/// literally cover the screen is the post-UI-effects (SE) compositor wave's job;
/// this demo proves the bind + decay seam, not the full-screen composite.
const SCREEN_FLASH_LABEL: &str = "SCREEN.FLASH";

/// Fallback fill for the `screen.flash` swatch before the first decay write —
/// transparent, matching the slot's resting value (the swatch reads as nothing
/// at rest, then shows the decaying flash color when a flashScreen fires).
const SCREEN_FLASH_FALLBACK_FILL: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

/// The health-bar swatch's label, shown beside the styleRanges-driven block.
/// Like the flash swatch, a bare bound `panel` has no intrinsic size, so the
/// bar is paired with this measured label in a stretch grid to draw a visible
/// block. (`bar` is Goal F; the demo uses `panel` + `styleRanges` instead.)
const HEALTH_BAR_LABEL: &str = "HEALTH";

/// Fraction-of-max bounds for the health-bar styleRanges bands. The bar is the
/// FIRST screen consumer of `styleRanges` on a `panel`: `player.health / max`
/// drives a three-band color (critical red ≤ 20%, warning amber ≤ 50%, ok green
/// default), demonstrating the continuous value→style map the crossing watcher
/// also keys off. The 20% bound matches the demo crossing in the dev map's
/// `setupLevel` (which fires `flashScreen` + `playSound` on the same threshold).
const HEALTH_CRITICAL_UP_TO: f32 = 0.2;
const HEALTH_WARNING_UP_TO: f32 = 0.5;

/// Max health the bar normalizes against (`player.health / max`). Matches the
/// dev pawn's authored 100 HP ceiling.
const HEALTH_BAR_MAX: f32 = 100.0;

/// Fallback fill for the health bar before the first `player.health` write or a
/// no-match value — the `ok` green endpoint so the bar reads healthy at rest.
const HEALTH_BAR_FALLBACK_FILL: [f32; 4] = [0.0, 0.85, 0.35, 1.0];

/// Vertical gap between the HUD rows (logical-reference px).
const ROW_GAP: f32 = 10.0;

/// Gap between the swatch color block and its label inside the swatch grid
/// (logical-reference px).
const SWATCH_GAP: f32 = 8.0;

/// Outer HUD padding from the anchored corner (logical-reference px).
const HUD_PADDING: f32 = 16.0;

/// First-resolve count-up duration for the health readout (ms). The proxy's
/// `player.health` target is the constant `100`, so this is a pure first-resolve
/// `from: 0` flourish (a 0→100 count-up on appear), NOT an authoritative ramp:
/// the value still snaps to whatever the slot reports — it just eases there from
/// `0` the first time it is seen.
const HEALTH_TWEEN_MS: f32 = 1200.0;

/// Ease duration for the flash swatch (ms). The proxy toggles `intro.flashColor`
/// between two endpoints every 500 ms (a hard step); a short tween with no `from`
/// smooths each toggle into a 150 ms cross-fade instead of a snap.
const FLASH_TWEEN_MS: f32 = 150.0;

/// Build the demo gameplay HUD descriptor. Returns a bottom-left-anchored
/// `AnchoredTree` carrying:
/// - a `text` bound to `player.health` (format `"HP {}"`),
/// - a `text` bound to `player.ammo` (format `"AMMO {}"`),
/// - a `panel` whose `fill` binds `intro.flashColor` (length-4 linear RGBA),
///   wrapped in a sized container so it draws a visible swatch.
///
/// The descriptor is structurally identical every frame; the retained gameplay
/// tree reuses it and only the bound values drive the per-frame diff. The
/// envelope mirrors `splash::build_splash_descriptor`'s composition style.
pub(crate) fn build_demo_descriptor() -> AnchoredTree {
    let health = Widget::Text(TextWidget {
        content: "HP --".to_string(),
        font_size: HUD_FONT_SIZE,
        color: ColorValue::Token(HUD_TEXT_COLOR_TOKEN.into()),
        font: None,
        bind: Some(TextBind {
            slot: "player.health".to_string(),
            format: Some("HP {}".to_string()),
            // First-resolve flourish: count up 0→100 over 1.2s with easeOut. The
            // proxy holds `player.health` at a constant 100, so this is purely the
            // `from: 0` first-resolve ramp, not an authoritative value change.
            tween: Some(TextTween {
                duration_ms: HEALTH_TWEEN_MS,
                easing: Easing::EaseOut,
                from: Some(0.0),
            }),
        }),
        style_ranges: None,
        id: None,
        focus_neighbors: Default::default(),
    });

    let ammo = Widget::Text(TextWidget {
        content: "AMMO --".to_string(),
        font_size: HUD_FONT_SIZE,
        color: ColorValue::Token(HUD_TEXT_COLOR_TOKEN.into()),
        font: None,
        bind: Some(TextBind {
            slot: "player.ammo".to_string(),
            format: Some("AMMO {}".to_string()),
            tween: None,
        }),
        style_ranges: None,
        id: None,
        focus_neighbors: Default::default(),
    });

    // The flash swatch: a bound panel paired with a measured label in a 2-column
    // grid. A bare panel has no intrinsic size, so it cannot draw a block alone;
    // pairing it with the label in a `Stretch` grid gives the panel cell a real
    // width (the grid's `1fr` tracks content-size to the label) and a real height
    // (grid items stretch to the label-row height), so the bound color reads as a
    // visible swatch. The panel binds `intro.flashColor`; the literal `fill` is
    // the fallback when the slot is absent/malformed.
    let swatch_panel = Widget::Panel(PanelWidget {
        fill: ColorValue::Literal(FLASH_FALLBACK_FILL),
        border: None,
        bind: Some(PanelBind {
            // Ease each 500 ms proxy toggle into a 150 ms cross-fade (no `from`,
            // so the first sight snaps to the live color, then eases on changes).
            slot: "intro.flashColor".to_string(),
            tween: Some(PanelTween {
                duration_ms: FLASH_TWEEN_MS,
                easing: Easing::EaseInOut,
                from: None,
            }),
        }),
        style_ranges: None,
        id: None,
        focus_neighbors: Default::default(),
    });
    let swatch_label = Widget::Text(TextWidget {
        content: SWATCH_LABEL.to_string(),
        font_size: HUD_FONT_SIZE,
        color: ColorValue::Token(HUD_TEXT_COLOR_TOKEN.into()),
        // Shape the swatch label against the `mono` font token — the second
        // registered face, exercised on a live screen alongside the body face
        // used by the readouts above.
        font: Some("mono".into()),
        bind: None,
        style_ranges: None,
        id: None,
        focus_neighbors: Default::default(),
    });
    let swatch = Widget::Grid(GridWidget {
        gap: SpacingValue::Literal(SWATCH_GAP),
        padding: SpacingValue::Literal(0.0),
        align: Align::Stretch,
        cols: 2,
        id: None,
        focus_neighbors: Default::default(),
        focus: None,
        restore_on_return: false,
        children: vec![swatch_panel, swatch_label],
    });

    // Health bar: a `panel` bound to the numeric `player.health` slot with a
    // three-band `styleRanges` map driving its color (critical/warning/ok). The
    // panel's `bind` slot is a NUMBER slot (not the length-4 fill array) — the
    // styleRanges value path; the band color overrides the literal `fill`. A
    // styleRanges panel demonstrates Goal E's continuous value→style map without
    // the Goal-F `bar` widget. Paired with a measured label in a stretch grid so
    // the intrinsically-sizeless panel draws a visible block.
    let health_bar_panel = Widget::Panel(PanelWidget {
        fill: ColorValue::Literal(HEALTH_BAR_FALLBACK_FILL),
        border: None,
        bind: Some(PanelBind {
            // No tween: the band color steps at each threshold (the styleRanges
            // value is the raw numeric slot, distinct from a fill-array ease).
            slot: "player.health".to_string(),
            tween: None,
        }),
        style_ranges: Some(StyleRanges {
            max: HEALTH_BAR_MAX,
            entries: vec![
                StyleEntry {
                    up_to: Some(HEALTH_CRITICAL_UP_TO),
                    color: Some(ColorValue::Token("critical".into())),
                    pulse: None,
                    flash: None,
                },
                StyleEntry {
                    up_to: Some(HEALTH_WARNING_UP_TO),
                    color: Some(ColorValue::Token("warning".into())),
                    pulse: None,
                    flash: None,
                },
                StyleEntry {
                    up_to: None,
                    color: Some(ColorValue::Token("ok".into())),
                    pulse: None,
                    flash: None,
                },
            ],
        }),
        id: None,
        focus_neighbors: Default::default(),
    });
    let health_bar_label = Widget::Text(TextWidget {
        content: HEALTH_BAR_LABEL.to_string(),
        font_size: HUD_FONT_SIZE,
        color: ColorValue::Token(HUD_TEXT_COLOR_TOKEN.into()),
        font: Some("mono".into()),
        bind: None,
        style_ranges: None,
        id: None,
        focus_neighbors: Default::default(),
    });
    let health_bar = Widget::Grid(GridWidget {
        gap: SpacingValue::Literal(SWATCH_GAP),
        padding: SpacingValue::Literal(0.0),
        align: Align::Stretch,
        cols: 2,
        id: None,
        focus_neighbors: Default::default(),
        focus: None,
        restore_on_return: false,
        children: vec![health_bar_panel, health_bar_label],
    });

    // `screen.flash` swatch: a panel bound to the engine-owned `screen.flash`
    // surface, rendering the flash-decay state's decaying RGBA. Paired with a
    // label in a stretch grid for the same sizing reason as the other swatches.
    let screen_flash_panel = Widget::Panel(PanelWidget {
        fill: ColorValue::Literal(SCREEN_FLASH_FALLBACK_FILL),
        border: None,
        bind: Some(PanelBind {
            // No tween: the App-side decay already smooths the alpha each tick,
            // so the panel renders the decayed value directly.
            slot: "screen.flash".to_string(),
            tween: None,
        }),
        style_ranges: None,
        id: None,
        focus_neighbors: Default::default(),
    });
    let screen_flash_label = Widget::Text(TextWidget {
        content: SCREEN_FLASH_LABEL.to_string(),
        font_size: HUD_FONT_SIZE,
        color: ColorValue::Token(HUD_TEXT_COLOR_TOKEN.into()),
        font: Some("mono".into()),
        bind: None,
        style_ranges: None,
        id: None,
        focus_neighbors: Default::default(),
    });
    let screen_flash = Widget::Grid(GridWidget {
        gap: SpacingValue::Literal(SWATCH_GAP),
        padding: SpacingValue::Literal(0.0),
        align: Align::Stretch,
        cols: 2,
        id: None,
        focus_neighbors: Default::default(),
        focus: None,
        restore_on_return: false,
        children: vec![screen_flash_panel, screen_flash_label],
    });

    // Bottom-left HUD column: health readout, ammo, the flash swatch, then the
    // styleRanges health bar, then the `screen.flash` swatch, padded in from the
    // anchored corner. The `intro.flashColor` swatch stays the first-emitted quad
    // (the gate tests read `quads[0]` as that swatch); the bar and screen-flash
    // quads follow it.
    let root = Widget::VStack(ContainerWidget {
        gap: SpacingValue::Literal(ROW_GAP),
        padding: SpacingValue::Literal(HUD_PADDING),
        align: Align::Start,
        fill: None,
        border: None,
        id: None,
        focus_neighbors: Default::default(),
        focus: None,
        restore_on_return: false,
        children: vec![health, ammo, swatch, health_bar, screen_flash],
    });

    // The demo HUD passes input through to gameplay (a HUD never captures).
    AnchoredTree::passthrough(Anchor::BottomLeft, [0.0, 0.0], root)
}

/// Registry name the pause menu is registered + pushed under (M13 Goal F, Task
/// 5). The App registers the descriptor at boot and pushes/pops it via the
/// engine push/pop API on `nav.menu`.
pub(crate) const PAUSE_MENU_NAME: &str = "pauseMenu";

/// Node ids for the pause-menu widgets. The Resume button's id and the volume
/// slider's id; focus starts on Resume.
const PAUSE_RESUME_ID: &str = "pauseResume";
const PAUSE_VOLUME_ID: &str = "pauseVolume";

/// Named reaction fired when Resume is activated (confirm or click). The demo
/// mod registers this reaction as a `closeDialog` (pop the top tree); firing it
/// through the reaction registry pops the pause menu, the same path a script
/// `closeDialog` takes. Keyboard Escape / gamepad B (`nav.cancel`) and a second
/// Start (`nav.menu`) also close it via the engine toggle, independent of this.
const PAUSE_RESUME_REACTION: &str = "resumePauseMenu";

/// The `audio.master` slider's range and step. Amplitude `[0, 1]`; the App-side
/// consumer converts the bound amplitude to decibels and applies it to the audio
/// main bus, so dragging this slider audibly changes volume.
const VOLUME_MIN: f32 = 0.0;
const VOLUME_MAX: f32 = 1.0;
const VOLUME_STEP: f32 = 0.1;

/// Build the demo pause-menu descriptor (M13 Goal F, Task 5): a centered,
/// capturing modal with a Resume button and an `audio.master`-bound volume
/// slider, fully gamepad-navigable. Pushed/popped via `nav.menu` (gamepad Start /
/// Escape-from-gameplay) through the engine push/pop API.
///
/// The slider captures `nav.left`/`nav.right` so the left stick / D-pad steps the
/// bound amplitude (a `setState` write to `audio.master` on the N+1 frame); up/
/// down moves focus between the two widgets. The tree captures input (freezes
/// gameplay, releases the cursor), and focus starts on the Resume button.
pub(crate) fn build_pause_menu_descriptor() -> AnchoredTree {
    let title = Widget::Text(TextWidget {
        content: "PAUSED".to_string(),
        font_size: HUD_FONT_SIZE,
        color: ColorValue::Token(HUD_TEXT_COLOR_TOKEN.into()),
        font: Some("mono".into()),
        bind: None,
        style_ranges: None,
        id: None,
        focus_neighbors: Default::default(),
    });

    // A `text` bound to `input.mode` displays the live pointer-vs-focus mode
    // (the engine-owned slot Task 5 writes). No format → the raw enum string
    // (`"pointer"` / `"focus"`) renders; this is the demo's CPU-asserted proof
    // that the mode slot drives a bound widget.
    let mode_readout = Widget::Text(TextWidget {
        content: "MODE --".to_string(),
        font_size: HUD_FONT_SIZE,
        color: ColorValue::Token(HUD_TEXT_COLOR_TOKEN.into()),
        font: Some("mono".into()),
        bind: Some(TextBind {
            slot: "input.mode".to_string(),
            format: Some("MODE {}".to_string()),
            tween: None,
        }),
        style_ranges: None,
        id: None,
        focus_neighbors: Default::default(),
    });

    let resume = Widget::Button(ButtonWidget {
        id: PAUSE_RESUME_ID.to_string(),
        label: "RESUME".to_string(),
        on_press: PAUSE_RESUME_REACTION.to_string(),
        focus_neighbors: Default::default(),
        repeat_on_hold: None,
    });

    let volume = Widget::Slider(SliderWidget {
        id: PAUSE_VOLUME_ID.to_string(),
        label: "VOLUME".to_string(),
        bind: SliderBind {
            slot: "audio.master".to_string(),
            tween: None,
        },
        min: VOLUME_MIN,
        max: VOLUME_MAX,
        step: VOLUME_STEP,
        // Left/right step the volume; up/down move focus between widgets.
        captures_nav: vec!["nav.left".to_string(), "nav.right".to_string()],
        focus_neighbors: Default::default(),
    });

    let root = Widget::VStack(ContainerWidget {
        gap: SpacingValue::Literal(ROW_GAP),
        padding: SpacingValue::Literal(HUD_PADDING),
        align: Align::Stretch,
        fill: None,
        border: None,
        id: None,
        focus_neighbors: Default::default(),
        focus: None,
        restore_on_return: false,
        children: vec![title, mode_readout, resume, volume],
    });

    AnchoredTree {
        anchor: Anchor::Center,
        offset: [0.0, 0.0],
        root,
        capture_mode: CaptureMode::Capture,
        initial_focus: Some(PAUSE_RESUME_ID.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // title, input.mode readout, resume button, volume slider
        assert_eq!(col.children.len(), 4);

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
            .register(PAUSE_MENU_NAME, build_pause_menu_descriptor());

        // No capturing tree up: gameplay keeps input.
        assert_eq!(stack.top_capture_mode(), UiCaptureMode::Passthrough);
        assert_ne!(stack.active_name(), Some(PAUSE_MENU_NAME));

        // First `nav.menu`: push the pause menu (it captures → menu focus).
        stack.push_named(PAUSE_MENU_NAME, None);
        assert_eq!(stack.active_name(), Some(PAUSE_MENU_NAME));
        assert_eq!(
            stack.top_capture_mode(),
            UiCaptureMode::Capture,
            "the pushed pause menu captures input",
        );

        // Second `nav.menu`: the menu is the top tree, so it pops back to gameplay.
        stack.pop();
        assert_ne!(stack.active_name(), Some(PAUSE_MENU_NAME));
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
}
