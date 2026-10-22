// Audio subsystem: owns kira's AudioManager, the mixer tree, and the
// primitive-typed boundary the frame loop talks to. No kira, wgpu or glam type
// crosses this crate's public surface.
// See: context/lib/audio.md

mod assets;
mod audio_core;
mod boundary;
mod buses;
mod ghost_bus;
mod mono;
mod muffle;
mod orientation;
mod playable;
mod spatial;
mod voices;
mod volume;

pub use boundary::{
    Attenuation, AttenuationCurve, AudioError, ListenerState, SoundAnchor, SoundRequest,
};
pub use buses::BusId;
pub use ghost_bus::GhostBusProfile;
pub use voices::SoundHandle;
pub use volume::linear_volume_to_decibels;

use std::path::Path;
use std::time::Duration;

use kira::{AudioManager, AudioManagerSettings, DefaultBackend};

use audio_core::{AudioCore, CAPACITIES, main_track_builder};
use mono::MonoFoldHandle;

/// Owns the kira audio manager and the spatial listener anchor.
///
/// Constructed once after the renderer is ready. If construction fails the
/// caller keeps its `Option<Audio>` as `None` and the game runs silent. The
/// type is concrete over kira's default backend, so no kira type crosses the
/// crate's public surface; in-crate tests drive the same core on a capturing
/// or mock backend.
pub struct Audio {
    core: AudioCore<DefaultBackend>,
}

impl Audio {
    /// Build the manager on the default (cpal) backend and create the listener
    /// anchor at the world origin. Returns `AudioError::Init` if the backend or
    /// listener allocation fails; the caller degrades to silent.
    pub fn new() -> Result<Self, AudioError> {
        let mono = MonoFoldHandle::default();
        let settings = AudioManagerSettings::<DefaultBackend> {
            capacities: CAPACITIES,
            main_track_builder: main_track_builder(&mono),
            ..Default::default()
        };
        let manager = AudioManager::<DefaultBackend>::new(settings)
            .map_err(|err| AudioError::Init(err.to_string()))?;
        Ok(Self {
            core: AudioCore::assemble(manager, mono)?,
        })
    }

    /// Name the pawn the listener is attached to for the plays that follow.
    /// `play` decides own-pawn treatment when it admits a sound, which is
    /// before this frame's [`update`](Self::update), so the frame loop names
    /// this frame's pawn here first; otherwise the first sounds after a level
    /// load or respawn would be judged against the previous pawn. `update`
    /// sets it again from its [`ListenerState`].
    pub fn set_listener_attached(&mut self, attached: Option<u64>) {
        self.core.set_listener_attached(attached);
    }

    /// Set the attenuation positional sounds admitted from now on start with.
    /// Sounds already playing keep theirs. An invalid value warns and falls back
    /// to [`Attenuation::DEFAULT`]; callers validate authored values first.
    pub fn set_attenuation(&mut self, attenuation: Attenuation) {
        self.core.set_attenuation(attenuation);
    }

    /// Set the runtime volume of a mixer bus, in decibels (0 dB = unity gain,
    /// negative attenuates, positive boosts). The level change fades over
    /// kira's default ~10 ms tween rather than cutting.
    pub fn set_bus_volume(&mut self, bus: BusId, decibels: f32) {
        self.core.set_bus_volume(bus, decibels);
    }

    /// Set the overall output volume, in decibels: the parent of the
    /// SFX/Music/UI buses, so it scales every bus at once. Same decibel and
    /// fade convention as [`set_bus_volume`](Self::set_bus_volume).
    pub fn set_main_volume(&mut self, db: f32) {
        self.core.set_main_volume(db);
    }

    /// Fold left and right together (mono) or return to stereo. The change
    /// crossfades on the audio thread, and a toggle reversed mid-crossfade
    /// turns back from the current mix.
    pub fn set_mono(&mut self, mono: bool) {
        self.core.set_mono(mono);
    }

    /// Low-pass the world-sound (SFX) bus, positional voices included; Music
    /// and UI are never filtered. `amount` 0 is none and passes the bus
    /// untouched, 1 is the maximum low-pass; a non-finite amount reads as 0.
    /// The cutoff falls log-linearly with `amount` from 20 kHz to 300 Hz. The
    /// effect moves toward a new amount over a short ramp on the audio thread,
    /// so a change reversed mid-ramp turns back from where it is.
    pub fn set_world_muffle(&mut self, amount: f32) {
        self.core.set_world_muffle(amount);
    }

    /// Move the Ghost bus to `profile` over `ramp` (at least 10 ms, linear).
    /// A change made mid-ramp starts from the bus's current values, so a
    /// reversed ramp turns back rather than stepping. The bus is silent until
    /// the first profile applies. An invalid profile (a field non-finite or out
    /// of range, or `high_pass_hz` at or above `low_pass_hz`) warns and leaves
    /// the bus as it is; callers resolve and validate first.
    pub fn set_ghost_bus_profile(&mut self, profile: GhostBusProfile, ramp: Duration) {
        self.core.set_ghost_bus_profile(profile, ramp);
    }

    /// Load every decodable sound under `<content_root>/sounds/` into the
    /// registry, replacing any sounds from a previously installed level.
    /// Missing directory or undecodable files degrade gracefully (warn, skip);
    /// never panics.
    pub fn load_level_sounds(&mut self, content_root: &Path) {
        self.core.load_level_sounds(content_root);
    }

    /// Drop every registered sound, releasing registry memory with the level.
    pub fn release_level_sounds(&mut self) {
        self.core.release_level_sounds();
    }

    /// Whether the level's sound registry holds `key`. Install-time key checks
    /// read this; `play` still drops an unknown key on its own.
    pub fn has_sound(&self, key: &str) -> bool {
        self.core.has_sound(key)
    }

    /// Start playing the requested sound, returning an opaque [`SoundHandle`]
    /// the caller can later pass to [`stop`](Self::stop), or `None` — never
    /// panicking — when the request can't be honored (unknown bus or key, a
    /// positioned request off SFX and Ghost or looping, a full bus, or a kira
    /// refusal; each warns). `Some` is the admission outcome: the request holds
    /// a voice on its bus. An unanchored request starts now; an anchored one is
    /// admitted now and starts at the next [`update`](Self::update), unless it
    /// is anchored on the listener's own pawn, which starts now, unpositioned.
    pub fn play(&mut self, req: SoundRequest) -> Option<SoundHandle> {
        self.core.play(req)
    }

    /// Stop a sound started via [`play`](Self::play) and release its voice
    /// slot. A no-op if `handle` is unknown.
    pub fn stop(&mut self, handle: SoundHandle) {
        self.core.stop(handle);
    }

    /// Fade out every sound anchored in the world — positional voices and
    /// own-pawn sounds alike — and release their slots. Unanchored sounds
    /// (music, UI) are untouched.
    pub fn fade_out_positional(&mut self) {
        self.core.fade_out_positional();
    }

    /// Per-frame audio step, third in frame order: re-anchors the listener,
    /// reclaims finished voices, moves tracked positional voices to where
    /// `resolve` places their entities, and starts newly admitted positional
    /// voices. `dt` (seconds) paces the reposition tween. See `audio.md` §3.
    pub fn update(
        &mut self,
        listener: ListenerState,
        dt: f32,
        resolve: impl FnMut(u64) -> Option<[f32; 3]>,
    ) {
        self.core.update(listener, dt, resolve);
    }

    /// Drop the manager, stopping the audio thread and releasing the device.
    /// Production teardown happens via `Drop` when `App` exits — `shutdown` is
    /// for deterministic cleanup.
    pub fn shutdown(self) {
        // Dropping the core's manager tears down the kira backend.
    }
}
