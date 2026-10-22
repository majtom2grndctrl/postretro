// Ghost tint: the luminance a ghost's tint draws at, and the tint that presents
// a held brightness at its hue, placed on the 8-bit grid the draw decodes.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use super::value::{FLASH_DARK_LIMIT, luminance};
use crate::ghost_instance::drawn_tint;

/// A tint at or below this luminance has no hue to keep: it presents as grey.
const BLACK_LUMINANCE: f32 = 1.0e-4;

/// At or below this presented opacity a ghost draws nothing its tint could
/// change, so it presents its asked tint.
pub(super) const INVISIBLE_OPACITY: f32 = 1.0e-4;

/// The luminance of the 8-bit tint the draw decodes, so packing can never
/// carry a tint across a flash threshold the clamp judged it short of.
pub(super) fn drawn_luminance(tint: [f32; 3]) -> f32 {
    luminance(drawn_tint(tint)).clamp(0.0, 1.0)
}

/// What a ghost draws at `opacity` with `tint` over black.
pub(super) fn drawn_brightness(opacity: f32, tint: [f32; 3]) -> f32 {
    opacity * drawn_luminance(tint)
}

/// The tint presented at luminance `held`, keeping its hue: scaled to `held`,
/// or, where even its brightest scale (a component at 1) is darker than
/// `held`, that scale mixed toward white, which keeps the hue and gives up
/// saturation. A black tint has no hue and presents as a grey of `held`.
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

/// `tint` on the 8-bit grid the draw decodes, rounded so that at `opacity` it
/// draws a brightness in `low..=high`. Nearest rounding keeps the tint
/// closest; then every channel rounded down, or up, and one step past each.
/// Where no grid tint lands in the range, the darkest one at or above `low`,
/// since `low` carries the dark limits the clamp judged against.
pub(super) fn on_grid(tint: [f32; 3], opacity: f32, low: f32, high: f32) -> [f32; 3] {
    let steps = tint.map(|channel| channel.clamp(0.0, 1.0) * 255.0);
    let grid = |round: fn(f32) -> f32| steps.map(|step| round(step).clamp(0.0, 255.0) / 255.0);
    let candidates = [
        grid(f32::round),
        grid(f32::floor),
        grid(f32::ceil),
        grid(|step| step.floor() - 1.0),
        grid(|step| step.ceil() + 1.0),
    ];
    let drawn = |rgb: &[f32; 3]| drawn_brightness(opacity, *rgb);
    candidates
        .iter()
        .find(|rgb| (low..=high).contains(&drawn(rgb)))
        .or_else(|| {
            candidates
                .iter()
                .filter(|rgb| drawn(rgb) >= low)
                .min_by(|a, b| drawn(a).total_cmp(&drawn(b)))
        })
        .copied()
        .unwrap_or(candidates[0])
}

/// The tint that draws brightness `held` at presented `opacity`, from the
/// (red-limited) tint whose drawn luminance is `luminance`. It passes
/// unchanged when the clamp presents what was asked at that opacity, or when
/// the ghost is invisible. Otherwise it is re-aimed at luminance
/// `held / opacity`, keeping its hue, and placed on the 8-bit grid on the side
/// of `held` away from what was asked: no brighter than a held or slowed rise,
/// no darker than a slowed fall. Rounding never makes a dark state the clamp
/// judged not to be one: a brightness at or above the dark limit draws at or
/// above it, and a darkening at or below its own limit draws at or below it,
/// even a grid step past `held`. The clamp keeps `held` as presented, so the
/// rounding never builds up from frame to frame.
pub(super) fn presented_tint(
    tint: [f32; 3],
    opacity: f32,
    luminance: f32,
    held: f32,
) -> [f32; 3] {
    let asked = opacity * luminance;
    if held == asked || opacity <= INVISIBLE_OPACITY {
        return tint;
    }
    let aimed = tint_at_luminance(tint, (held / opacity).min(1.0));
    let (mut low, high) = if held > asked {
        (held, f32::INFINITY)
    } else {
        (f32::NEG_INFINITY, held)
    };
    if held >= FLASH_DARK_LIMIT {
        low = low.max(FLASH_DARK_LIMIT);
    }
    // Over white the darkening draws one minus itself.
    if 1.0 - (opacity - held) >= FLASH_DARK_LIMIT {
        low = low.max(opacity - (1.0 - FLASH_DARK_LIMIT));
    }
    on_grid(aimed, opacity, low, high)
}
