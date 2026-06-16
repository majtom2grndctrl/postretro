// App-side gameplay-UI modal stack + named-tree registry: resolves Goal E's
// `PushTree`/`PopTree` system commands by name into a stack of descriptor trees,
// and exposes an engine push/pop API for pause/dialog. The renderer draws the
// stack bottom→top; the top tree's capture mode drives the input seam + focus.
// Pure CPU — no taffy, no GPU, no input subsystem state mutation (the App reads
// `top_capture_mode` and wires the seam).
//
// The boot splash stays OUTSIDE this stack (boot predates the store and game
// logic the stack assumes); gameplay UI only.
// See: context/lib/ui.md §1

#[cfg(test)]
use super::UiReadSnapshot;
use super::UiTreeEntry;
use super::descriptor::AnchoredTree;
use crate::input::UiCaptureMode;
use crate::scripting::data_descriptors::RegisteredUiTree;

/// Scope tier a registered tree belongs to. Precedence is **engine < mod**: a mod
/// tree registered under a name already held by an engine built-in *shadows* the
/// engine entry (the reskin path — last-wins, with a one-line warning at
/// registration time). The per-level tier is DEFERRED (single-level lifetime, no
/// runtime unload site today), so `setupLevel` trees register into `Mod` for now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeTier {
    /// Engine built-in (HUD, pause menu, on-screen keyboard) registered at boot.
    Engine,
    /// Mod/script-registered tree. Shadows an engine entry of the same name.
    Mod,
}

/// One registered tree plus its registration attributes: which scope tier owns it
/// and whether it composes as an always-on base layer every frame (the HUD case)
/// rather than only when pushed onto the modal stack.
#[derive(Debug, Clone)]
struct RegisteredTree {
    descriptor: AnchoredTree,
    tier: ScopeTier,
    /// `true` when this tree composes as a base layer on every gameplay frame
    /// (resolved through the always-on read seam), independent of the modal stack.
    /// A base/always-on layer NEVER captures input or takes focus — that derives
    /// from the pushed modal stack alone (see `ModalStack`).
    always_on: bool,
}

/// Named registry of UI trees: `name → RegisteredTree`. `PushTree` resolves a tree
/// by name through this map; the per-frame compose step resolves the HUD and every
/// always-on tree through it too. Tiered by scope (`engine < mod`): a mod
/// registration under an existing engine name shadows it (last-wins + warn).
/// Engine built-ins register at boot; script-side registration arrives with the UI
/// SDK. An unknown name is a no-op-with-warning at push time, never a panic.
#[derive(Debug, Default)]
pub(crate) struct UiTreeRegistry {
    trees: std::collections::HashMap<String, RegisteredTree>,
}

impl UiTreeRegistry {
    /// Register (or replace) a named tree at the given `tier`. `always_on` marks it
    /// as a per-frame base layer (the HUD); a pushed-only modal registers with
    /// `always_on = false`. When a `Mod` registration replaces an existing `Engine`
    /// entry under the same name, this is the deliberate reskin/shadow path — it
    /// warns once at registration time so the shadow is visible in the log. Any
    /// other replacement (engine→engine, mod→mod) is silent.
    pub(crate) fn register(
        &mut self,
        name: impl Into<String>,
        tree: AnchoredTree,
        tier: ScopeTier,
        always_on: bool,
    ) {
        let name = name.into();
        if tier == ScopeTier::Mod {
            if let Some(existing) = self.trees.get(&name) {
                if existing.tier == ScopeTier::Engine {
                    log::warn!(
                        "[UI] mod tree '{name}' shadows the engine built-in of the same name (reskin path)"
                    );
                }
            }
        }
        self.trees.insert(
            name,
            RegisteredTree {
                descriptor: tree,
                tier,
                always_on,
            },
        );
    }

    /// Resolve a registered tree by name, or `None` if no such name is registered.
    /// The HashMap holds at most one entry per name (a mod registration replaced
    /// any engine entry of the same name at registration time), so this read is the
    /// tiered-resolved descriptor directly.
    fn resolve(&self, name: &str) -> Option<&AnchoredTree> {
        self.trees.get(name).map(|t| &t.descriptor)
    }

    /// The always-on trees, each as a base-layer snapshot entry. The compose step
    /// appends these beneath the pushed modal stack every gameplay frame. The
    /// `capture_mode` is carried for diagnostics only — a base layer never captures
    /// input or takes focus (the pushed stack is the sole source for that), so the
    /// compose step does NOT feed these into `top_capture_mode`/`active_name`.
    ///
    /// Ordering is engine-tier-first, then mod-tier, each group in a stable sort by
    /// name so painter order is deterministic frame-over-frame (the HashMap's own
    /// iteration order is not). A mod always-on tree under a NEW name layers above
    /// the engine HUD; a mod tree under an EXISTING engine name already replaced it
    /// in the map (shadow), so it composes in that one slot.
    fn always_on_layers(&self) -> Vec<UiTreeEntry> {
        let mut entries: Vec<(ScopeTier, &String, &RegisteredTree)> = self
            .trees
            .iter()
            .filter(|(_, t)| t.always_on)
            .map(|(name, t)| (t.tier, name, t))
            .collect();
        // Engine tier first (base), then mod tier (overlays on top); within a tier,
        // sort by name for a deterministic painter order across frames.
        entries.sort_by(|a, b| {
            tier_order(a.0)
                .cmp(&tier_order(b.0))
                .then_with(|| a.1.cmp(b.1))
        });
        entries
            .into_iter()
            .map(|(_, name, t)| UiTreeEntry {
                name: name.clone(),
                descriptor: t.descriptor.clone(),
                capture_mode: t.descriptor.capture_mode.into(),
                on_commit: None,
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.trees.contains_key(name)
    }

    #[cfg(test)]
    fn tier_of(&self, name: &str) -> Option<ScopeTier> {
        self.trees.get(name).map(|t| t.tier)
    }
}

/// Painter-order rank for a scope tier: engine (base) below mod (overlay).
fn tier_order(tier: ScopeTier) -> u8 {
    match tier {
        ScopeTier::Engine => 0,
        ScopeTier::Mod => 1,
    }
}

/// One tree currently on the modal stack: its registry name, the descriptor
/// instance pushed, and the optional `onCommit` reaction carried from the
/// `PushTree` that opened it. `on_commit` is carried on the stack entry; the App
/// fires it from the text-entry commit path.
#[derive(Debug, Clone, PartialEq)]
struct StackedTree {
    name: String,
    descriptor: AnchoredTree,
    on_commit: Option<String>,
}

/// The gameplay-UI modal stack: a registry of named trees plus the live stack of
/// pushed trees (bottom→top). The HUD, when present, is the bottom of the stack;
/// modal trees (pause, dialog) stack above it. The top tree's capture mode is the
/// one the App acts on — it freezes gameplay + lower trees and releases the cursor.
///
/// Push/pop sources:
/// - script commands (`PushTree`/`PopTree`, drained from the system-command queue)
///   resolve a name through the registry (`push_named` / `pop`),
/// - the engine push/pop API (`push` / `pop`) for pause/dialog opened from Rust.
#[derive(Debug, Default)]
pub(crate) struct ModalStack {
    registry: UiTreeRegistry,
    stack: Vec<StackedTree>,
}

impl ModalStack {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Mutable access to the registry so the boot path can register engine
    /// built-in trees by name.
    pub(crate) fn registry_mut(&mut self) -> &mut UiTreeRegistry {
        &mut self.registry
    }

    /// Drain script-authored trees parsed off a manifest result into the registry
    /// at the given scope tier, carrying each entry's `always_on` attribute. It
    /// feeds the trees `setupMod` / `setupLevel` returned (as `RegisteredUiTree`
    /// envelopes) into the tiered registry at the register→VM-drop lifecycle
    /// point — before the mod-init / data-script VM context drops.
    ///
    /// Both mod-scope and level-scope trees register at `ScopeTier::Mod` today: a
    /// mod-tier registration under an existing engine name shadows it (last-wins +
    /// one-line warning, in `UiTreeRegistry::register`). The per-level tier is
    /// DEFERRED — single-level lifetime with no runtime unload site — so level
    /// trees go into the persistent (`Mod`) registry alongside mod trees. A
    /// duplicate name follows the same last-wins behavior and never aborts boot or
    /// level load.
    pub(crate) fn register_script_trees(
        &mut self,
        trees: impl IntoIterator<Item = RegisteredUiTree>,
        tier: ScopeTier,
    ) {
        for RegisteredUiTree {
            name,
            tree,
            always_on,
        } in trees
        {
            self.registry.register(name, tree, tier, always_on);
        }
    }

    /// Read a registered tree by `name`, or `None` if no such name is registered.
    /// Public `&self` read seam onto the registry's tiered resolution: keeps
    /// `UiTreeRegistry::resolve` private to `push_named`'s internal use. The
    /// per-frame compose step pulls the HUD via `always_on_layers` rather than
    /// resolving `HUD_NAME` by hand; production pushes resolve by name through
    /// `push_named`. Only the tiered-resolution tests exercise this accessor.
    #[cfg_attr(not(test), allow(dead_code))] // public read seam; production resolves via push_named — accessor is test-only
    pub(crate) fn tree(&self, name: &str) -> Option<&AnchoredTree> {
        self.registry.resolve(name)
    }

    /// The always-on base layers for this frame (the HUD and any mod-registered
    /// always-on overlays), engine-tier first then mod-tier, each in a deterministic
    /// per-name order. The compose step appends these as the bottom layers of the
    /// per-frame snapshot, with pushed modal entries (`entries`) on top.
    ///
    /// CAPTURE/FOCUS INVARIANT: these are draw-only base layers — they are NOT on
    /// the pushed modal stack, which is the SOLE source of `top_capture_mode` /
    /// `active_name` / `active_text_entry_target`. So an always-on layer never
    /// captures input or takes focus even if its descriptor declares
    /// `captureMode: capture`; an always-on overlay cannot steal input.
    pub(crate) fn always_on_layers(&self) -> Vec<UiTreeEntry> {
        self.registry.always_on_layers()
    }

    /// Resolve a registered tree by `name` and push it (script `PushTree` path).
    /// An unknown name warns and is a no-op — never a panic. `on_commit` is
    /// carried onto the entry so the App can fire it from the text-entry commit
    /// path.
    pub(crate) fn push_named(&mut self, name: &str, on_commit: Option<String>) {
        let Some(descriptor) = self.registry.resolve(name).cloned() else {
            log::warn!(
                "[UI] pushTree('{name}') — no tree registered under that name; ignoring (no panic)"
            );
            return;
        };
        self.stack.push(StackedTree {
            name: name.to_string(),
            descriptor,
            on_commit,
        });
    }

    /// Engine push API: push a descriptor tree directly (pause/dialog opened from
    /// Rust, not via a registered name). `name` labels the entry for diagnostics.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn push(&mut self, name: impl Into<String>, descriptor: AnchoredTree) {
        self.stack.push(StackedTree {
            name: name.into(),
            descriptor,
            on_commit: None,
        });
    }

    /// Pop the top tree. A pop on an empty stack warns and is a no-op (a script
    /// `closeDialog` with nothing open is a no-op, not a crash).
    pub(crate) fn pop(&mut self) {
        if self.stack.pop().is_none() {
            log::warn!("[UI] popTree — modal stack is already empty; ignoring (no panic)");
        }
    }

    /// The TOP tree's capture mode (the one the App acts on). `Passthrough` when
    /// the stack is empty or the top tree declares passthrough (the HUD case), so
    /// gameplay keeps input and the cursor stays captured.
    pub(crate) fn top_capture_mode(&self) -> UiCaptureMode {
        self.stack
            .last()
            .map(|t| t.descriptor.capture_mode.into())
            .unwrap_or(UiCaptureMode::Passthrough)
    }

    /// Number of trees on the stack.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.stack.len()
    }

    /// True when no tree is on the stack.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// The registry name of the top (active) tree, or `None` when empty. Only the
    /// top tree is "active"; lower trees are frozen (no focus, no activation). The
    /// App keys the focus engine on this (falling back to the always-on HUD).
    pub(crate) fn active_name(&self) -> Option<&str> {
        self.stack.last().map(|t| t.name.as_str())
    }

    /// The TOP tree's `text_entry_target` slot, when it declares one (M13
    /// Text-Entry, Task 3). `Some(slot)` is the "text entry is open" condition the
    /// App gates hardware-key routing on; `None` (empty stack, or a top tree with
    /// no text entry) means text entry is closed. Only the top tree is consulted —
    /// lower trees are frozen.
    pub(crate) fn active_text_entry_target(&self) -> Option<&str> {
        self.stack
            .last()
            .and_then(|t| t.descriptor.text_entry_target.as_deref())
    }

    /// The TOP tree's `on_commit` reaction, carried from the `PushTree` that
    /// opened it (M13 Text-Entry, Task 3). Fired by the App on commit (Enter or the
    /// `done` key), then the tree is popped. `None` when the stack is empty or the
    /// top tree carries no commit reaction.
    pub(crate) fn active_on_commit(&self) -> Option<&str> {
        self.stack.last().and_then(|t| t.on_commit.as_deref())
    }

    /// The live stack as snapshot entries, bottom→top. The App prepends the
    /// always-on HUD entry (the bottom-most gameplay UI layer) ahead of these
    /// modal overlays when composing the per-frame snapshot.
    pub(crate) fn entries(&self) -> Vec<UiTreeEntry> {
        self.stack
            .iter()
            .map(|t| UiTreeEntry {
                name: t.name.clone(),
                descriptor: t.descriptor.clone(),
                capture_mode: t.descriptor.capture_mode.into(),
                on_commit: t.on_commit.clone(),
            })
            .collect()
    }

    /// Build a read snapshot from the live modal stack alone (no HUD layer),
    /// drawn bottom→top. Used by the stack's own tests; the App composes the HUD
    /// layer in front of `entries()` instead.
    #[cfg(test)]
    pub(crate) fn build_snapshot(
        &self,
        slot_values: std::collections::HashMap<String, crate::scripting::slot_table::SlotValue>,
        time_seconds: f64,
    ) -> UiReadSnapshot {
        UiReadSnapshot::with_trees(
            self.entries(),
            slot_values,
            crate::render::ui::tree::CellValues::new(),
            time_seconds,
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::render::ui::descriptor::{
        Align, CaptureMode, ContainerWidget, SpacingValue, Widget,
    };
    use crate::render::ui::layout::Anchor;

    /// A minimal tree with the given capture mode.
    fn tree(capture_mode: CaptureMode) -> AnchoredTree {
        AnchoredTree {
            anchor: Anchor::Center,
            offset: [0.0, 0.0],
            root: Widget::VStack(ContainerWidget {
                gap: SpacingValue::Literal(0.0),
                padding: SpacingValue::Literal(0.0),
                align: Align::Start,
                fill: None,
                border: None,
                id: None,
                focus_neighbors: Default::default(),
                focus: None,
                restore_on_return: false,
                local_state: None,
                visible_when: None,
                role: None,
                children: vec![],
            }),
            capture_mode,
            initial_focus: None,
            text_entry_target: None,
            accessible_name: None,
            role: None,
        }
    }

    /// A capturing tree that declares a text-entry target slot — the "text entry
    /// is open" condition for the top tree.
    fn text_entry_tree(target: &str) -> AnchoredTree {
        let mut t = tree(CaptureMode::Capture);
        t.text_entry_target = Some(target.to_string());
        t
    }

    fn capturing() -> AnchoredTree {
        tree(CaptureMode::Capture)
    }

    fn passthrough() -> AnchoredTree {
        tree(CaptureMode::Passthrough)
    }

    /// Register a pushed-only (non-always-on) engine-tier tree under `name`. The
    /// stack/push tests register through this; tier/always-on don't affect push
    /// behavior, so they use the engine default.
    fn register_pushable(stack: &mut ModalStack, name: &str, tree: AnchoredTree) {
        stack
            .registry_mut()
            .register(name, tree, ScopeTier::Engine, false);
    }

    #[test]
    fn push_named_resolves_through_registry_and_becomes_active() {
        let mut stack = ModalStack::new();
        register_pushable(&mut stack, "pauseMenu", capturing());
        assert!(stack.is_empty());

        stack.push_named("pauseMenu", Some("resume".to_string()));
        assert_eq!(stack.len(), 1);
        assert_eq!(stack.active_name(), Some("pauseMenu"));
    }

    #[test]
    fn push_pop_changes_the_active_top_tree() {
        let mut stack = ModalStack::new();
        register_pushable(&mut stack, "hud", passthrough());
        register_pushable(&mut stack, "pause", capturing());

        stack.push_named("hud", None);
        stack.push_named("pause", None);
        // Two trees on the stack; the LAST pushed is active (top).
        assert_eq!(stack.len(), 2);
        assert_eq!(stack.active_name(), Some("pause"));

        // Popping restores the lower tree as active.
        stack.pop();
        assert_eq!(stack.len(), 1);
        assert_eq!(stack.active_name(), Some("hud"));
    }

    #[test]
    fn push_unknown_name_warns_and_is_a_noop_no_panic() {
        let mut stack = ModalStack::new();
        // No registration for "ghost"; the push must not panic and must not grow
        // the stack.
        stack.push_named("ghost", None);
        assert!(stack.is_empty(), "unknown tree name must not push anything");
    }

    #[test]
    fn pop_on_empty_stack_is_a_noop_no_panic() {
        let mut stack = ModalStack::new();
        stack.pop();
        assert!(stack.is_empty());
    }

    #[test]
    fn top_capturing_tree_drives_capture_mode() {
        let mut stack = ModalStack::new();
        register_pushable(&mut stack, "hud", passthrough());
        register_pushable(&mut stack, "pause", capturing());

        // Empty stack => passthrough (gameplay keeps input).
        assert_eq!(stack.top_capture_mode(), UiCaptureMode::Passthrough);

        // HUD (passthrough) on the stack => still passthrough; a HUD never
        // captures.
        stack.push_named("hud", None);
        assert_eq!(
            stack.top_capture_mode(),
            UiCaptureMode::Passthrough,
            "a passthrough HUD never captures",
        );

        // A capturing modal on top => capture (cursor releases, gameplay freezes).
        stack.push_named("pause", None);
        assert_eq!(stack.top_capture_mode(), UiCaptureMode::Capture);

        // Popping the modal restores the HUD's passthrough.
        stack.pop();
        assert_eq!(stack.top_capture_mode(), UiCaptureMode::Passthrough);
    }

    #[test]
    fn snapshot_preserves_bottom_to_top_painter_order() {
        let mut stack = ModalStack::new();
        register_pushable(&mut stack, "hud", passthrough());
        register_pushable(&mut stack, "pause", capturing());
        stack.push_named("hud", None);
        stack.push_named("pause", None);

        let snapshot = stack.build_snapshot(HashMap::new(), 0.0);
        // trees[0] is the bottom (HUD), the last entry is the top (pause).
        assert_eq!(snapshot.trees.len(), 2);
        assert_eq!(snapshot.trees[0].name, "hud");
        assert_eq!(snapshot.trees[1].name, "pause");
        // The top entry carries the capturing mode; the bottom passes through.
        assert_eq!(snapshot.trees[0].capture_mode, UiCaptureMode::Passthrough);
        assert_eq!(snapshot.trees[1].capture_mode, UiCaptureMode::Capture);
    }

    #[test]
    fn empty_stack_builds_an_empty_snapshot() {
        let stack = ModalStack::new();
        let snapshot = stack.build_snapshot(HashMap::new(), 0.0);
        assert!(
            snapshot.trees.is_empty(),
            "an empty stack publishes no trees (UI pass early-outs)",
        );
    }

    #[test]
    fn on_commit_is_carried_through_the_snapshot_entry() {
        let mut stack = ModalStack::new();
        register_pushable(&mut stack, "dialog", capturing());
        stack.push_named("dialog", Some("onYes".to_string()));

        let snapshot = stack.build_snapshot(HashMap::new(), 0.0);
        assert_eq!(
            snapshot.trees[0].on_commit.as_deref(),
            Some("onYes"),
            "the onCommit reaction rides the entry (carried, not fired)",
        );
    }

    #[test]
    fn active_text_entry_target_reads_only_the_top_tree() {
        // Only the TOP tree's text-entry target is consulted; a lower text-entry
        // tree under a plain capturing tree reads as closed.
        let mut stack = ModalStack::new();
        stack.push("editor", text_entry_tree("ui.textEntry"));
        assert_eq!(stack.active_text_entry_target(), Some("ui.textEntry"));

        // A non-text-entry tree on top closes text entry (the lower one is frozen).
        stack.push("confirm", capturing());
        assert_eq!(
            stack.active_text_entry_target(),
            None,
            "a top tree without a text-entry target reads as closed",
        );

        // Popping it restores the editor's target.
        stack.pop();
        assert_eq!(stack.active_text_entry_target(), Some("ui.textEntry"));
    }

    #[test]
    fn active_text_entry_target_is_none_on_empty_stack() {
        let stack = ModalStack::new();
        assert_eq!(stack.active_text_entry_target(), None);
    }

    #[test]
    fn active_on_commit_reads_the_top_trees_carried_reaction() {
        // The top tree's carried `on_commit` (from `PushTree { on_commit }`) is
        // exposed for the App to fire on commit; a tree pushed without one reads None.
        let mut stack = ModalStack::new();
        register_pushable(&mut stack, "dialog", capturing());
        stack.push_named("dialog", Some("onNameEntered".to_string()));
        assert_eq!(stack.active_on_commit(), Some("onNameEntered"));

        stack.push_named("dialog", None);
        assert_eq!(stack.active_on_commit(), None);
    }

    #[test]
    fn engine_push_api_pushes_a_descriptor_directly() {
        // The engine push/pop API takes a descriptor, not a registered name — the
        // pause/dialog path opened from Rust.
        let mut stack = ModalStack::new();
        stack.push("engineDialog", capturing());
        assert_eq!(stack.active_name(), Some("engineDialog"));
        assert_eq!(stack.top_capture_mode(), UiCaptureMode::Capture);
    }

    // ----- Tiered registry (engine < mod) + always-on compose (Task 2) -----

    /// A tree carrying an `id` on its root so descriptor identity is observable
    /// across tier replacements (the bare helper trees are structurally equal).
    fn identified(capture_mode: CaptureMode, root_id: &str) -> AnchoredTree {
        let mut t = tree(capture_mode);
        if let Widget::VStack(c) = &mut t.root {
            c.id = Some(root_id.to_string());
        }
        t
    }

    /// The `id` on a tree's root (set by `identified`), for asserting which
    /// descriptor won a tiered resolution.
    fn root_id(tree: &AnchoredTree) -> Option<&str> {
        match &tree.root {
            Widget::VStack(c) => c.id.as_deref(),
            _ => None,
        }
    }

    #[test]
    fn mod_tier_shadows_engine_entry_of_the_same_name() {
        // engine < mod: a mod registration under an existing engine name replaces
        // it in the single map slot (last-wins). Resolution stays the &self
        // `ModalStack::tree` seam and returns the mod descriptor.
        let mut stack = ModalStack::new();
        let reg = stack.registry_mut();
        reg.register(
            "hud",
            identified(CaptureMode::Passthrough, "engineHud"),
            ScopeTier::Engine,
            true,
        );
        // The shadow warning fires here (engine entry under "hud" already exists).
        reg.register(
            "hud",
            identified(CaptureMode::Passthrough, "modHud"),
            ScopeTier::Mod,
            true,
        );

        // The map holds exactly one "hud" entry: the mod descriptor, at the mod tier.
        assert_eq!(stack.registry.tier_of("hud"), Some(ScopeTier::Mod));
        assert_eq!(
            stack.tree("hud").and_then(root_id),
            Some("modHud"),
            "the mod tree shadows the engine built-in under the same name",
        );
    }

    #[test]
    fn engine_registration_does_not_shadow_a_later_mod_under_a_new_name() {
        // A mod tree under a NEW name does not replace any engine entry — both
        // coexist; tiered resolution returns each by its own name.
        let mut stack = ModalStack::new();
        let reg = stack.registry_mut();
        reg.register(
            "hud",
            identified(CaptureMode::Passthrough, "engineHud"),
            ScopeTier::Engine,
            true,
        );
        reg.register(
            "modOverlay",
            identified(CaptureMode::Passthrough, "overlay"),
            ScopeTier::Mod,
            true,
        );

        assert_eq!(stack.registry.tier_of("hud"), Some(ScopeTier::Engine));
        assert_eq!(stack.registry.tier_of("modOverlay"), Some(ScopeTier::Mod));
        assert_eq!(stack.tree("hud").and_then(root_id), Some("engineHud"));
        assert_eq!(stack.tree("modOverlay").and_then(root_id), Some("overlay"));
    }

    #[test]
    fn always_on_layers_compose_engine_first_then_mod_each_sorted_by_name() {
        // Always-on trees compose as base layers in a deterministic painter order:
        // engine tier (base) first, then mod tier (overlay), each group sorted by
        // name — independent of HashMap iteration order. Pushed-only modals
        // (always_on = false) never appear among the base layers.
        let mut stack = ModalStack::new();
        let reg = stack.registry_mut();
        reg.register("hud", passthrough(), ScopeTier::Engine, true);
        reg.register("engineBg", passthrough(), ScopeTier::Engine, true);
        reg.register("zModOverlay", passthrough(), ScopeTier::Mod, true);
        reg.register("aModOverlay", passthrough(), ScopeTier::Mod, true);
        // A pushed-only modal must NOT compose as a base layer.
        reg.register("pause", capturing(), ScopeTier::Engine, false);

        let names: Vec<String> = stack
            .always_on_layers()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(
            names,
            vec![
                "engineBg".to_string(), // engine tier, sorted by name
                "hud".to_string(),
                "aModOverlay".to_string(), // mod tier, sorted by name, above engine
                "zModOverlay".to_string(),
            ],
            "engine tier composes below mod tier; each group is sorted by name",
        );
    }

    #[test]
    fn compose_assembles_base_layers_below_pushed_modals() {
        // The compose order the App publishes: always-on base layers (bottom),
        // then pushed modal entries (top), in one snapshot `trees` vec.
        let mut stack = ModalStack::new();
        stack
            .registry_mut()
            .register("hud", passthrough(), ScopeTier::Engine, true);
        register_pushable(&mut stack, "pause", capturing());
        stack.push_named("pause", None);

        // Mirror the App's compose step (main.rs): always_on_layers ++ entries.
        let mut trees = stack.always_on_layers();
        trees.extend(stack.entries());

        let names: Vec<&str> = trees.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["hud", "pause"],
            "base/always-on layers compose below pushed modal entries",
        );
    }

    #[test]
    fn always_on_layer_never_captures_or_takes_focus_even_if_it_declares_capture() {
        // CAPTURE/FOCUS INVARIANT: an always-on tree declaring captureMode: Capture
        // composes as a base layer but is NOT on the pushed modal stack, which is
        // the SOLE source of top_capture_mode / active_name / active_text_entry.
        // So it must never capture input or take focus.
        let mut stack = ModalStack::new();
        stack.registry_mut().register(
            "greedyHud",
            text_entry_tree("ui.textEntry"), // capturing + declares text entry
            ScopeTier::Engine,
            true,
        );

        // It DOES compose as a base layer (it renders)...
        let layers = stack.always_on_layers();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].name, "greedyHud");
        // ...but the pushed stack is empty, so it captures/focuses nothing.
        assert!(stack.is_empty());
        assert_eq!(
            stack.top_capture_mode(),
            UiCaptureMode::Passthrough,
            "an always-on layer declaring Capture must not capture — the pushed \
             stack is empty",
        );
        assert_eq!(
            stack.active_name(),
            None,
            "an always-on layer never becomes the active (focused) tree",
        );
        assert_eq!(
            stack.active_text_entry_target(),
            None,
            "an always-on layer never opens text entry, even if it declares a target",
        );
    }

    // ----- Script-tree drain into the tiered registry (Task 3) -----

    /// A `RegisteredUiTree` envelope as the manifest parsers produce it: a named,
    /// identified tree plus its `always_on` registration attribute. Mirrors what
    /// `setupMod` / `setupLevel` return through `RegisteredUiTree` (Task 1).
    fn registered(name: &str, root_id: &str, always_on: bool) -> RegisteredUiTree {
        RegisteredUiTree {
            name: name.to_string(),
            tree: identified(CaptureMode::Passthrough, root_id),
            always_on,
        }
    }

    #[test]
    fn setup_mod_tree_is_resolvable_at_mod_tier_after_registration() {
        // A `setupMod`-registered tree resolves by name through the tiered
        // registry after mod-init, at the mod tier — the cold-boot drain point
        // feeding the by-name render resolution seam.
        let mut stack = ModalStack::new();
        stack.register_script_trees(
            vec![registered("objectiveBoard", "modBoard", false)],
            ScopeTier::Mod,
        );

        assert_eq!(
            stack.registry.tier_of("objectiveBoard"),
            Some(ScopeTier::Mod)
        );
        assert_eq!(
            stack.tree("objectiveBoard").and_then(root_id),
            Some("modBoard"),
            "a setupMod tree resolves by name through the registry after the drain",
        );
    }

    #[test]
    fn mod_tree_under_engine_hud_name_shadows_the_engine_hud() {
        // The reskin path: an engine HUD registered at boot, then a mod tree
        // drained under the SAME name from `setupMod`, shadows it (last-wins).
        // The shadow warning is emitted by `UiTreeRegistry::register` (Task 2);
        // here we prove the drain actually registers the mod tree at `Mod` tier
        // under that name so the shadow takes effect.
        let mut stack = ModalStack::new();
        stack.registry_mut().register(
            "hud",
            identified(CaptureMode::Passthrough, "engineHud"),
            ScopeTier::Engine,
            true,
        );

        stack.register_script_trees(vec![registered("hud", "modHud", true)], ScopeTier::Mod);

        assert_eq!(stack.registry.tier_of("hud"), Some(ScopeTier::Mod));
        assert_eq!(
            stack.tree("hud").and_then(root_id),
            Some("modHud"),
            "the drained mod tree shadows the engine HUD under the same name",
        );
    }

    #[test]
    fn setup_level_trees_register_into_the_persistent_mod_tier() {
        // Level-scope trees register into the persistent (`Mod`) tier today — the
        // per-level tier is DEFERRED. After the level-load drain they are
        // resolvable by name, exactly like mod-scope trees.
        let mut stack = ModalStack::new();
        stack.register_script_trees(
            vec![registered("levelBanner", "banner", true)],
            ScopeTier::Mod,
        );

        assert_eq!(stack.registry.tier_of("levelBanner"), Some(ScopeTier::Mod));
        assert_eq!(
            stack.tree("levelBanner").and_then(root_id),
            Some("banner"),
            "a setupLevel tree is resolvable in the persistent tier after level load",
        );
    }

    #[test]
    fn duplicate_drained_name_is_last_wins_and_never_aborts() {
        // A malformed/duplicate registration must not abort the drain: a second
        // entry under the same name wins (last-wins), the first is replaced, and
        // the whole drain completes for the remaining entries.
        let mut stack = ModalStack::new();
        stack.register_script_trees(
            vec![
                registered("dup", "first", false),
                registered("dup", "second", false),
                registered("other", "kept", false),
            ],
            ScopeTier::Mod,
        );

        assert_eq!(
            stack.tree("dup").and_then(root_id),
            Some("second"),
            "a duplicate name within one drain is last-wins, not an abort",
        );
        assert_eq!(
            stack.tree("other").and_then(root_id),
            Some("kept"),
            "entries after a duplicate still register — the drain never aborts",
        );
    }
}
