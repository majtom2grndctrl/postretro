// The mod author's optional `ModManifest.loading` block, as both SDK drains
// produce it. Names are not checked against the UI tree registry here; the
// engine skips unregistered names when it picks a tree for a load.
// See: context/lib/ui.md · context/lib/scripting.md §1

/// Mod-wide loading-screen declaration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModLoading {
    /// Loading-screen tree pool (`loading.tree`): UI tree registry names,
    /// deduplicated in authored order. Empty means absent; loads fall back to
    /// the engine's `loadingScreen` tree.
    pub tree: Vec<String>,
}
