// Ghost look clamp: every ghost's look through the photosensitivity limiter,
// one per-ghost judge per key, and each frame settled once it presents or not.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use std::hash::Hash;

use super::ghost_look::{AskedLook, LookValueClamp};
use super::keyed::KeyedClamp;
use crate::flash_limiter::{LimiterFrameInput, LimiterFrameUniform, pack_limiter_frame};
use crate::ghost_instance::GhostInstanceLook;
use crate::ghost_transparency::GhostTransparency;

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
/// untouched. [`Self::set_transparency`] says how the ghosts draw, so a
/// dithered ghost is judged on the coverage its screen door presents.
///
/// What each ghost draws over any background, and its tint's redness, share
/// one flash window, so one ghost presents at most 3 flashes in any 1 s over
/// any background, and over budget it rests at a held look. A ghost not
/// presented on a frame vanished: its opacity falls to nothing, which always
/// presents and counts once. A ghost appearing presents as asked and counts
/// its appearance the same way, however faint, since it may be one whose
/// history was dropped at rest; one already presenting when the limiter
/// turns on starts against its look and counts nothing.
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
    /// How these ghosts draw translucent.
    transparency: GhostTransparency,
}

impl<K> Default for GhostLookClamp<K> {
    fn default() -> Self {
        Self {
            looks: KeyedClamp::default(),
            frame_start: KeyedClamp::default(),
            unsettled: None,
            unconsumed: None,
            limiting: false,
            transparency: GhostTransparency::default(),
        }
    }
}

impl<K: Copy + Eq + Hash> GhostLookClamp<K> {
    /// How the ghosts this clamp limits draw translucent (`render.ghosts`;
    /// billboard ghosts always blend). Blend until set.
    pub fn set_transparency(&mut self, transparency: GhostTransparency) {
        self.transparency = transparency;
    }

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
    /// (`opacity` the ghost's whole drawn opacity: for a sprite ghost, the
    /// sprite's own opacity included): judged over every background as
    /// [`GhostLookClamp`] describes, its tint at the presented luminance
    /// keeping its hue, and over budget a red tint desaturated at unchanged
    /// luminance. Saturation passes through.
    pub fn present(&mut self, key: K, look: GhostInstanceLook) -> GhostInstanceLook {
        if !self.limiting {
            return look;
        }
        let asked = AskedLook {
            look,
            transparency: self.transparency,
        };
        self.looks.limit(key, asked).look
    }

    /// Ghost `key`'s look clamp, if the container holds one.
    #[cfg(test)]
    pub(super) fn look_clamp(&self, key: &K) -> Option<&LookValueClamp> {
        self.looks.clamp(key)
    }

    /// End the frame: a ghost not presented this frame vanished. Its opacity
    /// falls to nothing, counted once, and its history is kept until its clamp
    /// is at rest.
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
#[path = "ghost_look_clamp_tests.rs"]
mod tests;
