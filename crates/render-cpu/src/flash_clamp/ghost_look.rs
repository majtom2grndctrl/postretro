// Per-ghost look clamp: each ghost's opacity, the tint's redness and the tint's
// luminance on one flash window per ghost, so together they present at most
// three flashes a second.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use std::hash::Hash;

use super::keyed::{KeyClamp, KeyedClamp};
use super::value::{FlashWindow, Part, Strength, StrengthAndRed, luminance, vanishing};
use crate::flash_limiter::{LimiterFrameInput, LimiterFrameUniform, pack_limiter_frame};
use crate::ghost_instance::GhostInstanceLook;

/// A tint at or below this luminance has no hue to keep: it presents as grey.
const BLACK_LUMINANCE: f32 = 1.0e-4;

/// The tint presented at luminance `held`, keeping its hue: scaled to `held`,
/// or, where even its brightest scale (a component at 1) is darker than
/// `held`, that scale mixed toward white, which keeps the hue and gives up
/// saturation. A black tint has no hue and presents as a grey of `held`.
/// Either way the drawn luminance is `held`, the value the clamp records as
/// presented.
fn tint_at_luminance(tint: [f32; 3], held: f32) -> [f32; 3] {
    let current = luminance(tint);
    // Exact: even a residue of a few ulps is a level the clamp may be holding
    // an excursion against.
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
    // Re-aim at `held` once against the f32 rounding of the luma sum: the
    // clamp holds an excursion exactly at a flash threshold (a 0.1 rise, a
    // 0.8 dark state), and an ulp past either would present a counted flash
    // the clamp never admitted.
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

/// One ghost's look through the limiter. Its opacity, its tint's redness
/// (weighted by opacity) and its tint's luminance (Rec. 709) are three parts
/// of one flash window, so an onset of any part is admitted only while the
/// window holds it, its return, and the returns every part's earlier onsets
/// still owe. Each part keeps its own rate cap.
#[derive(Clone, Copy, Debug, Default)]
struct LookValueClamp {
    window: FlashWindow,
    /// Strength: the presented opacity. Colour: the tint, judged for redness.
    opacity: StrengthAndRed,
    /// The tint's presented luminance.
    luminance: Strength,
}

impl KeyClamp for LookValueClamp {
    type Value = GhostInstanceLook;

    fn fresh(look: GhostInstanceLook) -> Self {
        let mut clamp = Self::default();
        clamp.opacity.restart(look.opacity, look.tint);
        clamp.luminance.restart(luminance(look.tint));
        clamp
    }

    /// Opacity is limited first and, over budget, desaturates a red tint at
    /// unchanged luminance. The tint's luminance is limited next on the same
    /// window; over budget the tint holds its last presented luminance and
    /// keeps its hue, mixed toward white where scaling alone cannot reach
    /// that luminance. Saturation passes through: it never changes luminance.
    fn limit(&mut self, look: GhostInstanceLook, frame: &LimiterFrameUniform) -> GhostInstanceLook {
        self.window.age(frame.dt_window);
        let (opacity, tint) = self
            .opacity
            .limit(&mut self.window, look.opacity, look.tint, frame);
        let held = self.luminance.limit(
            &mut self.window,
            Part::Luminance,
            luminance(look.tint),
            frame,
        );
        GhostInstanceLook {
            opacity,
            saturation: look.saturation,
            tint: tint_at_luminance(tint, held),
        }
    }

    /// A vanished ghost's opacity falls to nothing and that fall counts. Its
    /// tint did not change, so its luminance holds, kept exactly as long as
    /// the rest of its history.
    fn absent(&mut self, frame: &LimiterFrameUniform) {
        self.window.age(frame.dt_window);
        self.opacity
            .limit(&mut self.window, 0.0, [0.0; 3], &vanishing(frame));
    }

    fn is_at_rest(&self) -> bool {
        self.window.is_empty() && self.opacity.presents_nothing()
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
/// Each ghost's opacity, tint redness and tint luminance share one flash
/// window, so one ghost presents at most 3 flashes in any 1 s across all
/// three. A ghost not presented on a frame vanished: its opacity falls to
/// nothing and that fall counts, but its tint did not change, so the tint's
/// luminance holds until the ghost's history ends.
#[derive(Clone, Debug)]
pub struct GhostLookClamp<K> {
    /// Every ghost's look clamp.
    looks: KeyedClamp<K, LookValueClamp>,
    /// `looks` as the unsettled frame began, restored if it never presents.
    /// Overwritten in place each frame, so it allocates nothing once warm.
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
    /// unchanged luminance; the tint's luminance is limited on the same
    /// window, and over budget the tint holds its last presented luminance
    /// and keeps its hue, mixed toward white where scaling alone cannot reach
    /// that luminance. Saturation passes through: it never changes luminance.
    pub fn present(&mut self, key: K, look: GhostInstanceLook) -> GhostInstanceLook {
        if !self.limiting {
            return look;
        }
        self.looks.limit(key, look)
    }

    /// End the frame: a ghost not presented this frame vanished. Its opacity
    /// falls to nothing; its tint luminance holds, kept exactly as long as the
    /// rest of its history.
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
