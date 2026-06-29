// Widget-accessor helpers, the taffy measure-seam callback, and reactive-
// visibility harvesting for the retained UI tree. These read the descriptor
// model that the focus walk and the layout build both consult.
// See: context/lib/ui.md §1 (retained tree), §4 (interaction / focus)

use std::collections::HashMap;

use glyphon::FontSystem;
use taffy::prelude::{NodeId, Size, TaffyTree};

use super::super::descriptor::{BindSource, LocalState, Predicate, Widget};
use super::super::text::measure_run;
use super::ImageSizes;
use super::draw::{FocusNeighbors, NodeInteraction};
use super::node_context::{NodeContext, VisibilityState};

/// taffy measure callback: resolve a leaf's intrinsic size from its content.
/// Text nodes shape their `content` at `font_size` through `font_system` and
/// report the real shaped-run extent; image nodes report their asset's natural
/// reference size from `image_sizes` (both content-driven — size from the real
/// asset/glyphs, not a wire-level number). Every other node has no intrinsic
/// content, so it reports the size taffy already knows (`known_dimensions`,
/// defaulting each unset axis to zero — the node sizes from its style/flex slot).
pub(crate) fn measure_node(
    known_dimensions: Size<Option<f32>>,
    node_context: Option<&mut NodeContext>,
    font_system: &mut FontSystem,
    image_sizes: &ImageSizes,
) -> Size<f32> {
    match node_context {
        Some(NodeContext::Text {
            content,
            font_size,
            family,
            last_resolved,
            ..
        }) => {
            // Measure the live bound string when the retained diff has resolved
            // one (so a content change re-measures correctly); otherwise the
            // literal `content` — the fresh/splash path never resolves, so it
            // always measures the literal, unchanged from before.
            let measured = last_resolved.as_deref().unwrap_or(content);
            // Shape against the node's theme-resolved family so a node measures
            // against the same face it draws with (a monospace run sizes
            // differently from the proportional body face).
            let (width, height) = measure_run(font_system, measured, *font_size, family);
            // Honor any axis taffy has already pinned (e.g. an explicit/stretched
            // size); measure only the unconstrained axes.
            Size {
                width: known_dimensions.width.unwrap_or(width),
                height: known_dimensions.height.unwrap_or(height),
            }
        }
        Some(NodeContext::Image { asset }) => {
            // Natural reference size keyed by asset. An unregistered key collapses
            // the image to zero (it simply does not contribute size/draw) — the
            // renderer pre-registers every key it references.
            let [w, h] = image_sizes.get(asset).copied().unwrap_or([0.0, 0.0]);
            Size {
                width: known_dimensions.width.unwrap_or(w),
                height: known_dimensions.height.unwrap_or(h),
            }
        }
        _ => Size {
            width: known_dimensions.width.unwrap_or(0.0),
            height: known_dimensions.height.unwrap_or(0.0),
        },
    }
}

/// A widget's authored focus id and neighbor overrides, for the focus-rect
/// export. Every kind carries `id`/`focus_neighbors` except `spacer` (id only,
/// never focusable). Returns the authored id (borrowed) and the exported
/// neighbor overrides.
pub(crate) fn focus_meta(widget: &Widget) -> (Option<&String>, FocusNeighbors) {
    match widget {
        Widget::Text(w) => (w.id.as_ref(), (&w.focus_neighbors).into()),
        Widget::Panel(w) => (w.id.as_ref(), (&w.focus_neighbors).into()),
        Widget::Image(w) => (w.id.as_ref(), (&w.focus_neighbors).into()),
        Widget::Spacer(w) => (w.id.as_ref(), FocusNeighbors::default()),
        Widget::VStack(w) | Widget::HStack(w) => (w.id.as_ref(), (&w.focus_neighbors).into()),
        Widget::Grid(w) => (w.id.as_ref(), (&w.focus_neighbors).into()),
        // Interactive widgets carry a REQUIRED id (focusable markers): button and
        // slider always export as focusable. `bar` is passive — id only.
        Widget::Button(w) => (Some(&w.id), (&w.focus_neighbors).into()),
        Widget::Slider(w) => (Some(&w.id), (&w.focus_neighbors).into()),
        Widget::Bar(w) => (w.id.as_ref(), FocusNeighbors::default()),
        // M13 G2: a non-visual announcement carries no focus id/neighbors.
        Widget::Announce(_) => (None, FocusNeighbors::default()),
    }
}

/// The interaction metadata for an interactive widget, or
/// `None` for passive nodes. `button` carries its activation reaction; `slider`
/// its bound-value step parameters. The focus-rect export attaches this so the
/// app can drive activation/value-step from the focused node id.
pub(crate) fn widget_interaction(widget: &Widget) -> Option<NodeInteraction> {
    match widget {
        Widget::Button(w) => Some(NodeInteraction::Button {
            on_press: w.on_press.clone(),
            repeat_on_hold: w.repeat_on_hold.map(Into::into),
        }),
        Widget::Slider(w) => Some(NodeInteraction::Slider {
            slot: w.bind.source.slot().unwrap_or_default().to_string(),
            min: w.min,
            max: w.max,
            step: w.step,
            captures_nav: w.captures_nav.clone(),
        }),
        _ => None,
    }
}

/// The `localState` scope a container opens for the focus walk's `{ local }`
/// predicate resolution, or `None`. Only `vstack`/`hstack` (`ContainerWidget`)
/// declare a scope; `grid` carries none. Mirrors `build_stack`'s `child_scope` so
/// the focus walk resolves a `selected`/`checked` `{ local }` predicate against the
/// same scope the draw build's bind resolution uses.
pub(crate) fn container_local_scope(widget: &Widget) -> Option<&str> {
    match widget {
        Widget::VStack(w) | Widget::HStack(w) => local_state_scope(w.local_state.as_ref()),
        _ => None,
    }
}

/// Resolve a widget's a11y state for the focus-rect readback (M13 G2):
/// `(selected, checked, disabled)`. `selected`/`checked` are a button's optional
/// `Predicate`s resolved against the frame snapshot to `1.0`/`0.0` (`None` when the
/// widget declares no predicate or is not a button). `disabled` is the button's or
/// slider's `disabled` bit (`false` for every other kind). The engine draws no
/// highlight from selected/checked — they are a11y metadata only.
pub(crate) fn widget_a11y_state(
    widget: &Widget,
    scope: Option<&str>,
    slot_values: &HashMap<String, postretro_entities::SlotValue>,
    cell_values: &super::CellValues,
) -> (Option<f32>, Option<f32>, bool) {
    let resolve = |p: &Predicate| {
        let predicate_scope = match &p.source {
            BindSource::Local { .. } => scope,
            BindSource::Slot { .. } => None,
        };
        super::predicate::resolve_predicate(
            &p.source,
            p.equals.as_ref(),
            predicate_scope,
            slot_values,
            cell_values,
        )
    };
    match widget {
        Widget::Button(w) => (
            w.selected.as_ref().map(&resolve),
            w.checked.as_ref().map(&resolve),
            w.disabled,
        ),
        Widget::Slider(w) => (None, None, w.disabled),
        _ => (None, None, false),
    }
}

/// The focus policy a container declares, or `None` for leaves and policy-less
/// containers. A declaring container opens a focus group its direct children join.
pub(crate) fn container_focus_policy(
    widget: &Widget,
) -> Option<&super::super::descriptor::FocusPolicy> {
    match widget {
        Widget::VStack(w) | Widget::HStack(w) => w.focus.as_ref(),
        Widget::Grid(w) => w.focus.as_ref(),
        _ => None,
    }
}

/// Whether `widget` or any descendant container declares `restoreOnReturn`.
/// Surfaced tree-wide on the focus rect list: the focus engine restores this
/// tree's saved focus on a returning pop when any of its containers opted in.
pub(crate) fn any_restore_on_return(widget: &Widget) -> bool {
    let declared = match widget {
        Widget::VStack(w) | Widget::HStack(w) => w.restore_on_return,
        Widget::Grid(w) => w.restore_on_return,
        _ => false,
    };
    declared
        || widget_children(widget)
            .is_some_and(|children| children.iter().any(any_restore_on_return))
}

/// A container's `children` for the lockstep focus walk, or `None` for leaves.
pub(crate) fn widget_children(widget: &Widget) -> Option<&[Widget]> {
    match widget {
        Widget::VStack(w) | Widget::HStack(w) => Some(&w.children),
        Widget::Grid(w) => Some(&w.children),
        _ => None,
    }
}

/// A widget's optional `visibleWhen` reactive-visibility predicate (M13 G2, Task
/// 2b). Lives on every widget variant; `None` means the node is always visible.
/// Harvested in lockstep with the taffy tree (`harvest_visibility`) so the diff
/// can toggle the matching node's taffy `Display`.
fn widget_visible_when(widget: &Widget) -> Option<&Predicate> {
    match widget {
        Widget::Text(w) => w.visible_when.as_ref(),
        Widget::Panel(w) => w.visible_when.as_ref(),
        Widget::Image(w) => w.visible_when.as_ref(),
        Widget::Spacer(w) => w.visible_when.as_ref(),
        Widget::VStack(w) | Widget::HStack(w) => w.visible_when.as_ref(),
        Widget::Grid(w) => w.visible_when.as_ref(),
        Widget::Button(w) => w.visible_when.as_ref(),
        Widget::Slider(w) => w.visible_when.as_ref(),
        Widget::Bar(w) => w.visible_when.as_ref(),
        Widget::Announce(w) => w.visible_when.as_ref(),
    }
}

/// Harvest reactive-visibility state in lockstep with the just-built taffy tree
/// (M13 G2, Task 2b). Walks the descriptor and the taffy graph together — they are
/// structurally 1:1 (`build_node` maps each widget to exactly one node, children in
/// order) — and records a `VisibilityState` for every node carrying a `visibleWhen`
/// predicate. Each predicate carries its nearest enclosing `localState` scope so a
/// `{ local }` predicate resolves against the same scope the draw/focus walks use;
/// a `{ slot }` predicate carries `None`. Nodes with no `visibleWhen` never enter
/// the map and stay visible.
pub(crate) fn harvest_visibility(
    taffy: &TaffyTree<NodeContext>,
    widget: &Widget,
    node: NodeId,
    scope: Option<&str>,
    out: &mut HashMap<NodeId, VisibilityState>,
) {
    if let Some(predicate) = widget_visible_when(widget) {
        // A `{ local }` predicate resolves against the nearest enclosing scope; a
        // `{ slot }` predicate reads the store and needs no scope.
        // Own-scope caveat: a container's own `visibleWhen` referencing its OWN
        // `localState` cell resolves against the PARENT scope here (the child scope
        // is opened after this block). It silently resolves 0.0 (cell not found).
        // The intended pattern places the cell on a parent container, or uses
        // Switch which injects `visibleWhen` on children — neither hits this case.
        let pred_scope = match &predicate.source {
            BindSource::Local { .. } => scope.map(str::to_string),
            BindSource::Slot { .. } => None,
        };
        // Capture the authored `Display` (`Grid` for a grid, `Flex`/default
        // otherwise) so a hide→show flip restores the right one.
        let visible_display = taffy.style(node).expect("node has a style").display;
        out.insert(
            node,
            VisibilityState {
                predicate: predicate.clone(),
                scope: pred_scope,
                visible_display,
                prev: None,
            },
        );
    }

    // A container declaring its own `localState` opens a scope for its subtree
    // (mirrors `build_stack`); children inherit otherwise.
    let child_scope = container_local_scope(widget).or(scope);

    if let Some(children) = widget_children(widget) {
        let taffy_children = taffy.children(node).expect("node children resolve");
        for (child_widget, child_node) in children.iter().zip(taffy_children) {
            harvest_visibility(taffy, child_widget, child_node, child_scope, out);
        }
    }
}

/// The scope id a container's `localState` declaration opens, if any. `None`
/// when the container declares no `localState`, so its subtree
/// inherits the enclosing scope.
pub(super) fn local_state_scope(local_state: Option<&LocalState>) -> Option<&str> {
    local_state.map(|ls| ls.scope.as_str())
}
