// Data-context descriptors: VM-free manifest POD types.
// See: context/lib/scripting.md §12 (Crate Architecture)

use std::collections::HashMap;

/// A pure impact-policy declaration returned through a manifest's `events`
/// child. The scripting runtime preserves the policy as JSON-compatible data;
/// impact-specific validation, merging, binding, and execution belong to the
/// engine layer that consumes this descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct ImpactEventDescriptor {
    /// SDK-derived stable identity shared by a base declaration and its
    /// cross-scope overrides.
    pub id: String,
    /// Optional tag selector for affected entities. No tag means every impact
    /// is eligible.
    pub filter_tag: Option<String>,
    /// Base or override policy emitted by the pure SDK builder.
    pub policy: Vec<serde_json::Value>,
}

/// Theme tokens supplied by `ModManifest.theme`. Three
/// category-scoped maps mirroring the engine theme tables (colors linear-RGBA,
/// fonts → registered family name, spacing → logical px). Drained into a
/// `ThemeDescriptor`, merged over `engine_default`, and installed via
/// `Renderer::set_ui_theme` by the boot/level-load callers in `main.rs`.
/// See: context/lib/ui.md §2.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModThemeTokens {
    pub colors: HashMap<String, [f32; 4]>,
    pub fonts: HashMap<String, String>,
    pub spacing: HashMap<String, f32>,
}

/// Font assets declared by `ModManifest.fonts`: family name → TTF
/// asset path. Installed into the font system via `register_ui_font` by the
/// boot/level-load callers in `main.rs`. See: context/lib/ui.md §2.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModFontAssets {
    pub families: HashMap<String, String>,
}
