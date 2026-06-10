// Hard-gate CPU assertion for the M13 Goal C demo gameplay HUD.
//
// Task 5 makes `demo::build_demo_descriptor` the first gameplay UI producer:
// `main.rs` publishes its `AnchoredTree` on the per-frame read snapshot and the
// renderer drives it through the retained gameplay `UiTree`
// (`UiPass::layout_gameplay_tree` → `UiTree::build_draw_data_retained`). These
// tests drive the REAL demo descriptor (not a hand-built fixture) through that
// same retained path with a slot-value map, proving the demo wiring end-to-end:
//
//   - Bind resolution: `player.health`/`player.ammo` Number slots resolve into
//     the formatted text runs ("HP 100" / "AMMO 50") and `intro.flashColor`
//     resolves into the swatch panel's quad color.
//   - Subscriber-aware diff + split: an appearance-only `intro.flashColor` change
//     rebuilds the draw list WITHOUT a relayout (`recompute_count` flat); a bound
//     text-content change (`player.health` 100→87) re-measures and DOES relayout.
//   - Settle frame: after the flash holds constant, a no-change frame performs no
//     draw-list rebuild and no recompute (the cached list is returned).
//   - Subscriber-awareness: an unbound slot changing invalidates nothing.
//
// These overlap Task 4's tree-level diff tests, but here they run through the
// actual demo descriptor — that is the point: prove the demo screen's wiring, not
// just the retained primitive. Pure CPU — no GPU adapter, no wgpu call.
//
// See: context/plans/in-progress/M13--state-system

use std::collections::HashMap;

use super::demo::build_demo_descriptor;
use super::tree::{ImageSizes, UiDrawData, UiTree};
use crate::scripting::slot_table::SlotValue;

const EPS: f32 = 1e-3;

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() <= EPS
}

fn font_system() -> glyphon::FontSystem {
    super::text::build_font_system()
}

/// The demo HUD carries no `image` nodes, so the measure seam never looks
/// anything up.
fn no_images() -> ImageSizes {
    ImageSizes::new()
}

/// A slot map matching the Task 2 proxy's at-rest writes: `player.health`=100,
/// `player.ammo`=50 (Number), and `intro.flashColor`=`rgba` (length-4 linear
/// RGBA array). The flash color is parameterized so the appearance-change tests
/// can vary it while health/ammo hold.
fn proxy_slots(health: f32, ammo: f32, flash: [f32; 4]) -> HashMap<String, SlotValue> {
    let mut slots = HashMap::new();
    slots.insert("player.health".to_string(), SlotValue::Number(health));
    slots.insert("player.ammo".to_string(), SlotValue::Number(ammo));
    slots.insert(
        "intro.flashColor".to_string(),
        SlotValue::Array(flash.to_vec()),
    );
    slots
}

/// The demo descriptor's fallback swatch fill (linear RGBA). The swatch panel
/// resolves `intro.flashColor` over this literal, so a quad carrying a DIFFERENT
/// color is the resolved flash (not the fallback).
const FALLBACK_FILL: [f32; 4] = [0.0, 0.65, 0.75, 1.0];

fn colors_eq(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| approx(*x, *y))
}

/// The resolved swatch-panel color from a built draw list: the demo's only quad
/// is the bound swatch panel (the HUD column carries no backdrop fill), so the
/// first quad's color is the resolved flash.
fn swatch_color(data: &UiDrawData) -> Option<[f32; 4]> {
    data.quads.instances.first().map(|q| q.color)
}

#[test]
fn demo_descriptor_resolves_binds_through_retained_path() {
    // Acceptance — bind resolution: with player.health=100, player.ammo=50, and
    // intro.flashColor set, the demo draw data carries the formatted text strings
    // and the swatch quad carries the resolved color.
    let tree = build_demo_descriptor();
    let mut ui = UiTree::from_descriptor(&tree);
    let mut fs = font_system();

    let flash = [0.0, 0.8, 0.9, 1.0];
    let data = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &proxy_slots(100.0, 50.0, flash),
    );

    // Both bound text runs resolve through their format templates.
    let contents: Vec<&str> = data.texts.iter().map(|t| t.content.as_str()).collect();
    assert!(
        contents.contains(&"HP 100"),
        "health bind resolved into 'HP 100', got {contents:?}",
    );
    assert!(
        contents.contains(&"AMMO 50"),
        "ammo bind resolved into 'AMMO 50', got {contents:?}",
    );

    // The swatch quad carries the resolved flash color (not the literal fallback).
    let color = swatch_color(&data).expect("the demo draws a swatch quad");
    assert!(
        colors_eq(color, flash),
        "swatch quad carries the resolved flash color {flash:?}, got {color:?}",
    );
    assert!(
        !colors_eq(color, FALLBACK_FILL),
        "resolved color must override the literal fallback fill",
    );
}

#[test]
fn demo_panel_fill_change_rebuilds_without_relayout() {
    // Acceptance — appearance-only split: flipping intro.flashColor (panel fill)
    // rebuilds the draw list with the new color but does NOT increment
    // recompute_count (no relayout — a bound fill is appearance-only).
    let tree = build_demo_descriptor();
    let mut ui = UiTree::from_descriptor(&tree);
    let mut fs = font_system();

    let solid = [0.0, 0.65, 0.75, 1.0];
    let first = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &proxy_slots(100.0, 50.0, solid),
    );
    assert_eq!(ui.recompute_count(), 1, "first frame computes once");
    assert_eq!(ui.draw_rebuild_count(), 1, "first frame builds the list");
    assert!(
        swatch_color(&first).is_some_and(|c| colors_eq(c, solid)),
        "first frame draws the solid flash color",
    );

    // Pulse color: only intro.flashColor changes; health/ammo hold.
    let pulse = [0.0, 0.80, 0.90, 1.0];
    let second = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &proxy_slots(100.0, 50.0, pulse),
    );
    assert_eq!(
        ui.recompute_count(),
        1,
        "appearance-only flash change must not relayout",
    );
    assert_eq!(
        ui.draw_rebuild_count(),
        2,
        "appearance-only change rebuilds the draw list",
    );
    assert!(
        swatch_color(&second).is_some_and(|c| colors_eq(c, pulse)),
        "the draw list reflects the new flash color",
    );
}

#[test]
fn demo_bound_text_change_triggers_relayout() {
    // Acceptance — content-change relayout: changing a bound text value
    // (player.health 100→87) re-measures the run and DOES increment
    // recompute_count.
    let tree = build_demo_descriptor();
    let mut ui = UiTree::from_descriptor(&tree);
    let mut fs = font_system();

    let flash = [0.0, 0.65, 0.75, 1.0];
    let first = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &proxy_slots(100.0, 50.0, flash),
    );
    assert_eq!(ui.recompute_count(), 1, "first frame computes once");
    assert!(
        first.texts.iter().any(|t| t.content == "HP 100"),
        "first frame draws 'HP 100'",
    );

    // Health drops 100 → 87; the formatted run width changes, forcing a relayout.
    let second = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &proxy_slots(87.0, 50.0, flash),
    );
    assert_eq!(
        ui.recompute_count(),
        2,
        "a bound text-content change relays out",
    );
    assert!(
        second.texts.iter().any(|t| t.content == "HP 87"),
        "the new health value is drawn",
    );
}

#[test]
fn demo_settled_frame_skips_rebuild_and_recompute() {
    // Acceptance — settle frame: after the flash holds constant (and health/ammo
    // hold), a no-change frame performs NO draw-list rebuild and NO recompute —
    // the dirty-gate short-circuits and the cached list is returned.
    let tree = build_demo_descriptor();
    let mut ui = UiTree::from_descriptor(&tree);
    let mut fs = font_system();

    let settled = proxy_slots(100.0, 50.0, [0.0, 0.65, 0.75, 1.0]);
    let first = ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &settled);
    assert_eq!(ui.recompute_count(), 1, "first frame computes once");
    assert_eq!(ui.draw_rebuild_count(), 1, "first frame builds the list");

    // Identical snapshot again: nothing dirtied, so neither layout nor the draw
    // list rebuilds — the cached list is returned unchanged.
    let second = ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &settled);
    assert_eq!(ui.recompute_count(), 1, "settled frame does not relayout");
    assert_eq!(
        ui.draw_rebuild_count(),
        1,
        "settled frame returns the cached draw list (no rebuild)",
    );
    assert_eq!(
        first.quads.instances.len(),
        second.quads.instances.len(),
        "cached list matches the first build",
    );
    assert_eq!(
        first.texts.len(),
        second.texts.len(),
        "cached text runs match the first build",
    );
}

#[test]
fn demo_unbound_slot_change_invalidates_nothing() {
    // Acceptance — subscriber-awareness: a slot with no binding in the demo tree
    // changing value must invalidate nothing — no relayout, no draw-list rebuild.
    // The demo binds player.health/player.ammo/intro.flashColor; we change an
    // unrelated `world.kills` slot.
    let tree = build_demo_descriptor();
    let mut ui = UiTree::from_descriptor(&tree);
    let mut fs = font_system();

    let mut slots = proxy_slots(100.0, 50.0, [0.0, 0.65, 0.75, 1.0]);
    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots);
    assert_eq!(ui.recompute_count(), 1);
    assert_eq!(ui.draw_rebuild_count(), 1, "first frame builds the list");

    // Add/changes only an unbound slot; every bound slot holds its value.
    slots.insert("world.kills".to_string(), SlotValue::Number(7.0));
    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots);
    assert_eq!(
        ui.recompute_count(),
        1,
        "an unbound slot change must not relayout",
    );
    assert_eq!(
        ui.draw_rebuild_count(),
        1,
        "an unbound slot change must not rebuild the draw list",
    );
}

#[test]
fn demo_swatch_quad_has_real_size_presence() {
    // The flash swatch must draw a visible quad, not a degenerate zero-area rect:
    // the demo gives the bound (intrinsically sizeless) panel real presence so the
    // swatch reads as a visible block. Assert the resolved swatch quad's width and
    // height are both non-trivial.
    let tree = build_demo_descriptor();
    let mut ui = UiTree::from_descriptor(&tree);
    let mut fs = font_system();
    let data = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &proxy_slots(100.0, 50.0, [0.0, 0.65, 0.75, 1.0]),
    );
    // Find the resolved swatch quad (the one carrying the flash color, not a
    // backdrop), and assert it has real width and height.
    let swatch = data
        .quads
        .instances
        .iter()
        .find(|q| colors_eq(q.color, [0.0, 0.65, 0.75, 1.0]))
        .expect("the demo draws a swatch quad carrying the flash color");
    assert!(
        swatch.rect[2] > 1.0 && swatch.rect[3] > 1.0,
        "swatch quad has real size presence (w,h) = ({}, {})",
        swatch.rect[2],
        swatch.rect[3],
    );
}
