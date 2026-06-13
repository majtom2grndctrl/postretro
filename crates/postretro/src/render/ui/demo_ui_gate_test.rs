// Hard-gate CPU assertion for the M13 demo gameplay HUD.
//
// `demo::build_demo_descriptor` is the first gameplay UI producer:
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
//   - Settle frame: after BOTH tweens settle (the health count-up at 1.2s and a
//     flash segment after 150ms), a no-change frame performs no draw-list rebuild
//     and no recompute (the cached list is returned).
//   - Subscriber-awareness: an unbound slot changing invalidates nothing.
//
// Value-tweening note (M13 UI Value-Tweening, Task 4): the demo now animates on
// early frames. The health text carries a `from: 0` first-resolve flourish that
// counts UP 0→100 over 1.2s (the proxy holds health constant at 100, so this is a
// pure on-appear ramp, not an authoritative value change), and the flash swatch
// eases each proxy toggle over a 150ms segment instead of stepping. These tests
// therefore drive `build_draw_data_retained` with ADVANCING synthetic
// `time_seconds`: the appearance-only / relayout split and the post-settle
// no-recompute guarantee are asserted AFTER the relevant tween has settled, so the
// split is tested against steady state, not against the in-flight count-up.
// `SETTLED_T` is past the 1.2s health duration; tests that exercise a fresh
// settled tree start there.
//
// These overlap Task 4's tree-level diff tests, but here they run through the
// actual demo descriptor — that is the point: prove the demo screen's wiring, not
// just the retained primitive. Pure CPU — no GPU adapter, no wgpu call.
//
// See: context/lib/ui.md

use std::collections::HashMap;

use super::demo::build_demo_descriptor;
use super::descriptor::{AnchoredTree, ColorValue, TextWidget, Widget};
use super::layout::Anchor;
use super::text::{UI_FONT_FAMILY, UI_MONO_FONT_FAMILY, measure_run};
use super::theme::UiTheme;
use super::tree::{ImageSizes, UiDrawData, UiTree};
use crate::scripting::slot_table::SlotValue;

const EPS: f32 = 1e-3;

/// A synthetic time (seconds) past BOTH demo tween durations: the health count-up
/// settles at 1.2s and any flash segment settles 150ms after its toggle, so a
/// frame at this time (with a freshly-built tree, `from: 0` health flourish driven
/// in one step) lands fully settled. Used by the split / settle tests so they
/// assert against steady state, not the in-flight count-up.
const SETTLED_T: f64 = 2.0;

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

/// The integer the `HP {}` readout currently displays, parsed back out of the
/// resolved health run. The health bind tweens `from: 0`, so the driver rounds an
/// eased `f32` into the formatted run — this recovers that displayed number so the
/// count-up can be asserted frame by frame.
fn health_value(data: &UiDrawData) -> i64 {
    let hp = data
        .texts
        .iter()
        .find(|t| t.content.starts_with("HP "))
        .expect("the demo draws an `HP <n>` readout");
    hp.content
        .trim_start_matches("HP ")
        .parse::<i64>()
        .unwrap_or_else(|_| panic!("health readout is `HP <integer>`, got {:?}", hp.content))
}

/// Build the demo tree and drive its first-resolve health count-up to a fully-
/// settled steady state, returning the primed `(ui, fs)` so callers continue from
/// a settled baseline.
///
/// The health bind carries a `from: 0` first-resolve flourish: the FIRST frame a
/// freshly-built tree sees anchors the count-up's start to that frame's time and
/// renders `HP 0`, regardless of how large the frame's `time_seconds` is. So a
/// single frame at a large time does NOT settle — settling requires a first frame
/// (start anchor) followed by a frame at/after the 1.2s duration. This helper does
/// exactly that with the at-rest proxy slots, leaving health pinned at `100` and
/// the flash snapped to `solid` (the panel has no `from`). Callers read the
/// rebuild/recompute counters after this returns to establish the settled baseline.
fn settle_demo(solid: [f32; 4]) -> (UiTree, glyphon::FontSystem) {
    let tree = build_demo_descriptor();
    let mut ui = UiTree::from_descriptor(&tree, &UiTheme::engine_default());
    let mut fs = font_system();
    let slots = proxy_slots(100.0, 50.0, solid);
    // Frame 0 anchors the count-up start and renders `HP 0`.
    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, 0.0);
    // A frame past the 1.2s duration settles the count-up to `HP 100`.
    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, SETTLED_T);
    (ui, fs)
}

#[test]
fn demo_descriptor_resolves_binds_through_retained_path() {
    // Acceptance — bind resolution: with player.health=100, player.ammo=50, and
    // intro.flashColor set, the demo draw data carries the formatted text strings
    // and the swatch quad carries the resolved color. The health text now tweens
    // 0→100 on first resolve, so the FIRST frame anchors the count-up at `HP 0`;
    // this drives an anchor frame at t=0 then a frame PAST the 1.2s count-up so the
    // run settles to "HP 100". The flash panel has no `from`, so it snaps to the
    // live color on the first frame already.
    let tree = build_demo_descriptor();
    let mut ui = UiTree::from_descriptor(&tree, &UiTheme::engine_default());
    let mut fs = font_system();

    let flash = [0.0, 0.8, 0.9, 1.0];
    // Anchor frame: starts the count-up at `from: 0`.
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &proxy_slots(100.0, 50.0, flash),
        0.0,
    );
    // Settled frame: past the 1.2s duration, the count-up reads its target exactly.
    let data = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &proxy_slots(100.0, 50.0, flash),
        SETTLED_T,
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
    // recompute_count (no relayout — a bound fill is appearance-only, even when
    // the change eases over the 150ms swatch tween). The health count-up is driven
    // to its settled value first (via `settle_demo`) so it does not contribute
    // rebuilds; from there only the flash changes. The eased frames advance time
    // past the 150ms flash segment so the eased fill settles EXACTLY on pulse.
    let solid = [0.0, 0.65, 0.75, 1.0];
    let (mut ui, mut fs) = settle_demo(solid);
    let recompute_before = ui.recompute_count();
    let rebuild_before = ui.draw_rebuild_count();

    // Pulse color: only intro.flashColor changes; health/ammo hold. The first
    // change frame retargets the eased fill (anchoring at SETTLED_T); a frame 0.2s
    // later — past the 150ms segment — settles the eased fill on the pulse endpoint.
    let pulse = [0.0, 0.80, 0.90, 1.0];
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &proxy_slots(100.0, 50.0, pulse),
        SETTLED_T,
    );
    let second = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &proxy_slots(100.0, 50.0, pulse),
        SETTLED_T + 0.2,
    );
    assert_eq!(
        ui.recompute_count(),
        recompute_before,
        "appearance-only flash change must not relayout",
    );
    assert!(
        ui.draw_rebuild_count() > rebuild_before,
        "appearance-only change rebuilds the draw list",
    );
    assert!(
        swatch_color(&second).is_some_and(|c| colors_eq(c, pulse)),
        "the eased fill settles on the new flash color past the segment",
    );
}

#[test]
fn demo_bound_text_change_triggers_relayout() {
    // Acceptance — content-change relayout: changing a bound text value
    // (player.health 100→87) re-measures the run and DOES increment
    // recompute_count. The count-up is settled first (via `settle_demo`) so the
    // baseline reads "HP 100"; the drop then retargets the count-up down toward 87,
    // and a frame past the 1.2s tween duration settles it on "HP 87".
    let flash = [0.0, 0.65, 0.75, 1.0];
    let (mut ui, mut fs) = settle_demo(flash);
    let recompute_before = ui.recompute_count();
    let first = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &proxy_slots(100.0, 50.0, flash),
        SETTLED_T,
    );
    assert!(
        first.texts.iter().any(|t| t.content == "HP 100"),
        "settled baseline draws 'HP 100'",
    );

    // Health drops 100 → 87; the eased count-up retargets and the formatted run
    // width changes as it descends, forcing relayouts. A frame past the 1.2s
    // duration (the change anchored at SETTLED_T) settles the run on "HP 87".
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &proxy_slots(87.0, 50.0, flash),
        SETTLED_T,
    );
    let second = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &proxy_slots(87.0, 50.0, flash),
        SETTLED_T + 2.0,
    );
    assert!(
        ui.recompute_count() > recompute_before,
        "a bound text-content change relays out",
    );
    assert!(
        second.texts.iter().any(|t| t.content == "HP 87"),
        "the new health value is drawn",
    );
}

#[test]
fn demo_settled_frame_skips_rebuild_and_recompute() {
    // Acceptance — settle frame: after BOTH tweens settle (the health count-up at
    // 1.2s and the flash, which has no `from`, snapped on first sight), a no-change
    // frame performs NO draw-list rebuild and NO recompute — the dirty-gate short-
    // circuits and the cached list is returned. The KEY tween-era guarantee: even
    // though `time_seconds` keeps advancing, a settled tween emits no per-frame
    // value change, so an in-flight animation never defeats the steady-state gate.
    let solid = [0.0, 0.65, 0.75, 1.0];
    let (mut ui, mut fs) = settle_demo(solid);
    let settled = proxy_slots(100.0, 50.0, solid);
    // A first steady frame at a settled time: the count-up already reads 100 and
    // the flash already snapped, so this frame may still close out the count-up's
    // last content change. Capture the post-settle baseline AFTER it.
    let first =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &settled, SETTLED_T);
    let recompute_settled = ui.recompute_count();
    let rebuild_settled = ui.draw_rebuild_count();
    assert!(
        first.texts.iter().any(|t| t.content == "HP 100"),
        "settled baseline draws 'HP 100'",
    );

    // A later steady frame: nothing dirtied and both tweens are settled, so neither
    // layout nor the draw list rebuilds even as time advances — the cached list is
    // returned unchanged.
    let second = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &settled,
        SETTLED_T + 1.0,
    );
    assert_eq!(
        ui.recompute_count(),
        recompute_settled,
        "post-settle frame does not relayout",
    );
    assert_eq!(
        ui.draw_rebuild_count(),
        rebuild_settled,
        "post-settle frame returns the cached draw list (no rebuild)",
    );
    assert_eq!(
        first.quads.instances.len(),
        second.quads.instances.len(),
        "cached list matches the settled build",
    );
    assert_eq!(
        first.texts.len(),
        second.texts.len(),
        "cached text runs match the settled build",
    );
    assert!(
        second.texts.iter().any(|t| t.content == "HP 100"),
        "post-settle frame still reads 'HP 100'",
    );
}

#[test]
fn demo_health_counts_up_zero_to_hundred_over_the_tween() {
    // Acceptance — DEMO count-up flourish: the HUD HP readout counts UP 0→100 over
    // ~1.2s on first resolve, even though the proxy holds `player.health` at a
    // constant 100 (the count-up is the `from: 0` first-resolve ramp, not a value
    // ramp). Drive monotonically advancing synthetic frames and assert the rendered
    // HP integer rises 0 → … → 100, reaching EXACTLY 100 at/after the 1.2s duration.
    let tree = build_demo_descriptor();
    let mut ui = UiTree::from_descriptor(&tree, &UiTheme::engine_default());
    let mut fs = font_system();
    let slots = proxy_slots(100.0, 50.0, [0.0, 0.65, 0.75, 1.0]);

    // Frame 0 anchors the count-up at its `from` and renders "HP 0".
    let f0 = ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, 0.0);
    assert_eq!(
        health_value(&f0),
        0,
        "frame 0 renders the `from: 0` count-up start (HP 0)",
    );

    // Advance through the 1.2s count-up on monotonically rising frames: the
    // displayed HP rises monotonically and stays within [previous, 100] (eased,
    // never overshooting the target). The first sample (early in the ramp) is
    // strictly between the `from: 0` start and the target — the flourish is visibly
    // counting up, not snapped to 100 in one step.
    let mut prev = 0;
    for (i, &t) in [0.3_f64, 0.6, 0.9, 1.1].iter().enumerate() {
        let f = ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, t);
        let v = health_value(&f);
        assert!(
            v >= prev && v <= 100,
            "HP {v} at t={t}s advances monotonically within [prev={prev}, 100]",
        );
        if i == 0 {
            assert!(
                v > 0 && v < 100,
                "early in the ramp the count-up is mid-flight (0 < HP {v} < 100)",
            );
        }
        prev = v;
    }

    // At/after the 1.2s duration the readout reads EXACTLY "HP 100".
    let settled =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, SETTLED_T);
    assert_eq!(
        health_value(&settled),
        100,
        "the count-up reaches exactly HP 100 at/after the 1.2s duration",
    );
    assert!(
        settled.texts.iter().any(|t| t.content == "HP 100"),
        "the settled readout is the formatted 'HP 100'",
    );
}

#[test]
fn demo_flash_swatch_eases_between_toggle_endpoints_mid_segment() {
    // Acceptance — DEMO flash ease: the swatch fill eases each proxy toggle instead
    // of stepping. Settle first, then toggle `intro.flashColor` to a far endpoint;
    // mid-segment (well inside the 150ms tween, before it settles) the swatch fill
    // sits strictly BETWEEN the two endpoints on at least one channel, and the
    // change is appearance-only (no relayout).
    let from_color = [0.0, 0.65, 0.75, 1.0];
    let to_color = [0.0, 0.20, 0.30, 1.0];
    let (mut ui, mut fs) = settle_demo(from_color);
    let recompute_before = ui.recompute_count();

    // Retarget the eased fill toward `to_color`, anchoring the segment at SETTLED_T.
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &proxy_slots(100.0, 50.0, to_color),
        SETTLED_T,
    );
    // Sample mid-segment: 75ms into the 150ms tween (half the duration), so the
    // eased fill is strictly between the endpoints, not yet settled.
    let mid = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &proxy_slots(100.0, 50.0, to_color),
        SETTLED_T + 0.075,
    );
    let color = swatch_color(&mid).expect("the demo draws a swatch quad");

    // Not yet at either endpoint — the fill is mid-cross-fade.
    assert!(
        !colors_eq(color, from_color),
        "mid-segment fill has left the `from` endpoint, got {color:?}",
    );
    assert!(
        !colors_eq(color, to_color),
        "mid-segment fill has not yet reached the `to` endpoint, got {color:?}",
    );
    // On the green channel (0.65 → 0.20) the eased value lies strictly between the
    // endpoints — the swatch is genuinely interpolating, not stepping.
    let g = color[1];
    assert!(
        g < from_color[1] - EPS && g > to_color[1] + EPS,
        "mid-segment green {g} lies strictly between {} and {}",
        to_color[1],
        from_color[1],
    );

    // The ease is appearance-only: no relayout across the eased frames.
    assert_eq!(
        ui.recompute_count(),
        recompute_before,
        "an eased fill change is appearance-only (no relayout)",
    );

    // Past the 150ms segment the eased fill settles exactly on the target endpoint.
    let settled = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &proxy_slots(100.0, 50.0, to_color),
        SETTLED_T + 0.2,
    );
    assert!(
        swatch_color(&settled).is_some_and(|c| colors_eq(c, to_color)),
        "the eased fill settles on the toggle endpoint past the 150ms segment",
    );
}

#[test]
fn demo_unbound_slot_change_invalidates_nothing() {
    // Acceptance — subscriber-awareness: a slot with no binding in the demo tree
    // changing value must invalidate nothing — no relayout, no draw-list rebuild.
    // The demo binds player.health/player.ammo/intro.flashColor; we change an
    // unrelated `world.kills` slot.
    let tree = build_demo_descriptor();
    let mut ui = UiTree::from_descriptor(&tree, &UiTheme::engine_default());
    let mut fs = font_system();

    let mut slots = proxy_slots(100.0, 50.0, [0.0, 0.65, 0.75, 1.0]);
    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, 0.0);
    assert_eq!(ui.recompute_count(), 1);
    assert_eq!(ui.draw_rebuild_count(), 1, "first frame builds the list");

    // Add/changes only an unbound slot; every bound slot holds its value.
    slots.insert("world.kills".to_string(), SlotValue::Number(7.0));
    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, 0.0);
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
    let mut ui = UiTree::from_descriptor(&tree, &UiTheme::engine_default());
    let mut fs = font_system();
    let data = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &proxy_slots(100.0, 50.0, [0.0, 0.65, 0.75, 1.0]),
        0.0,
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

/// The sRGB-encoded `[u8; 4]` a linear-RGBA color resolves to in a built draw
/// list. Mirrors `tree::linear_rgba_to_srgb_u8` (private there) via a round-trip
/// through a built tree, so the demo's resolved token color can be compared
/// against the theme value in the same encoding the draw list carries — without
/// re-deriving the sRGB transfer here. Matches `theme_gate_test::srgb_of`.
fn srgb_of(linear: [f32; 4]) -> [u8; 4] {
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: Widget::Text(TextWidget {
            content: "X".into(),
            font_size: 20.0,
            color: ColorValue::Literal(linear),
            font: None,
            bind: None,
            style_ranges: None,
        }),
    };
    let mut ui = UiTree::from_descriptor(&tree, &UiTheme::engine_default());
    let mut fs = font_system();
    ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots())
        .texts[0]
        .color
}

/// An empty slot map for the `srgb_of` helper's literal probe — that probe binds
/// nothing, so resolution always takes the literal path.
fn no_slots() -> HashMap<String, SlotValue> {
    HashMap::new()
}

#[test]
fn demo_hud_text_resolves_the_ok_token_color() {
    // Acceptance — the demo's HUD readouts color through a theme token, not the
    // old literal: built and resolved against the engine default theme, the
    // `player.health` run carries the `ok` token's RGBA (sRGB-encoded), and that
    // color differs from the pre-token literal the demo used to carry.
    let theme = UiTheme::engine_default();
    let ok = theme
        .color("ok")
        .expect("engine default has the `ok` token");
    let tree = build_demo_descriptor();
    let mut ui = UiTree::from_descriptor(&tree, &theme);
    let mut fs = font_system();
    let slots = proxy_slots(100.0, 50.0, [0.0, 0.65, 0.75, 1.0]);
    // Anchor the health count-up, then settle past 1.2s so the run reads "HP 100".
    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, 0.0);
    let data = ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, SETTLED_T);

    let hp = data
        .texts
        .iter()
        .find(|t| t.content == "HP 100")
        .expect("the demo draws the health readout");
    assert_eq!(
        hp.color,
        srgb_of(ok),
        "the HUD readout resolves the `ok` token's theme RGBA",
    );
    // The pre-token literal was a soft cyan-white; the resolved `ok` green must
    // not coincide with it, so this proves the token path actually drives color.
    const OLD_HUD_LITERAL: [f32; 4] = [0.55, 0.85, 0.90, 1.0];
    assert_ne!(
        hp.color,
        srgb_of(OLD_HUD_LITERAL),
        "the resolved token color is not the old hardcoded literal",
    );
}

#[test]
fn demo_swatch_label_resolves_the_mono_family() {
    // Acceptance — the swatch label shapes against the second registered face:
    // the `FLASH` run resolves to the `mono` family (not the body family the
    // readouts use), and its measured width matches the mono face (not body),
    // confirming the family selection reaches the measure seam.
    let theme = UiTheme::engine_default();
    let tree = build_demo_descriptor();
    let mut ui = UiTree::from_descriptor(&tree, &theme);
    let mut fs = font_system();
    let data = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &proxy_slots(100.0, 50.0, [0.0, 0.65, 0.75, 1.0]),
        0.0,
    );

    let label = data
        .texts
        .iter()
        .find(|t| t.content == "FLASH")
        .expect("the demo draws the swatch label");
    assert_eq!(
        label.family, UI_MONO_FONT_FAMILY,
        "the swatch label resolves to the mono family",
    );
    assert_ne!(
        label.family, UI_FONT_FAMILY,
        "the swatch label is shaped with the mono face, not the body face",
    );

    // The drawn line's device font size shapes wider against the mono face than
    // the body face for this label — the family reaches the measure seam, not
    // just the draw record. Compare the two faces at the run's own device size.
    let (mono_w, _) = measure_run(&mut fs, "FLASH", label.font_size, UI_MONO_FONT_FAMILY);
    let (body_w, _) = measure_run(&mut fs, "FLASH", label.font_size, UI_FONT_FAMILY);
    assert!(
        (mono_w - body_w).abs() > EPS,
        "mono and body faces measure `FLASH` differently (mono {mono_w}, body {body_w})",
    );
}

#[test]
fn demo_descriptor_round_trips_token_color_and_mono_font_on_the_wire() {
    // Acceptance — the token forms serialize in their wire shapes: the demo
    // descriptor round-trips byte-for-byte through serde JSON, and the serialized
    // form carries the HUD color token as a bare string and the swatch font as
    // `"mono"` (a token, not a literal array / absent key).
    let tree = build_demo_descriptor();
    let json = serde_json::to_string(&tree).expect("demo descriptor serializes");
    let roundtripped: AnchoredTree =
        serde_json::from_str(&json).expect("demo descriptor deserializes");
    assert_eq!(
        roundtripped, tree,
        "demo descriptor round-trips identically"
    );

    // Color token serializes as a bare string, font token as `"mono"`.
    assert!(
        json.contains(r#""color":"ok""#),
        "HUD color serializes as the bare token string, got {json}",
    );
    assert!(
        json.contains(r#""font":"mono""#),
        "swatch label font serializes as the `mono` token, got {json}",
    );
    // No HUD text re-serialized its color as a literal array — the token path
    // never rewrote a token into a literal.
    assert!(
        !json.contains(r#""color":[0.55,0.85,0.9"#),
        "no node re-serialized the old literal HUD color, got {json}",
    );
}
