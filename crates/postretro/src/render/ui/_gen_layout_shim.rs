// Generator-bin-only shim. Included ONLY by `src/bin/gen_script_types.rs` via
// `#[path]` to stand in for `layout::Anchor` without dragging the renderer's
// GPU-coupled draw-list projection into the typedef generator. NOT part of the
// engine's `render::ui::layout` (that is `layout.rs`). See the bin's header.
//
// MUST mirror every variant of `layout::Anchor` in layout.rs — add a variant
// there, add it here. The `widget_anchor_typedef_matches_layout_anchor_variants`
// test guards the emitted union, but this shim is a separate compilation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}
