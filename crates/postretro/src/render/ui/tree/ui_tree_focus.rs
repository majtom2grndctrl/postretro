// Focus / hit-test rect export for the retained `UiTree`: the lockstep
// descriptor+taffy walk that pairs each focusable node with its device-pixel rect.
// See: context/lib/ui.md §4 (interaction / focus)

use std::collections::HashMap;

use taffy::prelude::NodeId;

use super::super::descriptor::{AnchoredTree, Widget};
use super::super::layout::REFERENCE_HEIGHT;
use super::super::layout::REFERENCE_WIDTH;
use crate::scripting::slot_table::SlotValue;

use super::CellValues;
use super::draw::{
    FocusGroup, FocusRect, FocusRectList, anchor_fractions, canvas_origin, project_rect,
};
use super::ui_tree::UiTree;
use super::widget_meta::{
    any_restore_on_return, container_focus_policy, container_local_scope, focus_meta,
    widget_a11y_state, widget_children, widget_interaction,
};

impl UiTree {
    /// Export the flat hit-test / focus rect list for this tree against the
    /// descriptor it was built from. Walks the descriptor tree and the taffy tree
    /// in lockstep (they are structurally 1:1 — `build_node` maps each widget to
    /// exactly one node, children in order) so each focusable node's authored or
    /// auto-generated id pairs with its computed device-pixel rect.
    ///
    /// Uses the SAME device-pixel projection as the draw (`project_rect`,
    /// `canvas_origin`, `device_scale`) so a hit lands on exactly the rect drawn.
    /// Assumes layout is already computed for `device_size` (the caller's gate ran
    /// the compute). Pure read-back — no taffy mutation, no GPU.
    ///
    /// A node is exported as focusable when it carries an authored `id` OR sits
    /// (directly) under a container that declares a focus policy. The auto-id is
    /// the node's path from the root (`"0/2/1"`), regenerated deterministically
    /// each build — so it is stable across rebuilds for an unchanged structure but
    /// is runtime-only and never serialized. Authored ids carry across structural
    /// rebuilds (focus restore relies on them).
    pub(crate) fn export_focus_rects(
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
            String::new(),
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

    /// Lockstep descriptor+taffy walk for `export_focus_rects`. `path` is the
    /// node's slash-joined child-index path from the root (the auto-id when no id
    /// is authored). `group` is the index (into `out.groups`) of the nearest
    /// ancestor container that declared a focus policy. `z` rises in tree order so
    /// a later-drawn node hit-tests as topmost.
    #[allow(clippy::too_many_arguments)]
    fn collect_focus_node(
        &self,
        widget: &Widget,
        node: NodeId,
        path: String,
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

        let (authored_id, neighbors) = focus_meta(widget);
        // A node is focusable when it carries an authored id or is governed by an
        // ancestor focus group. Auto-id falls back to the tree path.
        let focusable = authored_id.is_some() || group.is_some();
        let id = authored_id.cloned().unwrap_or_else(|| {
            if path.is_empty() {
                "root".to_string()
            } else {
                path.clone()
            }
        });
        if focusable {
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
                interaction: widget_interaction(widget),
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

        // A container declaring a focus policy opens a new group its DIRECT
        // children join. Register the group before recursing so children carry its
        // index. Children of a non-policy container inherit the ancestor group.
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
            for (i, (child_widget, child_node)) in children.iter().zip(taffy_children).enumerate() {
                let child_layout = self.taffy.layout(child_node).expect("child has layout");
                let child_origin = [
                    ref_origin[0] + child_layout.location.x,
                    ref_origin[1] + child_layout.location.y,
                ];
                let child_path = if path.is_empty() {
                    i.to_string()
                } else {
                    format!("{path}/{i}")
                };
                self.collect_focus_node(
                    child_widget,
                    child_node,
                    child_path,
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
