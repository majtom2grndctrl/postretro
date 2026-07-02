// Typedef-support color resolver for scripting-core `style_ranges::evaluate`.
// Runtime UI color resolution lives in the CPU `postretro-ui` tree.
// The bridge never evaluates styleRanges, so this pass-through is not a runtime
// UI path.
use super::descriptor::ColorValue;
use super::theme::UiTheme;

pub fn resolve_color(value: &ColorValue, _theme: &UiTheme) -> [f32; 4] {
    match value {
        ColorValue::Literal(rgba) => *rgba,
        // Unknown-token resolution lives in `postretro-ui`; the generator
        // never evaluates styleRanges, so a magenta stub mirrors the engine's
        // degrade-visibly token fallback.
        ColorValue::Token(_) => [1.0, 0.0, 1.0, 1.0],
    }
}
