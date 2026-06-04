// Audio subsystem: owns kira's AudioManager, the mixer tree, and the
// primitive-typed boundary the frame loop talks to. No wgpu/glam types cross
// this module's public surface.
// See: context/lib/audio.md

mod assets;
mod buses;
mod voices;

pub use buses::BusId;
pub use voices::SoundHandle;

use std::path::Path;

use buses::BusTree;
use kira::listener::ListenerHandle;
use kira::{AudioManager, AudioManagerSettings, Capacities, DefaultBackend, Tween};

use assets::LoadedSound;
pub(crate) use assets::SoundRegistry;
use voices::VoiceTable;

/// Build a kira-convention orientation quaternion `[x, y, z, w]` from a forward
/// and up vector, both world-space primitives. kira's unrotated listener faces
/// `-Z` with `+X` right and `+Y` up, so this constructs the rotation taking that
/// reference basis onto the camera basis derived from `forward`/`up`. Done with
/// plain f32 math so no glam/quaternion type crosses the module boundary.
///
/// Degenerate inputs (zero-length forward, or forward parallel to up) fall back
/// to identity rather than producing NaNs; spatialization is out of scope for
/// M12 Task 4, so the listener just stays anchored and oriented best-effort.
fn orientation_from_forward_up(forward: [f32; 3], up: [f32; 3]) -> [f32; 4] {
    // Camera basis: -Z = forward (look), +X = right, +Y = up.
    let f = normalize(forward);
    // `right = forward × up`; `up' = right × forward` re-orthogonalizes.
    let right = normalize(cross(f, up));
    if length(f) < f32::EPSILON || length(right) < f32::EPSILON {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let u = cross(right, f);

    // Columns of the rotation matrix mapping reference axes to the camera basis:
    // +X -> right, +Y -> u, -Z -> f  (so +Z -> -f).
    let m00 = right[0];
    let m10 = right[1];
    let m20 = right[2];
    let m01 = u[0];
    let m11 = u[1];
    let m21 = u[2];
    let m02 = -f[0];
    let m12 = -f[1];
    let m22 = -f[2];

    // Standard matrix-to-quaternion conversion (Shepperd's method, trace case
    // plus the three diagonal-dominant cases for numerical stability).
    let trace = m00 + m11 + m22;
    let (x, y, z, w) = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        ((m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s)
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        (0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s)
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        ((m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s)
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        ((m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s)
    };
    [x, y, z, w]
}

/// Map a boundary bus name to its [`BusId`]. Case-insensitive; the accepted
/// names mirror the registry collection convention (`sfx`, `music`, `ui`).
/// Unknown names return `None` so `play` can warn-and-drop.
fn parse_bus(name: &str) -> Option<BusId> {
    match name.to_ascii_lowercase().as_str() {
        "sfx" => Some(BusId::Sfx),
        "music" => Some(BusId::Music),
        "ui" => Some(BusId::UI),
        _ => None,
    }
}

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

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = length(v);
    if len < f32::EPSILON {
        v
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

/// Failures from audio init or asset loading. Init failure is non-fatal: the
/// caller logs and runs the game silent (`Audio` stays `None`). The kira
/// backend error is captured as a string so the backend type never leaks
/// across this boundary.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// The kira backend (cpal device/stream) failed to start.
    #[error("audio backend init failed: {0}")]
    Init(String),
}

/// Listener pose handed across the subsystem boundary each frame. Primitives
/// only — the glam-typed `Camera` is converted at the call site, not here.
/// `up` is world up `[0.0, 1.0, 0.0]`; `Camera` has no up accessor and uses
/// `Vec3::Y` internally.
// Boundary type consumed by the per-frame audio step (Task 4); defined now to
// pin the subsystem contract.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ListenerState {
    /// World-space listener position.
    pub position: [f32; 3],
    /// Normalized forward (look) direction.
    pub forward: [f32; 3],
    /// Normalized world up, always `[0.0, 1.0, 0.0]`.
    pub up: [f32; 3],
}

/// A request to play a sound, crossing the boundary as primitives only.
/// The target bus and sound are named keys resolved inside the subsystem; the
/// bus tree and sound registry land in later tasks.
// Boundary type consumed by the play API (Task 3); defined now to pin the
// subsystem contract.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoundRequest {
    /// Mixer bus to route this sound to (e.g. "sfx", "music", "ui").
    pub bus: String,
    /// Registry key of the sound asset to play.
    pub sound: String,
    /// Whether the sound loops until stopped.
    pub looping: bool,
}

/// Owns the kira audio manager and the spatial listener anchor.
///
/// Constructed once after the renderer is ready. If construction fails the
/// caller keeps its `Option<Audio>` as `None` and the game runs silent. Later
/// tasks extend this with bus handles, a sound registry, and a play API.
pub struct Audio<B: kira::backend::Backend = DefaultBackend> {
    // Read by the sound registry and play API in later tasks.
    #[allow(dead_code)]
    manager: AudioManager<B>,
    /// Single listener created at init as the anchor for later spatial work.
    /// Dropping it removes the listener from kira, so it lives as long as the
    /// manager does.
    #[allow(dead_code)]
    listener: ListenerHandle,
    /// Per-level sound assets, keyed by content-relative name. Populated at
    /// level install and cleared at unload so it follows level lifetime, like
    /// textures (`resource_management.md` §7.2). Consumed by the play API
    /// (Task 4).
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
    /// bus tree (Task 2); a handful of listeners covers the single anchor plus
    /// headroom. Clock/modulator capacities stay at kira defaults — unused so
    /// far. These bound kira's preallocation, not a hard runtime ceiling we
    /// expect to hit.
    const CAPACITIES: Capacities = Capacities {
        sub_track_capacity: 16,
        send_track_capacity: 16,
        clock_capacity: 8,
        modulator_capacity: 16,
        listener_capacity: 4,
    };

    /// Identity orientation `[x, y, z, w]`. The listener is re-oriented each
    /// frame in a later task; init just needs a valid quaternion.
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
    /// negative attenuates, positive boosts). Applied instantly. The public
    /// volume control for SFX/Music/UI; delegates to the bus tree.
    #[allow(dead_code)]
    pub fn set_bus_volume(&mut self, bus: BusId, decibels: f32) {
        self.buses.set_volume(bus, decibels);
    }

    /// Reserve a voice slot on `bus`, returning `true` on success (count
    /// incremented) or `false` when the bus is at its cap. The play API (Task 4)
    /// calls this before starting a sound and drops-and-logs on `false`.
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

    /// Load every decodable sound under `<content_root>/_sounds/` into the
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

    /// The per-level sound registry, for the play API (Task 4). Read-only so
    /// callers resolve `SoundRequest::sound` keys to loaded entries.
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
    #[allow(dead_code)]
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
            Some(LoadedSound::Streaming { .. }) => {
                match self
                    .registry
                    .get(&req.sound)
                    .and_then(LoadedSound::open_streaming)
                {
                    Some(data) => Playable::Streaming(data),
                    // `open_streaming` already warned; nothing acquired yet.
                    None => return None,
                }
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
    /// Stops the kira handle immediately (zero-length default tween) and drops it
    /// from the active table. A no-op if `handle` is unknown — already finished
    /// and reclaimed, or never minted by this `Audio`.
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    /// Consumes `self`; equivalent to letting `Audio` fall out of scope.
    /// Wired into the app shutdown path by a later task.
    #[allow(dead_code)]
    pub fn shutdown(self) {
        // Dropping `manager` (and `listener`) tears down the kira backend.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kira::backend::mock::MockBackend;

    /// Absolute path to the repo's `content/dev`, so the committed sound fixtures
    /// resolve regardless of where `cargo test` runs from. Mirrors the helper in
    /// `assets.rs`.
    fn dev_content_root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/dev")
            .canonicalize()
            .expect("content/dev exists relative to the crate manifest")
    }

    /// Build an `Audio` on the always-available mock backend with the engine's
    /// real capacities and bus tree, plus the level's fixture sounds loaded.
    /// Exercises the production code paths without a sound device. Returns the
    /// `Audio` and its `MockBackend` manager pieces — kira's mock backend lives
    /// inside the manager, reached via `manager.backend_mut()`.
    fn mock_audio() -> Audio<MockBackend> {
        let settings = AudioManagerSettings::<MockBackend> {
            capacities: Audio::CAPACITIES,
            ..Default::default()
        };
        let mut manager =
            AudioManager::<MockBackend>::new(settings).expect("mock backend always starts");
        let listener = manager
            .add_listener([0.0_f32, 0.0, 0.0], Audio::IDENTITY_ORIENTATION)
            .expect("listener allocates under mock backend");
        let buses = BusTree::build(&mut manager).expect("bus tree builds under mock backend");

        let mut audio = Audio {
            manager,
            listener,
            registry: SoundRegistry::new(),
            buses,
            voices: VoiceTable::new(),
        };
        audio.load_level_sounds(&dev_content_root());
        audio
    }

    /// A one-shot SFX request for the committed static fixture.
    fn sfx_request() -> SoundRequest {
        SoundRequest {
            bus: "sfx".to_string(),
            sound: "sfx/test_tone".to_string(),
            looping: false,
        }
    }

    #[test]
    fn parse_bus_is_case_insensitive_and_rejects_unknown() {
        assert_eq!(parse_bus("sfx"), Some(BusId::Sfx));
        assert_eq!(parse_bus("Music"), Some(BusId::Music));
        assert_eq!(parse_bus("UI"), Some(BusId::UI));
        assert_eq!(parse_bus("master"), None);
        assert_eq!(parse_bus(""), None);
    }

    #[test]
    fn orientation_from_identity_forward_is_identity() {
        // Camera looking down -Z with +Y up is kira's unrotated reference: the
        // resulting quaternion should be (near) identity.
        let q = orientation_from_forward_up([0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);
        assert!((q[0]).abs() < 1e-5, "x ~ 0, got {}", q[0]);
        assert!((q[1]).abs() < 1e-5, "y ~ 0, got {}", q[1]);
        assert!((q[2]).abs() < 1e-5, "z ~ 0, got {}", q[2]);
        assert!((q[3].abs() - 1.0).abs() < 1e-5, "w ~ ±1, got {}", q[3]);
    }

    #[test]
    fn orientation_is_finite_for_degenerate_inputs() {
        // Zero forward and forward-parallel-to-up must not produce NaNs.
        for q in [
            orientation_from_forward_up([0.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            orientation_from_forward_up([0.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
        ] {
            assert!(q.iter().all(|c| c.is_finite()), "orientation {q:?} finite");
        }
    }

    #[test]
    fn play_returns_handle_and_holds_a_voice() {
        let mut audio = mock_audio();
        assert_eq!(audio.active_voices(BusId::Sfx), 0);

        let handle = audio.play(sfx_request());
        assert!(handle.is_some(), "loaded SFX fixture plays");
        assert_eq!(
            audio.active_voices(BusId::Sfx),
            1,
            "playing a sound holds one SFX voice",
        );
    }

    #[test]
    fn stop_releases_the_voice() {
        let mut audio = mock_audio();
        let handle = audio.play(sfx_request()).expect("fixture plays");
        assert_eq!(audio.active_voices(BusId::Sfx), 1);

        audio.stop(handle);
        assert_eq!(
            audio.active_voices(BusId::Sfx),
            0,
            "stop releases the held voice",
        );
    }

    #[test]
    fn stop_on_unknown_handle_is_a_noop() {
        let mut audio = mock_audio();
        // An id never minted by this Audio resolves to nothing.
        audio.stop(SoundHandle::from_raw_for_test(9999));
        assert_eq!(audio.active_voices(BusId::Sfx), 0);
    }

    #[test]
    fn unknown_bus_returns_none_without_holding_a_voice() {
        let mut audio = mock_audio();
        let handle = audio.play(SoundRequest {
            bus: "reverb".to_string(),
            sound: "sfx/test_tone".to_string(),
            looping: false,
        });
        assert!(handle.is_none(), "unknown bus is dropped");
        for bus in BusId::ALL {
            assert_eq!(audio.active_voices(bus), 0, "no voice acquired on any bus");
        }
    }

    #[test]
    fn unknown_sound_returns_none_without_holding_a_voice() {
        let mut audio = mock_audio();
        let handle = audio.play(SoundRequest {
            bus: "sfx".to_string(),
            sound: "sfx/does_not_exist".to_string(),
            looping: false,
        });
        assert!(handle.is_none(), "unknown sound is dropped");
        assert_eq!(
            audio.active_voices(BusId::Sfx),
            0,
            "a missing sound never consumes a voice",
        );
    }

    #[test]
    fn looping_request_holds_its_voice_across_a_sweep() {
        // A looping sound never reaches `Stopped`, so the finished-voice sweep in
        // `update` must leave it holding its voice. Advance playback well past the
        // 0.25s fixture, then sweep: the loop is still held.
        let mut audio = mock_audio();
        let handle = audio.play(SoundRequest {
            bus: "sfx".to_string(),
            sound: "sfx/test_tone".to_string(),
            looping: true,
        });
        assert!(handle.is_some(), "looping fixture plays");
        assert_eq!(audio.active_voices(BusId::Sfx), 1);

        advance_playback(&mut audio, 8);
        audio.update(forward_listener(), 1.0 / 60.0);

        assert_eq!(
            audio.active_voices(BusId::Sfx),
            1,
            "a looping sound keeps its voice across the sweep",
        );
    }

    #[test]
    fn finished_sweep_reclaims_a_stopped_one_shot() {
        // A non-looping sound that has run to its end reports `Stopped`; the sweep
        // inside `update` must release its voice or the bus leaks capacity.
        let mut audio = mock_audio();
        let _handle = audio.play(sfx_request()).expect("fixture plays");
        assert_eq!(audio.active_voices(BusId::Sfx), 1);

        // Drive kira past the clip end. At the mock backend's 1 Hz sample rate,
        // each `process()` advances two seconds of audio time — a handful clears
        // the 0.25s fixture — and `on_start_processing` flushes the play command.
        advance_playback(&mut audio, 8);

        audio.update(forward_listener(), 1.0 / 60.0);
        assert_eq!(
            audio.active_voices(BusId::Sfx),
            0,
            "the sweep reclaims the finished one-shot's voice",
        );
    }

    /// A listener looking down -Z, world up — the kira reference pose.
    fn forward_listener() -> ListenerState {
        ListenerState {
            position: [0.0, 0.0, 0.0],
            forward: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
        }
    }

    /// Step the mock renderer so queued play commands take effect and playback
    /// advances. `on_start_processing` drains command buffers; `process` advances
    /// audio time. Interleaved `steps` times.
    fn advance_playback(audio: &mut Audio<MockBackend>, steps: usize) {
        for _ in 0..steps {
            audio.manager.backend_mut().on_start_processing();
            audio.manager.backend_mut().process();
        }
    }
}
