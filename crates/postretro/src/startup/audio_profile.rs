// App-boundary mapping from the mod's audio profile to the audio subsystem.
// See: context/lib/audio.md §1 (Ghost bus), §5

use std::time::Duration;

use postretro_foundation::GhostAudioValues;
use postretro_scripting_core::runtime::{
    ModAttenuation, ModAttenuationCurve, ModAudioProfile, ModGhostAudio,
};

use crate::App;

/// Translate the scripting-core attenuation into the audio crate's own. This
/// is the single chokepoint between the two vocabularies: scripting-core never
/// depends on the audio crate. The curve `match` has no `_` arm, so a new
/// curve fails to compile here rather than silently degrading.
pub(crate) fn audio_attenuation(attenuation: ModAttenuation) -> postretro_audio::Attenuation {
    postretro_audio::Attenuation {
        min_distance: attenuation.min_distance,
        max_distance: attenuation.max_distance,
        curve: match attenuation.curve {
            ModAttenuationCurve::Linear => postretro_audio::AttenuationCurve::Linear,
            ModAttenuationCurve::Quadratic => postretro_audio::AttenuationCurve::Quadratic,
        },
    }
}

/// The scripting-core manifest ghost profile as the per-field values the bus
/// resolution composes. The floor values live only in scripting-core
/// (`ModGhostAudio::FLOOR`); neither foundation nor the audio crate carries any.
pub(crate) fn ghost_audio_values(ghost: ModGhostAudio) -> GhostAudioValues {
    GhostAudioValues {
        volume: ghost.volume,
        high_pass_hz: ghost.high_pass_hz,
        low_pass_hz: ghost.low_pass_hz,
        drive: ghost.drive,
    }
}

/// Translate a resolved ghost bus profile into the audio crate's own, at the
/// same chokepoint as attenuation.
pub(crate) fn audio_ghost_profile(ghost: GhostAudioValues) -> postretro_audio::GhostBusProfile {
    postretro_audio::GhostBusProfile {
        volume: ghost.volume,
        high_pass_hz: ghost.high_pass_hz,
        low_pass_hz: ghost.low_pass_hz,
        drive: ghost.drive,
    }
}

impl App {
    /// Commit a mod's audio profile, at mod init and on each committed staged
    /// reload. The attenuation applies to positional sounds started from now
    /// on; sounds already playing keep theirs. The manifest ghost profile
    /// re-resolves the shared Ghost bus at once (the bus's own de-click ramp):
    /// fields a script set keep their values and ramps, and cutoffs the
    /// new profile crosses against them present the floor's. A silent run
    /// (no audio device) still resolves, so its warnings match.
    pub(crate) fn apply_mod_audio_profile(&mut self, profile: ModAudioProfile) {
        if let Some(audio) = self
            .session
            .as_mut()
            .and_then(|session| session.audio.as_mut())
        {
            audio.set_attenuation(audio_attenuation(profile.attenuation));
        }
        self.ghost_bus.set_manifest(profile.ghost);
        self.present_ghost_bus(Duration::ZERO);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_attenuation_matches_the_audio_crate_default() {
        assert_eq!(
            audio_attenuation(ModAttenuation::default()),
            postretro_audio::Attenuation::DEFAULT,
            "scripting-core's seed and the audio crate's default must not drift",
        );
    }

    // AC: the floor bus profile applies no distortion. The audio crate's ghost
    // bus tests prove drive 0 leaves the signal untouched.
    #[test]
    fn floor_ghost_profile_reaches_the_bus_valid_and_undistorted() {
        let floor = audio_ghost_profile(ghost_audio_values(ModGhostAudio::FLOOR));
        assert!(
            floor.is_valid(),
            "{floor:?} must be a profile the bus accepts"
        );
        assert_eq!(floor.drive, 0.0);
    }

    #[test]
    fn ghost_cutoff_range_matches_the_audio_crate() {
        // Every cutoff scripting-core accepts must be one the bus accepts, and
        // the reverse, or a validated manifest could be refused at the bus.
        assert_eq!(
            ModGhostAudio::MIN_CUTOFF_HZ,
            postretro_audio::GhostBusProfile::MIN_CUTOFF_HZ
        );
        assert_eq!(
            ModGhostAudio::MAX_CUTOFF_HZ,
            postretro_audio::GhostBusProfile::MAX_CUTOFF_HZ
        );
    }

    #[test]
    fn authored_ghost_profile_converts_field_for_field() {
        let authored = ModGhostAudio {
            volume: 0.7,
            high_pass_hz: 250.0,
            low_pass_hz: 3000.0,
            drive: 0.3,
        };
        assert_eq!(
            audio_ghost_profile(ghost_audio_values(authored)),
            postretro_audio::GhostBusProfile {
                volume: 0.7,
                high_pass_hz: 250.0,
                low_pass_hz: 3000.0,
                drive: 0.3,
            },
        );
    }

    #[test]
    fn authored_attenuation_converts_field_for_field() {
        let authored = ModAttenuation {
            min_distance: 4.0,
            max_distance: 80.0,
            curve: ModAttenuationCurve::Quadratic,
        };
        assert_eq!(
            audio_attenuation(authored),
            postretro_audio::Attenuation {
                min_distance: 4.0,
                max_distance: 80.0,
                curve: postretro_audio::AttenuationCurve::Quadratic,
            },
        );
    }
}
