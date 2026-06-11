// Demo gameplay HUD descriptor: the hardcoded Rust description of the M13 Goal C
// state-binding demo, authored in 1280x720 logical-reference space and laid out
// through the retained gameplay `UiTree` (`UiPass::layout_gameplay_tree`). It is
// the FIRST gameplay UI producer — `main.rs` publishes this tree on the
// once-per-frame read snapshot, so the renderer drives it through the
// subscriber-aware retained path (Task 4).
//
// This is a demo, not a HUD design: three bound nodes prove the binding seam
// end-to-end. Two `text` nodes bind `player.health` / `player.ammo` (Number →
// formatted string); one `panel` binds `intro.flashColor` (length-4 linear RGBA
// array → fill). The descriptor is structurally identical every frame, so the
// retained tree reuses it and only the bound VALUES drive the diff (text-content
// change relays out; panel-fill change is appearance-only).
//
// See: context/lib/scripting.md §3 (defineStore / DefinitionOnly) ·
//      context/plans/in-progress/M13--state-system

use super::descriptor::{
    Align, AnchoredTree, ColorValue, ContainerWidget, GridWidget, PanelBind, PanelWidget,
    SpacingValue, TextBind, TextWidget, Widget,
};
use super::layout::Anchor;

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

/// Vertical gap between the HUD rows (logical-reference px).
const ROW_GAP: f32 = 10.0;

/// Gap between the swatch color block and its label inside the swatch grid
/// (logical-reference px).
const SWATCH_GAP: f32 = 8.0;

/// Outer HUD padding from the anchored corner (logical-reference px).
const HUD_PADDING: f32 = 16.0;

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
            tween: None,
        }),
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
            slot: "intro.flashColor".to_string(),
            tween: None,
        }),
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
    });
    let swatch = Widget::Grid(GridWidget {
        gap: SpacingValue::Literal(SWATCH_GAP),
        padding: SpacingValue::Literal(0.0),
        align: Align::Stretch,
        cols: 2,
        children: vec![swatch_panel, swatch_label],
    });

    // Bottom-left HUD column: health over ammo over the flash swatch, padded in
    // from the anchored corner.
    let root = Widget::VStack(ContainerWidget {
        gap: SpacingValue::Literal(ROW_GAP),
        padding: SpacingValue::Literal(HUD_PADDING),
        align: Align::Start,
        fill: None,
        border: None,
        children: vec![health, ammo, swatch],
    });

    AnchoredTree {
        anchor: Anchor::BottomLeft,
        offset: [0.0, 0.0],
        root,
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
        // health, ammo, swatch container
        assert_eq!(col.children.len(), 3, "three HUD rows");

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
    }
}
