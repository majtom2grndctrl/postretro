// Keyed flash clamps: one limiter clamp per presented source (a ghost, today),
// each judged as a full-screen change on the screen channels' rules.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use std::collections::HashMap;
use std::hash::Hash;

use crate::flash_limiter::{LimiterFrameInput, LimiterFrameUniform, pack_limiter_frame};

/// One key's clamp, as a [`KeyedClamp`] holds it: everything the key presents,
/// judged on the clamp's own flash window.
pub(super) trait KeyClamp: Clone {
    /// What a key asks to present on a frame, and what it presents.
    type Value: Copy;

    /// A clamp starting fresh against `value`, for a key already presenting
    /// it as history starts: nothing to rate-cap from and room in its window,
    /// so its first limit of `value` presents it unchanged.
    fn fresh(value: Self::Value) -> Self;

    /// A clamp for a key appearing with `value` on a frame after history
    /// started. Its first limit of `value` presents it unchanged too; a clamp
    /// may count the appearance against its budget. The same as
    /// [`Self::fresh`] unless the clamp says otherwise.
    fn appearing(value: Self::Value) -> Self {
        Self::fresh(value)
    }

    /// Limit `value` for one presented frame. Returns what presents.
    fn limit(&mut self, value: Self::Value, frame: &LimiterFrameUniform) -> Self::Value;

    /// Record a frame on which the key presented nothing.
    fn absent(&mut self, frame: &LimiterFrameUniform);

    /// Whether the clamp holds nothing a fresh one at zero would not.
    fn is_at_rest(&self) -> bool;
}

#[derive(Clone, Debug)]
struct Entry<C> {
    clamp: C,
    /// Whether the key was limited this frame.
    limited: bool,
}

/// One [`KeyClamp`] per key, for sources the renderer packs CPU-side. Keys
/// should be generation-checked ids, so a re-tenanted slot is a different key
/// and starts its own history. Renderer-side presentation state only: nothing
/// here is readable by scripts or crosses the wire.
///
/// Each presented frame: [`Self::begin_frame`], then [`Self::limit`] once per
/// presented key, then [`Self::end_frame`].
///
/// - Off, values pass through and no history is kept.
/// - The frame that turns the limiter on, and a new container's first frame,
///   start every key fresh against its own value ([`KeyClamp::fresh`]), so
///   enabling never dips one and counts nothing.
/// - A key first limited on a later frame appeared ([`KeyClamp::appearing`]);
///   its first frame packs unchanged.
/// - A key not limited on a frame presented nothing ([`KeyClamp::absent`]).
///   Its history is kept until its clamp is at rest, so a source strobing by
///   appearing and vanishing is limited like any other strobe; then it is
///   dropped.
#[derive(Debug)]
pub(super) struct KeyedClamp<K, C> {
    entries: HashMap<K, Entry<C>>,
    frame: LimiterFrameUniform,
    /// Whether the previous frame ran with the limiter on.
    history_live: bool,
    /// Whether this frame started history fresh, so every key limited on it
    /// was already presenting.
    history_starts: bool,
}

/// Keys a container holds room for from construction. A table that never
/// allocated is the shared empty one, and cloning from it into a snapshot frees
/// the snapshot's table, so every container starts with one.
const INITIAL_KEYS: usize = 16;

impl<K, C> Default for KeyedClamp<K, C> {
    fn default() -> Self {
        Self {
            entries: HashMap::with_capacity(INITIAL_KEYS),
            // Fail safe: limited at the default frame time until the first
            // `begin_frame` says otherwise.
            frame: pack_limiter_frame(LimiterFrameInput::default(), true, true),
            history_live: false,
            history_starts: false,
        }
    }
}

impl<K: Clone, C: Clone> Clone for KeyedClamp<K, C> {
    fn clone(&self) -> Self {
        Self {
            entries: self.entries.clone(),
            frame: self.frame,
            history_live: self.history_live,
            history_starts: self.history_starts,
        }
    }

    /// Reuses this container's table while it is the source's size, so a
    /// per-frame snapshot allocates only when the source has grown past it.
    fn clone_from(&mut self, source: &Self) {
        self.entries.clone_from(&source.entries);
        self.frame = source.frame;
        self.history_live = source.history_live;
        self.history_starts = source.history_starts;
    }
}

impl<K: Copy + Eq + Hash, C: KeyClamp> KeyedClamp<K, C> {
    /// Start a presented frame. `frame` carries presented-frame time and the
    /// fail-safe enable flag (`pack_limiter_frame`). History starts fresh when
    /// the limiter turns on, on this container's first frame, or when `frame`
    /// says history starts (`init`).
    pub(super) fn begin_frame(&mut self, frame: &LimiterFrameUniform) {
        let fresh = frame.enabled && (frame.init || !self.history_live);
        self.history_live = frame.enabled;
        self.history_starts = fresh;
        self.frame = *frame;
        if !frame.enabled || fresh {
            // Off keeps nothing; a fresh start counts nothing from before it.
            // Every key limited this frame then starts against its own value.
            self.entries.clear();
        }
    }

    /// Limit `key`'s `value` for this frame. Returns what presents.
    pub(super) fn limit(&mut self, key: K, value: C::Value) -> C::Value {
        if !self.frame.enabled {
            return value;
        }
        let history_starts = self.history_starts;
        let entry = self.entries.entry(key).or_insert_with(|| Entry {
            clamp: if history_starts {
                C::fresh(value)
            } else {
                C::appearing(value)
            },
            limited: false,
        });
        debug_assert!(
            !entry.limited,
            "a key is limited once per frame, between begin_frame and end_frame"
        );
        entry.limited = true;
        entry.clamp.limit(value, &self.frame)
    }

    /// Keys the container holds room for without allocating.
    #[cfg(test)]
    pub(super) fn capacity(&self) -> usize {
        self.entries.capacity()
    }

    /// `key`'s clamp, if the container holds one.
    #[cfg(test)]
    pub(super) fn clamp(&self, key: &K) -> Option<&C> {
        self.entries.get(key).map(|entry| &entry.clamp)
    }

    /// End the frame: every key not limited presented nothing this frame. A
    /// key whose clamp is at rest is dropped, its history over.
    pub(super) fn end_frame(&mut self) {
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
