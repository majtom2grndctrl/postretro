// Serde descriptor model for the UI widget tree: the internally-tagged `Widget`
// enum (seven kinds), its field structs, and the `AnchoredTree` placement
// envelope. Pure data — no rendering, no taffy, no retained tree.
// See: context/lib/ui.md

use serde::{Deserialize, Serialize};

use super::layout::Anchor;
use super::style_ranges::StyleRanges;

/// A color slot on a widget: either a literal linear-RGBA value or a named theme
/// token resolved against the active theme (resolution is a later step — the wire
/// model only records which form was authored). Untagged so the wire form stays a
/// bare array (`[r, g, b, a]`) or a bare string (`"critical"`), with no wrapper
/// object — every pre-theming literal descriptor round-trips byte-identically.
///
/// `Literal` is declared FIRST: serde tries untagged variants in declaration
/// order, and a JSON array can only match `Literal`, a JSON string only `Token`,
/// so the forms are disjoint and the ordering merely pins the array path first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ColorValue {
    Literal([f32; 4]),
    Token(String),
}

/// A spacing slot (gap/padding) on a container: either a literal logical-px value
/// or a named theme token. Untagged so the wire form stays a bare JSON number
/// (`4.0`) or a bare string (`"tight"`). `Literal` wraps a bare `f32` (no newtype)
/// so a literal re-serializes as the same number the pre-theming fixtures emit.
///
/// `Literal` is declared FIRST for the same reason as `ColorValue`: a JSON number
/// can only match `Literal`, a JSON string only `Token`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SpacingValue {
    Literal(f32),
    Token(String),
}

/// Whether a tree captures input (freezing gameplay + lower trees and releasing
/// the cursor) or passes it through to gameplay. Declared on the `AnchoredTree`
/// envelope so a JSON-authored tree states its own behavior. `Passthrough` is the
/// default — a HUD never captures, and an omitted `captureMode` keeps the pre-F
/// wire form byte-identical (`skip_serializing_if` below). The app-side modal
/// stack reads the TOP tree's mode to drive the input-dispatch seam and focus.
///
/// This is the descriptor/wire twin of `input::UiCaptureMode`; the modal stack
/// converts one to the other via `into()`. Kept separate so the descriptor module
/// carries no input-subsystem dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureMode {
    /// Tree consumes input; gameplay + lower trees freeze, cursor releases.
    Capture,
    /// Tree ignores input; events flow through to gameplay (HUD behavior).
    #[default]
    Passthrough,
}

/// How a container moves focus among its children (M13 Goal F, Task 3). Authored
/// on a container as the additive `focus` field — an untagged union so the wire
/// form is either a bare string (`"linear"` / `"spatial"`) or an object carrying
/// the policy plus optional `wrap`/`repeat`. The string forms are shorthand for
/// the object with default `wrap`/`repeat`. The focus engine (app-side) reads the
/// resolved policy off the exported focus-rect list to move focus through the tree.
///
/// `Shorthand` is declared FIRST so a bare JSON string lands on it (untagged
/// variants are tried in declaration order; a string can only match `Shorthand`,
/// an object only `Detailed`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FocusPolicy {
    /// Bare-string shorthand: `"linear"` or `"spatial"`, default wrap, no repeat.
    Shorthand(FocusKind),
    /// Object form: the policy kind plus optional `wrap` and hold-to-repeat config.
    Detailed {
        policy: FocusKind,
        /// Whether directional/next-prev nav wraps past the ends (defaults true).
        #[serde(default = "default_wrap", skip_serializing_if = "is_true")]
        wrap: bool,
        /// Hold-to-repeat timing for held directions; absent means no repeat.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repeat: Option<RepeatPolicy>,
    },
}

/// The two focus-traversal kinds. `Linear` walks the container's children in tree
/// order; `Spatial` picks the nearest child center in the pressed direction's
/// half-plane (grid navigation). Maps to the camelCase wire literals `"linear"` /
/// `"spatial"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FocusKind {
    Linear,
    Spatial,
}

/// Hold-to-repeat timing for a container's directional nav (M13 Goal F, Task 3).
/// `initial_delay_ms` is the dwell before the first auto-repeat; `interval_ms` is
/// the cadence after that. The focus engine accumulates dt against these. Confirm
/// and cancel never repeat regardless of this policy.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepeatPolicy {
    pub initial_delay_ms: f32,
    pub interval_ms: f32,
}

impl FocusPolicy {
    /// The traversal kind, regardless of which wire form authored it.
    pub fn kind(&self) -> FocusKind {
        match self {
            FocusPolicy::Shorthand(kind) => *kind,
            FocusPolicy::Detailed { policy, .. } => *policy,
        }
    }

    /// Whether nav wraps past the ends. The shorthand form defaults to `true`.
    pub fn wrap(&self) -> bool {
        match self {
            FocusPolicy::Shorthand(_) => true,
            FocusPolicy::Detailed { wrap, .. } => *wrap,
        }
    }

    /// The hold-to-repeat policy, if the container declared one.
    pub fn repeat(&self) -> Option<RepeatPolicy> {
        match self {
            FocusPolicy::Shorthand(_) => None,
            FocusPolicy::Detailed { repeat, .. } => *repeat,
        }
    }
}

/// serde default for `FocusPolicy::Detailed::wrap` — wrap is on unless authored off.
fn default_wrap() -> bool {
    true
}

/// `skip_serializing_if` predicate: omit `wrap` when it is the `true` default, so
/// a wrap-on container round-trips without emitting the key.
fn is_true(b: &bool) -> bool {
    *b
}

/// `skip_serializing_if` predicate for `restore_on_return`: omit when `false`
/// (the default) so a pre-F container round-trips byte-identically.
fn is_false(b: &bool) -> bool {
    !*b
}

/// Per-direction focus-neighbor overrides authored on a node (M13 Goal F, Task 3).
/// Each field, when set, names the node id focus jumps to when that direction is
/// pressed while this node is focused — overriding the container's focus policy.
/// All fields default to absent, and the whole struct skip-serializes when empty,
/// so a node that authors no override round-trips byte-identically.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusNeighbors {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub down: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<String>,
}

impl FocusNeighbors {
    /// True when no direction is overridden — the `skip_serializing_if` predicate
    /// so an override-less node omits the `focusNeighbors` key entirely.
    pub fn is_empty(&self) -> bool {
        self.up.is_none() && self.down.is_none() && self.left.is_none() && self.right.is_none()
    }
}

/// Top-level placement envelope wrapping the root widget. `anchor`/`offset`
/// live ONLY here, not on widget variants: a widget tree is placed once, as a
/// whole, against the logical-reference canvas (see `layout::Anchor`). `offset`
/// is logical-reference px, `[x, y]` (+x right, +y down), matching
/// `UiElement::offset`.
///
/// `capture_mode` declares whether the tree captures input or passes it through;
/// it defaults to `Passthrough` and skip-serializes when passthrough, so a
/// pre-F descriptor (no `captureMode` key) round-trips byte-identically. The
/// modal stack reads the TOP tree's mode to drive the input seam.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchoredTree {
    pub anchor: Anchor,
    pub offset: [f32; 2],
    pub root: Widget,
    /// Input capture behavior; defaults to `Passthrough`. Skip-serialized when
    /// passthrough so a HUD/pre-F tree omits the key entirely (wire-identical).
    #[serde(default, skip_serializing_if = "CaptureMode::is_passthrough")]
    pub capture_mode: CaptureMode,
    /// Authored id of the node focus starts on when this tree becomes the top of
    /// the modal stack (M13 Goal F, Task 3). Absent selects the first focusable
    /// node in tree order. Skip-serialized when absent so a pre-F tree omits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_focus: Option<String>,
}

impl CaptureMode {
    /// True for the default `Passthrough`. Used by `skip_serializing_if` so a
    /// passthrough tree omits the `captureMode` key (pre-F wire compatibility).
    fn is_passthrough(&self) -> bool {
        matches!(self, CaptureMode::Passthrough)
    }
}

impl AnchoredTree {
    /// Build a passthrough-mode tree (the HUD/splash default). Most programmatic
    /// trees never capture, so this keeps their construction terse and lets the
    /// `capture_mode` field be added without touching every call site.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn passthrough(anchor: Anchor, offset: [f32; 2], root: Widget) -> Self {
        Self {
            anchor,
            offset,
            root,
            capture_mode: CaptureMode::Passthrough,
            initial_focus: None,
        }
    }
}

/// One node in the UI widget tree. Internally tagged on `kind` (`"text"`,
/// `"panel"`, …) so the wire form is a flat object — `{ "kind": "text", ... }`.
///
/// Internally-tagged serde requires struct variants (not tuple variants): the
/// tag is read by buffering the object through `serde_json::Value`, which a
/// tuple variant cannot map onto. Container kinds (`vstack`/`hstack`/`grid`)
/// carry positional `children`; leaf kinds (`text`/`image`/`spacer`) carry no
/// `children` field. Compare `scripting::data_descriptors::ReactionDescriptor`,
/// which discriminates by manual key-presence instead — this enum deliberately
/// uses serde's tag mechanism.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Widget {
    Text(TextWidget),
    Panel(PanelWidget),
    Image(ImageWidget),
    // `rename_all = "camelCase"` would emit `"vStack"`/`"hStack"`; the wire form
    // is all-lowercase `"vstack"`/`"hstack"`, so pin those two explicitly.
    #[serde(rename = "vstack")]
    VStack(ContainerWidget),
    #[serde(rename = "hstack")]
    HStack(ContainerWidget),
    Grid(GridWidget),
    Spacer(SpacerWidget),
    // M13 Goal F, Task 4 — the first interactive widgets. `button`/`slider` are
    // focusable (their focusable marker is plugged into `tree::focus_meta` /
    // `tree::widget_interaction`); `bar` is a passive bound display widget.
    Button(ButtonWidget),
    Slider(SliderWidget),
    Bar(BarWidget),
}

/// Leaf text run. `content` is the literal string; `font_size` is logical px;
/// `color` is linear RGBA. The run is sized by the glyphon measure seam and laid
/// out in its container's flow.
///
/// `bind` is the optional state-binding: when present, the rendered string is
/// resolved from a store slot at draw-data build time and `content` serves only
/// as the fallback for an absent slot (see `tree::resolve_text`).
/// Absent on every static widget, so unbound text round-trips unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextWidget {
    pub content: String,
    pub font_size: f32,
    pub color: ColorValue,
    /// Authored stable id (M13 Goal F, Task 3). When present it carries across
    /// structural rebuilds for focus restore. Absent on every pre-F widget, so
    /// id-less text round-trips byte-identically (auto-gen ids are runtime-only,
    /// never serialized — see `tree`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Directional focus-neighbor overrides (M13 Goal F, Task 3). When a direction
    /// is set, nav in that direction jumps straight to the named node, bypassing
    /// the container policy. Absent on every pre-F widget (skip-serialized empty).
    #[serde(default, skip_serializing_if = "FocusNeighbors::is_empty")]
    pub focus_neighbors: FocusNeighbors,
    /// Optional theme font name. Absent on every pre-theming widget, so fontless
    /// text keeps its old wire form exactly (the key is omitted, not `null`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<TextBind>,
    /// Optional continuous value→style map (M13 Goal E). When present, the
    /// rendered value (the display value mid-tween) is mapped to a band color +
    /// pulse/flash effect, overriding `color`. Meaningful only alongside `bind`
    /// (the bound slot supplies the value); present without `bind` it warns once
    /// per tree build and never fires. Absent on every pre-E widget, so a
    /// styleRange-less widget keeps its old wire form (the key is omitted, not
    /// `null`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_ranges: Option<StyleRanges>,
}

/// State binding for a `text` widget. `slot` is a dotted slot name (e.g.
/// `"player.health"`) read from the frame's snapshot; `format` is an optional
/// template with a single `{}` placeholder substituted by the resolved value's
/// string form. With `format` absent, the value's default string form is drawn.
/// Multi-value templates are out of scope — one `{}` max.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBind {
    pub slot: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Optional value-tweening config (M13 UI Value-Tweening). When present, the
    /// tween runtime eases the resolved numeric value toward each new target
    /// over `duration_ms` using `easing` instead of snapping. Absent on every
    /// pre-tweening bind, so a tween-less bind keeps its old wire form (the key
    /// is omitted, not `null`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tween: Option<TextTween>,
}

/// Easing curve for a value tween (M13). A closed serde enum: each variant maps
/// to a camelCase wire literal (`"linear"`, `"easeIn"`, `"easeOut"`,
/// `"easeInOut"`). Shared by `TextTween` and `PanelTween`; the tween runtime
/// samples the curve at each frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

/// Value-tweening config for a `text` bind (M13). When a bound numeric slot's
/// value changes, the displayed value eases toward the new target over
/// `duration_ms` (milliseconds) using `easing`. `from` is the optional explicit
/// starting value for the FIRST tween (before any slot value has been seen);
/// when absent the runtime starts from the first observed value. The wire shape
/// differs from `PanelTween` only in `from`'s JSON type (a number here).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextTween {
    pub duration_ms: f32,
    pub easing: Easing,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<f32>,
}

/// Solid-fill panel with an optional 9-slice border. `fill` is linear RGBA. The
/// panel fills its flex/grid slot (it has no intrinsic size). Container-level
/// backdrops (the splash's framed panel) are expressed as a `ContainerWidget`
/// `fill`/`border` instead — a parent drawing its own backdrop beneath flowed
/// children — so an overlapping composition needs no standalone sized panel.
///
/// `bind` is the optional state-binding: when present, the panel `fill` is
/// resolved from a store slot holding a length-4 linear-RGBA array at draw-data
/// build time, with the literal `fill` serving as the fallback for an absent or
/// malformed slot (see `tree::resolve_panel_fill`). Absent on static panels, so
/// unbound panels round-trip unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelWidget {
    pub fill: ColorValue,
    pub border: Option<Border>,
    /// Authored stable id (M13 Goal F, Task 3). See `TextWidget::id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Directional focus-neighbor overrides (M13 Goal F, Task 3). See
    /// `TextWidget::focus_neighbors`.
    #[serde(default, skip_serializing_if = "FocusNeighbors::is_empty")]
    pub focus_neighbors: FocusNeighbors,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<PanelBind>,
    /// Optional continuous value→style map (M13 Goal E). When present, the
    /// rendered value (the display value mid-tween) is mapped to a band color +
    /// pulse/flash effect, overriding `fill`. Meaningful only alongside `bind`;
    /// present without `bind` it warns once per tree build and never fires.
    /// Absent on every pre-E panel, so a styleRange-less panel round-trips
    /// unchanged (the key is omitted, not `null`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_ranges: Option<StyleRanges>,
}

/// State binding for a `panel` widget. `slot` is a dotted slot name whose value
/// must be a `SlotValue::Array` of exactly 4 f32 (linear `[r, g, b, a]`); it
/// replaces the literal `fill`. A wrong variant, wrong length, or absent slot
/// falls back to the literal `fill` (see `tree::resolve_panel_fill`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelBind {
    pub slot: String,
    /// Optional value-tweening config (M13). When present, the tween runtime
    /// eases the resolved RGBA fill toward each new target over `duration_ms`.
    /// Absent on every pre-tweening bind, so a tween-less bind keeps its old wire
    /// form (the key is omitted, not `null`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tween: Option<PanelTween>,
}

/// Value-tweening config for a `panel` bind (M13). When the bound RGBA slot's
/// value changes, the displayed fill eases toward the new target over
/// `duration_ms` (milliseconds) using `easing`. `from` is the optional explicit
/// starting color for the FIRST tween; when absent the runtime starts from the
/// first observed value. The wire shape differs from `TextTween` only in
/// `from`'s JSON type (a length-4 linear-RGBA array here).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelTween {
    pub duration_ms: f32,
    pub easing: Easing,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<[f32; 4]>,
}

/// Leaf image referencing a texture asset by key. The image has no wire-level
/// size: it sizes from the asset's NATURAL pixel dimensions (content-driven, the
/// same category as text measurement). The renderer threads each asset's natural
/// reference size into the measure seam (see `tree::UiTree::build_draw_data`), so
/// the on-screen image is always shaped to the real asset and never stretched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageWidget {
    pub asset: String,
    /// Authored stable id (M13 Goal F, Task 3). See `TextWidget::id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Directional focus-neighbor overrides (M13 Goal F, Task 3). See
    /// `TextWidget::focus_neighbors`.
    #[serde(default, skip_serializing_if = "FocusNeighbors::is_empty")]
    pub focus_neighbors: FocusNeighbors,
}

/// Stack container (`vstack`/`hstack`). Lays its `children` out along one axis
/// with `gap` between them, `padding` inside its bounds, and cross-axis
/// `align`. `children` carries no `skip_serializing_if`: an empty container
/// must serialize `"children":[]` so round-trip identity holds.
///
/// A container may carry its own backdrop: an optional solid `fill` (linear
/// RGBA) and/or 9-slice `border`, drawn as a quad sized to the container's full
/// laid-out rect, BENEATH its flowed children (painter's order — see
/// `tree::collect_node`). This expresses "a backing panel wrapping content"
/// natively: the splash's framed panel is an outer container (border-colored
/// fill + padding) wrapping an inner container (panel-colored fill) that flows
/// the logo + version text, with no absolute overlap. Both skip-serialize when
/// absent, so a fill-less container round-trips byte-identically.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerWidget {
    pub gap: SpacingValue,
    pub padding: SpacingValue,
    pub align: Align,
    /// Optional backdrop fill (linear RGBA), drawn beneath the children.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<ColorValue>,
    /// Optional 9-slice border framing the backdrop (drawn with the fill).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<Border>,
    /// Authored stable id (M13 Goal F, Task 3). See `TextWidget::id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Directional focus-neighbor overrides on the container node itself.
    #[serde(default, skip_serializing_if = "FocusNeighbors::is_empty")]
    pub focus_neighbors: FocusNeighbors,
    /// Focus-traversal policy for this container's children (M13 Goal F, Task 3).
    /// Absent leaves the container's children outside any focus group of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<FocusPolicy>,
    /// Restore this container's last-focused descendant when a tree popped above
    /// it returns focus here (M13 Goal F, Task 3). Skip-serialized when `false`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub restore_on_return: bool,
    pub children: Vec<Widget>,
}

/// Grid container. Like a stack but flows `children` across a fixed number of
/// columns. Shares the stack fields; adds `cols`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GridWidget {
    pub gap: SpacingValue,
    pub padding: SpacingValue,
    pub align: Align,
    pub cols: u32,
    /// Authored stable id (M13 Goal F, Task 3). See `TextWidget::id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Directional focus-neighbor overrides on the grid node itself.
    #[serde(default, skip_serializing_if = "FocusNeighbors::is_empty")]
    pub focus_neighbors: FocusNeighbors,
    /// Focus-traversal policy for this grid's children. A grid typically authors
    /// `"spatial"` so nav moves nearest-neighbor by direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<FocusPolicy>,
    /// Restore this grid's last-focused descendant on return (see `ContainerWidget`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub restore_on_return: bool,
    pub children: Vec<Widget>,
}

/// Flexible-space leaf. `flex_grow` is the proportional share of leftover space
/// it claims inside its parent container.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpacerWidget {
    pub flex_grow: f32,
    /// Authored stable id (M13 Goal F, Task 3). See `TextWidget::id`. A spacer is
    /// not focusable, but it may still carry an id for neighbor references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// Interactive button (M13 Goal F, Task 4). Focusable; activation — a focus-engine
/// `confirm` on the focused button, or a pointer click — fires the `on_press`
/// named reaction through the same reaction registry every entity/system reaction
/// uses. The button renders its `label` as a centered text run.
///
/// `id` is required (unlike the optional `id` on passive widgets): activation maps
/// the focused node id back to this button's `on_press`, so the id must be stable.
/// `focus_neighbors` carries directional overrides exactly like the passive widgets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ButtonWidget {
    pub id: String,
    pub label: String,
    /// Named reaction fired on activation (confirm or click). Resolved against the
    /// reaction registry by the app — the same vocabulary entity/system reactions use.
    pub on_press: String,
    /// Directional focus-neighbor overrides (M13 Goal F, Task 3). See
    /// `TextWidget::focus_neighbors`.
    #[serde(default, skip_serializing_if = "FocusNeighbors::is_empty")]
    pub focus_neighbors: FocusNeighbors,
}

/// Interactive slider (M13 Goal F, Task 4). Focusable; nav steps it captures
/// (`captures_nav`, e.g. `["nav.left", "nav.right"]`) adjust its value by `step`
/// within `[min, max]` and emit a `setState` write to the bound slot on the N+1
/// frame. The slider renders its `label` and current numeric value as text.
///
/// `bind` follows the `PanelBind`/`TextBind` shape (slot name + optional tween).
/// `id` is required for the same reason as `ButtonWidget::id` — nav-capture and
/// value-step resolve through the focused node id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SliderWidget {
    pub id: String,
    pub label: String,
    pub bind: SliderBind,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    /// Nav wire names this slider consumes (e.g. `["nav.left", "nav.right"]`).
    /// An array, NOT a bool — a slider gives the named nav intents first refusal,
    /// stepping its value instead of moving focus. Absent/empty means the slider
    /// captures no nav (focus moves normally). Skip-serialized when empty so a
    /// capture-less slider omits the key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub captures_nav: Vec<String>,
    /// Directional focus-neighbor overrides (M13 Goal F, Task 3).
    #[serde(default, skip_serializing_if = "FocusNeighbors::is_empty")]
    pub focus_neighbors: FocusNeighbors,
}

/// State binding for a `slider` widget. Mirrors `PanelBind`'s shape (slot name +
/// optional tween) so the bind vocabulary stays uniform across bound widgets; a
/// slider binds a numeric slot, so its tween is the `TextTween` (number) shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SliderBind {
    pub slot: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tween: Option<TextTween>,
}

/// Passive horizontal value bar (M13 Goal F, Task 4). Not focusable. Renders a
/// `background` quad with a `fill` quad whose width is `value/max` clamped to
/// `[0, 1]` of the bar's laid-out width. `bind` follows the `PanelBind`/`TextBind`
/// shape; the bar uses the eased display value like other bound widgets, and
/// `style_ranges` (M13 Goal E) recolors the fill when present.
///
/// `fill`/`background` are color slots (literal or theme token). `bar` is
/// horizontal-only in v1 (a vertical field is a later additive change).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BarWidget {
    pub bind: SliderBind,
    pub max: f32,
    pub fill: ColorValue,
    pub background: ColorValue,
    /// Authored stable id (M13 Goal F, Task 3). See `TextWidget::id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Optional continuous value→style map (M13 Goal E): recolors the `fill` band
    /// by `value/max`. Calls the widget-agnostic `style_ranges::evaluate`. Absent
    /// on a plain bar (skip-serialized), so a styleRange-less bar omits the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_ranges: Option<StyleRanges>,
}

/// Cross-axis alignment of a container's children.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Align {
    Start,
    Center,
    End,
    Stretch,
}

/// 9-slice border descriptor: the source `texture` asset key, the `slice`
/// inset (logical px, `[left, top, right, bottom]`) that splits it into the
/// nine regions, and a linear-RGBA `tint`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Border {
    pub texture: String,
    pub slice: [f32; 4],
    pub tint: ColorValue,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tree exercising all seven kinds wrapped in the placement envelope.
    /// Field order matches the Rust struct declaration order so the
    /// re-serialized JSON is byte-identical to this source (serde emits fields
    /// in declaration order). The tag `kind` always serializes first.
    const ALL_KINDS_JSON: &str = r#"{"anchor":"center","offset":[10.0,-20.0],"root":{"kind":"vstack","gap":4.0,"padding":8.0,"align":"start","children":[{"kind":"text","content":"hello","fontSize":18.0,"color":[1.0,1.0,1.0,1.0]},{"kind":"panel","fill":[0.1,0.2,0.3,1.0],"border":{"texture":"ui/frame","slice":[8.0,8.0,8.0,8.0],"tint":[1.0,1.0,1.0,1.0]}},{"kind":"hstack","gap":2.0,"padding":0.0,"align":"center","children":[{"kind":"image","asset":"ui/logo"},{"kind":"spacer","flexGrow":1.0}]},{"kind":"grid","gap":1.0,"padding":3.0,"align":"stretch","cols":2,"children":[{"kind":"image","asset":"ui/icon"}]}]}}"#;

    #[test]
    fn anchored_tree_round_trips_all_seven_kinds_identically() {
        let tree: AnchoredTree =
            serde_json::from_str(ALL_KINDS_JSON).expect("fixture must deserialize");
        let reserialized = serde_json::to_string(&tree).expect("must serialize");
        assert_eq!(reserialized, ALL_KINDS_JSON);
    }

    #[test]
    fn empty_container_round_trips_with_explicit_children_array() {
        // An empty container must keep `"children":[]` across a round-trip —
        // no `skip_serializing_if` — so identity holds for childless stacks.
        let json = r#"{"anchor":"topLeft","offset":[0.0,0.0],"root":{"kind":"vstack","gap":0.0,"padding":0.0,"align":"start","children":[]}}"#;
        let tree: AnchoredTree = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&tree).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn unknown_kind_deserializes_to_error_not_panic() {
        // An unrecognized `kind` tag is a serde error, never a panic — mod
        // authors get a rejected document, not a crash.
        let json = r#"{"kind":"carousel"}"#;
        let result: Result<Widget, _> = serde_json::from_str(json);
        assert!(result.is_err(), "unknown kind must be a serde error");
    }

    #[test]
    fn anchor_serializes_to_camel_case_wire_form() {
        // Pins the cross-boundary casing: TopLeft -> "topLeft", Center ->
        // "center". The envelope reuses `layout::Anchor`.
        assert_eq!(
            serde_json::to_string(&Anchor::TopLeft).unwrap(),
            r#""topLeft""#
        );
        assert_eq!(
            serde_json::to_string(&Anchor::BottomRight).unwrap(),
            r#""bottomRight""#
        );
        assert_eq!(
            serde_json::to_string(&Anchor::Center).unwrap(),
            r#""center""#
        );
    }

    #[test]
    fn align_serializes_to_camel_case_wire_form() {
        assert_eq!(serde_json::to_string(&Align::Start).unwrap(), r#""start""#);
        assert_eq!(
            serde_json::to_string(&Align::Stretch).unwrap(),
            r#""stretch""#
        );
    }

    #[test]
    fn bound_text_round_trips_with_slot_and_format() {
        // A `text` node carrying a `bind` with both `slot` and `format` keeps its
        // camelCase wire form byte-for-byte. Field order: content, fontSize,
        // color, then the nested bind { slot, format }.
        let json = r#"{"kind":"text","content":"0","fontSize":18.0,"color":[1.0,1.0,1.0,1.0],"bind":{"slot":"player.health","format":"HP {}"}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn bound_text_round_trips_with_format_absent() {
        // A `bind` with no `format` omits the field entirely (skip_serializing_if).
        let json = r#"{"kind":"text","content":"0","fontSize":18.0,"color":[1.0,1.0,1.0,1.0],"bind":{"slot":"player.ammo"}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn unbound_text_serializes_without_a_bind_field() {
        // An unbound text widget must not emit a `bind` key — static widgets keep
        // their pre-binding wire form so old descriptors round-trip unchanged.
        let json = r#"{"kind":"text","content":"hello","fontSize":12.0,"color":[1.0,1.0,1.0,1.0]}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn bound_panel_round_trips_with_slot() {
        // A `panel` node binding its `fill` to a color slot keeps its wire form.
        // Field order: fill, border (null when absent), then bind { slot }.
        let json = r#"{"kind":"panel","fill":[0.0,0.0,0.0,1.0],"border":null,"bind":{"slot":"intro.flashColor"}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn unbound_panel_serializes_without_a_bind_field() {
        // An unbound panel must not emit a `bind` key.
        let json = r#"{"kind":"panel","fill":[0.1,0.2,0.3,1.0],"border":null}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn text_color_token_and_literal_each_round_trip_in_its_own_form() {
        // The wire form of a color slot is disjoint: a token serializes as a bare
        // string, a literal as a bare array. Each must re-serialize byte-identically
        // to the form it was authored in — the untagged union never rewrites one
        // form into the other.
        let token = r#"{"kind":"text","content":"HP","fontSize":18.0,"color":"critical"}"#;
        let token_widget: Widget = serde_json::from_str(token).expect("token must deserialize");
        assert_eq!(serde_json::to_string(&token_widget).unwrap(), token);

        let literal = r#"{"kind":"text","content":"HP","fontSize":18.0,"color":[1.0,0.0,0.0,1.0]}"#;
        let literal_widget: Widget =
            serde_json::from_str(literal).expect("literal must deserialize");
        assert_eq!(serde_json::to_string(&literal_widget).unwrap(), literal);
    }

    #[test]
    fn color_value_parses_array_to_literal_and_string_to_token() {
        // Pin the variant the disjoint JSON forms land on (declaration order makes
        // `Literal` first, but arrays and strings are unambiguous either way).
        let lit: ColorValue = serde_json::from_str("[1.0,0.0,0.0,1.0]").unwrap();
        assert_eq!(lit, ColorValue::Literal([1.0, 0.0, 0.0, 1.0]));
        let tok: ColorValue = serde_json::from_str(r#""critical""#).unwrap();
        assert_eq!(tok, ColorValue::Token("critical".to_string()));
    }

    #[test]
    fn spacing_value_token_and_literal_each_round_trip_in_its_own_form() {
        // A spacing token serializes as a bare string, a literal as a bare JSON
        // number — `SpacingValue::Literal` wraps a bare `f32`, so `4.0` stays `4.0`.
        let token: SpacingValue = serde_json::from_str(r#""tight""#).expect("token deserializes");
        assert_eq!(token, SpacingValue::Token("tight".to_string()));
        assert_eq!(serde_json::to_string(&token).unwrap(), r#""tight""#);

        let literal: SpacingValue = serde_json::from_str("4.0").expect("literal deserializes");
        assert_eq!(literal, SpacingValue::Literal(4.0));
        assert_eq!(serde_json::to_string(&literal).unwrap(), "4.0");
    }

    #[test]
    fn container_spacing_token_round_trips_identically() {
        // A container may carry token gap/padding (bare strings) the same way it
        // carries literal numbers; the wire form stays a flat object either way.
        let json = r#"{"kind":"vstack","gap":"m","padding":"s","align":"start","children":[]}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(serde_json::to_string(&widget).unwrap(), json);
    }

    #[test]
    fn text_font_token_round_trips_and_absent_font_omits_the_field() {
        // A `font` token round-trips byte-identically; an absent font omits the key
        // entirely (skip_serializing_if), so pre-theming fontless text is unchanged.
        let with_font = r#"{"kind":"text","content":"x","fontSize":12.0,"color":[1.0,1.0,1.0,1.0],"font":"mono"}"#;
        let widget: Widget = serde_json::from_str(with_font).expect("must deserialize");
        assert_eq!(serde_json::to_string(&widget).unwrap(), with_font);

        let no_font = r#"{"kind":"text","content":"x","fontSize":12.0,"color":[1.0,1.0,1.0,1.0]}"#;
        let widget: Widget = serde_json::from_str(no_font).expect("must deserialize");
        assert_eq!(serde_json::to_string(&widget).unwrap(), no_font);
    }

    #[test]
    fn container_with_fill_and_border_round_trips_identically() {
        // A container carrying a backdrop `fill` + 9-slice `border` (the splash's
        // framed-panel vocabulary). Field order matches the struct declaration
        // (gap, padding, align, fill, border, children) so the re-serialized JSON
        // is byte-identical. `fill`/`border` skip-serialize when absent, so a
        // fill-less container keeps its old wire form — pinned by the all-kinds
        // and empty-container round-trips above.
        let json = r#"{"kind":"vstack","gap":0.0,"padding":4.0,"align":"center","fill":[0.1,0.55,0.62,1.0],"border":{"texture":"","slice":[12.0,12.0,12.0,12.0],"tint":[0.1,0.55,0.62,1.0]},"children":[]}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn bound_text_round_trips_with_tween() {
        // A `text` bind carrying a `tween` keeps its camelCase wire form
        // byte-for-byte. Field order inside tween: durationMs, easing, from.
        let json = r#"{"kind":"text","content":"0","fontSize":18.0,"color":[1.0,1.0,1.0,1.0],"bind":{"slot":"player.health","tween":{"durationMs":1200.0,"easing":"easeOut","from":0.0}}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn bound_panel_round_trips_with_tween_and_from_absent() {
        // A `panel` bind tween with no `from` omits the `from` key entirely
        // (skip_serializing_if) and round-trips byte-identically.
        let json = r#"{"kind":"panel","fill":[0.0,0.0,0.0,1.0],"border":null,"bind":{"slot":"intro.flashColor","tween":{"durationMs":150.0,"easing":"easeInOut"}}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
        // Belt-and-suspenders: the absent `from` emits no `from` key.
        assert!(
            !reserialized.contains("from"),
            "absent from must emit no key"
        );
    }

    #[test]
    fn bound_panel_round_trips_with_tween_from_array() {
        // A `panel` bind tween whose `from` is a length-4 linear-RGBA array keeps
        // its wire form (the panel-side `from` type, distinct from text's number).
        let json = r#"{"kind":"panel","fill":[0.0,0.0,0.0,1.0],"border":null,"bind":{"slot":"intro.flashColor","tween":{"durationMs":300.0,"easing":"linear","from":[1.0,0.0,0.0,1.0]}}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn tween_less_binds_serialize_without_a_tween_field() {
        // A bind with no `tween` must not emit a `tween` key — pre-tweening binds
        // keep their exact wire form so old descriptors round-trip unchanged.
        let text = r#"{"kind":"text","content":"0","fontSize":18.0,"color":[1.0,1.0,1.0,1.0],"bind":{"slot":"player.ammo"}}"#;
        let widget: Widget = serde_json::from_str(text).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, text);
        assert!(
            !reserialized.contains("tween"),
            "tween-less text emits no tween key"
        );

        let panel = r#"{"kind":"panel","fill":[0.0,0.0,0.0,1.0],"border":null,"bind":{"slot":"intro.flashColor"}}"#;
        let widget: Widget = serde_json::from_str(panel).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, panel);
        assert!(
            !reserialized.contains("tween"),
            "tween-less panel emits no tween key"
        );
    }

    #[test]
    fn easing_variants_serialize_to_camel_case_wire_form() {
        assert_eq!(
            serde_json::to_string(&Easing::Linear).unwrap(),
            r#""linear""#
        );
        assert_eq!(
            serde_json::to_string(&Easing::EaseIn).unwrap(),
            r#""easeIn""#
        );
        assert_eq!(
            serde_json::to_string(&Easing::EaseOut).unwrap(),
            r#""easeOut""#
        );
        assert_eq!(
            serde_json::to_string(&Easing::EaseInOut).unwrap(),
            r#""easeInOut""#
        );
        // And each parses back from its literal.
        let parsed: Easing = serde_json::from_str(r#""easeInOut""#).unwrap();
        assert_eq!(parsed, Easing::EaseInOut);
    }

    #[test]
    fn style_range_less_text_round_trips_byte_identically() {
        // A pre-E `text` widget carrying no `styleRanges` must keep its EXACT wire
        // form: the new field skip-serializes when absent (default), so a
        // styleRanges-less descriptor is byte-identical across a round-trip. This
        // is the locked-wire-format guarantee for Goal E's additive field.
        let json = r#"{"kind":"text","content":"0","fontSize":18.0,"color":[1.0,1.0,1.0,1.0],"bind":{"slot":"player.health"}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
        assert!(
            !reserialized.contains("styleRanges"),
            "absent styleRanges emits no key"
        );
    }

    #[test]
    fn style_range_less_panel_round_trips_byte_identically() {
        // The panel-side twin: a styleRanges-less panel keeps its pre-E wire form.
        let json = r#"{"kind":"panel","fill":[0.1,0.2,0.3,1.0],"border":null}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
        assert!(
            !reserialized.contains("styleRanges"),
            "absent styleRanges emits no key"
        );
    }

    #[test]
    fn capture_mode_bearing_tree_round_trips_in_camel_case() {
        // A `captureMode: "capture"` envelope round-trips byte-for-byte. Field
        // order: anchor, offset, root, then captureMode (declaration order).
        let json = r#"{"anchor":"center","offset":[0.0,0.0],"root":{"kind":"spacer","flexGrow":1.0},"captureMode":"capture"}"#;
        let tree: AnchoredTree = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(tree.capture_mode, CaptureMode::Capture);
        let reserialized = serde_json::to_string(&tree).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn capture_mode_absent_round_trips_byte_identically_as_passthrough() {
        // A pre-F descriptor with no `captureMode` key deserializes to the default
        // `Passthrough` and re-serializes WITHOUT the key (skip_serializing_if), so
        // the wire form stays byte-identical to the pre-F shape.
        let json =
            r#"{"anchor":"center","offset":[0.0,0.0],"root":{"kind":"spacer","flexGrow":1.0}}"#;
        let tree: AnchoredTree = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(
            tree.capture_mode,
            CaptureMode::Passthrough,
            "absent captureMode defaults to passthrough",
        );
        let reserialized = serde_json::to_string(&tree).expect("must serialize");
        assert_eq!(reserialized, json);
        assert!(
            !reserialized.contains("captureMode"),
            "passthrough captureMode emits no key",
        );
    }

    #[test]
    fn capture_mode_serializes_to_camel_case_wire_form() {
        assert_eq!(
            serde_json::to_string(&CaptureMode::Capture).unwrap(),
            r#""capture""#
        );
        assert_eq!(
            serde_json::to_string(&CaptureMode::Passthrough).unwrap(),
            r#""passthrough""#
        );
    }

    #[test]
    fn text_with_style_ranges_round_trips_in_camel_case() {
        // A `text` widget carrying `styleRanges` keeps its camelCase wire form
        // byte-for-byte. Field order: content, fontSize, color, bind, then
        // styleRanges { max, entries: [{ upTo, color }, { color }] }.
        let json = r#"{"kind":"text","content":"0","fontSize":18.0,"color":[1.0,1.0,1.0,1.0],"bind":{"slot":"player.health"},"styleRanges":{"max":100.0,"entries":[{"upTo":0.25,"color":"critical"},{"color":"ok"}]}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    // --- M13 Goal F, Task 3: focus wire-form additive fields ---

    #[test]
    fn focus_field_less_widget_round_trips_byte_identically() {
        // A pre-F widget carrying none of the new focus fields (`id`,
        // `focusNeighbors`, `focus`, `restoreOnReturn`) keeps its EXACT wire form:
        // every new field skip-serializes when absent/default, so the descriptor is
        // byte-identical across a round-trip. The locked-wire guarantee for Task 3.
        let json = r#"{"kind":"vstack","gap":4.0,"padding":8.0,"align":"start","children":[{"kind":"text","content":"hi","fontSize":12.0,"color":[1.0,1.0,1.0,1.0]}]}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
        for key in ["\"id\"", "focusNeighbors", "\"focus\"", "restoreOnReturn"] {
            assert!(!reserialized.contains(key), "absent {key} emits no key");
        }
    }

    #[test]
    fn container_focus_policy_shorthand_and_detailed_round_trip() {
        // The `focus` field is an untagged union: a bare string shorthand
        // (`"linear"`) or a detailed object. Both round-trip byte-identically.
        let shorthand = r#"{"kind":"vstack","gap":0.0,"padding":0.0,"align":"start","focus":"linear","children":[]}"#;
        let w: Widget = serde_json::from_str(shorthand).expect("deserialize");
        assert_eq!(serde_json::to_string(&w).unwrap(), shorthand);

        // Detailed form with wrap:false and a repeat policy. `wrap` skip-serializes
        // only when true (its default), so an authored `false` is emitted.
        let detailed = r#"{"kind":"grid","gap":0.0,"padding":0.0,"align":"start","cols":2,"focus":{"policy":"spatial","wrap":false,"repeat":{"initialDelayMs":300.0,"intervalMs":80.0}},"children":[]}"#;
        let w: Widget = serde_json::from_str(detailed).expect("deserialize");
        assert_eq!(serde_json::to_string(&w).unwrap(), detailed);
    }

    #[test]
    fn focus_policy_accessors_resolve_kind_wrap_and_repeat() {
        // Shorthand: linear kind, wrap defaults on, no repeat.
        let sh: FocusPolicy = serde_json::from_str(r#""linear""#).unwrap();
        assert_eq!(sh.kind(), FocusKind::Linear);
        assert!(sh.wrap());
        assert!(sh.repeat().is_none());

        // Detailed: spatial, wrap off, repeat carried through.
        let det: FocusPolicy = serde_json::from_str(
            r#"{"policy":"spatial","wrap":false,"repeat":{"initialDelayMs":250.0,"intervalMs":60.0}}"#,
        )
        .unwrap();
        assert_eq!(det.kind(), FocusKind::Spatial);
        assert!(!det.wrap());
        let r = det.repeat().expect("repeat carried");
        assert_eq!(r.initial_delay_ms, 250.0);
        assert_eq!(r.interval_ms, 60.0);
    }

    #[test]
    fn node_id_and_focus_neighbors_round_trip() {
        // An authored `id` and partial `focusNeighbors` keep their camelCase wire
        // form; the unset neighbor directions omit their keys.
        let json = r#"{"kind":"text","content":"A","fontSize":12.0,"color":[1.0,1.0,1.0,1.0],"id":"btnA","focusNeighbors":{"down":"btnB","right":"btnC"}}"#;
        let w: Widget = serde_json::from_str(json).expect("deserialize");
        assert_eq!(serde_json::to_string(&w).unwrap(), json);
    }

    #[test]
    fn anchored_tree_initial_focus_and_restore_on_return_round_trip() {
        // `initialFocus` lives on the envelope beside `captureMode`;
        // `restoreOnReturn` on the container. Both round-trip byte-identically.
        let json = r#"{"anchor":"center","offset":[0.0,0.0],"root":{"kind":"vstack","gap":0.0,"padding":0.0,"align":"start","restoreOnReturn":true,"children":[]},"captureMode":"capture","initialFocus":"btnA"}"#;
        let tree: AnchoredTree = serde_json::from_str(json).expect("deserialize");
        assert_eq!(tree.initial_focus.as_deref(), Some("btnA"));
        assert_eq!(serde_json::to_string(&tree).unwrap(), json);
    }

    // --- M13 Goal F, Task 4: interactive widgets ---

    #[test]
    fn button_round_trips_with_on_press_in_camel_case() {
        // A `button` carrying id/label/onPress keeps its camelCase wire form.
        // Field order: kind, id, label, onPress (declaration order).
        let json = r#"{"kind":"button","id":"resume","label":"Resume","onPress":"resumeGame"}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        assert!(matches!(widget, Widget::Button(_)));
        let reserialized = serde_json::to_string(&widget).expect("must serialize");
        assert_eq!(reserialized, json);
    }

    #[test]
    fn button_with_focus_neighbors_round_trips_and_capture_less_omits_keys() {
        let json = r#"{"kind":"button","id":"a","label":"A","onPress":"fa","focusNeighbors":{"down":"b"}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(serde_json::to_string(&widget).unwrap(), json);
        // A neighborless button omits the focusNeighbors key entirely.
        let plain = r#"{"kind":"button","id":"a","label":"A","onPress":"fa"}"#;
        let widget: Widget = serde_json::from_str(plain).expect("must deserialize");
        assert_eq!(serde_json::to_string(&widget).unwrap(), plain);
    }

    #[test]
    fn slider_round_trips_with_captures_nav_array() {
        // `capturesNav` is an ARRAY of nav wire names, not a bool. The slider
        // round-trips byte-identically. Field order: kind, id, label, bind, min,
        // max, step, capturesNav.
        let json = r#"{"kind":"slider","id":"vol","label":"Volume","bind":{"slot":"audio.master"},"min":0.0,"max":1.0,"step":0.1,"capturesNav":["nav.left","nav.right"]}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        match &widget {
            Widget::Slider(s) => {
                assert_eq!(s.captures_nav, vec!["nav.left", "nav.right"]);
                assert_eq!(s.min, 0.0);
                assert_eq!(s.max, 1.0);
                assert_eq!(s.step, 0.1);
            }
            _ => panic!("expected slider"),
        }
        assert_eq!(serde_json::to_string(&widget).unwrap(), json);
    }

    #[test]
    fn slider_omits_empty_captures_nav_and_supports_bind_tween() {
        // No capturesNav and no tween: both keys omitted.
        let plain = r#"{"kind":"slider","id":"vol","label":"Volume","bind":{"slot":"audio.master"},"min":0.0,"max":1.0,"step":0.1}"#;
        let widget: Widget = serde_json::from_str(plain).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).unwrap();
        assert_eq!(reserialized, plain);
        assert!(!reserialized.contains("capturesNav"));
        assert!(!reserialized.contains("tween"));

        // A bind tween (number shape, TextTween) round-trips.
        let tween = r#"{"kind":"slider","id":"vol","label":"Volume","bind":{"slot":"audio.master","tween":{"durationMs":120.0,"easing":"easeOut"}},"min":0.0,"max":1.0,"step":0.1}"#;
        let widget: Widget = serde_json::from_str(tween).expect("must deserialize");
        assert_eq!(serde_json::to_string(&widget).unwrap(), tween);
    }

    #[test]
    fn bar_round_trips_with_max_fill_background() {
        // A `bar` binding `player.health` with max/fill/background. Field order:
        // kind, bind, max, fill, background.
        let json = r#"{"kind":"bar","bind":{"slot":"player.health"},"max":100.0,"fill":[0.0,1.0,0.0,1.0],"background":[0.1,0.1,0.1,1.0]}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        assert!(matches!(widget, Widget::Bar(_)));
        assert_eq!(serde_json::to_string(&widget).unwrap(), json);
    }

    #[test]
    fn bar_round_trips_with_style_ranges_and_token_colors() {
        // A bar may use theme-token color slots and a styleRanges map. Both
        // round-trip byte-identically; absent id/styleRanges omit their keys.
        let json = r#"{"kind":"bar","bind":{"slot":"player.health"},"max":100.0,"fill":"ok","background":"panel.default","styleRanges":{"max":100.0,"entries":[{"upTo":0.25,"color":"critical"},{"color":"ok"}]}}"#;
        let widget: Widget = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(serde_json::to_string(&widget).unwrap(), json);

        let plain = r#"{"kind":"bar","bind":{"slot":"player.health"},"max":100.0,"fill":[0.0,1.0,0.0,1.0],"background":[0.1,0.1,0.1,1.0]}"#;
        let widget: Widget = serde_json::from_str(plain).expect("must deserialize");
        let reserialized = serde_json::to_string(&widget).unwrap();
        assert_eq!(reserialized, plain);
        assert!(!reserialized.contains("styleRanges"));
        assert!(!reserialized.contains("\"id\""));
    }
}
