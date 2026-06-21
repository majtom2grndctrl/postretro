// Descriptor → taffy node construction for the retained UI tree. Resolves every
// theme token into the concrete value the node carries, so the per-frame walk is
// theme-free.
// See: context/lib/ui.md §1 (retained tree)

use std::cell::RefCell;

use taffy::prelude::{
    Display, FlexDirection, NodeId, Size, Style, TaffyTree, evenly_sized_tracks, length,
};

use super::super::descriptor::{
    BindSource, Border, ButtonWidget, ContainerWidget, GridWidget, ImageWidget, PanelWidget,
    SliderWidget, TextBind, TextWidget, Widget,
};
use super::super::style_ranges::StyleEffectState;
use super::super::theme::UiTheme;

use super::style::{
    build_node_style_ranges, container_base_style, resolve_border, resolve_color, resolve_font,
    resolve_spacing,
};
use super::ui_tree::NodeContext;
use super::widget_meta::local_state_scope;

/// Default text size (logical-reference px) for an interactive `button`/`slider`
/// label run. The widgets carry no per-instance `font_size` in v1 (a later
/// additive field could expose it); their labels measure/draw at this size.
const INTERACTIVE_LABEL_FONT_SIZE: f32 = 18.0;

/// Default text color for an interactive `button`/`slider` label run. These
/// widgets do not expose a color field in the descriptor contract, so the
/// renderer keeps their base label color as a literal instead of naming a theme
/// token that mods are not required to define.
pub(crate) const INTERACTIVE_LABEL_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Default bar size (logical-reference px, `[width, height]`). A `bar` has no
/// intrinsic content to measure, so its leaf carries an explicit style size; a
/// container's `align`/stretch may still override it. Horizontal-only in v1.
const DEFAULT_BAR_SIZE: [f32; 2] = [120.0, 12.0];
/// Recursively build a taffy node (and its children) for one descriptor widget.
/// Resolves every theme token (color/spacing/font) against `theme` into the
/// concrete value the node carries, so the per-frame walk is theme-free.
///
/// `scope` is the nearest enclosing `localState` scope id,
/// threaded down so a `{ local }` bind on this node (or a descendant) resolves
/// against the right scope at draw time. A container declaring its own
/// `localState` overrides `scope` for its subtree (see `build_stack`/`build_grid`).
pub(crate) fn build_node(
    taffy: &mut TaffyTree<NodeContext>,
    widget: &Widget,
    theme: &UiTheme,
    scope: Option<&str>,
) -> NodeId {
    /// The scope id stored on a node's bind context: the nearest enclosing scope
    /// when the bind reads a presentation cell, else `None` (a `{ slot }` bind or
    /// no bind reads no cell, so it needs no scope). Keeping it `None` for slot
    /// binds means the draw walk's `lookup_bound` never consults the cell map for
    /// them — slot resolution is unchanged.
    fn bind_scope_for(source: Option<&BindSource>, scope: Option<&str>) -> Option<String> {
        match source {
            Some(BindSource::Local { local }) => {
                if scope.is_none() {
                    log::warn!(
                        "[UI] local bind \"{local}\" has no enclosing localState scope; \
                         falling back to literal"
                    );
                }
                scope.map(str::to_string)
            }
            _ => None,
        }
    }
    match widget {
        Widget::Text(TextWidget {
            content,
            font_size,
            color,
            font,
            bind,
            style_ranges,
            // `id`/`focus_neighbors` are read by the focus-rect export, not the
            // draw build — the draw walk ignores them.
            ..
        }) => {
            let style_ranges =
                build_node_style_ranges(style_ranges.as_ref(), bind.is_some(), theme, "text");
            // Text nodes are sized by the measure closure in `build_draw_data`,
            // which shapes `content` at `font_size` through glyphon and returns
            // the real shaped-run extent. The node carries no explicit style size.
            // `bind` rides along for draw-time resolution (layout uses `content`).
            taffy
                .new_leaf_with_context(
                    Style::default(),
                    NodeContext::Text {
                        content: content.clone(),
                        font_size: *font_size,
                        // Resolve the color token (or literal) against the theme;
                        // an unknown token degrades to opaque magenta + one warn.
                        color: resolve_color(color, theme),
                        // Resolve the optional font token to a concrete family:
                        // `None` → the `primary` token, `Some(name)` → that token,
                        // unknown → `primary` + one warn.
                        family: resolve_font(font, theme),
                        bind_scope: bind_scope_for(bind.as_ref().map(|b| &b.source), scope),
                        bind: bind.clone(),
                        last_resolved: None,
                        // Tween state is born on the first numeric resolution, not
                        // at build: the fresh path never tweens, and the retained
                        // diff initializes it when the slot first reads a `Number`.
                        tween: None,
                        style_ranges,
                        style_state: RefCell::new(StyleEffectState::default()),
                        // A `text` widget's styleRanges read its bound numeric slot,
                        // not a predicate (the predicate path is the button's).
                        predicate_bind: None,
                        predicate_scope: None,
                    },
                )
                .expect("taffy leaf creation must succeed")
        }
        Widget::Panel(PanelWidget {
            fill,
            border,
            bind,
            style_ranges,
            // `id`/`focus_neighbors` are read by the focus-rect export, not here.
            ..
        }) => {
            let style_ranges =
                build_node_style_ranges(style_ranges.as_ref(), bind.is_some(), theme, "panel");
            // A panel leaf sizes to fill its flex/grid slot (it has no intrinsic
            // size). Container backdrops are expressed on the container instead.
            // `bind` rides along for draw-time fill resolution.
            taffy
                .new_leaf_with_context(
                    Style::default(),
                    NodeContext::Panel {
                        // Resolve the fill token (or literal) against the theme.
                        fill: resolve_color(fill, theme),
                        border: resolve_border(border.as_ref(), theme),
                        bind_scope: bind_scope_for(bind.as_ref().map(|b| &b.source), scope),
                        bind: bind.clone(),
                        last_resolved: None,
                        // Born on the first length-4 array resolution (see above).
                        tween: None,
                        style_ranges,
                        style_state: RefCell::new(StyleEffectState::default()),
                    },
                )
                .expect("taffy leaf creation must succeed")
        }
        Widget::Image(ImageWidget { asset, .. }) => taffy
            .new_leaf_with_context(
                Style::default(),
                NodeContext::Image {
                    asset: asset.clone(),
                },
            )
            .expect("taffy leaf creation must succeed"),
        Widget::VStack(container) => {
            build_stack(taffy, container, FlexDirection::Column, theme, scope)
        }
        Widget::HStack(container) => {
            build_stack(taffy, container, FlexDirection::Row, theme, scope)
        }
        Widget::Grid(grid) => build_grid(taffy, grid, theme, scope),
        Widget::Spacer(spacer) => {
            // Flexible space: claims a proportional share of leftover space in its
            // parent container via flex_grow. No draw payload.
            let style = Style {
                flex_grow: spacer.flex_grow,
                ..Default::default()
            };
            taffy
                .new_leaf(style)
                .expect("taffy leaf creation must succeed")
        }
        Widget::Button(button) => build_button(taffy, button, theme, scope),
        Widget::Slider(slider) => build_slider(taffy, slider, theme, scope),
        Widget::Bar(bar) => build_bar(taffy, bar, theme, scope),
        // M13 G2: a non-visual announcement lays out as an empty zero-size leaf
        // (no quad, no glyph). Routing its text to the a11y layer is a later task.
        Widget::Announce(_) => taffy
            .new_leaf(Style::default())
            .expect("taffy leaf creation must succeed"),
    }
}

/// Build an interactive `button` leaf. Renders its `label`
/// as a centered text run shaping against the theme `primary` face. The button is a
/// pure text leaf for layout/draw; its focusable marker + activation (`on_press`)
/// ride the focus-rect export (`focus_meta` / `widget_interaction`), not the draw
/// payload. The base label color is the renderer-owned literal white default;
/// styleRanges, when present, may still replace it through the existing text
/// color path.
///
/// M13 G2: a button's `bind` is a [`Predicate`] (not a slot bind) accepted as the
/// `styleRanges` value source — a tab/segmented button self-highlights when its
/// predicate is true. The predicate + styleRanges thread onto the internal Text
/// `NodeContext` so the existing styleRanges color path drives the label color (the
/// author-wired highlight; no new visual primitive). The styleRanges bind
/// precondition is satisfied by the predicate, so its band-color tokens pre-resolve
/// the same way a bound text/panel's do.
fn build_button(
    taffy: &mut TaffyTree<NodeContext>,
    button: &ButtonWidget,
    theme: &UiTheme,
    scope: Option<&str>,
) -> NodeId {
    let style_ranges = build_node_style_ranges(
        button.style_ranges.as_ref(),
        button.bind.is_some(),
        theme,
        "button",
    );
    let predicate_scope = match button.bind.as_ref().map(|p| &p.source) {
        Some(BindSource::Local { .. }) => scope.map(str::to_string),
        _ => None,
    };
    taffy
        .new_leaf_with_context(
            Style::default(),
            NodeContext::Text {
                // M13 G2 migration: `label` is now `Option` (label-XOR-labelledBy).
                // The label-text rendering and `labelledBy` resolution are a later
                // task; for now an absent inline label renders empty.
                content: button.label.clone().unwrap_or_default(),
                font_size: INTERACTIVE_LABEL_FONT_SIZE,
                color: INTERACTIVE_LABEL_COLOR,
                family: resolve_font(&None, theme),
                // A button label is static text — no slot bind, tween, or format.
                bind_scope: None,
                bind: None,
                last_resolved: None,
                tween: None,
                style_ranges,
                style_state: RefCell::new(StyleEffectState::default()),
                // The button's reactive highlight: its `bind` Predicate drives the
                // styleRanges value (resolved to 0.0/1.0 at draw build).
                predicate_bind: button.bind.clone(),
                predicate_scope,
            },
        )
        .expect("taffy leaf creation must succeed")
}

/// Build an interactive `slider` leaf. Renders `label` plus
/// the current numeric value as one text run: it binds the slot through a
/// synthesized `"<label>: {}"` format so the value display reuses the existing
/// bound-text resolution + tween machinery (the slider's bind tween eases the
/// shown number). The focusable marker + nav-capture/value-step ride the
/// focus-rect export, not the draw payload.
fn build_slider(
    taffy: &mut TaffyTree<NodeContext>,
    slider: &SliderWidget,
    theme: &UiTheme,
    scope: Option<&str>,
) -> NodeId {
    // Synthesize a text bind so the value display rides the bound-text path:
    // `content` is the fallback (label with no value yet), `format` injects the
    // resolved number after the label. The slider's bind tween carries through.
    let format = format!("{}: {{}}", slider.label.as_deref().unwrap_or_default());
    let bind = TextBind {
        source: slider.bind.source.clone(),
        format: Some(format),
        tween: slider.bind.tween.clone(),
    };
    let bind_scope = match &bind.source {
        BindSource::Local { .. } => scope.map(str::to_string),
        BindSource::Slot { .. } => None,
    };
    taffy
        .new_leaf_with_context(
            Style::default(),
            NodeContext::Text {
                content: slider.label.clone().unwrap_or_default(),
                font_size: INTERACTIVE_LABEL_FONT_SIZE,
                color: INTERACTIVE_LABEL_COLOR,
                family: resolve_font(&None, theme),
                bind_scope,
                bind: Some(bind),
                last_resolved: None,
                tween: None,
                style_ranges: None,
                style_state: RefCell::new(StyleEffectState::default()),
                // A slider's value display binds a numeric slot, not a predicate.
                predicate_bind: None,
                predicate_scope: None,
            },
        )
        .expect("taffy leaf creation must succeed")
}

/// Build a passive horizontal `bar` leaf. Carries an explicit
/// style size (a bar has no content to measure) and a `NodeContext::Bar` draw
/// payload. Its `fill`/`background` color tokens resolve against the theme at
/// build time; `style_ranges`' band colors pre-resolve too (theme-free draw walk),
/// gated on the bind precondition like text/panel styleRanges.
fn build_bar(
    taffy: &mut TaffyTree<NodeContext>,
    bar: &super::super::descriptor::BarWidget,
    theme: &UiTheme,
    scope: Option<&str>,
) -> NodeId {
    let bind_scope = match &bar.bind.source {
        BindSource::Local { .. } => scope.map(str::to_string),
        BindSource::Slot { .. } => None,
    };
    let style = Style {
        size: Size {
            width: length(DEFAULT_BAR_SIZE[0]),
            height: length(DEFAULT_BAR_SIZE[1]),
        },
        ..Default::default()
    };
    // A bar always binds (a value to display), so the styleRanges bind precondition
    // is satisfied; pre-resolve its band-color tokens to literals for the draw walk.
    let style_ranges = build_node_style_ranges(bar.style_ranges.as_ref(), true, theme, "bar");
    taffy
        .new_leaf_with_context(
            style,
            NodeContext::Bar {
                bind_scope,
                bind: bar.bind.clone(),
                max: bar.max.clone(),
                fill: resolve_color(&bar.fill, theme),
                background: resolve_color(&bar.background, theme),
                last_resolved: None,
                last_max_resolved: None,
                tween: None,
                style_ranges,
                style_state: RefCell::new(StyleEffectState::default()),
            },
        )
        .expect("taffy leaf creation must succeed")
}

/// Optional backdrop `NodeContext` for a container declaring a `fill`/`border`.
/// `None` when the container draws no backdrop. The backdrop quad is sized to the
/// container's full laid-out rect and drawn beneath its children (painter's order
/// in `collect_node`), so a `fill`-bearing container reads as a backing panel
/// wrapping its content.
fn container_backdrop(fill: Option<[f32; 4]>, border: Option<&Border>) -> Option<NodeContext> {
    match (fill, border) {
        (None, None) => None,
        // A border with no fill still needs a fill color for the quad path; use
        // a transparent fill so only the 9-slice rim shows. (The splash always
        // pairs a fill with its border, so this is the defensive branch.)
        (fill, border) => Some(NodeContext::Panel {
            fill: fill.unwrap_or([0.0; 4]),
            border: border.cloned(),
            // Container backdrops never bind — only `panel` leaves carry a bind.
            bind_scope: None,
            bind: None,
            last_resolved: None,
            tween: None,
            // Backdrops carry no styleRanges (styleRanges live on bound leaves).
            style_ranges: None,
            style_state: RefCell::new(StyleEffectState::default()),
        }),
    }
}

/// Build a flex stack node (`vstack` → column, `hstack` → row). A container with
/// a `fill`/`border` carries a `NodeContext::Panel` backdrop drawn beneath its
/// children; otherwise it is a pure layout node with no draw payload.
fn build_stack(
    taffy: &mut TaffyTree<NodeContext>,
    container: &ContainerWidget,
    direction: FlexDirection,
    theme: &UiTheme,
    scope: Option<&str>,
) -> NodeId {
    // A container declaring its own `localState` opens a scope its subtree's
    // `{ local }` binds resolve against; otherwise children inherit the enclosing
    // scope. Nesting overrides by tree depth (nearest declaring ancestor wins).
    let child_scope = local_state_scope(container.local_state.as_ref()).or(scope);
    let children: Vec<NodeId> = container
        .children
        .iter()
        .map(|child| build_node(taffy, child, theme, child_scope))
        .collect();
    // Resolve the spacing tokens to scalar `f32` BEFORE `container_base_style` —
    // its resolved-scalar signature stays unchanged; resolution is the only seam
    // that moved (an unknown token degrades to 0.0 + one warn via `resolve_spacing`).
    let style = Style {
        display: Display::Flex,
        flex_direction: direction,
        ..container_base_style(
            resolve_spacing(&container.gap, theme),
            resolve_spacing(&container.padding, theme),
            container.align,
        )
    };
    let node = taffy
        .new_with_children(style, &children)
        .expect("taffy container creation must succeed");
    // Resolve the optional backdrop fill (token or literal) and border tint
    // against the theme into concrete values carried on the backdrop context.
    let fill = container.fill.as_ref().map(|c| resolve_color(c, theme));
    let border = resolve_border(container.border.as_ref(), theme);
    if let Some(ctx) = container_backdrop(fill, border.as_ref()) {
        taffy
            .set_node_context(node, Some(ctx))
            .expect("setting a fresh container's backdrop context must succeed");
    }
    node
}

/// Build a CSS-grid node: `cols` equal flexible tracks, `gap` both axes.
fn build_grid(
    taffy: &mut TaffyTree<NodeContext>,
    grid: &GridWidget,
    theme: &UiTheme,
    scope: Option<&str>,
) -> NodeId {
    // A grid carries no `localState` of its own (only stack containers declare
    // scopes — the `local_state` field lives on `ContainerWidget`), so its
    // children simply inherit the enclosing scope.
    let children: Vec<NodeId> = grid
        .children
        .iter()
        .map(|child| build_node(taffy, child, theme, scope))
        .collect();
    // `evenly_sized_tracks(N)` yields N equal `1fr` tracks — the descriptor's
    // "N equal columns" maps straight onto it.
    let cols = grid.cols.try_into().unwrap_or(u16::MAX);
    let style = Style {
        display: Display::Grid,
        grid_template_columns: evenly_sized_tracks(cols),
        // Resolve the spacing tokens to scalar `f32` against the theme.
        ..container_base_style(
            resolve_spacing(&grid.gap, theme),
            resolve_spacing(&grid.padding, theme),
            grid.align,
        )
    };
    taffy
        .new_with_children(style, &children)
        .expect("taffy grid creation must succeed")
}
