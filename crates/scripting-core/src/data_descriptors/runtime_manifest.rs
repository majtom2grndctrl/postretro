// Runtime-side manifest types that embed render::ui descriptor data.
// See: context/lib/scripting.md §12 (Crate Architecture)

use crate::ui::descriptor::AnchoredTree;

use super::{CrossingDescriptor, NamedReaction};

/// A script-registered UI tree: a named [`AnchoredTree`] plus the `alwaysOn`
/// registration attribute. Drained from `ModManifest.uiTrees` (mod scope) and
/// `setupLevel()` (level scope) returns.
#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredUiTree {
    /// Registry name the render path resolves the tree by.
    pub name: String,
    /// The placement envelope + widget tree, parsed via the G1a bridge.
    pub tree: AnchoredTree,
    /// `alwaysOn` registration attribute: a tree that stays resolvable even when
    /// it is not on top of the modal stack. Defaults to `false` when absent.
    pub always_on: bool,
}

/// The full bundle returned by a level's `setupLevel(ctx)` export.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LevelManifest {
    pub reactions: Vec<NamedReaction>,
    /// State-crossing watchers (M13 HUD dynamics). Parsed alongside `reactions`
    /// from the widened `{ reactions, crossings }` setup-manifest return and
    /// drained into the per-level `DataRegistry`; cleared on level unload.
    pub crossings: Vec<CrossingDescriptor>,
    /// Per-level UI trees declared via the `uiTrees` field. A malformed entry is
    /// logged and skipped rather than aborting level load (`ui.md` §1.1).
    pub ui_trees: Vec<RegisteredUiTree>,
}
