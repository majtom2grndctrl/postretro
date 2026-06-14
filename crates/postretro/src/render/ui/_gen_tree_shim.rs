// Generator-bin-only shim. Included ONLY by `src/bin/gen_script_types.rs` via
// `#[path]` to satisfy `style_ranges::evaluate`'s call to `tree::resolve_color`
// without dragging the renderer's GPU-coupled `tree.rs` into the typedef
// generator. The bridge never evaluates styleRanges, so this pass-through is
// never exercised at runtime. NOT part of the engine's `render::ui::tree`.
use super::descriptor::ColorValue;
use super::theme::UiTheme;

pub(crate) fn resolve_color(value: &ColorValue, _theme: &UiTheme) -> [f32; 4] {
    match value {
        ColorValue::Literal(rgba) => *rgba,
        // Unknown-token resolution lives in the real renderer; the generator
        // never evaluates styleRanges, so a magenta stub mirrors the engine's
        // degrade-visibly token fallback.
        ColorValue::Token(_) => [1.0, 0.0, 1.0, 1.0],
    }
}
