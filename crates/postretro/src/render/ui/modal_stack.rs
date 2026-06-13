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

/// Named registry of engine built-in trees: `name → AnchoredTree`. `PushTree`
/// resolves a tree by name through this map. Engine built-ins register at boot;
/// script-side registration arrives with a later goal. An unknown name is a
/// no-op-with-warning at push time, never a panic.
#[derive(Debug, Default)]
pub(crate) struct UiTreeRegistry {
    trees: std::collections::HashMap<String, AnchoredTree>,
}

impl UiTreeRegistry {
    /// Register (or replace) a named tree. Engine built-ins call this at boot.
    pub(crate) fn register(&mut self, name: impl Into<String>, tree: AnchoredTree) {
        self.trees.insert(name.into(), tree);
    }

    /// Resolve a registered tree by name, or `None` if no such name is registered.
    fn resolve(&self, name: &str) -> Option<&AnchoredTree> {
        self.trees.get(name)
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.trees.contains_key(name)
    }
}

/// One tree currently on the modal stack: its registry name, the descriptor
/// instance pushed, and the optional `onCommit` reaction carried from the
/// `PushTree` that opened it. `on_commit` is carried only — fired by a later goal.
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

    /// Resolve a registered tree by `name` and push it (script `PushTree` path).
    /// An unknown name warns and is a no-op — never a panic. `on_commit` is
    /// carried onto the entry for a later goal to fire on commit.
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
        UiReadSnapshot::with_trees(self.entries(), slot_values, time_seconds, None)
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
                children: vec![],
            }),
            capture_mode,
            initial_focus: None,
            text_entry_target: None,
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

    #[test]
    fn push_named_resolves_through_registry_and_becomes_active() {
        let mut stack = ModalStack::new();
        stack.registry_mut().register("pauseMenu", capturing());
        assert!(stack.is_empty());

        stack.push_named("pauseMenu", Some("resume".to_string()));
        assert_eq!(stack.len(), 1);
        assert_eq!(stack.active_name(), Some("pauseMenu"));
    }

    #[test]
    fn push_pop_changes_the_active_top_tree() {
        let mut stack = ModalStack::new();
        stack.registry_mut().register("hud", passthrough());
        stack.registry_mut().register("pause", capturing());

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
        stack.registry_mut().register("hud", passthrough());
        stack.registry_mut().register("pause", capturing());

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
        stack.registry_mut().register("hud", passthrough());
        stack.registry_mut().register("pause", capturing());
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
        stack.registry_mut().register("dialog", capturing());
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
        stack.registry_mut().register("dialog", capturing());
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
}
