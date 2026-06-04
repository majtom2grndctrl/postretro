// Mixer bus tree (Master → SFX/Music/UI), per-bus volume, and the per-bus
// active-voice budget. kira's main track IS Master; the three buses are
// sub-tracks of it.
// See: context/lib/audio.md §1 (Mixer bus tree)

use kira::backend::Backend;
use kira::track::{TrackBuilder, TrackHandle};
use kira::{AudioManager, ResourceLimitReached, Tween};

/// The three controllable mixer buses. Master is kira's main track and is
/// addressed separately (see `Audio::set_master_volume`), so it has no variant
/// here — every routable in-world/UI sound targets one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusId {
    /// In-world sound effects (footsteps, gunshots, impacts).
    Sfx,
    /// Music streams.
    Music,
    /// UI / menu sounds.
    UI,
}

impl BusId {
    /// All buses, in a fixed order. Used to size and iterate the bus arrays.
    pub(crate) const ALL: [BusId; 3] = [BusId::Sfx, BusId::Music, BusId::UI];

    /// Dense index into the per-bus arrays held by `BusTree`.
    pub(crate) fn index(self) -> usize {
        match self {
            BusId::Sfx => 0,
            BusId::Music => 1,
            BusId::UI => 2,
        }
    }

    /// Active-voice cap for this bus: the maximum number of sounds that may
    /// play simultaneously on it. A bus at its cap drops further requests.
    ///
    /// INVARIANT: each bus's kira `TrackBuilder::sound_capacity` is sized
    /// `>= voice_cap` (see [`Self::track_sound_capacity`]), so a `play` accepted
    /// by the voice counter is never silently dropped at the kira layer.
    pub(crate) fn voice_cap(self) -> usize {
        match self {
            BusId::Sfx => 32,
            BusId::Music => 4,
            BusId::UI => 8,
        }
    }

    /// kira sound capacity to preallocate for this bus's sub-track. Sized equal
    /// to the voice cap: the engine's voice counter is the binding limit, and
    /// kira's per-track sound pool must be at least as large so accepted plays
    /// always find a slot.
    fn track_sound_capacity(self) -> usize {
        self.voice_cap()
    }

    /// Human-readable bus name for log messages.
    fn name(self) -> &'static str {
        match self {
            BusId::Sfx => "sfx",
            BusId::Music => "music",
            BusId::UI => "ui",
        }
    }
}

/// Per-bus active-voice tally with a fixed cap. The play path (Task 4) reserves
/// a slot before starting a sound and releases it when the sound finishes;
/// `stop`/cleanup decrements.
#[derive(Debug, Default)]
struct VoiceCounter {
    active: usize,
}

/// The constructed bus tree: one kira sub-track handle per bus plus the voice
/// budget. Dropping a handle removes its track from kira, so this lives as long
/// as the manager.
pub(crate) struct BusTree {
    // Indexed by `BusId::index`. Held to keep the sub-tracks alive and to drive
    // per-bus volume.
    tracks: [TrackHandle; BusId::ALL.len()],
    counters: [VoiceCounter; BusId::ALL.len()],
}

impl BusTree {
    /// Build SFX/Music/UI as sub-tracks of `manager`'s main track (Master).
    ///
    /// Generic over the backend so tests drive it with `MockBackend` while the
    /// engine uses `DefaultBackend`. Each sub-track starts at unity gain (0 dB)
    /// with a sound pool sized to its voice cap. Returns `ResourceLimitReached`
    /// if the manager's `sub_track_capacity` is exhausted; the caller folds that
    /// into the fault-tolerant init path.
    ///
    /// kira 0.12 note: sub-tracks created via `manager.add_sub_track` are
    /// children of the main track. `MainTrackHandle` exposes no `add_sub_track`
    /// in 0.12, so we route through the manager rather than `main_track()`.
    pub(crate) fn build<B: Backend>(
        manager: &mut AudioManager<B>,
    ) -> Result<Self, ResourceLimitReached> {
        debug_assert!(
            Self::caps_within_capacity(),
            "[Audio] sum of per-bus voice caps exceeds the provisioned budget"
        );

        // Build in `BusId::ALL` order so the array lines up with `index()`.
        let sfx = Self::build_bus(manager, BusId::Sfx)?;
        let music = Self::build_bus(manager, BusId::Music)?;
        let ui = Self::build_bus(manager, BusId::UI)?;

        Ok(Self {
            tracks: [sfx, music, ui],
            counters: Default::default(),
        })
    }

    fn build_bus<B: Backend>(
        manager: &mut AudioManager<B>,
        bus: BusId,
    ) -> Result<TrackHandle, ResourceLimitReached> {
        // 0 dB == unity gain; volume is set in decibels (kira's `Value<Decibels>`).
        let builder = TrackBuilder::new()
            .volume(0.0)
            .sound_capacity(bus.track_sound_capacity());
        let handle = manager.add_sub_track(builder)?;

        debug_assert!(
            handle.sound_capacity() >= bus.voice_cap(),
            "[Audio] bus {} kira sound capacity {} < voice cap {}",
            bus.name(),
            handle.sound_capacity(),
            bus.voice_cap(),
        );

        Ok(handle)
    }

    /// Mutable access to a bus's kira sub-track, for the play path to start
    /// sounds on it. The caller is responsible for the voice budget; this only
    /// hands back the track handle.
    pub(crate) fn track_mut(&mut self, bus: BusId) -> &mut TrackHandle {
        &mut self.tracks[bus.index()]
    }

    /// Set a bus's runtime volume in decibels. 0 dB is unity, negative
    /// attenuates, positive boosts. Applied instantly (no fade).
    pub(crate) fn set_volume(&mut self, bus: BusId, decibels: f32) {
        self.tracks[bus.index()].set_volume(decibels, Tween::default());
    }

    /// Try to reserve a voice slot on `bus`. Returns `true` and increments the
    /// active count on success; returns `false` (count unchanged) when the bus
    /// is at its cap. The play path (Task 4) drops-and-logs on `false`.
    pub(crate) fn try_acquire_voice(&mut self, bus: BusId) -> bool {
        let cap = bus.voice_cap();
        let counter = &mut self.counters[bus.index()];
        if counter.active >= cap {
            log::warn!(
                "[Audio] bus {} at voice cap {}, dropping sound request",
                bus.name(),
                cap,
            );
            return false;
        }
        counter.active += 1;
        true
    }

    /// Release a previously reserved voice slot. Saturating: a release with no
    /// outstanding voice is a no-op rather than an underflow.
    pub(crate) fn release_voice(&mut self, bus: BusId) {
        let counter = &mut self.counters[bus.index()];
        counter.active = counter.active.saturating_sub(1);
    }

    /// Current active-voice count on `bus`.
    pub(crate) fn active_voices(&self, bus: BusId) -> usize {
        self.counters[bus.index()].active
    }

    /// INVARIANT check: the sum of per-bus voice caps must stay within the
    /// budget the mixer is provisioned for. Each bus owns its own kira sound
    /// pool (sized `>= voice_cap`), so the real ceiling in 0.12 is per-track,
    /// not a single global "sound" field. This gate guards against the caps
    /// drifting past the conservative total the subsystem was designed for, so
    /// a `play` accepted by the voice counter always finds a kira slot.
    fn caps_within_capacity() -> bool {
        // Provisioned headroom for simultaneous voices across all buses.
        // Current caps sum to 32 + 4 + 8 = 44.
        const MAX_TOTAL_VOICES: usize = 64;
        let sum: usize = BusId::ALL.iter().map(|bus| bus.voice_cap()).sum();
        sum <= MAX_TOTAL_VOICES
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kira::backend::mock::MockBackend;
    use kira::{AudioManagerSettings, Capacities};

    /// Build a manager on the always-available mock backend with the same
    /// capacities the engine uses, so the bus tree exercises the real path
    /// without a sound device.
    fn mock_manager() -> AudioManager<MockBackend> {
        let settings = AudioManagerSettings::<MockBackend> {
            capacities: Capacities {
                sub_track_capacity: 16,
                send_track_capacity: 16,
                clock_capacity: 8,
                modulator_capacity: 16,
                listener_capacity: 4,
            },
            ..Default::default()
        };
        AudioManager::<MockBackend>::new(settings).expect("mock backend always starts")
    }

    #[test]
    fn bus_tree_builds_all_three_buses_under_mock_backend() {
        let mut manager = mock_manager();
        let tree = BusTree::build(&mut manager).expect("bus tree builds under mock backend");

        // Three sub-tracks were created off the main track.
        assert_eq!(manager.num_sub_tracks(), BusId::ALL.len());
        // Each bus starts with zero active voices.
        for bus in BusId::ALL {
            assert_eq!(tree.active_voices(bus), 0);
        }
    }

    #[test]
    fn each_bus_track_sound_capacity_covers_its_voice_cap() {
        let mut manager = mock_manager();
        let tree = BusTree::build(&mut manager).expect("bus tree builds");

        // The kira sound pool must be at least the voice cap so an accepted
        // play never hits kira's per-track sound limit.
        for bus in BusId::ALL {
            let handle = &tree.tracks[bus.index()];
            assert!(
                handle.sound_capacity() >= bus.voice_cap(),
                "bus {:?} sound_capacity {} < voice_cap {}",
                bus,
                handle.sound_capacity(),
                bus.voice_cap(),
            );
        }
    }

    #[test]
    fn set_volume_commits_without_panicking() {
        // kira's mock backend exposes no read-back of post-effects volume, so
        // this asserts the control-plane call accepts a decibel value and
        // commits a command through a processing tick (no panic, command
        // buffer not overflowed).
        let mut manager = mock_manager();
        let mut tree = BusTree::build(&mut manager).expect("bus tree builds");

        tree.set_volume(BusId::Sfx, -6.0);
        tree.set_volume(BusId::Music, 0.0);
        tree.set_volume(BusId::UI, -3.0);

        manager.backend_mut().on_start_processing();
    }

    #[test]
    fn voice_counter_increments_and_decrements() {
        let mut manager = mock_manager();
        let mut tree = BusTree::build(&mut manager).expect("bus tree builds");

        assert!(tree.try_acquire_voice(BusId::Sfx));
        assert!(tree.try_acquire_voice(BusId::Sfx));
        assert_eq!(tree.active_voices(BusId::Sfx), 2);

        tree.release_voice(BusId::Sfx);
        assert_eq!(tree.active_voices(BusId::Sfx), 1);

        // Buses are independent.
        assert_eq!(tree.active_voices(BusId::Music), 0);
    }

    #[test]
    fn voice_counter_rejects_at_cap_and_recovers_after_release() {
        let mut manager = mock_manager();
        let mut tree = BusTree::build(&mut manager).expect("bus tree builds");

        // Fill the Music bus exactly to its cap.
        let cap = BusId::Music.voice_cap();
        for _ in 0..cap {
            assert!(tree.try_acquire_voice(BusId::Music));
        }
        assert_eq!(tree.active_voices(BusId::Music), cap);

        // One past the cap is rejected and leaves the count untouched.
        assert!(!tree.try_acquire_voice(BusId::Music));
        assert_eq!(tree.active_voices(BusId::Music), cap);

        // Freeing a slot lets exactly one more in.
        tree.release_voice(BusId::Music);
        assert!(tree.try_acquire_voice(BusId::Music));
        assert_eq!(tree.active_voices(BusId::Music), cap);
    }

    #[test]
    fn release_with_no_active_voices_is_a_saturating_noop() {
        let mut manager = mock_manager();
        let mut tree = BusTree::build(&mut manager).expect("bus tree builds");

        tree.release_voice(BusId::UI);
        assert_eq!(tree.active_voices(BusId::UI), 0);
    }

    #[test]
    fn sum_of_voice_caps_stays_within_provisioned_budget() {
        let sum: usize = BusId::ALL.iter().map(|bus| bus.voice_cap()).sum();
        assert!(
            BusTree::caps_within_capacity(),
            "sum of voice caps {sum} exceeds provisioned budget",
        );
    }
}
