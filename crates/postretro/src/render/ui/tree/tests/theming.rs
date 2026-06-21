// Theme-token resolution into the draw list (color/spacing/font).

use super::common::*;
use crate::render::ui::text::{UI_FONT_FAMILY, UI_MONO_FONT_FAMILY};
/// A single text leaf carrying a color slot (token or literal) and an optional
/// font token — the resolution-under-test inputs.
fn themed_text(color: ColorValue, font: Option<&str>) -> AnchoredTree {
    AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: Widget::Text(TextWidget {
            content: "X".into(),
            font_size: 20.0,
            color,
            font: font.map(str::to_string),
            bind: None,
            style_ranges: None,
            id: None,
            focus_neighbors: Default::default(),
            visible_when: None,
            role: None,
        }),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    }
}

#[test]
fn text_color_token_resolves_to_theme_rgba_in_draw_list() {
    // A `color: "critical"` token resolves to the theme's `critical` RGBA; the
    // produced text run carries that color (sRGB-encoded). Proves token slots
    // resolve against the active theme at build time.
    let theme = UiTheme::engine_default();
    let critical = theme.color("critical").unwrap();
    let tree = themed_text(ColorValue::Token("critical".into()), None);
    let mut ui = UiTree::from_descriptor(&tree, &theme);
    let mut fs = font_system();
    let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    assert_eq!(data.texts.len(), 1);
    assert_eq!(
        data.texts[0].color,
        srgb_of(critical),
        "token color resolved to the theme's critical RGBA",
    );
}

#[test]
fn unknown_color_token_resolves_to_opaque_magenta() {
    // An unknown color token degrades to opaque magenta [1,0,1,1] — visible,
    // never invisible or a panic.
    let theme = UiTheme::engine_default();
    let tree = themed_text(ColorValue::Token("no.such.color".into()), None);
    let mut ui = UiTree::from_descriptor(&tree, &theme);
    let mut fs = font_system();
    let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    assert_eq!(
        data.texts[0].color,
        srgb_of([1.0, 0.0, 1.0, 1.0]),
        "unknown color token degrades to opaque magenta",
    );
}

#[test]
fn spacing_token_resolves_into_layout_gap() {
    // A container `gap: "l"` (theme `l` = 16px) lays its two children out with
    // exactly the theme-defined spacing — proving spacing tokens resolve into
    // the taffy style before layout.
    let theme = UiTheme::engine_default();
    let l = theme.spacing("l").unwrap();
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: Widget::VStack(ContainerWidget {
            gap: SpacingValue::Token("l".into()),
            padding: SpacingValue::Literal(0.0),
            align: Align::Start,
            fill: None,
            border: None,
            id: None,
            focus_neighbors: Default::default(),
            focus: None,
            restore_on_return: false,
            local_state: None,
            visible_when: None,
            role: None,
            children: vec![text("AB", 30.0), text("CD", 30.0)],
        }),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme);
    let mut fs = font_system();
    ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    let children: Vec<_> = ui.taffy.children(ui.root).unwrap();
    let c0 = *ui.taffy.layout(children[0]).unwrap();
    let c1 = *ui.taffy.layout(children[1]).unwrap();
    assert!(
        approx(c1.location.y - (c0.location.y + c0.size.height), l),
        "token gap resolved to the theme's `l` spacing ({l}px), got {}",
        c1.location.y - (c0.location.y + c0.size.height),
    );
}

#[test]
fn unknown_spacing_token_lays_out_as_zero() {
    // An unknown gap token degrades to 0.0 — the two children abut with no gap.
    let theme = UiTheme::engine_default();
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: Widget::VStack(ContainerWidget {
            gap: SpacingValue::Token("no.such.spacing".into()),
            padding: SpacingValue::Literal(0.0),
            align: Align::Start,
            fill: None,
            border: None,
            id: None,
            focus_neighbors: Default::default(),
            focus: None,
            restore_on_return: false,
            local_state: None,
            visible_when: None,
            role: None,
            children: vec![text("AB", 30.0), text("CD", 30.0)],
        }),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme);
    let mut fs = font_system();
    ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    let children: Vec<_> = ui.taffy.children(ui.root).unwrap();
    let c0 = *ui.taffy.layout(children[0]).unwrap();
    let c1 = *ui.taffy.layout(children[1]).unwrap();
    assert!(
        approx(c1.location.y - (c0.location.y + c0.size.height), 0.0),
        "unknown spacing token lays out as 0.0, got {}",
        c1.location.y - (c0.location.y + c0.size.height),
    );
}

#[test]
fn font_token_mono_resolves_to_the_mono_family_on_the_node() {
    // `font: "mono"` resolves to the theme's mono family on the node's
    // `NodeContext::Text` and the produced `UiText` line — so the run shapes
    // and draws against the registered monospace face.
    let theme = UiTheme::engine_default();
    let tree = themed_text(ColorValue::Literal([1.0; 4]), Some("mono"));
    let mut ui = UiTree::from_descriptor(&tree, &theme);
    // The node carries the resolved family before any draw.
    if let Some(NodeContext::Text { family, .. }) = ui.taffy.get_node_context(ui.root) {
        assert_eq!(family, UI_MONO_FONT_FAMILY, "node carries the mono family");
    } else {
        panic!("root must be a text node");
    }
    let mut fs = font_system();
    let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    assert_eq!(
        data.texts[0].family, UI_MONO_FONT_FAMILY,
        "the drawn line selects the mono family",
    );
}

#[test]
fn absent_font_resolves_to_the_primary_family() {
    // A text widget with no `font` token resolves to the `primary` family —
    // the pre-theming default, so fontless text keeps the primary face.
    let theme = UiTheme::engine_default();
    let tree = themed_text(ColorValue::Literal([1.0; 4]), None);
    let ui = UiTree::from_descriptor(&tree, &theme);
    if let Some(NodeContext::Text { family, .. }) = ui.taffy.get_node_context(ui.root) {
        assert_eq!(
            family, UI_FONT_FAMILY,
            "absent font selects the primary family"
        );
    } else {
        panic!("root must be a text node");
    }
}

#[test]
fn unknown_font_token_falls_back_to_primary_family() {
    // An unknown font token degrades to the `primary` family (not magenta,
    // not a panic) — text still renders in the default face.
    let theme = UiTheme::engine_default();
    let tree = themed_text(ColorValue::Literal([1.0; 4]), Some("no.such.font"));
    let ui = UiTree::from_descriptor(&tree, &theme);
    if let Some(NodeContext::Text { family, .. }) = ui.taffy.get_node_context(ui.root) {
        assert_eq!(
            family, UI_FONT_FAMILY,
            "unknown font token falls back to the primary family",
        );
    } else {
        panic!("root must be a text node");
    }
}

#[test]
fn override_theme_changes_resolved_token_values_on_rebuild() {
    // Rebuilding the SAME descriptor against an override theme yields the new
    // token value with NO descriptor change — the resolution seam reads the
    // theme passed at build, so a generation bump (which installs a new theme)
    // re-resolves tokens. Mirrors the engine-side setter's effect at the tree
    // level (the `UiPass` generation gate decides WHEN to rebuild; this proves
    // the rebuild produces the new values).
    let default = UiTheme::engine_default();
    let override_theme = default.with_override(&ThemeDescriptor {
        colors: StdHashMap::from([("critical".to_string(), [0.0, 1.0, 1.0, 1.0])]),
        ..Default::default()
    });
    let tree = themed_text(ColorValue::Token("critical".into()), None);

    let mut fs = font_system();
    let mut before = UiTree::from_descriptor(&tree, &default);
    let data_before = before.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    let mut after = UiTree::from_descriptor(&tree, &override_theme);
    let data_after = after.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());

    assert_eq!(
        data_before.texts[0].color,
        srgb_of(default.color("critical").unwrap()),
    );
    assert_eq!(
        data_after.texts[0].color,
        srgb_of([0.0, 1.0, 1.0, 1.0]),
        "rebuilding against the override theme re-resolves the token value",
    );
    assert_ne!(
        data_before.texts[0].color, data_after.texts[0].color,
        "the same descriptor resolves to different colors under different themes",
    );
}
