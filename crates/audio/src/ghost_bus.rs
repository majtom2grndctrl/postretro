// Ghost bus profile and its kira effect chain: distortion drive, high-pass,
// low-pass and volume on the Ghost track under SFX.
// See: context/lib/audio.md §1 (Ghost bus)

use std::time::Duration;

use kira::effect::distortion::{DistortionBuilder, DistortionHandle, DistortionKind};
use kira::effect::filter::{FilterBuilder, FilterHandle, FilterMode};
use kira::effect::volume_control::{VolumeControlBuilder, VolumeControlHandle};
use kira::track::TrackBuilder;
use kira::{Decibels, Easing, Mix, StartTime, Tween};

use crate::volume::linear_volume_to_decibels;

/// The Ghost bus mix, one shared profile for every ghost voice. The engine's
/// floor values and the manifest resolution live with the mod's audio profile;
/// this type only carries a resolved profile across the boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GhostBusProfile {
    /// Linear `0..=1`, on the perceptual taper bus volumes use (40·log₁₀ v dB);
    /// 0 silences the bus.
    pub volume: f32,
    /// High-pass cutoff in Hz, `20..=20000`, below `low_pass_hz`.
    pub high_pass_hz: f32,
    /// Low-pass cutoff in Hz, `20..=20000`, above `high_pass_hz`.
    pub low_pass_hz: f32,
    /// Distortion drive `0..=1`. 0 applies no distortion at all: the stage
    /// passes the signal through untouched.
    pub drive: f32,
}

impl GhostBusProfile {
    /// Lowest accepted filter cutoff.
    pub const MIN_CUTOFF_HZ: f32 = 20.0;
    /// Highest accepted filter cutoff.
    pub const MAX_CUTOFF_HZ: f32 = 20_000.0;

    /// Every field finite and in range, and the high-pass strictly below the
    /// low-pass.
    pub fn is_valid(&self) -> bool {
        let unit = |value: f32| value.is_finite() && (0.0..=1.0).contains(&value);
        let cutoff =
            |hz: f32| hz.is_finite() && (Self::MIN_CUTOFF_HZ..=Self::MAX_CUTOFF_HZ).contains(&hz);
        unit(self.volume)
            && unit(self.drive)
            && cutoff(self.high_pass_hz)
            && cutoff(self.low_pass_hz)
            && self.high_pass_hz < self.low_pass_hz
    }
}

/// Distortion drive at `drive` 1. kira's distortion divides its output back by
/// the drive gain, so a higher drive squashes peaks harder rather than getting
/// louder; 24 dB (×16) is a heavily saturated, still-recognisable sound.
const MAX_DRIVE_DECIBELS: f32 = 24.0;

/// Shortest profile change. A zero-length change would step the volume and
/// cutoffs within one block and click; 10 ms is kira's own de-click tween.
const MIN_PROFILE_RAMP: Duration = Duration::from_millis(10);

/// Pre-clip gain for `drive`: linear in decibels, 0 dB at drive 0.
pub(crate) fn drive_decibels(drive: f32) -> f32 {
    unit_or_zero(drive) * MAX_DRIVE_DECIBELS
}

/// Wet mix for `drive`: the distorted signal fades in with the drive, so drive
/// 0 is fully dry and the stage leaves the signal bit-for-bit untouched (kira
/// mixes `wet·√mix + dry·√(1 − mix)`).
pub(crate) fn drive_mix(drive: f32) -> f32 {
    unit_or_zero(drive)
}

fn unit_or_zero(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The distortion stage as it stands at `drive`.
pub(crate) fn distortion_stage(drive: f32) -> DistortionBuilder {
    DistortionBuilder::new()
        .kind(DistortionKind::SoftClip)
        .drive(Decibels(drive_decibels(drive)))
        .mix(Mix(drive_mix(drive)))
}

/// Handles to the Ghost track's effects. The chain runs drive → high-pass →
/// low-pass → volume: the filters follow the drive so the harmonics it adds
/// stay inside the band, and the bus output stays band-limited at any drive.
pub(crate) struct GhostBusEffects {
    drive: DistortionHandle,
    high_pass: FilterHandle,
    low_pass: FilterHandle,
    volume: VolumeControlHandle,
}

impl GhostBusEffects {
    /// Add the chain to the Ghost track's builder. The bus starts silent, with
    /// open filters and no drive, until [`apply`](Self::apply) sets the mod's
    /// resolved profile: no profile, no ghost mix.
    pub(crate) fn attach(builder: &mut TrackBuilder) -> Self {
        let drive = builder.add_effect(distortion_stage(0.0));
        let high_pass = builder.add_effect(
            FilterBuilder::new()
                .mode(FilterMode::HighPass)
                .cutoff(f64::from(GhostBusProfile::MIN_CUTOFF_HZ)),
        );
        let low_pass = builder.add_effect(
            FilterBuilder::new()
                .mode(FilterMode::LowPass)
                .cutoff(f64::from(GhostBusProfile::MAX_CUTOFF_HZ)),
        );
        let volume = builder.add_effect(VolumeControlBuilder::new(Decibels::SILENCE));
        Self {
            drive,
            high_pass,
            low_pass,
            volume,
        }
    }

    /// Move every parameter to `profile` over `ramp` (at least 10 ms), linear.
    /// kira starts each change from the parameter's current value, so a change
    /// reversed mid-ramp turns back from where it is and never steps. The
    /// caller validates `profile` first.
    pub(crate) fn apply(&mut self, profile: GhostBusProfile, ramp: Duration) {
        let tween = Tween {
            start_time: StartTime::Immediate,
            duration: ramp.max(MIN_PROFILE_RAMP),
            easing: Easing::Linear,
        };
        self.drive
            .set_drive(Decibels(drive_decibels(profile.drive)), tween);
        self.drive.set_mix(Mix(drive_mix(profile.drive)), tween);
        self.high_pass
            .set_cutoff(f64::from(profile.high_pass_hz), tween);
        self.low_pass
            .set_cutoff(f64::from(profile.low_pass_hz), tween);
        self.volume
            .set_volume(Decibels(linear_volume_to_decibels(profile.volume)), tween);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kira::Frame;
    use kira::effect::EffectBuilder;
    use kira::info::MockInfoBuilder;

    const SAMPLE_RATE: f64 = 48_000.0;

    fn loud_tone(frames: usize) -> Vec<Frame> {
        (0..frames)
            .map(|n| {
                let s = 0.9 * ((n as f64 / SAMPLE_RATE) * 440.0 * std::f64::consts::TAU).sin();
                Frame {
                    left: s as f32,
                    right: s as f32,
                }
            })
            .collect()
    }

    fn through_distortion_stage(drive: f32, input: &[Frame]) -> Vec<Frame> {
        let (mut effect, _handle) = distortion_stage(drive).build();
        let info = MockInfoBuilder::new().build();
        let mut frames = input.to_vec();
        effect.on_start_processing();
        effect.process(&mut frames, 1.0 / SAMPLE_RATE, &info);
        frames
    }

    #[test]
    fn zero_drive_passes_a_loud_signal_through_the_distortion_stage_untouched() {
        let input = loud_tone(512);
        assert_eq!(
            through_distortion_stage(0.0, &input),
            input,
            "drive 0 applies no distortion at all",
        );
        assert_ne!(
            through_distortion_stage(0.5, &input),
            input,
            "a nonzero drive does distort the same signal",
        );
    }

    #[test]
    fn drive_maps_linearly_onto_decibels_and_wet_mix() {
        assert_eq!(drive_decibels(0.0), 0.0);
        assert_eq!(drive_mix(0.0), 0.0);
        assert_eq!(drive_decibels(1.0), MAX_DRIVE_DECIBELS);
        assert_eq!(drive_mix(1.0), 1.0);
        assert!((drive_decibels(0.5) - MAX_DRIVE_DECIBELS / 2.0).abs() < 1e-6);
        assert_eq!(drive_mix(f32::NAN), 0.0, "a non-finite drive reads as none");
    }

    #[test]
    fn profile_validity_requires_ranges_and_ordered_cutoffs() {
        let valid = GhostBusProfile {
            volume: 0.6,
            high_pass_hz: 200.0,
            low_pass_hz: 4000.0,
            drive: 0.0,
        };
        assert!(valid.is_valid());
        for invalid in [
            GhostBusProfile {
                volume: 1.5,
                ..valid
            },
            GhostBusProfile {
                drive: -0.1,
                ..valid
            },
            GhostBusProfile {
                high_pass_hz: 10.0,
                ..valid
            },
            GhostBusProfile {
                low_pass_hz: 25_000.0,
                ..valid
            },
            GhostBusProfile {
                high_pass_hz: 4000.0,
                ..valid
            },
            GhostBusProfile {
                volume: f32::NAN,
                ..valid
            },
        ] {
            assert!(!invalid.is_valid(), "{invalid:?} is invalid");
        }
    }
}
