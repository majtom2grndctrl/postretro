// CPU gate for the engine fallback HUD descriptor.
// See: context/lib/ui.md

use std::collections::HashMap;

use super::demo::build_demo_descriptor;
use super::text::{UI_MONO_FONT_FAMILY, measure_run};
use super::theme::UiTheme;
use super::tree::{ImageSizes, UiDrawData, UiTree};
use crate::scripting::slot_table::SlotValue;

const FALLBACK_MARKER: &str = "FALLBACK HUD HP --";
const FALLBACK_HEALTH: &str = "FALLBACK HUD HP 42";

fn font_system() -> glyphon::FontSystem {
    super::text::build_font_system()
}

fn no_images() -> ImageSizes {
    ImageSizes::new()
}

fn no_cells() -> super::tree::CellValues {
    super::tree::CellValues::new()
}

fn health_slots(health: f32) -> HashMap<String, SlotValue> {
    let mut slots = HashMap::new();
    slots.insert("player.health".to_string(), SlotValue::Number(health));
    slots
}

fn render_fallback(health: f32) -> UiDrawData {
    let tree = build_demo_descriptor();
    let mut ui = UiTree::from_descriptor(&tree, &UiTheme::engine_default());
    let mut fs = font_system();
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &health_slots(health),
        &no_cells(),
        0.0,
    )
}

#[test]
fn fallback_hud_resolves_health_text_through_retained_path() {
    let data = render_fallback(42.0);
    let contents: Vec<&str> = data.texts.iter().map(|t| t.content.as_str()).collect();
    assert!(
        contents.contains(&FALLBACK_HEALTH),
        "fallback health bind resolved through retained draw data, got {contents:?}",
    );
    assert!(
        !contents.contains(&FALLBACK_MARKER),
        "bound health replaces the fallback marker once player.health is present",
    );
}

#[test]
fn fallback_hud_marker_is_fallback_only_and_no_demo_surfaces_render() {
    let data = render_fallback(42.0);
    let contents: Vec<&str> = data.texts.iter().map(|t| t.content.as_str()).collect();
    for removed in ["AMMO", "FLASH", "SCREEN.FLASH"] {
        assert!(
            contents.iter().all(|content| !content.contains(removed)),
            "removed demo surface {removed:?} must not render: {contents:?}",
        );
    }
    assert_eq!(
        data.quads.instances.len(),
        0,
        "fallback HUD is text-only; production mod HUD owns panels and bars",
    );
}

#[test]
fn fallback_hud_uses_mono_theme_font() {
    let data = render_fallback(42.0);
    let label = data
        .texts
        .iter()
        .find(|t| t.content == FALLBACK_HEALTH)
        .expect("fallback health text renders");
    assert_eq!(
        label.family, UI_MONO_FONT_FAMILY,
        "fallback marker resolves through the mono token",
    );

    let mut fs = font_system();
    let (mono_w, _) = measure_run(
        &mut fs,
        FALLBACK_HEALTH,
        label.font_size,
        UI_MONO_FONT_FAMILY,
    );
    assert!(
        mono_w > 0.0,
        "mono fallback text must measure to a visible width",
    );
}
