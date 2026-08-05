// Data-context descriptors: VM-free manifest POD types.
// See: context/lib/scripting.md §12 (Crate Architecture)

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::data_descriptors::DescriptorError;

/// Mod-global switching policy. Omission preserves the original direct-select
/// behavior: commits are immediate, wheel selection has no dwell, and reloads
/// may be interrupted unless the current weapon opts out.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchingDescriptor {
    pub commit_on_direct_select: bool,
    pub cycle_commit_dwell_ms: f32,
    pub block_during_reload: bool,
}

impl Default for SwitchingDescriptor {
    fn default() -> Self {
        Self {
            commit_on_direct_select: true,
            cycle_commit_dwell_ms: 0.0,
            block_during_reload: false,
        }
    }
}

impl SwitchingDescriptor {
    pub fn validate(self) -> Result<Self, DescriptorError> {
        if !self.cycle_commit_dwell_ms.is_finite() || self.cycle_commit_dwell_ms < 0.0 {
            return Err(DescriptorError::InvalidShape {
                reason: format!(
                    "`switching.cycleCommitDwellMs` must be a finite value >= 0.0, got {}",
                    self.cycle_commit_dwell_ms
                ),
            });
        }
        Ok(self)
    }
}

/// A pure impact-policy declaration returned through a manifest's `events`
/// child. The scripting runtime preserves the policy as JSON-compatible data;
/// impact-specific validation, merging, binding, and execution belong to the
/// engine layer that consumes this descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct ImpactEventDescriptor {
    /// Author-assigned stable identity shared by a base declaration and its
    /// cross-scope overrides.
    pub id: String,
    /// Distinguishes a refinement from the base declaration it references.
    pub is_override: bool,
    /// Mod-scope map-tag selector. Empty means every level; level-local
    /// declarations retain this field but apply to their declaring level.
    pub levels: Vec<String>,
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
