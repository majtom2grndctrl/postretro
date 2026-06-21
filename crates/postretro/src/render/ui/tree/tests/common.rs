// Shared fixtures, builders, and re-exports for the colocated `tree` tests.
// Each topic submodule does `use super::common::*;` to pull these in. The
// re-exports stand in for the single flat `use super::*;` the tests used before
// the module split. Test infrastructure only — see context/lib/testing_guide.md §4.

#![allow(unused_imports)]

// Tree internals the tests reach into (private to `tree`, visible to this
// descendant module). Re-exported so each topic file resolves them by name.
pub(crate) use super::super::bindings::colors_eq;
pub(crate) use super::super::draw::linear_rgba_to_srgb_u8;
pub(crate) use super::super::predicate::resolve_predicate;
pub(crate) use super::super::ui_tree::NodeContext;
pub(crate) use super::super::{
    CellValues, FocusKind, FocusNeighbors, FocusRect, FocusRectList, ImageSizes, NodeInteraction,
    UiDrawData, UiTree,
};

// Descriptor model + sibling-module types the tests construct.
pub(crate) use super::super::build::INTERACTIVE_LABEL_COLOR;
pub(crate) use crate::render::ui::descriptor::{
    Align, AnchoredTree, BarMax, BarMaxStateRef, BarWidget, BindSource, ButtonWidget, CaptureMode,
    ColorValue, ContainerWidget, Easing, GridWidget, LocalState, PanelBind, PanelTween,
    PanelWidget, Predicate, PredicateValue, SliderBind, SliderWidget, SpacerWidget, SpacingValue,
    TextBind, TextTween, TextWidget, Widget,
};
pub(crate) use crate::render::ui::layout::{Anchor, REFERENCE_HEIGHT, REFERENCE_WIDTH};
pub(crate) use crate::render::ui::style_ranges::{StyleEntry, StyleRanges};
pub(crate) use crate::render::ui::theme::{ThemeDescriptor, UiTheme};
pub(crate) use crate::scripting::slot_table::SlotValue;
pub(crate) use taffy::prelude::NodeId;

pub(crate) use std::collections::HashMap;
pub(crate) use std::collections::HashMap as StdHashMap;
pub(crate) use taffy::geometry::Size as TaffySize;

/// Device-pixel comparison tolerance; rects snap to whole pixels but float
/// rounding leaves sub-ulp residue.
pub(crate) const EPS: f32 = 1e-3;

pub(crate) fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() <= EPS
}

/// The engine default theme — every required token resolves, so a literal
/// descriptor's tokens resolve to themselves and these layout tests behave
/// exactly as before theming threaded through `from_descriptor`.
pub(crate) fn theme() -> UiTheme {
    UiTheme::engine_default()
}

/// A headless `FontSystem` (embedded Inter face registered, no GPU). Text
/// nodes measure through this in `build_draw_data`, so every layout test
/// supplies one — cosmic-text shaping runs fully on the CPU.
pub(crate) fn font_system() -> glyphon::FontSystem {
    crate::render::ui::text::build_font_system()
}

/// An empty `ImageSizes` map — most layout tests carry no `image` nodes, so
/// the measure seam never looks anything up.
pub(crate) fn no_images() -> ImageSizes {
    ImageSizes::new()
}

/// An empty slot-value map — most layout tests have no bound widgets, so
/// resolution always takes the literal-fallback path.
pub(crate) fn no_slots() -> HashMap<String, SlotValue> {
    HashMap::new()
}

pub(crate) fn no_cells() -> CellValues {
    CellValues::new()
}

pub(crate) fn spacer(flex_grow: f32) -> Widget {
    Widget::Spacer(SpacerWidget {
        flex_grow,
        id: None,
        visible_when: None,
        role: None,
    })
}

pub(crate) fn vstack(gap: f32, padding: f32, align: Align, children: Vec<Widget>) -> Widget {
    Widget::VStack(ContainerWidget {
        gap: SpacingValue::Literal(gap),
        padding: SpacingValue::Literal(padding),
        align,
        fill: None,
        border: None,
        id: None,
        focus_neighbors: Default::default(),
        focus: None,
        restore_on_return: false,
        local_state: None,
        visible_when: None,
        role: None,
        children,
    })
}

pub(crate) fn hstack(gap: f32, padding: f32, align: Align, children: Vec<Widget>) -> Widget {
    Widget::HStack(ContainerWidget {
        gap: SpacingValue::Literal(gap),
        padding: SpacingValue::Literal(padding),
        align,
        fill: None,
        border: None,
        id: None,
        focus_neighbors: Default::default(),
        focus: None,
        restore_on_return: false,
        local_state: None,
        visible_when: None,
        role: None,
        children,
    })
}

/// One numeric slot map (the dominant single-slot fixture for tween, styleRanges,
/// focus, and bar tests).
pub(crate) fn number_slots(slot: &str, value: f32) -> HashMap<String, SlotValue> {
    let mut slots = HashMap::new();
    slots.insert(slot.to_string(), SlotValue::Number(value));
    slots
}

/// Parse a rendered text run's content back to an `f32` (the displayed value
/// the tween driver rounded to an integer).
pub(crate) fn text_value(data: &UiDrawData) -> f32 {
    data.texts[0]
        .content
        .parse::<f32>()
        .expect("displayed text is an integer string")
}

/// A tweened bound text leaf.
pub(crate) fn tweened_text(
    content: &str,
    slot: &str,
    format: Option<&str>,
    tween: TextTween,
) -> Widget {
    Widget::Text(TextWidget {
        content: content.into(),
        font_size: 20.0,
        color: ColorValue::Literal([1.0, 1.0, 1.0, 1.0]),
        font: None,
        bind: Some(TextBind {
            source: BindSource::Slot { slot: slot.into() },
            format: format.map(str::to_string),
            tween: Some(tween),
        }),
        style_ranges: None,
        id: None,
        focus_neighbors: Default::default(),
        visible_when: None,
        role: None,
    })
}

/// A tweened bound panel leaf wrapped in a stretch container (so the leaf gets
/// a non-zero laid-out rect).
pub(crate) fn tweened_panel_in_stack(fill: [f32; 4], slot: &str, tween: PanelTween) -> Widget {
    Widget::VStack(ContainerWidget {
        gap: SpacingValue::Literal(0.0),
        padding: SpacingValue::Literal(0.0),
        align: Align::Stretch,
        fill: Some(ColorValue::Literal([0.0, 0.0, 0.0, 1.0])),
        border: None,
        id: None,
        focus_neighbors: Default::default(),
        focus: None,
        restore_on_return: false,
        local_state: None,
        visible_when: None,
        role: None,
        children: vec![Widget::Panel(PanelWidget {
            fill: ColorValue::Literal(fill),
            border: None,
            bind: Some(PanelBind {
                source: BindSource::Slot { slot: slot.into() },
                tween: Some(tween),
            }),
            style_ranges: None,
            id: None,
            focus_neighbors: Default::default(),
            visible_when: None,
            role: None,
        })],
    })
}

/// A bound text leaf: literal `content` fallback plus a `{ slot }` bind, no tween.
pub(crate) fn bound_text(content: &str, slot: &str, format: Option<&str>) -> Widget {
    Widget::Text(TextWidget {
        content: content.into(),
        font_size: 20.0,
        color: ColorValue::Literal([1.0, 1.0, 1.0, 1.0]),
        font: None,
        bind: Some(TextBind {
            source: BindSource::Slot { slot: slot.into() },
            format: format.map(str::to_string),
            tween: None,
        }),
        style_ranges: None,
        id: None,
        focus_neighbors: Default::default(),
        visible_when: None,
        role: None,
    })
}

/// One `intro.flashColor` RGBA slot map for the bound-panel/flash tests.
pub(crate) fn flash_slots(rgba: [f32; 4]) -> HashMap<String, SlotValue> {
    let mut slots = HashMap::new();
    slots.insert(
        "intro.flashColor".to_string(),
        SlotValue::Array(rgba.to_vec()),
    );
    slots
}

/// Find the bound panel quad's color (the inner leaf, which differs from the
/// container backdrop's literal black). Returns the first quad whose color is
/// not the backdrop black.
pub(crate) fn flash_quad_color(data: &UiDrawData) -> Option<[f32; 4]> {
    data.quads
        .instances
        .iter()
        .map(|q| q.color)
        .find(|c| !colors_eq(*c, [0.0, 0.0, 0.0, 1.0]))
}

/// sRGB encode a linear color via the tree's draw-path converter.
pub(crate) fn srgb_of(linear: [f32; 4]) -> [u8; 4] {
    linear_rgba_to_srgb_u8(linear)
}

/// A `{ slot }`-sourced predicate with an optional `equals` comparand.
pub(crate) fn pred(slot: &str, equals: Option<PredicateValue>) -> Predicate {
    Predicate {
        source: BindSource::Slot { slot: slot.into() },
        equals,
    }
}

/// A top-left, passthrough `AnchoredTree` envelope around `root`.
pub(crate) fn anchored(root: Widget) -> AnchoredTree {
    AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root,
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    }
}

/// A text leaf carrying an authored focus `id` (no bind).
pub(crate) fn text_id(content: &str, id: &str) -> Widget {
    Widget::Text(TextWidget {
        content: content.into(),
        font_size: 20.0,
        color: ColorValue::Literal([1.0; 4]),
        font: None,
        id: Some(id.to_string()),
        focus_neighbors: crate::render::ui::descriptor::FocusNeighbors::default(),
        bind: None,
        style_ranges: None,
        visible_when: None,
        role: None,
    })
}

/// A text leaf, for flex/grid distribution tests. Sized by the measure seam:
/// `content` is shaped at `font_size` through glyphon, so the leaf's intrinsic
/// size comes from real glyph metrics.
pub(crate) fn text(content: &str, font_size: f32) -> Widget {
    Widget::Text(TextWidget {
        content: content.into(),
        font_size,
        color: ColorValue::Literal([1.0, 1.0, 1.0, 1.0]),
        font: None,
        bind: None,
        style_ranges: None,
        id: None,
        focus_neighbors: Default::default(),
        visible_when: None,
        role: None,
    })
}
