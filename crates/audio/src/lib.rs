// Audio subsystem: owns kira's AudioManager, the mixer tree, and the
// primitive-typed boundary the frame loop talks to. No wgpu/glam types cross
// this crate's public surface.
// See: context/lib/audio.md

mod assets;
mod boundary;
mod buses;
mod orientation;
mod voices;

pub use boundary::{AudioError, ListenerState, SoundRequest};
pub use buses::BusId;
pub use voices::SoundHandle;

use std::path::Path;

use buses::BusTree;
use kira::listener::ListenerHandle;
use kira::{AudioManager, AudioManagerSettings, Capacities, DefaultBackend, Tween};

use assets::LoadedSound;
use assets::SoundRegistry;
use boundary::parse_bus;
use orientation::orientation_from_forward_up;
use voices::VoiceTable;

/// A sound resolved out of the registry into its playable kira form, ready to
/// hand to a track. Static clones the decoded buffer (cheap Arc bump); Streaming
/// is a freshly re-opened decoder. Kept module-private so kira sound-data types
/// never cross the public surface.
enum Playable {
    Static(kira::sound::static_sound::StaticSoundData),
    Streaming(kira::sound::streaming::StreamingSoundData<kira::sound::FromFileError>),
}

/// A freshly started sound's raw kira playback handle, carried out of the
/// track-borrow scope in `play` so the handle can be registered in the voice
/// table after the bus borrow ends. Module-private; never crosses the boundary.
enum Started {
    Static(kira::sound::static_sound::StaticSoundHandle),
    Streaming(kira::sound::streaming::StreamingSoundHandle<kira::sound::FromFileError>),
}

/// Owns the kira audio manager and the spatial listener anchor.
///
/// Constructed once after the renderer is ready. If construction fails the
/// caller keeps its `Option<Audio>` as `None` and the game runs silent.
pub struct Audio<B: kira::backend::Backend = DefaultBackend> {
    // Drives the kira audio thread; bus, listener, and play operations route
    // through it. Accessed directly by tests (via `backend_mut()`).
    manager: AudioManager<B>,
    /// Single listener created at init as the anchor for spatial work.
    /// Dropping it removes the listener from kira, so it lives as long as the
    /// manager does.
    listener: ListenerHandle,
    /// Per-level sound assets, keyed by content-relative name. Populated at
    /// level install and cleared at unload so it follows level lifetime, like
    /// textures (`resource_management.md` §7.2). Consumed by `play`.
    registry: SoundRegistry,
    /// Master → SFX/Music/UI mixer tree plus the per-bus voice budget. The play
    /// API routes sounds to a bus and consults its voice counter.
    buses: BusTree,
    /// Live playback handles for sounds started via `play`, keyed by the opaque
    /// `SoundHandle`. `stop` looks handles up here; the per-frame sweep reclaims
    /// finished non-looping voices from it.
    voices: VoiceTable,
}

impl Audio {
    /// Conservative initial mixer capacity. Sub-tracks back the SFX/Music/UI
    /// bus tree; a handful of listeners covers the single anchor plus headroom.
    /// Clock/modulator capacities stay at kira defaults — unused so far. These
    /// bound kira's preallocation, not a hard runtime ceiling we expect to hit.
    const CAPACITIES: Capacities = Capacities {
        sub_track_capacity: 16,
        send_track_capacity: 16,
        clock_capacity: 8,
        modulator_capacity: 16,
        listener_capacity: 4,
    };

    /// Identity orientation `[x, y, z, w]`. The listener is re-oriented each
    /// frame in `update`; init just needs a valid quaternion.
    const IDENTITY_ORIENTATION: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    /// Build the manager on the default (cpal) backend and create the listener
    /// anchor at the world origin. Returns `AudioError::Init` if the backend or
    /// listener allocation fails; the caller degrades to silent.
    pub fn new() -> Result<Self, AudioError> {
        let settings = AudioManagerSettings::<DefaultBackend> {
            capacities: Self::CAPACITIES,
            ..Default::default()
        };

        let mut manager = AudioManager::<DefaultBackend>::new(settings)
            .map_err(|err| AudioError::Init(err.to_string()))?;

        // mint::Vector3/Quaternion accept these primitive arrays via `From`,
        // so no mint types appear here.
        let listener = manager
            .add_listener([0.0_f32, 0.0, 0.0], Self::IDENTITY_ORIENTATION)
            .map_err(|err| AudioError::Init(err.to_string()))?;

        // Build the bus tree right after the manager/listener. A sub-track
        // allocation failure folds into the fault-tolerant init path: the
        // caller logs and runs silent.
        let buses =
            BusTree::build(&mut manager).map_err(|err| AudioError::Init(err.to_string()))?;

        Ok(Self {
            manager,
            listener,
            registry: SoundRegistry::new(),
            buses,
            voices: VoiceTable::new(),
        })
    }
}

/// Backend-agnostic surface. kira's playback/track/listener handles aren't
/// parameterized by backend, so every method below works whether the manager
/// runs on the real `DefaultBackend` (production) or `MockBackend` (tests). The
/// generic impl is what lets unit tests drive `play`/`stop`/`update` without a
/// sound device.
impl<B: kira::backend::Backend> Audio<B> {
    /// Set the runtime volume of a mixer bus, in decibels (0 dB = unity gain,
    /// negative attenuates, positive boosts). The public volume control for
    /// SFX/Music/UI; delegates to the bus tree.
    pub fn set_bus_volume(&mut self, bus: BusId, decibels: f32) {
        self.buses.set_volume(bus, decibels);
    }

    /// Set the overall output volume, in decibels (0 dB = unity gain, negative
    /// attenuates, positive boosts). This drives kira's main track — the parent
    /// of the SFX/Music/UI sub-tracks — so it scales every bus at once. Same
    /// decibel/`Tween` convention as [`set_bus_volume`](Self::set_bus_volume):
    /// the level change fades over kira's default ~10 ms tween rather than
    /// cutting.
    pub fn set_main_volume(&mut self, db: f32) {
        self.manager.main_track().set_volume(db, Tween::default());
    }

    /// Reserve a voice slot on `bus`, returning `true` on success (count
    /// incremented) or `false` when the bus is at its cap. `play` calls this
    /// before starting a sound and drops-and-logs on `false`.
    #[allow(dead_code)]
    pub(crate) fn try_acquire_voice(&mut self, bus: BusId) -> bool {
        self.buses.try_acquire_voice(bus)
    }

    /// Release a voice slot previously reserved on `bus`. Saturating no-op if
    /// the bus has no outstanding voices.
    #[allow(dead_code)]
    pub(crate) fn release_voice(&mut self, bus: BusId) {
        self.buses.release_voice(bus);
    }

    /// Current active-voice count on `bus`.
    #[allow(dead_code)]
    pub(crate) fn active_voices(&self, bus: BusId) -> usize {
        self.buses.active_voices(bus)
    }

    /// Load every decodable sound under `<content_root>/sounds/` into the
    /// registry, replacing any sounds from a previously installed level. Wired
    /// into `install_level_payload` so the sound set follows level lifetime.
    /// Missing directory or undecodable files degrade gracefully (warn, skip);
    /// never panics. Delegates to the asset module.
    pub fn load_level_sounds(&mut self, content_root: &Path) {
        self.registry.load_from_content_root(content_root);
    }

    /// Drop every registered sound. Wired into the level-unload / shutdown path
    /// so registry memory is released with the level. After this the registry is
    /// empty and a subsequent `load_level_sounds` repopulates it.
    pub fn release_level_sounds(&mut self) {
        self.registry.clear();
    }

    /// The per-level sound registry. Read-only so callers resolve
    /// `SoundRequest::sound` keys to loaded entries.
    #[allow(dead_code)]
    pub(crate) fn registry(&self) -> &SoundRegistry {
        &self.registry
    }

    /// Start playing the requested sound on its target bus, returning an opaque
    /// [`SoundHandle`] the caller can later pass to [`stop`](Self::stop). Returns
    /// `None` — never panicking — when the request can't be honored:
    ///
    /// - the bus name is unrecognized (warns),
    /// - the sound key isn't in the registry (warns),
    /// - the bus is at its voice cap (already dropped-and-logged by
    ///   [`try_acquire_voice`](Self::try_acquire_voice)),
    /// - or kira refuses the play / a streaming asset became unreadable (warns,
    ///   and the just-acquired voice is released so the bus doesn't leak).
    ///
    /// A `looping` request applies a whole-clip loop region so the sound repeats
    /// until `stop`; it therefore holds its voice indefinitely (the finished-voice
    /// sweep never reclaims a looping sound, which never reaches `Stopped`).
    pub fn play(&mut self, req: SoundRequest) -> Option<SoundHandle> {
        let bus = match parse_bus(&req.bus) {
            Some(bus) => bus,
            None => {
                log::warn!(
                    "[Audio] unknown bus '{}' for sound '{}' — request dropped",
                    req.bus,
                    req.sound,
                );
                return None;
            }
        };

        // Resolve the asset before touching the voice budget so a missing sound
        // never consumes a slot. Clone the entry's playable form out of the
        // registry borrow so the subsequent `&mut self` track/voice work is clear
        // of the immutable registry borrow.
        let playable = match self.registry.get(&req.sound) {
            Some(LoadedSound::Static(data)) => Playable::Static(data.as_ref().clone()),
            Some(entry @ LoadedSound::Streaming { .. }) => {
                // `open_streaming` does blocking disk I/O on this thread.
                // Acceptable while music is the only streaming sound. If
                // streaming sounds ever play on SFX/UI buses at gameplay
                // frequency, move decoding off the game thread.
                // `open_streaming` already warned on failure; nothing acquired yet.
                Playable::Streaming(entry.open_streaming()?)
            }
            None => {
                log::warn!("[Audio] unknown sound '{}' — request dropped", req.sound);
                return None;
            }
        };

        // Reserve the voice last. On any failure past this point the slot is
        // released so the bus counter stays honest.
        if !self.buses.try_acquire_voice(bus) {
            // `try_acquire_voice` dropped-and-logged.
            return None;
        }

        // Start the sound on the bus's track. Scope the `&mut` track borrow so it
        // ends before the voice-budget bookkeeping below (both borrow
        // `self.buses`). `play` is the only kira call here; on `Err` the voice
        // slot is released so the bus counter stays honest.
        let started = {
            let track = self.buses.track_mut(bus);
            match playable {
                Playable::Static(data) => {
                    let data = if req.looping {
                        // Whole-clip loop: repeat from the start until stopped.
                        data.loop_region(0.0..)
                    } else {
                        data
                    };
                    // Normalize the error to a string here: the two sound-data
                    // kinds carry different kira error types (`()` vs
                    // `FromFileError`), so the arms can't share a `Result` type.
                    track
                        .play(data)
                        .map(Started::Static)
                        .map_err(|err| err.to_string())
                }
                Playable::Streaming(data) => {
                    let data = if req.looping {
                        data.loop_region(0.0..)
                    } else {
                        data
                    };
                    track
                        .play(data)
                        .map(Started::Streaming)
                        .map_err(|err| err.to_string())
                }
            }
        };

        match started {
            Ok(Started::Static(h)) => Some(self.voices.insert_static(h, bus)),
            Ok(Started::Streaming(h)) => Some(self.voices.insert_streaming(h, bus)),
            Err(err) => {
                log::warn!("[Audio] kira rejected sound '{}': {err}", req.sound);
                self.buses.release_voice(bus);
                None
            }
        }
    }

    /// Stop a sound started via [`play`](Self::play) and release its voice slot.
    /// The voice is removed from the active table immediately; kira applies its
    /// default ~10 ms tween to fade the audio out. A no-op if `handle` is
    /// unknown — already finished and reclaimed, or never minted by this `Audio`.
    pub fn stop(&mut self, handle: SoundHandle) {
        if let Some(bus) = self.voices.remove_and_stop(handle, Tween::default()) {
            self.buses.release_voice(bus);
        }
    }

    /// Per-frame audio step. Runs third in frame order (Input → Game logic →
    /// **Audio** → Render → Present). Control-plane only: re-anchors the kira
    /// listener to the camera pose and sweeps finished voices. Never decodes or
    /// touches disk, so it never blocks the frame.
    ///
    /// Voice reclamation: kira advances non-looping sounds to `Stopped` on its
    /// own audio thread. The sweep observes that and releases one bus voice slot
    /// per finished sound, dropping its handle — without this, buses would leak
    /// capacity as one-shot sounds finished. Looping sounds never reach `Stopped`
    /// and so hold their voice until [`stop`](Self::stop).
    ///
    /// `dt` is the frame delta in seconds. Spatialization is out of scope for now;
    /// the listener pose is updated instantly (no tween) and `dt` is currently
    /// unused beyond satisfying the per-frame contract.
    pub fn update(&mut self, listener: ListenerState, _dt: f32) {
        // Anchor the listener to the camera. Position as a primitive array;
        // orientation as a kira-convention quaternion built from forward/up.
        self.listener
            .set_position(listener.position, Tween::default());
        let orientation = orientation_from_forward_up(listener.forward, listener.up);
        self.listener.set_orientation(orientation, Tween::default());

        // Reclaim finished non-looping voices so buses don't leak capacity.
        for bus in self.voices.reclaim_finished() {
            self.buses.release_voice(bus);
        }
    }

    /// Drop the manager, stopping the audio thread and releasing the device.
    /// Production teardown happens via `Drop` when `App` exits — `shutdown` is
    /// for deterministic cleanup (e.g. tests).
    pub fn shutdown(self) {
        // Dropping `manager` (and `listener`) tears down the kira backend.
    }
}

#[cfg(test)]
mod tests;
