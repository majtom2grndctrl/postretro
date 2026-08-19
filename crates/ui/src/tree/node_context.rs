// Per-node draw payload (`NodeContext`) and reactive-visibility state
// (`VisibilityState`) carried alongside each taffy node in the retained UI tree.
// See: context/lib/ui.md §1 (retained tree), §3 (display vs. authoritative value)

use std::cell::RefCell;

use taffy::prelude::Display;

use super::super::descriptor::{
    BarExitFade, BarMax, Border, BoundScalar, PanelBind, Predicate, SliderBind, TextBind,
};
use super::super::style_ranges::{StyleEffectState, StyleRanges};
use super::style::TweenState;

/// One resolved radial scalar. Literals are immutable draw values; a bound
/// scalar retains the descriptor source plus presentation-local display state.
///
/// `last_resolved` is deliberately raw and unclamped. The collector applies the
/// ring's radius/thickness/sweep limits only while emitting draw data, so an
/// over-range eased value can later return in-range without being stuck at the
/// previous draw clamp.
#[derive(Debug, Clone)]
pub enum RingScalar {
    Literal(f32),
    Bound {
        source: BoundScalar,
        /// Nearest declaring `localState` scope for a `{ local }` source.
        /// Slot sources need no scope.
        bind_scope: Option<String>,
        /// Last raw source or eased display value observed by the retained diff.
        last_resolved: Option<f32>,
        /// Born on first numeric resolution when the source carries a tween.
        tween: Option<TweenState<f32>>,
    },
}

/// Per-node draw payload carried alongside each taffy node. Pure layout nodes
/// (stacks, grids, spacers) carry `None`; only nodes that emit a draw entry hold
/// data here. taffy owns the geometry; this owns "what to draw in that rect".
#[derive(Debug, Clone)]
pub enum NodeContext {
    /// Shaped-text run. `color` is linear RGBA from the descriptor; the draw-list
    /// build converts it to text-renderer `[u8; 4]` sRGB. Carries its own `font_size`
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
        /// from the `text` widget's `font` token (or the `primary` token when the
        /// widget names none) at tree-build time, so the measure seam and the
        /// draw step both select the same registered face. See `resolve_font`.
        family: String,
        bind: Option<TextBind>,
        /// The nearest declaring `localState` scope id resolved at build time, for
        /// a `{ local }` bind. `None` for a `{ slot }`/`{ fact }` bind or a
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
        /// Optional `Predicate` styleRanges value source (M13 G2 — a button's
        /// `bind`). When `Some`, the styleRanges value is the predicate resolved to
        /// `0.0`/`1.0` (the author-wired self-highlight), taking priority over the
        /// `bind` slot/tween path. `None` for an ordinary `text` node, whose
        /// styleRanges read the bound numeric slot. `predicate_scope` is the
        /// nearest declaring `localState` scope for a `{ local }` predicate.
        predicate_bind: Option<Predicate>,
        predicate_scope: Option<String>,
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
        /// `NodeContext::Text::bind_scope`). `None` for slot/fact binds and backdrops.
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
        max: BarMax,
        fill: [f32; 4],
        background: [f32; 4],
        /// Optional retained exit policy. Only a Bar carries this narrow
        /// lifecycle presentation behavior; it is not a generic opacity API.
        exit_fade: Option<BarExitFade>,
        /// Nearest declaring `localState` scope id for a `{ local }` bind (see
        /// `NodeContext::Text::bind_scope`). `None` for a slot/fact bind.
        bind_scope: Option<String>,
        /// Last resolved (or eased) value the diff observed, for change detection
        /// and to feed the draw the eased display fraction. `None` until first diff.
        last_resolved: Option<f32>,
        /// Last resolved denominator for `max`, including `BarMax::State`. A
        /// state-backed max changes only the bar's fill width/style band, so it is
        /// an appearance dependency just like the bound value.
        last_max_resolved: Option<f32>,
        /// Live tween state when `bind`'s `tween` is `Some` and the slot has
        /// resolved to a `Number` at least once. While `Some`, the driver eases the
        /// displayed value toward the slot target; the draw reads `display`.
        tween: Option<TweenState<f32>>,
        style_ranges: Option<StyleRanges>,
        style_state: RefCell<StyleEffectState>,
    },
    /// Passive SDF annulus/arc. Colors resolve at tree-build time, keeping the
    /// frame draw walk theme-free. Its radial values are fixed-layout,
    /// appearance-only drivers: their raw sources ease independently while the
    /// collector applies draw-only range clamps.
    Ring {
        diameter: f32,
        radius: RingScalar,
        thickness: RingScalar,
        start_angle: RingScalar,
        sweep: RingScalar,
        fill: [f32; 4],
        track: Option<[f32; 4]>,
    },
}

/// Reactive-visibility state for one node carrying a `visibleWhen` predicate. The
/// predicate + its build-time `localState` scope are immutable; `prev` is the last
/// resolved value (`None` until the first diff, so the first frame always applies
/// the resolved state). Stored on `UiTree::visibility` — never inside
/// `NodeContext`, since a `visibleWhen` may sit on a pure layout container (a
/// stack/grid) that carries no draw context. Keeping it off the descriptor walk
/// is the invariant: visibility is a layout/draw/focus concern only, so a hidden
/// subtree never tears down its `localState` cells.
#[derive(Debug, Clone)]
pub struct VisibilityState {
    pub predicate: Predicate,
    /// Nearest declaring `localState` scope for a `{ local }` predicate; `None`
    /// for a `{ slot }` or `{ fact }` predicate, or a local predicate with no
    /// enclosing scope.
    pub scope: Option<String>,
    /// The node's authored `Display` when visible — `Flex` for a stack/leaf,
    /// `Grid` for a grid container. Restored on a hide→show flip so a grid's
    /// track layout survives the round-trip (restoring a grid to `Flex` would
    /// corrupt it). Captured at build from the just-set taffy style.
    pub visible_display: Display,
    /// Last resolved `0.0`/`1.0`. `None` before the first diff resolves it.
    pub prev: Option<f32>,
    /// Runtime state for a `Bar`'s narrow exit fade. `None` for all other
    /// reactive nodes and plain bars. The capture is presentation-local: it is
    /// sourced after binding resolution and never writes gameplay state.
    pub bar_exit_fade: Option<BarExitFadeState>,
}

/// Retained, linear exit presentation for one bar. A false first resolution
/// never initializes `started_at`, so the bar begins hidden; a true→false flip
/// captures the displayed numerator/denominator and retains them until expiry.
#[derive(Debug, Clone)]
pub struct BarExitFadeState {
    pub duration_seconds: f64,
    pub started_at: Option<f64>,
    pub captured_value: Option<f32>,
    pub captured_max: Option<f32>,
}

impl BarExitFadeState {
    pub fn new(duration_ms: f32) -> Self {
        Self {
            duration_seconds: f64::from(duration_ms) / 1_000.0,
            started_at: None,
            captured_value: None,
            captured_max: None,
        }
    }

    pub fn alpha_at(&self, now: f64) -> Option<f32> {
        let started_at = self.started_at?;
        let progress = ((now - started_at) / self.duration_seconds).clamp(0.0, 1.0) as f32;
        Some(1.0 - progress)
    }

    pub fn clear(&mut self) {
        self.started_at = None;
        self.captured_value = None;
        self.captured_max = None;
    }
}
