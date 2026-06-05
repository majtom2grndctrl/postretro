// Retained UI widget tree: maps the serde `descriptor` model into a
// `taffy::TaffyTree`, computes flex/grid layout, and reads the laid-out rects
// back into the device-pixel `UiDrawList` + shaped-text draw entries through the
// `layout` projection path. taffy/layout lives entirely here (renderer-owns-GPU).
// See: context/plans/in-progress/M13--descriptor-tree-layout

use taffy::prelude::{
    AlignItems, AvailableSpace, Display, FlexDirection, Layout, NodeId, Size, Style, TaffyTree,
    evenly_sized_tracks, length,
};

use super::descriptor::{
    Align, AnchoredTree, Border, ContainerWidget, GridWidget, ImageWidget, PanelWidget, TextWidget,
    Widget,
};
use super::layout::{Anchor, REFERENCE_HEIGHT, REFERENCE_WIDTH};
use glyphon::FontSystem;

use super::text::{UiText, measure_run};
use super::{UiDrawList, UiInstance};

/// Per-node draw payload carried alongside each taffy node. Pure layout nodes
/// (stacks, grids, spacers) carry `None`; only nodes that emit a draw entry hold
/// data here. taffy owns the geometry; this owns "what to draw in that rect".
#[derive(Debug, Clone)]
enum NodeContext {
    /// Shaped-text run. `color` is linear RGBA from the descriptor; the draw-list
    /// build converts it to glyphon's `[u8; 4]` sRGB. Carries its own `font_size`
    /// (device-scaled at draw time) since taffy does not retain it.
    Text {
        content: String,
        font_size: f32,
        color: [f32; 4],
    },
    /// Solid-fill panel quad, optionally framed by a 9-slice `border`. `fill`
    /// stays linear `[f32; 4]` — no sRGB conversion on the quad path.
    Panel {
        fill: [f32; 4],
        border: Option<Border>,
    },
    /// Textured image quad. `asset` is the texture key the renderer binds; the
    /// rect comes from layout. Image batching/binding lands with the renderer
    /// wiring — the tree records the key so the draw step can group by it.
    Image { asset: String },
}

/// Map descriptor cross-axis `Align` to taffy `AlignItems`.
fn align_items(align: Align) -> AlignItems {
    match align {
        Align::Start => AlignItems::Start,
        Align::Center => AlignItems::Center,
        Align::End => AlignItems::End,
        Align::Stretch => AlignItems::Stretch,
    }
}

/// Container (stack/grid) shared style: scalar `padding` → all four edges,
/// `gap` → both axes, `align` → `align_items`.
fn container_base_style(gap: f32, padding: f32, align: Align) -> Style {
    Style {
        align_items: Some(align_items(align)),
        gap: Size {
            width: length(gap),
            height: length(gap),
        },
        padding: taffy::geometry::Rect {
            left: length(padding),
            right: length(padding),
            top: length(padding),
            bottom: length(padding),
        },
        ..Default::default()
    }
}

/// One retained UI tree: the taffy tree, its root node, and the placement
/// envelope's `anchor`/`offset`. One per top-level `AnchoredTree` — F's modal
/// stack wants independent trees, so the tree is owned per-descriptor.
pub(crate) struct UiTree {
    taffy: TaffyTree<NodeContext>,
    root: NodeId,
    anchor: Anchor,
    offset: [f32; 2],
    /// Device size the cached layout was computed against. `None` until the
    /// first `build_draw_data`. A change here forces a recompute even when the
    /// tree is otherwise unchanged (resize re-resolves the letterbox/scale and
    /// any device-space sizing), since taffy's dirty state only tracks the tree.
    last_viewport: Option<[u32; 2]>,
    /// Number of times `compute_layout_with_measure` actually ran. The gate
    /// skips the compute on an unchanged frame, so this stops incrementing when
    /// nothing dirtied — tests assert against it to prove the cached path.
    recompute_count: u32,
}

impl UiTree {
    /// Build the retained tree from a descriptor envelope. Recursively maps each
    /// `Widget` to a taffy node with the mapped `Style` + (for leaves/panels) a
    /// `NodeContext` draw payload.
    pub(crate) fn from_descriptor(tree: &AnchoredTree) -> Self {
        let mut taffy = TaffyTree::new();
        let root = build_node(&mut taffy, &tree.root);
        Self {
            taffy,
            root,
            anchor: tree.anchor,
            offset: tree.offset,
            // A freshly built tree has no cached layout — taffy reports the root
            // dirty, so the first `build_draw_data` recomputes. No viewport seen
            // yet, so any first device size also counts as a change.
            last_viewport: None,
            recompute_count: 0,
        }
    }

    /// How many times this tree has actually recomputed layout. The gate in
    /// `build_draw_data` only bumps this when a structural change (the tree was
    /// rebuilt, leaving taffy's root dirty) or a viewport change forces a
    /// recompute; an unchanged frame reuses the cached layout and leaves this
    /// flat. Tests read it to prove the no-change frame skipped the compute.
    #[cfg(test)]
    pub(crate) fn recompute_count(&self) -> u32 {
        self.recompute_count
    }

    /// Compute layout against the 1280x720 logical-reference canvas, then read
    /// the laid-out rects back into a device-pixel draw list + shaped-text lines.
    ///
    /// Two-stage placement: taffy lays the tree out at the canvas origin, then
    /// the root's computed content size is placed in reference space per the
    /// envelope's `anchor`/`offset`, and finally every node's reference-space
    /// rect is projected to device pixels (uniform scale + letterbox) via the
    /// `layout` projection path. Quads land in `UiDrawList`; text runs in the
    /// returned `Vec<UiText>` (device-positioned, device-scaled font size).
    ///
    /// Text nodes are sized through `font_system`: the measure closure shapes
    /// each text node's `content` at its `font_size` and returns the real
    /// shaped-run extent (logical-reference px), so layout reflects actual glyph
    /// metrics. Only the CPU `FontSystem` is threaded in (via
    /// `UiTextRenderer::font_system_mut`) — glyphon's GPU atlas/renderer stay in
    /// the renderer, and the tree holds no GPU/font state of its own.
    ///
    /// Layout recompute is gated on change: it runs only when taffy reports the
    /// root dirty (the tree was rebuilt from a new descriptor — a structural
    /// change leaves the cache empty) or when `device_size` differs from the
    /// viewport the cached layout was computed against. On an unchanged frame —
    /// same tree, same viewport — no `compute_layout_with_measure` call is made;
    /// the cached `taffy::Layout` rects are read back unchanged. The draw-list
    /// production below always runs, so the cached path still yields draw data.
    pub(crate) fn build_draw_data(
        &mut self,
        device_size: [u32; 2],
        font_system: &mut FontSystem,
    ) -> UiDrawData {
        // Gate: recompute only on a structural change (taffy's root cache is
        // empty after a rebuild) or a viewport change. taffy caches computed
        // layout internally and only recomputes dirtied subtrees; this gate
        // decides whether to call compute *at all* for the no-change frame.
        let viewport_changed = self.last_viewport != Some(device_size);
        let structural_change = self
            .taffy
            .dirty(self.root)
            .expect("root node exists in its own tree");
        if viewport_changed || structural_change {
            // Lay the tree out with the reference canvas as the available space,
            // so percentage/stretch resolve against 1280x720. taffy positions
            // the root at its own origin; the anchor/offset transform re-places
            // it after.
            //
            // `compute_layout_with_measure` gives each leaf a measure callback.
            // Text nodes shape through `font_system` and return their real glyph
            // extent; every other node returns its known/taffy-resolved size
            // unchanged. The closure borrows `font_system` mutably (cosmic-text
            // shaping needs `&mut FontSystem`); taffy hands it each node's `&mut
            // NodeContext`, so the closure never has to borrow `self.taffy` while
            // it runs.
            self.taffy
                .compute_layout_with_measure(
                    self.root,
                    Size {
                        width: AvailableSpace::Definite(REFERENCE_WIDTH),
                        height: AvailableSpace::Definite(REFERENCE_HEIGHT),
                    },
                    |known_dimensions, _available_space, _node_id, node_context, _style| {
                        measure_node(known_dimensions, node_context, font_system)
                    },
                )
                .expect("taffy layout must succeed for a well-formed UI tree");
            self.last_viewport = Some(device_size);
            self.recompute_count += 1;
        }

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

        let scale = super::layout::device_scale(device_size);
        let canvas_origin = canvas_origin(device_size, scale);

        let mut data = UiDrawData::default();
        self.collect_node(self.root, root_origin, canvas_origin, scale, &mut data);
        data
    }

    /// Walk a node and its descendants, accumulating draw entries. `ref_origin`
    /// is the node's top-left in reference space (parent origin + the node's
    /// taffy-relative location). Children recurse with their own absolute origin.
    fn collect_node(
        &self,
        node: NodeId,
        ref_origin: [f32; 2],
        canvas_origin: [f32; 2],
        scale: f32,
        data: &mut UiDrawData,
    ) {
        let layout = self.taffy.layout(node).expect("node has computed layout");
        let context = self.taffy.get_node_context(node);

        match context {
            Some(NodeContext::Panel { fill, border }) => {
                data.quads.push(project_quad(
                    ref_origin,
                    layout,
                    scale,
                    canvas_origin,
                    *fill,
                    border.as_ref(),
                ));
            }
            Some(NodeContext::Image { asset: _ }) => {
                // White-tinted image quad; the asset key drives texture binding in
                // the renderer wiring step. UV/full-texture defaults apply.
                let rect = project_rect(ref_origin, layout, scale, canvas_origin);
                data.quads.push(UiInstance::image(rect));
            }
            Some(NodeContext::Text {
                content,
                font_size,
                color,
            }) => {
                // Device-pixel top-left + device-scaled font size; color converts
                // linear RGBA -> sRGB [u8; 4] at draw-list build time.
                let rect = project_rect(ref_origin, layout, scale, canvas_origin);
                data.texts.push(UiText::new(
                    content.clone(),
                    [rect[0], rect[1]],
                    font_size * scale,
                    linear_rgba_to_srgb_u8(*color),
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
            self.collect_node(child, child_origin, canvas_origin, scale, data);
        }
    }
}

/// taffy measure callback: resolve a leaf's intrinsic size. Text nodes shape
/// their `content` at `font_size` through `font_system` and report the real
/// shaped-run extent (logical-reference px); every other node has no intrinsic
/// content to measure, so it reports the size taffy already knows
/// (`known_dimensions`, defaulting each unset axis to zero — the node sizes from
/// its style/flex slot).
fn measure_node(
    known_dimensions: Size<Option<f32>>,
    node_context: Option<&mut NodeContext>,
    font_system: &mut FontSystem,
) -> Size<f32> {
    if let Some(NodeContext::Text {
        content, font_size, ..
    }) = node_context
    {
        let (width, height) = measure_run(font_system, content, *font_size);
        // Honor any axis taffy has already pinned (e.g. an explicit/stretched
        // size); measure only the unconstrained axes.
        return Size {
            width: known_dimensions.width.unwrap_or(width),
            height: known_dimensions.height.unwrap_or(height),
        };
    }
    Size {
        width: known_dimensions.width.unwrap_or(0.0),
        height: known_dimensions.height.unwrap_or(0.0),
    }
}

/// Recursively build a taffy node (and its children) for one descriptor widget.
fn build_node(taffy: &mut TaffyTree<NodeContext>, widget: &Widget) -> NodeId {
    match widget {
        Widget::Text(TextWidget {
            content,
            font_size,
            color,
        }) => {
            // No explicit size: text nodes are sized by the measure closure in
            // `build_draw_data`, which shapes `content` at `font_size` through
            // glyphon and returns the real shaped-run extent. The `NodeContext`
            // carries the content/font_size the closure reads back.
            taffy
                .new_leaf_with_context(
                    Style::default(),
                    NodeContext::Text {
                        content: content.clone(),
                        font_size: *font_size,
                        color: *color,
                    },
                )
                .expect("taffy leaf creation must succeed")
        }
        Widget::Panel(PanelWidget { fill, border }) => {
            // A panel sizes to fill its slot (parent stretch / explicit parent
            // size). No intrinsic size of its own — it is a backing fill.
            taffy
                .new_leaf_with_context(
                    Style::default(),
                    NodeContext::Panel {
                        fill: *fill,
                        border: border.clone(),
                    },
                )
                .expect("taffy leaf creation must succeed")
        }
        Widget::Image(ImageWidget { asset }) => taffy
            .new_leaf_with_context(
                Style::default(),
                NodeContext::Image {
                    asset: asset.clone(),
                },
            )
            .expect("taffy leaf creation must succeed"),
        Widget::VStack(container) => build_stack(taffy, container, FlexDirection::Column),
        Widget::HStack(container) => build_stack(taffy, container, FlexDirection::Row),
        Widget::Grid(grid) => build_grid(taffy, grid),
        Widget::Spacer(spacer) => {
            // Flexible space: claims a proportional share of leftover space in its
            // parent container via flex_grow. No draw payload.
            let style = Style {
                flex_grow: spacer.flex_grow,
                ..Default::default()
            };
            taffy
                .new_leaf(style)
                .expect("taffy leaf creation must succeed")
        }
    }
}

/// Build a flex stack node (`vstack` → column, `hstack` → row).
fn build_stack(
    taffy: &mut TaffyTree<NodeContext>,
    container: &ContainerWidget,
    direction: FlexDirection,
) -> NodeId {
    let children: Vec<NodeId> = container
        .children
        .iter()
        .map(|child| build_node(taffy, child))
        .collect();
    let style = Style {
        display: Display::Flex,
        flex_direction: direction,
        ..container_base_style(container.gap, container.padding, container.align)
    };
    taffy
        .new_with_children(style, &children)
        .expect("taffy container creation must succeed")
}

/// Build a CSS-grid node: `cols` equal flexible tracks, `gap` both axes.
fn build_grid(taffy: &mut TaffyTree<NodeContext>, grid: &GridWidget) -> NodeId {
    let children: Vec<NodeId> = grid
        .children
        .iter()
        .map(|child| build_node(taffy, child))
        .collect();
    // `evenly_sized_tracks(N)` yields N equal `1fr` tracks — the descriptor's
    // "N equal columns" maps straight onto it.
    let cols = grid.cols.try_into().unwrap_or(u16::MAX);
    let style = Style {
        display: Display::Grid,
        grid_template_columns: evenly_sized_tracks(cols),
        ..container_base_style(grid.gap, grid.padding, grid.align)
    };
    taffy
        .new_with_children(style, &children)
        .expect("taffy grid creation must succeed")
}

/// Computed draw entries from one tree: a device-pixel quad `UiDrawList`
/// (panels/images) and device-positioned shaped-text lines. Quads draw first,
/// text composites over them — the order the UI pass records in.
#[derive(Debug, Default)]
pub(crate) struct UiDrawData {
    pub quads: UiDrawList,
    pub texts: Vec<UiText>,
}

/// Project a node's reference-space rect (origin + taffy size) to a device-pixel
/// `[x, y, w, h]`, snapping each edge to a whole device pixel. Mirrors
/// `layout::project_element`'s edge-snap so abutting nodes stay gap-free.
fn project_rect(
    ref_origin: [f32; 2],
    layout: &Layout,
    scale: f32,
    canvas_origin: [f32; 2],
) -> [f32; 4] {
    let dev_left = canvas_origin[0] + ref_origin[0] * scale;
    let dev_top = canvas_origin[1] + ref_origin[1] * scale;
    let dev_right = dev_left + layout.size.width * scale;
    let dev_bottom = dev_top + layout.size.height * scale;

    let x = dev_left.round();
    let y = dev_top.round();
    [x, y, dev_right.round() - x, dev_bottom.round() - y]
}

/// Project a panel rect into a `UiInstance`, scaling/snapping any 9-slice border
/// margin the same way `layout::project_element` does so the shader's corner
/// regions land on whole device pixels.
fn project_quad(
    ref_origin: [f32; 2],
    layout: &Layout,
    scale: f32,
    canvas_origin: [f32; 2],
    fill: [f32; 4],
    border: Option<&Border>,
) -> UiInstance {
    let rect = project_rect(ref_origin, layout, scale, canvas_origin);
    let margin = match border {
        // 9-slice insets are `[left, top, right, bottom]` logical px; scale + snap.
        Some(b) => [
            (b.slice[0] * scale).round(),
            (b.slice[1] * scale).round(),
            (b.slice[2] * scale).round(),
            (b.slice[3] * scale).round(),
        ],
        None => [0.0; 4],
    };
    UiInstance::panel(rect, fill, margin)
}

/// Fractional anchor position in `[0,1]` per axis, x right / y down — the same
/// table `layout::Anchor::fractions` exposes (private there), reused here for the
/// whole-tree placement transform.
fn anchor_fractions(anchor: Anchor) -> (f32, f32) {
    match anchor {
        Anchor::TopLeft => (0.0, 0.0),
        Anchor::Top => (0.5, 0.0),
        Anchor::TopRight => (1.0, 0.0),
        Anchor::Left => (0.0, 0.5),
        Anchor::Center => (0.5, 0.5),
        Anchor::Right => (1.0, 0.5),
        Anchor::BottomLeft => (0.0, 1.0),
        Anchor::Bottom => (0.5, 1.0),
        Anchor::BottomRight => (1.0, 1.0),
    }
}

/// Top-left of the scaled 1280x720 canvas in device pixels, centered so the
/// letterbox margin splits evenly. Same rule as `layout::canvas_origin` (private
/// there) — reused so tree-laid rects share the splash's letterbox.
fn canvas_origin(device_size: [u32; 2], scale: f32) -> [f32; 2] {
    let scaled_w = REFERENCE_WIDTH * scale;
    let scaled_h = REFERENCE_HEIGHT * scale;
    [
        (device_size[0] as f32 - scaled_w) * 0.5,
        (device_size[1] as f32 - scaled_h) * 0.5,
    ]
}

/// Convert a linear-RGBA `[f32; 4]` color to glyphon's sRGB-encoded `[u8; 4]`.
/// RGB channels go through the sRGB transfer function; alpha is linear (stays a
/// straight 0..1 → 0..255 scale). Matches the `UiText` color contract.
fn linear_rgba_to_srgb_u8(color: [f32; 4]) -> [u8; 4] {
    let encode = |c: f32| -> u8 {
        let c = c.clamp(0.0, 1.0);
        let srgb = if c <= 0.003_130_8 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        };
        (srgb * 255.0).round() as u8
    };
    [
        encode(color[0]),
        encode(color[1]),
        encode(color[2]),
        (color[3].clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Device-pixel comparison tolerance; rects snap to whole pixels but float
    /// rounding leaves sub-ulp residue.
    const EPS: f32 = 1e-3;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() <= EPS
    }

    /// A headless `FontSystem` (embedded Inter face registered, no GPU). Text
    /// nodes measure through this in `build_draw_data`, so every layout test
    /// supplies one — cosmic-text shaping runs fully on the CPU.
    fn font_system() -> glyphon::FontSystem {
        super::super::text::build_font_system()
    }

    /// A fixed-size panel leaf: panels have no intrinsic size, so give the tree
    /// an explicit size via a single-child stack with a sized panel is awkward —
    /// instead test fixtures build sized leaves directly through descriptors.
    fn panel(fill: [f32; 4]) -> Widget {
        Widget::Panel(PanelWidget { fill, border: None })
    }

    fn spacer(flex_grow: f32) -> Widget {
        Widget::Spacer(SpacerWidget { flex_grow })
    }

    fn vstack(gap: f32, padding: f32, align: Align, children: Vec<Widget>) -> Widget {
        Widget::VStack(ContainerWidget {
            gap,
            padding,
            align,
            children,
        })
    }

    fn hstack(gap: f32, padding: f32, align: Align, children: Vec<Widget>) -> Widget {
        Widget::HStack(ContainerWidget {
            gap,
            padding,
            align,
            children,
        })
    }

    use super::super::descriptor::SpacerWidget;

    /// A text leaf, for flex/grid distribution tests. Sized by the measure seam:
    /// `content` is shaped at `font_size` through glyphon, so the leaf's intrinsic
    /// size comes from real glyph metrics.
    fn text(content: &str, font_size: f32) -> Widget {
        Widget::Text(TextWidget {
            content: content.into(),
            font_size,
            color: [1.0, 1.0, 1.0, 1.0],
        })
    }

    #[test]
    fn vstack_distributes_children_along_column_with_gap() {
        // A column of two sized text leaves: the second sits directly below the
        // first, separated by exactly the container gap. Cross-axis Start keeps
        // both at x = padding. The container content-sizes to its children, so the
        // column height is `h0 + gap + h1`.
        let gap = 20.0;
        let pad = 8.0;
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            // Two single-line text leaves; each is shaped to its real glyph
            // extent by the measure seam. Exact dimensions come from Inter; the
            // test asserts only the relative column layout (gap, stacking).
            root: vstack(
                gap,
                pad,
                Align::Start,
                vec![text("AB", 40.0), text("CD", 40.0)],
            ),
        };
        let mut ui = UiTree::from_descriptor(&tree);
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs);

        let children: Vec<_> = ui.taffy.children(ui.root).unwrap();
        let c0 = *ui.taffy.layout(children[0]).unwrap();
        let c1 = *ui.taffy.layout(children[1]).unwrap();
        // Both children indent by the padding on the cross axis.
        assert!(approx(c0.location.x, pad) && approx(c1.location.x, pad));
        // First child sits at the padding top; second is one height + gap below.
        assert!(approx(c0.location.y, pad), "first child at top padding");
        assert!(
            approx(c1.location.y - (c0.location.y + c0.size.height), gap),
            "gap of {gap} between the two children (got {})",
            c1.location.y - (c0.location.y + c0.size.height),
        );
        // The column content-sizes to its children + gap + padding on both edges.
        let root = ui.taffy.layout(ui.root).unwrap();
        assert!(
            approx(
                root.size.height,
                c0.size.height + gap + c1.size.height + 2.0 * pad
            ),
            "column height is children + gap + vertical padding",
        );
        // Two text leaves produced two device-positioned text runs, no quads.
        assert_eq!(data.texts.len(), 2);
        assert!(data.quads.is_empty());
    }

    #[test]
    fn nested_hstack_in_vstack_distributes_inner_row_along_x() {
        // Outer column holds one inner row; the row lays its two sized text leaves
        // left-to-right separated by the row gap. Asserts the nested container's
        // children flow on the main (x) axis with the gap applied — the
        // vstack-of-hstack composition the task calls out.
        let gap = 12.0;
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: vstack(
                0.0,
                0.0,
                Align::Start,
                vec![hstack(
                    gap,
                    0.0,
                    Align::Start,
                    vec![text("AB", 30.0), text("CD", 30.0)],
                )],
            ),
        };
        let mut ui = UiTree::from_descriptor(&tree);
        let mut fs = font_system();
        ui.build_draw_data([1280, 720], &mut fs);

        let row = ui.taffy.children(ui.root).unwrap()[0];
        let cells: Vec<_> = ui.taffy.children(row).unwrap();
        let a = *ui.taffy.layout(cells[0]).unwrap();
        let b = *ui.taffy.layout(cells[1]).unwrap();
        // Both leaves share the row's top (same y); the second is one width + gap
        // to the right of the first.
        assert!(
            approx(a.location.y, b.location.y),
            "row children share a baseline row"
        );
        assert!(
            approx(b.location.x - a.location.x, a.size.width + gap),
            "second leaf is one width + gap right of the first (got {})",
            b.location.x - a.location.x,
        );
        // The inner row content-sizes to both leaves plus the single gap.
        let row_layout = ui.taffy.layout(row).unwrap();
        assert!(
            approx(row_layout.size.width, a.size.width + gap + b.size.width),
            "row width is both leaves + one gap",
        );
    }

    #[test]
    fn spacer_maps_to_flex_grow_and_emits_no_draw_payload() {
        // A row of `text — spacer — text`: the spacer is a pure layout node
        // (flex_grow, no `NodeContext`) that sits between the two leaves without
        // overlapping them, while the leaves still produce their text runs.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: hstack(
                0.0,
                0.0,
                Align::Start,
                vec![text("X", 40.0), spacer(1.0), text("Y", 40.0)],
            ),
        };
        let mut ui = UiTree::from_descriptor(&tree);
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs);

        let cells: Vec<_> = ui.taffy.children(ui.root).unwrap();
        let x = *ui.taffy.layout(cells[0]).unwrap();
        let s = *ui.taffy.layout(cells[1]).unwrap();
        let y = *ui.taffy.layout(cells[2]).unwrap();
        // Main-axis order is X, spacer, Y with no overlap.
        assert!(
            s.location.x >= x.location.x + x.size.width - EPS,
            "spacer after X"
        );
        assert!(
            y.location.x >= s.location.x + s.size.width - EPS,
            "Y after spacer"
        );
        // Spacer carries no draw payload; the two text leaves do.
        assert!(ui.taffy.get_node_context(cells[1]).is_none());
        assert_eq!(data.texts.len(), 2, "only the two text leaves draw");
        assert!(data.quads.is_empty());
    }

    #[test]
    fn child_rects_scale_uniformly_at_4k() {
        // The same tree at 3840x2160 (3x the reference) produces device rects 3x
        // the size and position of the 1280x720 result. Mirrors layout.rs's
        // `center_panel_scales_uniformly_at_4k`. Sized text leaves give the row a
        // non-zero extent to scale.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: hstack(
                40.0,
                0.0,
                Align::Start,
                vec![text("AAAA", 20.0), text("BBBB", 20.0)],
            ),
        };
        let mut fs = font_system();
        let mut ui_ref = UiTree::from_descriptor(&tree);
        let data_ref = ui_ref.build_draw_data([1280, 720], &mut fs);
        let mut ui_4k = UiTree::from_descriptor(&tree);
        let data_4k = ui_4k.build_draw_data([3840, 2160], &mut fs);

        assert_eq!(data_ref.texts.len(), 2);
        assert_eq!(data_4k.texts.len(), 2);
        // Each text run's device position + font size scale by exactly 3.
        for i in 0..2 {
            let p_ref = data_ref.texts[i].position;
            let p_4k = data_4k.texts[i].position;
            assert!(
                approx(p_4k[0], p_ref[0] * 3.0) && approx(p_4k[1], p_ref[1] * 3.0),
                "text {i} position scales 3x: {p_ref:?} -> {p_4k:?}",
            );
            assert!(
                approx(
                    data_4k.texts[i].font_size,
                    data_ref.texts[i].font_size * 3.0
                ),
                "text {i} font size scales 3x",
            );
        }
    }

    #[test]
    fn grid_places_children_across_equal_columns() {
        // A 2-column grid with four sized cells: cells 0/1 share row 0, cells 2/3
        // share row 1. Columns are equal width; cell 1 sits to the right of cell
        // 0 by one column width + gap.
        let cell = || {
            Widget::Text(TextWidget {
                content: "XX".into(),
                font_size: 10.0,
                color: [1.0; 4],
            })
        };
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: Widget::Grid(GridWidget {
                gap: 8.0,
                padding: 0.0,
                align: Align::Start,
                cols: 2,
                children: vec![cell(), cell(), cell(), cell()],
            }),
        };
        let mut ui = UiTree::from_descriptor(&tree);
        let mut fs = font_system();
        ui.build_draw_data([1280, 720], &mut fs);
        let cells: Vec<_> = ui.taffy.children(ui.root).unwrap();
        assert_eq!(cells.len(), 4);
        let l = |n: NodeId| {
            let lay = ui.taffy.layout(n).unwrap();
            (
                lay.location.x,
                lay.location.y,
                lay.size.width,
                lay.size.height,
            )
        };
        let (x0, y0, w0, _) = l(cells[0]);
        let (x1, y1, _, _) = l(cells[1]);
        let (x2, y2, _, _) = l(cells[2]);
        // Cells 0 and 1 are on the same row; 1 is one column + gap to the right.
        assert!(approx(y0, y1), "cells 0 and 1 share a row");
        assert!(
            approx(x1 - x0, w0 + 8.0),
            "column 1 is one track + gap right of column 0 (got {})",
            x1 - x0
        );
        // Cell 2 wraps to row 1, back at column 0's x.
        assert!(approx(x2, x0), "cell 2 wraps to column 0");
        assert!(y2 > y0, "cell 2 is on a lower row");
    }

    #[test]
    fn anchored_tree_centers_against_non_16_9_letterbox() {
        // At 1280x1440 the canvas letterboxes vertically: scale = min(1.0, 2.0) =
        // 1.0, canvas origin y = (1440 - 720)/2 = 360. A center-anchored sized
        // panel lands centered in the 1280x720 canvas, then shifted down by 360.
        let tree = AnchoredTree {
            anchor: Anchor::Center,
            offset: [0.0, 0.0],
            // A single text leaf so the root has a finite measured size to center.
            // Its size is the real shaped extent — the test derives the expected
            // centered position from that measured size, not a fixed number.
            root: Widget::Text(TextWidget {
                content: "ABCDEFGH".into(),
                font_size: 40.0,
                color: [1.0, 1.0, 1.0, 1.0],
            }),
        };
        let mut ui = UiTree::from_descriptor(&tree);
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 1440], &mut fs);
        // Read back the root's measured size and recompute the centered top-left
        // in the 1280x720 canvas, then apply the +360 vertical letterbox offset.
        // Scale is 1.0 here, so device px == reference px. `project_rect` snaps
        // the device top-left to a whole pixel, so round to match.
        let root_size = ui.taffy.layout(ui.root).unwrap().size;
        let expected_x = ((REFERENCE_WIDTH - root_size.width) / 2.0).round();
        let expected_y = ((REFERENCE_HEIGHT - root_size.height) / 2.0 + 360.0).round();
        let t = &data.texts[0];
        assert!(
            approx(t.position[0], expected_x),
            "centered x in canvas: {} != {}",
            t.position[0],
            expected_x,
        );
        assert!(
            approx(t.position[1], expected_y),
            "centered y plus vertical letterbox offset: {} != {}",
            t.position[1],
            expected_y,
        );
    }

    #[test]
    fn panel_quad_rects_snap_to_integer_device_pixels() {
        // A grid of panels at a fractional scale must produce whole-pixel rects.
        let cell = || {
            Widget::Text(TextWidget {
                content: "x".into(),
                font_size: 13.0,
                color: [0.5, 0.5, 0.5, 1.0],
            })
        };
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [3.5, 7.25],
            root: vstack(
                7.0,
                5.0,
                Align::Start,
                vec![panel([0.2, 0.4, 0.6, 1.0]), cell()],
            ),
        };
        let mut ui = UiTree::from_descriptor(&tree);
        let mut fs = font_system();
        // Fractional scale: 1281x721 -> scale ~1.00078.
        let data = ui.build_draw_data([1281, 721], &mut fs);
        assert!(!data.quads.is_empty(), "panel produced a quad");
        for q in &data.quads.instances {
            for v in q.rect {
                assert!(
                    approx(v, v.round()),
                    "quad rect component {v} not snapped to a whole device pixel",
                );
            }
        }
    }

    #[test]
    fn text_color_converts_linear_rgba_to_srgb_u8() {
        // Linear 1.0 -> sRGB 255; linear 0.0 -> 0; alpha is linear-scaled. A
        // mid-gray linear 0.5 encodes to ~188 in sRGB (not 128).
        assert_eq!(
            linear_rgba_to_srgb_u8([1.0, 0.0, 1.0, 1.0]),
            [255, 0, 255, 255]
        );
        let mid = linear_rgba_to_srgb_u8([0.5, 0.5, 0.5, 0.5]);
        assert!(
            (185..=192).contains(&mid[0]),
            "linear 0.5 encodes to ~188 sRGB, got {}",
            mid[0],
        );
        assert_eq!(mid[3], 128, "alpha stays linear (0.5 -> 128)");
    }

    /// Lay out a single text leaf and return its taffy-computed size — the size
    /// the measure seam produced from shaped glyph metrics.
    fn measured_text_size(content: &str, font_size: f32) -> taffy::geometry::Size<f32> {
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: text(content, font_size),
        };
        let mut ui = UiTree::from_descriptor(&tree);
        let mut fs = font_system();
        ui.build_draw_data([1280, 720], &mut fs);
        ui.taffy.layout(ui.root).unwrap().size
    }

    #[test]
    fn text_node_width_differs_with_content_via_shaped_measurement() {
        // Construct two trees whose text leaves differ only in content (same font
        // size). Real shaping gives them different advances, so the measure seam
        // must report different widths. Content is immutable in Goal B — this is a
        // two-tree comparison, not runtime mutation.
        let narrow = measured_text_size("i", 40.0);
        let wide = measured_text_size("WWWWWWWW", 40.0);

        assert!(
            wide.width > narrow.width + EPS,
            "eight wide glyphs must shape wider than a single narrow one ({} vs {})",
            wide.width,
            narrow.width,
        );
        // Both single-line runs report a positive line-box height.
        assert!(
            narrow.height > 0.0 && wide.height > 0.0,
            "shaped text reports a positive line height",
        );
    }

    #[test]
    fn text_node_width_tracks_proportional_glyph_advances() {
        // The glyph-count placeholder this replaced sized every glyph identically
        // (`chars * font_size * 0.5`). Real shaping is proportional: a string of
        // narrow glyphs ("ll") shapes narrower than the same count of wide glyphs
        // ("WW"). Equal width here would mean we were still counting chars.
        let narrow = measured_text_size("llll", 40.0);
        let wide = measured_text_size("WWWW", 40.0);

        assert!(
            wide.width > narrow.width + EPS,
            "four wide glyphs must shape wider than four narrow glyphs ({} vs {}) \
             — proportional advances, not a glyph count",
            wide.width,
            narrow.width,
        );
    }

    #[test]
    fn text_node_size_is_not_the_glyph_count_estimate() {
        // The replaced placeholder was exactly `chars * font_size * 0.5` wide by
        // `font_size` tall. Assert the shaped size does NOT coincide with that
        // formula, proving the size comes from glyph metrics. Inter's "MMMM" is
        // wide and the line box is `font_size * 1.25` tall, so neither axis lands
        // on the old estimate.
        let content = "MMMM";
        let font_size = 40.0;
        let size = measured_text_size(content, font_size);

        let placeholder_w = content.chars().count() as f32 * font_size * 0.5;
        let placeholder_h = font_size;
        assert!(
            (size.width - placeholder_w).abs() > 1.0,
            "shaped width {} must not match the old glyph-count estimate {}",
            size.width,
            placeholder_w,
        );
        assert!(
            (size.height - placeholder_h).abs() > 1.0,
            "shaped line-box height {} must not match the old font-size estimate {}",
            size.height,
            placeholder_h,
        );
    }

    /// A two-leaf column tree, reused by the dirty-gating tests so they all lay
    /// out the same shape.
    fn gating_tree() -> AnchoredTree {
        AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: vstack(
                10.0,
                4.0,
                Align::Start,
                vec![text("AB", 30.0), text("CD", 30.0)],
            ),
        }
    }

    #[test]
    fn unchanged_frame_reuses_cached_layout_without_recompute() {
        // First layout populates taffy's cache (count 1); a second call with the
        // same tree and same viewport hits the gate's no-change path and reuses
        // the cached subtree layout — the compute counter must stay flat.
        let mut ui = UiTree::from_descriptor(&gating_tree());
        let mut fs = font_system();

        ui.build_draw_data([1280, 720], &mut fs);
        assert_eq!(ui.recompute_count(), 1, "first layout computes once");

        ui.build_draw_data([1280, 720], &mut fs);
        assert_eq!(
            ui.recompute_count(),
            1,
            "same tree + same viewport must not recompute",
        );
    }

    #[test]
    fn viewport_change_forces_layout_recompute() {
        // A different device size re-resolves the letterbox/scale, so the gate
        // must recompute even though the tree is byte-for-byte identical.
        let mut ui = UiTree::from_descriptor(&gating_tree());
        let mut fs = font_system();

        ui.build_draw_data([1280, 720], &mut fs);
        assert_eq!(ui.recompute_count(), 1);

        ui.build_draw_data([3840, 2160], &mut fs);
        assert_eq!(
            ui.recompute_count(),
            2,
            "a changed viewport must trigger a recompute",
        );
    }

    #[test]
    fn rebuilt_tree_recomputes_from_empty_cache() {
        // Structural change = a new tree built from a (possibly new) descriptor.
        // The fresh tree's root cache is empty, so its first layout computes even
        // at the same viewport the previous tree was laid out against.
        let mut fs = font_system();

        let mut first = UiTree::from_descriptor(&gating_tree());
        first.build_draw_data([1280, 720], &mut fs);
        first.build_draw_data([1280, 720], &mut fs);
        assert_eq!(first.recompute_count(), 1, "cached after the first layout");

        // Reshape: a structurally different descriptor yields a new tree, which
        // must recompute on its first layout regardless of viewport.
        let reshaped = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: vstack(
                10.0,
                4.0,
                Align::Start,
                vec![text("AB", 30.0), text("CD", 30.0), text("EF", 30.0)],
            ),
        };
        let mut second = UiTree::from_descriptor(&reshaped);
        second.build_draw_data([1280, 720], &mut fs);
        assert_eq!(
            second.recompute_count(),
            1,
            "a rebuilt/reshaped tree recomputes on its first layout",
        );
    }

    #[test]
    fn cached_frame_draw_data_matches_recomputed_frame() {
        // The gate skips the *compute*, not the draw-list production. The cached
        // frame reads back the same taffy::Layout rects, so its draw data must be
        // identical to the freshly-computed frame's.
        let mut ui = UiTree::from_descriptor(&gating_tree());
        let mut fs = font_system();

        let computed = ui.build_draw_data([1280, 720], &mut fs);
        let cached = ui.build_draw_data([1280, 720], &mut fs);
        // Confirm the second call really took the cached path.
        assert_eq!(ui.recompute_count(), 1, "second frame did not recompute");

        assert_eq!(computed.quads.instances.len(), cached.quads.instances.len());
        assert_eq!(computed.texts.len(), cached.texts.len());
        for (a, b) in computed.texts.iter().zip(cached.texts.iter()) {
            assert!(
                approx(a.position[0], b.position[0]) && approx(a.position[1], b.position[1]),
                "cached text position {:?} differs from computed {:?}",
                b.position,
                a.position,
            );
            assert!(
                approx(a.font_size, b.font_size),
                "cached font size {} differs from computed {}",
                b.font_size,
                a.font_size,
            );
            assert_eq!(a.content, b.content, "cached text content differs");
        }
    }
}
