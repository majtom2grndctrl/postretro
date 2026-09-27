// Registry sounds resolved into their playable kira form, and the one routine
// that starts them on a plain mixer track.
// See: context/lib/audio.md §4

use kira::track::TrackHandle;

use crate::assets::{LoadedSound, SoundRegistry};
use crate::voices::KiraVoice;

/// A sound resolved out of the registry into its playable kira form, ready to
/// hand to a track. Static clones the decoded buffer (cheap Arc bump); Streaming
/// is a freshly re-opened decoder. Crate-private so kira sound-data types never
/// cross the public surface.
pub(crate) enum Playable {
    Static(kira::sound::static_sound::StaticSoundData),
    Streaming(kira::sound::streaming::StreamingSoundData<kira::sound::FromFileError>),
}

impl Playable {
    /// Resolve `key` out of the registry, warning and returning `None` when it
    /// is unknown or a streaming asset cannot be reopened. Callers resolve only
    /// after admission, so a refused request never reopens a streaming file,
    /// and release the reserved slot when this returns `None`.
    pub(crate) fn resolve(registry: &SoundRegistry, key: &str) -> Option<Self> {
        match registry.get(key) {
            Some(LoadedSound::Static(data)) => Some(Self::Static(data.as_ref().clone())),
            Some(entry @ LoadedSound::Streaming { .. }) => {
                // `open_streaming` does blocking disk I/O on this thread.
                // Acceptable while music is the only streaming sound. If
                // streaming sounds ever play on SFX/UI buses at gameplay
                // frequency, move decoding off the game thread.
                // `open_streaming` already warned on failure.
                entry.open_streaming().map(Self::Streaming)
            }
            None => {
                log::warn!("[Audio] unknown sound '{key}' — request dropped");
                None
            }
        }
    }

    /// Apply a whole-clip loop region so the sound repeats until stopped.
    pub(crate) fn looped(self) -> Self {
        match self {
            Self::Static(data) => Self::Static(data.loop_region(0.0..)),
            Self::Streaming(data) => Self::Streaming(data.loop_region(0.0..)),
        }
    }

    /// Start this sound on a plain mixer track. The error is normalized to a
    /// string because the two sound-data kinds carry different kira error types.
    pub(crate) fn start_on(self, track: &mut TrackHandle) -> Result<KiraVoice, String> {
        match self {
            Self::Static(data) => track
                .play(data)
                .map(KiraVoice::Static)
                .map_err(|err| err.to_string()),
            Self::Streaming(data) => track
                .play(data)
                .map(KiraVoice::Streaming)
                .map_err(|err| err.to_string()),
        }
    }
}
