// Serde descriptor model for the UI widget tree: the internally-tagged `Widget`
// enum (seven kinds), its field structs, and the `AnchoredTree` placement
// envelope. Pure data — no rendering, no taffy, no retained tree.
// See: context/plans/in-progress/M13--descriptor-tree-layout

use serde::{Deserialize, Serialize};

use super::layout::Anchor;

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
/// `color` is linear RGBA.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextWidget {
    pub content: String,
    pub font_size: f32,
    pub color: [f32; 4],
}

/// Solid-fill panel with an optional 9-slice border. `fill` is linear RGBA.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelWidget {
    pub fill: [f32; 4],
    pub border: Option<Border>,
}

/// Leaf image referencing a texture asset by key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageWidget {
    pub asset: String,
}

/// Stack container (`vstack`/`hstack`). Lays its `children` out along one axis
/// with `gap` between them, `padding` inside its bounds, and cross-axis
/// `align`. `children` carries no `skip_serializing_if`: an empty container
/// must serialize `"children":[]` so round-trip identity holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerWidget {
    pub gap: f32,
    pub padding: f32,
    pub align: Align,
    pub children: Vec<Widget>,
}

/// Grid container. Like a stack but flows `children` across a fixed number of
/// columns. Shares the stack fields; adds `cols`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GridWidget {
    pub gap: f32,
    pub padding: f32,
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
    pub tint: [f32; 4],
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
}
