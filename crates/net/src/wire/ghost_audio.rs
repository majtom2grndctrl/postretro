// Level-scoped ghost-audio record: the Ghost bus fields `setGhostAudio` has set, with their ramps.
// See: context/lib/networking.md §Current contract

use bitcode::{Decode, Encode};

use super::WireGhostScalarRamp;

/// Ghost bus field mask bits: which `setGhostAudio` fields a script has set.
pub const GHOST_AUDIO_VOLUME: u8 = 1 << 0;
pub const GHOST_AUDIO_HIGH_PASS: u8 = 1 << 1;
pub const GHOST_AUDIO_LOW_PASS: u8 = 1 << 2;
pub const GHOST_AUDIO_DRIVE: u8 = 1 << 3;
pub const GHOST_AUDIO_FIELD_BITS: u8 =
    GHOST_AUDIO_VOLUME | GHOST_AUDIO_HIGH_PASS | GHOST_AUDIO_LOW_PASS | GHOST_AUDIO_DRIVE;

/// Why a ghost-audio record failed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhostAudioFault {
    /// A field-mask bit past bit 3.
    ReservedFieldBits,
    /// A mask bit without its ramp, or a ramp whose bit is clear.
    FieldMismatch,
    /// A non-finite ramp value or start time.
    NonFiniteRamp,
    /// A negative or non-finite ramp duration.
    InvalidDuration,
}

/// The host's Ghost bus script fields, one record per snapshot envelope.
///
/// Level-scoped, so it binds to no entity: the snapshot it rides names its
/// level through the participation epoch the transport already gates on, and
/// its freshness is that snapshot's sequence. Every snapshot carries the whole
/// set, so a late joiner's first snapshot is its baseline and an empty mask
/// means the host has set no field. Values are the host's unclamped ramps on
/// its script clock; the client rebases each start time.
///
/// `fields` is the literal `u8` mask the wire format names, beside `Option`
/// slots, so a reserved bit or a mask/slot disagreement stays representable
/// and validation (not bitcode) rejects it.
#[derive(Debug, Clone, Copy, PartialEq, Default, Encode, Decode)]
pub struct WireGhostAudioState {
    pub fields: u8,
    pub volume: Option<WireGhostScalarRamp>,
    pub high_pass_hz: Option<WireGhostScalarRamp>,
    pub low_pass_hz: Option<WireGhostScalarRamp>,
    pub drive: Option<WireGhostScalarRamp>,
}

impl WireGhostAudioState {
    /// Each field in mask-bit order, with its bit.
    #[must_use]
    pub fn slots(&self) -> [(u8, Option<WireGhostScalarRamp>); 4] {
        [
            (GHOST_AUDIO_VOLUME, self.volume),
            (GHOST_AUDIO_HIGH_PASS, self.high_pass_hz),
            (GHOST_AUDIO_LOW_PASS, self.low_pass_hz),
            (GHOST_AUDIO_DRIVE, self.drive),
        ]
    }

    /// Reject what the wire format forbids. Cutoff ranges and crossing are the
    /// receiving peer's to resolve against its own manifest profile.
    pub fn validate(&self) -> Result<(), GhostAudioFault> {
        if self.fields & !GHOST_AUDIO_FIELD_BITS != 0 {
            return Err(GhostAudioFault::ReservedFieldBits);
        }
        let slots = self.slots();
        if slots
            .iter()
            .any(|(bit, ramp)| (self.fields & bit != 0) != ramp.is_some())
        {
            return Err(GhostAudioFault::FieldMismatch);
        }
        for ramp in slots.into_iter().filter_map(|(_, ramp)| ramp) {
            if !ramp.from.is_finite() || !ramp.to.is_finite() || !ramp.start_time.is_finite() {
                return Err(GhostAudioFault::NonFiniteRamp);
            }
            if !ramp.duration_ms.is_finite() || ramp.duration_ms < 0.0 {
                return Err(GhostAudioFault::InvalidDuration);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(from: f32, to: f32) -> WireGhostScalarRamp {
        WireGhostScalarRamp {
            from,
            to,
            start_time: 8.25,
            duration_ms: 1500.0,
        }
    }

    fn full() -> WireGhostAudioState {
        WireGhostAudioState {
            fields: GHOST_AUDIO_FIELD_BITS,
            volume: Some(ramp(0.6, 0.9)),
            high_pass_hz: Some(ramp(200.0, 250.0)),
            low_pass_hz: Some(ramp(4000.0, 5000.0)),
            drive: Some(WireGhostScalarRamp {
                duration_ms: 0.0,
                ..ramp(0.0, 0.3)
            }),
        }
    }

    // The record round-trips empty, partial and full: only the set fields ride.
    #[test]
    fn ghost_audio_record_round_trips_every_field_mask() {
        let partial = WireGhostAudioState {
            fields: GHOST_AUDIO_VOLUME | GHOST_AUDIO_LOW_PASS,
            volume: Some(ramp(0.6, 0.9)),
            low_pass_hz: Some(ramp(4000.0, 5000.0)),
            ..WireGhostAudioState::default()
        };
        for state in [WireGhostAudioState::default(), partial, full()] {
            assert_eq!(state.validate(), Ok(()));
            let decoded: WireGhostAudioState =
                crate::wire::decode(&crate::wire::encode(&state)).unwrap();
            assert_eq!(decoded, state);
        }
    }

    // AC4 (audio half): each case the wire format names is a rejection, and
    // survives the codec to be rejected.
    #[test]
    fn ghost_audio_record_rejects_each_case_the_wire_format_names() {
        let base = full();
        let with = |slot: fn(&mut WireGhostAudioState) -> &mut Option<WireGhostScalarRamp>,
                    value: Option<WireGhostScalarRamp>| {
            let mut state = base;
            *slot(&mut state) = value;
            state
        };
        let cases: Vec<(&str, WireGhostAudioState, GhostAudioFault)> = vec![
            (
                "field bit 4",
                WireGhostAudioState {
                    fields: GHOST_AUDIO_FIELD_BITS | 1 << 4,
                    ..base
                },
                GhostAudioFault::ReservedFieldBits,
            ),
            (
                "field bit 7 alone",
                WireGhostAudioState {
                    fields: 1 << 7,
                    ..WireGhostAudioState::default()
                },
                GhostAudioFault::ReservedFieldBits,
            ),
            (
                "mask bit without ramp",
                with(|state| &mut state.high_pass_hz, None),
                GhostAudioFault::FieldMismatch,
            ),
            (
                "ramp without mask bit",
                WireGhostAudioState {
                    fields: GHOST_AUDIO_FIELD_BITS & !GHOST_AUDIO_DRIVE,
                    ..base
                },
                GhostAudioFault::FieldMismatch,
            ),
            (
                "NaN volume target",
                with(|state| &mut state.volume, Some(ramp(0.6, f32::NAN))),
                GhostAudioFault::NonFiniteRamp,
            ),
            (
                "infinite low-pass from-value",
                with(
                    |state| &mut state.low_pass_hz,
                    Some(ramp(f32::INFINITY, 4000.0)),
                ),
                GhostAudioFault::NonFiniteRamp,
            ),
            (
                "NaN start time",
                with(
                    |state| &mut state.drive,
                    Some(WireGhostScalarRamp {
                        start_time: f64::NAN,
                        ..ramp(0.0, 0.3)
                    }),
                ),
                GhostAudioFault::NonFiniteRamp,
            ),
            (
                "negative duration",
                with(
                    |state| &mut state.high_pass_hz,
                    Some(WireGhostScalarRamp {
                        duration_ms: -1.0,
                        ..ramp(200.0, 250.0)
                    }),
                ),
                GhostAudioFault::InvalidDuration,
            ),
            (
                "NaN duration",
                with(
                    |state| &mut state.volume,
                    Some(WireGhostScalarRamp {
                        duration_ms: f32::NAN,
                        ..ramp(0.6, 0.9)
                    }),
                ),
                GhostAudioFault::InvalidDuration,
            ),
            (
                "infinite duration",
                with(
                    |state| &mut state.low_pass_hz,
                    Some(WireGhostScalarRamp {
                        duration_ms: f32::INFINITY,
                        ..ramp(4000.0, 5000.0)
                    }),
                ),
                GhostAudioFault::InvalidDuration,
            ),
        ];
        for (name, state, fault) in cases {
            assert_eq!(state.validate(), Err(fault), "{name}");
            let decoded: WireGhostAudioState =
                crate::wire::decode(&crate::wire::encode(&state)).unwrap();
            assert_eq!(
                decoded.validate(),
                Err(fault),
                "{name} survives the codec to be rejected"
            );
        }
    }
}
