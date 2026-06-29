// The retained `UiTree`'s draw-list collection walk (`collect_node`): reads each
// laid-out node's `NodeContext` and emits device-pixel quads / shaped-text runs,
// applying bound values, tweens, and styleRanges. A second `impl UiTree` block.
// See: context/lib/ui.md §1 (retained tree), §3 (display vs. authoritative value)

use std::collections::HashMap;

use taffy::prelude::NodeId;

use super::super::UiInstance;
use super::super::layout::{REFERENCE_HEIGHT, REFERENCE_WIDTH};
use super::super::style_ranges::evaluate;
use super::super::text::UiText;
use super::super::theme::UiTheme;
use postretro_entities::SlotValue;

use super::CellValues;
use super::bindings::DrawWalkCtx;
use super::draw::{
    UiDrawData, anchor_fractions, bar_max_value, bar_slot_value, canvas_origin,
    linear_rgba_to_srgb_u8, project_quad, project_rect, resolve_panel_fill, resolve_text,
    style_text_value, style_value,
};
use super::node_context::NodeContext;
use super::predicate::resolve_predicate;
use super::ui_tree::UiTree;

impl UiTree {
    /// Read the cached taffy layout back into a fresh `UiDrawData`, resolving any
    /// bound text/panel nodes against the live `slot_values`. Pure read-back — it
    /// assumes layout is already computed for `device_size` (the caller's gate
    /// ran the compute when needed). Shared by the fresh path (`build_draw_data`,
    /// which calls it every frame) and the retained path (which calls it only
    /// when the draw list needs rebuilding). `time_seconds` is the frame's
    /// dt-accumulated clock the styleRange pulse/flash effects advance against.
    pub(super) fn collect_draw_data(
        &self,
        device_size: [u32; 2],
        slot_values: &HashMap<String, SlotValue>,
        cell_values: &CellValues,
        time_seconds: f64,
    ) -> UiDrawData {
        // Place the root in reference space: anchor it on the canvas, then back
        // the root's top-left out by the anchor fraction of the root's size (the
        // anchor is both the canvas reference point and the root's pivot). This
        // mirrors `layout::project_element`'s pivot math, but applied ONCE to the
        // whole tree, with taffy-relative child positions added underneath.
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

        // styleRange band colors were pre-resolved to literals at build time, so
        // the draw-time evaluator never looks a token up; this inert theme satisfies
        // its `&UiTheme` parameter without
        // re-introducing the theme to the per-frame walk.
        let inert_theme = UiTheme::engine_default();

        let walk = DrawWalkCtx {
            canvas_origin,
            scale,
            slot_values,
            cell_values,
            time_seconds,
            inert_theme: &inert_theme,
        };

        let mut data = UiDrawData::default();
        self.collect_node(self.root, root_origin, &walk, &mut data);
        data
    }

    pub(super) fn collect_node(
        &self,
        node: NodeId,
        ref_origin: [f32; 2],
        walk: &DrawWalkCtx<'_>,
        data: &mut UiDrawData,
    ) {
        // Reactive visibility (M13 G2, Task 2b): a `Display::None` node (a false
        // `visibleWhen`) and its entire subtree draw nothing — skip the whole
        // subtree so zero quads/glyphs are emitted. The node stays in the taffy
        // tree (just hidden), so this only affects the draw, not the structure.
        if self.is_display_none(node) {
            return;
        }
        let DrawWalkCtx {
            canvas_origin,
            scale,
            slot_values,
            cell_values,
            time_seconds,
            inert_theme,
        } = *walk;
        let layout = self.taffy.layout(node).expect("node has computed layout");
        let context = self.taffy.get_node_context(node);

        match context {
            Some(NodeContext::Panel {
                fill,
                border,
                bind_scope,
                bind,
                last_resolved,
                tween,
                style_ranges,
                style_state,
            }) => {
                // A bound panel resolves its fill from the slot snapshot; an
                // absent/malformed slot falls back to the literal `fill`. For a
                // TWEENED bind whose driver has produced an eased display fill
                // (`tween` is `Some` and `last_resolved` holds it), render that
                // eased fill instead of re-resolving the raw slot — so the
                // per-channel easing reaches the draw. The fresh/splash path never
                // populates `tween`, so it resolves the target directly (inert).
                let mut fill = match (tween, last_resolved) {
                    (Some(_), Some(eased)) => *eased,
                    _ => resolve_panel_fill(
                        bind.as_ref(),
                        bind_scope.as_deref(),
                        *fill,
                        slot_values,
                        cell_values,
                    ),
                };
                // styleRanges overrides the fill: the bound numeric
                // value maps to a band color + pulse/flash. Its band colors were
                // pre-resolved to literals at build, so the evaluator's theme arg
                // is inert here. The base color is the resolved `fill` above (a
                // band with no color keeps it).
                if let Some(ranges) = style_ranges {
                    if let Some(value) = style_value(
                        bind.as_ref(),
                        bind_scope.as_deref(),
                        slot_values,
                        cell_values,
                    ) {
                        fill = evaluate(
                            ranges,
                            value,
                            fill,
                            inert_theme,
                            &mut style_state.borrow_mut(),
                            time_seconds,
                        );
                    }
                }
                data.quads.push(project_quad(
                    ref_origin,
                    layout,
                    scale,
                    canvas_origin,
                    fill,
                    border.as_ref(),
                ));
            }
            Some(NodeContext::Image { asset }) => {
                // White-tinted image quad grouped by its `asset` key so the
                // renderer can bind the matching texture for that group. UV/full-
                // texture defaults apply. Quads for the same key concatenate into
                // one batch; the renderer resolves the key→bind-group at encode.
                let rect = project_rect(ref_origin, layout, scale, canvas_origin);
                data.image_quad_for(asset).push(UiInstance::image(rect));
            }
            Some(NodeContext::Bar {
                bind_scope,
                bind,
                max,
                fill,
                background,
                last_resolved,
                last_max_resolved: _,
                tween,
                style_ranges,
                style_state,
            }) => {
                // Background quad fills the whole laid-out rect.
                let rect = project_rect(ref_origin, layout, scale, canvas_origin);
                data.quads
                    .push(UiInstance::panel(rect, *background, [0.0; 4]));

                // The displayed value: the eased tween display when active (the
                // styleRanges/fill-fraction contract reads the value the widget
                // RENDERS, which mid-tween is the display value), else the raw slot
                // `Number`. The fresh/splash path never tweens, so it reads the slot.
                let value = match (tween, last_resolved) {
                    (Some(_), Some(displayed)) => *displayed,
                    _ => bar_slot_value(bind, bind_scope.as_deref(), slot_values, cell_values),
                };
                let max_value = bar_max_value(max, slot_values);
                let fraction = if max_value > 0.0 {
                    (value / max_value).clamp(0.0, 1.0)
                } else {
                    0.0
                };

                // styleRanges recolors the fill from the normalized displayed
                // fraction. Text/panel pass their displayed scalar directly; a
                // bar's rendered scalar is its fill fraction.
                let mut fill_color = *fill;
                if let Some(ranges) = style_ranges {
                    fill_color = evaluate(
                        ranges,
                        fraction,
                        fill_color,
                        inert_theme,
                        &mut style_state.borrow_mut(),
                        time_seconds,
                    );
                }

                // Fill quad: same top-left/height, width scaled by the fraction.
                // Snap to whole device pixels like the background rect.
                let fill_width = (rect[2] * fraction).round();
                if fill_width > 0.0 {
                    let fill_rect = [rect[0], rect[1], fill_width, rect[3]];
                    data.quads
                        .push(UiInstance::panel(fill_rect, fill_color, [0.0; 4]));
                }
            }
            Some(NodeContext::Text {
                content,
                font_size,
                color,
                family,
                bind_scope,
                bind,
                last_resolved,
                tween,
                style_ranges,
                style_state,
                predicate_bind,
                predicate_scope,
            }) => {
                // A bound text node resolves its drawn string from the slot
                // snapshot (through the optional `{}` format template); an absent
                // slot falls back to the literal `content`. Layout already used
                // the literal `content` (or the resolved/displayed string in
                // `last_resolved`) for measurement (see `measure_node`), so
                // resolution only swaps the rendered string, never the geometry.
                //
                // For a TWEENED bind whose driver has produced a displayed value
                // (`tween` is `Some`, with the rounded/formatted display string in
                // `last_resolved`), render that string so the eased value reaches
                // the draw and matches what the measure seam shaped. The
                // fresh/splash path never populates `tween`, so it resolves the
                // target directly (inert).
                let resolved = match (tween, last_resolved) {
                    (Some(_), Some(displayed)) => displayed.clone(),
                    _ => resolve_text(
                        bind.as_ref(),
                        bind_scope.as_deref(),
                        content,
                        slot_values,
                        cell_values,
                    ),
                };
                // styleRanges overrides the run's color: the bound
                // value (the eased tween display when a tween is active, else the
                // raw slot number) maps to a band color + pulse/flash. Band colors
                // were pre-resolved to literals at build, so the theme arg is inert.
                let color = match style_ranges {
                    Some(ranges) => {
                        // A button's `bind` Predicate (M13 G2) is the styleRanges
                        // value source when present: resolve it to 0.0/1.0 (the
                        // author-wired self-highlight). An ordinary text node has no
                        // predicate, so it reads the bound numeric slot via
                        // `style_text_value` (the eased tween display when active).
                        let value = match predicate_bind {
                            Some(p) => Some(resolve_predicate(
                                &p.source,
                                p.equals.as_ref(),
                                predicate_scope.as_deref(),
                                slot_values,
                                cell_values,
                            )),
                            None => style_text_value(
                                bind.as_ref(),
                                bind_scope.as_deref(),
                                tween.as_ref(),
                                slot_values,
                                cell_values,
                            ),
                        };
                        match value {
                            Some(value) => evaluate(
                                ranges,
                                value,
                                *color,
                                inert_theme,
                                &mut style_state.borrow_mut(),
                                time_seconds,
                            ),
                            None => *color,
                        }
                    }
                    None => *color,
                };
                // Device-pixel top-left + device-scaled font size; color converts
                // linear RGBA -> sRGB [u8; 4] at draw-list build time. The run is
                // laid out in flow (its container's `align` centers it on the
                // measured run width), so no per-node centering shift is applied.
                let rect = project_rect(ref_origin, layout, scale, canvas_origin);
                data.texts.push(UiText::new(
                    resolved,
                    [rect[0], rect[1]],
                    font_size * scale,
                    linear_rgba_to_srgb_u8(color),
                    // The theme-resolved family carried on the node (from the
                    // widget's `font` token, or `primary` when it names none), so the
                    // drawn line shapes against the same registered face the
                    // measure seam sized it with.
                    family.clone(),
                ));
            }
            None => {}
        }

        // Recurse into children: each child's reference origin is this node's
        // reference origin plus the child's taffy-relative location.
        for child in self.taffy.children(node).expect("node children resolve") {
            let child_layout = self.taffy.layout(child).expect("child has layout");
            let child_origin = [
                ref_origin[0] + child_layout.location.x,
                ref_origin[1] + child_layout.location.y,
            ];
            self.collect_node(child, child_origin, walk, data);
        }
    }
}
