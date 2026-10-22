// Per-ghost look clamp: each ghost's opacity, the tint's redness and the ghost's
// drawn brightness on one flash window per ghost, so together they present at
// most three flashes a second.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use std::hash::Hash;

use super::keyed::{KeyClamp, KeyedClamp};
use super::value::{
    FlashWindow, Part, Strength, StrengthAndRed, luminance, unit_strength, vanishing,
};
use crate::flash_limiter::{LimiterFrameInput, LimiterFrameUniform, pack_limiter_frame};
use crate::ghost_instance::{GhostInstanceLook, drawn_tint};

/// A tint at or below this luminance has no hue to keep: it presents as grey.
const BLACK_LUMINANCE: f32 = 1.0e-4;

/// At or below this presented opacity a ghost draws nothing its tint could
/// change, so it presents its asked tint.
const INVISIBLE_OPACITY: f32 = 1.0e-4;

/// What a ghost draws at `opacity` with `tint` over black: the opacity times
/// the luminance of the 8-bit tint the draw decodes, so packing can never carry
/// a tint across a flash threshold the clamp judged it short of.
fn drawn_brightness(opacity: f32, tint: [f32; 3]) -> f32 {
    opacity * luminance(drawn_tint(tint))
}

/// The tint presented at luminance `held`, keeping its hue: scaled to `held`,
/// or, where even its brightest scale (a component at 1) is darker than
/// `held`, that scale mixed toward white, which keeps the hue and gives up
/// saturation. A black tint has no hue and presents as a grey of `held`.
/// Either way its luminance is `held`, before 8-bit packing.
fn tint_at_luminance(tint: [f32; 3], held: f32) -> [f32; 3] {
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
/// draws no brighter than `held` (`at_least` false) or no darker (`at_least`
/// true). Nearest rounding keeps the tint closest. Where that lands on the
/// wrong side, every channel rounds the other way, and where even that sits a
/// rounding residue past `held`, one step further.
fn on_grid(tint: [f32; 3], opacity: f32, held: f32, at_least: bool) -> [f32; 3] {
    let steps = tint.map(|channel| channel.clamp(0.0, 1.0) * 255.0);
    let grid = |round: fn(f32) -> f32| steps.map(|step| round(step).clamp(0.0, 255.0) / 255.0);
    let on_side = |rgb: [f32; 3]| {
        let drawn = drawn_brightness(opacity, rgb);
        if at_least {
            drawn >= held
        } else {
            drawn <= held
        }
    };
    let nearest = grid(f32::round);
    if on_side(nearest) {
        return nearest;
    }
    let edge = if at_least {
        grid(f32::ceil)
    } else {
        grid(f32::floor)
    };
    if on_side(edge) {
        return edge;
    }
    if at_least {
        grid(|step| step.ceil() + 1.0)
    } else {
        grid(|step| step.floor() - 1.0)
    }
}

/// The tint that draws brightness `held` at presented `opacity`, from the
/// (red-limited) tint that would draw `asked`. It passes unchanged when the
/// clamp presents what was asked, or when the ghost is invisible. Otherwise
/// it is re-aimed at luminance `held / opacity`, keeping its hue, and placed
/// on the 8-bit grid on the side of `held` the clamp limited from: no
/// brighter than a held or slowed rise, no darker than a slowed fall. Its
/// drawn excursions and dark states are therefore never past the ones the
/// clamp judged, while the clamp keeps `held` as presented, so the rounding
/// never builds up from frame to frame.
fn tint_at_brightness(tint: [f32; 3], opacity: f32, asked: f32, held: f32) -> [f32; 3] {
    if held == asked || opacity <= INVISIBLE_OPACITY {
        return tint;
    }
    let aimed = tint_at_luminance(tint, (held / opacity).min(1.0));
    on_grid(aimed, opacity, held, held > asked)
}

/// One ghost's look through the limiter. Its opacity, its tint's redness
/// (weighted by opacity) and its drawn brightness (opacity times the tint's
/// Rec. 709 luminance, its look over black) are three parts of one merging
/// flash window. An onset of any part is admitted only while the window holds
/// it, its return, and the returns every part's earlier onsets still owe, and
/// an opacity change that moves the brightness on the same frame counts once.
/// Each part keeps its own rate cap.
///
/// Opacity bounds the change over any background: a black-tinted ghost
/// strobing its opacity over a bright wall darkens the wall, which brightness
/// over black never sees. Brightness catches what opacity alone misses: a tint
/// strobe, or opacity and tint moving together.
#[derive(Clone, Copy, Debug)]
struct LookValueClamp {
    window: FlashWindow,
    /// Strength: the presented opacity. Colour: the tint, judged for redness.
    opacity: StrengthAndRed,
    /// The drawn brightness.
    brightness: Strength,
}

impl KeyClamp for LookValueClamp {
    type Value = GhostInstanceLook;

    fn fresh(look: GhostInstanceLook) -> Self {
        let mut opacity = StrengthAndRed::default();
        opacity.restart(look.opacity, look.tint);
        let mut brightness = Strength::default();
        brightness.restart(drawn_brightness(unit_strength(look.opacity), look.tint));
        Self {
            window: FlashWindow::merging(),
            opacity,
            brightness,
        }
    }

    /// Opacity is limited first and, over budget, desaturates a red tint at
    /// unchanged luminance. The drawn brightness is limited next on the same
    /// window, from the presented opacity; the tint then presents the held
    /// brightness at that opacity, keeping its hue. Saturation passes through
    /// unlimited: it changes what draws only where a coloured tint meets a
    /// coloured texture, which the clamp cannot see.
    fn limit(&mut self, look: GhostInstanceLook, frame: &LimiterFrameUniform) -> GhostInstanceLook {
        self.window.age(frame.dt_window);
        let (opacity, tint) = self
            .opacity
            .limit(&mut self.window, look.opacity, look.tint, frame);
        let asked = drawn_brightness(opacity, tint);
        let held = self
            .brightness
            .limit(&mut self.window, Part::Brightness, asked, frame);
        GhostInstanceLook {
            opacity,
            saturation: look.saturation,
            tint: tint_at_brightness(tint, opacity, asked, held),
        }
    }

    /// A vanished ghost's opacity falls to nothing and that fall counts. Its
    /// drawn brightness falls with it on the same frame, the same change, so
    /// the two count once.
    fn absent(&mut self, frame: &LimiterFrameUniform) {
        self.window.age(frame.dt_window);
        let frame = vanishing(frame);
        self.opacity.limit(&mut self.window, 0.0, [0.0; 3], &frame);
        self.brightness
            .limit(&mut self.window, Part::Brightness, 0.0, &frame);
    }

    fn is_at_rest(&self) -> bool {
        self.window.is_empty()
            && self.opacity.presents_nothing()
            && self.brightness.presented() == 0.0
    }
}

/// Every ghost's look through the photosensitivity limiter, the one place a
/// ghost draw collector limits a look. Keys are a collector's ghost identity
/// (its link and generation-checked entity), so an entity seen through two
/// links is two ghosts with their own history.
///
/// Each frame: [`Self::begin_frame`], then [`Self::present`] once per
/// presented ghost (culled or not: the limiter judges the source, not its
/// framing), then [`Self::end_frame`], then [`Self::finish_presentation`] once
/// the renderer knows whether the frame presented. A frame begun with no
/// limiter input (a capture) passes every look through and leaves history
/// untouched.
///
/// Each ghost's opacity, tint redness and drawn brightness share one flash
/// window, so one ghost presents at most 3 flashes in any 1 s across all
/// three. A ghost not presented on a frame vanished: its opacity and drawn
/// brightness fall to nothing together, and that fall counts once.
#[derive(Clone, Debug)]
pub struct GhostLookClamp<K> {
    /// Every ghost's look clamp.
    looks: KeyedClamp<K, LookValueClamp>,
    /// `looks` as the unsettled frame began, restored if it never presents.
    /// Overwritten in place each frame. Both tables start allocated at the
    /// same size and only swap, so the snapshot allocates only when `looks`
    /// outgrows it.
    frame_start: KeyedClamp<K, LookValueClamp>,
    /// The limiter input of the frame begun and not yet settled.
    unsettled: Option<LimiterFrameUniform>,
    /// Presented time of frames that never presented, which the next limited
    /// frame takes on.
    unconsumed: Option<LimiterFrameInput>,
    /// This frame runs the limiter: it was begun with a limiter input.
    limiting: bool,
}

impl<K> Default for GhostLookClamp<K> {
    fn default() -> Self {
        Self {
            looks: KeyedClamp::default(),
            frame_start: KeyedClamp::default(),
            unsettled: None,
            unconsumed: None,
            limiting: false,
        }
    }
}

impl<K: Copy + Eq + Hash> GhostLookClamp<K> {
    /// Start a frame. `frame` is this frame's limiter input (the ghost ramp's
    /// presented-frame time and the fail-safe enable flag), or `None` for a
    /// capture, which never involves the limiter. Time from earlier frames
    /// that never presented merges into it. A frame begun before the last one
    /// settled settles that one as presented.
    pub fn begin_frame(&mut self, frame: Option<&LimiterFrameUniform>) {
        self.limiting = frame.is_some();
        self.unsettled = None;
        let Some(frame) = frame else {
            return;
        };
        let frame = match self.unconsumed.take() {
            Some(unconsumed) => pack_limiter_frame(
                unconsumed.merge(LimiterFrameInput {
                    elapsed_seconds: frame.dt_window,
                }),
                frame.enabled,
                frame.init,
            ),
            None => *frame,
        };
        self.frame_start.clone_from(&self.looks);
        self.looks.begin_frame(&frame);
        self.unsettled = Some(frame);
    }

    /// The look ghost `key` presents this frame, from its unclamped look
    /// (`opacity` already the link's opacity times membership).
    ///
    /// Opacity is limited and, over budget, desaturates a red tint at
    /// unchanged luminance. The drawn brightness (opacity times the tint's
    /// luminance) is limited on the same window, and over budget the tint
    /// presents the held brightness at the presented opacity, keeping its
    /// hue. Saturation passes through unlimited: it changes what draws only
    /// where a coloured tint meets a coloured texture, which the clamp cannot
    /// see.
    pub fn present(&mut self, key: K, look: GhostInstanceLook) -> GhostInstanceLook {
        if !self.limiting {
            return look;
        }
        self.looks.limit(key, look)
    }

    /// End the frame: a ghost not presented this frame vanished. Its opacity
    /// and drawn brightness fall to nothing, counted once, and its history is
    /// kept until its clamp is at rest.
    pub fn end_frame(&mut self) {
        if !self.limiting {
            return;
        }
        self.looks.end_frame();
    }

    /// Settle the frame last begun once the renderer knows whether it
    /// presented. A frame that presented nothing (its surface acquire failed)
    /// never consumes its presented time: every ghost's clamp returns to where
    /// the frame began, and its time merges into the next limited frame's, as
    /// the resolve's limiter input does.
    pub fn finish_presentation(&mut self, presented: bool) {
        let Some(frame) = self.unsettled.take() else {
            return;
        };
        if presented {
            return;
        }
        std::mem::swap(&mut self.looks, &mut self.frame_start);
        // The frame's time already holds any earlier unconsumed time.
        self.unconsumed = Some(LimiterFrameInput {
            elapsed_seconds: frame.dt_window,
        });
    }
}

#[cfg(test)]
#[path = "ghost_look_tests.rs"]
mod tests;
