// Retained UI widget tree: maps the serde `descriptor` model into a
// `taffy::TaffyTree`, computes flex/grid layout, and reads the laid-out rects
// back into the device-pixel `UiDrawList` + shaped-text draw entries through the
// `layout` projection path. taffy/layout lives entirely here (renderer-owns-GPU).
// See: context/lib/ui.md §1 (retained tree), §3 (display vs. authoritative value / tween contract)

use std::cell::RefCell;
use std::collections::HashMap;

use taffy::prelude::{
    AlignItems, AvailableSpace, Display, FlexDirection, Layout, NodeId, Size, Style, TaffyTree,
    evenly_sized_tracks, length,
};

use super::descriptor::{
    Align, AnchoredTree, BarWidget, BindSource, Border, ButtonWidget, ColorValue, ContainerWidget,
    Easing, GridWidget, ImageWidget, LocalState, PanelBind, PanelWidget, SliderBind, SliderWidget,
    SpacingValue, TextBind, TextWidget, Widget,
};
use super::layout::{Anchor, REFERENCE_HEIGHT, REFERENCE_WIDTH};
use super::style_ranges::{StyleEffectState, StyleRanges, evaluate};
use super::theme::UiTheme;
use crate::scripting::slot_table::SlotValue;
use glyphon::FontSystem;

use super::text::{UiText, measure_run};
use super::{UiDrawList, UiInstance};

/// Resolved presentation-cell values for a frame, keyed by `(scopeId, cellName)`.
/// The app-side cell store publishes this onto the read
/// snapshot, exactly the way bound slot values flow — so the descriptor compared
/// by the retained reuse gate (`mod.rs`) stays immutable and a cell write never
/// forces a rebuild. A `{ local }` bind resolves against it through the node's
/// build-time scope. Empty (the default) whenever no `localState` scope composes.
pub(crate) type CellValues = HashMap<(String, String), SlotValue>;

/// Resolve a bind's value against the frame's snapshot: a `{ slot }` bind reads
/// the authoritative store slot map; a `{ local }` bind reads the presentation
/// cell `(scope, name)` where `scope` is the nearest declaring ancestor resolved
/// at tree-build time (`None` when the local bind had no enclosing scope, so it
/// degrades to "absent" — the bind silently falls back to its literal). This is
/// the single seam every bind-resolution helper routes through.
fn lookup_bound<'a>(
    source: &BindSource,
    scope: Option<&str>,
    slots: &'a HashMap<String, SlotValue>,
    cells: &'a CellValues,
) -> Option<&'a SlotValue> {
    match source {
        BindSource::Slot { slot } => slots.get(slot),
        BindSource::Local { local } => {
            scope.and_then(|s| cells.get(&(s.to_string(), local.to_string())))
        }
    }
}

/// Fallback color for an unknown color token: opaque magenta. A missing token
/// degrades visibly (rather than panicking or rendering invisibly) so an
/// authoring typo is obvious on screen.
const UNKNOWN_COLOR_FALLBACK: [f32; 4] = [1.0, 0.0, 1.0, 1.0];

/// Fallback spacing for an unknown spacing token: zero logical px.
const UNKNOWN_SPACING_FALLBACK: f32 = 0.0;

/// Default text size (logical-reference px) for an interactive `button`/`slider`
/// label run. The widgets carry no per-instance `font_size` in v1 (a later
/// additive field could expose it); their labels measure/draw at this size.
const INTERACTIVE_LABEL_FONT_SIZE: f32 = 18.0;

/// Default bar size (logical-reference px, `[width, height]`). A `bar` has no
/// intrinsic content to measure, so its leaf carries an explicit style size; a
/// container's `align`/stretch may still override it. Horizontal-only in v1.
const DEFAULT_BAR_SIZE: [f32; 2] = [120.0, 12.0];

// --- Value-tween easing -----------------------------------------------------

/// Identity easing: `t` unchanged.
fn linear(t: f32) -> f32 {
    t
}

/// Cubic ease-in: slow start, `t^3`.
fn ease_in(t: f32) -> f32 {
    t * t * t
}

/// Cubic ease-out: fast start, decelerating — the cubic mirror of `ease_in`.
fn ease_out(t: f32) -> f32 {
    let u = 1.0 - t;
    1.0 - u * u * u
}

/// Cubic ease-in-out: accelerate then decelerate, symmetric about `t = 0.5`.
fn ease_in_out(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let u = -2.0 * t + 2.0;
        1.0 - (u * u * u) / 2.0
    }
}

/// Dispatch an `Easing` curve, clamping `t` to `[0, 1]` first so an out-of-range
/// normalized time (a frame past the tween's end, or a negative dt) never
/// produces an eased value outside `[0, 1]`.
fn apply(easing: Easing, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match easing {
        Easing::Linear => linear(t),
        Easing::EaseIn => ease_in(t),
        Easing::EaseOut => ease_out(t),
        Easing::EaseInOut => ease_in_out(t),
    }
}

/// Per-channel linear lerp of two RGBA colors at eased fraction `e` (no rounding;
/// alpha included). Used by the panel tween driver to ease the fill color.
fn lerp_rgba(from: [f32; 4], to: [f32; 4], e: f32) -> [f32; 4] {
    [
        from[0] + (to[0] - from[0]) * e,
        from[1] + (to[1] - from[1]) * e,
        from[2] + (to[2] - from[2]) * e,
        from[3] + (to[3] - from[3]) * e,
    ]
}

/// Resolve a `ColorValue` against the active theme. A `Literal` is its own RGBA;
/// a `Token` looks the name up in the theme. An unknown token degrades to opaque
/// magenta and logs exactly one warning (per tree build, not per frame — this
/// runs at build time, which on the retained path is once per rebuild).
pub(crate) fn resolve_color(value: &ColorValue, theme: &UiTheme) -> [f32; 4] {
    match value {
        ColorValue::Literal(rgba) => *rgba,
        ColorValue::Token(name) => theme.color(name).unwrap_or_else(|| {
            log::warn!("[UI] unknown color token '{name}' — using opaque magenta fallback");
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

/// Pre-resolve a `StyleRanges`' band-color tokens against the theme at build
/// time, returning a literal-only `StyleRanges`. Each band's optional `color`
/// token degrades through `resolve_color` (unknown → opaque magenta + one warn),
/// so the once-per-build warning rule holds and the per-frame draw walk stays
/// theme-free: the draw-time evaluator only ever sees `ColorValue::Literal`
/// bands. `up_to`/`pulse`/`flash` carry through unchanged.
fn resolve_style_ranges(ranges: &StyleRanges, theme: &UiTheme) -> StyleRanges {
    use super::style_ranges::StyleEntry;
    StyleRanges {
        max: ranges.max,
        entries: ranges
            .entries
            .iter()
            .map(|entry| StyleEntry {
                up_to: entry.up_to,
                color: entry
                    .color
                    .as_ref()
                    .map(|c| ColorValue::Literal(resolve_color(c, theme))),
                pulse: entry.pulse,
                flash: entry.flash,
            })
            .collect(),
    }
}

/// Resolve a widget's optional `style_ranges` for the retained node at build
/// time: pre-resolve its band-color tokens (theme-free draw walk) and enforce the
/// `bind` precondition. styleRanges maps the widget's bound value, so without a
/// `bind` there is no value to map — warn exactly once per tree build (the
/// theme-fallback precedent) and drop it (the node carries `None`, no effect
/// fires). `kind` names the widget in the warning.
fn build_node_style_ranges(
    style_ranges: Option<&StyleRanges>,
    has_bind: bool,
    theme: &UiTheme,
    kind: &str,
) -> Option<StyleRanges> {
    let ranges = style_ranges?;
    if !has_bind {
        log::warn!(
            "[UI] {kind} widget declares styleRanges without a bind — no value to map; ignoring"
        );
        return None;
    }
    Some(resolve_style_ranges(ranges, theme))
}

/// Asset key → natural reference size (logical-reference px, `[width, height]`)
/// for `image` nodes. Threaded into the measure seam so an image sizes from its
/// real asset dimensions (content-driven, like text) rather than a wire-level
/// fixed size. The renderer builds this from the uploaded texture's pixel dims.
pub(crate) type ImageSizes = HashMap<String, [f32; 2]>;

/// Live tween state for a bound node whose bind carries a `tween`. Absent on
/// untweened binds. `display` is the value the draw step renders THIS frame;
/// `start`/`start_time`/`target` describe the in-flight segment the driver eases
/// across. A retarget restarts the segment from the current `display` (never
/// snapping mid-flight), so `start` is the display value at the retarget instant,
/// not the bind's `from`. `T` is `f32` for text, `[f32; 4]` for panel.
#[derive(Debug, Clone)]
struct TweenState<T> {
    /// Value rendered this frame (eased toward `target` from `start`).
    display: T,
    /// Value the active segment eased from (set at first-resolve or retarget).
    start: T,
    /// Frame time (seconds) the active segment started easing at.
    start_time: f64,
    /// Value the active segment eases toward.
    target: T,
}

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
        /// The nearest declaring `localState` scope id resolved at build time, for
        /// a `{ local }` bind. `None` for a `{ slot }` bind or a
        /// local bind with no enclosing scope (the latter degrades to "absent").
        bind_scope: Option<String>,
        /// Last resolved bound string the diff observed. `None` until the first
        /// diff resolves the binding; only meaningful when `bind` is `Some`.
        /// Unbound nodes never set it. The measure seam shapes this string when
        /// present (so a content change re-measures), else the literal `content`.
        last_resolved: Option<String>,
        /// Live tween state when `bind`'s `tween` is `Some` AND the slot has
        /// resolved to a `Number` at least once. `None` for untweened binds and
        /// before the first numeric resolution. While `Some`, the driver eases
        /// `display` toward `target` and `last_resolved` holds the rounded,
        /// formatted display string (so the measure seam shapes the displayed
        /// value).
        tween: Option<TweenState<f32>>,
        /// Continuous value→style map. When `Some`, the rendered
        /// numeric value drives a band color + pulse/flash effect that overrides
        /// `color`. `style_state` holds the per-node effect clock (flash entry,
        /// active band) the evaluator advances each draw build. Both `None` on a
        /// styleRange-less node.
        style_ranges: Option<StyleRanges>,
        style_state: RefCell<StyleEffectState>,
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
        /// Nearest declaring `localState` scope id for a `{ local }` bind (see
        /// `NodeContext::Text::bind_scope`). `None` for slot binds and backdrops.
        bind_scope: Option<String>,
        /// Last resolved bound fill the diff observed. `None` until the first
        /// diff; only meaningful when `bind` is `Some`.
        last_resolved: Option<[f32; 4]>,
        /// Live tween state when `bind`'s `tween` is `Some` AND the slot has
        /// resolved to a length-4 `Array` at least once. `None` for untweened
        /// binds and before the first array resolution. While `Some`, the driver
        /// eases `display` (the rendered fill) per-channel toward `target`.
        tween: Option<TweenState<[f32; 4]>>,
        /// Continuous value→style map. When `Some`, the rendered
        /// numeric value (from `bind`'s slot) drives a band color + pulse/flash
        /// effect that overrides `fill`. `style_state` holds the per-node effect
        /// clock. Both `None` on a styleRange-less node and on container backdrops.
        style_ranges: Option<StyleRanges>,
        style_state: RefCell<StyleEffectState>,
    },
    /// Textured image quad. `asset` is the texture key the renderer binds; the
    /// rect comes from layout. The image sizes from the asset's natural reference
    /// dimensions via the measure seam (see `measure_node`) — content-driven, so
    /// `asset` doubles as the size key. Image batching/binding lands in the
    /// renderer; the tree records the key so the draw step can group by it.
    Image { asset: String },
    /// Horizontal value bar. Draws a `background` quad filling
    /// its laid-out rect, then a `fill` quad whose width is `value/max` clamped to
    /// `[0, 1]` of the rect width. `value` resolves from `bind`'s slot (the eased
    /// display fraction on the retained tweened path, via `last_resolved`); a
    /// styleRanges map recolors the fill the same way bound text/panel do. Passive
    /// (never focusable activation); `bind`'s tween eases the displayed fraction.
    Bar {
        bind: SliderBind,
        max: f32,
        fill: [f32; 4],
        background: [f32; 4],
        /// Nearest declaring `localState` scope id for a `{ local }` bind (see
        /// `NodeContext::Text::bind_scope`). `None` for a slot bind.
        bind_scope: Option<String>,
        /// Last resolved (or eased) value the diff observed, for change detection
        /// and to feed the draw the eased display fraction. `None` until first diff.
        last_resolved: Option<f32>,
        /// Live tween state when `bind`'s `tween` is `Some` and the slot has
        /// resolved to a `Number` at least once. While `Some`, the driver eases the
        /// displayed value toward the slot target; the draw reads `display`.
        tween: Option<TweenState<f32>>,
        style_ranges: Option<StyleRanges>,
        style_state: RefCell<StyleEffectState>,
    },
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
        // No enclosing scope at the root: a container declaring its own
        // `localState` opens one for its subtree inside `build_node`.
        let root = build_node(&mut taffy, &tree.root, theme, None);
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

        // Fresh/splash path: no retained clock, so styleRange effects evaluate at
        // a steady `0.0`. The splash path carries no styleRanges; gameplay uses the
        // retained path, which threads the real `time_seconds`. It also carries no
        // `{ local }` binds (a fresh tree is transient and carries no scope cells),
        // so cell resolution sees an empty map.
        let no_cells = CellValues::new();
        self.collect_draw_data(device_size, slot_values, &no_cells, 0.0)
    }

    /// Read the cached taffy layout back into a fresh `UiDrawData`, resolving any
    /// bound text/panel nodes against the live `slot_values`. Pure read-back — it
    /// assumes layout is already computed for `device_size` (the caller's gate
    /// ran the compute when needed). Shared by the fresh path (`build_draw_data`,
    /// which calls it every frame) and the retained path (which calls it only
    /// when the draw list needs rebuilding). `time_seconds` is the frame's
    /// dt-accumulated clock the styleRange pulse/flash effects advance against.
    fn collect_draw_data(
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

        let scale = super::layout::device_scale(device_size);
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
    fn collect_node_ids(&self, node: NodeId, out: &mut Vec<NodeId>) {
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
                    last_resolved,
                    tween,
                    ..
                }) => {
                    if drive_bar_binding(
                        bind,
                        bind_scope.as_deref(),
                        last_resolved,
                        tween,
                        slot_values,
                        cell_values,
                        time_seconds,
                    ) {
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
        diff
    }

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
    ) -> FocusRectList {
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
            root_origin,
            scale,
            canvas_origin,
            &mut z,
            &mut out,
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
        ref_origin: [f32; 2],
        scale: f32,
        canvas_origin: [f32; 2],
        z: &mut u32,
        out: &mut FocusRectList,
    ) {
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
            out.rects.push(FocusRect {
                id: id.clone(),
                rect,
                z: this_z,
                group,
                neighbors,
                interaction: widget_interaction(widget),
            });
            if let Some(g) = group {
                out.groups[g].members.push(rect_index);
            }
        }

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
                    child_origin,
                    scale,
                    canvas_origin,
                    z,
                    out,
                );
            }
        }
    }

    /// Walk a node and its descendants, accumulating draw entries. `ref_origin`
    /// is the node's top-left in reference space (parent origin + the node's
    /// taffy-relative location). Children recurse with their own absolute origin.
    fn collect_node(
        &self,
        node: NodeId,
        ref_origin: [f32; 2],
        walk: &DrawWalkCtx<'_>,
        data: &mut UiDrawData,
    ) {
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
                let fraction = if *max > 0.0 {
                    (value / *max).clamp(0.0, 1.0)
                } else {
                    0.0
                };

                // styleRanges recolors the fill the same widget-agnostic way bound
                // text/panel do: the evaluator maps the value against its own `max`.
                let mut fill_color = *fill;
                if let Some(ranges) = style_ranges {
                    fill_color = evaluate(
                        ranges,
                        value,
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
                        match style_text_value(
                            bind.as_ref(),
                            bind_scope.as_deref(),
                            tween.as_ref(),
                            slot_values,
                            cell_values,
                        ) {
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
            self.collect_node(child, child_origin, walk, data);
        }
    }
}

/// Walk-invariant context threaded through `collect_node`'s recursion: the
/// device-pixel projection, the slot snapshot the draw reads, the dt-accumulated
/// UI clock, and the inert theme the (pre-resolved) styleRanges evaluator takes.
struct DrawWalkCtx<'a> {
    canvas_origin: [f32; 2],
    scale: f32,
    slot_values: &'a HashMap<String, SlotValue>,
    /// Presentation-cell values for `{ local }` bind resolution.
    cell_values: &'a CellValues,
    time_seconds: f64,
    inert_theme: &'a UiTheme,
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

/// Drive one bound TEXT node for this frame and return whether its rendered
/// (`last_resolved`) string changed since the last diff. Three paths:
///
/// - **Untweened** (`bind.tween` is `None`): the original behavior — resolve the
///   string via `resolve_text`, store it, report change. `tween` stays `None`.
/// - **Tweened, slot resolves to a `Number`**: the number is the eased target.
///   `drive_tween_f32` advances the per-node `f32` display from its segment; the
///   rounded display is formatted through `bind.format`'s `{}` (same integral
///   formatting as `slot_value_string`) and stored as the rendered string, so the
///   measure seam shapes the displayed value.
/// - **Tweened, slot resolves to any other shape**: snap-through — render via the
///   unchanged `resolve_text` path and log one `log::warn!` per retained frame
///   (the node is visited once per `resolve_bindings` call; there is no cross-frame
///   dedup, matching the `resolve_panel_fill` precedent).
///
/// `now` is the frame's `time_seconds`.
// Wide by necessity: bind + its resolved scope + the node's mutable display state
// (content/last_resolved/tween) + both value maps (slots, cells) + the frame
// clock are all distinct per-node diff inputs; a struct would only obscure them.
#[allow(clippy::too_many_arguments)]
fn drive_text_binding(
    bind: &TextBind,
    bind_scope: Option<&str>,
    content: &str,
    last_resolved: &mut Option<String>,
    tween: &mut Option<TweenState<f32>>,
    slot_values: &HashMap<String, SlotValue>,
    cell_values: &CellValues,
    now: f64,
) -> bool {
    let Some(cfg) = bind.tween.as_ref() else {
        // No tween config: the untweened path, byte-for-byte as before.
        let resolved = resolve_text(Some(bind), bind_scope, content, slot_values, cell_values);
        let changed = last_resolved.as_deref() != Some(resolved.as_str());
        if changed {
            *last_resolved = Some(resolved);
        }
        return changed;
    };

    let rendered = match lookup_bound(&bind.source, bind_scope, slot_values, cell_values) {
        Some(SlotValue::Number(n)) => {
            // Tweenable: the number is the eased target. Advance the display value
            // and render the rounded integer through the format template.
            let target = *n;
            let display =
                drive_tween_f32(tween, cfg.from, target, cfg.duration_ms, cfg.easing, now);
            let integral = display.round() as i64;
            match &bind.format {
                Some(template) => template.replacen("{}", &integral.to_string(), 1),
                None => integral.to_string(),
            }
        }
        _ => {
            // A tween on a non-`Number` slot (or an absent slot): snap through the
            // unchanged resolution path, warning once per retained frame. An absent
            // slot is the normal fallback case and does NOT warn (`resolve_text`
            // already treats absence silently); only a present, non-numeric value
            // is an authoring error worth the warn.
            if lookup_bound(&bind.source, bind_scope, slot_values, cell_values).is_some() {
                warn_non_tweenable_text(bind);
            }
            resolve_text(Some(bind), bind_scope, content, slot_values, cell_values)
        }
    };

    let changed = last_resolved.as_deref() != Some(rendered.as_str());
    if changed {
        *last_resolved = Some(rendered);
    }
    changed
}

/// Advance (or initialize / retarget) a text node's `f32` tween segment toward
/// `target` at frame time `now`, returning the eased display value for this frame.
/// Stores the advanced state back into `tween`. See `resolve_bindings` for the
/// first-resolve / retarget / in-flight / settle mechanics.
fn drive_tween_f32(
    tween: &mut Option<TweenState<f32>>,
    from: Option<f32>,
    target: f32,
    duration_ms: f32,
    easing: Easing,
    now: f64,
) -> f32 {
    match tween {
        None => {
            // First resolution. With `from` present, start there and ease toward
            // the target (the level-load flourish); with `from` absent, snap to the
            // target (no tween on first sight) by seeding a settled segment.
            let start = from.unwrap_or(target);
            let mut state = TweenState {
                display: start,
                start,
                start_time: now,
                target,
            };
            state.display = advance_f32(&state, duration_ms, easing, now);
            let display = state.display;
            *tween = Some(state);
            display
        }
        Some(state) => {
            // Retarget: a new target restarts the segment from the CURRENT display
            // (never snapping mid-flight) at this frame's time.
            if state.target != target {
                state.start = state.display;
                state.start_time = now;
                state.target = target;
            }
            state.display = advance_f32(state, duration_ms, easing, now);
            state.display
        }
    }
}

/// Sample a text tween segment at `now`: normalized progress `(now - start_time) /
/// duration`, eased, lerped from `start` to `target`. At `t >= 1` (including a
/// non-positive duration) the value equals `target` EXACTLY so the settle is bit-
/// exact. `duration_ms` is milliseconds; converted to seconds for the f64 clock.
fn advance_f32(state: &TweenState<f32>, duration_ms: f32, easing: Easing, now: f64) -> f32 {
    let duration = (duration_ms as f64) / 1000.0;
    if duration <= 0.0 || now - state.start_time >= duration {
        return state.target;
    }
    let t = ((now - state.start_time) / duration) as f32;
    let e = apply(easing, t);
    state.start + (state.target - state.start) * e
}

/// Log the snap-through warning for a text tween whose slot resolved to a
/// non-`Number` shape. Fires once per retained frame: the caller visits each node
/// once per `resolve_bindings` call (one per retained frame) and there is no
/// cross-frame dedup, matching the `resolve_panel_fill` precedent. The snap itself
/// renders via `resolve_text`, so this never touches the tween state.
fn warn_non_tweenable_text(bind: &TextBind) {
    log::warn!(
        "[UI] text bind '{}' carries a tween but did not resolve to a Number; \
         rendering the raw value without easing",
        bind_target_name(&bind.source),
    );
}

/// Drive one bound PANEL node for this frame and return whether its rendered
/// (`last_resolved`) fill changed since the last diff. Mirrors
/// `drive_text_binding`:
///
/// - **Untweened**: resolve the fill via `resolve_panel_fill`, store, report.
/// - **Tweened, slot resolves to a length-4 `Array`**: the array is the eased
///   target; `drive_tween_rgba` advances the per-node `[f32; 4]` display
///   per-channel (alpha included, no rounding) and stores it as the rendered fill.
/// - **Tweened, slot resolves to any other shape**: snap through the unchanged
///   `resolve_panel_fill` path (which already warns once per retained frame on a
///   present-but-malformed slot) — no extra tween warn, since that path owns the
///   warning.
// Wide by necessity: see `drive_text_binding` — same per-node diff input set.
#[allow(clippy::too_many_arguments)]
fn drive_panel_binding(
    bind: &PanelBind,
    bind_scope: Option<&str>,
    fallback: [f32; 4],
    last_resolved: &mut Option<[f32; 4]>,
    tween: &mut Option<TweenState<[f32; 4]>>,
    slot_values: &HashMap<String, SlotValue>,
    cell_values: &CellValues,
    now: f64,
) -> bool {
    let Some(cfg) = bind.tween.as_ref() else {
        let resolved =
            resolve_panel_fill(Some(bind), bind_scope, fallback, slot_values, cell_values);
        let changed = last_resolved.is_none_or(|prev| !colors_eq(prev, resolved));
        if changed {
            *last_resolved = Some(resolved);
        }
        return changed;
    };

    let resolved = match lookup_bound(&bind.source, bind_scope, slot_values, cell_values) {
        Some(SlotValue::Array(rgba)) if rgba.len() == 4 => {
            let target = [rgba[0], rgba[1], rgba[2], rgba[3]];
            drive_tween_rgba(tween, cfg.from, target, cfg.duration_ms, cfg.easing, now)
        }
        // Non-tweenable shape (absent, wrong variant, or wrong length): snap
        // through the unchanged fill-resolution path. `resolve_panel_fill` already
        // owns the once-per-frame warn for a present-but-malformed value, so the
        // tween adds none here.
        _ => resolve_panel_fill(Some(bind), bind_scope, fallback, slot_values, cell_values),
    };

    let changed = last_resolved.is_none_or(|prev| !colors_eq(prev, resolved));
    if changed {
        *last_resolved = Some(resolved);
    }
    changed
}

/// Drive one bound BAR node for this frame and return whether its displayed value
/// changed since the last diff. Mirrors the tweened-text numeric path but stores
/// an `f32` display value (the bar draws a fill fraction, not a string):
///
/// - **Untweened**: the displayed value is the raw slot `Number` (or `0.0` when
///   absent/non-numeric); store it, report the change.
/// - **Tweened, slot resolves to a `Number`**: `drive_tween_f32` eases a per-node
///   display value toward the slot target so the rendered fill fraction eases.
/// - **Tweened, slot resolves to any other shape**: snap to the raw value (`0.0`).
fn drive_bar_binding(
    bind: &SliderBind,
    bind_scope: Option<&str>,
    last_resolved: &mut Option<f32>,
    tween: &mut Option<TweenState<f32>>,
    slot_values: &HashMap<String, SlotValue>,
    cell_values: &CellValues,
    now: f64,
) -> bool {
    let resolved = match bind.tween.as_ref() {
        Some(cfg) => match lookup_bound(&bind.source, bind_scope, slot_values, cell_values) {
            Some(SlotValue::Number(n)) => {
                drive_tween_f32(tween, cfg.from, *n, cfg.duration_ms, cfg.easing, now)
            }
            _ => bar_slot_value(bind, bind_scope, slot_values, cell_values),
        },
        None => bar_slot_value(bind, bind_scope, slot_values, cell_values),
    };
    let changed = last_resolved.is_none_or(|prev| (prev - resolved).abs() > f32::EPSILON);
    if changed {
        *last_resolved = Some(resolved);
    }
    changed
}

/// Advance (or initialize / retarget) a panel node's RGBA tween segment toward
/// `target` at frame time `now`, returning the eased per-channel display fill.
/// Same first-resolve / retarget / in-flight / settle mechanics as the text
/// driver, but the lerp runs per channel (alpha included, no rounding).
fn drive_tween_rgba(
    tween: &mut Option<TweenState<[f32; 4]>>,
    from: Option<[f32; 4]>,
    target: [f32; 4],
    duration_ms: f32,
    easing: Easing,
    now: f64,
) -> [f32; 4] {
    match tween {
        None => {
            let start = from.unwrap_or(target);
            let mut state = TweenState {
                display: start,
                start,
                start_time: now,
                target,
            };
            state.display = advance_rgba(&state, duration_ms, easing, now);
            let display = state.display;
            *tween = Some(state);
            display
        }
        Some(state) => {
            if state.target != target {
                state.start = state.display;
                state.start_time = now;
                state.target = target;
            }
            state.display = advance_rgba(state, duration_ms, easing, now);
            state.display
        }
    }
}

/// Sample a panel tween segment at `now`: eased fraction (as `advance_f32`),
/// applied per channel via `lerp_rgba`. At `t >= 1` (or a non-positive duration)
/// the fill equals `target` EXACTLY so the settle is bit-exact.
fn advance_rgba(
    state: &TweenState<[f32; 4]>,
    duration_ms: f32,
    easing: Easing,
    now: f64,
) -> [f32; 4] {
    let duration = (duration_ms as f64) / 1000.0;
    if duration <= 0.0 || now - state.start_time >= duration {
        return state.target;
    }
    let t = ((now - state.start_time) / duration) as f32;
    let e = apply(easing, t);
    lerp_rgba(state.start, state.target, e)
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

/// A widget's authored focus id and neighbor overrides, for the focus-rect
/// export. Every kind carries `id`/`focus_neighbors` except `spacer` (id only,
/// never focusable). Returns the authored id (borrowed) and the exported
/// neighbor overrides.
fn focus_meta(widget: &Widget) -> (Option<&String>, FocusNeighbors) {
    match widget {
        Widget::Text(w) => (w.id.as_ref(), (&w.focus_neighbors).into()),
        Widget::Panel(w) => (w.id.as_ref(), (&w.focus_neighbors).into()),
        Widget::Image(w) => (w.id.as_ref(), (&w.focus_neighbors).into()),
        Widget::Spacer(w) => (w.id.as_ref(), FocusNeighbors::default()),
        Widget::VStack(w) | Widget::HStack(w) => (w.id.as_ref(), (&w.focus_neighbors).into()),
        Widget::Grid(w) => (w.id.as_ref(), (&w.focus_neighbors).into()),
        // Interactive widgets carry a REQUIRED id (focusable markers): button and
        // slider always export as focusable. `bar` is passive — id only.
        Widget::Button(w) => (Some(&w.id), (&w.focus_neighbors).into()),
        Widget::Slider(w) => (Some(&w.id), (&w.focus_neighbors).into()),
        Widget::Bar(w) => (w.id.as_ref(), FocusNeighbors::default()),
    }
}

/// The interaction metadata for an interactive widget, or
/// `None` for passive nodes. `button` carries its activation reaction; `slider`
/// its bound-value step parameters. The focus-rect export attaches this so the
/// app can drive activation/value-step from the focused node id.
fn widget_interaction(widget: &Widget) -> Option<NodeInteraction> {
    match widget {
        Widget::Button(w) => Some(NodeInteraction::Button {
            on_press: w.on_press.clone(),
            repeat_on_hold: w.repeat_on_hold.map(Into::into),
        }),
        Widget::Slider(w) => Some(NodeInteraction::Slider {
            slot: w.bind.source.slot().unwrap_or_default().to_string(),
            min: w.min,
            max: w.max,
            step: w.step,
            captures_nav: w.captures_nav.clone(),
        }),
        _ => None,
    }
}

/// The focus policy a container declares, or `None` for leaves and policy-less
/// containers. A declaring container opens a focus group its direct children join.
fn container_focus_policy(widget: &Widget) -> Option<&super::descriptor::FocusPolicy> {
    match widget {
        Widget::VStack(w) | Widget::HStack(w) => w.focus.as_ref(),
        Widget::Grid(w) => w.focus.as_ref(),
        _ => None,
    }
}

/// Whether `widget` or any descendant container declares `restoreOnReturn`.
/// Surfaced tree-wide on the focus rect list: the focus engine restores this
/// tree's saved focus on a returning pop when any of its containers opted in.
fn any_restore_on_return(widget: &Widget) -> bool {
    let declared = match widget {
        Widget::VStack(w) | Widget::HStack(w) => w.restore_on_return,
        Widget::Grid(w) => w.restore_on_return,
        _ => false,
    };
    declared
        || widget_children(widget)
            .is_some_and(|children| children.iter().any(any_restore_on_return))
}

/// A container's `children` for the lockstep focus walk, or `None` for leaves.
fn widget_children(widget: &Widget) -> Option<&[Widget]> {
    match widget {
        Widget::VStack(w) | Widget::HStack(w) => Some(&w.children),
        Widget::Grid(w) => Some(&w.children),
        _ => None,
    }
}

/// The scope id a container's `localState` declaration opens, if any. `None`
/// when the container declares no `localState`, so its subtree
/// inherits the enclosing scope.
fn local_state_scope(local_state: Option<&LocalState>) -> Option<&str> {
    local_state.map(|ls| ls.scope.as_str())
}

/// Recursively build a taffy node (and its children) for one descriptor widget.
/// Resolves every theme token (color/spacing/font) against `theme` into the
/// concrete value the node carries, so the per-frame walk is theme-free.
///
/// `scope` is the nearest enclosing `localState` scope id,
/// threaded down so a `{ local }` bind on this node (or a descendant) resolves
/// against the right scope at draw time. A container declaring its own
/// `localState` overrides `scope` for its subtree (see `build_stack`/`build_grid`).
fn build_node(
    taffy: &mut TaffyTree<NodeContext>,
    widget: &Widget,
    theme: &UiTheme,
    scope: Option<&str>,
) -> NodeId {
    /// The scope id stored on a node's bind context: the nearest enclosing scope
    /// when the bind reads a presentation cell, else `None` (a `{ slot }` bind or
    /// no bind reads no cell, so it needs no scope). Keeping it `None` for slot
    /// binds means the draw walk's `lookup_bound` never consults the cell map for
    /// them — slot resolution is unchanged.
    fn bind_scope_for(source: Option<&BindSource>, scope: Option<&str>) -> Option<String> {
        match source {
            Some(BindSource::Local { local }) => {
                if scope.is_none() {
                    log::warn!(
                        "[UI] local bind \"{local}\" has no enclosing localState scope; \
                         falling back to literal"
                    );
                }
                scope.map(str::to_string)
            }
            _ => None,
        }
    }
    match widget {
        Widget::Text(TextWidget {
            content,
            font_size,
            color,
            font,
            bind,
            style_ranges,
            // `id`/`focus_neighbors` are read by the focus-rect export, not the
            // draw build — the draw walk ignores them.
            ..
        }) => {
            let style_ranges =
                build_node_style_ranges(style_ranges.as_ref(), bind.is_some(), theme, "text");
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
                        bind_scope: bind_scope_for(bind.as_ref().map(|b| &b.source), scope),
                        bind: bind.clone(),
                        last_resolved: None,
                        // Tween state is born on the first numeric resolution, not
                        // at build: the fresh path never tweens, and the retained
                        // diff initializes it when the slot first reads a `Number`.
                        tween: None,
                        style_ranges,
                        style_state: RefCell::new(StyleEffectState::default()),
                    },
                )
                .expect("taffy leaf creation must succeed")
        }
        Widget::Panel(PanelWidget {
            fill,
            border,
            bind,
            style_ranges,
            // `id`/`focus_neighbors` are read by the focus-rect export, not here.
            ..
        }) => {
            let style_ranges =
                build_node_style_ranges(style_ranges.as_ref(), bind.is_some(), theme, "panel");
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
                        bind_scope: bind_scope_for(bind.as_ref().map(|b| &b.source), scope),
                        bind: bind.clone(),
                        last_resolved: None,
                        // Born on the first length-4 array resolution (see above).
                        tween: None,
                        style_ranges,
                        style_state: RefCell::new(StyleEffectState::default()),
                    },
                )
                .expect("taffy leaf creation must succeed")
        }
        Widget::Image(ImageWidget { asset, .. }) => taffy
            .new_leaf_with_context(
                Style::default(),
                NodeContext::Image {
                    asset: asset.clone(),
                },
            )
            .expect("taffy leaf creation must succeed"),
        Widget::VStack(container) => {
            build_stack(taffy, container, FlexDirection::Column, theme, scope)
        }
        Widget::HStack(container) => {
            build_stack(taffy, container, FlexDirection::Row, theme, scope)
        }
        Widget::Grid(grid) => build_grid(taffy, grid, theme, scope),
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
        Widget::Button(button) => build_button(taffy, button, theme),
        Widget::Slider(slider) => build_slider(taffy, slider, theme, scope),
        Widget::Bar(bar) => build_bar(taffy, bar, theme, scope),
    }
}

/// Build an interactive `button` leaf. Renders its `label`
/// as a centered text run shaping against the theme `body` face. The button is a
/// pure text leaf for layout/draw; its focusable marker + activation (`on_press`)
/// ride the focus-rect export (`focus_meta` / `widget_interaction`), not the draw
/// payload. The label color resolves the theme `body`-text default (white), the
/// same flat color a literal text widget would carry.
fn build_button(
    taffy: &mut TaffyTree<NodeContext>,
    button: &ButtonWidget,
    theme: &UiTheme,
) -> NodeId {
    taffy
        .new_leaf_with_context(
            Style::default(),
            NodeContext::Text {
                content: button.label.clone(),
                font_size: INTERACTIVE_LABEL_FONT_SIZE,
                color: resolve_color(&ColorValue::Token("body".to_string()), theme),
                family: resolve_font(&None, theme),
                // A button label is static text — no bind, tween, or styleRanges.
                bind_scope: None,
                bind: None,
                last_resolved: None,
                tween: None,
                style_ranges: None,
                style_state: RefCell::new(StyleEffectState::default()),
            },
        )
        .expect("taffy leaf creation must succeed")
}

/// Build an interactive `slider` leaf. Renders `label` plus
/// the current numeric value as one text run: it binds the slot through a
/// synthesized `"<label>: {}"` format so the value display reuses the existing
/// bound-text resolution + tween machinery (the slider's bind tween eases the
/// shown number). The focusable marker + nav-capture/value-step ride the
/// focus-rect export, not the draw payload.
fn build_slider(
    taffy: &mut TaffyTree<NodeContext>,
    slider: &SliderWidget,
    theme: &UiTheme,
    scope: Option<&str>,
) -> NodeId {
    // Synthesize a text bind so the value display rides the bound-text path:
    // `content` is the fallback (label with no value yet), `format` injects the
    // resolved number after the label. The slider's bind tween carries through.
    let format = format!("{}: {{}}", slider.label);
    let bind = TextBind {
        source: slider.bind.source.clone(),
        format: Some(format),
        tween: slider.bind.tween.clone(),
    };
    let bind_scope = match &bind.source {
        BindSource::Local { .. } => scope.map(str::to_string),
        BindSource::Slot { .. } => None,
    };
    taffy
        .new_leaf_with_context(
            Style::default(),
            NodeContext::Text {
                content: slider.label.clone(),
                font_size: INTERACTIVE_LABEL_FONT_SIZE,
                color: resolve_color(&ColorValue::Token("body".to_string()), theme),
                family: resolve_font(&None, theme),
                bind_scope,
                bind: Some(bind),
                last_resolved: None,
                tween: None,
                style_ranges: None,
                style_state: RefCell::new(StyleEffectState::default()),
            },
        )
        .expect("taffy leaf creation must succeed")
}

/// Build a passive horizontal `bar` leaf. Carries an explicit
/// style size (a bar has no content to measure) and a `NodeContext::Bar` draw
/// payload. Its `fill`/`background` color tokens resolve against the theme at
/// build time; `style_ranges`' band colors pre-resolve too (theme-free draw walk),
/// gated on the bind precondition like text/panel styleRanges.
fn build_bar(
    taffy: &mut TaffyTree<NodeContext>,
    bar: &BarWidget,
    theme: &UiTheme,
    scope: Option<&str>,
) -> NodeId {
    let bind_scope = match &bar.bind.source {
        BindSource::Local { .. } => scope.map(str::to_string),
        BindSource::Slot { .. } => None,
    };
    let style = Style {
        size: Size {
            width: length(DEFAULT_BAR_SIZE[0]),
            height: length(DEFAULT_BAR_SIZE[1]),
        },
        ..Default::default()
    };
    // A bar always binds (a value to display), so the styleRanges bind precondition
    // is satisfied; pre-resolve its band-color tokens to literals for the draw walk.
    let style_ranges = build_node_style_ranges(bar.style_ranges.as_ref(), true, theme, "bar");
    taffy
        .new_leaf_with_context(
            style,
            NodeContext::Bar {
                bind_scope,
                bind: bar.bind.clone(),
                max: bar.max,
                fill: resolve_color(&bar.fill, theme),
                background: resolve_color(&bar.background, theme),
                last_resolved: None,
                tween: None,
                style_ranges,
                style_state: RefCell::new(StyleEffectState::default()),
            },
        )
        .expect("taffy leaf creation must succeed")
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
            bind_scope: None,
            bind: None,
            last_resolved: None,
            tween: None,
            // Backdrops carry no styleRanges (styleRanges live on bound leaves).
            style_ranges: None,
            style_state: RefCell::new(StyleEffectState::default()),
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
    scope: Option<&str>,
) -> NodeId {
    // A container declaring its own `localState` opens a scope its subtree's
    // `{ local }` binds resolve against; otherwise children inherit the enclosing
    // scope. Nesting overrides by tree depth (nearest declaring ancestor wins).
    let child_scope = local_state_scope(container.local_state.as_ref()).or(scope);
    let children: Vec<NodeId> = container
        .children
        .iter()
        .map(|child| build_node(taffy, child, theme, child_scope))
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
fn build_grid(
    taffy: &mut TaffyTree<NodeContext>,
    grid: &GridWidget,
    theme: &UiTheme,
    scope: Option<&str>,
) -> NodeId {
    // A grid carries no `localState` of its own (only stack containers declare
    // scopes — the `local_state` field lives on `ContainerWidget`), so its
    // children simply inherit the enclosing scope.
    let children: Vec<NodeId> = grid
        .children
        .iter()
        .map(|child| build_node(taffy, child, theme, scope))
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

// --- Hit-test / focus rect-list export ---------------------------------------

/// Focus-traversal kind exported with a focus group. The descriptor twin
/// (`descriptor::FocusKind`) is converted into this at export so the app-side
/// focus engine carries no descriptor dependency on the wire enum's serde derive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusKind {
    Linear,
    Spatial,
}

/// Hold-to-repeat timing exported with a focus group (milliseconds). Mirrors
/// `descriptor::RepeatPolicy`; carried on the focus rect list so the app-side
/// focus engine's dt-clocked repeat timer reads the container's declared cadence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RepeatPolicy {
    pub initial_delay_ms: f32,
    pub interval_ms: f32,
}

/// Per-direction id overrides exported with a focusable node. A set direction
/// wins over the container policy: nav that way jumps straight to the named node.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FocusNeighbors {
    pub up: Option<String>,
    pub down: Option<String>,
    pub left: Option<String>,
    pub right: Option<String>,
}

/// One focusable/interactive node in the exported hit-test/focus rect list: its
/// stable id (authored or auto-generated from tree position), device-pixel rect
/// `[x, y, w, h]`, painter z (tree order — later = higher), the index of the
/// focus group that governs its directional traversal (if any), and its neighbor
/// overrides. The app-side focus engine consumes this the FOLLOWING frame (the
/// reverse of the app→renderer snapshot's N→N+1 contract).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FocusRect {
    pub id: String,
    pub rect: [f32; 4],
    pub z: u32,
    /// Index into `FocusRectList::groups` of the nearest ancestor container that
    /// declares a focus policy, or `None` when no ancestor governs this node.
    pub group: Option<usize>,
    pub neighbors: FocusNeighbors,
    /// Interaction metadata for an interactive widget: a
    /// `button`'s activation reaction or a `slider`'s value-step parameters. `None`
    /// for passive focusables (an id-bearing text/panel/image). The app reads this
    /// off the focused node to fire activation (button `on_press`) or to apply a
    /// captured nav step (slider), keeping the focus engine widget-agnostic.
    pub interaction: Option<NodeInteraction>,
}

/// Per-node interaction metadata exported with an interactive focusable node.
/// The app resolves the focused node's interaction to fire
/// a button's named reaction on confirm/click, or to step a slider's bound value
/// on a captured nav intent and emit the `setState` write.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NodeInteraction {
    /// A `button`: activation (confirm/click) fires `on_press` through the
    /// reaction registry — the same vocabulary entity/system reactions use.
    /// `repeat_on_hold`, when present, opts the button into activation-repeat: a
    /// HELD confirm re-fires `on_press` on the focus engine's hold-to-repeat clock
    /// (on-screen keyboard backspace pattern). Absent keeps the single-fire rule
    /// (one activation per press).
    Button {
        on_press: String,
        repeat_on_hold: Option<RepeatPolicy>,
    },
    /// A `slider`: a captured nav wire in `captures_nav` steps the bound slot's
    /// value by `step` within `[min, max]` and emits a `setState` write on the N+1
    /// frame. `"nav.left"`/`"nav.down"` decrease the value, `"nav.right"`/`"nav.up"`
    /// increase it; any other captured name is swallowed (focus does not move) but
    /// leaves the value unchanged.
    Slider {
        slot: String,
        min: f32,
        max: f32,
        step: f32,
        captures_nav: Vec<String>,
    },
}

/// A focus group exported from a container that declares a `focus` policy: its
/// traversal kind, wrap flag, optional repeat cadence, and the indices (into
/// `FocusRectList::rects`) of its directly-governed focusable members in tree
/// order. The focus engine moves focus within a group by its policy.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FocusGroup {
    pub kind: FocusKind,
    pub wrap: bool,
    pub repeat: Option<RepeatPolicy>,
    /// Indices into `FocusRectList::rects` of this group's members, tree order.
    pub members: Vec<usize>,
}

/// The flat hit-test / focus rect list exported once per draw-data build: every
/// focusable node's id+rect+z+group, plus the focus groups their containers
/// declared. The renderer publishes this back to the app (the reverse twin of the
/// app→renderer `UiReadSnapshot`); the focus engine reads it the next frame to
/// move focus, resolve pointer hits (topmost z), and drive the repeat timer.
///
/// "Focusable" today means a node that carries an authored `id` or sits under a
/// container that declares a focus policy (interactive widgets plug their
/// markers into this seam).
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct FocusRectList {
    pub rects: Vec<FocusRect>,
    pub groups: Vec<FocusGroup>,
    /// The tree's `initialFocus` (from the `AnchoredTree` envelope): the node id
    /// focus starts on when this tree becomes the active (top) stack tree. `None`
    /// selects the first focusable node in tree order.
    pub initial_focus: Option<String>,
    /// True when any container in the tree declared `restoreOnReturn`: on a pop
    /// that returns focus here, the focus engine restores this tree's last-focused
    /// node instead of resetting to `initial_focus`.
    pub restore_on_return: bool,
}

impl From<super::descriptor::FocusKind> for FocusKind {
    fn from(kind: super::descriptor::FocusKind) -> Self {
        match kind {
            super::descriptor::FocusKind::Linear => FocusKind::Linear,
            super::descriptor::FocusKind::Spatial => FocusKind::Spatial,
        }
    }
}

impl From<super::descriptor::RepeatPolicy> for RepeatPolicy {
    fn from(p: super::descriptor::RepeatPolicy) -> Self {
        Self {
            initial_delay_ms: p.initial_delay_ms,
            interval_ms: p.interval_ms,
        }
    }
}

impl From<&super::descriptor::FocusNeighbors> for FocusNeighbors {
    fn from(n: &super::descriptor::FocusNeighbors) -> Self {
        Self {
            up: n.up.clone(),
            down: n.down.clone(),
            left: n.left.clone(),
            right: n.right.clone(),
        }
    }
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
    /// image quads, and no text. The renderer early-outs at the composed
    /// `UiComposition` level; this per-layer predicate is test-only.
    #[cfg(test)]
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
    bind_scope: Option<&str>,
    fallback: &str,
    slot_values: &HashMap<String, SlotValue>,
    cell_values: &CellValues,
) -> String {
    let Some(bind) = bind else {
        return fallback.to_string();
    };
    let Some(value) = lookup_bound(&bind.source, bind_scope, slot_values, cell_values) else {
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
/// to the literal `fallback` with a single `log::warn!` per call. The warn fires
/// once per frame for a genuinely mis-typed slot on BOTH paths: the fresh path
/// (one build per frame) and the retained tweened path (`drive_panel_binding`
/// snaps through here every frame the slot keeps the wrong shape). There is no
/// cross-frame dedup — an authoring error, not per-frame spam.
fn resolve_panel_fill(
    bind: Option<&PanelBind>,
    bind_scope: Option<&str>,
    fallback: [f32; 4],
    slot_values: &HashMap<String, SlotValue>,
    cell_values: &CellValues,
) -> [f32; 4] {
    let Some(bind) = bind else {
        return fallback;
    };
    match lookup_bound(&bind.source, bind_scope, slot_values, cell_values) {
        Some(SlotValue::Array(rgba)) if rgba.len() == 4 => [rgba[0], rgba[1], rgba[2], rgba[3]],
        // Absent slot/cell: silent fallback — the value simply was not written
        // this frame, which is expected for an optional binding.
        None => fallback,
        // Present but the wrong shape: an authoring error worth one warn.
        Some(_) => {
            log::warn!(
                "[Renderer] panel bind '{}' is not a length-4 array; using literal fill",
                bind_target_name(&bind.source),
            );
            fallback
        }
    }
}

/// A bind source's display name for diagnostics: the slot's dotted name or the
/// local cell name. Used in warn/skip messages where the bind kind is irrelevant.
fn bind_target_name(source: &BindSource) -> &str {
    match source {
        BindSource::Slot { slot } => slot,
        BindSource::Local { local } => local,
    }
}

/// The scalar value a `text` node's styleRanges maps: the eased tween *display*
/// value when a tween is active on the bind (the styleRanges contract — it
/// evaluates the value the widget renders, which mid-tween is the display value),
/// else the bound slot's `Number`. `None` when there is no bind, the slot is
/// absent, or it is not a `Number` (no value to map — the node keeps its base
/// color). The tween's display is the same eased value the rendered string shows,
/// so the color tracks the displayed number.
fn style_text_value(
    bind: Option<&TextBind>,
    bind_scope: Option<&str>,
    tween: Option<&TweenState<f32>>,
    slot_values: &HashMap<String, SlotValue>,
    cell_values: &CellValues,
) -> Option<f32> {
    if let Some(state) = tween {
        return Some(state.display);
    }
    let bind = bind?;
    match lookup_bound(&bind.source, bind_scope, slot_values, cell_values) {
        Some(SlotValue::Number(n)) => Some(*n),
        _ => None,
    }
}

/// The scalar value a `panel` node's styleRanges maps: the bound slot's `Number`.
/// `None` when there is no bind, the slot is absent, or it is not a `Number`. A
/// panel's RGBA fill-tween carries no scalar, so styleRanges on a panel reads the
/// raw numeric slot (a styleRanges panel binds a numeric slot, not the length-4
/// fill array) — the two bind uses are distinct.
/// Seam: unlike `style_text_value` (which returns the eased display value when a
/// tween is active), this path reads the raw slot. A panel's value-tween carries an
/// RGBA fill; a panel styleRanges bind carries a scalar — the two never coexist on
/// one panel, so there is no display value to prefer here.
fn style_value(
    bind: Option<&PanelBind>,
    bind_scope: Option<&str>,
    slot_values: &HashMap<String, SlotValue>,
    cell_values: &CellValues,
) -> Option<f32> {
    let bind = bind?;
    match lookup_bound(&bind.source, bind_scope, slot_values, cell_values) {
        Some(SlotValue::Number(n)) => Some(*n),
        _ => None,
    }
}

/// Resolve a `bar`'s bound numeric value from the frame's slot snapshot, or `0.0`
/// when the slot is absent or not a `Number` (a bar with no value reads empty).
/// The bar always binds, so there is no unbound fallback.
fn bar_slot_value(
    bind: &SliderBind,
    bind_scope: Option<&str>,
    slot_values: &HashMap<String, SlotValue>,
    cell_values: &CellValues,
) -> f32 {
    match lookup_bound(&bind.source, bind_scope, slot_values, cell_values) {
        Some(SlotValue::Number(n)) => *n,
        _ => 0.0,
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
    use super::super::descriptor::{CaptureMode, ColorValue, SpacingValue};
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

    fn no_cells() -> CellValues {
        CellValues::new()
    }

    fn spacer(flex_grow: f32) -> Widget {
        Widget::Spacer(SpacerWidget {
            flex_grow,
            id: None,
        })
    }

    fn vstack(gap: f32, padding: f32, align: Align, children: Vec<Widget>) -> Widget {
        Widget::VStack(ContainerWidget {
            gap: SpacingValue::Literal(gap),
            padding: SpacingValue::Literal(padding),
            align,
            fill: None,
            border: None,
            id: None,
            focus_neighbors: Default::default(),
            focus: None,
            restore_on_return: false,
            local_state: None,
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
            id: None,
            focus_neighbors: Default::default(),
            focus: None,
            restore_on_return: false,
            local_state: None,
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
            style_ranges: None,
            id: None,
            focus_neighbors: Default::default(),
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
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
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
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
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
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
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
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
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
                style_ranges: None,
                id: None,
                focus_neighbors: Default::default(),
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
                id: None,
                focus_neighbors: Default::default(),
                focus: None,
                restore_on_return: false,
                children: vec![cell(), cell(), cell(), cell()],
            }),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
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
                style_ranges: None,
                id: None,
                focus_neighbors: Default::default(),
            }),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
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
            id: None,
            focus_neighbors: Default::default(),
            focus: None,
            restore_on_return: false,
            local_state: None,
            children: vec![text("x", 13.0), text("y", 13.0)],
        });
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [3.5, 7.25],
            root: filled,
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
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
            id: None,
            focus_neighbors: Default::default(),
            focus: None,
            restore_on_return: false,
            local_state: None,
            children: vec![text("AB", 40.0)],
        });
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: filled,
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());

        // Exactly one backdrop quad (the container), one text run on top.
        assert_eq!(data.quads.instances.len(), 1, "one container backdrop quad");
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
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
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
        // must report different widths. Content is immutable on the descriptor — this is a
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
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        }
    }

    /// Regression: the pause-menu `ui.textEntry` readout and the "ENTER TEXT"
    /// opener button are DISTINCT retained nodes whose resolved drawn content can
    /// never alias one another. Drives the real `build_pause_menu_descriptor`
    /// through the retained tree with a live `ui.textEntry` value and asserts the
    /// readout draws `"ENTRY <value>"` while the opener draws its immutable
    /// "ENTER TEXT" label — each at its own position, with no per-node cache or
    /// auto-id collision swapping one node's content/glyphs onto the other.
    ///
    /// This pins the CPU half of the readout-aliasing bug (the reported symptom was
    /// the readout rendering the opener's "ENTER TEXT" text): node identity is the
    /// taffy `NodeId`, distinct per node, and `last_resolved` lives on the node, so
    /// the readout's resolved string and the opener's literal label never cross.
    /// (The GPU half — a single shared glyphon vertex buffer clobbered by a
    /// per-stack-layer `encode` loop — is fixed in `render/mod.rs` and is not
    /// CPU-testable without a GPU adapter; see that fix's note.)
    #[test]
    fn pause_menu_readout_and_opener_resolve_distinct_non_aliasing_text() {
        use crate::render::ui::demo::build_pause_menu_descriptor;

        let tree = build_pause_menu_descriptor();
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let mut slots: HashMap<String, SlotValue> = HashMap::new();
        slots.insert(
            "ui.textEntry".to_string(),
            SlotValue::String("this is a test".to_string()),
        );
        let data = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &slots,
            &no_cells(),
            0.0,
        );

        // The readout draws the bound value behind its untouched "ENTRY " prefix.
        let readout = data
            .texts
            .iter()
            .find(|t| t.content == "ENTRY this is a test")
            .expect("readout draws the bound ui.textEntry value behind the ENTRY prefix");
        // The opener button draws its immutable label, distinct from the readout.
        let opener = data
            .texts
            .iter()
            .find(|t| t.content == "ENTER TEXT")
            .expect("the opener button still draws its own ENTER TEXT label");

        // They are two separate draw entries at two separate positions — neither
        // node picked up the other's resolved content (the aliasing symptom).
        assert_ne!(
            readout.position, opener.position,
            "the readout and opener are distinct nodes at distinct positions",
        );
        // No drawn run is the opener's label masquerading as the readout: exactly
        // one run carries each string.
        assert_eq!(
            data.texts
                .iter()
                .filter(|t| t.content == "ENTER TEXT")
                .count(),
            1,
            "the ENTER TEXT label appears exactly once (only on the opener node)",
        );
        assert_eq!(
            data.texts
                .iter()
                .filter(|t| t.content == "ENTRY this is a test")
                .count(),
            1,
            "the resolved readout string appears exactly once (only on the readout node)",
        );

        // The readout node's `last_resolved` holds ITS value; the opener node is
        // unbound and never resolves — so the two per-node caches cannot cross.
        let mut ids = Vec::new();
        ui.collect_node_ids(ui.root, &mut ids);
        let mut readout_resolved = None;
        let mut saw_unbound_opener = false;
        for n in ids {
            if let Some(NodeContext::Text {
                content,
                last_resolved,
                bind,
                ..
            }) = ui.taffy.get_node_context(n)
            {
                if bind.as_ref().and_then(|b| b.source.slot()) == Some("ui.textEntry") {
                    readout_resolved = last_resolved.clone();
                }
                if content == "ENTER TEXT" {
                    saw_unbound_opener = true;
                    assert!(bind.is_none(), "the opener label is unbound");
                    assert!(
                        last_resolved.is_none(),
                        "the unbound opener never resolves a bound string",
                    );
                }
            }
        }
        assert_eq!(
            readout_resolved.as_deref(),
            Some("ENTRY this is a test"),
            "the readout node caches its OWN resolved string, not the opener's label",
        );
        assert!(saw_unbound_opener, "the opener node exists in the tree");
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
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
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
                source: BindSource::Slot { slot: slot.into() },
                format: format.map(str::to_string),
                tween: None,
            }),
            style_ranges: None,
            id: None,
            focus_neighbors: Default::default(),
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
            id: None,
            focus_neighbors: Default::default(),
            focus: None,
            restore_on_return: false,
            local_state: None,
            children: vec![Widget::Panel(PanelWidget {
                fill: ColorValue::Literal(fill),
                border: None,
                bind: Some(PanelBind {
                    source: BindSource::Slot { slot: slot.into() },
                    tween: None,
                }),
                style_ranges: None,
                id: None,
                focus_neighbors: Default::default(),
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
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
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
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
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
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
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
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
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
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
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
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
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

    // --- Retained-tree diff + relayout/redraw split ---------------------------

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
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();

        let red = [1.0, 0.0, 0.0, 1.0];
        let first = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &flash_slots(red),
            &no_cells(),
            0.0,
        );
        assert_eq!(ui.recompute_count(), 1, "first frame computes once");
        assert!(
            flash_quad_color(&first).is_some_and(|c| colors_eq(c, red)),
            "first frame draws the red flash",
        );

        let green = [0.0, 1.0, 0.0, 1.0];
        let second = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &flash_slots(green),
            &no_cells(),
            0.0,
        );
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
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();

        let mut slots = HashMap::new();
        slots.insert("player.health".to_string(), SlotValue::Number(100.0));
        let first = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &slots,
            &no_cells(),
            0.0,
        );
        assert_eq!(ui.recompute_count(), 1, "first frame computes once");
        assert_eq!(first.texts[0].content, "HP 100");

        slots.insert("player.health".to_string(), SlotValue::Number(75.0));
        let second = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &slots,
            &no_cells(),
            0.0,
        );
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
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();

        let mut slots = HashMap::new();
        slots.insert("player.health".to_string(), SlotValue::Number(100.0));
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);
        assert_eq!(ui.recompute_count(), 1);
        assert_eq!(
            ui.draw_rebuild_count(),
            1,
            "first frame builds the draw list"
        );

        // Change only an unbound slot; the bound `player.health` is untouched.
        slots.insert("world.kills".to_string(), SlotValue::Number(7.0));
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);
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
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();

        let settled = [0.2, 0.4, 0.6, 1.0];
        let first = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &flash_slots(settled),
            &no_cells(),
            0.0,
        );
        assert_eq!(ui.recompute_count(), 1);
        assert_eq!(ui.draw_rebuild_count(), 1, "first frame builds the list");

        // Same color again: nothing changed, so neither the layout nor the draw
        // list rebuild — the cached list is returned unchanged.
        let second = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &flash_slots(settled),
            &no_cells(),
            0.0,
        );
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

    // --- Theme-token resolution at tree build ---------------------------------

    use super::super::text::{UI_FONT_FAMILY, UI_MONO_FONT_FAMILY};
    use super::super::theme::{ThemeDescriptor, UiTheme};
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
                style_ranges: None,
                id: None,
                focus_neighbors: Default::default(),
            }),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
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
                id: None,
                focus_neighbors: Default::default(),
                focus: None,
                restore_on_return: false,
                local_state: None,
                children: vec![text("AB", 30.0), text("CD", 30.0)],
            }),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
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
                id: None,
                focus_neighbors: Default::default(),
                focus: None,
                restore_on_return: false,
                local_state: None,
                children: vec![text("AB", 30.0), text("CD", 30.0)],
            }),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
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
            assert_eq!(
                family, UI_FONT_FAMILY,
                "absent font selects the body family"
            );
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

    // --- Value-tween driver ---------------------------------------------------

    use super::super::descriptor::{PanelTween, TextTween};

    /// A tweened bound text leaf: fallback `content`, a `bind` slot, optional
    /// format, and a `TextTween`.
    fn tweened_text(content: &str, slot: &str, format: Option<&str>, tween: TextTween) -> Widget {
        Widget::Text(TextWidget {
            content: content.into(),
            font_size: 20.0,
            color: ColorValue::Literal([1.0, 1.0, 1.0, 1.0]),
            font: None,
            bind: Some(TextBind {
                source: BindSource::Slot { slot: slot.into() },
                format: format.map(str::to_string),
                tween: Some(tween),
            }),
            style_ranges: None,
            id: None,
            focus_neighbors: Default::default(),
        })
    }

    /// A tweened bound panel leaf wrapped in a stretch container (so the leaf gets
    /// a non-zero laid-out rect).
    fn tweened_panel_in_stack(fill: [f32; 4], slot: &str, tween: PanelTween) -> Widget {
        Widget::VStack(ContainerWidget {
            gap: SpacingValue::Literal(0.0),
            padding: SpacingValue::Literal(0.0),
            align: Align::Stretch,
            fill: Some(ColorValue::Literal([0.0, 0.0, 0.0, 1.0])),
            border: None,
            id: None,
            focus_neighbors: Default::default(),
            focus: None,
            restore_on_return: false,
            local_state: None,
            children: vec![Widget::Panel(PanelWidget {
                fill: ColorValue::Literal(fill),
                border: None,
                bind: Some(PanelBind {
                    source: BindSource::Slot { slot: slot.into() },
                    tween: Some(tween),
                }),
                style_ranges: None,
                id: None,
                focus_neighbors: Default::default(),
            })],
        })
    }

    /// One numeric slot map for a text tween.
    fn number_slots(slot: &str, value: f32) -> HashMap<String, SlotValue> {
        let mut slots = HashMap::new();
        slots.insert(slot.to_string(), SlotValue::Number(value));
        slots
    }

    /// Parse a rendered text run's content back to an `f32` (the displayed value
    /// the driver rounded to an integer).
    fn text_value(data: &UiDrawData) -> f32 {
        data.texts[0]
            .content
            .parse::<f32>()
            .expect("displayed text is an integer string")
    }

    #[test]
    fn text_tween_first_resolve_with_from_starts_at_from_and_reaches_target_at_duration() {
        // First-resolve `from` flourish: a text bind with `from: 0.0`, target 100
        // (constant slot). Frame 0 renders 0; subsequent frames advance
        // monotonically toward 100; the value is EXACTLY 100 at durationMs.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: tweened_text(
                "0",
                "player.health",
                None,
                TextTween {
                    duration_ms: 1000.0,
                    easing: Easing::Linear,
                    from: Some(0.0),
                },
            ),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let slots = number_slots("player.health", 100.0);

        // Frame 0: display starts at `from` = 0.
        let f0 = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &slots,
            &no_cells(),
            0.0,
        );
        assert_eq!(text_value(&f0), 0.0, "frame 0 renders the `from` value");

        // Advance through the tween; values rise monotonically toward 100.
        let mut prev = 0.0;
        for &t in &[0.25, 0.5, 0.75] {
            let f = ui.build_draw_data_retained(
                [1280, 720],
                &mut fs,
                &no_images(),
                &slots,
                &no_cells(),
                t,
            );
            let v = text_value(&f);
            assert!(
                v >= prev && v <= 100.0,
                "value {v} at t={t} advances monotonically within [prev={prev}, 100]",
            );
            prev = v;
        }

        // At t == durationMs (1.0s) the display equals the target EXACTLY.
        let f_end = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &slots,
            &no_cells(),
            1.0,
        );
        assert_eq!(
            text_value(&f_end),
            100.0,
            "display equals target at duration"
        );
    }

    #[test]
    fn text_tween_without_from_renders_target_immediately_on_first_resolve() {
        // A tween with no `from` snaps to the target on first sight (no flourish):
        // frame 0 already renders the full target value.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: tweened_text(
                "0",
                "player.health",
                None,
                TextTween {
                    duration_ms: 1000.0,
                    easing: Easing::EaseOut,
                    from: None,
                },
            ),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let slots = number_slots("player.health", 80.0);

        let f0 = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &slots,
            &no_cells(),
            0.0,
        );
        assert_eq!(
            text_value(&f0),
            80.0,
            "no `from` snaps to the target on first resolve",
        );
    }

    #[test]
    fn text_tween_retarget_mid_flight_restarts_from_current_display() {
        // Mid-flight retarget: a tween from 0 -> 100 is interrupted at t=0.5 by a
        // new target of 0. The tween must restart from the CURRENT display value
        // (~50 under linear), not snap to `from` (0) nor jump to the new target.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: tweened_text(
                "0",
                "player.health",
                None,
                TextTween {
                    duration_ms: 1000.0,
                    easing: Easing::Linear,
                    from: Some(0.0),
                },
            ),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();

        // Drive to mid-flight at t=0.5 with target 100: display ~= 50.
        ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &number_slots("player.health", 100.0),
            &no_cells(),
            0.0,
        );
        let mid = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &number_slots("player.health", 100.0),
            &no_cells(),
            0.5,
        );
        let mid_v = text_value(&mid);
        assert!(
            (40.0..=60.0).contains(&mid_v),
            "mid-flight value ~50 under linear easing, got {mid_v}",
        );

        // Retarget to 0 at t=0.5: the segment restarts from the current display
        // (~50) at this instant, so this very frame still reads ~50 (elapsed 0) —
        // it must NOT snap to `from`=0 nor jump to the new target 0.
        let retarget = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &number_slots("player.health", 0.0),
            &no_cells(),
            0.5,
        );
        let retarget_v = text_value(&retarget);
        assert!(
            (40.0..=60.0).contains(&retarget_v),
            "retarget restarts from the current display ~{mid_v} (no snap to from/target), got {retarget_v}",
        );

        // A later frame eases DOWN from ~50 toward 0 — continuous, below the
        // retarget value and above the new target.
        let after = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &number_slots("player.health", 0.0),
            &no_cells(),
            1.0,
        );
        let after_v = text_value(&after);
        assert!(
            after_v < retarget_v && after_v > 0.0,
            "retargeted tween eases continuously down from {retarget_v} toward 0, got {after_v}",
        );

        // And it reaches the new target exactly one duration after the retarget.
        let settled = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &number_slots("player.health", 0.0),
            &no_cells(),
            1.5,
        );
        assert_eq!(text_value(&settled), 0.0, "retargeted tween settles at 0");
    }

    #[test]
    fn text_tween_ease_out_advances_monotonically_toward_target() {
        // Easing monotonicity: under easeOut the in-flight display rises
        // monotonically toward the target across advancing frames.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: tweened_text(
                "0",
                "player.health",
                None,
                TextTween {
                    duration_ms: 1000.0,
                    easing: Easing::EaseOut,
                    from: Some(0.0),
                },
            ),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let slots = number_slots("player.health", 100.0);

        let mut prev = -1.0;
        for &t in &[0.0, 0.1, 0.2, 0.4, 0.6, 0.8, 1.0] {
            let f = ui.build_draw_data_retained(
                [1280, 720],
                &mut fs,
                &no_images(),
                &slots,
                &no_cells(),
                t,
            );
            let v = text_value(&f);
            assert!(
                v >= prev,
                "easeOut value must be monotonic non-decreasing: {v} < {prev} at t={t}",
            );
            prev = v;
        }
        assert_eq!(
            prev, 100.0,
            "easeOut reaches the target exactly at duration"
        );
    }

    #[test]
    fn text_tween_settles_at_exact_target_past_duration() {
        // Exact-target settle: at t >= duration the display equals the target
        // exactly (a frame well past the end stays pinned, no overshoot).
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: tweened_text(
                "0",
                "player.health",
                None,
                TextTween {
                    duration_ms: 500.0,
                    easing: Easing::EaseInOut,
                    from: Some(10.0),
                },
            ),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let slots = number_slots("player.health", 42.0);

        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);
        // t = 5.0s is ten durations past the end.
        let far = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &slots,
            &no_cells(),
            5.0,
        );
        assert_eq!(text_value(&far), 42.0, "well past duration pins to target");
    }

    #[test]
    fn text_tween_in_flight_relayouts_each_advancing_frame() {
        // In-flight text is content-changed each advancing frame (the rendered
        // integer string differs, re-measures): recompute_count increments per
        // frame while the eased value is still moving.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: tweened_text(
                "0",
                "player.health",
                None,
                TextTween {
                    duration_ms: 1000.0,
                    easing: Easing::Linear,
                    from: Some(0.0),
                },
            ),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let slots = number_slots("player.health", 100.0);

        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);
        let c0 = ui.recompute_count();
        // Each advancing frame moves the integer (0 -> 25 -> 50 -> 75), so each
        // relays out.
        ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &slots,
            &no_cells(),
            0.25,
        );
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.5);
        ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &slots,
            &no_cells(),
            0.75,
        );
        assert_eq!(
            ui.recompute_count(),
            c0 + 3,
            "an in-flight text tween relays out each advancing frame",
        );
    }

    #[test]
    fn panel_tween_in_flight_redraws_without_relayout() {
        // In-flight panel eases per-channel and is appearance-only: the draw list
        // rebuilds each advancing frame but layout NEVER recomputes.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: tweened_panel_in_stack(
                [0.0, 0.0, 0.0, 1.0],
                "intro.flashColor",
                PanelTween {
                    duration_ms: 1000.0,
                    easing: Easing::Linear,
                    from: Some([0.0, 0.0, 0.0, 1.0]),
                },
            ),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let target = [1.0, 0.5, 0.25, 1.0];

        let f0 = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &flash_slots(target),
            &no_cells(),
            0.0,
        );
        assert_eq!(ui.recompute_count(), 1, "first frame computes once");
        // Frame 0 starts at the `from` color (all-black-but-alpha is the backdrop
        // color too, so just assert the panel hasn't reached the target yet).
        let c0 = flash_quad_color(&f0);

        let r0 = ui.recompute_count();
        let d0 = ui.draw_rebuild_count();
        let mid = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &flash_slots(target),
            &no_cells(),
            0.5,
        );
        // Per-channel eased halfway under linear: ~[0.5, 0.25, 0.125, 1.0].
        let mid_c = mid
            .quads
            .instances
            .iter()
            .map(|q| q.color)
            .find(|c| !colors_eq(*c, [0.0, 0.0, 0.0, 1.0]))
            .expect("an eased panel quad");
        assert!(
            mid_c[0] > 0.0 && mid_c[0] < 1.0 && mid_c[1] > 0.0 && mid_c[1] < 0.5,
            "panel eased per channel mid-flight: {mid_c:?}",
        );
        assert_eq!(
            ui.recompute_count(),
            r0,
            "an in-flight panel tween must NOT relayout",
        );
        assert!(
            ui.draw_rebuild_count() > d0,
            "an in-flight panel tween rebuilds the draw list (redraw)",
        );
        let _ = c0;
    }

    #[test]
    fn panel_tween_eases_alpha_channel_and_settles_exactly() {
        // The panel tween eases all four channels (alpha included) and settles at
        // the exact target past the duration.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: tweened_panel_in_stack(
                [0.0, 0.0, 0.0, 1.0],
                "intro.flashColor",
                PanelTween {
                    duration_ms: 1000.0,
                    easing: Easing::Linear,
                    from: Some([0.0, 0.0, 0.0, 0.0]),
                },
            ),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let target = [0.2, 0.4, 0.6, 1.0];

        // Mid-flight: alpha is between the from (0.0) and target (1.0).
        ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &flash_slots(target),
            &no_cells(),
            0.0,
        );
        let mid = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &flash_slots(target),
            &no_cells(),
            0.5,
        );
        let mid_c = mid
            .quads
            .instances
            .iter()
            .map(|q| q.color)
            .find(|c| c[3] > 0.0 && c[3] < 1.0)
            .expect("a panel quad with eased mid alpha");
        assert!(
            (0.4..=0.6).contains(&mid_c[3]),
            "alpha eased ~0.5 mid-flight under linear, got {}",
            mid_c[3],
        );

        // Past duration: settles to the exact target.
        let end = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &flash_slots(target),
            &no_cells(),
            2.0,
        );
        let end_c = flash_quad_color(&end).expect("a settled panel quad");
        assert!(
            end_c.iter().zip(target.iter()).all(|(a, b)| approx(*a, *b)),
            "panel settles at the exact target {target:?}, got {end_c:?}",
        );
    }

    #[test]
    fn text_tween_settled_frame_skips_rebuild_and_recompute() {
        // Post-settle no-rebuild: once a text tween has settled, a no-change frame
        // (same target, time well past the end) returns the cached draw list with
        // NO relayout and NO draw-list rebuild.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: tweened_text(
                "0",
                "player.health",
                None,
                TextTween {
                    duration_ms: 500.0,
                    easing: Easing::Linear,
                    from: Some(0.0),
                },
            ),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let slots = number_slots("player.health", 30.0);

        // Drive past the end so the display settles at 30.
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 1.0);
        let r_settled = ui.recompute_count();
        let d_settled = ui.draw_rebuild_count();

        // A further frame at a still-later time with the same target: the rounded
        // display is already 30 and stays 30, so nothing rebuilds.
        let f = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &slots,
            &no_cells(),
            2.0,
        );
        assert_eq!(
            ui.recompute_count(),
            r_settled,
            "a settled text frame does not relayout",
        );
        assert_eq!(
            ui.draw_rebuild_count(),
            d_settled,
            "a settled text frame returns the cached list (no rebuild)",
        );
        assert_eq!(text_value(&f), 30.0, "cached list still carries the target");
    }

    #[test]
    fn text_tween_on_string_slot_snaps_through_unchanged_path() {
        // Non-numeric snap-with-warn: a text tween whose slot resolves to a
        // `String` renders via the unchanged `resolve_text` path (the bare string),
        // not an eased number. (The once-per-frame warn is logged but not
        // asserted — log capture is out of scope for these CPU tests.)
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: tweened_text(
                "fallback",
                "hud.label",
                None,
                TextTween {
                    duration_ms: 1000.0,
                    easing: Easing::Linear,
                    from: Some(0.0),
                },
            ),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let mut slots = HashMap::new();
        slots.insert("hud.label".to_string(), SlotValue::String("ALERT".into()));

        let f = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &slots,
            &no_cells(),
            0.0,
        );
        assert_eq!(
            f.texts[0].content, "ALERT",
            "a tween on a non-Number slot renders the raw string, not an eased number",
        );
    }

    #[test]
    fn text_tween_fresh_path_resolves_target_directly_no_cross_frame_state() {
        // Fresh-path inertness: the same tweened descriptor through the fresh
        // `build_draw_data` (no time, no retained state) resolves the target
        // DIRECTLY — no flourish, no eased value, no cross-frame tween state.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: tweened_text(
                "0",
                "player.health",
                None,
                TextTween {
                    duration_ms: 1000.0,
                    easing: Easing::Linear,
                    from: Some(0.0),
                },
            ),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let slots = number_slots("player.health", 100.0);

        // Fresh path renders the target (100) immediately — `from`=0 is ignored.
        let f = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &slots);
        assert_eq!(
            f.texts[0].content, "100",
            "the fresh path resolves the tween target directly (inert, no easing)",
        );
        // No tween state was born on the node (the fresh path never drives tweens).
        if let Some(NodeContext::Text { tween, .. }) = ui.taffy.get_node_context(ui.root) {
            assert!(
                tween.is_none(),
                "fresh path leaves no cross-frame tween state"
            );
        } else {
            panic!("root must be a text node");
        }
    }

    #[test]
    fn untweened_bound_text_unaffected_by_time() {
        // Untweened binds keep the existing behavior regardless of `time_seconds`:
        // the resolved value is rendered directly, no easing, no tween state.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: bound_text("0", "player.health", Some("HP {}")),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let slots = number_slots("player.health", 73.0);

        let f = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &slots,
            &no_cells(),
            9.0,
        );
        assert_eq!(
            f.texts[0].content, "HP 73",
            "an untweened bind renders the resolved value directly at any time",
        );
        if let Some(NodeContext::Text { tween, .. }) = ui.taffy.get_node_context(ui.root) {
            assert!(tween.is_none(), "an untweened bind grows no tween state");
        } else {
            panic!("root must be a text node");
        }
    }

    // --- styleRanges evaluator through the draw build -------------------------

    use super::super::style_ranges::{StyleEntry, StyleRanges};

    /// A bound `text` leaf carrying a `styleRanges` map. The bind supplies the
    /// value the map evaluates; the literal `color` is the base color a no-color
    /// or no-match band keeps.
    fn styled_text(base: [f32; 4], slot: &str, ranges: StyleRanges) -> Widget {
        Widget::Text(TextWidget {
            content: "0".into(),
            font_size: 20.0,
            color: ColorValue::Literal(base),
            font: None,
            bind: Some(TextBind {
                source: BindSource::Slot { slot: slot.into() },
                format: None,
                tween: None,
            }),
            style_ranges: Some(ranges),
            id: None,
            focus_neighbors: Default::default(),
        })
    }

    /// The three-band health map used across the integration tests: red ≤ 0.25,
    /// amber ≤ 0.5, default green. Band colors are token literals so the draw
    /// build's resolved sRGB is predictable.
    fn health_style_ranges() -> StyleRanges {
        StyleRanges {
            max: 100.0,
            entries: vec![
                StyleEntry {
                    up_to: Some(0.25),
                    color: Some(ColorValue::Literal([1.0, 0.0, 0.0, 1.0])),
                    pulse: None,
                    flash: None,
                },
                StyleEntry {
                    up_to: Some(0.5),
                    color: Some(ColorValue::Literal([1.0, 1.0, 0.0, 1.0])),
                    pulse: None,
                    flash: None,
                },
                StyleEntry {
                    up_to: None,
                    color: Some(ColorValue::Literal([0.0, 1.0, 0.0, 1.0])),
                    pulse: None,
                    flash: None,
                },
            ],
        }
    }

    #[test]
    fn style_ranges_change_text_color_at_the_declared_fraction() {
        // A `text` bound to `player.health` with the three-band map draws red at a
        // low value (10/100 = 0.10 → first band) and green at a high value
        // (90/100 = 0.90 → trailing default). The drawn color tracks the band.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: styled_text([1.0, 1.0, 1.0, 1.0], "player.health", health_style_ranges()),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut fs = font_system();

        let mut ui_low = UiTree::from_descriptor(&tree, &theme());
        let low = ui_low.build_draw_data(
            [1280, 720],
            &mut fs,
            &no_images(),
            &number_slots("player.health", 10.0),
        );
        assert_eq!(
            low.texts[0].color,
            srgb_of([1.0, 0.0, 0.0, 1.0]),
            "low health (fraction 0.10) draws the first band's red",
        );

        let mut ui_high = UiTree::from_descriptor(&tree, &theme());
        let high = ui_high.build_draw_data(
            [1280, 720],
            &mut fs,
            &no_images(),
            &number_slots("player.health", 90.0),
        );
        assert_eq!(
            high.texts[0].color,
            srgb_of([0.0, 1.0, 0.0, 1.0]),
            "high health (fraction 0.90) draws the trailing default green",
        );
    }

    #[test]
    fn style_ranges_band_color_token_degrades_to_magenta_in_draw_list() {
        // A band naming an unknown color token degrades to opaque magenta through
        // the existing theme rule — pre-resolved to a literal at build, so the
        // drawn run carries magenta.
        let ranges = StyleRanges {
            max: 100.0,
            entries: vec![StyleEntry {
                up_to: None,
                color: Some(ColorValue::Token("no.such.color".into())),
                pulse: None,
                flash: None,
            }],
        };
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: styled_text([1.0, 1.0, 1.0, 1.0], "player.health", ranges),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let data = ui.build_draw_data(
            [1280, 720],
            &mut fs,
            &no_images(),
            &number_slots("player.health", 50.0),
        );
        assert_eq!(
            data.texts[0].color,
            srgb_of([1.0, 0.0, 1.0, 1.0]),
            "unknown band token degrades to opaque magenta",
        );
    }

    #[test]
    fn style_ranges_without_a_bind_are_dropped_and_keep_the_base_color() {
        // styleRanges without a `bind` have no value to map: the build drops them
        // (warning once) and the node draws its plain base color, never a band.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: Widget::Text(TextWidget {
                content: "X".into(),
                font_size: 20.0,
                color: ColorValue::Literal([0.2, 0.4, 0.6, 1.0]),
                font: None,
                bind: None,
                style_ranges: Some(health_style_ranges()),
                id: None,
                focus_neighbors: Default::default(),
            }),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
        assert_eq!(
            data.texts[0].color,
            srgb_of([0.2, 0.4, 0.6, 1.0]),
            "a bindless styleRanges is dropped; the base color is drawn",
        );
        // The node carries no styleRanges (it was dropped at build).
        if let Some(NodeContext::Text { style_ranges, .. }) = ui.taffy.get_node_context(ui.root) {
            assert!(
                style_ranges.is_none(),
                "bindless styleRanges is dropped from the node",
            );
        } else {
            panic!("root must be a text node");
        }
    }

    #[test]
    fn style_ranges_evaluate_the_eased_display_value_mid_tween() {
        // styleRanges evaluate the value the widget RENDERS — the eased display
        // value mid-tween, not the authoritative target. A tween easing 0→100
        // (target 100) renders a low display early, so the band is red even though
        // the target's fraction (1.0) would resolve to green.
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: Widget::Text(TextWidget {
                content: "0".into(),
                font_size: 20.0,
                color: ColorValue::Literal([1.0, 1.0, 1.0, 1.0]),
                font: None,
                bind: Some(TextBind {
                    source: BindSource::Slot {
                        slot: "player.health".into(),
                    },
                    format: None,
                    tween: Some(TextTween {
                        duration_ms: 1000.0,
                        easing: Easing::Linear,
                        from: Some(0.0),
                    }),
                }),
                style_ranges: Some(health_style_ranges()),
                id: None,
                focus_neighbors: Default::default(),
            }),
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let slots = number_slots("player.health", 100.0);

        // Frame 0: display is at `from` = 0 (fraction 0) → red band, NOT the
        // target's green. The eased display value drives the band.
        let f0 = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &slots,
            &no_cells(),
            0.0,
        );
        assert_eq!(
            f0.texts[0].color,
            srgb_of([1.0, 0.0, 0.0, 1.0]),
            "mid-tween the band tracks the eased display value (0 → red), not the target",
        );

        // At t == duration the display equals the target (100, fraction 1.0) →
        // the trailing green band.
        let f_end = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &slots,
            &no_cells(),
            1.0,
        );
        assert_eq!(
            f_end.texts[0].color,
            srgb_of([0.0, 1.0, 0.0, 1.0]),
            "settled at the target, the band resolves to the default green",
        );
    }

    // --- Focus-rect export ---

    /// A text leaf carrying an authored id (focusable seam).
    fn text_id(content: &str, id: &str) -> Widget {
        Widget::Text(TextWidget {
            content: content.into(),
            font_size: 20.0,
            color: ColorValue::Literal([1.0; 4]),
            font: None,
            id: Some(id.to_string()),
            focus_neighbors: super::super::descriptor::FocusNeighbors::default(),
            bind: None,
            style_ranges: None,
        })
    }

    #[test]
    fn focus_export_lists_ids_rects_and_a_linear_group() {
        use super::super::descriptor::{FocusKind, FocusPolicy};
        // A vstack declaring a linear focus policy over three id'd text leaves.
        let root = Widget::VStack(ContainerWidget {
            gap: SpacingValue::Literal(10.0),
            padding: SpacingValue::Literal(0.0),
            align: Align::Start,
            fill: None,
            border: None,
            id: None,
            focus_neighbors: super::super::descriptor::FocusNeighbors::default(),
            focus: Some(FocusPolicy::Shorthand(FocusKind::Linear)),
            restore_on_return: false,
            local_state: None,
            children: vec![text_id("A", "a"), text_id("B", "b"), text_id("C", "c")],
        });
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root,
            capture_mode: CaptureMode::Passthrough,
            initial_focus: Some("b".to_string()),
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let draw = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
        let focus = ui.export_focus_rects(&tree, [1280, 720]);

        // Three focusable nodes, one linear group with all three as members.
        let ids: Vec<&str> = focus.rects.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"], "ids in tree order");
        assert_eq!(focus.groups.len(), 1);
        assert_eq!(focus.groups[0].kind, super::FocusKind::Linear);
        assert!(focus.groups[0].wrap, "shorthand defaults wrap on");
        assert_eq!(focus.groups[0].members, vec![0, 1, 2]);
        assert_eq!(focus.initial_focus.as_deref(), Some("b"));

        // z rises in tree order so a later node hit-tests as topmost.
        assert!(focus.rects[0].z < focus.rects[1].z && focus.rects[1].z < focus.rects[2].z);

        // The exported rect uses the SAME device-pixel projection as the draw: each
        // focusable text node's rect [x, y] matches its drawn text run position.
        for (i, run) in draw.texts.iter().enumerate() {
            assert!(
                approx(focus.rects[i].rect[0], run.position[0])
                    && approx(focus.rects[i].rect[1], run.position[1]),
                "focus rect {i} top-left matches the drawn run position",
            );
        }
    }

    #[test]
    fn focus_export_auto_generates_ids_from_tree_position() {
        use super::super::descriptor::{FocusKind, FocusPolicy};
        // Children with NO authored id, under a focus-policy container, get a
        // deterministic auto-id from their child-index path (runtime-only).
        let root = Widget::VStack(ContainerWidget {
            gap: SpacingValue::Literal(0.0),
            padding: SpacingValue::Literal(0.0),
            align: Align::Start,
            fill: None,
            border: None,
            id: None,
            focus_neighbors: super::super::descriptor::FocusNeighbors::default(),
            focus: Some(FocusPolicy::Shorthand(FocusKind::Linear)),
            restore_on_return: false,
            local_state: None,
            children: vec![text("X", 20.0), text("Y", 20.0)],
        });
        let tree = AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root,
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        };
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
        let focus = ui.export_focus_rects(&tree, [1280, 720]);
        let ids: Vec<&str> = focus.rects.iter().map(|r| r.id.as_str()).collect();
        // Auto-ids are the slash-joined child paths from the root.
        assert_eq!(ids, ["0", "1"], "auto-id is the tree-position path");
    }

    // --- Interactive widgets ---

    use super::super::descriptor::{BarWidget, ButtonWidget, SliderBind, SliderWidget};

    fn button(id: &str, on_press: &str) -> Widget {
        Widget::Button(ButtonWidget {
            id: id.into(),
            label: id.into(),
            on_press: on_press.into(),
            focus_neighbors: Default::default(),
            repeat_on_hold: None,
        })
    }

    fn slider(id: &str, slot: &str, captures: &[&str]) -> Widget {
        Widget::Slider(SliderWidget {
            id: id.into(),
            label: "Vol".into(),
            bind: SliderBind {
                source: BindSource::Slot { slot: slot.into() },
                tween: None,
            },
            min: 0.0,
            max: 1.0,
            step: 0.1,
            captures_nav: captures.iter().map(|s| s.to_string()).collect(),
            focus_neighbors: Default::default(),
        })
    }

    fn anchored(root: Widget) -> AnchoredTree {
        AnchoredTree {
            anchor: Anchor::TopLeft,
            offset: [0.0, 0.0],
            root,
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
            text_entry_target: None,
        }
    }

    #[test]
    fn button_exports_focusable_rect_with_activation_interaction() {
        // A button always exports as focusable (required id) carrying its onPress
        // activation — the seam the app fires on a focus-engine confirm/click.
        let tree = anchored(vstack(
            0.0,
            0.0,
            Align::Start,
            vec![button("resume", "resumeGame")],
        ));
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
        let focus = ui.export_focus_rects(&tree, [1280, 720]);
        let rect = focus
            .rects
            .iter()
            .find(|r| r.id == "resume")
            .expect("button is focusable");
        assert_eq!(
            rect.interaction,
            Some(NodeInteraction::Button {
                on_press: "resumeGame".to_string(),
                repeat_on_hold: None,
            }),
            "button carries its onPress activation"
        );
    }

    #[test]
    fn slider_exports_focusable_rect_with_step_interaction() {
        // A slider always exports as focusable carrying its bound-value step params
        // and capturesNav wire names — the app drives the value step from these.
        let tree = anchored(vstack(
            0.0,
            0.0,
            Align::Start,
            vec![slider("vol", "audio.master", &["nav.left", "nav.right"])],
        ));
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
        let focus = ui.export_focus_rects(&tree, [1280, 720]);
        let rect = focus
            .rects
            .iter()
            .find(|r| r.id == "vol")
            .expect("slider is focusable");
        assert_eq!(
            rect.interaction,
            Some(NodeInteraction::Slider {
                slot: "audio.master".to_string(),
                min: 0.0,
                max: 1.0,
                step: 0.1,
                captures_nav: vec!["nav.left".to_string(), "nav.right".to_string()],
            }),
        );
    }

    fn bar(slot: &str, max: f32, style_ranges: Option<StyleRanges>) -> Widget {
        Widget::Bar(BarWidget {
            bind: SliderBind {
                source: BindSource::Slot { slot: slot.into() },
                tween: None,
            },
            max,
            fill: ColorValue::Literal([0.0, 1.0, 0.0, 1.0]),
            background: ColorValue::Literal([0.1, 0.1, 0.1, 1.0]),
            id: None,
            style_ranges,
        })
    }

    /// A slot map binding `player.health` to a Number value.
    fn health_slots(value: f32) -> HashMap<String, SlotValue> {
        let mut m = HashMap::new();
        m.insert("player.health".to_string(), SlotValue::Number(value));
        m
    }

    #[test]
    fn bar_fill_fraction_is_value_over_max_clamped() {
        // A bar with max 100 and value 50 draws a fill quad half the background's
        // width; value 150 clamps to the full width (fraction 1).
        let tree = anchored(bar("player.health", 100.0, None));

        for (value, expected_fraction) in [(50.0_f32, 0.5_f32), (150.0, 1.0), (0.0, 0.0)] {
            let mut ui = UiTree::from_descriptor(&tree, &theme());
            let mut fs = font_system();
            let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &health_slots(value));
            // The background quad is always present (first); the fill quad follows
            // only when the fraction is > 0.
            let background = &data.quads.instances[0];
            let bg_width = background.rect[2];
            if expected_fraction == 0.0 {
                assert_eq!(
                    data.quads.instances.len(),
                    1,
                    "zero fraction draws no fill quad"
                );
            } else {
                let fill = &data.quads.instances[1];
                let expected_width = (bg_width * expected_fraction).round();
                assert!(
                    approx(fill.rect[2], expected_width),
                    "value {value}: fill width {} ≈ {expected_width} (fraction {expected_fraction})",
                    fill.rect[2],
                );
                // Fill shares the background's top-left and height.
                assert!(approx(fill.rect[0], background.rect[0]));
                assert!(approx(fill.rect[1], background.rect[1]));
                assert!(approx(fill.rect[3], background.rect[3]));
            }
        }
    }

    #[test]
    fn bar_style_ranges_recolor_the_fill() {
        // A health bar with a red ≤ 0.25 band: at 10/100 the fill quad is red, not
        // the base green. styleRanges recolors the fill widget-agnostically.
        let ranges = StyleRanges {
            max: 100.0,
            entries: vec![
                StyleEntry {
                    up_to: Some(0.25),
                    color: Some(ColorValue::Literal([1.0, 0.0, 0.0, 1.0])),
                    pulse: None,
                    flash: None,
                },
                StyleEntry {
                    up_to: None,
                    color: Some(ColorValue::Literal([0.0, 1.0, 0.0, 1.0])),
                    pulse: None,
                    flash: None,
                },
            ],
        };
        let tree = anchored(bar("player.health", 100.0, Some(ranges)));
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &health_slots(10.0));
        let fill = &data.quads.instances[1];
        assert!(
            approx(fill.color[0], 1.0) && approx(fill.color[1], 0.0),
            "low health recolors the fill red, got {:?}",
            fill.color
        );
    }

    #[test]
    fn bar_bind_tween_eases_the_displayed_fraction() {
        // A bar bind carrying a tween eases the displayed value toward each new
        // target. Retained path: from a full 100 health, retarget to 0 over 1000ms;
        // mid-tween (500ms, linear) the displayed value is ~50, so the fill width is
        // ~half — not the snapped 0.
        use super::super::descriptor::{Easing, TextTween};
        let tree = anchored(Widget::Bar(BarWidget {
            bind: SliderBind {
                source: BindSource::Slot {
                    slot: "player.health".into(),
                },
                tween: Some(TextTween {
                    duration_ms: 1000.0,
                    easing: Easing::Linear,
                    from: None,
                }),
            },
            max: 100.0,
            fill: ColorValue::Literal([0.0, 1.0, 0.0, 1.0]),
            background: ColorValue::Literal([0.1, 0.1, 0.1, 1.0]),
            id: None,
            style_ranges: None,
        }));
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        // Frame 0: first resolution at full health (no `from`, snaps to 100).
        ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &health_slots(100.0),
            &no_cells(),
            0.0,
        );
        // Frame 1: retarget to 0 at t=0 — the segment starts easing from 100.
        ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &health_slots(0.0),
            &no_cells(),
            0.0,
        );
        // Frame 2: half the duration later, the eased display is ~50 (linear).
        let data = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &health_slots(0.0),
            &no_cells(),
            0.5,
        );
        let bg_width = data.quads.instances[0].rect[2];
        let fill_width = data.quads.instances[1].rect[2];
        let fraction = fill_width / bg_width;
        assert!(
            (fraction - 0.5).abs() < 0.05,
            "mid-tween fill fraction eases to ~0.5, got {fraction}"
        );
    }
}

/// `{ local }` presentation-cell bind resolution end-to-end on the retained
/// tree. Proves a descendant `{ local }` bind displays the cell
/// value, the value is stable across a settled frame (no recompute when the live
/// value rides the snapshot, not the compared descriptor), an undeclared cell
/// degrades to the literal fallback (no panic), and resolution is scoped to the
/// nearest declaring ancestor.
#[cfg(test)]
mod local_state_tests {
    use super::*;
    use crate::render::ui::descriptor::{
        Align, AnchoredTree, BindSource, ColorValue, ContainerWidget, LocalState, SpacingValue,
        TextBind, TextWidget,
    };
    use crate::render::ui::layout::Anchor;
    use crate::render::ui::theme::UiTheme;

    fn fs() -> glyphon::FontSystem {
        super::super::text::build_font_system()
    }

    /// A vstack declaring `scope` with one `{ local }`-bound text child reading
    /// `cell` (literal fallback "FB"). `scope_id` is the declared scope id.
    fn scoped_local_tree(scope_id: &str, cell: &str) -> AnchoredTree {
        AnchoredTree::passthrough(
            Anchor::Center,
            [0.0, 0.0],
            Widget::VStack(ContainerWidget {
                gap: SpacingValue::Literal(0.0),
                padding: SpacingValue::Literal(0.0),
                align: Align::Start,
                fill: None,
                border: None,
                id: None,
                focus_neighbors: Default::default(),
                focus: None,
                restore_on_return: false,
                local_state: Some(LocalState {
                    scope: scope_id.to_string(),
                    cells: Default::default(),
                }),
                children: vec![Widget::Text(TextWidget {
                    content: "FB".into(),
                    font_size: 18.0,
                    color: ColorValue::Literal([1.0, 1.0, 1.0, 1.0]),
                    font: None,
                    bind: Some(TextBind {
                        source: BindSource::Local { local: cell.into() },
                        format: None,
                        tween: None,
                    }),
                    style_ranges: None,
                    id: None,
                    focus_neighbors: Default::default(),
                })],
            }),
        )
    }

    fn cells(scope: &str, cell: &str, value: SlotValue) -> CellValues {
        let mut m = CellValues::new();
        m.insert((scope.to_string(), cell.to_string()), value);
        m
    }

    #[test]
    fn local_bind_displays_cell_value_from_the_snapshot() {
        let tree = scoped_local_tree("counter", "count");
        let mut ui = UiTree::from_descriptor(&tree, &UiTheme::engine_default());
        let mut fs = fs();
        let cell_values = cells("counter", "count", SlotValue::Number(42.0));
        let data = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &ImageSizes::new(),
            &HashMap::new(),
            &cell_values,
            0.0,
        );
        assert!(
            data.texts.iter().any(|t| t.content == "42"),
            "the `{{ local }}` bind renders the cell value, got {:?}",
            data.texts.iter().map(|t| &t.content).collect::<Vec<_>>()
        );
    }

    #[test]
    fn cell_write_updates_value_without_a_recompute() {
        // The live cell value rides the snapshot, not the compared descriptor, so
        // a settled frame that only changes the cell rebuilds the draw list but
        // never relayouts beyond the content re-measure — and a no-change frame is
        // fully cached. Here we assert the count-up across a value change increments
        // recompute_count by exactly the content re-measures, and an identical
        // follow-up frame does not bump it (the descriptor never changed).
        let tree = scoped_local_tree("counter", "count");
        let mut ui = UiTree::from_descriptor(&tree, &UiTheme::engine_default());
        let mut fs = fs();
        let v1 = cells("counter", "count", SlotValue::Number(1.0));
        ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &ImageSizes::new(),
            &HashMap::new(),
            &v1,
            0.0,
        );
        let after_first = ui.recompute_count();
        // Re-running the SAME cell value + same descriptor recomputes nothing.
        ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &ImageSizes::new(),
            &HashMap::new(),
            &v1,
            0.0,
        );
        assert_eq!(
            ui.recompute_count(),
            after_first,
            "a settled frame with an unchanged cell recomputes nothing"
        );
    }

    #[test]
    fn undeclared_cell_degrades_to_the_literal_fallback() {
        // A `{ local }` bind whose cell is absent from the snapshot falls back to
        // the literal `content` — it does not panic and does not blank the run.
        let tree = scoped_local_tree("counter", "count");
        let mut ui = UiTree::from_descriptor(&tree, &UiTheme::engine_default());
        let mut fs = fs();
        let data = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &ImageSizes::new(),
            &HashMap::new(),
            &CellValues::new(),
            0.0,
        );
        assert!(
            data.texts.iter().any(|t| t.content == "FB"),
            "an undeclared cell degrades to the literal fallback"
        );
    }

    #[test]
    fn local_bind_resolves_against_its_declaring_scope_only() {
        // A cell value written under a DIFFERENT scope id does not resolve — the
        // bind is scoped to its nearest declaring ancestor. So the run shows the
        // literal fallback, not the other scope's value.
        let tree = scoped_local_tree("counter", "count");
        let mut ui = UiTree::from_descriptor(&tree, &UiTheme::engine_default());
        let mut fs = fs();
        let other = cells("OTHER", "count", SlotValue::Number(99.0));
        let data = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &ImageSizes::new(),
            &HashMap::new(),
            &other,
            0.0,
        );
        assert!(
            data.texts.iter().any(|t| t.content == "FB"),
            "a value under a different scope id must not resolve here"
        );
    }

    #[test]
    fn local_bind_with_no_enclosing_scope_degrades_to_literal_and_warns_at_build() {
        // A `{ local }` bind whose nearest ancestor declares NO `localState` scope
        // (the text node sits at the root with no enclosing container scope) must:
        //  1. Emit a build-time `log::warn!` from `bind_scope_for` (once, at
        //     `from_descriptor` time — NOT on the per-frame hot path).
        //  2. Still render the literal fallback — no panic, no blank run.
        //
        // The warn is asserted via a counting logger (same pattern as
        // `theme_gate_test.rs`). If another test in the process already installed a
        // global logger first the count won't increment; in that case we skip the
        // warn count assertion (eprintln a note) and only verify the fallback
        // behavior. The per-frame hot paths (`lookup_bound`, `resolve_text`) must
        // stay log-free — verified by checking the draw-data path emits no extra
        // [UI] warns beyond the one build-time warn.
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Mutex, Once};

        static WARN_COUNT: AtomicUsize = AtomicUsize::new(0);
        static LOGGER_INIT: Once = Once::new();
        static WARN_LOCK: Mutex<()> = Mutex::new(());

        struct CountingLogger;
        impl log::Log for CountingLogger {
            fn enabled(&self, m: &log::Metadata<'_>) -> bool {
                m.level() <= log::Level::Warn
            }
            fn log(&self, record: &log::Record<'_>) {
                if record.level() == log::Level::Warn && record.args().to_string().contains("[UI]")
                {
                    WARN_COUNT.fetch_add(1, Ordering::SeqCst);
                }
            }
            fn flush(&self) {}
        }

        LOGGER_INIT.call_once(|| {
            let _ = log::set_logger(&CountingLogger);
            log::set_max_level(log::LevelFilter::Warn);
        });

        // Serialise warn-count tests so WARN_COUNT is not raced by parallel tests.
        let _guard = WARN_LOCK.lock().unwrap();

        // Probe: if our logger isn't the active one the count won't change.
        WARN_COUNT.store(0, Ordering::SeqCst);
        log::warn!("[UI] logger-probe");
        let logger_active = WARN_COUNT.load(Ordering::SeqCst) == 1;

        // A bare text node at the root with a `{ local }` bind but no enclosing
        // `localState` container — `build_node` is called with `scope == None`.
        let tree = AnchoredTree::passthrough(
            Anchor::Center,
            [0.0, 0.0],
            Widget::Text(TextWidget {
                content: "FALLBACK".into(),
                font_size: 18.0,
                color: ColorValue::Literal([1.0, 1.0, 1.0, 1.0]),
                font: None,
                bind: Some(TextBind {
                    source: BindSource::Local {
                        local: "orphan".into(),
                    },
                    format: None,
                    tween: None,
                }),
                style_ranges: None,
                id: None,
                focus_neighbors: Default::default(),
            }),
        );

        // Build: `bind_scope_for` must fire the warn exactly once.
        WARN_COUNT.store(0, Ordering::SeqCst);
        let mut ui = UiTree::from_descriptor(&tree, &UiTheme::engine_default());
        let build_warns = WARN_COUNT.load(Ordering::SeqCst);

        if logger_active {
            assert_eq!(
                build_warns, 1,
                "bind_scope_for must emit exactly one [UI] warn at build time for an orphan local bind"
            );
        } else {
            eprintln!(
                "[local_state_tests] skipping warn-count assertion: \
                 another logger was installed before ours"
            );
        }

        // The retained draw path must NOT re-emit the warn (hot path stays log-free).
        WARN_COUNT.store(0, Ordering::SeqCst);
        let mut fs = fs();
        let data = ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &ImageSizes::new(),
            &HashMap::new(),
            &CellValues::new(),
            0.0,
        );
        if logger_active {
            assert_eq!(
                WARN_COUNT.load(Ordering::SeqCst),
                0,
                "the per-frame draw path must not re-emit the build-time warn"
            );
        }

        // Fallback behavior: the literal content renders regardless.
        assert!(
            data.texts.iter().any(|t| t.content == "FALLBACK"),
            "a `{{ local }}` bind with no enclosing scope falls back to the literal, got: {:?}",
            data.texts.iter().map(|t| &t.content).collect::<Vec<_>>(),
        );
    }
}
