// Ghost bus script override: the `setGhostAudio` fields and their script-time ramps.
// See: context/lib/audio.md §1 (Ghost bus) · context/lib/scripting.md §10.4

use serde::{Deserialize, Serialize};

use super::ghost_look::GhostScalarRamp;

/// `setGhostAudio({ volume?, highPassHz?, lowPassHz?, drive?, periodMs? })` from
/// `postretro/audio`: a target-less system reaction body.
pub const SET_GHOST_AUDIO: &str = "setGhostAudio";

/// One value per Ghost bus field, as the profile resolves them: the floor,
/// then the manifest `audio.ghost`, then each field `setGhostAudio` set.
/// Volume and drive are linear `0..=1`; cutoffs are in Hz.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GhostAudioValues {
    pub volume: f32,
    pub high_pass_hz: f32,
    pub low_pass_hz: f32,
    pub drive: f32,
}

impl GhostAudioValues {
    /// The high-pass reaches the low-pass: the bus would pass no band.
    pub fn cutoffs_cross(&self) -> bool {
        // A NaN cutoff never reaches here (every source validates finiteness),
        // but reads as crossed rather than as a usable band.
        self.high_pass_hz.partial_cmp(&self.low_pass_hz) != Some(std::cmp::Ordering::Less)
    }
}

/// One validated `setGhostAudio` call: the fields it names, each in range, and
/// the ramp length (`periodMs`, default 0: lands at once).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GhostAudioTargets {
    pub volume: Option<f32>,
    pub high_pass_hz: Option<f32>,
    pub low_pass_hz: Option<f32>,
    pub drive: Option<f32>,
    pub period_ms: f32,
}

impl GhostAudioTargets {
    pub fn names_a_field(&self) -> bool {
        self.volume.is_some()
            || self.high_pass_hz.is_some()
            || self.low_pass_hz.is_some()
            || self.drive.is_some()
    }
}

/// The level-scoped Ghost bus fields `setGhostAudio` has set, each with its
/// ramp in host script time. An unset field follows the manifest profile; a set
/// one never does again, so a committed reload that changes `audio.ghost`
/// leaves it and its ramp alone. Level unload and demotion drop it whole.
///
/// Field order is the replicated mask's: bit 0 volume, bit 1 high-pass,
/// bit 2 low-pass, bit 3 drive. Each ramp's `from` is the unclamped value at
/// the host tick the set applied, carried as is.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct GhostAudioOverride {
    pub volume: Option<GhostScalarRamp>,
    pub high_pass_hz: Option<GhostScalarRamp>,
    pub low_pass_hz: Option<GhostScalarRamp>,
    pub drive: Option<GhostScalarRamp>,
}

impl GhostAudioOverride {
    /// No script has set any field.
    pub fn is_empty(&self) -> bool {
        self.volume.is_none()
            && self.high_pass_hz.is_none()
            && self.low_pass_hz.is_none()
            && self.drive.is_none()
    }

    /// The values at script time `time`, per field: the ramp where one is set,
    /// otherwise `base`. Unclamped and unchecked for crossed cutoffs.
    pub fn resolve(&self, base: GhostAudioValues, time: f64) -> GhostAudioValues {
        let field = |ramp: Option<GhostScalarRamp>, base: f32| {
            ramp.map_or(base, |ramp| ramp.value_at(time))
        };
        GhostAudioValues {
            volume: field(self.volume, base.volume),
            high_pass_hz: field(self.high_pass_hz, base.high_pass_hz),
            low_pass_hz: field(self.low_pass_hz, base.low_pass_hz),
            drive: field(self.drive, base.drive),
        }
    }

    /// The values each field is headed for once `step` applies: the step's
    /// target where it names the field, else the ramp in flight's target, else
    /// `base`. A step is judged against these, not against the values now.
    pub fn targets_after(
        &self,
        base: GhostAudioValues,
        step: &GhostAudioTargets,
    ) -> GhostAudioValues {
        let field = |named: Option<f32>, ramp: Option<GhostScalarRamp>, base: f32| {
            named.unwrap_or_else(|| ramp.map_or(base, |ramp| ramp.to))
        };
        GhostAudioValues {
            volume: field(step.volume, self.volume, base.volume),
            high_pass_hz: field(step.high_pass_hz, self.high_pass_hz, base.high_pass_hz),
            low_pass_hz: field(step.low_pass_hz, self.low_pass_hz, base.low_pass_hz),
            drive: field(step.drive, self.drive, base.drive),
        }
    }

    /// Start a ramp on each field `step` names, from `from` (the values as they
    /// stood before the applying tick) at script time `start`. A field `step`
    /// omits keeps its value and any ramp in flight.
    pub fn set(&mut self, from: GhostAudioValues, step: &GhostAudioTargets, start: f64) {
        let ramp = |from: f32, to: f32| GhostScalarRamp {
            from,
            to,
            start_time: start,
            duration_ms: step.period_ms,
        };
        if let Some(to) = step.volume {
            self.volume = Some(ramp(from.volume, to));
        }
        if let Some(to) = step.high_pass_hz {
            self.high_pass_hz = Some(ramp(from.high_pass_hz, to));
        }
        if let Some(to) = step.low_pass_hz {
            self.low_pass_hz = Some(ramp(from.low_pass_hz, to));
        }
        if let Some(to) = step.drive {
            self.drive = Some(ramp(from.drive, to));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: GhostAudioValues = GhostAudioValues {
        volume: 0.6,
        high_pass_hz: 200.0,
        low_pass_hz: 4000.0,
        drive: 0.0,
    };

    fn step(volume: Option<f32>, low_pass_hz: Option<f32>, period_ms: f32) -> GhostAudioTargets {
        GhostAudioTargets {
            volume,
            high_pass_hz: None,
            low_pass_hz,
            drive: None,
            period_ms,
        }
    }

    // A periodMs ramp presents its midpoint halfway through and holds its
    // target after; an omitted field follows the manifest.
    #[test]
    fn a_ramp_presents_its_midpoint_halfway_and_unset_fields_follow_the_manifest() {
        let mut fields = GhostAudioOverride::default();
        fields.set(MANIFEST, &step(Some(1.0), None, 1500.0), 10.0);
        let mid = fields.resolve(MANIFEST, 10.75);
        assert!((mid.volume - 0.8).abs() < 1e-6, "{mid:?}");
        assert_eq!(
            (mid.high_pass_hz, mid.low_pass_hz, mid.drive),
            (200.0, 4000.0, 0.0)
        );
        assert_eq!(
            fields.resolve(MANIFEST, 9.0).volume,
            0.6,
            "before its start"
        );
        assert_eq!(fields.resolve(MANIFEST, 40.0).volume, 1.0, "after its end");
    }

    // A second set mid-ramp starts from the value at that moment, and a
    // field the second set omits keeps its ramp in flight.
    #[test]
    fn a_second_set_mid_ramp_starts_from_the_value_then_and_leaves_other_ramps() {
        let mut fields = GhostAudioOverride::default();
        fields.set(MANIFEST, &step(Some(1.0), Some(2000.0), 1000.0), 0.0);
        let then = fields.resolve(MANIFEST, 0.5);
        fields.set(then, &step(Some(0.0), None, 1000.0), 0.5);
        let ramp = fields.volume.expect("volume ramp");
        assert!((ramp.from - 0.8).abs() < 1e-6, "from the value at 0.5 s");
        assert_eq!(ramp.to, 0.0);
        let low = fields.low_pass_hz.expect("low-pass ramp kept");
        assert_eq!((low.from, low.to, low.start_time), (4000.0, 2000.0, 0.0));
    }

    // periodMs 0 lands at its start time.
    #[test]
    fn a_zero_period_lands_at_once() {
        let mut fields = GhostAudioOverride::default();
        fields.set(MANIFEST, &step(None, Some(1000.0), 0.0), 3.0);
        assert_eq!(fields.resolve(MANIFEST, 3.0).low_pass_hz, 1000.0);
    }

    // A step is judged against the targets after it, a ramp in flight's
    // target included.
    #[test]
    fn targets_after_a_step_read_ramps_in_flight_by_their_target() {
        let mut fields = GhostAudioOverride::default();
        let rise = GhostAudioTargets {
            high_pass_hz: Some(3000.0),
            ..step(None, None, 2000.0)
        };
        fields.set(MANIFEST, &rise, 0.0);
        let after = fields.targets_after(MANIFEST, &step(None, Some(2500.0), 0.0));
        assert_eq!((after.high_pass_hz, after.low_pass_hz), (3000.0, 2500.0));
        assert!(after.cutoffs_cross());
        assert!(!MANIFEST.cutoffs_cross());
    }
}
