// Retained UI widget tree: maps the serde `descriptor` model into a
// `taffy::TaffyTree`, computes flex/grid layout, and reads the laid-out rects
// back into the device-pixel `UiDrawList` + shaped-text draw entries through the
// `layout` projection path. taffy/layout lives entirely here (renderer-owns-GPU).

use std::collections::HashMap;

use taffy::prelude::{
    AlignItems, AvailableSpace, Display, FlexDirection, Layout, NodeId, Size, Style, TaffyTree,
    evenly_sized_tracks, length,
};

use super::descriptor::{
    Align, AnchoredTree, Border, ColorValue, ContainerWidget, GridWidget, ImageWidget, PanelBind,
    PanelWidget, SpacingValue, TextBind, TextWidget, Widget,
};
use super::layout::{Anchor, REFERENCE_HEIGHT, REFERENCE_WIDTH};
use super::theme::UiTheme;
use crate::scripting::slot_table::SlotValue;
use glyphon::FontSystem;

use super::text::{UiText, measure_run};
use super::{UiDrawList, UiInstance};

/// Fallback color for an unknown color token: opaque magenta. A missing token
/// degrades visibly (rather than panicking or rendering invisibly) so an
/// authoring typo is obvious on screen — see the M13 fonts+theming spec.
const UNKNOWN_COLOR_FALLBACK: [f32; 4] = [1.0, 0.0, 1.0, 1.0];

/// Fallback spacing for an unknown spacing token: zero logical px.
const UNKNOWN_SPACING_FALLBACK: f32 = 0.0;

/// Resolve a `ColorValue` against the active theme. A `Literal` is its own RGBA;
/// a `Token` looks the name up in the theme. An unknown token degrades to opaque
/// magenta and logs exactly one warning (per tree build, not per frame — this
/// runs at build time, which on the retained path is once per rebuild).
fn resolve_color(value: &ColorValue, theme: &UiTheme) -> [f32; 4] {
    match value {
        ColorValue::Literal(rgba) => *rgba,
        ColorValue::Token(name) => theme.color(name).unwrap_or_else(|| {
            log::warn!(
                "[UI] unknown color token '{name}' — using opaque magenta fallback"
            );
            UNKNOWN_COLOR_FALLBACK
        }),
    }
}

/// Resolve a `SpacingValue` against the active theme. A `Literal` is its own px;
/// a `Token` looks the name up. An unknown token degrades to `0.0` and logs
/// exactly one warning per tree build.
fn resolve_spacing(value: &SpacingValue, theme: &UiTheme) -> f32 {
    match value {
        SpacingValue::Literal(px) => *px,
        SpacingValue::Token(name) => theme.spacing(name).unwrap_or_else(|| {
            log::warn!("[UI] unknown spacing token '{name}' — using 0.0 fallback");
            UNKNOWN_SPACING_FALLBACK
        }),
    }
}

/// Resolve a `text` widget's optional `font` token to a concrete family string.
/// `None` selects the `body` token's family; `Some(name)` looks the token up. An
/// unknown font token degrades to the `body` family and logs exactly one warning
/// per tree build. The `body` token is a required theme token (it always
/// resolves on the engine default), so the unwrap-to-body path never recurses
/// into a second miss; a theme that somehow lacks `body` falls back to the
/// embedded body family constant rather than panicking.
fn resolve_font(font: &Option<String>, theme: &UiTheme) -> String {
    let body = || {
        theme
            .font("body")
            .unwrap_or(super::text::UI_FONT_FAMILY)
            .to_string()
    };
    match font {
        None => body(),
        Some(name) => match theme.font(name) {
            Some(family) => family.to_string(),
            None => {
                log::warn!("[UI] unknown font token '{name}' — using body family fallback");
                body()
            }
        },
    }
}

/// Resolve a `Border`'s theme-tokened `tint` against the active theme into a
/// concrete-RGBA `Border`. `None` passes through (no border). The `texture` and
/// `slice` are wire literals carried unchanged; only the `tint` color slot
/// resolves (a `Token` against the theme, an unknown token degrading to opaque
/// magenta + one warn via `resolve_color`).
fn resolve_border(border: Option<&Border>, theme: &UiTheme) -> Option<Border> {
    border.map(|b| Border {
        texture: b.texture.clone(),
        slice: b.slice,
        tint: ColorValue::Literal(resolve_color(&b.tint, theme)),
    })
}

/// Asset key → natural reference size (logical-reference px, `[width, height]`)
/// for `image` nodes. Threaded into the measure seam so an image sizes from its
/// real asset dimensions (content-driven, like text) rather than a wire-level
/// fixed size. The renderer builds this from the uploaded texture's pixel dims.
pub(crate) type ImageSizes = HashMap<String, [f32; 2]>;

/// Per-node draw payload carried alongside each taffy node. Pure layout nodes
/// (stacks, grids, spacers) carry `None`; only nodes that emit a draw entry hold
/// data here. taffy owns the geometry; this owns "what to draw in that rect".
#[derive(Debug, Clone)]
enum NodeContext {
    /// Shaped-text run. `color` is linear RGBA from the descriptor; the draw-list
    /// build converts it to glyphon's `[u8; 4]` sRGB. Carries its own `font_size`
    /// (device-scaled at draw time) since taffy does not retain it.
    ///
    /// `bind` carries the optional state-binding: when `Some`, `content` is the
    /// literal fallback and the drawn string is resolved from the frame's slot
    /// values. On a retained tree the per-frame diff resolves the binding BEFORE
    /// layout and stores the resolved string in `last_resolved`; the measure seam
    /// then shapes that resolved string (falling back to the literal `content`
    /// when nothing is resolved yet), so a content change re-measures
    /// (layout-affecting). `content` itself is never overwritten — it stays the
    /// immutable fallback so an absent slot always resolves back to the literal.
    /// `last_resolved` caches the string the diff last saw, so the diff only
    /// re-measures/relays when the resolved string actually changes.
    Text {
        content: String,
        font_size: f32,
        color: [f32; 4],
        /// Theme-resolved font family this run shapes and draws with. Sourced
        /// from the `text` widget's `font` token (or the `body` token when the
        /// widget names none) at tree-build time, so the measure seam and the
        /// draw step both select the same registered face. See `resolve_font`.
        family: String,
        bind: Option<TextBind>,
        /// Last resolved bound string the diff observed. `None` until the first
        /// diff resolves the binding; only meaningful when `bind` is `Some`.
        /// Unbound nodes never set it. The measure seam shapes this string when
        /// present (so a content change re-measures), else the literal `content`.
        last_resolved: Option<String>,
    },
    /// Solid-fill panel quad, optionally framed by a 9-slice `border`. `fill`
    /// stays linear `[f32; 4]` — no sRGB conversion on the quad path. Carried by
    /// `panel` leaf nodes AND by container nodes that declare a backdrop (the
    /// container's `fill`/`border`); a container draws its backdrop quad beneath
    /// its children in painter's order (see `collect_node`).
    ///
    /// `bind` carries the optional state-binding: when `Some`, `fill` is the
    /// fallback and the drawn color is resolved from the frame's slot values at
    /// `collect_node` time. Container backdrops never bind, so they carry `None`.
    /// A bound fill is appearance-only: a change refreshes the draw list but never
    /// relays out. `last_resolved` caches the color the diff last saw so it can
    /// detect that change without re-measuring.
    Panel {
        fill: [f32; 4],
        border: Option<Border>,
        bind: Option<PanelBind>,
        /// Last resolved bound fill the diff observed. `None` until the first
        /// diff; only meaningful when `bind` is `Some`.
        last_resolved: Option<[f32; 4]>,
    },
    /// Textured image quad. `asset` is the texture key the renderer binds; the
    /// rect comes from layout. The image sizes from the asset's natural reference
    /// dimensions via the measure seam (see `measure_node`) — content-driven, so
    /// `asset` doubles as the size key. Image batching/binding lands in the
    /// renderer; the tree records the key so the draw step can group by it.
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
/// envelope's `anchor`/`offset`. One per top-level `AnchoredTree` — a future
/// modal-stack goal will want independent trees per layer, so the tree is owned
/// per-descriptor rather than shared.
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
    /// The draw list produced by the last retained build, cached so a true
    /// no-change frame (no relayout, no bound value changed, no viewport change)
    /// returns it without re-walking the tree. `None` until the first retained
    /// build. The fresh/splash path (`build_draw_data`) never reads or fills it —
    /// it always rebuilds. See `build_draw_data_retained`.
    cached_draw_data: Option<UiDrawData>,
    /// Number of times the retained path actually rebuilt the draw list (walked
    /// `collect_node`) rather than returning the cached one. Tests assert against
    /// it to prove a settled frame performs NO draw-list rebuild.
    #[cfg(test)]
    draw_rebuild_count: u32,
}

impl UiTree {
    /// Build the retained tree from a descriptor envelope. Recursively maps each
    /// `Widget` to a taffy node with the mapped `Style`, plus a `NodeContext` draw
    /// payload on drawing nodes (text/panel/image leaves, and containers carrying
    /// a backdrop `fill`/`border`).
    ///
    /// Every theme token (color/spacing/font slot) is resolved against `theme` at
    /// build time into its concrete value carried on the node, so the per-frame
    /// layout/draw walk never touches the theme. An unknown token degrades visibly
    /// and logs exactly one warning per build (see `resolve_color`/`resolve_spacing`
    /// /`resolve_font`); the resolution happens once here, not per frame.
    pub(crate) fn from_descriptor(tree: &AnchoredTree, theme: &UiTheme) -> Self {
        let mut taffy = TaffyTree::new();
        let root = build_node(&mut taffy, &tree.root, theme);
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
            cached_draw_data: None,
            #[cfg(test)]
            draw_rebuild_count: 0,
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

    /// How many times the retained path rebuilt the draw list. A settled frame
    /// returns the cached list without re-walking, so this stays flat — tests
    /// read it to prove the no-change frame skipped the draw-list rebuild.
    #[cfg(test)]
    pub(crate) fn draw_rebuild_count(&self) -> u32 {
        self.draw_rebuild_count
    }

    /// Mark `node` dirty so the next layout gate recomputes it. taffy only
    /// exposes a dirty *query* (`dirty`) on this tree today; the retained diff
    /// needs to *force* a re-measure when a bound text node's resolved content
    /// changes (its measured extent may differ), so this wraps taffy's
    /// `mark_dirty`. taffy propagates the dirty flag up to the root, so the
    /// gate's `taffy.dirty(root)` check observes it (verified by the retained
    /// content-change test).
    fn mark_dirty(&mut self, node: NodeId) {
        self.taffy
            .mark_dirty(node)
            .expect("node exists in its own tree");
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
    /// Image nodes are sized through `image_sizes`: the measure closure looks up
    /// each image's `asset` key and returns its natural reference size — the same
    /// content-driven path as text (size from the real asset, not a wire-level
    /// number). An unknown key measures to zero (the image collapses), so the
    /// renderer must pre-register every key the descriptor references.
    ///
    /// Layout recompute is gated on change: it runs only when taffy reports the
    /// root dirty (the tree was rebuilt from a new descriptor — a structural
    /// change leaves the cache empty) or when `device_size` differs from the
    /// viewport the cached layout was computed against. On an unchanged frame —
    /// same tree, same viewport — no `compute_layout_with_measure` call is made;
    /// the cached `taffy::Layout` rects are read back unchanged. Draw-list
    /// production (via `collect_draw_data`) always runs after the layout gate.
    ///
    /// This is the splash/fresh path: a fresh `UiTree` is always dirty, so the
    /// gate never short-circuits here. The gameplay path uses
    /// `build_draw_data_retained`, which retains the tree across frames and
    /// benefits from the gate.
    ///
    /// `slot_values` is the frame's resolved state-store read snapshot (cloned
    /// out of the live `SlotTable`, keyed by dotted slot name). Bound text/panel
    /// nodes resolve their drawn string/color against it at `collect_node` time;
    /// an absent slot falls back to the literal descriptor value. Layout never
    /// depends on it — only the drawn payload does — so binding never re-triggers
    /// a recompute.
    pub(crate) fn build_draw_data(
        &mut self,
        device_size: [u32; 2],
        font_system: &mut FontSystem,
        image_sizes: &ImageSizes,
        slot_values: &HashMap<String, SlotValue>,
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
                        measure_node(known_dimensions, node_context, font_system, image_sizes)
                    },
                )
                .expect("taffy layout must succeed for a well-formed UI tree");
            self.last_viewport = Some(device_size);
            self.recompute_count += 1;
        }

        self.collect_draw_data(device_size, slot_values)
    }

    /// Read the cached taffy layout back into a fresh `UiDrawData`, resolving any
    /// bound text/panel nodes against the live `slot_values`. Pure read-back — it
    /// assumes layout is already computed for `device_size` (the caller's gate
    /// ran the compute when needed). Shared by the fresh path (`build_draw_data`,
    /// which calls it every frame) and the retained path (which calls it only
    /// when the draw list needs rebuilding).
    fn collect_draw_data(
        &self,
        device_size: [u32; 2],
        slot_values: &HashMap<String, SlotValue>,
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

        let scale = super::layout::device_scale(device_size);
        let canvas_origin = canvas_origin(device_size, scale);

        let mut data = UiDrawData::default();
        self.collect_node(
            self.root,
            root_origin,
            canvas_origin,
            scale,
            slot_values,
            &mut data,
        );
        data
    }

    /// Retained-tree build: the across-frames optimization. Runs the
    /// subscriber-aware bound-value diff BEFORE the gate, then splits layout
    /// recompute from draw-list rebuild so each only runs when its inputs change.
    ///
    /// The diff (`resolve_bindings`) walks ONLY bound nodes and classifies each
    /// changed binding:
    /// - **bound text content changed** → layout-affecting: the resolved string
    ///   is stored and the node is marked dirty, forcing a relayout (the shaped
    ///   extent may differ) — `recompute_count` increments.
    /// - **bound panel fill changed** → appearance-only: the draw list rebuilds
    ///   but layout does NOT — `recompute_count` stays flat.
    ///
    /// A slot with no binding in the tree never compares, so it invalidates
    /// nothing (no rebuild, no relayout).
    ///
    /// Layout recompute is gated on `viewport_changed || taffy.dirty(root)`
    /// (the latter set by a structural rebuild or the diff's `mark_dirty`).
    /// Draw-list rebuild runs on `layout recomputed || any bound value changed
    /// || viewport changed`; otherwise the cached `UiDrawData` is cloned and
    /// returned, so a settled frame walks nothing.
    pub(crate) fn build_draw_data_retained(
        &mut self,
        device_size: [u32; 2],
        font_system: &mut FontSystem,
        image_sizes: &ImageSizes,
        slot_values: &HashMap<String, SlotValue>,
    ) -> UiDrawData {
        // Subscriber-aware diff: resolve bound nodes against the new snapshot,
        // marking content-changed text nodes dirty for relayout and flagging any
        // appearance change. Runs before the gate so its `mark_dirty` is visible
        // to `taffy.dirty(root)` below.
        let BindingDiff {
            content_changed,
            appearance_changed,
        } = self.resolve_bindings(slot_values);

        let viewport_changed = self.last_viewport != Some(device_size);
        // taffy reports the root dirty after a structural rebuild OR after the
        // diff marked a content-changed text node dirty (taffy propagates the
        // flag to the root). `content_changed` is OR-ed in as a belt-and-braces
        // guard in case dirty propagation ever fails to reach the root.
        let structural_or_content = content_changed
            || self
                .taffy
                .dirty(self.root)
                .expect("root node exists in its own tree");

        if viewport_changed || structural_or_content {
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
                .expect("taffy layout must succeed for a well-formed UI tree");
            self.last_viewport = Some(device_size);
            self.recompute_count += 1;
        }

        // Draw-list rebuild gate: rebuild when layout changed, when any bound
        // value (content or appearance) changed, when the viewport changed, or
        // when there is no cached list yet (first retained frame). Otherwise
        // return the cached list — a true no-change frame walks nothing.
        let layout_recomputed = viewport_changed || structural_or_content;
        let needs_rebuild = layout_recomputed
            || appearance_changed
            || content_changed
            || self.cached_draw_data.is_none();

        if needs_rebuild {
            let data = self.collect_draw_data(device_size, slot_values);
            #[cfg(test)]
            {
                self.draw_rebuild_count += 1;
            }
            self.cached_draw_data = Some(data.clone());
            data
        } else {
            self.cached_draw_data
                .clone()
                .expect("cache populated when not rebuilding")
        }
    }

    /// Depth-first collect every node id under `node` (inclusive) into `out`.
    /// taffy 0.10 has no whole-tree id iterator, so the diff walks the parent→
    /// children graph from the root to enumerate nodes to resolve.
    fn collect_node_ids(&self, node: NodeId, out: &mut Vec<NodeId>) {
        out.push(node);
        for child in self.taffy.children(node).expect("node children resolve") {
            self.collect_node_ids(child, out);
        }
    }

    /// Subscriber-aware bound-value diff. Walks every node, resolves the bound
    /// ones against `slot_values`, and reports whether any layout-affecting
    /// (text content) or appearance-only (panel fill) binding changed since the
    /// last diff. Unbound nodes and slots without a binding are never compared.
    /// Side effects: stores each text node's freshly resolved string in
    /// `last_resolved` and marks it dirty when it changed (so the measure seam
    /// reshapes it); records each panel's resolved fill in `last_resolved`.
    fn resolve_bindings(&mut self, slot_values: &HashMap<String, SlotValue>) -> BindingDiff {
        // Collect node ids first (depth-first from the root) to avoid borrowing
        // the taffy tree while mutating node contexts / marking dirty in the loop.
        let mut nodes: Vec<NodeId> = Vec::new();
        self.collect_node_ids(self.root, &mut nodes);
        let mut diff = BindingDiff::default();
        for node in nodes {
            // Resolve against an immutable borrow, then drop it before any
            // mutation (mark_dirty / set_node_context) on the same tree.
            let resolution = match self.taffy.get_node_context(node) {
                Some(NodeContext::Text {
                    content,
                    bind: Some(bind),
                    last_resolved,
                    ..
                }) => {
                    let resolved = resolve_text(Some(bind), content, slot_values);
                    let changed = last_resolved.as_deref() != Some(resolved.as_str());
                    Some(Resolution::Text { resolved, changed })
                }
                Some(NodeContext::Panel {
                    fill,
                    bind: Some(bind),
                    last_resolved,
                    ..
                }) => {
                    let resolved = resolve_panel_fill(Some(bind), *fill, slot_values);
                    let changed = last_resolved.is_none_or(|prev| !colors_eq(prev, resolved));
                    Some(Resolution::Panel { resolved, changed })
                }
                _ => None,
            };

            match resolution {
                Some(Resolution::Text { resolved, changed }) => {
                    if changed {
                        diff.content_changed = true;
                        if let Some(NodeContext::Text { last_resolved, .. }) =
                            self.taffy.get_node_context_mut(node)
                        {
                            *last_resolved = Some(resolved);
                        }
                        // Content change may re-measure: force a relayout.
                        self.mark_dirty(node);
                    }
                }
                Some(Resolution::Panel { resolved, changed }) => {
                    if changed {
                        diff.appearance_changed = true;
                        if let Some(NodeContext::Panel { last_resolved, .. }) =
                            self.taffy.get_node_context_mut(node)
                        {
                            *last_resolved = Some(resolved);
                        }
                        // Appearance-only: no mark_dirty, no relayout.
                    }
                }
                None => {}
            }
        }
        diff
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
        slot_values: &HashMap<String, SlotValue>,
        data: &mut UiDrawData,
    ) {
        let layout = self.taffy.layout(node).expect("node has computed layout");
        let context = self.taffy.get_node_context(node);

        match context {
            Some(NodeContext::Panel {
                fill, border, bind, ..
            }) => {
                // A bound panel resolves its fill from the slot snapshot; an
                // absent/malformed slot falls back to the literal `fill`.
                let fill = resolve_panel_fill(bind.as_ref(), *fill, slot_values);
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
            Some(NodeContext::Text {
                content,
                font_size,
                color,
                family,
                bind,
                ..
            }) => {
                // A bound text node resolves its drawn string from the slot
                // snapshot (through the optional `{}` format template); an absent
                // slot falls back to the literal `content`. Layout already used
                // the literal `content` for measurement (see `measure_node`), so
                // resolution only swaps the rendered string, never the geometry.
                let resolved = resolve_text(bind.as_ref(), content, slot_values);
                // Device-pixel top-left + device-scaled font size; color converts
                // linear RGBA -> sRGB [u8; 4] at draw-list build time. The run is
                // laid out in flow (its container's `align` centers it on the
                // measured run width), so no per-node centering shift is applied.
                let rect = project_rect(ref_origin, layout, scale, canvas_origin);
                data.texts.push(UiText::new(
                    resolved,
                    [rect[0], rect[1]],
                    font_size * scale,
                    linear_rgba_to_srgb_u8(*color),
                    // The theme-resolved family carried on the node (from the
                    // widget's `font` token, or `body` when it names none), so the
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
            self.collect_node(child, child_origin, canvas_origin, scale, slot_values, data);
        }
    }
}

/// Result of one retained-frame bound-value diff. Each flag is set when at least
/// one bound node of that class changed since the previous diff. `content_changed`
/// is layout-affecting (forces a relayout); `appearance_changed` is appearance-
/// only (forces a draw-list rebuild but never a relayout).
#[derive(Default)]
struct BindingDiff {
    content_changed: bool,
    appearance_changed: bool,
}

/// One bound node's freshly resolved value plus whether it differs from the
/// node's last-resolved value. Carried out of the immutable resolution borrow so
/// the mutable write-back/mark-dirty happens after the borrow ends.
enum Resolution {
    Text { resolved: String, changed: bool },
    Panel { resolved: [f32; 4], changed: bool },
}

/// Exact per-channel equality for a resolved fill. The diff compares the resolved
/// color against the last-resolved one to decide whether the appearance changed;
/// both sides come from the same resolution path (slot array or literal fallback),
/// so bit-identical values compare equal and the flash settling to a constant
/// color stops re-flagging.
fn colors_eq(a: [f32; 4], b: [f32; 4]) -> bool {
    a == b
}

/// taffy measure callback: resolve a leaf's intrinsic size from its content.
/// Text nodes shape their `content` at `font_size` through `font_system` and
/// report the real shaped-run extent; image nodes report their asset's natural
/// reference size from `image_sizes` (both content-driven — size from the real
/// asset/glyphs, not a wire-level number). Every other node has no intrinsic
/// content, so it reports the size taffy already knows (`known_dimensions`,
/// defaulting each unset axis to zero — the node sizes from its style/flex slot).
fn measure_node(
    known_dimensions: Size<Option<f32>>,
    node_context: Option<&mut NodeContext>,
    font_system: &mut FontSystem,
    image_sizes: &ImageSizes,
) -> Size<f32> {
    match node_context {
        Some(NodeContext::Text {
            content,
            font_size,
            family,
            last_resolved,
            ..
        }) => {
            // Measure the live bound string when the retained diff has resolved
            // one (so a content change re-measures correctly); otherwise the
            // literal `content` — the fresh/splash path never resolves, so it
            // always measures the literal, unchanged from before.
            let measured = last_resolved.as_deref().unwrap_or(content);
            // Shape against the node's theme-resolved family so a node measures
            // against the same face it draws with (a monospace run sizes
            // differently from the proportional body face).
            let (width, height) = measure_run(font_system, measured, *font_size, family);
            // Honor any axis taffy has already pinned (e.g. an explicit/stretched
            // size); measure only the unconstrained axes.
            Size {
                width: known_dimensions.width.unwrap_or(width),
                height: known_dimensions.height.unwrap_or(height),
            }
        }
        Some(NodeContext::Image { asset }) => {
            // Natural reference size keyed by asset. An unregistered key collapses
            // the image to zero (it simply does not contribute size/draw) — the
            // renderer pre-registers every key it references.
            let [w, h] = image_sizes.get(asset).copied().unwrap_or([0.0, 0.0]);
            Size {
                width: known_dimensions.width.unwrap_or(w),
                height: known_dimensions.height.unwrap_or(h),
            }
        }
        _ => Size {
            width: known_dimensions.width.unwrap_or(0.0),
            height: known_dimensions.height.unwrap_or(0.0),
        },
    }
}

/// Recursively build a taffy node (and its children) for one descriptor widget.
/// Resolves every theme token (color/spacing/font) against `theme` into the
/// concrete value the node carries, so the per-frame walk is theme-free.
fn build_node(taffy: &mut TaffyTree<NodeContext>, widget: &Widget, theme: &UiTheme) -> NodeId {
    match widget {
        Widget::Text(TextWidget {
            content,
            font_size,
            color,
            font,
            bind,
        }) => {
            // Text nodes are sized by the measure closure in `build_draw_data`,
            // which shapes `content` at `font_size` through glyphon and returns
            // the real shaped-run extent. The node carries no explicit style size.
            // `bind` rides along for draw-time resolution (layout uses `content`).
            taffy
                .new_leaf_with_context(
                    Style::default(),
                    NodeContext::Text {
                        content: content.clone(),
                        font_size: *font_size,
                        // Resolve the color token (or literal) against the theme;
                        // an unknown token degrades to opaque magenta + one warn.
                        color: resolve_color(color, theme),
                        // Resolve the optional font token to a concrete family:
                        // `None` → the `body` token, `Some(name)` → that token,
                        // unknown → `body` + one warn.
                        family: resolve_font(font, theme),
                        bind: bind.clone(),
                        last_resolved: None,
                    },
                )
                .expect("taffy leaf creation must succeed")
        }
        Widget::Panel(PanelWidget { fill, border, bind }) => {
            // A panel leaf sizes to fill its flex/grid slot (it has no intrinsic
            // size). Container backdrops are expressed on the container instead.
            // `bind` rides along for draw-time fill resolution.
            taffy
                .new_leaf_with_context(
                    Style::default(),
                    NodeContext::Panel {
                        // Resolve the fill token (or literal) against the theme.
                        fill: resolve_color(fill, theme),
                        border: resolve_border(border.as_ref(), theme),
                        bind: bind.clone(),
                        last_resolved: None,
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
        Widget::VStack(container) => build_stack(taffy, container, FlexDirection::Column, theme),
        Widget::HStack(container) => build_stack(taffy, container, FlexDirection::Row, theme),
        Widget::Grid(grid) => build_grid(taffy, grid, theme),
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

/// Optional backdrop `NodeContext` for a container declaring a `fill`/`border`.
/// `None` when the container draws no backdrop. The backdrop quad is sized to the
/// container's full laid-out rect and drawn beneath its children (painter's order
/// in `collect_node`), so a `fill`-bearing container reads as a backing panel
/// wrapping its content.
fn container_backdrop(fill: Option<[f32; 4]>, border: Option<&Border>) -> Option<NodeContext> {
    match (fill, border) {
        (None, None) => None,
        // A border with no fill still needs a fill color for the quad path; use
        // a transparent fill so only the 9-slice rim shows. (The splash always
        // pairs a fill with its border, so this is the defensive branch.)
        (fill, border) => Some(NodeContext::Panel {
            fill: fill.unwrap_or([0.0; 4]),
            border: border.cloned(),
            // Container backdrops never bind — only `panel` leaves carry a bind.
            bind: None,
            last_resolved: None,
        }),
    }
}

/// Build a flex stack node (`vstack` → column, `hstack` → row). A container with
/// a `fill`/`border` carries a `NodeContext::Panel` backdrop drawn beneath its
/// children; otherwise it is a pure layout node with no draw payload.
fn build_stack(
    taffy: &mut TaffyTree<NodeContext>,
    container: &ContainerWidget,
    direction: FlexDirection,
    theme: &UiTheme,
) -> NodeId {
    let children: Vec<NodeId> = container
        .children
        .iter()
        .map(|child| build_node(taffy, child, theme))
        .collect();
    // Resolve the spacing tokens to scalar `f32` BEFORE `container_base_style` —
    // its resolved-scalar signature stays unchanged; resolution is the only seam
    // that moved (an unknown token degrades to 0.0 + one warn via `resolve_spacing`).
    let style = Style {
        display: Display::Flex,
        flex_direction: direction,
        ..container_base_style(
            resolve_spacing(&container.gap, theme),
            resolve_spacing(&container.padding, theme),
            container.align,
        )
    };
    let node = taffy
        .new_with_children(style, &children)
        .expect("taffy container creation must succeed");
    // Resolve the optional backdrop fill (token or literal) and border tint
    // against the theme into concrete values carried on the backdrop context.
    let fill = container.fill.as_ref().map(|c| resolve_color(c, theme));
    let border = resolve_border(container.border.as_ref(), theme);
    if let Some(ctx) = container_backdrop(fill, border.as_ref()) {
        taffy
            .set_node_context(node, Some(ctx))
            .expect("setting a fresh container's backdrop context must succeed");
    }
    node
}

/// Build a CSS-grid node: `cols` equal flexible tracks, `gap` both axes.
fn build_grid(taffy: &mut TaffyTree<NodeContext>, grid: &GridWidget, theme: &UiTheme) -> NodeId {
    let children: Vec<NodeId> = grid
        .children
        .iter()
        .map(|child| build_node(taffy, child, theme))
        .collect();
    // `evenly_sized_tracks(N)` yields N equal `1fr` tracks — the descriptor's
    // "N equal columns" maps straight onto it.
    let cols = grid.cols.try_into().unwrap_or(u16::MAX);
    let style = Style {
        display: Display::Grid,
        grid_template_columns: evenly_sized_tracks(cols),
        // Resolve the spacing tokens to scalar `f32` against the theme.
        ..container_base_style(
            resolve_spacing(&grid.gap, theme),
            resolve_spacing(&grid.padding, theme),
            grid.align,
        )
    };
    taffy
        .new_with_children(style, &children)
        .expect("taffy grid creation must succeed")
}

/// Computed draw entries from one tree: a device-pixel panel quad `UiDrawList`,
/// per-asset image quad lists, and device-positioned shaped-text lines. Panels
/// draw first (one batch, the pass's white texel), then each image group (one
/// batch per `asset`, its own bound texture), then text composites over them —
/// the order the UI pass records in.
///
/// Image quads are split out from panels because each `asset` key binds a
/// distinct texture: the renderer resolves the key through its image registry to
/// a bind group, so the tree groups image quads by key rather than folding them
/// into the panel list. `images` preserves first-seen key order so draw order is
/// deterministic.
#[derive(Debug, Default, Clone)]
pub(crate) struct UiDrawData {
    pub quads: UiDrawList,
    /// Image quad batches keyed by `asset`, in first-seen order. Each entry is
    /// `(asset_key, quads)`; the renderer binds the key's texture for its quads.
    pub images: Vec<(String, UiDrawList)>,
    pub texts: Vec<UiText>,
}

impl UiDrawData {
    /// `true` when this tree produced no drawable output: no panel quads, no
    /// image quads, and no text. The renderer's gameplay path early-outs the UI
    /// pass (no `begin_render_pass`) on an empty tree.
    pub(crate) fn is_empty(&self) -> bool {
        self.quads.is_empty()
            && self.texts.is_empty()
            && self.images.iter().all(|(_, list)| list.is_empty())
    }

    /// Mutable handle to the quad list for `asset`, creating an empty list in
    /// first-seen order if the key is new. Keeps all quads sharing a texture in
    /// one batch so the renderer issues one draw per bound image.
    fn image_quad_for(&mut self, asset: &str) -> &mut UiDrawList {
        if let Some(idx) = self.images.iter().position(|(k, _)| k == asset) {
            return &mut self.images[idx].1;
        }
        self.images.push((asset.to_string(), UiDrawList::new()));
        &mut self.images.last_mut().expect("just pushed").1
    }
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

/// Resolve a bound text node's drawn string from the frame's slot snapshot.
/// Unbound (`bind == None`) returns the literal `fallback`. Bound: look up
/// `bind.slot`; if the slot is absent from the snapshot, fall back to the literal
/// `fallback` (no panic, no warn — absence is the normal "slot not written this
/// frame" case). Present: format the value to a string and, if `bind.format` is
/// `Some(template)`, substitute its single `{}` with that string; with no format,
/// the value's bare string is drawn.
fn resolve_text(
    bind: Option<&TextBind>,
    fallback: &str,
    slot_values: &HashMap<String, SlotValue>,
) -> String {
    let Some(bind) = bind else {
        return fallback.to_string();
    };
    let Some(value) = slot_values.get(&bind.slot) else {
        return fallback.to_string();
    };
    let rendered = slot_value_string(value);
    match &bind.format {
        // Single-placeholder substitution; multi-value templates are out of
        // scope, so only the first `{}` is replaced.
        Some(template) => template.replacen("{}", &rendered, 1),
        None => rendered,
    }
}

/// A `SlotValue`'s natural string form for text binding. `Number` formats
/// cleanly: an integral value prints with no decimals (`42`, not `42.0`), a
/// fractional value keeps its default float form (`12.5`). `Boolean`/`String`/
/// `Enum` print their natural representation. `Array` has no text rendering (it
/// is the panel-color shape), so it formats to an empty string — a text widget
/// should not bind an array slot.
fn slot_value_string(value: &SlotValue) -> String {
    match value {
        SlotValue::Number(n) => {
            if n.fract() == 0.0 && n.is_finite() {
                format!("{}", *n as i64)
            } else {
                n.to_string()
            }
        }
        SlotValue::Boolean(b) => b.to_string(),
        SlotValue::String(s) => s.clone(),
        SlotValue::Enum(e) => e.clone(),
        SlotValue::Array(_) => String::new(),
    }
}

/// Resolve a bound panel's fill from the frame's slot snapshot. Unbound returns
/// the literal `fallback`. Bound: look up `bind.slot`; a `SlotValue::Array` of
/// exactly 4 f32 is used as the linear `[r, g, b, a]` fill. An absent slot falls
/// back silently (the normal "not written this frame" case). A present-but-
/// malformed value (wrong variant, or an array whose length is not 4) falls back
/// to the literal `fallback` with a single `log::warn!` — a per-build authoring
/// error, not per-frame spam (a fresh tree builds once per frame today, so the
/// warn fires at most once per frame for a genuinely mis-typed slot).
fn resolve_panel_fill(
    bind: Option<&PanelBind>,
    fallback: [f32; 4],
    slot_values: &HashMap<String, SlotValue>,
) -> [f32; 4] {
    let Some(bind) = bind else {
        return fallback;
    };
    match slot_values.get(&bind.slot) {
        Some(SlotValue::Array(rgba)) if rgba.len() == 4 => [rgba[0], rgba[1], rgba[2], rgba[3]],
        // Absent slot: silent fallback — the slot simply was not written this
        // frame, which is expected for an optional binding.
        None => fallback,
        // Present but the wrong shape: an authoring error worth one warn.
        Some(_) => {
            log::warn!(
                "[Renderer] panel bind slot '{}' is not a length-4 array; using literal fill",
                bind.slot,
            );
            fallback
        }
    }
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
    use super::super::descriptor::{ColorValue, SpacingValue};
    use super::*;

    /// Device-pixel comparison tolerance; rects snap to whole pixels but float
    /// rounding leaves sub-ulp residue.
    const EPS: f32 = 1e-3;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() <= EPS
    }

    /// The engine default theme — every required token resolves, so a literal
    /// descriptor's tokens resolve to themselves and these layout tests behave
    /// exactly as before theming threaded through `from_descriptor`.
    fn theme() -> UiTheme {
        UiTheme::engine_default()
    }

    /// A headless `FontSystem` (embedded Inter face registered, no GPU). Text
    /// nodes measure through this in `build_draw_data`, so every layout test
    /// supplies one — cosmic-text shaping runs fully on the CPU.
    fn font_system() -> glyphon::FontSystem {
        super::super::text::build_font_system()
    }

    /// An empty `ImageSizes` map — most layout tests carry no `image` nodes, so
    /// the measure seam never looks anything up.
    fn no_images() -> ImageSizes {
        ImageSizes::new()
    }

    /// An empty slot-value map — most layout tests have no bound widgets, so
    /// resolution always takes the literal-fallback path.
    fn no_slots() -> HashMap<String, SlotValue> {
        HashMap::new()
    }

    fn spacer(flex_grow: f32) -> Widget {
        Widget::Spacer(SpacerWidget { flex_grow })
    }

    fn vstack(gap: f32, padding: f32, align: Align, children: Vec<Widget>) -> Widget {
        Widget::VStack(ContainerWidget {
            gap: SpacingValue::Literal(gap),
            padding: SpacingValue::Literal(padding),
            align,
            fill: None,
            border: None,
            children,
        })
    }

    fn hstack(gap: f32, padding: f32, align: Align, children: Vec<Widget>) -> Widget {
        Widget::HStack(ContainerWidget {
            gap: SpacingValue::Literal(gap),
            padding: SpacingValue::Literal(padding),
            align,
            fill: None,
            border: None,
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
            color: ColorValue::Literal([1.0, 1.0, 1.0, 1.0]),
            font: None,
            bind: None,
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
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());

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
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());

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
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());

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
        let mut ui_ref = UiTree::from_descriptor(&tree, &theme());
        let data_ref = ui_ref.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
        let mut ui_4k = UiTree::from_descriptor(&tree, &theme());
        let data_4k = ui_4k.build_draw_data([3840, 2160], &mut fs, &no_images(), &no_slots());

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
                color: ColorValue::Literal([1.0; 4]),
                font: None,
                bind: None,
            })
        };
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: Widget::Grid(GridWidget {
                gap: SpacingValue::Literal(8.0),
                padding: SpacingValue::Literal(0.0),
                align: Align::Start,
                cols: 2,
                children: vec![cell(), cell(), cell(), cell()],
            }),
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
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
                color: ColorValue::Literal([1.0, 1.0, 1.0, 1.0]),
                font: None,
                bind: None,
            }),
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 1440], &mut fs, &no_images(), &no_slots());
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
    fn container_backdrop_quad_rects_snap_to_integer_device_pixels() {
        // A container with a backdrop `fill` content-sizes to its text children
        // and emits a backdrop quad; at a fractional scale that quad's rect must
        // still snap to whole device pixels. (Bare panel leaves have no intrinsic
        // size now, so the backdrop is the canonical quad-producing path.)
        let filled = Widget::VStack(ContainerWidget {
            gap: SpacingValue::Literal(7.0),
            padding: SpacingValue::Literal(5.0),
            align: Align::Start,
            fill: Some(ColorValue::Literal([0.2, 0.4, 0.6, 1.0])),
            border: None,
            children: vec![text("x", 13.0), text("y", 13.0)],
        });
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [3.5, 7.25],
            root: filled,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        // Fractional scale: 1281x721 -> scale ~1.00078.
        let data = ui.build_draw_data([1281, 721], &mut fs, &no_images(), &no_slots());
        assert!(!data.quads.is_empty(), "container backdrop produced a quad");
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
    fn container_backdrop_draws_beneath_children_sized_to_full_rect() {
        // A filled container emits ONE backdrop quad sized to its own full laid-out
        // rect, and its children draw on top (painter's order). The backdrop is the
        // first draw entry; the text children produce runs over it.
        let filled = Widget::VStack(ContainerWidget {
            gap: SpacingValue::Literal(0.0),
            padding: SpacingValue::Literal(10.0),
            align: Align::Start,
            fill: Some(ColorValue::Literal([0.1, 0.2, 0.3, 1.0])),
            border: None,
            children: vec![text("AB", 40.0)],
        });
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: filled,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());

        // Exactly one backdrop quad (the container), one text run on top.
        assert_eq!(data.quads.len(), 1, "one container backdrop quad");
        assert_eq!(data.texts.len(), 1, "one child text run drawn over it");

        // The backdrop spans the container's full rect: it covers the child run
        // (which is inset by the padding), so the quad is wider+taller than the run.
        let quad = data.quads.instances[0].rect;
        let run_top = data.texts[0].position[1];
        assert!(
            quad[1] < run_top,
            "backdrop top {} sits above the padded child run top {run_top}",
            quad[1],
        );
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
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
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
        let mut ui = UiTree::from_descriptor(&gating_tree(), &theme());
        let mut fs = font_system();

        ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
        assert_eq!(ui.recompute_count(), 1, "first layout computes once");

        ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
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
        let mut ui = UiTree::from_descriptor(&gating_tree(), &theme());
        let mut fs = font_system();

        ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
        assert_eq!(ui.recompute_count(), 1);

        ui.build_draw_data([3840, 2160], &mut fs, &no_images(), &no_slots());
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

        let mut first = UiTree::from_descriptor(&gating_tree(), &theme());
        first.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
        first.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
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
        let mut second = UiTree::from_descriptor(&reshaped, &theme());
        second.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
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
        let mut ui = UiTree::from_descriptor(&gating_tree(), &theme());
        let mut fs = font_system();

        let computed = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
        let cached = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
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

    /// A bound text leaf, fallback `content` plus a `bind` slot and optional
    /// format template.
    fn bound_text(content: &str, slot: &str, format: Option<&str>) -> Widget {
        Widget::Text(TextWidget {
            content: content.into(),
            font_size: 20.0,
            color: ColorValue::Literal([1.0, 1.0, 1.0, 1.0]),
            font: None,
            bind: Some(TextBind {
                slot: slot.into(),
                format: format.map(str::to_string),
            }),
        })
    }

    /// A bound panel leaf, fallback `fill` plus a `bind` slot. Wrapped in a
    /// stretch container so the panel leaf gets a non-zero laid-out rect (a bare
    /// panel has no intrinsic size).
    fn bound_panel_in_stack(fill: [f32; 4], slot: &str) -> Widget {
        Widget::VStack(ContainerWidget {
            gap: SpacingValue::Literal(0.0),
            padding: SpacingValue::Literal(0.0),
            align: Align::Stretch,
            fill: Some(ColorValue::Literal([0.0, 0.0, 0.0, 1.0])),
            border: None,
            children: vec![Widget::Panel(PanelWidget {
                fill: ColorValue::Literal(fill),
                border: None,
                bind: Some(PanelBind { slot: slot.into() }),
            })],
        })
    }

    #[test]
    fn bound_text_resolves_slot_value_through_format_template() {
        // A text node bound to `player.health` with a "HP {}" template renders the
        // slot's numeric value substituted into the template. The integral Number
        // 87 formats without a trailing ".0".
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: bound_text("0", "player.health", Some("HP {}")),
        };
        let mut slots = HashMap::new();
        slots.insert("player.health".to_string(), SlotValue::Number(87.0));

        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &slots);

        assert_eq!(data.texts.len(), 1);
        assert_eq!(
            data.texts[0].content, "HP 87",
            "slot resolved into template"
        );
    }

    #[test]
    fn bound_text_without_format_renders_bare_value() {
        // No template: the resolved value's bare string form is drawn. A
        // fractional Number keeps its decimals.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: bound_text("0", "player.ammo", None),
        };
        let mut slots = HashMap::new();
        slots.insert("player.ammo".to_string(), SlotValue::Number(12.5));

        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &slots);

        assert_eq!(data.texts[0].content, "12.5");
    }

    #[test]
    fn bound_text_falls_back_to_literal_when_slot_absent() {
        // The slot is not present in the snapshot (not written this frame): the
        // node renders its literal `content` fallback rather than panicking.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: bound_text("fallback", "player.health", Some("HP {}")),
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());

        assert_eq!(
            data.texts[0].content, "fallback",
            "absent slot falls back to literal content, not the template",
        );
    }

    #[test]
    fn bound_panel_resolves_color_slot_into_fill() {
        // A panel whose fill is bound to `intro.flashColor` (a length-4 linear
        // RGBA array) draws that color, overriding its literal fallback fill.
        let resolved = [0.25, 0.5, 0.75, 1.0];
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: bound_panel_in_stack([0.0, 0.0, 0.0, 1.0], "intro.flashColor"),
        };
        let mut slots = HashMap::new();
        slots.insert(
            "intro.flashColor".to_string(),
            SlotValue::Array(resolved.to_vec()),
        );

        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &slots);

        // Two quads: the container backdrop, then the bound panel leaf. Find the
        // one carrying the resolved color.
        let found = data.quads.instances.iter().any(|q| {
            q.color
                .iter()
                .zip(resolved.iter())
                .all(|(a, b)| approx(*a, *b))
        });
        assert!(found, "a panel quad carries the resolved flash color");
    }

    #[test]
    fn bound_panel_falls_back_on_malformed_array_length() {
        // A present slot of the wrong shape (a length-3 array) is malformed: the
        // panel falls back to its literal fill (and warns once — not asserted).
        let fallback = [0.9, 0.1, 0.2, 1.0];
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: bound_panel_in_stack(fallback, "intro.flashColor"),
        };
        let mut slots = HashMap::new();
        slots.insert(
            "intro.flashColor".to_string(),
            SlotValue::Array(vec![0.1, 0.2, 0.3]),
        );

        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &slots);

        let found = data.quads.instances.iter().any(|q| {
            q.color
                .iter()
                .zip(fallback.iter())
                .all(|(a, b)| approx(*a, *b))
        });
        assert!(
            found,
            "malformed-length array falls back to the literal fill"
        );
    }

    #[test]
    fn bound_panel_falls_back_when_slot_absent() {
        // No slot written: the panel draws its literal fill, silently (no warn).
        let fallback = [0.3, 0.6, 0.9, 1.0];
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: bound_panel_in_stack(fallback, "intro.flashColor"),
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());

        let found = data.quads.instances.iter().any(|q| {
            q.color
                .iter()
                .zip(fallback.iter())
                .all(|(a, b)| approx(*a, *b))
        });
        assert!(found, "absent slot falls back to the literal fill");
    }

    // --- Retained-tree diff + relayout/redraw split (Task 4) -----------------

    /// A length-4 RGBA slot map for the bound panel flash color.
    fn flash_slots(rgba: [f32; 4]) -> HashMap<String, SlotValue> {
        let mut slots = HashMap::new();
        slots.insert(
            "intro.flashColor".to_string(),
            SlotValue::Array(rgba.to_vec()),
        );
        slots
    }

    /// Find the bound panel quad's color (the inner leaf, which differs from the
    /// container backdrop's literal black). Returns the first quad whose color is
    /// not the backdrop black.
    fn flash_quad_color(data: &UiDrawData) -> Option<[f32; 4]> {
        data.quads
            .instances
            .iter()
            .map(|q| q.color)
            .find(|c| !colors_eq(*c, [0.0, 0.0, 0.0, 1.0]))
    }

    #[test]
    fn retained_panel_fill_change_rebuilds_draw_list_without_recompute() {
        // Acceptance (a): an appearance-only bound change (the panel flash color)
        // refreshes the draw list WITHOUT a taffy relayout. The first frame
        // computes once; a frame that only changes the bound fill rebuilds the
        // draw list (new color visible) but leaves `recompute_count` flat.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: bound_panel_in_stack([0.0, 0.0, 0.0, 1.0], "intro.flashColor"),
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();

        let red = [1.0, 0.0, 0.0, 1.0];
        let first =
            ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &flash_slots(red));
        assert_eq!(ui.recompute_count(), 1, "first frame computes once");
        assert!(
            flash_quad_color(&first).is_some_and(|c| colors_eq(c, red)),
            "first frame draws the red flash",
        );

        let green = [0.0, 1.0, 0.0, 1.0];
        let second =
            ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &flash_slots(green));
        assert_eq!(
            ui.recompute_count(),
            1,
            "appearance-only fill change must not relayout",
        );
        assert!(
            flash_quad_color(&second).is_some_and(|c| colors_eq(c, green)),
            "draw list reflects the new flash color",
        );
    }

    #[test]
    fn retained_bound_text_content_change_triggers_relayout() {
        // Acceptance (b): a bound text-content change (which re-measures) DOES
        // trigger a relayout — `recompute_count` increments.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: bound_text("0", "player.health", Some("HP {}")),
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();

        let mut slots = HashMap::new();
        slots.insert("player.health".to_string(), SlotValue::Number(100.0));
        let first = ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots);
        assert_eq!(ui.recompute_count(), 1, "first frame computes once");
        assert_eq!(first.texts[0].content, "HP 100");

        slots.insert("player.health".to_string(), SlotValue::Number(75.0));
        let second = ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots);
        assert_eq!(
            ui.recompute_count(),
            2,
            "a bound text-content change relays out",
        );
        assert_eq!(second.texts[0].content, "HP 75", "new content is drawn");
    }

    #[test]
    fn retained_unbound_slot_change_invalidates_nothing() {
        // Acceptance (c): the diff is subscriber-aware — a slot with no binding in
        // the tree changing value must invalidate nothing: no relayout, no
        // draw-list rebuild. The tree binds `player.health`; we change an unrelated
        // `world.kills` slot.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: bound_text("0", "player.health", Some("HP {}")),
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();

        let mut slots = HashMap::new();
        slots.insert("player.health".to_string(), SlotValue::Number(100.0));
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots);
        assert_eq!(ui.recompute_count(), 1);
        assert_eq!(
            ui.draw_rebuild_count(),
            1,
            "first frame builds the draw list"
        );

        // Change only an unbound slot; the bound `player.health` is untouched.
        slots.insert("world.kills".to_string(), SlotValue::Number(7.0));
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots);
        assert_eq!(
            ui.recompute_count(),
            1,
            "an unbound slot change must not relayout",
        );
        assert_eq!(
            ui.draw_rebuild_count(),
            1,
            "an unbound slot change must not rebuild the draw list",
        );
    }

    #[test]
    fn retained_settled_frame_skips_draw_rebuild_and_recompute() {
        // Acceptance (d): after the flash settles to a constant color, a no-change
        // frame performs NO draw-list rebuild and NO relayout — the dirty-gate
        // short-circuits and the cached list is returned.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: bound_panel_in_stack([0.0, 0.0, 0.0, 1.0], "intro.flashColor"),
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();

        let settled = [0.2, 0.4, 0.6, 1.0];
        let first =
            ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &flash_slots(settled));
        assert_eq!(ui.recompute_count(), 1);
        assert_eq!(ui.draw_rebuild_count(), 1, "first frame builds the list");

        // Same color again: nothing changed, so neither the layout nor the draw
        // list rebuild — the cached list is returned unchanged.
        let second =
            ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &flash_slots(settled));
        assert_eq!(ui.recompute_count(), 1, "settled frame does not relayout");
        assert_eq!(
            ui.draw_rebuild_count(),
            1,
            "settled frame returns the cached draw list (no rebuild)",
        );
        // The returned (cached) list still carries the settled color.
        assert!(
            flash_quad_color(&second).is_some_and(|c| colors_eq(c, settled)),
            "cached draw list still reflects the settled color",
        );
        assert_eq!(
            first.quads.instances.len(),
            second.quads.instances.len(),
            "cached list matches the first build",
        );
    }

    // --- Theme-token resolution at tree build (Task 4) -----------------------

    use super::super::theme::{ThemeDescriptor, UiTheme};
    use super::super::text::{UI_FONT_FAMILY, UI_MONO_FONT_FAMILY};
    use std::collections::HashMap as StdHashMap;

    /// A `UiText`-colored quad's sRGB-decoded approximate linear color is hard to
    /// invert exactly; instead assert on the run's color in sRGB space by encoding
    /// the EXPECTED linear value the same way `linear_rgba_to_srgb_u8` does.
    fn srgb_of(linear: [f32; 4]) -> [u8; 4] {
        linear_rgba_to_srgb_u8(linear)
    }

    /// A single text leaf carrying a color slot (token or literal) and an optional
    /// font token — the resolution-under-test inputs.
    fn themed_text(color: ColorValue, font: Option<&str>) -> AnchoredTree {
        AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: Widget::Text(TextWidget {
                content: "X".into(),
                font_size: 20.0,
                color,
                font: font.map(str::to_string),
                bind: None,
            }),
        }
    }

    #[test]
    fn text_color_token_resolves_to_theme_rgba_in_draw_list() {
        // A `color: "critical"` token resolves to the theme's `critical` RGBA; the
        // produced text run carries that color (sRGB-encoded). Proves token slots
        // resolve against the active theme at build time.
        let theme = UiTheme::engine_default();
        let critical = theme.color("critical").unwrap();
        let tree = themed_text(ColorValue::Token("critical".into()), None);
        let mut ui = UiTree::from_descriptor(&tree, &theme);
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
        assert_eq!(data.texts.len(), 1);
        assert_eq!(
            data.texts[0].color,
            srgb_of(critical),
            "token color resolved to the theme's critical RGBA",
        );
    }

    #[test]
    fn unknown_color_token_resolves_to_opaque_magenta() {
        // An unknown color token degrades to opaque magenta [1,0,1,1] — visible,
        // never invisible or a panic.
        let theme = UiTheme::engine_default();
        let tree = themed_text(ColorValue::Token("no.such.color".into()), None);
        let mut ui = UiTree::from_descriptor(&tree, &theme);
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
        assert_eq!(
            data.texts[0].color,
            srgb_of([1.0, 0.0, 1.0, 1.0]),
            "unknown color token degrades to opaque magenta",
        );
    }

    #[test]
    fn spacing_token_resolves_into_layout_gap() {
        // A container `gap: "l"` (theme `l` = 16px) lays its two children out with
        // exactly the theme-defined spacing — proving spacing tokens resolve into
        // the taffy style before layout.
        let theme = UiTheme::engine_default();
        let l = theme.spacing("l").unwrap();
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: Widget::VStack(ContainerWidget {
                gap: SpacingValue::Token("l".into()),
                padding: SpacingValue::Literal(0.0),
                align: Align::Start,
                fill: None,
                border: None,
                children: vec![text("AB", 30.0), text("CD", 30.0)],
            }),
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme);
        let mut fs = font_system();
        ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
        let children: Vec<_> = ui.taffy.children(ui.root).unwrap();
        let c0 = *ui.taffy.layout(children[0]).unwrap();
        let c1 = *ui.taffy.layout(children[1]).unwrap();
        assert!(
            approx(c1.location.y - (c0.location.y + c0.size.height), l),
            "token gap resolved to the theme's `l` spacing ({l}px), got {}",
            c1.location.y - (c0.location.y + c0.size.height),
        );
    }

    #[test]
    fn unknown_spacing_token_lays_out_as_zero() {
        // An unknown gap token degrades to 0.0 — the two children abut with no gap.
        let theme = UiTheme::engine_default();
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: Widget::VStack(ContainerWidget {
                gap: SpacingValue::Token("no.such.spacing".into()),
                padding: SpacingValue::Literal(0.0),
                align: Align::Start,
                fill: None,
                border: None,
                children: vec![text("AB", 30.0), text("CD", 30.0)],
            }),
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme);
        let mut fs = font_system();
        ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
        let children: Vec<_> = ui.taffy.children(ui.root).unwrap();
        let c0 = *ui.taffy.layout(children[0]).unwrap();
        let c1 = *ui.taffy.layout(children[1]).unwrap();
        assert!(
            approx(c1.location.y - (c0.location.y + c0.size.height), 0.0),
            "unknown spacing token lays out as 0.0, got {}",
            c1.location.y - (c0.location.y + c0.size.height),
        );
    }

    #[test]
    fn font_token_mono_resolves_to_the_mono_family_on_the_node() {
        // `font: "mono"` resolves to the theme's mono family on the node's
        // `NodeContext::Text` and the produced `UiText` line — so the run shapes
        // and draws against the registered monospace face.
        let theme = UiTheme::engine_default();
        let tree = themed_text(ColorValue::Literal([1.0; 4]), Some("mono"));
        let mut ui = UiTree::from_descriptor(&tree, &theme);
        // The node carries the resolved family before any draw.
        if let Some(NodeContext::Text { family, .. }) = ui.taffy.get_node_context(ui.root) {
            assert_eq!(family, UI_MONO_FONT_FAMILY, "node carries the mono family");
        } else {
            panic!("root must be a text node");
        }
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
        assert_eq!(
            data.texts[0].family, UI_MONO_FONT_FAMILY,
            "the drawn line selects the mono family",
        );
    }

    #[test]
    fn absent_font_resolves_to_the_body_family() {
        // A text widget with no `font` token resolves to the `body` family — the
        // pre-theming default, so fontless text keeps the body face.
        let theme = UiTheme::engine_default();
        let tree = themed_text(ColorValue::Literal([1.0; 4]), None);
        let ui = UiTree::from_descriptor(&tree, &theme);
        if let Some(NodeContext::Text { family, .. }) = ui.taffy.get_node_context(ui.root) {
            assert_eq!(family, UI_FONT_FAMILY, "absent font selects the body family");
        } else {
            panic!("root must be a text node");
        }
    }

    #[test]
    fn unknown_font_token_falls_back_to_body_family() {
        // An unknown font token degrades to the `body` family (not magenta, not a
        // panic) — text still renders in the default face.
        let theme = UiTheme::engine_default();
        let tree = themed_text(ColorValue::Literal([1.0; 4]), Some("no.such.font"));
        let ui = UiTree::from_descriptor(&tree, &theme);
        if let Some(NodeContext::Text { family, .. }) = ui.taffy.get_node_context(ui.root) {
            assert_eq!(
                family, UI_FONT_FAMILY,
                "unknown font token falls back to the body family",
            );
        } else {
            panic!("root must be a text node");
        }
    }

    #[test]
    fn override_theme_changes_resolved_token_values_on_rebuild() {
        // Rebuilding the SAME descriptor against an override theme yields the new
        // token value with NO descriptor change — the resolution seam reads the
        // theme passed at build, so a generation bump (which installs a new theme)
        // re-resolves tokens. Mirrors the engine-side setter's effect at the tree
        // level (the `UiPass` generation gate decides WHEN to rebuild; this proves
        // the rebuild produces the new values).
        let default = UiTheme::engine_default();
        let override_theme = default.with_override(&ThemeDescriptor {
            colors: StdHashMap::from([("critical".to_string(), [0.0, 1.0, 1.0, 1.0])]),
            ..Default::default()
        });
        let tree = themed_text(ColorValue::Token("critical".into()), None);

        let mut fs = font_system();
        let mut before = UiTree::from_descriptor(&tree, &default);
        let data_before = before.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
        let mut after = UiTree::from_descriptor(&tree, &override_theme);
        let data_after = after.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());

        assert_eq!(
            data_before.texts[0].color,
            srgb_of(default.color("critical").unwrap()),
        );
        assert_eq!(
            data_after.texts[0].color,
            srgb_of([0.0, 1.0, 1.0, 1.0]),
            "rebuilding against the override theme re-resolves the token value",
        );
        assert_ne!(
            data_before.texts[0].color, data_after.texts[0].color,
            "the same descriptor resolves to different colors under different themes",
        );
    }
}
