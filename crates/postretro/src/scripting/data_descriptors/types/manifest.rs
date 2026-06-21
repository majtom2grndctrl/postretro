// Data-context descriptors: UI-tree/theme/font/level manifest types.
// See: context/lib/scripting.md

use super::super::*;

/// A script-registered UI tree: a named [`AnchoredTree`] plus the `alwaysOn`
/// registration attribute. Drained from `ModManifest.uiTrees`
/// (mod scope) and `setupLevel()` (level scope) returns. Parsed and held on
/// the manifest result; drained into the app-side `UiTreeRegistry` at the
/// caller's scope tier before the authoring VM drops.
/// See: context/lib/ui.md §1.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RegisteredUiTree {
    /// Registry name the render path resolves the tree by.
    pub(crate) name: String,
    /// The placement envelope + widget tree, parsed via the G1a bridge.
    pub(crate) tree: AnchoredTree,
    /// `alwaysOn` registration attribute: a tree that stays resolvable even when
    /// it is not on top of the modal stack. Defaults to `false` when absent.
    pub(crate) always_on: bool,
}

/// Theme tokens supplied by `ModManifest.theme`. Three
/// category-scoped maps mirroring the engine theme tables (colors linear-RGBA,
/// fonts → registered family name, spacing → logical px). Drained into a
/// `ThemeDescriptor`, merged over `engine_default`, and installed via
/// `Renderer::set_ui_theme` by the boot/level-load callers in `main.rs`.
/// See: context/lib/ui.md §2.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ModThemeTokens {
    pub(crate) colors: HashMap<String, [f32; 4]>,
    pub(crate) fonts: HashMap<String, String>,
    pub(crate) spacing: HashMap<String, f32>,
}

/// Font assets declared by `ModManifest.fonts`: family name → TTF
/// asset path. Installed into the font system via `register_ui_font` by the
/// boot/level-load callers in `main.rs`. See: context/lib/ui.md §2.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ModFontAssets {
    pub(crate) families: HashMap<String, String>,
}

/// The full bundle returned by a level's `setupLevel(ctx)` export.
///
/// Entity-type descriptors are not part of this manifest — they arrive via
/// `ModManifest.entities` (mod-init only) and are drained into
/// `DataRegistry` before any level is loaded. `LevelManifest` carries
/// per-level reactions and state-crossing watchers.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LevelManifest {
    pub(crate) reactions: Vec<NamedReaction>,
    /// State-crossing watchers (M13 HUD dynamics). Parsed alongside `reactions`
    /// from the widened `{ reactions, crossings }` setup-manifest return and
    /// drained into the per-level `DataRegistry`; cleared on level unload.
    pub(crate) crossings: Vec<CrossingDescriptor>,
    /// Per-level UI trees declared via the `uiTrees` field. A malformed entry is
    /// logged and skipped rather than aborting level load (`ui.md` §1.1).
    pub(crate) ui_trees: Vec<RegisteredUiTree>,
}
