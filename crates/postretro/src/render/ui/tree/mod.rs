// Retained UI widget tree: maps the serde `descriptor` model into a
// `taffy::TaffyTree`, computes flex/grid layout, and reads the laid-out rects
// back into the device-pixel `UiDrawList` + shaped-text draw entries through the
// `layout` projection path. taffy/layout lives entirely here (renderer-owns-GPU).
// See: context/lib/ui.md §1 (retained tree), §3 (display vs. authoritative value / tween contract)

use std::collections::HashMap;

use postretro_entities::SlotValue;

/// Per-frame bound-value diff + tween drivers (content vs. appearance change).
mod bindings;
/// Descriptor → taffy node construction.
mod build;
/// Device-pixel projection, draw-data assembly, value→string/fill resolution,
/// and the exported hit-test / focus rect-list types.
mod draw;
/// Per-node draw payload (`NodeContext`) and reactive-visibility state.
mod node_context;
/// Bind/predicate resolution against the per-frame slot + cell snapshot.
mod predicate;
/// Theme-token resolution, value-tween easing, and shared container styling.
mod style;
/// The retained `UiTree` struct, the layout/draw gate, and the per-frame diff.
mod ui_tree;
/// The draw-list collection walk (`collect_node`/`collect_draw_data`), a second
/// `impl UiTree` block.
mod ui_tree_collect;
/// Focus / hit-test rect export for `UiTree` (a second `impl` block).
mod ui_tree_focus;
/// Widget-accessor helpers, the measure-seam callback, and reactive-visibility
/// harvesting the focus walk and layout build both read.
mod widget_meta;

#[cfg(test)]
mod tests;

// Re-exports so external `tree::X` references keep resolving after the split.
pub(crate) use draw::{
    FocusGroup, FocusKind, FocusRect, FocusRectList, NodeInteraction, RepeatPolicy, UiDrawData,
};
// `FocusNeighbors` is consumed only from `#[cfg(test)]` modules elsewhere in the
// crate (focus-engine tests), so the non-test build sees the re-export as unused.
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use draw::FocusNeighbors;
#[allow(unused_imports)]
pub(crate) use style::resolve_color;
pub(crate) use ui_tree::UiTree;

/// Asset key → natural reference size (logical-reference px, `[width, height]`)
/// for `image` nodes. Threaded into the measure seam so an image sizes from its
/// real asset dimensions (content-driven, like text) rather than a wire-level
/// fixed size. The renderer builds this from the uploaded texture's pixel dims.
pub(crate) type ImageSizes = HashMap<String, [f32; 2]>;

/// Resolved presentation-cell values for a frame, keyed by `(scopeId, cellName)`.
/// The app-side cell store publishes this onto the read
/// snapshot, exactly the way bound slot values flow — so the descriptor compared
/// by the retained reuse gate (`mod.rs`) stays immutable and a cell write never
/// forces a rebuild. A `{ local }` bind resolves against it through the node's
/// build-time scope. Empty (the default) whenever no `localState` scope composes.
pub(crate) type CellValues = HashMap<(String, String), SlotValue>;
