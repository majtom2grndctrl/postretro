// Runtime-side manifest types that embed render::ui descriptor data.
// See: context/lib/scripting.md §13 (Crate Architecture)

use postretro_foundation::PresentationEasing;
use serde::{Deserialize, Serialize};

use crate::ui::descriptor::{AnchoredTree, Widget};

use super::{
    CrossingDescriptor, ImpactEventDescriptor, NamedReaction, TriggerEventDescriptor,
    TriggerPoolDescriptor,
};

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

/// Renderer-consumed definition for one passive, world-anchored transient.
/// Scripts retain this as manifest data; the app resolves its timing parameters
/// when an impact plans a spawn, and the renderer resolves its widget subtree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresentationTemplate {
    /// Binding-derived stable template handle. It is not author-facing mutable
    /// data: TypeScript's compiler supplies it from a direct `const` binding.
    pub id: String,
    pub root: Widget,
    pub lifetime_ms: u32,
    pub motion: PresentationTemplateMotion,
    pub fade: PresentationTemplateFade,
    pub spawn_scatter: PresentationTemplateSpawnScatter,
}

impl PresentationTemplate {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() {
            return Err("presentation template `id` must be nonempty".to_string());
        }
        if !self.motion.rise.is_finite() {
            return Err("presentation template `motion.rise` must be finite".to_string());
        }
        if !self.spawn_scatter.radius.is_finite() || self.spawn_scatter.radius < 0.0 {
            return Err(
                "presentation template `spawnScatter.radius` must be finite and non-negative"
                    .to_string(),
            );
        }
        if self.fade.start_ms > self.lifetime_ms {
            return Err(
                "presentation template `fade.startMs` must not exceed `lifetimeMs`".to_string(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresentationTemplateMotion {
    pub rise: f32,
    pub easing: PresentationEasing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresentationTemplateFade {
    pub start_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresentationTemplateSpawnScatter {
    pub radius: f32,
}

/// The full bundle returned by a level's `setupLevel(ctx)` export.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LevelManifest {
    pub reactions: Vec<NamedReaction>,
    /// Level-local impact-policy declarations. Task 5 composes these after
    /// mod-global declarations and performs the author-id merge.
    pub events: Vec<ImpactEventDescriptor>,
    /// State-crossing watchers (M13 HUD dynamics). Parsed alongside `reactions`
    /// from the widened `{ reactions, events, crossings, triggerEvents, triggerPools }` setup-manifest return and
    /// drained into the per-level `DataRegistry`; cleared on level unload.
    pub crossings: Vec<CrossingDescriptor>,
    /// Trigger-volume enter/exit watchers declared via the `triggerEvents`
    /// field. Composes with mod-global `ModManifest.triggerEvents` entries
    /// matched by the `levels` tag selector; per-level and cleared on unload.
    pub trigger_events: Vec<TriggerEventDescriptor>,
    /// Trigger-volume pool declarations. Their `levels` selector is retained
    /// for the shared descriptor contract, but level-local pools always apply
    /// to the level that declared them.
    pub trigger_pools: Vec<TriggerPoolDescriptor>,
    /// Per-level UI trees declared via the `uiTrees` field. A malformed entry is
    /// logged and skipped rather than aborting level load (`ui.md` §1.1).
    pub ui_trees: Vec<RegisteredUiTree>,
}
