// Shared descriptor value/leaf types: color & spacing slots, alignment, the
// 9-slice border, easing curves, presentation-cell init values, and the
// `{ slot } | { local }` bind source — the small data types widget structs share.
// See: context/lib/ui.md

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Border {
    pub texture: String,
    pub slice: [f32; 4],
    pub tint: ColorValue,
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

/// The source a widget bind reads from — the `{ slot }` vs `{ local }` wire
/// alternative shared by every bound widget (M13 G1b, Task 5). Untagged so the
/// wire form stays a flat sibling key inside the bind object: a store binding is
/// `{ "slot": "player.health" }`; a presentation-cell binding is
/// `{ "local": "count" }`. The two are disjoint (each carries a different key),
/// so serde's untagged dispatch is unambiguous.
///
/// `Slot` is declared FIRST so a bind object carrying a `slot` key lands on it
/// (untagged variants are tried in declaration order). `Slot` references the
/// authoritative store by dotted name; `Local` references a presentation cell
/// declared on the nearest ancestor's `localState` scope BY NAME — the scope id
/// is resolved at tree-build time against the nearest declaring ancestor, never
/// authored on the bind itself (so the bind stays scope-agnostic and the same
/// descriptor round-trips byte-identically).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BindSource {
    /// Authoritative store slot read by dotted name (`"player.health"`).
    Slot { slot: String },
    /// Presentation cell read by name from the nearest `localState` scope.
    Local { local: String },
}

impl BindSource {
    /// The store slot name when this is a `{ slot }` binding, else `None`. The
    /// retained tree's tween/styleRanges paths that read the raw store snapshot
    /// use this; a `{ local }` binding has no store slot.
    pub fn slot(&self) -> Option<&str> {
        match self {
            BindSource::Slot { slot } => Some(slot),
            BindSource::Local { .. } => None,
        }
    }
}

/// A scalar comparand for a [`Predicate`] (M13 G2). v1 admits ONLY a number,
/// boolean, or string — the three SlotValue/CellInit shapes a predicate equality
/// can meaningfully compare against. Untagged so the wire form is a bare JSON
/// scalar (`5`, `true`, `"on"`), with no wrapper object. An rgba/array comparand
/// is deliberately unrepresentable: a JSON array matches none of these variants,
/// so serde rejects it at deserialize time and the hand-written bridge surfaces
/// the same as a named load-time error (see `data_descriptors`).
///
/// `Number` is declared first so an integral JSON literal lands on it (untagged
/// variants are tried in declaration order; a JSON number can only match
/// `Number`, a bool only `Boolean`, a string only `String` — the forms are
/// disjoint and ordering merely pins the number path first).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PredicateValue {
    Number(f64),
    Boolean(bool),
    String(String),
}

/// A reactive predicate (M13 G2): a [`BindSource`] read against an optional
/// `equals` comparand. With `equals` present the predicate is true when the bound
/// value equals the comparand; with `equals` absent it is the bound value's own
/// truthiness (a later task — Task 2a — defines resolution). Carried by a widget's
/// `visibleWhen`, a button's `selected`/`checked`, and accepted as a `bind` source
/// for a button's styleRanges.
///
/// The `source` is `#[serde(flatten)]` so the wire form keeps `slot`/`local` as
/// flat siblings of `equals`: `{ "slot": "hud.tab", "equals": "stats" }` or
/// `{ "local": "open" }`. `equals` skip-serializes when absent, so a truthiness
/// predicate round-trips byte-identically with no `equals` key.
//
// `deny_unknown_fields` is omitted: it is incompatible with `#[serde(flatten)]`,
// which the `source` alternative requires. The shape is otherwise closed by
// `BindSource` + `PredicateValue`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Predicate {
    #[serde(flatten)]
    pub source: BindSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<PredicateValue>,
}

/// Declared initial value for a presentation cell (M13 G1b, Task 5). Mirrors the
/// `SlotValue` shapes a bind resolves: a number, boolean, string, or length-4
/// linear-RGBA array. Untagged so the wire form is a bare JSON scalar/array —
/// `{ "count": 0 }`, `{ "flash": [1,0,0,1] }` — with no wrapper object. `Number`
/// is declared first so an integral JSON literal lands on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CellInit {
    Number(f64),
    Boolean(bool),
    Array([f32; 4]),
    String(String),
}

/// Presentation-cell scope declared on a container (M13 G1b, Task 5). `scope` is
/// a stable id (author-supplied or SDK-stabilized) addressable from BOTH the app
/// stage (cell writes) and the render stage (`{ local }` bind resolution). `cells`
/// maps each cell name to its declared initial value, used to seed the app-side
/// cell store the first time this scope is composed.
///
/// This is presentation-only state — NOT the authoritative store (`ui.md` §3/§6):
/// no schema, no persistence, no dotted-name namespace. `cells` is a `BTreeMap`
/// so serialization is deterministic (stable key order) and the descriptor
/// round-trips byte-identically.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalState {
    pub scope: String,
    pub cells: BTreeMap<String, CellInit>,
}
