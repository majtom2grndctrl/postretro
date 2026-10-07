// Scroll containers (`scroll: { maxHeight }` on VStack/Grid): the retained
// viewport offsets, the per-frame clamp / wheel / scroll-into-view update, and
// the offset lookup the draw and focus walks share.
// See: context/lib/ui.md §4 (interaction / focus)

use taffy::prelude::{Display, NodeId, TaffyTree};

use super::super::UiWheelScroll;
use super::super::descriptor::Widget;
use super::draw::{intersect_rects, project_rect};
use super::node_context::NodeContext;
use super::widget_meta::{is_interactive, widget_children, widget_id};

/// Logical-reference pixels one wheel line scrolls a container.
pub const WHEEL_LINE_SCROLL: f32 = 40.0;

/// One frame's scroll inputs for a retained tree. Only the top (active) tree
/// receives them; a lower layer gets the default and holds its offsets.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScrollInput<'a> {
    /// The focused node id the focus engine resolved last frame. When it lies
    /// outside a scroll viewport, the viewport scrolls by the minimum distance.
    pub focused_id: Option<&'a str>,
    /// The pointer wheel this frame, when a capturing tree consumed it.
    pub wheel: Option<UiWheelScroll>,
}

/// One scroll container's retained presentation state. The offset is in
/// logical-reference pixels from the content's top; it never writes back to a
/// slot and resets when the tree is rebuilt.
#[derive(Debug, Clone, PartialEq)]
pub struct ScrollState {
    pub(super) node: NodeId,
    pub(super) offset: f32,
    /// Largest offset that keeps the content's end inside the viewport,
    /// recomputed with each layout.
    max_offset: f32,
}

/// The scroll offset applied to `node`'s children, when `node` scrolls.
pub(super) fn scroll_offset(states: &[ScrollState], node: NodeId) -> Option<f32> {
    states.iter().find(|s| s.node == node).map(|s| s.offset)
}

/// The `maxHeight` of a container that scrolls. An `HStack` never does: its
/// `scroll` is ignored with a registration-time diagnostic.
pub(super) fn scroll_max_height(widget: &Widget) -> Option<f32> {
    match widget {
        Widget::VStack(container) => container.scroll.map(|s| s.max_height),
        Widget::Grid(grid) => grid.scroll.map(|s| s.max_height),
        _ => None,
    }
}

/// Every scroll container in a tree plus the interactive stops inside them,
/// harvested once at build in lockstep with the taffy tree.
#[derive(Debug, Default)]
pub(super) struct ScrollViews {
    pub(super) states: Vec<ScrollState>,
    /// `(authored id, node)` of each interactive widget under a scroll
    /// container: the only stops scroll-into-view can move.
    targets: Vec<(String, NodeId)>,
    /// The focused id the last update saw. A change, or a relayout, brings the
    /// focused stop into view; a settled frame only compares.
    last_focus: Option<String>,
}

/// Device placement of a laid-out tree: the root's reference origin, the
/// reference→device scale, and the letterboxed canvas origin.
#[derive(Debug, Clone, Copy)]
pub(super) struct Placement {
    pub(super) root_origin: [f32; 2],
    pub(super) scale: f32,
    pub(super) canvas_origin: [f32; 2],
}

impl ScrollViews {
    pub(super) fn harvest(taffy: &TaffyTree<NodeContext>, widget: &Widget, node: NodeId) -> Self {
        let mut views = Self::default();
        views.harvest_into(taffy, widget, node, false);
        views
    }

    fn harvest_into(
        &mut self,
        taffy: &TaffyTree<NodeContext>,
        widget: &Widget,
        node: NodeId,
        inside: bool,
    ) {
        if inside
            && is_interactive(widget)
            && let Some(id) = widget_id(widget)
        {
            self.targets.push((id.clone(), node));
        }
        let scrolls = scroll_max_height(widget).is_some();
        if scrolls {
            self.states.push(ScrollState {
                node,
                offset: 0.0,
                max_offset: 0.0,
            });
        }
        if let Some(children) = widget_children(widget) {
            let taffy_children = taffy.children(node).expect("node children resolve");
            for (child_widget, child_node) in children.iter().zip(taffy_children) {
                self.harvest_into(taffy, child_widget, child_node, inside || scrolls);
            }
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    /// Apply this frame's scroll work after layout. Returns whether any offset
    /// moved (the draw list and focus export must rebuild).
    ///
    /// Order: a relayout re-clamps every offset (content that shrank draws from
    /// a clamped offset, never past its end — P18); a changed focus, or a
    /// relayout, brings the focused stop into view by the minimum distance; the
    /// wheel then scrolls the container under the cursor.
    pub(super) fn update(
        &mut self,
        taffy: &TaffyTree<NodeContext>,
        placement: Placement,
        relaid: bool,
        input: ScrollInput<'_>,
    ) -> bool {
        if self.states.is_empty() {
            return false;
        }
        let mut moved = false;
        if relaid {
            for state in &mut self.states {
                state.max_offset = max_offset(taffy, state.node);
                moved |= set_offset(state, state.offset);
            }
        }
        let focus_changed = self.last_focus.as_deref() != input.focused_id;
        if focus_changed {
            self.last_focus = input.focused_id.map(str::to_string);
        }
        if (focus_changed || relaid)
            && let Some(focused) = input.focused_id
            && let Some(node) = self
                .targets
                .iter()
                .find(|(id, _)| id == focused)
                .map(|(_, node)| *node)
        {
            moved |= self.scroll_into_view(taffy, node);
        }
        if let Some(wheel) = input.wheel {
            moved |= self.apply_wheel(taffy, placement, wheel);
        }
        moved
    }

    /// Scroll every scroll ancestor of `node`, innermost first, by the minimum
    /// distance that shows it: a node below the viewport lands with its bottom
    /// on the viewport's bottom, one above with its top on the viewport's top.
    fn scroll_into_view(&mut self, taffy: &TaffyTree<NodeContext>, node: NodeId) -> bool {
        if taffy.style(node).expect("node has a style").display == Display::None {
            return false;
        }
        let height = taffy.layout(node).expect("node has layout").size.height;
        let mut moved = false;
        let mut ancestor = taffy.parent(node);
        while let Some(container) = ancestor {
            if let Some(index) = self.states.iter().position(|s| s.node == container) {
                // Both origins already include every offset, so `top` is where
                // the node draws relative to this viewport's top edge.
                let top = self.origin(taffy, node)[1] - self.origin(taffy, container)[1];
                let viewport = taffy
                    .layout(container)
                    .expect("node has layout")
                    .size
                    .height;
                let state = &mut self.states[index];
                if top < 0.0 {
                    moved |= set_offset(state, state.offset + top);
                } else if top + height > viewport {
                    moved |= set_offset(state, state.offset + top + height - viewport);
                }
            }
            ancestor = taffy.parent(container);
        }
        moved
    }

    /// Scroll the innermost overflowing container whose visible viewport holds
    /// the cursor.
    fn apply_wheel(
        &mut self,
        taffy: &TaffyTree<NodeContext>,
        placement: Placement,
        wheel: UiWheelScroll,
    ) -> bool {
        let [px, py] = wheel.position;
        let mut target: Option<(usize, usize)> = None;
        for (index, state) in self.states.iter().enumerate() {
            if state.max_offset <= 0.0 {
                continue;
            }
            let Some(viewport) = self.visible_viewport(taffy, placement, state.node) else {
                continue;
            };
            let inside = px >= viewport[0]
                && px < viewport[0] + viewport[2]
                && py >= viewport[1]
                && py < viewport[1] + viewport[3];
            if !inside {
                continue;
            }
            let depth = depth_of(taffy, state.node);
            if target.is_none_or(|(_, best)| depth > best) {
                target = Some((index, depth));
            }
        }
        let Some((index, _)) = target else {
            return false;
        };
        let delta = wheel.lines * WHEEL_LINE_SCROLL + wheel.pixels / placement.scale.max(f32::EPSILON);
        let state = &mut self.states[index];
        set_offset(state, state.offset - delta)
    }

    /// A container's device viewport cut by every scrolling ancestor's.
    fn visible_viewport(
        &self,
        taffy: &TaffyTree<NodeContext>,
        placement: Placement,
        node: NodeId,
    ) -> Option<[f32; 4]> {
        let mut rect = self.device_rect(taffy, placement, node);
        let mut ancestor = taffy.parent(node);
        while let Some(container) = ancestor {
            if self.states.iter().any(|s| s.node == container) {
                rect = intersect_rects(rect, self.device_rect(taffy, placement, container));
            }
            ancestor = taffy.parent(container);
        }
        (rect[2] > 0.0 && rect[3] > 0.0).then_some(rect)
    }

    fn device_rect(
        &self,
        taffy: &TaffyTree<NodeContext>,
        placement: Placement,
        node: NodeId,
    ) -> [f32; 4] {
        let origin = self.origin(taffy, node);
        let reference = [
            placement.root_origin[0] + origin[0],
            placement.root_origin[1] + origin[1],
        ];
        project_rect(
            reference,
            taffy.layout(node).expect("node has layout"),
            placement.scale,
            placement.canvas_origin,
        )
    }

    /// `node`'s top-left relative to the root, with every scrolling ancestor's
    /// offset applied: where the draw walk places it.
    fn origin(&self, taffy: &TaffyTree<NodeContext>, node: NodeId) -> [f32; 2] {
        let mut origin = [0.0, 0.0];
        let mut current = node;
        while let Some(parent) = taffy.parent(current) {
            let location = taffy.layout(current).expect("node has layout").location;
            origin[0] += location.x;
            origin[1] += location.y;
            if let Some(offset) = scroll_offset(&self.states, parent) {
                origin[1] -= offset;
            }
            current = parent;
        }
        origin
    }
}

/// Clamp `target` into the state's range and store it. Returns whether the
/// offset moved.
fn set_offset(state: &mut ScrollState, target: f32) -> bool {
    let clamped = target.clamp(0.0, state.max_offset.max(0.0));
    if clamped == state.offset {
        return false;
    }
    state.offset = clamped;
    true
}

/// How far a container's content reaches past its viewport: the lowest
/// visible child's bottom plus the bottom padding, less the viewport height.
fn max_offset(taffy: &TaffyTree<NodeContext>, node: NodeId) -> f32 {
    let layout = taffy.layout(node).expect("node has layout");
    let mut content_bottom: f32 = 0.0;
    for child in taffy.children(node).expect("node children resolve") {
        if taffy.style(child).expect("node has a style").display == Display::None {
            continue;
        }
        let child_layout = taffy.layout(child).expect("child has layout");
        content_bottom = content_bottom.max(child_layout.location.y + child_layout.size.height);
    }
    (content_bottom + layout.padding.bottom - layout.size.height).max(0.0)
}

fn depth_of(taffy: &TaffyTree<NodeContext>, node: NodeId) -> usize {
    let mut depth = 0;
    let mut current = node;
    while let Some(parent) = taffy.parent(current) {
        depth += 1;
        current = parent;
    }
    depth
}
