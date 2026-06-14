// Generator-bin-only shim. Included ONLY by `src/bin/gen_script_types.rs` via
// `#[path]` to stand in for `layout::Anchor` without dragging the renderer's
// GPU-coupled draw-list projection into the typedef generator. NOT part of the
// engine's `render::ui::layout` (that is `layout.rs`). See the bin's header.
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
