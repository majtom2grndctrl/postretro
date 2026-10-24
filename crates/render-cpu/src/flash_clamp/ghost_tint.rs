// Ghost tint: the luminance a ghost's tint draws at, and the 8-bit tint that
// draws a held luminance at its hue.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use super::value::luminance;
use crate::ghost_instance::drawn_tint;

/// A tint at or below this luminance has no hue to keep: it presents as grey.
const BLACK_LUMINANCE: f32 = 1.0e-4;

/// The luminance of the 8-bit tint the draw decodes, so the clamp judges the
/// tint that draws.
pub(super) fn drawn_luminance(tint: [f32; 3]) -> f32 {
    luminance(drawn_tint(tint)).clamp(0.0, 1.0)
}

/// `tint` re-aimed at luminance `held` (see [`tint_at_luminance`]) and placed
/// on the 8-bit grid the draw decodes, with the luminance it draws at. The
/// rounding moves that luminance at most a grid step from `held`; the clamp
/// judges the rounded value, so rounding never carries a tint past what was
/// judged.
pub(super) fn on_grid_at_luminance(tint: [f32; 3], held: f32) -> ([f32; 3], f32) {
    let grid = drawn_tint(tint_at_luminance(tint, held));
    (grid, drawn_luminance(grid))
}

/// The tint presented at luminance `held`, keeping its hue angle: scaled to
/// `held`, or, where even its brightest scale (a component at 1) is darker
/// than `held`, that scale mixed toward white, which keeps the hue and gives
/// up saturation. A black tint has no hue and presents as a grey of `held`.
/// Either way its luminance is `held`, before 8-bit packing.
pub(super) fn tint_at_luminance(tint: [f32; 3], held: f32) -> [f32; 3] {
    let current = luminance(tint);
    if current == held {
        return tint;
    }
    let peak = tint[0].max(tint[1]).max(tint[2]);
    let mut presented = if current <= BLACK_LUMINANCE {
        [held; 3]
    } else if held * peak <= current {
        let scale = held / current;
        tint.map(|channel| (channel * scale).min(1.0))
    } else {
        toward_white(tint.map(|channel| channel / peak), held)
    };
    // Re-aim at `held` once against the f32 rounding of the luma sum.
    let measured = luminance(presented);
    if measured > BLACK_LUMINANCE && measured != held {
        presented = if measured > held {
            presented.map(|channel| channel * (held / measured))
        } else {
            toward_white(presented, held)
        };
    }
    presented
}

/// `rgb`, no component above 1, mixed toward white until its luminance is
/// `held`. The mix keeps the hue: each component's share of the spread from
/// the darkest to the brightest is unchanged.
fn toward_white(rgb: [f32; 3], held: f32) -> [f32; 3] {
    let lum = luminance(rgb);
    if lum >= 1.0 {
        return rgb;
    }
    let mix = ((held - lum) / (1.0 - lum)).clamp(0.0, 1.0);
    rgb.map(|channel| channel + (1.0 - channel) * mix)
}
