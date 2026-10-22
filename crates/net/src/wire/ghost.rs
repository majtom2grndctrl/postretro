// Ghost-link state record: armed state, member categories and script-set look ramps.
// See: context/lib/networking.md §Current contract

use bitcode::{Decode, Encode};

/// The no-link index: never a map-order link index on the wire.
pub const NO_WIRE_GHOST_LINK: u16 = u16::MAX;

/// Member-category bits in the PRL id-56 order: players, entities, projectiles,
/// movers. Every other bit is reserved.
pub const GHOST_LINK_MEMBER_BITS: u8 = 0b1111;

/// Look field mask bits: which `setColorFilter` fields a script has set.
pub const GHOST_LOOK_TINT: u8 = 1 << 0;
pub const GHOST_LOOK_OPACITY: u8 = 1 << 1;
pub const GHOST_LOOK_SATURATION: u8 = 1 << 2;
pub const GHOST_LOOK_FIELD_BITS: u8 = GHOST_LOOK_TINT | GHOST_LOOK_OPACITY | GHOST_LOOK_SATURATION;

/// One scalar look field's ramp. `from` is the unclamped value at the applying
/// host tick, `start_time` the host script time it starts at, in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WireGhostScalarRamp {
    pub from: f32,
    pub to: f32,
    pub start_time: f64,
    pub duration_ms: f32,
}

/// The tint field's ramp: an `[r, g, b]` color, otherwise as [`WireGhostScalarRamp`].
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WireGhostTintRamp {
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub start_time: f64,
    pub duration_ms: f32,
}

/// Why a ghost-link record failed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhostLinkFault {
    /// The index is the no-link sentinel.
    NoLinkIndex,
    /// A member-category bit past bit 3.
    ReservedMemberBits,
    /// A look-mask bit past bit 2.
    ReservedLookBits,
    /// A look-mask bit without its ramp, or a ramp whose bit is clear.
    LookFieldMismatch,
    /// A non-finite ramp value or start time.
    NonFiniteRamp,
    /// A negative or non-finite ramp duration.
    InvalidDuration,
}

/// One link's replicated state, bound on the client by map-order index and
/// source entity like [`super::WireTeleporterState`].
///
/// `look_fields` is the literal `u8` mask the wire format names; each set bit
/// carries its ramp in the matching slot. The explicit mask beside `Option`
/// slots keeps a reserved bit or a mask/slot disagreement representable, so
/// validation (not bitcode) rejects it.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WireGhostLinkState {
    pub index: u16,
    pub source_entity: u32,
    pub armed: bool,
    /// Member categories in the PRL bit order ([`GHOST_LINK_MEMBER_BITS`]).
    pub members: u8,
    pub look_fields: u8,
    pub tint: Option<WireGhostTintRamp>,
    pub opacity: Option<WireGhostScalarRamp>,
    pub saturation: Option<WireGhostScalarRamp>,
}

impl WireGhostLinkState {
    /// A record carrying no script-set look field.
    #[must_use]
    pub fn new(index: u16, source_entity: u32, armed: bool, members: u8) -> Self {
        Self {
            index,
            source_entity,
            armed,
            members,
            look_fields: 0,
            tint: None,
            opacity: None,
            saturation: None,
        }
    }

    /// Reject what the wire format forbids. Whether the index names an
    /// installed link is engine-side binding.
    pub fn validate(&self) -> Result<(), GhostLinkFault> {
        if self.index == NO_WIRE_GHOST_LINK {
            return Err(GhostLinkFault::NoLinkIndex);
        }
        if self.members & !GHOST_LINK_MEMBER_BITS != 0 {
            return Err(GhostLinkFault::ReservedMemberBits);
        }
        if self.look_fields & !GHOST_LOOK_FIELD_BITS != 0 {
            return Err(GhostLinkFault::ReservedLookBits);
        }
        let carried = [
            (GHOST_LOOK_TINT, self.tint.is_some()),
            (GHOST_LOOK_OPACITY, self.opacity.is_some()),
            (GHOST_LOOK_SATURATION, self.saturation.is_some()),
        ];
        if carried
            .iter()
            .any(|(bit, present)| (self.look_fields & bit != 0) != *present)
        {
            return Err(GhostLinkFault::LookFieldMismatch);
        }
        let mut ramps = [None; 3];
        if let Some(tint) = self.tint {
            let values = tint.from.iter().chain(&tint.to);
            if !values.copied().all(f32::is_finite) {
                return Err(GhostLinkFault::NonFiniteRamp);
            }
            ramps[0] = Some((tint.start_time, tint.duration_ms));
        }
        for (slot, ramp) in [self.opacity, self.saturation].into_iter().enumerate() {
            if let Some(ramp) = ramp {
                if !ramp.from.is_finite() || !ramp.to.is_finite() {
                    return Err(GhostLinkFault::NonFiniteRamp);
                }
                ramps[slot + 1] = Some((ramp.start_time, ramp.duration_ms));
            }
        }
        for (start_time, duration_ms) in ramps.into_iter().flatten() {
            if !start_time.is_finite() {
                return Err(GhostLinkFault::NonFiniteRamp);
            }
            if !duration_ms.is_finite() || duration_ms < 0.0 {
                return Err(GhostLinkFault::InvalidDuration);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scalar(from: f32, to: f32) -> WireGhostScalarRamp {
        WireGhostScalarRamp {
            from,
            to,
            start_time: 12.5,
            duration_ms: 1500.0,
        }
    }

    fn full() -> WireGhostLinkState {
        WireGhostLinkState {
            look_fields: GHOST_LOOK_FIELD_BITS,
            tint: Some(WireGhostTintRamp {
                from: [1.0, 1.0, 1.0],
                to: [1.0, 0.4, 0.3],
                start_time: 3.25,
                duration_ms: 0.0,
            }),
            opacity: Some(scalar(0.5, 0.6)),
            saturation: Some(scalar(0.0, 0.2)),
            ..WireGhostLinkState::new(2, 41, true, GHOST_LINK_MEMBER_BITS)
        }
    }

    // The whole record shape round-trips, look ramps included, so the later
    // look fields need no further layout change.
    #[test]
    fn ghost_link_record_round_trips_state_and_every_look_ramp() {
        for state in [
            WireGhostLinkState::new(0, 7, false, 0),
            WireGhostLinkState::new(u16::MAX - 1, u32::MAX, true, 0b0101),
            full(),
        ] {
            assert_eq!(state.validate(), Ok(()));
            let decoded: WireGhostLinkState =
                crate::wire::decode(&crate::wire::encode(&state)).unwrap();
            assert_eq!(decoded, state);
        }
    }

    // AC4 (snapshot half): each case the wire format names is a rejection.
    #[test]
    fn ghost_link_record_rejects_each_case_the_wire_format_names() {
        let base = full();
        let cases: Vec<(&str, WireGhostLinkState, GhostLinkFault)> = vec![
            (
                "no-link index",
                WireGhostLinkState {
                    index: NO_WIRE_GHOST_LINK,
                    ..base
                },
                GhostLinkFault::NoLinkIndex,
            ),
            (
                "member bit 4",
                WireGhostLinkState {
                    members: 0b1_0000,
                    ..base
                },
                GhostLinkFault::ReservedMemberBits,
            ),
            (
                "member bit 7",
                WireGhostLinkState {
                    members: 0b1000_0001,
                    ..base
                },
                GhostLinkFault::ReservedMemberBits,
            ),
            (
                "look bit 3",
                WireGhostLinkState {
                    look_fields: GHOST_LOOK_FIELD_BITS | 1 << 3,
                    ..base
                },
                GhostLinkFault::ReservedLookBits,
            ),
            (
                "mask bit without ramp",
                WireGhostLinkState {
                    opacity: None,
                    ..base
                },
                GhostLinkFault::LookFieldMismatch,
            ),
            (
                "ramp without mask bit",
                WireGhostLinkState {
                    look_fields: GHOST_LOOK_TINT | GHOST_LOOK_OPACITY,
                    ..base
                },
                GhostLinkFault::LookFieldMismatch,
            ),
            (
                "NaN opacity target",
                WireGhostLinkState {
                    opacity: Some(scalar(0.5, f32::NAN)),
                    ..base
                },
                GhostLinkFault::NonFiniteRamp,
            ),
            (
                "infinite saturation start value",
                WireGhostLinkState {
                    saturation: Some(scalar(f32::INFINITY, 0.0)),
                    ..base
                },
                GhostLinkFault::NonFiniteRamp,
            ),
            (
                "NaN tint channel",
                WireGhostLinkState {
                    tint: base.tint.map(|tint| WireGhostTintRamp {
                        to: [0.0, f32::NAN, 0.0],
                        ..tint
                    }),
                    ..base
                },
                GhostLinkFault::NonFiniteRamp,
            ),
            (
                "infinite start time",
                WireGhostLinkState {
                    opacity: Some(WireGhostScalarRamp {
                        start_time: f64::INFINITY,
                        ..scalar(0.0, 1.0)
                    }),
                    ..base
                },
                GhostLinkFault::NonFiniteRamp,
            ),
            (
                "negative duration",
                WireGhostLinkState {
                    saturation: Some(WireGhostScalarRamp {
                        duration_ms: -1.0,
                        ..scalar(0.0, 1.0)
                    }),
                    ..base
                },
                GhostLinkFault::InvalidDuration,
            ),
            (
                "NaN duration",
                WireGhostLinkState {
                    tint: base.tint.map(|tint| WireGhostTintRamp {
                        duration_ms: f32::NAN,
                        ..tint
                    }),
                    ..base
                },
                GhostLinkFault::InvalidDuration,
            ),
            (
                "infinite duration",
                WireGhostLinkState {
                    opacity: Some(WireGhostScalarRamp {
                        duration_ms: f32::INFINITY,
                        ..scalar(0.0, 1.0)
                    }),
                    ..base
                },
                GhostLinkFault::InvalidDuration,
            ),
        ];
        for (name, state, fault) in cases {
            assert_eq!(state.validate(), Err(fault), "{name}");
            let decoded: WireGhostLinkState =
                crate::wire::decode(&crate::wire::encode(&state)).unwrap();
            assert_eq!(
                decoded.validate(),
                Err(fault),
                "{name} survives the codec to be rejected"
            );
        }
    }
}
