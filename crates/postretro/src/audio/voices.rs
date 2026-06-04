// Active-voice tracking for played sounds: the opaque `SoundHandle` id, the
// engine-owned table mapping ids to live kira playback handles, and the
// finished-voice sweep that reclaims bus capacity when a sound stops.
// See: context/lib/audio.md §1 (per-bus active-voice cap)

use std::collections::HashMap;

use kira::Tween;
use kira::sound::PlaybackState;
use kira::sound::static_sound::StaticSoundHandle;
use kira::sound::streaming::StreamingSoundHandle;

use super::BusId;

/// Opaque, primitive-backed id for a playing sound. Returned by `Audio::play`
/// and consumed by `Audio::stop`. Deliberately wraps a plain `u64` so no kira
/// type leaks across the audio public surface (the boundary-is-a-contract
/// invariant in `index.md` §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SoundHandle(u64);

impl SoundHandle {
    /// Mint a handle from a raw id, for tests that need to probe `stop` with an
    /// id this `Audio` never issued. Not part of the runtime surface — ids are
    /// only ever minted internally by [`VoiceTable::insert`].
    #[cfg(test)]
    pub(crate) fn from_raw_for_test(raw: u64) -> Self {
        Self(raw)
    }
}

/// A live kira playback handle, one variant per kira sound-data kind. Held
/// internally only — never exposed across the module boundary. Both variants
/// share the `state()` / `stop()` control surface the sweep and `stop` use.
enum KiraVoice {
    Static(StaticSoundHandle),
    /// `StreamingSoundData::from_file` is parameterized over kira's
    /// `FromFileError`, so its handle carries that same error type.
    Streaming(StreamingSoundHandle<kira::sound::FromFileError>),
}

impl KiraVoice {
    /// Current kira playback state. `Stopped` means the sound finished (a
    /// non-looping clip ran to its end) and its voice can be reclaimed.
    fn state(&self) -> PlaybackState {
        match self {
            KiraVoice::Static(h) => h.state(),
            KiraVoice::Streaming(h) => h.state(),
        }
    }

    /// Stop playback with the given tween (immediate when `Tween::default()`
    /// fade-out is zero-length — see `Audio::stop`).
    fn stop(&mut self, tween: Tween) {
        match self {
            KiraVoice::Static(h) => h.stop(tween),
            KiraVoice::Streaming(h) => h.stop(tween),
        }
    }
}

/// One playing sound: its kira handle plus the bus its voice was reserved on,
/// so reclamation releases the right counter.
struct ActiveVoice {
    voice: KiraVoice,
    bus: BusId,
}

/// Engine-owned table of currently playing sounds, keyed by `SoundHandle`. The
/// play path inserts; `stop` and the finished-voice sweep remove. Ids are
/// monotonically minted and never reused within a session, so a stale
/// `SoundHandle` (whose sound already stopped) resolves to nothing rather than
/// to an unrelated voice.
#[derive(Default)]
pub(crate) struct VoiceTable {
    voices: HashMap<SoundHandle, ActiveVoice>,
    next_id: u64,
}

impl VoiceTable {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Register a freshly started static voice, returning its opaque id.
    pub(crate) fn insert_static(&mut self, handle: StaticSoundHandle, bus: BusId) -> SoundHandle {
        self.insert(KiraVoice::Static(handle), bus)
    }

    /// Register a freshly started streaming voice, returning its opaque id.
    pub(crate) fn insert_streaming(
        &mut self,
        handle: StreamingSoundHandle<kira::sound::FromFileError>,
        bus: BusId,
    ) -> SoundHandle {
        self.insert(KiraVoice::Streaming(handle), bus)
    }

    fn insert(&mut self, voice: KiraVoice, bus: BusId) -> SoundHandle {
        let id = SoundHandle(self.next_id);
        self.next_id += 1;
        self.voices.insert(id, ActiveVoice { voice, bus });
        id
    }

    /// Stop and remove the voice for `id`, returning the bus whose counter must
    /// be released. `None` if the id is unknown (already finished or never
    /// existed) — `stop` is a no-op in that case.
    pub(crate) fn remove_and_stop(&mut self, id: SoundHandle, tween: Tween) -> Option<BusId> {
        let mut active = self.voices.remove(&id)?;
        active.voice.stop(tween);
        Some(active.bus)
    }

    /// Drop every voice whose kira playback has reached `Stopped`, returning the
    /// bus of each so the caller can release one voice slot per reclaimed sound.
    /// This is how non-looping sounds return their bus capacity: kira advances
    /// them to `Stopped` on its audio thread, and the per-frame sweep observes
    /// that and decrements the bus counter. Looping sounds never reach
    /// `Stopped` on their own, so they hold their voice until `stop`.
    pub(crate) fn reclaim_finished(&mut self) -> Vec<BusId> {
        let finished: Vec<SoundHandle> = self
            .voices
            .iter()
            .filter(|(_, active)| active.voice.state() == PlaybackState::Stopped)
            .map(|(id, _)| *id)
            .collect();

        finished
            .into_iter()
            .filter_map(|id| self.voices.remove(&id).map(|active| active.bus))
            .collect()
    }
}
