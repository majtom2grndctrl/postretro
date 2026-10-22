// Per-ghost look clamp: each ghost's opacity and tint through two keyed per-value
// channels, one judging opacity (with the tint's redness), one the tint's luminance.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use std::hash::Hash;

use super::keyed::KeyedValueClamp;
use super::value::luminance;
use crate::flash_limiter::LimiterFrameUniform;
use crate::ghost_instance::GhostInstanceLook;

/// Which of a ghost's two limiter channels a clamp key names. Part of every
/// key, so a ghost's two channels never share history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GhostClampChannel {
    /// Strength: the presented opacity. Colour: the tint, judged for redness.
    Opacity,
    /// Strength: the tint's luminance (Rec. 709). Colour: neutral, so this
    /// channel never counts red.
    TintLuminance,
}

/// A tint at or below this luminance has no hue to keep: it presents as grey.
const BLACK_LUMINANCE: f32 = 1.0e-4;

/// The tint presented at luminance `held`, keeping its hue: scaled to `held`
/// (each component clamped to 1), or a grey of `held` when the tint is black.
fn tint_at_luminance(tint: [f32; 3], held: f32) -> [f32; 3] {
    let current = luminance(tint);
    // Exact: even a residue of a few ulps is a level the clamp may be holding
    // an excursion against.
    if current == held {
        return tint;
    }
    let mut presented = if current <= BLACK_LUMINANCE {
        [held; 3]
    } else {
        let scale = held / current;
        tint.map(|channel| (channel * scale).min(1.0))
    };
    // Re-aim at `held` once against the f32 rounding of the luma sum: the
    // clamp holds an excursion exactly at a flash threshold (a 0.1 rise, a
    // 0.8 dark state), and an ulp past either would present a counted flash
    // the clamp never admitted.
    let measured = luminance(presented);
    if measured > BLACK_LUMINANCE && measured != held {
        presented = presented.map(|channel| (channel * (held / measured)).min(1.0));
    }
    presented
}

/// Every ghost's look through the photosensitivity limiter, the one place a
/// ghost draw collector limits a look. Keys are a collector's ghost identity
/// (its link and generation-checked entity), so an entity seen through two
/// links is two ghosts with their own history.
///
/// Each presented frame: [`Self::begin_frame`], then [`Self::present`] once
/// per presented ghost (culled or not: the limiter judges the source, not its
/// framing), then [`Self::end_frame`]. A frame begun with no limiter input
/// (a capture) passes every look through and leaves history untouched.
#[derive(Clone, Debug)]
pub struct GhostLookClamp<K> {
    channels: KeyedValueClamp<(K, GhostClampChannel)>,
    /// This frame runs the limiter: it was begun with a limiter input.
    limiting: bool,
}

impl<K> Default for GhostLookClamp<K> {
    fn default() -> Self {
        Self {
            channels: KeyedValueClamp::default(),
            limiting: false,
        }
    }
}

impl<K: Copy + Eq + Hash> GhostLookClamp<K> {
    /// Start a presented frame. `frame` is this frame's limiter input (the
    /// ghost ramp's presented-frame time and the fail-safe enable flag), or
    /// `None` for a capture, which never involves the limiter.
    pub fn begin_frame(&mut self, frame: Option<&LimiterFrameUniform>) {
        self.limiting = frame.is_some();
        if let Some(frame) = frame {
            self.channels.begin_frame(frame);
        }
    }

    /// The look ghost `key` presents this frame, from its unclamped look
    /// (`opacity` already the link's opacity times membership).
    ///
    /// The opacity channel limits opacity and, over budget, desaturates a red
    /// tint at unchanged luminance. The luminance channel limits the tint's
    /// luminance; over budget the tint holds its last presented luminance and
    /// keeps its hue. Saturation passes through: it never changes luminance.
    pub fn present(&mut self, key: K, look: GhostInstanceLook) -> GhostInstanceLook {
        if !self.limiting {
            return look;
        }
        let (opacity, tint) =
            self.channels
                .limit((key, GhostClampChannel::Opacity), look.opacity, look.tint);
        let (held, _) = self.channels.limit(
            (key, GhostClampChannel::TintLuminance),
            luminance(look.tint),
            [1.0; 3],
        );
        GhostInstanceLook {
            opacity,
            saturation: look.saturation,
            tint: tint_at_luminance(tint, held),
        }
    }

    /// End the frame: a ghost not presented this frame fell to nothing, on
    /// both channels.
    pub fn end_frame(&mut self) {
        if self.limiting {
            self.channels.end_frame();
        }
    }
}

#[cfg(test)]
#[path = "ghost_look_tests.rs"]
mod tests;
