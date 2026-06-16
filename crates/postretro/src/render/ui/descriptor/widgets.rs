// The widget vocabulary: the internally-tagged `Widget` enum (eleven kinds) and
// its per-kind field structs, plus the bind/tween value types those widgets carry.
// Pure serde data — no rendering, no taffy, no retained tree.
// See: context/lib/ui.md

use serde::{Deserialize, Serialize};

use super::super::style_ranges::StyleRanges;
use super::accessibility::Role;
use super::focus::{FocusNeighbors, FocusPolicy, RepeatPolicy};
use super::values::{
    Align, BindSource, Border, ColorValue, Easing, LocalState, Predicate, SpacingValue,
};

/// One node in the UI widget tree. Internally tagged on `kind` (`"text"`,
/// `"panel"`, …) so the wire form is a flat object — `{ "kind": "text", ... }`.
///
/// Internally-tagged serde requires struct variants (not tuple variants): the
/// tag is read by buffering the object through `serde_json::Value`, which a
/// tuple variant cannot map onto. Container kinds (`vstack`/`hstack`/`grid`)
/// carry positional `children`; leaf kinds (`text`/`panel`/`image`/`spacer`/
/// `button`/`slider`/`bar`/`announce`) carry no
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
    // M13 G2 — a net-new non-visual widget. It lays out as nothing (no quad, no
    // glyph); its sole payload is an a11y live-region announcement a later task
    // routes to the platform a11y layer with the declared `priority`.
    Announce(AnnounceWidget),
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
    /// Optional reactive visibility predicate (M13 G2). When present, the node is
    /// shown only while the predicate resolves true (resolution is Task 2b).
    /// Absent on every pre-G2 widget, so a predicate-less widget round-trips
    /// byte-identically (skip-serialized).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_when: Option<Predicate>,
    /// Optional a11y role override (M13 G2). Absent uses the kind's implicit role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
}

/// State binding for a `text` widget. The bind source is either a `{ slot }`
/// store binding (a dotted slot name like `"player.health"`) or a `{ local }`
/// presentation-cell binding, flattened into the bind object as a sibling of
/// `format`/`tween`. `format` is an optional template with a single `{}`
/// placeholder substituted by the resolved value's string form; with `format`
/// absent, the value's default string form is drawn. One `{}` max.
//
// `deny_unknown_fields` is omitted: it is incompatible with `#[serde(flatten)]`,
// which the `source` alternative requires to keep `slot`/`local` flat siblings
// of `format`/`tween`. The bind shape is otherwise closed by `BindSource`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBind {
    #[serde(flatten)]
    pub source: BindSource,
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

/// Value-tweening config for a `text` bind (M13). When a bound numeric slot's
/// value changes, the displayed value eases toward the new target over
/// `duration_ms` (milliseconds) using `easing`. `from` is the optional explicit
/// starting value for the FIRST tween (before any slot value has been seen);
/// when absent the runtime starts from the first observed value. The wire shape
/// differs from `PanelTween` only in `from`'s JSON type (a number here).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PanelWidget {
    pub fill: ColorValue,
    // `default` (without `skip_serializing_if`) so an absent `border` key
    // deserializes to `None` — the SDK Luau factory cannot emit an explicit
    // `null` table value (the lua→json walker drops nil-valued keys), so a
    // border-less panel omits the key. Serialization is unchanged: `None` still
    // emits `border: null` (no skip), so every existing fixture round-trips
    // byte-identically; only the *absent-key* input is newly accepted.
    #[serde(default)]
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
    /// Optional reactive visibility predicate (M13 G2). See `TextWidget::visible_when`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_when: Option<Predicate>,
    /// Optional a11y role override (M13 G2). See `TextWidget::role`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
}

/// Bind source for a `panel` widget: either a `{ slot }` dotted store name whose
/// value must be a `SlotValue::Array` of exactly 4 f32 (linear `[r, g, b, a]`)
/// replacing the literal `fill`, or a `{ local }` presentation-cell name. A wrong
/// variant, wrong length, absent value, or undeclared cell falls back to the
/// literal `fill` (see `tree::resolve_panel_fill`).
//
// `deny_unknown_fields` omitted — see `TextBind` (incompatible with the flattened
// `source` alternative).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelBind {
    #[serde(flatten)]
    pub source: BindSource,
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
///
/// Accessible name (M13 G2): an image is name-XOR-decorative — exactly one of
/// `label` or `decorative: true` is required (the bridge enforces it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageWidget {
    pub asset: String,
    /// Authored stable id (M13 Goal F, Task 3). See `TextWidget::id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Directional focus-neighbor overrides (M13 Goal F, Task 3). See
    /// `TextWidget::focus_neighbors`.
    #[serde(default, skip_serializing_if = "FocusNeighbors::is_empty")]
    pub focus_neighbors: FocusNeighbors,
    /// Accessible name (M13 G2). A named image announces `label`; a decorative one
    /// is hidden from a11y. Name-XOR-decorative is a bridge precondition, not a
    /// serde constraint. Skip-serialized when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Marks the image purely decorative (M13 G2) — hidden from a11y, no name
    /// required. Skip-serialized when `false` so a pre-G2 image round-trips
    /// byte-identically.
    #[serde(default, skip_serializing_if = "is_false")]
    pub decorative: bool,
    /// Optional reactive visibility predicate (M13 G2). See `TextWidget::visible_when`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_when: Option<Predicate>,
    /// Optional a11y role override (M13 G2). See `TextWidget::role`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
    /// Presentation-cell scope declared on this container (M13 G1b, Task 5). When
    /// present, descendant `{ local }` binds resolve against the named cells, the
    /// cells seed the app-side cell store, and the scope id keys the cell store +
    /// the reconcile/clear sweep. Absent on every pre-G1b container, so a
    /// localState-less container round-trips byte-identically (skip-serialized).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_state: Option<LocalState>,
    /// Optional reactive visibility predicate (M13 G2). See `TextWidget::visible_when`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_when: Option<Predicate>,
    /// Optional a11y role override (M13 G2). See `TextWidget::role`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    pub children: Vec<Widget>,
}

/// Grid container. Like a stack but flows `children` across a fixed number of
/// columns. Shares the stack fields; adds `cols`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
    /// Optional reactive visibility predicate (M13 G2). See `TextWidget::visible_when`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_when: Option<Predicate>,
    /// Optional a11y role override (M13 G2). See `TextWidget::role`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    pub children: Vec<Widget>,
}

/// `skip_serializing_if` predicate for boolean flags that default to `false`
/// (`restore_on_return`, `decorative`, `disabled`): omit when `false` so a
/// pre-feature widget round-trips byte-identically.
fn is_false(b: &bool) -> bool {
    !*b
}

/// Flexible-space leaf. `flex_grow` is the proportional share of leftover space
/// it claims inside its parent container.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpacerWidget {
    pub flex_grow: f32,
    /// Authored stable id (M13 Goal F, Task 3). See `TextWidget::id`. A spacer is
    /// not focusable, but it may still carry an id for neighbor references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Optional reactive visibility predicate (M13 G2). See `TextWidget::visible_when`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_when: Option<Predicate>,
    /// Optional a11y role override (M13 G2). See `TextWidget::role`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
}

/// Interactive button (M13 Goal F, Task 4). Focusable; activation — a focus-engine
/// `confirm` on the focused button, or a pointer click — fires the `on_press`
/// named reaction through the same reaction registry every entity/system reaction
/// uses. The button renders its accessible name as a centered text run.
///
/// `id` is required (unlike the optional `id` on passive widgets): activation maps
/// the focused node id back to this button's `on_press`, so the id must be stable.
/// `focus_neighbors` carries directional overrides exactly like the passive widgets.
///
/// Accessible name (M13 G2): exactly one of `label` or `labelled_by` is required
/// (the bridge enforces it). `label` is the inline name; `labelled_by` names an
/// authored node id whose text supplies the name. Both are `Option` at the type
/// level — the exactly-one-of rule is a bridge precondition, not a serde constraint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ButtonWidget {
    pub id: String,
    /// Inline accessible name (M13 G2 migration: was a required `String`, now
    /// `Option`). Skip-serialized when absent so a `labelledBy` button omits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Accessible name by reference (M13 G2): an authored node id whose text names
    /// this button. Skip-serialized when absent. Exactly-one-of with `label`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labelled_by: Option<String>,
    /// Named reaction fired on activation (confirm or click). Resolved against the
    /// reaction registry by the app — the same vocabulary entity/system reactions use.
    pub on_press: String,
    /// Directional focus-neighbor overrides (M13 Goal F, Task 3). See
    /// `TextWidget::focus_neighbors`.
    #[serde(default, skip_serializing_if = "FocusNeighbors::is_empty")]
    pub focus_neighbors: FocusNeighbors,
    /// Opt-in activation-repeat (M13 Text-Entry, Task 2). When set, a HELD confirm
    /// on this focused button re-fires `on_press` on the focus engine's existing
    /// hold-to-repeat clock — initial delay, then interval — reusing the exact wire
    /// shape (`{ initialDelayMs, intervalMs }`) of a container's nav `repeat`. This
    /// is the ONE activation-repeat exception (the on-screen keyboard's backspace);
    /// absent keeps F's single-fire rule (one `on_press` per press regardless of
    /// hold). Skip-serialized when absent so a flag-less button round-trips byte-
    /// identically with its pre-text-entry wire form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_on_hold: Option<RepeatPolicy>,
    /// Reactive selected state (M13 G2): the tab/segmented-control `aria-selected`
    /// predicate. There is NO static-bool form — selection is always reactive.
    /// Skip-serialized when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<Predicate>,
    /// Reactive checked state (M13 G2): the checkbox/radio `aria-checked` predicate.
    /// No static-bool form. Skip-serialized when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<Predicate>,
    /// Optional value-binding for `style_ranges` (M13 G2): a [`Predicate`] accepted
    /// as the styleRanges value source on a button (a later task wires
    /// `build_button` to feed its internal Text NodeContext). Passed through for
    /// round-trip here. Skip-serialized when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<Predicate>,
    /// Optional continuous value→style map (M13 G2), reusing the Text `styleRanges`
    /// shape. Meaningful alongside `bind`; passed through for round-trip here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_ranges: Option<StyleRanges>,
    /// Disabled state (M13 G2): a disabled button is non-interactive and a11y-
    /// disabled. Skip-serialized when `false` so a pre-G2 button is byte-identical.
    #[serde(default, skip_serializing_if = "is_false")]
    pub disabled: bool,
    /// Optional reactive visibility predicate (M13 G2). See `TextWidget::visible_when`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_when: Option<Predicate>,
    /// Optional a11y role override (M13 G2). See `TextWidget::role`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
}

/// Interactive slider (M13 Goal F, Task 4). Focusable; nav steps it captures
/// (`captures_nav`, e.g. `["nav.left", "nav.right"]`) adjust its value by `step`
/// within `[min, max]` and emit a `setState` write to the bound slot on the N+1
/// frame. The slider renders its accessible name and current numeric value as text.
///
/// `bind` follows the `PanelBind`/`TextBind` shape (`BindSource` + optional tween).
/// `id` is required for the same reason as `ButtonWidget::id` — nav-capture and
/// value-step resolve through the focused node id.
///
/// Accessible name (M13 G2): exactly one of `label` or `labelled_by` is required
/// (the bridge enforces it), mirroring `ButtonWidget`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SliderWidget {
    pub id: String,
    /// Inline accessible name (M13 G2 migration: was a required `String`, now
    /// `Option`). Skip-serialized when absent. Exactly-one-of with `labelled_by`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Accessible name by reference (M13 G2): an authored node id whose text names
    /// this slider. Skip-serialized when absent. Exactly-one-of with `label`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labelled_by: Option<String>,
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
    /// Disabled state (M13 G2). See `ButtonWidget::disabled`. Skip-serialized when
    /// `false` so a pre-G2 slider is byte-identical.
    #[serde(default, skip_serializing_if = "is_false")]
    pub disabled: bool,
    /// Optional reactive visibility predicate (M13 G2). See `TextWidget::visible_when`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_when: Option<Predicate>,
    /// Optional a11y role override (M13 G2). See `TextWidget::role`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
}

/// Bind source for a `slider` widget: either a `{ slot }` dotted store name or a
/// `{ local }` cell name; mirrors `PanelBind`'s `BindSource`-based shape so the
/// bind vocabulary stays uniform across bound widgets. A slider binds a numeric
/// value, so its tween follows the `TextTween` (number) shape.
//
// `deny_unknown_fields` omitted — see `TextBind` (incompatible with the flattened
// `source` alternative).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SliderBind {
    #[serde(flatten)]
    pub source: BindSource,
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BarWidget {
    pub bind: SliderBind,
    pub max: BarMax,
    pub fill: ColorValue,
    pub background: ColorValue,
    /// Authored stable id (M13 Goal F, Task 3). See `TextWidget::id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Optional continuous value→style map (M13 Goal E): recolors the `fill` band
    /// from the displayed fill fraction. Calls the widget-agnostic
    /// `style_ranges::evaluate`. Absent on a plain bar (skip-serialized), so a
    /// styleRange-less bar omits the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_ranges: Option<StyleRanges>,
    /// Optional reactive visibility predicate (M13 G2). See `TextWidget::visible_when`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_when: Option<Predicate>,
    /// Optional a11y role override (M13 G2). See `TextWidget::role`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
}

/// Denominator for a `bar` fill fraction. A literal number keeps the original
/// wire shape; a state reference uses the same dotted slot object shape as
/// widget binds (`{ "slot": "player.maxHealth" }`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BarMax {
    Literal(f32),
    State(BarMaxStateRef),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BarMaxStateRef {
    pub slot: String,
}

/// Non-visual a11y announcement (M13 G2). Lays out as nothing; its `text` is a
/// live-region message a later task routes to the platform a11y layer at the
/// declared `priority`. `text` is required; `priority` defaults to `polite` and
/// skip-serializes when polite (the `CaptureMode::is_passthrough` pattern), so an
/// `{ kind: "announce", text }` widget round-trips byte-identically.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnounceWidget {
    pub text: String,
    /// Live-region urgency. Defaults to `polite`; skip-serialized when polite.
    #[serde(default, skip_serializing_if = "Priority::is_polite")]
    pub priority: Priority,
    /// Optional reactive visibility predicate (M13 G2). See `TextWidget::visible_when`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_when: Option<Predicate>,
}

/// Live-region announcement urgency (M13 G2). `polite` waits for a pause in the
/// a11y output queue; `assertive` interrupts. `Default` + `is_polite` skip mirror
/// `CaptureMode`'s passthrough handling so a default-priority announce omits the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Priority {
    #[default]
    Polite,
    Assertive,
}

impl Priority {
    /// True for the default `Polite`. Used by `skip_serializing_if` so a polite
    /// announce omits the `priority` key (round-trip byte-identity).
    fn is_polite(&self) -> bool {
        matches!(self, Priority::Polite)
    }
}
