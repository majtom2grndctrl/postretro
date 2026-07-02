// Scripting-core UI anchor definition. Shared by SDK typedef generation and
// re-exported by the CPU `postretro-ui` layout module.
//
// MUST expose every authored anchor variant plus `ALL`/`wire`. The
// `widget_anchor_typedef_matches_layout_anchor_variants` test derives the
// expected `WidgetAnchor` union from `Anchor::ALL`/`wire()`.
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
    /// Source list used by downstream drift guards.
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

    /// Stable wire spelling for each anchor. Exhaustive `match` (no `_` arm).
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
