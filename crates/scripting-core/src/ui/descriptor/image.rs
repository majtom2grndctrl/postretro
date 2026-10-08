// The `image` widget descriptor and its authored-size contract, shared by the
// script bridges and raw serde descriptor loads.
// See: context/lib/ui.md

use serde::{Deserialize, Serialize};

use super::accessibility::Role;
use super::focus::FocusNeighbors;
use super::values::Predicate;
use super::widgets::is_false;

/// Leaf image referencing a texture asset by key. Without an authored size it
/// lays out at the asset's NATURAL reference size (content-driven, the same
/// category as text measurement): the renderer threads each asset's natural
/// size into the measure seam (see `tree::UiTree::build_draw_data`).
///
/// Authored `width` / `height` (logical-reference px) override that: one axis
/// alone keeps the source aspect, both give an exact box.
///
/// Accessible name (M13 G2): an image is name-XOR-decorative — exactly one of
/// `label` or `decorative: true` is required (the bridge enforces it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    deny_unknown_fields,
    try_from = "ImageWidgetWire"
)]
pub struct ImageWidget {
    pub asset: String,
    /// Optional authored width in logical-reference px. Skip-serialized when
    /// absent so an unsized image round-trips byte-identically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f32>,
    /// Optional authored height in logical-reference px. Skip-serialized when
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f32>,
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

/// Serde-only input shape for [`ImageWidget`], so editable JSON assets uphold
/// the same size contract as the JS and Luau bridges.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImageWidgetWire {
    asset: String,
    #[serde(default)]
    width: Option<f32>,
    #[serde(default)]
    height: Option<f32>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    focus_neighbors: FocusNeighbors,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    decorative: bool,
    #[serde(default)]
    visible_when: Option<Predicate>,
    #[serde(default)]
    role: Option<Role>,
}

impl TryFrom<ImageWidgetWire> for ImageWidget {
    type Error = String;

    fn try_from(wire: ImageWidgetWire) -> Result<Self, Self::Error> {
        let image = Self {
            asset: wire.asset,
            width: wire.width,
            height: wire.height,
            id: wire.id,
            focus_neighbors: wire.focus_neighbors,
            label: wire.label,
            decorative: wire.decorative,
            visible_when: wire.visible_when,
            role: wire.role,
        };
        image.validate()?;
        Ok(image)
    }
}

impl ImageWidget {
    /// Validate the authored size contract shared by script bridges and raw
    /// serde descriptor loads: each given axis is finite and positive.
    pub(crate) fn validate(&self) -> Result<(), String> {
        for (field, value) in [("width", self.width), ("height", self.height)] {
            if value.is_some_and(|value| !value.is_finite() || value <= 0.0) {
                return Err(format!(
                    "`image.{field}` must be a finite number greater than zero"
                ));
            }
        }
        Ok(())
    }
}
