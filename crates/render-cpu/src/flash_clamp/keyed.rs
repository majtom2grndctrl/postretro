// Keyed per-value flash clamps: one limiter channel per presented source (a
// ghost, today), each judged as a full-screen change on the screen channels' rules.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use std::collections::HashMap;
use std::hash::Hash;

use super::value::ValueClamp;
use crate::flash_limiter::{LimiterFrameInput, LimiterFrameUniform, pack_limiter_frame};

#[derive(Clone, Copy, Debug)]
struct Entry {
    clamp: ValueClamp,
    /// Whether the key was limited this frame.
    limited: bool,
}

/// One [`ValueClamp`] per key, for sources the renderer packs CPU-side. Keys
/// should be generation-checked ids, so a re-tenanted slot is a different key
/// and starts its own history. Renderer-side presentation state only: nothing
/// here is readable by scripts or crosses the wire.
///
/// Each presented frame: [`Self::begin_frame`], then [`Self::limit`] once per
/// presented key, then [`Self::end_frame`].
///
/// - Off, values pass through and no history is kept.
/// - The frame that turns the limiter on, and a new container's first frame,
///   start every key fresh against its own value, so enabling never dips one.
/// - A key limited for the first time starts fresh against its value, so its
///   first frame packs unchanged.
/// - A key not limited on a frame presented nothing: it falls to zero and that
///   fall counts. Its history is kept until its window empties, so a source
///   strobing by appearing and vanishing is limited like any other strobe;
///   then it is dropped.
#[derive(Clone, Debug)]
pub struct KeyedValueClamp<K> {
    entries: HashMap<K, Entry>,
    frame: LimiterFrameUniform,
    /// Whether the previous frame ran with the limiter on.
    history_live: bool,
}

impl<K> Default for KeyedValueClamp<K> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            // Fail safe: limited at the default frame time until the first
            // `begin_frame` says otherwise.
            frame: pack_limiter_frame(LimiterFrameInput::default(), true, true),
            history_live: false,
        }
    }
}

impl<K: Copy + Eq + Hash> KeyedValueClamp<K> {
    /// Start a presented frame. `frame` carries presented-frame time and the
    /// fail-safe enable flag (`pack_limiter_frame`). History starts fresh when
    /// the limiter turns on, on this container's first frame, or when `frame`
    /// says history starts (`init`).
    pub fn begin_frame(&mut self, frame: &LimiterFrameUniform) {
        let fresh = frame.enabled && (frame.init || !self.history_live);
        self.history_live = frame.enabled;
        self.frame = *frame;
        if !frame.enabled || fresh {
            // Off keeps nothing; a fresh start counts nothing from before it.
            // Every key limited this frame then starts against its own value.
            self.entries.clear();
        }
    }

    /// Limit `key`'s blend `strength` and color `rgb` for this frame. Returns
    /// the presented strength and color.
    pub fn limit(&mut self, key: K, strength: f32, rgb: [f32; 3]) -> (f32, [f32; 3]) {
        if !self.frame.enabled {
            return (strength, rgb);
        }
        let entry = self.entries.entry(key).or_insert_with(|| Entry {
            clamp: ValueClamp::fresh(strength, rgb),
            limited: false,
        });
        debug_assert!(
            !entry.limited,
            "a key is limited once per frame, between begin_frame and end_frame"
        );
        entry.limited = true;
        entry.clamp.limit(strength, rgb, &self.frame)
    }

    /// End the frame: every key not limited presented nothing this frame. A
    /// key whose window has emptied at zero is dropped, its history over.
    pub fn end_frame(&mut self) {
        if !self.frame.enabled {
            return;
        }
        let frame = self.frame;
        self.entries.retain(|_, entry| {
            if std::mem::take(&mut entry.limited) {
                return true;
            }
            entry.clamp.absent(&frame);
            !entry.clamp.is_at_rest()
        });
    }
}

#[cfg(test)]
#[path = "keyed_tests.rs"]
mod tests;
