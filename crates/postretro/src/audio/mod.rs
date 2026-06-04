// Audio subsystem: owns kira's AudioManager, the mixer tree, and the
// primitive-typed boundary the frame loop talks to. No wgpu/glam types cross
// this module's public surface.
// See: context/lib/audio.md

mod assets;
mod buses;

pub use buses::BusId;

use std::path::Path;

use buses::BusTree;
use kira::listener::ListenerHandle;
use kira::{AudioManager, AudioManagerSettings, Capacities, DefaultBackend};

pub(crate) use assets::SoundRegistry;

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
pub struct Audio {
    // Read by the sound registry and play API in later tasks.
    #[allow(dead_code)]
    manager: AudioManager<DefaultBackend>,
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
    /// API (Task 4) routes sounds to a bus and consults its voice counter.
    buses: BusTree,
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
        })
    }

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

    /// Drop the manager, stopping the audio thread and releasing the device.
    /// Consumes `self`; equivalent to letting `Audio` fall out of scope.
    /// Wired into the app shutdown path by a later task.
    #[allow(dead_code)]
    pub fn shutdown(self) {
        // Dropping `manager` (and `listener`) tears down the kira backend.
    }
}
