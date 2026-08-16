// Passive presentation-template layout: one widget subtree becomes an
// anchor-relative draw list without a modal tree, focus state, or input surface.
// See: context/lib/ui.md §1 (presentation layer), §3 (display-value tweens)

use std::collections::HashMap;

use cosmic_text::FontSystem;
use postretro_entities::{PresentationFact, PresentationFacts, SlotValue};
use taffy::prelude::{AvailableSpace, Display, NodeId, Size, TaffyTree};

use super::bindings::{
    BindingDiff, drive_bar_binding, drive_bar_max, drive_panel_binding, drive_text_binding,
};
use super::build::build_node;
use super::draw::{UiDrawData, bar_max_value, bar_slot_value};
use super::node_context::{NodeContext, VisibilityState};
use super::predicate::PRESENTATION_FACT_SCOPE;
use super::predicate::resolve_predicate;
use super::ui_tree_collect::collect_draw_data_from_layout_into;
use super::widget_meta::{harvest_image_nodes, harvest_visibility, measure_node};
use super::{CellValues, ImageSizes};
use crate::descriptor::Widget;
use crate::layout::{REFERENCE_HEIGHT, REFERENCE_WIDTH};
use crate::theme::UiTheme;

/// Per-instance layout state for one passive presentation template.
///
/// This deliberately does not use [`super::UiTree`]: it has no anchor envelope,
/// retained gameplay-tree identity, modal/input state, focus export, or hit-test
/// data. It keeps only the taffy nodes plus display-value and visibility state
/// needed to reuse a fixed template layout while producer-stamped facts change
/// or visibility transitions occur.
pub struct PresentationTemplateLayout {
    taffy: TaffyTree<NodeContext>,
    root: NodeId,
    node_ids: Vec<NodeId>,
    visibility: HashMap<NodeId, VisibilityState>,
    image_nodes: Vec<NodeId>,
    dirty_text: Vec<NodeId>,
    visibility_flips: Vec<(NodeId, Display)>,
    capture_bar_exit: Vec<NodeId>,
    last_viewport: Option<[u32; 2]>,
    last_image_sizes_generation: Option<u64>,
    #[cfg(any(test, feature = "test-fixtures"))]
    recompute_count: u32,
}

impl PresentationTemplateLayout {
    /// Build renderer-local layout state for one live template instance. Theme
    /// tokens and the fixed node traversal resolve once; subsequent fact updates
    /// reuse bounded per-instance storage.
    pub fn from_widget(root_widget: &Widget, theme: &UiTheme) -> Self {
        let mut taffy = TaffyTree::new();
        let root = build_node(&mut taffy, root_widget, theme, None);

        let mut node_ids = Vec::new();
        collect_node_ids(&taffy, root, &mut node_ids);

        let mut visibility = HashMap::new();
        harvest_visibility(&taffy, root_widget, root, None, &mut visibility);

        let mut image_nodes = Vec::new();
        harvest_image_nodes(&taffy, root_widget, root, &mut image_nodes);

        Self {
            taffy,
            root,
            dirty_text: Vec::with_capacity(node_ids.len()),
            visibility_flips: Vec::with_capacity(visibility.len()),
            capture_bar_exit: Vec::with_capacity(visibility.len()),
            node_ids,
            visibility,
            image_nodes,
            last_viewport: None,
            last_image_sizes_generation: None,
            #[cfg(any(test, feature = "test-fixtures"))]
            recompute_count: 0,
        }
    }

    /// Update the reusable renderer-local snapshot from producer-stamped facts.
    /// Direct `{ fact }` binds use a reserved scope, so root and nested widgets do
    /// not need retained `localState` merely to read instance data.
    pub fn update_fact_cell_values(facts: &PresentationFacts, cells: &mut CellValues) {
        cells.retain(|(scope, name), _| {
            scope != PRESENTATION_FACT_SCOPE || facts.contains_key(name)
        });
        for (name, fact) in facts {
            if let Some(value) = cells.iter_mut().find_map(|((scope, cell_name), value)| {
                (scope == PRESENTATION_FACT_SCOPE && cell_name == name).then_some(value)
            }) {
                update_fact_value(value, fact);
            } else {
                cells.insert(
                    (PRESENTATION_FACT_SCOPE.to_string(), name.clone()),
                    fact_slot_value(fact),
                );
            }
        }
    }

    /// Resolve this frame's facts, update any local display tween, and lower the
    /// template into device-pixel data relative to `[0, 0]`. The renderer adds the
    /// projected world anchor afterwards, keeping layout independent of camera
    /// placement. Only text-content changes, image-size availability, or a
    /// viewport change re-run taffy; fixed-size bars merely rebuild their fill.
    #[allow(clippy::too_many_arguments)]
    pub fn build_draw_data(
        &mut self,
        device_size: [u32; 2],
        font_system: &mut FontSystem,
        image_sizes: &ImageSizes,
        image_sizes_generation: u64,
        cell_values: &CellValues,
        time_seconds: f64,
    ) -> UiDrawData {
        let mut draw = UiDrawData::default();
        self.build_draw_data_into(
            device_size,
            font_system,
            image_sizes,
            image_sizes_generation,
            cell_values,
            time_seconds,
            &mut draw,
        );
        draw
    }

    /// Reusable-storage form of [`Self::build_draw_data`]. Renderer-owned live
    /// layouts keep this buffer beside tween state, eliminating the per-instance
    /// temporary draw allocation on warm frames.
    #[allow(clippy::too_many_arguments)]
    pub fn build_draw_data_into(
        &mut self,
        device_size: [u32; 2],
        font_system: &mut FontSystem,
        image_sizes: &ImageSizes,
        image_sizes_generation: u64,
        cell_values: &CellValues,
        time_seconds: f64,
        draw: &mut UiDrawData,
    ) {
        // Presentation templates are facts-only. Keeping the global slot map
        // physically absent from this entry point prevents a future caller from
        // accidentally making a transient re-read live game state.
        let slot_values = HashMap::new();
        let BindingDiff {
            content_changed,
            appearance_changed: _,
        } = self.resolve_facts(&slot_values, cell_values, time_seconds);

        let viewport_changed = self.last_viewport != Some(device_size);
        let image_sizes_changed = self.last_image_sizes_generation != Some(image_sizes_generation);
        if image_sizes_changed {
            self.mark_image_nodes_dirty();
        }
        let layout_changed = content_changed
            || image_sizes_changed
            || self
                .taffy
                .dirty(self.root)
                .expect("presentation root exists in its own layout");
        if viewport_changed || layout_changed {
            self.taffy
                .compute_layout_with_measure(
                    self.root,
                    Size {
                        width: AvailableSpace::Definite(REFERENCE_WIDTH),
                        height: AvailableSpace::Definite(REFERENCE_HEIGHT),
                    },
                    |known_dimensions, _available_space, _node_id, node_context, _style| {
                        measure_node(known_dimensions, node_context, font_system, image_sizes)
                    },
                )
                .expect("taffy layout must succeed for a well-formed presentation template");
            self.last_viewport = Some(device_size);
            self.last_image_sizes_generation = Some(image_sizes_generation);
            #[cfg(any(test, feature = "test-fixtures"))]
            {
                self.recompute_count += 1;
            }
        }

        collect_draw_data_from_layout_into(
            &self.taffy,
            self.root,
            [0.0, 0.0],
            crate::layout::device_scale(device_size),
            [0.0, 0.0],
            &slot_values,
            cell_values,
            time_seconds,
            &self.visibility,
            draw,
        );
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn recompute_count(&self) -> u32 {
        self.recompute_count
    }

    fn mark_image_nodes_dirty(&mut self) {
        for &node in &self.image_nodes {
            self.taffy
                .mark_dirty(node)
                .expect("presentation node exists in its own layout");
        }
    }

    fn resolve_facts(
        &mut self,
        slot_values: &HashMap<String, SlotValue>,
        cell_values: &CellValues,
        time_seconds: f64,
    ) -> BindingDiff {
        let mut diff = BindingDiff::default();
        self.dirty_text.clear();
        for node in self.node_ids.iter().copied() {
            match self.taffy.get_node_context_mut(node) {
                Some(NodeContext::Text {
                    content,
                    bind_scope,
                    bind: Some(bind),
                    last_resolved,
                    tween,
                    ..
                }) => {
                    if drive_text_binding(
                        bind,
                        bind_scope.as_deref(),
                        content,
                        last_resolved,
                        tween,
                        slot_values,
                        cell_values,
                        time_seconds,
                    ) {
                        diff.content_changed = true;
                        self.dirty_text.push(node);
                    }
                }
                Some(NodeContext::Panel {
                    fill,
                    bind_scope,
                    bind: Some(bind),
                    last_resolved,
                    tween,
                    ..
                }) => {
                    if drive_panel_binding(
                        bind,
                        bind_scope.as_deref(),
                        *fill,
                        last_resolved,
                        tween,
                        slot_values,
                        cell_values,
                        time_seconds,
                    ) {
                        diff.appearance_changed = true;
                    }
                }
                Some(NodeContext::Bar {
                    bind_scope,
                    bind,
                    max,
                    last_resolved,
                    last_max_resolved,
                    tween,
                    ..
                }) => {
                    let value_changed = drive_bar_binding(
                        bind,
                        bind_scope.as_deref(),
                        last_resolved,
                        tween,
                        slot_values,
                        cell_values,
                        time_seconds,
                    );
                    let max_changed = drive_bar_max(max, last_max_resolved, slot_values);
                    if value_changed || max_changed {
                        diff.appearance_changed = true;
                    }
                }
                _ => {}
            }
        }
        for node in self.dirty_text.drain(..) {
            self.taffy
                .mark_dirty(node)
                .expect("presentation node exists in its own layout");
        }

        self.visibility_flips.clear();
        self.capture_bar_exit.clear();
        for (node, state) in &mut self.visibility {
            let resolved = resolve_predicate(
                &state.predicate.source,
                state.predicate.equals.as_ref(),
                state.scope.as_deref(),
                slot_values,
                cell_values,
            );
            let is_visible = resolved >= 0.5;
            let was_visible = state.prev.map(|value| value >= 0.5);
            if is_visible {
                if let Some(exit) = state.bar_exit_fade.as_mut()
                    && exit.started_at.is_some()
                {
                    exit.clear();
                    diff.appearance_changed = true;
                }
                if was_visible != Some(true) {
                    self.visibility_flips.push((*node, state.visible_display));
                }
            } else if was_visible.is_none() {
                self.visibility_flips.push((*node, Display::None));
            } else if was_visible == Some(true) {
                if let Some(exit) = state.bar_exit_fade.as_mut() {
                    exit.started_at = Some(time_seconds);
                    self.capture_bar_exit.push(*node);
                    diff.appearance_changed = true;
                } else {
                    self.visibility_flips.push((*node, Display::None));
                }
            } else if let Some(exit) = state.bar_exit_fade.as_mut()
                && let Some(started_at) = exit.started_at
            {
                if time_seconds - started_at >= exit.duration_seconds - f64::EPSILON {
                    exit.clear();
                    self.visibility_flips.push((*node, Display::None));
                } else {
                    diff.appearance_changed = true;
                }
            }
            state.prev = Some(resolved);
        }

        for node in self.capture_bar_exit.drain(..) {
            let (value, max) = match self.taffy.get_node_context(node) {
                Some(NodeContext::Bar {
                    bind,
                    bind_scope,
                    max,
                    last_resolved,
                    last_max_resolved,
                    tween,
                    ..
                }) => {
                    let value = match (tween, last_resolved) {
                        (Some(_), Some(displayed)) => *displayed,
                        _ => bar_slot_value(bind, bind_scope.as_deref(), slot_values, cell_values),
                    };
                    let max = last_max_resolved.unwrap_or_else(|| bar_max_value(max, slot_values));
                    (value, max)
                }
                _ => continue,
            };
            let exit = self
                .visibility
                .get_mut(&node)
                .and_then(|state| state.bar_exit_fade.as_mut())
                .expect("only a Bar can request an exit fade");
            exit.captured_value = Some(value);
            exit.captured_max = Some(max);
        }
        for (node, display) in self.visibility_flips.drain(..) {
            let mut style = self
                .taffy
                .style(node)
                .expect("presentation node has a style")
                .clone();
            style.display = display;
            self.taffy
                .set_style(node, style)
                .expect("presentation node exists in its own layout");
            self.taffy
                .mark_dirty(node)
                .expect("presentation node exists in its own layout");
            diff.appearance_changed = true;
        }
        diff
    }
}

fn fact_slot_value(fact: &PresentationFact) -> SlotValue {
    match fact {
        PresentationFact::Number(value) => SlotValue::Number(*value),
        PresentationFact::Text(value) => SlotValue::String(value.clone()),
        PresentationFact::Bool(value) => SlotValue::Boolean(*value),
    }
}

fn update_fact_value(value: &mut SlotValue, fact: &PresentationFact) {
    match (value, fact) {
        (SlotValue::Number(current), PresentationFact::Number(next)) => *current = *next,
        (SlotValue::String(current), PresentationFact::Text(next)) => current.clone_from(next),
        (SlotValue::Boolean(current), PresentationFact::Bool(next)) => *current = *next,
        (current, fact) => *current = fact_slot_value(fact),
    }
}

fn collect_node_ids(taffy: &TaffyTree<NodeContext>, node: NodeId, out: &mut Vec<NodeId>) {
    out.push(node);
    for child in taffy
        .children(node)
        .expect("presentation node children resolve")
    {
        collect_node_ids(taffy, child, out);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::descriptor::{
        Align, BarMax, BarWidget, BindSource, ColorValue, ContainerWidget, ImageWidget, Predicate,
        SliderBind, SpacingValue, TextBind, TextWidget, Widget,
    };

    const EPSILON: f32 = 1.0e-3;

    fn container(children: Vec<Widget>) -> ContainerWidget {
        ContainerWidget {
            gap: SpacingValue::Literal(5.0),
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
            children,
        }
    }

    fn bound_bar(name: &str) -> Widget {
        Widget::Bar(BarWidget {
            bind: SliderBind {
                source: BindSource::Fact {
                    fact: name.to_string(),
                },
                tween: None,
            },
            max: BarMax::Literal(1.0),
            fill: ColorValue::Literal([0.0, 1.0, 0.0, 1.0]),
            background: ColorValue::Literal([0.0, 0.0, 0.0, 1.0]),
            width: Some(100.0),
            height: Some(10.0),
            id: None,
            style_ranges: None,
            visible_when: None,
            exit_fade: None,
            role: None,
        })
    }

    fn bound_text(name: &str) -> Widget {
        Widget::Text(TextWidget {
            content: "missing".to_string(),
            font_size: 20.0,
            color: ColorValue::Literal([1.0; 4]),
            id: None,
            focus_neighbors: Default::default(),
            font: None,
            bind: Some(TextBind {
                source: BindSource::Fact {
                    fact: name.to_string(),
                },
                format: None,
                tween: None,
            }),
            style_ranges: None,
            visible_when: None,
            role: None,
        })
    }

    fn facts(entries: &[(&str, PresentationFact)]) -> PresentationFacts {
        entries
            .iter()
            .map(|(name, fact)| ((*name).to_string(), fact.clone()))
            .collect::<BTreeMap<_, _>>()
    }

    fn build(
        layout: &mut PresentationTemplateLayout,
        facts: &PresentationFacts,
        font_system: &mut FontSystem,
    ) -> UiDrawData {
        let mut cells = CellValues::with_capacity(facts.len());
        PresentationTemplateLayout::update_fact_cell_values(facts, &mut cells);
        layout.build_draw_data([1280, 720], font_system, &ImageSizes::new(), 0, &cells, 0.0)
    }

    #[test]
    fn presentation_fact_cells_update_in_place_and_remove_stale_names() {
        let mut cells = CellValues::new();
        PresentationTemplateLayout::update_fact_cell_values(
            &facts(&[
                ("value", PresentationFact::Text("100".to_string())),
                ("stale", PresentationFact::Bool(true)),
            ]),
            &mut cells,
        );
        PresentationTemplateLayout::update_fact_cell_values(
            &facts(&[("value", PresentationFact::Number(42.0))]),
            &mut cells,
        );

        assert_eq!(cells.len(), 1);
        assert_eq!(
            cells.iter().find_map(|((scope, name), value)| {
                (scope == PRESENTATION_FACT_SCOPE && name == "value").then_some(value)
            }),
            Some(&SlotValue::Number(42.0))
        );
    }

    fn assert_rect_approx(actual: [f32; 4], expected: [f32; 4]) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!(
                (actual - expected).abs() < EPSILON,
                "expected {expected} ± {EPSILON}, got {actual}"
            );
        }
    }

    #[test]
    fn presentation_vstack_bars_use_instance_facts_and_anchor_translation() {
        let root = Widget::VStack(container(vec![bound_bar("health"), bound_bar("shield")]));
        let mut layout = PresentationTemplateLayout::from_widget(&root, &UiTheme::engine_default());
        let instance_facts = facts(&[
            ("health", PresentationFact::Number(0.5)),
            ("shield", PresentationFact::Number(0.25)),
        ]);
        let mut font_system = crate::text::build_font_system();
        let relative = build(&mut layout, &instance_facts, &mut font_system);
        let mut anchored = UiDrawData::default();
        anchored.append_translated(&relative, [400.0, 300.0], 1.0);

        assert_eq!(
            anchored.quads.len(),
            4,
            "two bars emit background + fill each"
        );
        assert_rect_approx(
            anchored.quads.instances[0].rect,
            [400.0, 300.0, 100.0, 10.0],
        );
        assert_rect_approx(anchored.quads.instances[1].rect, [400.0, 300.0, 50.0, 10.0]);
        assert_rect_approx(
            anchored.quads.instances[2].rect,
            [400.0, 315.0, 100.0, 10.0],
        );
        assert_rect_approx(anchored.quads.instances[3].rect, [400.0, 315.0, 25.0, 10.0]);

        let changed_facts = facts(&[
            ("health", PresentationFact::Number(0.8)),
            ("shield", PresentationFact::Number(0.4)),
        ]);
        let changed = build(&mut layout, &changed_facts, &mut font_system);
        assert_eq!(
            layout.recompute_count(),
            1,
            "fixed-size bars update without relayout"
        );
        assert!((changed.quads.instances[1].rect[2] - 80.0).abs() < EPSILON);
        assert!((changed.quads.instances[3].rect[2] - 40.0).abs() < EPSILON);
    }

    #[test]
    fn presentation_hstack_texts_resolve_facts_at_projected_anchor() {
        let root = Widget::HStack(container(vec![bound_text("left"), bound_text("right")]));
        let mut layout = PresentationTemplateLayout::from_widget(&root, &UiTheme::engine_default());
        let instance_facts = facts(&[
            ("left", PresentationFact::Text("42".to_string())),
            ("right", PresentationFact::Text("CRIT".to_string())),
        ]);
        let mut font_system = crate::text::build_font_system();
        let relative = build(&mut layout, &instance_facts, &mut font_system);
        let mut anchored = UiDrawData::default();
        anchored.append_translated(&relative, [480.0, 270.0], 1.0);

        assert_eq!(anchored.texts.len(), 2);
        assert_eq!(anchored.texts[0].content, "42");
        assert_eq!(anchored.texts[1].content, "CRIT");
        assert!((anchored.texts[0].position[0] - 480.0).abs() < EPSILON);
        assert!((anchored.texts[0].position[1] - 270.0).abs() < EPSILON);
        assert!(
            anchored.texts[1].position[0] > anchored.texts[0].position[0],
            "the second text stays horizontally flowed rather than overlapping"
        );
        assert!(
            (anchored.texts[1].position[1] - 270.0).abs() < EPSILON,
            "both row texts keep the projected anchor's y coordinate"
        );
    }

    #[test]
    fn presentation_bool_fact_drives_visibility_without_local_state() {
        let Widget::Bar(mut bar) = bound_bar("health") else {
            unreachable!();
        };
        bar.visible_when = Some(Predicate {
            source: BindSource::Fact {
                fact: "shown".to_string(),
            },
            equals: None,
        });
        let root = Widget::Bar(bar);
        let mut layout = PresentationTemplateLayout::from_widget(&root, &UiTheme::engine_default());
        let mut font_system = crate::text::build_font_system();

        let hidden = facts(&[
            ("health", PresentationFact::Number(0.5)),
            ("shown", PresentationFact::Bool(false)),
        ]);
        assert!(
            build(&mut layout, &hidden, &mut font_system)
                .quads
                .is_empty()
        );

        let shown = facts(&[
            ("health", PresentationFact::Number(0.5)),
            ("shown", PresentationFact::Bool(true)),
        ]);
        assert_eq!(build(&mut layout, &shown, &mut font_system).quads.len(), 2);
    }

    #[test]
    fn presentation_image_uses_renderer_size_and_anchor_translation() {
        let root = Widget::Image(ImageWidget {
            asset: "presentation/icon".to_string(),
            id: None,
            focus_neighbors: Default::default(),
            label: None,
            decorative: true,
            visible_when: None,
            role: None,
        });
        let mut layout = PresentationTemplateLayout::from_widget(&root, &UiTheme::engine_default());
        let mut image_sizes = ImageSizes::new();
        image_sizes.insert("presentation/icon".to_string(), [16.0, 8.0]);
        let mut font_system = crate::text::build_font_system();
        let relative = layout.build_draw_data(
            [1280, 720],
            &mut font_system,
            &image_sizes,
            1,
            &CellValues::new(),
            0.0,
        );
        let mut anchored = UiDrawData::default();
        anchored.append_translated(&relative, [320.0, 180.0], 1.0);

        assert_eq!(anchored.images.len(), 1);
        assert_eq!(anchored.images[0].0, "presentation/icon");
        assert_rect_approx(
            anchored.images[0].1.instances[0].rect,
            [320.0, 180.0, 16.0, 8.0],
        );
    }

    #[test]
    fn presentation_text_fact_remeasures_only_when_rendered_content_changes() {
        let root = Widget::VStack(container(vec![bound_text("value")]));
        let mut layout = PresentationTemplateLayout::from_widget(&root, &UiTheme::engine_default());
        let mut font_system = crate::text::build_font_system();

        let first = facts(&[("value", PresentationFact::Text("1".to_string()))]);
        let _ = build(&mut layout, &first, &mut font_system);
        assert_eq!(layout.recompute_count(), 1);

        let _ = build(&mut layout, &first, &mut font_system);
        assert_eq!(
            layout.recompute_count(),
            1,
            "unchanged fact does not relayout"
        );

        let wider = facts(&[("value", PresentationFact::Text("1000".to_string()))]);
        let output = build(&mut layout, &wider, &mut font_system);
        assert_eq!(
            layout.recompute_count(),
            2,
            "measured text fact dirties layout"
        );
        assert_eq!(output.texts[0].content, "1000");
    }
}
