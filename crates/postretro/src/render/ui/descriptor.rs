// Serde descriptor model for the UI widget tree: the internally-tagged `Widget`
// enum (seven kinds), its field structs, and the `AnchoredTree` placement
// envelope. Pure data — no rendering, no taffy, no retained tree.
// See: context/plans/in-progress/M13--descriptor-tree-layout

use serde::{Deserialize, Serialize};

use super::layout::Anchor;

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

impl ColorValue {
    /// The literal RGBA when this is a `Literal`, else `fallback`. Token
    /// resolution against a theme is a later task; until then a token slot reads
    /// as the caller's fallback so literal-only paths behave exactly as before.
    pub(crate) fn as_literal_or(&self, fallback: [f32; 4]) -> [f32; 4] {
        match self {
            ColorValue::Literal(rgba) => *rgba,
            ColorValue::Token(_) => fallback,
        }
    }
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

impl SpacingValue {
    /// The literal px when this is a `Literal`, else `fallback`. Token resolution
    /// is a later task; see `ColorValue::as_literal_or`.
    pub(crate) fn as_literal_or(&self, fallback: f32) -> f32 {
        match self {
            SpacingValue::Literal(px) => *px,
            SpacingValue::Token(_) => fallback,
        }
    }
}

/// Top-level placement envelope wrapping the root widget. `anchor`/`offset`
/// live ONLY here, not on widget variants: a widget tree is placed once, as a
/// whole, against the logical-reference canvas (see `layout::Anchor`). `offset`
/// is logical-reference px, `[x, y]` (+x right, +y down), matching
/// `UiElement::offset`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchoredTree {
    pub anchor: Anchor,
    pub offset: [f32; 2],
    pub root: Widget,
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
}

/// Leaf text run. `content` is the literal string; `font_size` is logical px;
/// `color` is linear RGBA. The run is sized by the glyphon measure seam and laid
/// out in its container's flow.
///
/// `bind` is the optional state-binding (Goal C): when present, the rendered
/// string is resolved from a store slot at draw-data build time and `content`
/// serves only as the fallback for an absent slot (see `tree::resolve_text`).
/// Absent on every static widget, so unbound text round-trips unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextWidget {
    pub content: String,
    pub font_size: f32,
    pub color: ColorValue,
    /// Optional theme font name. Absent on every pre-theming widget, so fontless
    /// text keeps its old wire form exactly (the key is omitted, not `null`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<TextBind>,
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
}

/// Solid-fill panel with an optional 9-slice border. `fill` is linear RGBA. The
/// panel fills its flex/grid slot (it has no intrinsic size). Container-level
/// backdrops (the splash's framed panel) are expressed as a `ContainerWidget`
/// `fill`/`border` instead — a parent drawing its own backdrop beneath flowed
/// children — so an overlapping composition needs no standalone sized panel.
///
/// `bind` is the optional state-binding (Goal C): when present, the panel `fill`
/// is resolved from a store slot holding a length-4 linear-RGBA array at
/// draw-data build time, with the literal `fill` serving as the fallback for an
/// absent or malformed slot (see `tree::resolve_panel_fill`). Absent on static
/// panels, so unbound panels round-trip unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelWidget {
    pub fill: ColorValue,
    pub border: Option<Border>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<PanelBind>,
}

/// State binding for a `panel` widget. `slot` is a dotted slot name whose value
/// must be a `SlotValue::Array` of exactly 4 f32 (linear `[r, g, b, a]`); it
/// replaces the literal `fill`. A wrong variant, wrong length, or absent slot
/// falls back to the literal `fill` (see `tree::resolve_panel_fill`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelBind {
    pub slot: String,
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
    pub children: Vec<Widget>,
}

/// Flexible-space leaf. `flex_grow` is the proportional share of leftover space
/// it claims inside its parent container.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpacerWidget {
    pub flex_grow: f32,
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
        let with_font =
            r#"{"kind":"text","content":"x","fontSize":12.0,"color":[1.0,1.0,1.0,1.0],"font":"mono"}"#;
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
}
