// Theme-token resolution, value-tween easing, and shared container styling for
// the retained UI tree. Pure CPU — no taffy node creation, no GPU.
// See: context/lib/ui.md §2 (theme tokens), §3 (tween contract)

use taffy::prelude::{AlignItems, Size, Style, length};

use super::super::descriptor::{Align, Border, ColorValue, Easing, SpacingValue};
use super::super::style_ranges::StyleRanges;
use super::super::theme::UiTheme;

/// Fallback color for an unknown color token: opaque magenta. A missing token
/// degrades visibly (rather than panicking or rendering invisibly) so an
/// authoring typo is obvious on screen.
const UNKNOWN_COLOR_FALLBACK: [f32; 4] = [1.0, 0.0, 1.0, 1.0];

/// Fallback spacing for an unknown spacing token: zero logical px.
const UNKNOWN_SPACING_FALLBACK: f32 = 0.0;

// --- Value-tween easing -----------------------------------------------------

/// Identity easing: `t` unchanged.
fn linear(t: f32) -> f32 {
    t
}

/// Cubic ease-in: slow start, `t^3`.
fn ease_in(t: f32) -> f32 {
    t * t * t
}

/// Cubic ease-out: fast start, decelerating — the cubic mirror of `ease_in`.
fn ease_out(t: f32) -> f32 {
    let u = 1.0 - t;
    1.0 - u * u * u
}

/// Cubic ease-in-out: accelerate then decelerate, symmetric about `t = 0.5`.
fn ease_in_out(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let u = -2.0 * t + 2.0;
        1.0 - (u * u * u) / 2.0
    }
}

/// Dispatch an `Easing` curve, clamping `t` to `[0, 1]` first so an out-of-range
/// normalized time (a frame past the tween's end, or a negative dt) never
/// produces an eased value outside `[0, 1]`.
pub(crate) fn apply(easing: Easing, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match easing {
        Easing::Linear => linear(t),
        Easing::EaseIn => ease_in(t),
        Easing::EaseOut => ease_out(t),
        Easing::EaseInOut => ease_in_out(t),
    }
}

/// Per-channel linear lerp of two RGBA colors at eased fraction `e` (no rounding;
/// alpha included). Used by the panel tween driver to ease the fill color.
pub(crate) fn lerp_rgba(from: [f32; 4], to: [f32; 4], e: f32) -> [f32; 4] {
    [
        from[0] + (to[0] - from[0]) * e,
        from[1] + (to[1] - from[1]) * e,
        from[2] + (to[2] - from[2]) * e,
        from[3] + (to[3] - from[3]) * e,
    ]
}

/// Resolve a `ColorValue` against the active theme. A `Literal` is its own RGBA;
/// a `Token` looks the name up in the theme. An unknown token degrades to opaque
/// magenta and logs exactly one warning (per tree build, not per frame — this
/// runs at build time, which on the retained path is once per rebuild).
pub(crate) fn resolve_color(value: &ColorValue, theme: &UiTheme) -> [f32; 4] {
    match value {
        ColorValue::Literal(rgba) => *rgba,
        ColorValue::Token(name) => theme.color(name).unwrap_or_else(|| {
            log::warn!("[UI] unknown color token '{name}' — using opaque magenta fallback");
            UNKNOWN_COLOR_FALLBACK
        }),
    }
}

/// Resolve a `SpacingValue` against the active theme. A `Literal` is its own px;
/// a `Token` looks the name up. An unknown token degrades to `0.0` and logs
/// exactly one warning per tree build.
pub(crate) fn resolve_spacing(value: &SpacingValue, theme: &UiTheme) -> f32 {
    match value {
        SpacingValue::Literal(px) => *px,
        SpacingValue::Token(name) => theme.spacing(name).unwrap_or_else(|| {
            log::warn!("[UI] unknown spacing token '{name}' — using 0.0 fallback");
            UNKNOWN_SPACING_FALLBACK
        }),
    }
}

/// Resolve a `text` widget's optional `font` token to a concrete family string.
/// `None` selects the `primary` token's family; `Some(name)` looks the token up.
/// An unknown font token degrades to the `primary` family and logs exactly one
/// warning per tree build. The `primary` token is a required theme token (it
/// always resolves on the engine default), so the unwrap-to-primary path never
/// recurses into a second miss; a theme that somehow lacks `primary` falls back
/// to the embedded Inter family constant rather than panicking.
pub(crate) fn resolve_font(font: &Option<String>, theme: &UiTheme) -> String {
    let primary = || {
        theme
            .font("primary")
            .unwrap_or(super::super::text::UI_FONT_FAMILY)
            .to_string()
    };
    match font {
        None => primary(),
        Some(name) => match theme.font(name) {
            Some(family) => family.to_string(),
            None => {
                log::warn!("[UI] unknown font token '{name}' — using primary family fallback");
                primary()
            }
        },
    }
}

/// Resolve a `Border`'s theme-tokened `tint` against the active theme into a
/// concrete-RGBA `Border`. `None` passes through (no border). The `texture` and
/// `slice` are wire literals carried unchanged; only the `tint` color slot
/// resolves (a `Token` against the theme, an unknown token degrading to opaque
/// magenta + one warn via `resolve_color`).
pub(crate) fn resolve_border(border: Option<&Border>, theme: &UiTheme) -> Option<Border> {
    border.map(|b| Border {
        texture: b.texture.clone(),
        slice: b.slice,
        tint: ColorValue::Literal(resolve_color(&b.tint, theme)),
    })
}

/// Pre-resolve a `StyleRanges`' band-color tokens against the theme at build
/// time, returning a literal-only `StyleRanges`. Each band's optional `color`
/// token degrades through `resolve_color` (unknown → opaque magenta + one warn),
/// so the once-per-build warning rule holds and the per-frame draw walk stays
/// theme-free: the draw-time evaluator only ever sees `ColorValue::Literal`
/// bands. `up_to`/`pulse`/`flash` carry through unchanged.
fn resolve_style_ranges(ranges: &StyleRanges, theme: &UiTheme) -> StyleRanges {
    use super::super::style_ranges::StyleEntry;
    StyleRanges {
        max: ranges.max,
        entries: ranges
            .entries
            .iter()
            .map(|entry| StyleEntry {
                up_to: entry.up_to,
                color: entry
                    .color
                    .as_ref()
                    .map(|c| ColorValue::Literal(resolve_color(c, theme))),
                pulse: entry.pulse,
                flash: entry.flash,
            })
            .collect(),
    }
}

/// Resolve a widget's optional `style_ranges` for the retained node at build
/// time: pre-resolve its band-color tokens (theme-free draw walk) and enforce the
/// `bind` precondition. styleRanges maps the widget's bound value, so without a
/// `bind` there is no value to map — warn exactly once per tree build (the
/// theme-fallback precedent) and drop it (the node carries `None`, no effect
/// fires). `kind` names the widget in the warning.
pub(crate) fn build_node_style_ranges(
    style_ranges: Option<&StyleRanges>,
    has_bind: bool,
    theme: &UiTheme,
    kind: &str,
) -> Option<StyleRanges> {
    let ranges = style_ranges?;
    if !has_bind {
        log::warn!(
            "[UI] {kind} widget declares styleRanges without a bind — no value to map; ignoring"
        );
        return None;
    }
    Some(resolve_style_ranges(ranges, theme))
}

/// Live tween state for a bound node whose bind carries a `tween`. Absent on
/// untweened binds. `display` is the value the draw step renders THIS frame;
/// `start`/`start_time`/`target` describe the in-flight segment the driver eases
/// across. A retarget restarts the segment from the current `display` (never
/// snapping mid-flight), so `start` is the display value at the retarget instant,
/// not the bind's `from`. `T` is `f32` for text, `[f32; 4]` for panel.
#[derive(Debug, Clone)]
pub(crate) struct TweenState<T> {
    /// Value rendered this frame (eased toward `target` from `start`).
    pub display: T,
    /// Value the active segment eased from (set at first-resolve or retarget).
    pub start: T,
    /// Frame time (seconds) the active segment started easing at.
    pub start_time: f64,
    /// Value the active segment eases toward.
    pub target: T,
}

/// Map descriptor cross-axis `Align` to taffy `AlignItems`.
fn align_items(align: Align) -> AlignItems {
    match align {
        Align::Start => AlignItems::Start,
        Align::Center => AlignItems::Center,
        Align::End => AlignItems::End,
        Align::Stretch => AlignItems::Stretch,
    }
}

/// Container (stack/grid) shared style: scalar `padding` → all four edges,
/// `gap` → both axes, `align` → `align_items`.
pub(crate) fn container_base_style(gap: f32, padding: f32, align: Align) -> Style {
    Style {
        align_items: Some(align_items(align)),
        gap: Size {
            width: length(gap),
            height: length(gap),
        },
        padding: taffy::geometry::Rect {
            left: length(padding),
            right: length(padding),
            top: length(padding),
            bottom: length(padding),
        },
        ..Default::default()
    }
}
