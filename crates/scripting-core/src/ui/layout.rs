// Generator-bin-only shim. Included ONLY by `src/bin/gen_script_types.rs` via
// `#[path]` to stand in for `layout::Anchor` without dragging the renderer's
// GPU-coupled draw-list projection into the typedef generator. NOT part of the
// engine's `render::ui::layout` (that is `layout.rs`). See the bin's header.
//
// MUST mirror `layout::Anchor` in layout.rs — its variants AND its `ALL`/`wire`
// API — add a variant there, add it here. `typedef.rs` is compiled into BOTH
// the engine bin (real `layout`) and this generator bin (this shim); the
// `widget_anchor_typedef_matches_layout_anchor_variants` test derives the
// expected `WidgetAnchor` union from `Anchor::ALL`/`wire()`, so this shim must
// expose the same API or the gen-bin test build breaks.
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

impl Anchor {
    /// Mirror of `layout::Anchor::ALL`. Keep in lockstep with layout.rs,
    /// including the `#[cfg(test)]` gate (test-only drift-guard support).
    #[cfg(test)]
    pub const ALL: &[Anchor] = &[
        Anchor::TopLeft,
        Anchor::Top,
        Anchor::TopRight,
        Anchor::Left,
        Anchor::Center,
        Anchor::Right,
        Anchor::BottomLeft,
        Anchor::Bottom,
        Anchor::BottomRight,
    ];

    /// Mirror of `layout::Anchor::wire`. Exhaustive `match` (no `_` arm) — keep
    /// in lockstep with layout.rs, including the `#[cfg(test)]` gate.
    #[cfg(test)]
    pub fn wire(self) -> &'static str {
        match self {
            Anchor::TopLeft => "topLeft",
            Anchor::Top => "top",
            Anchor::TopRight => "topRight",
            Anchor::Left => "left",
            Anchor::Center => "center",
            Anchor::Right => "right",
            Anchor::BottomLeft => "bottomLeft",
            Anchor::Bottom => "bottom",
            Anchor::BottomRight => "bottomRight",
        }
    }
}
