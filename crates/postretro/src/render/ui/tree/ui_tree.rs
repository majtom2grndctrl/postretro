// The retained `UiTree`: its taffy tree, the layout/draw dirty gate, and the
// per-frame bound-value diff. The draw payload types live in `node_context`; the
// draw-list collection walk is a second `impl` in `ui_tree_collect`.
// See: context/lib/ui.md §1 (retained tree), §3 (display vs. authoritative value)

use std::collections::HashMap;

use glyphon::FontSystem;
use taffy::prelude::{AvailableSpace, Display, NodeId, Size, TaffyTree};

use super::super::descriptor::AnchoredTree;
use super::super::layout::{Anchor, REFERENCE_HEIGHT, REFERENCE_WIDTH};
use super::super::theme::UiTheme;
use crate::scripting::slot_table::SlotValue;

use super::bindings::{
    BindingDiff, drive_bar_binding, drive_bar_max, drive_panel_binding, drive_text_binding,
};
use super::build::build_node;
use super::draw::UiDrawData;
use super::predicate::resolve_predicate;
use super::widget_meta::{harvest_visibility, measure_node};
use super::{CellValues, ImageSizes};

pub(crate) use super::node_context::{NodeContext, VisibilityState};

/// One retained UI tree: the taffy tree, its root node, and the placement
/// envelope's `anchor`/`offset`. One per top-level `AnchoredTree` — a future
/// modal-stack goal will want independent trees per layer, so the tree is owned
/// per-descriptor rather than shared.
pub(crate) struct UiTree {
    pub(super) taffy: TaffyTree<NodeContext>,
    pub(super) root: NodeId,
    pub(super) anchor: Anchor,
    pub(super) offset: [f32; 2],
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
    /// Per-node reactive-visibility state for nodes carrying a `visibleWhen`
    /// predicate (M13 G2, Task 2b). Keyed by the taffy `NodeId`; populated once at
    /// build from the descriptor's `visible_when` fields (harvested in lockstep
    /// with the taffy tree, so each predicate carries its nearest `localState`
    /// scope for `{ local }` resolution). The diff (`resolve_bindings`) resolves
    /// each predicate per frame against the snapshot and, on a resolved-value
    /// flip, toggles the node's taffy `Display` (`None` ⇄ `Flex`) and marks it
    /// dirty. A node without `visibleWhen` never appears here and stays visible.
    visibility: HashMap<NodeId, VisibilityState>,
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
        // No enclosing scope at the root: a container declaring its own
        // `localState` opens one for its subtree inside `build_node`.
        let root = build_node(&mut taffy, &tree.root, theme, None);
        // Harvest `visibleWhen` predicates in lockstep with the just-built taffy
        // tree (M13 G2, Task 2b). The first `resolve_bindings` applies each
        // predicate's resolved state (`prev` starts `None`, so the first frame is
        // always treated as a change).
        let mut visibility = HashMap::new();
        harvest_visibility(&taffy, &tree.root, root, None, &mut visibility);
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
            visibility,
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

    /// Whether `node` is currently hidden (`Display::None`) by a false
    /// `visibleWhen` predicate (M13 G2, Task 2b). The draw and focus walks query
    /// this to skip a hidden subtree without removing it from the taffy tree (the
    /// descriptor↔taffy 1:1 lockstep must hold). taffy's flexbox/grid layout
    /// already gives a `Display::None` node zero size, so this only excludes it
    /// from the per-frame draw/focus read-back.
    pub(super) fn is_display_none(&self, node: NodeId) -> bool {
        self.taffy.style(node).expect("node has a style").display == Display::None
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
    /// The fresh-build path: a fresh `UiTree` is always dirty, so the gate never
    /// short-circuits here. Production gameplay uses `build_draw_data_retained`,
    /// which retains the tree across frames and benefits from the gate; this
    /// fresh build now backs the layout/theming/binding unit tests, which drive a
    /// one-shot `UiTree` without the retained bookkeeping.
    ///
    /// `slot_values` is the frame's resolved state-store read snapshot (cloned
    /// out of the live `SlotTable`, keyed by dotted slot name). Bound text/panel
    /// nodes resolve their drawn string/color against it at `collect_node` time;
    /// an absent slot falls back to the literal descriptor value. Layout never
    /// depends on it — only the drawn payload does — so binding never re-triggers
    /// a recompute.
    #[cfg_attr(not(test), allow(dead_code))]
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

        // Fresh/splash path: no retained clock, so styleRange effects evaluate at
        // a steady `0.0`. The splash path carries no styleRanges; gameplay uses the
        // retained path, which threads the real `time_seconds`. It also carries no
        // `{ local }` binds (a fresh tree is transient and carries no scope cells),
        // so cell resolution sees an empty map.
        let no_cells = CellValues::new();
        self.collect_draw_data(device_size, slot_values, &no_cells, 0.0)
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
        // Resolved presentation-cell values for the frame, keyed by
        // `(scopeId, cellName)`. `{ local }` binds resolve
        // against this the same way `{ slot }` binds resolve against `slot_values`;
        // it rides the snapshot, so a cell write never forces a rebuild.
        cell_values: &CellValues,
        // Deterministic, dt-accumulated frame time (seconds). The tween driver
        // (`resolve_bindings`) reads it to advance eased display values: a tween's
        // normalized progress is `(time_seconds - start_time) / duration`.
        time_seconds: f64,
    ) -> UiDrawData {
        // Subscriber-aware diff + tween driver: resolve bound nodes against the
        // new snapshot at this frame's time, easing tweened display values and
        // classifying each change. Runs before the gate so its `mark_dirty` is
        // visible to `taffy.dirty(root)` below.
        let BindingDiff {
            content_changed,
            appearance_changed,
        } = self.resolve_bindings(slot_values, cell_values, time_seconds);

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
            let data = self.collect_draw_data(device_size, slot_values, cell_values, time_seconds);
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
    pub(super) fn collect_node_ids(&self, node: NodeId, out: &mut Vec<NodeId>) {
        out.push(node);
        for child in self.taffy.children(node).expect("node children resolve") {
            self.collect_node_ids(child, out);
        }
    }

    /// Subscriber-aware bound-value diff AND tween driver. Walks every node,
    /// resolves the bound ones against `slot_values` at the frame's
    /// `time_seconds`, and reports whether any layout-affecting (text content) or
    /// appearance-only (panel fill) binding changed since the last diff. Unbound
    /// nodes and slots without a binding are never compared.
    ///
    /// For a TWEENED bind whose slot resolves to a tweenable shape (a text bind to
    /// a `Number`, a panel bind to a length-4 `Array`), the resolved value is the
    /// tween *target*; the driver eases a per-node display value toward it:
    /// - **First resolution** with `from` present starts the display at `from` and
    ///   eases toward the target (the level-load flourish); with `from` absent the
    ///   display snaps to the target (no tween on first sight).
    /// - **Target change** (retarget) restarts the eased segment from the *current
    ///   display value* at this frame's time — a mid-flight retarget never snaps.
    /// - **In flight** advances the eased display from the segment's start time
    ///   using `(now - start_time) / duration` (`duration_ms` converted to
    ///   seconds). At `t >= 1` the display equals the target EXACTLY (settle).
    ///
    /// The driver classifies through the SAME `BindingDiff` as the untweened path:
    /// a text change (the rendered, rounded string differs) is content-changed
    /// (re-measures → `mark_dirty`); a panel change is appearance-only (redraw, no
    /// relayout). A tweened text node stores its rounded/formatted display string
    /// in `last_resolved` so the measure seam shapes the displayed value; a tweened
    /// panel stores its eased fill in `last_resolved` so the diff settles.
    ///
    /// A tween whose slot resolves to any OTHER shape snaps through the unchanged
    /// resolution path (`resolve_text`/`resolve_panel_fill`) and logs one
    /// `log::warn!` per retained frame: each node is visited once per
    /// `resolve_bindings` call (one per retained frame) and there is no cross-frame
    /// dedup, matching the `resolve_panel_fill` precedent.
    ///
    /// Side effects: stores each text node's freshly resolved (or displayed) string
    /// in `last_resolved` and marks it dirty when it changed; records each panel's
    /// resolved (or eased) fill in `last_resolved`; mutates per-node tween state.
    #[allow(clippy::collapsible_match)]
    fn resolve_bindings(
        &mut self,
        slot_values: &HashMap<String, SlotValue>,
        cell_values: &CellValues,
        time_seconds: f64,
    ) -> BindingDiff {
        // Collect node ids first (depth-first from the root) to avoid borrowing
        // the taffy tree while mutating node contexts / marking dirty in the loop.
        let mut nodes: Vec<NodeId> = Vec::new();
        self.collect_node_ids(self.root, &mut nodes);
        let mut diff = BindingDiff::default();
        // Text nodes whose displayed string changed: deferred so `mark_dirty`
        // (which borrows the tree) runs after the per-node mutable borrow drops.
        let mut dirty_text: Vec<NodeId> = Vec::new();
        for node in nodes {
            // One mutable borrow per node: the tween driver both reads the prior
            // segment and writes the advanced one, so a read-then-write split would
            // need two borrows. `mark_dirty` is deferred (collected above) so the
            // borrow can drop first.
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
                        dirty_text.push(node);
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
                        // Appearance-only: no mark_dirty, no relayout.
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
                        // A bar is fixed-size: a value change only recolors/resizes
                        // its fill quad — appearance-only, never a relayout.
                        diff.appearance_changed = true;
                    }
                }
                _ => {}
            }
        }
        // Content change may re-measure: force a relayout on each changed text node.
        for node in dirty_text {
            self.mark_dirty(node);
        }

        // Reactive visibility (M13 G2, Task 2b): resolve each `visibleWhen`
        // predicate against this frame's snapshot and, on a resolved-value FLIP
        // since the last diff, toggle the node's taffy `Display` (`None` ⇄ `Flex`)
        // and mark it dirty so the layout gate recomputes and the per-frame
        // focus-rect re-export reflects it. A targeted invalidation: visibility
        // flips are rare, authored-frequency events (`lib/ui.md` §3), so this
        // re-uses the same relayout path bound content changes take. The first
        // diff always applies (`prev` is `None`). The node STAYS in the taffy tree
        // — only its `Display` flips — so the descriptor↔taffy 1:1 lockstep that
        // `export_focus_rects` walks survives. Visibility is NEVER applied in the
        // descriptor walk (`presentation_cells.rs::reconcile`), so a hidden subtree
        // never tears down its `localState` cells.
        let mut visibility_flips: Vec<(NodeId, Display)> = Vec::new();
        for (node, state) in self.visibility.iter_mut() {
            let resolved = resolve_predicate(
                &state.predicate.source,
                state.predicate.equals.as_ref(),
                state.scope.as_deref(),
                slot_values,
                cell_values,
            );
            if state.prev != Some(resolved) {
                state.prev = Some(resolved);
                // A true predicate (`1.0`) shows the node at its authored
                // `Display` (`Flex`/`Grid`); a false one (`0.0`) hides it via
                // `Display::None` while leaving it in the tree.
                let display = if resolved >= 0.5 {
                    state.visible_display
                } else {
                    Display::None
                };
                visibility_flips.push((*node, display));
            }
        }
        for (node, display) in visibility_flips {
            let mut style = self.taffy.style(node).expect("node has a style").clone();
            style.display = display;
            self.taffy
                .set_style(node, style)
                .expect("node exists in its own tree");
            // Mark dirty so the layout gate recomputes (a `Display::None` subtree
            // contributes zero size) and `export_ui_focus_rects` re-exports.
            self.mark_dirty(node);
            // A flip flags the draw list for rebuild: the hidden/shown subtree's
            // quads/glyphs must drop or reappear in the next collect.
            diff.appearance_changed = true;
        }
        diff
    }
}
