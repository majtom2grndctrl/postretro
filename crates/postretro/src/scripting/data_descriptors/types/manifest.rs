// Data-context descriptors: VM-free manifest POD types.
// See: context/lib/scripting.md §12 (Crate Architecture)

use std::collections::HashMap;

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
