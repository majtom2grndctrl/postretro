// Named UI-tree registry with engine < mod < level scope tiers.
// See: context/lib/ui.md §1.1

use crate::UiTreeEntry;
use crate::descriptor::AnchoredTree;
use postretro_scripting_core::data_descriptors::RegisteredUiTree;

/// Scope tier a registered tree belongs to. Precedence is
/// **engine < mod < level**: a mod tree registered under a name already held by
/// an engine built-in *shadows* the engine entry (the reskin path — last-wins,
/// with a one-line warning at registration time), and a level tree can shadow
/// both for that level's lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeTier {
    /// Engine built-in (HUD, pause menu, on-screen keyboard) registered at boot.
    Engine,
    /// Mod/script-registered tree. Shadows an engine entry of the same name.
    Mod,
    /// Level data-script tree. Cleared on level unload.
    Level,
}

/// One registered tree plus its registration attributes: whether it composes as
/// an always-on base layer every frame (the HUD case), and whether it visually
/// occludes lower pushed entries while retained on the modal stack.
#[derive(Debug, Clone)]
struct RegisteredTree {
    descriptor: AnchoredTree,
    /// `true` when this tree composes as a base layer on every gameplay frame
    /// (resolved through the always-on read seam), independent of the modal stack.
    /// A base/always-on layer NEVER captures input or takes focus — that derives
    /// from the pushed modal stack alone (see `ModalStack`).
    always_on: bool,
    /// When pushed, visually occlude lower pushed entries without removing
    /// them. Popping this entry reveals the retained lower entry again.
    hide_below: bool,
}

#[derive(Debug, Default)]
struct TieredRegisteredTree {
    engine: Option<RegisteredTree>,
    mod_scope: Option<RegisteredTree>,
    level: Option<RegisteredTree>,
}

impl TieredRegisteredTree {
    #[cfg(test)]
    pub(super) fn get(&self, tier: ScopeTier) -> Option<&RegisteredTree> {
        match tier {
            ScopeTier::Engine => self.engine.as_ref(),
            ScopeTier::Mod => self.mod_scope.as_ref(),
            ScopeTier::Level => self.level.as_ref(),
        }
    }

    pub(super) fn set(&mut self, tier: ScopeTier, tree: RegisteredTree) {
        match tier {
            ScopeTier::Engine => self.engine = Some(tree),
            ScopeTier::Mod => self.mod_scope = Some(tree),
            ScopeTier::Level => self.level = Some(tree),
        }
    }

    pub(super) fn remove(&mut self, tier: ScopeTier) {
        match tier {
            ScopeTier::Engine => self.engine = None,
            ScopeTier::Mod => self.mod_scope = None,
            ScopeTier::Level => self.level = None,
        }
    }

    pub(super) fn resolved(&self) -> Option<(ScopeTier, &RegisteredTree)> {
        self.level
            .as_ref()
            .map(|tree| (ScopeTier::Level, tree))
            .or_else(|| self.mod_scope.as_ref().map(|tree| (ScopeTier::Mod, tree)))
            .or_else(|| self.engine.as_ref().map(|tree| (ScopeTier::Engine, tree)))
    }

    pub(super) fn is_empty(&self) -> bool {
        self.engine.is_none() && self.mod_scope.is_none() && self.level.is_none()
    }
}

/// Named registry of UI trees: `name → tiered entries`. `PushTree` resolves a
/// tree by name through this map; the per-frame compose step resolves the HUD
/// and every always-on tree through it too. Tiered by scope
/// (`engine < mod < level`): a mod registration under an existing engine name
/// shadows it without destroying the engine fallback, and a level registration
/// shadows both without destroying either longer-lived fallback. Engine built-ins
/// register at boot; script-side registration arrives with the UI SDK. An
/// unknown name is a no-op-with-warning at push time, never a panic.
#[derive(Debug, Default)]
pub struct UiTreeRegistry {
    trees: std::collections::HashMap<String, TieredRegisteredTree>,
}

impl UiTreeRegistry {
    /// Register (or replace) a named tree at the given `tier`. `always_on` marks it
    /// as a per-frame base layer (the HUD); a pushed-only modal registers with
    /// `always_on = false`. When a `Mod` registration replaces an existing `Engine`
    /// entry under the same name, this is the deliberate reskin/shadow path — it
    /// warns once at registration time so the shadow is visible in the log. Any
    /// other replacement (engine→engine, mod→mod, level→level) is silent.
    pub fn register(
        &mut self,
        name: impl Into<String>,
        tree: AnchoredTree,
        tier: ScopeTier,
        always_on: bool,
    ) {
        self.register_with_presentation(name, tree, tier, always_on, false);
    }

    pub(super) fn register_with_presentation(
        &mut self,
        name: impl Into<String>,
        tree: AnchoredTree,
        tier: ScopeTier,
        always_on: bool,
        hide_below: bool,
    ) {
        let name = name.into();
        // Every registration path — mod init, level load, staged reload — lands
        // here, so the engine panels' reserved names are enforced once.
        if matches!(
            name.as_str(),
            crate::demo::ACCESSIBILITY_PANEL_NAME
                | crate::demo::DISPLAY_MODE_CONFIRM_NAME
                | crate::demo::CONTROLS_PANEL_NAME
                | crate::demo::CONTROLS_CAPTURE_NAME
                | crate::demo::CONTROLS_DIALOG_NAME
        ) && tier != ScopeTier::Engine
        {
            log::warn!(
                "[UI] rejected {tier:?}-scope tree '{name}': the name is reserved for an engine panel or dialog"
            );
            return;
        }
        // Registration is the one build-time point that knows the tree's name, so
        // focus-authoring diagnostics fire here rather than per push or per frame.
        crate::tree::warn_focus_authoring(&name, &tree);
        let entry = self.trees.entry(name.clone()).or_default();
        if tier == ScopeTier::Mod && entry.mod_scope.is_none() && entry.engine.is_some() {
            log::warn!(
                "[UI] mod tree '{name}' shadows the engine built-in of the same name (reskin path)"
            );
        }
        entry.set(
            tier,
            RegisteredTree {
                descriptor: tree,
                always_on,
                hide_below,
            },
        );
    }

    pub(crate) fn replace_tier(
        &mut self,
        tier: ScopeTier,
        trees: impl IntoIterator<Item = RegisteredUiTree>,
    ) {
        for entry in self.trees.values_mut() {
            entry.remove(tier);
        }
        self.trees.retain(|_, entry| !entry.is_empty());
        for RegisteredUiTree {
            name,
            tree,
            always_on,
            hide_below,
        } in trees
        {
            self.register_with_presentation(name, tree, tier, always_on, hide_below);
        }
    }

    /// Resolve a registered tree by name, or `None` if no such name is registered.
    /// Tiered resolution picks level, then mod, then engine.
    pub(super) fn resolve(&self, name: &str) -> Option<&AnchoredTree> {
        self.resolve_with_tier(name).map(|(_, tree)| tree)
    }

    pub(super) fn resolve_with_tier(&self, name: &str) -> Option<(ScopeTier, &AnchoredTree)> {
        self.trees
            .get(name)
            .and_then(TieredRegisteredTree::resolved)
            .map(|(tier, t)| (tier, &t.descriptor))
    }

    pub(super) fn resolved_trees(&self) -> impl Iterator<Item = (ScopeTier, &AnchoredTree)> {
        self.trees
            .values()
            .filter_map(TieredRegisteredTree::resolved)
            .map(|(tier, t)| (tier, &t.descriptor))
    }

    pub(super) fn resolve_pushable(&self, name: &str) -> Option<(ScopeTier, &AnchoredTree, bool)> {
        self.trees
            .get(name)
            .and_then(TieredRegisteredTree::resolved)
            .map(|(tier, tree)| (tier, &tree.descriptor, tree.hide_below))
    }

    /// The always-on trees, each as a base-layer snapshot entry. The compose step
    /// appends these beneath the pushed modal stack every gameplay frame. The
    /// `capture_mode` is carried for diagnostics only — a base layer never captures
    /// input or takes focus (the pushed stack is the sole source for that), so the
    /// compose step does NOT feed these into `top_capture_mode`/`active_name`.
    ///
    /// Ordering is engine-tier-first, then mod-tier, then level-tier; each group
    /// sorts by name so painter order is deterministic frame-over-frame (the
    /// HashMap's own iteration order is not). A higher-tier always-on tree under
    /// a NEW name layers above lower tiers; a higher-tier tree under an EXISTING
    /// name already replaced it in the map (shadow), so it composes in that one
    /// slot.
    pub(super) fn always_on_layers(&self) -> Vec<UiTreeEntry> {
        let mut entries: Vec<(ScopeTier, &String, &RegisteredTree)> = self
            .trees
            .iter()
            .filter_map(|(name, t)| {
                let (tier, tree) = t.resolved()?;
                tree.always_on.then_some((tier, name, tree))
            })
            .collect();
        entries.sort_by(|a, b| {
            tier_order(a.0)
                .cmp(&tier_order(b.0))
                .then_with(|| a.1.cmp(b.1))
        });
        entries
            .into_iter()
            .map(|(tier, name, t)| UiTreeEntry {
                name: name.clone(),
                tier,
                descriptor: t.descriptor.clone(),
                capture_mode: t.descriptor.capture_mode,
                on_commit: None,
            })
            .collect()
    }

    #[cfg(test)]
    pub(super) fn tier_of(&self, name: &str) -> Option<ScopeTier> {
        self.trees
            .get(name)
            .and_then(TieredRegisteredTree::resolved)
            .map(|(tier, _)| tier)
    }

    #[cfg(test)]
    pub(super) fn has_tier(&self, name: &str, tier: ScopeTier) -> bool {
        self.trees.get(name).and_then(|t| t.get(tier)).is_some()
    }
}

/// Painter-order rank for a scope tier: engine (base), mod, then level overlays.
fn tier_order(tier: ScopeTier) -> u8 {
    match tier {
        ScopeTier::Engine => 0,
        ScopeTier::Mod => 1,
        ScopeTier::Level => 2,
    }
}
