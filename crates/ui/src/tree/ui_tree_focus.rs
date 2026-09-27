// Focus / hit-test rect export for the retained `UiTree` (the lockstep
// descriptor+taffy walk), plus the focus-authoring diagnostics run at registration.
// See: context/lib/ui.md §4 (interaction / focus)

use std::collections::{HashMap, HashSet};

use taffy::prelude::NodeId;

use super::super::descriptor::{AnchoredTree, Widget};
use super::super::layout::REFERENCE_HEIGHT;
use super::super::layout::REFERENCE_WIDTH;
use postretro_entities::SlotValue;

use super::CellValues;
use super::draw::{
    FocusGroup, FocusRect, FocusRectList, anchor_fractions, canvas_origin, project_rect,
};
use super::ui_tree::UiTree;
use super::widget_meta::{
    any_restore_on_return, authored_focus_neighbors, container_focus_policy, container_local_scope,
    focus_meta, is_interactive, widget_a11y_state, widget_children, widget_id, widget_interaction,
};

impl UiTree {
    /// Export the flat hit-test / focus rect list for this tree against the
    /// descriptor it was built from. Walks descriptor nodes with their matching
    /// outer taffy nodes in lockstep; private visual descendants (such as a
    /// slider's track and readout) remain inside that outer focus rect.
    ///
    /// Uses the SAME device-pixel projection as the draw (`project_rect`,
    /// `canvas_origin`, `device_scale`) so a hit lands on exactly the rect drawn.
    /// Assumes layout is already computed for `device_size` (the caller's gate ran
    /// the compute). Pure read-back — no taffy mutation, no GPU.
    ///
    /// Only interactive widgets (those `widget_interaction` recognizes) export as
    /// focus stops; they join the group of their nearest focus-policy ancestor,
    /// however deeply nested under passive containers. Passive nodes — text,
    /// images, layout containers — never export, even with an authored `id` (a
    /// passive id is a `labelledBy` reference target, not a focus stop).
    ///
    /// Every interactive kind requires an authored id, so each exported id is
    /// authored and carries across structural rebuilds (focus restore relies on
    /// that).
    pub fn export_focus_rects(
        &self,
        descriptor: &AnchoredTree,
        device_size: [u32; 2],
        slot_values: &HashMap<String, SlotValue>,
        cell_values: &CellValues,
    ) -> FocusRectList {
        let root_size = self.taffy.layout(self.root).expect("root has layout").size;
        let (afx, afy) = anchor_fractions(self.anchor);
        let anchor_x = REFERENCE_WIDTH * afx + self.offset[0];
        let anchor_y = REFERENCE_HEIGHT * afy + self.offset[1];
        let root_origin = [
            anchor_x - root_size.width * afx,
            anchor_y - root_size.height * afy,
        ];
        let scale = super::super::layout::device_scale(device_size);
        let canvas_origin = canvas_origin(device_size, scale);

        let mut out = FocusRectList {
            initial_focus: descriptor.initial_focus.clone(),
            restore_on_return: any_restore_on_return(&descriptor.root),
            ..Default::default()
        };
        let mut z = 0u32;
        self.collect_focus_node(
            &descriptor.root,
            self.root,
            None,
            None,
            root_origin,
            scale,
            canvas_origin,
            &mut z,
            &mut out,
            slot_values,
            cell_values,
        );
        out
    }

    /// Lockstep descriptor+taffy walk for `export_focus_rects`. `group` is the
    /// index (into `out.groups`) of the nearest ancestor container that declared a
    /// focus policy. `z` rises in tree order so a later-drawn node hit-tests as
    /// topmost.
    #[allow(clippy::too_many_arguments)]
    fn collect_focus_node(
        &self,
        widget: &Widget,
        node: NodeId,
        group: Option<usize>,
        scope: Option<&str>,
        ref_origin: [f32; 2],
        scale: f32,
        canvas_origin: [f32; 2],
        z: &mut u32,
        out: &mut FocusRectList,
        slot_values: &HashMap<String, SlotValue>,
        cell_values: &CellValues,
    ) {
        // Reactive visibility (M13 G2, Task 2b): a `Display::None` node (a false
        // `visibleWhen`) and its subtree are unreachable for focus — emit no
        // FocusRect, register no focus group, and never recurse. The subtree's
        // focusables thus drop out of the rect list (so they cannot be navigated
        // to) and out of any `initial_focus` candidacy (the engine cannot select
        // an id that isn't present).
        if self.is_display_none(node) {
            return;
        }
        let layout = self.taffy.layout(node).expect("node has computed layout");
        let this_z = *z;
        *z += 1;

        // Only an interactive widget is a focus stop. A passive node under a group
        // (a menu title, a label, a nested layout stack) must not become one, or
        // nav stops on it; its group still propagates to its children below.
        // Interactive kinds carry a required id, so `focus_meta` always yields one
        // here; no fallback id is needed.
        let stop = widget_interaction(widget).and_then(|interaction| {
            let (id, neighbors) = focus_meta(widget);
            id.map(|id| (id, neighbors, interaction))
        });
        if let Some((id, neighbors, interaction)) = stop {
            let rect = project_rect(ref_origin, layout, scale, canvas_origin);
            let rect_index = out.rects.len();
            // M13 G2: resolve the widget's a11y `selected`/`checked` predicates (if
            // any) to 0.0/1.0 and read its `disabled` bit. These ride the readback
            // as a11y metadata — the engine draws no highlight from them; the author
            // wires the visual through `styleRanges` (resolved in the draw build).
            let (selected, checked, disabled) =
                widget_a11y_state(widget, scope, slot_values, cell_values);
            out.rects.push(FocusRect {
                id: id.clone(),
                rect,
                z: this_z,
                group,
                neighbors,
                interaction: Some(interaction),
                selected,
                checked,
                disabled,
            });
            if let Some(g) = group {
                out.groups[g].members.push(rect_index);
            }
        }

        // A container declaring its own `localState` opens a scope its subtree's
        // `{ local }` predicate binds resolve against (mirrors `build_stack`).
        let child_scope = container_local_scope(widget).or(scope);

        // A container declaring a focus policy opens a new group its interactive
        // descendants join. Register the group before recursing so children carry
        // its index. Children of a non-policy container inherit the ancestor group.
        let child_group = match container_focus_policy(widget) {
            Some(policy) => {
                let idx = out.groups.len();
                out.groups.push(FocusGroup {
                    kind: policy.kind().into(),
                    wrap: policy.wrap(),
                    repeat: policy.repeat().map(Into::into),
                    members: Vec::new(),
                });
                Some(idx)
            }
            None => group,
        };

        if let Some(children) = widget_children(widget) {
            let taffy_children = self.taffy.children(node).expect("node children resolve");
            for (child_widget, child_node) in children.iter().zip(taffy_children) {
                let child_layout = self.taffy.layout(child_node).expect("child has layout");
                let child_origin = [
                    ref_origin[0] + child_layout.location.x,
                    ref_origin[1] + child_layout.location.y,
                ];
                self.collect_focus_node(
                    child_widget,
                    child_node,
                    child_group,
                    child_scope,
                    child_origin,
                    scale,
                    canvas_origin,
                    z,
                    out,
                    slot_values,
                    cell_values,
                );
            }
        }
    }
}

/// Warn about focus authoring the export ignores: an `initialFocus` or a
/// `focusNeighbors` target that names no interactive widget in the tree, and
/// `focusNeighbors` authored on a passive widget. Runs once when a named tree is
/// registered — the tree name is known there, not at `UiTree` build — so the
/// per-frame export stays log-free. Logging only: at runtime an unmatched
/// initial focus falls back to the first focusable node, and an unmatched
/// neighbor to the group policy.
pub fn warn_focus_authoring(tree_name: &str, tree: &AnchoredTree) {
    let mut interactive = HashSet::new();
    collect_interactive_ids(tree_name, &tree.root, &mut interactive);

    let initial = tree.initial_focus.as_deref();
    if let Some(initial) = initial.filter(|id| !interactive.contains(id)) {
        log::warn!(
            "[UI] tree '{tree_name}': initialFocus '{initial}' is not an interactive \
             widget in the tree; focus starts on the first focusable node instead"
        );
    }
    warn_neighbors(tree_name, &tree.root, &interactive);
}

/// Collect every interactive widget's id, warning (once per registration) about
/// an empty id or one that duplicates another interactive id already seen in
/// `tree_name`. Reads only the id (`widget_id`), not the neighbor overrides
/// `focus_meta` also computes.
fn collect_interactive_ids<'a>(tree_name: &str, widget: &'a Widget, out: &mut HashSet<&'a str>) {
    if is_interactive(widget) {
        let id = widget_id(widget).map_or("", String::as_str);
        if id.is_empty() {
            log::warn!("[UI] tree '{tree_name}': an interactive widget has an empty id");
        } else if !out.insert(id) {
            log::warn!(
                "[UI] tree '{tree_name}': interactive id '{id}' is registered more than once"
            );
        }
    }
    for child in widget_children(widget).unwrap_or_default() {
        collect_interactive_ids(tree_name, child, out);
    }
}

fn warn_neighbors(tree_name: &str, widget: &Widget, interactive: &HashSet<&str>) {
    if let Some(neighbors) = authored_focus_neighbors(widget).filter(|n| !n.is_empty()) {
        let id = widget_id(widget).map_or("<no id>", String::as_str);
        if is_interactive(widget) {
            let directions = [
                ("up", &neighbors.up),
                ("down", &neighbors.down),
                ("left", &neighbors.left),
                ("right", &neighbors.right),
            ];
            let unmatched = directions.into_iter().filter_map(|(direction, target)| {
                let target = target.as_deref()?;
                (!interactive.contains(target)).then_some((direction, target))
            });
            for (direction, target) in unmatched {
                log::warn!(
                    "[UI] tree '{tree_name}': widget '{id}' focusNeighbors.{direction} \
                     '{target}' is not an interactive widget in the tree; that direction \
                     falls back to the group policy"
                );
            }
        } else {
            log::warn!(
                "[UI] tree '{tree_name}': passive widget '{id}' authors focusNeighbors; \
                 ignored (only buttons and sliders are focus stops)"
            );
        }
    }
    for child in widget_children(widget).unwrap_or_default() {
        warn_neighbors(tree_name, child, interactive);
    }
}
