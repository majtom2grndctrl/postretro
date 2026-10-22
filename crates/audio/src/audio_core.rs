// The backend-generic audio core: kira's manager, the mixer tree, the voice
// tables and the frame step. `Audio` seals it over the default backend.
// See: context/lib/audio.md

use std::path::Path;
use std::time::Duration;

use kira::backend::Backend;
use kira::listener::ListenerHandle;
use kira::track::MainTrackBuilder;
use kira::{AudioManager, Capacities, Tween};

use crate::assets::SoundRegistry;
use crate::boundary::{
    Attenuation, AudioError, ListenerState, SoundAnchor, SoundRequest, parse_bus,
};
use crate::buses::{BusId, BusTree};
use crate::ghost_bus::GhostBusProfile;
use crate::mono::{MonoFoldBuilder, MonoFoldHandle};
use crate::muffle::WorldMuffleHandle;
use crate::orientation::orientation_from_forward_up;
use crate::playable::Playable;
use crate::spatial::{SpatialVoices, is_finite_point};
use crate::voices::{SoundHandle, VoiceTable};

/// Fade applied to every positional voice when its world goes away (unload,
/// restart, return to frontend): long enough to avoid a click, short enough to
/// finish before the next level's first frame.
const POSITIONAL_UNLOAD_FADE: Duration = Duration::from_millis(150);

/// Conservative initial mixer capacity. Sub-tracks back the SFX/Music/UI
/// bus tree; a handful of listeners covers the single anchor plus headroom.
/// Clock/modulator capacities stay at kira defaults — unused so far. These
/// bound kira's preallocation, not a hard runtime ceiling we expect to hit.
pub(crate) const CAPACITIES: Capacities = Capacities {
    sub_track_capacity: 16,
    send_track_capacity: 16,
    clock_capacity: 8,
    modulator_capacity: 16,
    listener_capacity: 4,
};

/// Identity orientation `[x, y, z, w]`. The listener is re-oriented each
/// frame in `update`; init just needs a valid quaternion.
pub(crate) const IDENTITY_ORIENTATION: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// The main track carries the mono fold, so it composes after every
/// sub-track, spatial panning included.
pub(crate) fn main_track_builder(mono: &MonoFoldHandle) -> MainTrackBuilder {
    MainTrackBuilder::new().with_effect(MonoFoldBuilder::new(mono.clone()))
}

/// The backend-generic audio core behind [`Audio`](crate::Audio): the kira
/// manager, the mixer tree and every voice. Production runs it on
/// `DefaultBackend`; in-crate tests drive it on a capturing or mock backend.
pub(crate) struct AudioCore<B: Backend> {
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
    /// Master → SFX/Music/UI (Ghost under SFX) mixer tree plus the per-bus
    /// voice budget. The play API routes sounds to a bus and consults its
    /// voice counter.
    buses: BusTree,
    /// Live playback handles for unpositioned sounds started via `play`, keyed
    /// by the opaque `SoundHandle`. `stop` looks handles up here; the per-frame
    /// sweep reclaims finished non-looping voices from it.
    voices: VoiceTable,
    /// Positional voices: the spatial chokepoint (`audio.md` §5).
    spatial: SpatialVoices,
    /// Attenuation captured by each positional sound as it is admitted.
    attenuation: Attenuation,
    /// The pawn the listener is attached to, as of the last
    /// [`set_listener_attached`](Self::set_listener_attached) or `update`. A
    /// sound anchored on it plays unpositioned and keeps that treatment for its
    /// whole life.
    attached: Option<u64>,
    /// The last finite listener position. A contact set resolves its nearest
    /// contact against it; a non-finite listener update leaves it unchanged.
    listener_position: [f32; 3],
    /// Control for the main-track mono fold.
    mono: MonoFoldHandle,
    /// Control for the world-sound (SFX bus) low-pass.
    muffle: WorldMuffleHandle,
}

/// Backend-agnostic surface. kira's playback/track/listener handles aren't
/// parameterized by backend, so every method below works whether the manager
/// runs on the real `DefaultBackend` (production) or `MockBackend` (tests). The
/// generic impl is what lets unit tests drive `play`/`stop`/`update` without a
/// sound device.
impl<B: Backend> AudioCore<B> {
    /// Finish a core around a started `manager` whose main track carries the
    /// fold `mono` controls: create the listener anchor and the bus tree. A
    /// listener or sub-track allocation failure folds into the fault-tolerant
    /// init path: the caller logs and runs silent.
    pub(crate) fn assemble(
        mut manager: AudioManager<B>,
        mono: MonoFoldHandle,
    ) -> Result<Self, AudioError> {
        // mint::Vector3/Quaternion accept these primitive arrays via `From`,
        // so no mint types appear here.
        let listener = manager
            .add_listener([0.0_f32, 0.0, 0.0], IDENTITY_ORIENTATION)
            .map_err(|err| AudioError::Init(err.to_string()))?;
        let muffle = WorldMuffleHandle::default();
        let buses = BusTree::build(&mut manager, &muffle)
            .map_err(|err| AudioError::Init(err.to_string()))?;
        Ok(Self {
            manager,
            listener,
            registry: SoundRegistry::new(),
            buses,
            voices: VoiceTable::new(),
            spatial: SpatialVoices::default(),
            attenuation: Attenuation::DEFAULT,
            attached: None,
            listener_position: [0.0; 3],
            mono,
            muffle,
        })
    }

    /// Name the pawn the listener is attached to for the plays that follow.
    /// `play` decides own-pawn treatment when it admits a sound, which is
    /// before this frame's [`update`](Self::update), so the frame loop names
    /// this frame's pawn here first; otherwise the first sounds after a level
    /// load or respawn would be judged against the previous pawn. `update`
    /// sets it again from its [`ListenerState`].
    pub(crate) fn set_listener_attached(&mut self, attached: Option<u64>) {
        self.attached = attached;
    }

    /// Set the attenuation positional sounds admitted from now on start with.
    /// Sounds already playing keep theirs. An invalid value warns and falls back
    /// to [`Attenuation::DEFAULT`]; callers validate authored values first.
    pub(crate) fn set_attenuation(&mut self, attenuation: Attenuation) {
        self.attenuation = if attenuation.is_valid() {
            attenuation
        } else {
            log::warn!("[Audio] invalid attenuation {attenuation:?}; using the default");
            Attenuation::DEFAULT
        };
    }

    /// Set the runtime volume of a mixer bus, in decibels (0 dB = unity gain,
    /// negative attenuates, positive boosts). The public volume control for
    /// SFX/Music/UI; delegates to the bus tree.
    pub(crate) fn set_bus_volume(&mut self, bus: BusId, decibels: f32) {
        self.buses.set_volume(bus, decibels);
    }

    /// Set the overall output volume, in decibels (0 dB = unity gain, negative
    /// attenuates, positive boosts). This drives kira's main track — the parent
    /// of the SFX/Music/UI sub-tracks — so it scales every bus at once. Same
    /// decibel/`Tween` convention as [`set_bus_volume`](Self::set_bus_volume):
    /// the level change fades over kira's default ~10 ms tween rather than
    /// cutting.
    pub(crate) fn set_main_volume(&mut self, db: f32) {
        self.manager.main_track().set_volume(db, Tween::default());
    }

    /// Fold left and right together (mono) or return to stereo. The change
    /// crossfades on the audio thread, and a toggle reversed mid-crossfade
    /// turns back from the current mix.
    pub(crate) fn set_mono(&mut self, mono: bool) {
        self.mono.set(mono);
    }

    /// Muffle the SFX bus by `amount` in 0..=1 (the app drives it while the
    /// camera eye is in a fluid); a non-finite amount reads as 0.
    pub(crate) fn set_world_muffle(&mut self, amount: f32) {
        self.muffle.set(amount);
    }

    /// Move the Ghost bus to `profile` over `ramp`. An invalid profile warns
    /// and leaves the bus as it is; callers resolve and validate first.
    pub(crate) fn set_ghost_bus_profile(&mut self, profile: GhostBusProfile, ramp: Duration) {
        if !profile.is_valid() {
            log::warn!(
                "[Audio] invalid ghost bus profile {profile:?}; the bus keeps its current mix"
            );
            return;
        }
        self.buses.set_ghost_profile(profile, ramp);
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
    pub(crate) fn load_level_sounds(&mut self, content_root: &Path) {
        self.registry.load_from_content_root(content_root);
    }

    /// Drop every registered sound. Wired into the level-unload / shutdown path
    /// so registry memory is released with the level. After this the registry is
    /// empty and a subsequent `load_level_sounds` repopulates it.
    pub(crate) fn release_level_sounds(&mut self) {
        self.registry.clear();
    }

    /// Whether the level's sound registry holds `key`. Install-time key checks
    /// read this; `play` still drops an unknown key on its own.
    pub(crate) fn has_sound(&self, key: &str) -> bool {
        self.registry.contains(key)
    }

    /// The per-level sound registry. Read-only so callers resolve
    /// `SoundRequest::sound` keys to loaded entries.
    #[allow(dead_code)]
    pub(crate) fn registry(&self) -> &SoundRegistry {
        &self.registry
    }

    /// Start playing the requested sound, returning an opaque [`SoundHandle`] the
    /// caller can later pass to [`stop`](Self::stop). Returns `None` — never
    /// panicking — when the request can't be honored:
    ///
    /// - the bus name is unrecognized, or a positioned request names a bus that
    ///   hosts no positional voices (anything but SFX and Ghost) or asks to loop
    ///   (warns),
    /// - the bus is at its voice cap, or kira still holds every slot the request
    ///   would need (warns; refused, never queued),
    /// - the sound key isn't in the registry (warns, and the voice is released),
    /// - or kira refuses the play (warns, and the voice is released).
    ///
    /// An unanchored request starts now on its bus. An anchored request is
    /// admitted now and starts at the next [`update`](Self::update), against
    /// that frame's listener; one anchored on the listener's own pawn (as last
    /// named by [`set_listener_attached`](Self::set_listener_attached) or
    /// `update`) instead starts now, unpositioned. A `looping` request applies
    /// a whole-clip loop region and holds its voice until `stop`.
    ///
    /// `Some` is the admission outcome: the request now holds a voice on its
    /// bus. A positional voice that later fails to start releases that voice at
    /// the audio step.
    pub(crate) fn play(&mut self, req: SoundRequest) -> Option<SoundHandle> {
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
        if req.anchor.is_some() && (!bus.hosts_positional() || req.looping) {
            log::warn!(
                "[Audio] positioned sound '{}' must be a one-shot on the sfx or ghost bus — request dropped",
                req.sound,
            );
            return None;
        }

        match req.anchor {
            Some(anchor) if !self.is_attached(&anchor) => {
                self.admit_positional(bus, &req.sound, anchor)
            }
            anchor => self.play_unpositioned(bus, &req.sound, req.looping, anchor.is_some()),
        }
    }

    /// Resolve `sound` for a request that already holds a voice on `bus`,
    /// releasing that voice when the sound cannot be resolved. Resolution runs
    /// after admission because a streaming asset reopens its file here: a
    /// refused request never pays for the open.
    fn resolve_admitted(&mut self, bus: BusId, sound: &str, looping: bool) -> Option<Playable> {
        let Some(playable) = Playable::resolve(&self.registry, sound) else {
            self.buses.release_voice(bus);
            return None;
        };
        Some(if looping { playable.looped() } else { playable })
    }

    fn is_attached(&self, anchor: &SoundAnchor) -> bool {
        matches!(anchor, SoundAnchor::Entity { key, .. } if Some(*key) == self.attached)
    }

    fn play_unpositioned(
        &mut self,
        bus: BusId,
        sound: &str,
        looping: bool,
        anchored: bool,
    ) -> Option<SoundHandle> {
        // kira frees a finished sound's slot on its own thread, after the engine
        // reclaims the voice, so its pool can be fuller than the counter says.
        let track = self.buses.track(bus);
        if track.num_sounds() >= track.sound_capacity() {
            log::warn!("[Audio] bus {bus:?} has no free kira sound slot — '{sound}' dropped");
            return None;
        }
        if !self.buses.try_acquire_voice(bus) {
            return None;
        }
        let playable = self.resolve_admitted(bus, sound, looping)?;
        match playable.start_on(self.buses.track_mut(bus)) {
            Ok(voice) => Some(self.voices.insert(voice, bus, anchored)),
            Err(err) => {
                log::warn!("[Audio] kira rejected sound '{sound}': {err}");
                self.buses.release_voice(bus);
                None
            }
        }
    }

    fn admit_positional(
        &mut self,
        bus: BusId,
        sound: &str,
        anchor: SoundAnchor,
    ) -> Option<SoundHandle> {
        if !self.spatial.has_room(bus, self.buses.track(bus)) {
            log::warn!("[Audio] bus {bus:?} has no free kira track slot — '{sound}' dropped");
            return None;
        }
        if !self.buses.try_acquire_voice(bus) {
            return None;
        }
        let playable = self.resolve_admitted(bus, sound, false)?;
        let handle = self.voices.mint();
        self.spatial
            .admit(handle, bus, playable, anchor, self.attenuation);
        Some(handle)
    }

    /// Stop a sound started via [`play`](Self::play) and release its voice slot.
    /// The voice is removed immediately; kira applies its default ~10 ms tween
    /// to fade the audio out. A no-op if `handle` is unknown — already finished
    /// and reclaimed, or never minted by this `Audio`.
    pub(crate) fn stop(&mut self, handle: SoundHandle) {
        if let Some(bus) = self
            .voices
            .remove_and_stop(handle, Tween::default())
            .or_else(|| self.spatial.stop(handle, Tween::default()))
        {
            self.buses.release_voice(bus);
        }
    }

    /// Fade out every sound anchored in the world — positional voices and
    /// own-pawn sounds alike — and release their slots. Called when the world
    /// goes away, so no sound outlives its level or follows an entity into the
    /// next one. Unanchored sounds (music, UI) are untouched.
    pub(crate) fn fade_out_positional(&mut self) {
        let tween = Tween {
            duration: POSITIONAL_UNLOAD_FADE,
            ..Tween::default()
        };
        self.spatial.stop_all(tween, &mut self.buses);
        for bus in self.voices.stop_anchored(tween) {
            self.buses.release_voice(bus);
        }
    }

    /// Per-frame audio step. Runs third in frame order (Input → Game logic →
    /// **Audio** → Render → Present). Control-plane only: never decodes or
    /// touches disk, so it never blocks the frame.
    ///
    /// In order, it:
    /// 1. re-anchors the kira listener to `listener` (a non-finite position or
    ///    orientation keeps the last finite one) and records its pawn;
    /// 2. reclaims finished one-shots, so a voice that ended frees its slot here
    ///    and not earlier in the frame;
    /// 3. moves each tracked positional voice to where `resolve` now places its
    ///    entity, freezing it at its last finite position once `resolve`
    ///    returns `None` or a non-finite point;
    /// 4. starts the positional voices admitted since the last step, releasing
    ///    the slot of any whose anchor has no finite point.
    ///
    /// `resolve` maps an anchor's entity key to its presented world position.
    /// `dt` (seconds) paces the reposition tween.
    pub(crate) fn update(
        &mut self,
        listener: ListenerState,
        dt: f32,
        mut resolve: impl FnMut(u64) -> Option<[f32; 3]>,
    ) {
        // A non-finite pose would reach kira's panning and attenuation math and
        // poison the mix; the listener keeps its last finite pose instead.
        if is_finite_point(listener.position) {
            self.listener_position = listener.position;
            self.listener
                .set_position(listener.position, Tween::default());
        }
        let orientation = orientation_from_forward_up(listener.forward, listener.up);
        if orientation.iter().all(|component| component.is_finite()) {
            self.listener.set_orientation(orientation, Tween::default());
        }
        self.attached = listener.attached;

        for bus in self.voices.reclaim_finished() {
            self.buses.release_voice(bus);
        }
        self.spatial.reclaim_finished(&mut self.buses);

        self.spatial.update(
            &mut self.buses,
            self.listener.id(),
            self.listener_position,
            dt,
            &mut resolve,
        );
    }
}

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
