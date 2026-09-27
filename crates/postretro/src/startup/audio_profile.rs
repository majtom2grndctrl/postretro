// App-boundary mapping from the mod's audio profile to the audio subsystem.
// See: context/lib/audio.md §5

use postretro_scripting_core::runtime::{ModAttenuation, ModAttenuationCurve, ModAudioProfile};

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

impl App {
    /// Commit a mod's audio profile. The attenuation applies to positional
    /// sounds started from now on; sounds already playing keep theirs. A silent
    /// run (no audio device) has nothing to configure.
    pub(crate) fn apply_mod_audio_profile(&mut self, profile: ModAudioProfile) {
        if let Some(audio) = self
            .session
            .as_mut()
            .and_then(|session| session.audio.as_mut())
        {
            audio.set_attenuation(audio_attenuation(profile.attenuation));
        }
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
