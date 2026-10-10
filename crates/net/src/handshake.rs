// Immutable-admission comparison and protocol constants.
// See: context/lib/networking.md

use crate::wire::{ProtocolVersion, WireError};

pub use crate::wire::{ClosingCause, DivergenceReason, HoldingCause};

/// The application vocabulary: a new message or channel layout bumps it.
/// Weapon activation outcomes advanced it to PRL8 and observer cues to PRL9;
/// the client's revealed-level declaration and the two revealed holding
/// causes advance it to protocol 10. The id is four ASCII bytes, so the
/// counter byte continues as a hex digit: protocol 10 is "PRLA". Independent of
/// the baked PRL file format.
pub const PROTOCOL_ID: u32 = 0x_5052_4C41; // "PRLA"
/// E15's admission/parity envelopes and participation-framed traffic layouts.
/// E17 adds `blocked` to `WireKinematicMoverState`; E16 consumed epoch 16 for
/// `drop_pressed` on the Input channel and `JoinSeed` advances this to 18. The
/// dedicated E16 presentation channel and payload family advance this to 19.
/// `WireMovementState::Sliding` changes the snapshot wire layout, advancing it
/// to 20 so transport rejects pre-slide peers before snapshot decode. The
/// faction-sentiment sparse snapshot record advances it to 21.
/// Protected knockback velocity in player movement advances it to 22.
/// Hit records carrying their contact normal advance it to 23.
/// Activation input and the four-part shot identity advance it to 24.
/// The tuning-payload epoch remains independent.
/// Frozen projectile facts and reliable observer cues advance this to 25.
/// The weapon-switch declaration's client tick advances it to 26.
pub const WIRE_VERSION: u32 = 26;

#[must_use]
pub const fn transport_protocol_id() -> u64 {
    ((PROTOCOL_ID as u64) << 32) | (WIRE_VERSION as u64)
}

#[must_use]
pub const fn protocol_version() -> ProtocolVersion {
    ProtocolVersion {
        app_protocol_id: PROTOCOL_ID,
        wire_version: WIRE_VERSION,
    }
}

/// Gate only immutable build constants. Mutable content belongs to parity and is
/// deliberately never compared here.
pub fn validate_handshake(
    expected: ProtocolVersion,
    received: ProtocolVersion,
) -> Result<(), ClosingCause> {
    if expected == received {
        Ok(())
    } else {
        Err(ClosingCause::Protocol { expected, received })
    }
}

/// Decode failures have no authentic peer value; use an impossible all-zero
/// protocol for the terminal diagnostic.
#[must_use]
pub fn malformed_version(_err: &WireError) -> ProtocolVersion {
    ProtocolVersion {
        app_protocol_id: 0,
        wire_version: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcode::{Decode, Encode};

    /// The five shipped variants before [`ServerControlMessage::SessionRoster`]
    /// was appended. Keeping this local historical mirror lets the test measure
    /// bitcode's enum-tag layout rather than assuming that a sixth variant leaves
    /// it unchanged.
    #[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
    enum PreRosterServerControlMessage {
        Divergence(DivergenceReason),
        Tuning(Vec<u8>),
        Relevel(String),
        SwitchRefused(crate::wire::ServerSwitchRefused),
        SwitchAccepted(crate::wire::ServerSwitchAccepted),
    }

    /// Historical client Control layout before `JoinSeed` was appended. This
    /// measures positional bitcode tags directly, guarding the shipped
    /// admission/parity/switch discriminants against an accidental insertion.
    #[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
    enum PreJoinSeedClientControlMessage {
        Admission {
            protocol: ProtocolVersion,
            mod_id: String,
            mod_version: String,
        },
        Parity(crate::wire::ParityDeclaration),
        SwitchDeclaration(crate::wire::ClientSwitchDeclaration),
    }

    /// Historical client Control layout before `Revealed` was appended.
    #[derive(Debug, Clone, PartialEq, Encode, Decode)]
    enum PreRevealedClientControlMessage {
        Admission {
            protocol: ProtocolVersion,
            mod_id: String,
            mod_version: String,
        },
        Parity(crate::wire::ParityDeclaration),
        SwitchDeclaration(crate::wire::ClientSwitchDeclaration),
        JoinSeed {
            slots: std::collections::BTreeMap<String, crate::wire::JoinSeedValue>,
        },
    }

    /// Historical holding causes before the two revealed causes were appended.
    #[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
    enum PreRevealedHoldingCause {
        ModDigest {
            expected: [u8; 32],
            received: [u8; 32],
        },
        HostLevelAbsent,
        LevelAbsent {
            expected_identity: String,
        },
        LevelIdentity {
            expected: String,
            received: String,
        },
        LevelDigest {
            identity: String,
            expected: [u8; 32],
            received: [u8; 32],
        },
    }

    #[test]
    fn validate_handshake_accepts_only_matching_protocol_constants() {
        let version = protocol_version();
        assert_eq!(validate_handshake(version, version), Ok(()));
    }

    // A hit declaration's contacts carry their normals; a peer that sends the
    // prior record layout is refused before any bitcode decode.
    #[test]
    fn hit_record_normals_refuse_the_previous_wire_version() {
        const PRE_CONTACT_NORMAL_WIRE_VERSION: u32 = 22;
        const {
            assert!(
                WIRE_VERSION > PRE_CONTACT_NORMAL_WIRE_VERSION,
                "hit records gained `normal`"
            );
        };
        assert_ne!(
            transport_protocol_id(),
            ((PROTOCOL_ID as u64) << 32) | u64::from(PRE_CONTACT_NORMAL_WIRE_VERSION),
        );
    }

    #[test]
    fn knockback_snapshot_layout_refuses_previous_wire_version() {
        const PRE_KNOCKBACK_WIRE_VERSION: u32 = 21;
        assert_eq!(
            PROTOCOL_ID, 0x_5052_4C41,
            "revealed vocabulary requires application protocol PRLA (protocol 10)"
        );
        const {
            assert!(
                WIRE_VERSION > PRE_KNOCKBACK_WIRE_VERSION,
                "protected knockback velocity changes the snapshot layout"
            );
        };
        assert_ne!(
            transport_protocol_id(),
            ((PROTOCOL_ID as u64) << 32) | u64::from(PRE_KNOCKBACK_WIRE_VERSION),
            "gate 1 rejects the prior snapshot layout before app decode"
        );
        let previous = ProtocolVersion {
            app_protocol_id: PROTOCOL_ID,
            wire_version: PRE_KNOCKBACK_WIRE_VERSION,
        };
        assert!(matches!(
            validate_handshake(protocol_version(), previous),
            Err(ClosingCause::Protocol { .. })
        ));
    }

    #[test]
    fn session_roster_append_preserves_shipped_control_encodings() {
        use crate::wire::{ServerControlMessage, ServerSwitchAccepted, ServerSwitchRefused};

        let divergence = DivergenceReason::Closing(ClosingCause::Protocol {
            expected: ProtocolVersion {
                app_protocol_id: 0x5052_4c35,
                wire_version: 15,
            },
            received: ProtocolVersion {
                app_protocol_id: 0x5052_4c34,
                wire_version: 14,
            },
        });
        let cases = [
            (
                PreRosterServerControlMessage::Divergence(divergence.clone()),
                ServerControlMessage::Divergence(divergence),
            ),
            (
                PreRosterServerControlMessage::Tuning(vec![0, 1, 2, 3]),
                ServerControlMessage::Tuning(vec![0, 1, 2, 3]),
            ),
            (
                PreRosterServerControlMessage::Relevel("campaign-test".to_owned()),
                ServerControlMessage::Relevel("campaign-test".to_owned()),
            ),
            (
                PreRosterServerControlMessage::SwitchRefused(ServerSwitchRefused {
                    declaration_id: 17,
                    slot: 3,
                }),
                ServerControlMessage::SwitchRefused(ServerSwitchRefused {
                    declaration_id: 17,
                    slot: 3,
                }),
            ),
            (
                PreRosterServerControlMessage::SwitchAccepted(ServerSwitchAccepted {
                    declaration_id: 18,
                    slot: 4,
                }),
                ServerControlMessage::SwitchAccepted(ServerSwitchAccepted {
                    declaration_id: 18,
                    slot: 4,
                }),
            ),
        ];

        for (before, after) in cases {
            assert_eq!(
                bitcode::encode(&before),
                crate::wire::encode(&after),
                "appending SessionRoster changed a shipped control encoding; bump WIRE_VERSION"
            );
        }
    }

    #[test]
    fn join_seed_append_preserves_shipped_client_control_encodings() {
        use crate::wire::{ClientControlMessage, ClientSwitchDeclaration, ParityDeclaration};

        let protocol = ProtocolVersion {
            app_protocol_id: 0x5052_4c36,
            wire_version: 17,
        };
        let parity = ParityDeclaration {
            mod_digest: [0x17; 32],
            level: Some(("campaign-test".to_owned(), [0x18; 32])),
        };
        let switch = ClientSwitchDeclaration {
            declaration_id: 17,
            slot: 3,
            client_tick: 17,
        };
        let cases = [
            (
                PreJoinSeedClientControlMessage::Admission {
                    protocol,
                    mod_id: "postretro.test".to_owned(),
                    mod_version: "1.0.0".to_owned(),
                },
                ClientControlMessage::Admission {
                    protocol,
                    mod_id: "postretro.test".to_owned(),
                    mod_version: "1.0.0".to_owned(),
                },
            ),
            (
                PreJoinSeedClientControlMessage::Parity(parity.clone()),
                ClientControlMessage::Parity(parity),
            ),
            (
                PreJoinSeedClientControlMessage::SwitchDeclaration(switch),
                ClientControlMessage::SwitchDeclaration(switch),
            ),
        ];

        for (before, after) in cases {
            assert_eq!(
                bitcode::encode(&before),
                crate::wire::encode(&after),
                "appending JoinSeed changed a shipped client control encoding; bump WIRE_VERSION"
            );
        }
    }

    #[test]
    fn revealed_append_preserves_shipped_client_control_encodings() {
        use crate::wire::{
            ClientControlMessage, ClientSwitchDeclaration, JoinSeedValue, ParityDeclaration,
        };
        use std::collections::BTreeMap;

        let protocol = ProtocolVersion {
            app_protocol_id: 0x5052_4c39,
            wire_version: 25,
        };
        let parity = ParityDeclaration {
            mod_digest: [0x25; 32],
            level: Some(("campaign-test".to_owned(), [0x26; 32])),
        };
        let switch = ClientSwitchDeclaration {
            declaration_id: 25,
            slot: 2,
            client_tick: 26,
        };
        let slots = BTreeMap::from([("kplayer0000000001".to_owned(), JoinSeedValue::Number(3.0))]);
        let cases = [
            (
                PreRevealedClientControlMessage::Admission {
                    protocol,
                    mod_id: "postretro.test".to_owned(),
                    mod_version: "1.0.0".to_owned(),
                },
                ClientControlMessage::Admission {
                    protocol,
                    mod_id: "postretro.test".to_owned(),
                    mod_version: "1.0.0".to_owned(),
                },
            ),
            (
                PreRevealedClientControlMessage::Parity(parity.clone()),
                ClientControlMessage::Parity(parity),
            ),
            (
                PreRevealedClientControlMessage::SwitchDeclaration(switch),
                ClientControlMessage::SwitchDeclaration(switch),
            ),
            (
                PreRevealedClientControlMessage::JoinSeed {
                    slots: slots.clone(),
                },
                ClientControlMessage::JoinSeed { slots },
            ),
        ];

        for (before, after) in cases {
            assert_eq!(
                bitcode::encode(&before),
                crate::wire::encode(&after),
                "appending Revealed changed a shipped client control encoding; bump WIRE_VERSION"
            );
        }
    }

    #[test]
    fn revealed_causes_append_preserves_shipped_holding_cause_encodings() {
        let cases = [
            (
                PreRevealedHoldingCause::ModDigest {
                    expected: [1; 32],
                    received: [2; 32],
                },
                HoldingCause::ModDigest {
                    expected: [1; 32],
                    received: [2; 32],
                },
            ),
            (
                PreRevealedHoldingCause::HostLevelAbsent,
                HoldingCause::HostLevelAbsent,
            ),
            (
                PreRevealedHoldingCause::LevelAbsent {
                    expected_identity: "e1m1".to_owned(),
                },
                HoldingCause::LevelAbsent {
                    expected_identity: "e1m1".to_owned(),
                },
            ),
            (
                PreRevealedHoldingCause::LevelIdentity {
                    expected: "e1m1".to_owned(),
                    received: "e1m2".to_owned(),
                },
                HoldingCause::LevelIdentity {
                    expected: "e1m1".to_owned(),
                    received: "e1m2".to_owned(),
                },
            ),
            (
                PreRevealedHoldingCause::LevelDigest {
                    identity: "e1m1".to_owned(),
                    expected: [3; 32],
                    received: [4; 32],
                },
                HoldingCause::LevelDigest {
                    identity: "e1m1".to_owned(),
                    expected: [3; 32],
                    received: [4; 32],
                },
            ),
        ];

        for (before, after) in cases {
            assert_eq!(
                bitcode::encode(&before),
                crate::wire::encode(&after),
                "appending revealed holding causes changed a shipped encoding; bump WIRE_VERSION"
            );
        }
    }

    // A previous-build peer cannot represent the revealed declaration or the
    // revealed holding causes, so gate 1 must refuse it before any decode.
    #[test]
    fn revealed_vocabulary_refuses_previous_protocol_id() {
        const PRE_REVEALED_PROTOCOL_ID: u32 = 0x_5052_4C39; // "PRL9"
        const {
            assert!(
                PROTOCOL_ID != PRE_REVEALED_PROTOCOL_ID,
                "the revealed vocabulary bumps the application protocol"
            );
        };
        assert_ne!(
            transport_protocol_id(),
            ((PRE_REVEALED_PROTOCOL_ID as u64) << 32) | u64::from(WIRE_VERSION),
            "gate 1 refuses a peer that predates the revealed vocabulary"
        );
        let previous = ProtocolVersion {
            app_protocol_id: PRE_REVEALED_PROTOCOL_ID,
            wire_version: WIRE_VERSION,
        };
        assert!(matches!(
            validate_handshake(protocol_version(), previous),
            Err(ClosingCause::Protocol { .. })
        ));
    }

    #[test]
    fn roster_entry_fields_stay_claim_free() {
        let entry = crate::wire::RosterEntry {
            seat: 4,
            connected: true,
        };

        // Exhaustive destructuring is the privacy drift guard for AC-ROSTER-2.
        // Any future field must be explicitly classified before it can cross
        // the roster boundary.
        let crate::wire::RosterEntry { seat, connected } = entry;
        let _host_minted_or_observed = (seat, connected);
    }
}

#[cfg(test)]
mod activation_epoch_tests {
    use super::*;
    use bitcode::{Decode, Encode};

    #[derive(Clone, Encode, Decode)]
    enum PreCueServerMessage {
        TimeSync(crate::timesync::TimeSyncEcho),
        ShotVerdicts(crate::wire::ShotVerdictsMessage),
        ActivationOutcomes(Vec<crate::wire::ActivationOutcome>),
    }

    #[test]
    fn observer_cue_append_preserves_shipped_server_message_tags() {
        use crate::wire::{ServerMessage, ShotVerdictsMessage};
        let echo = crate::timesync::TimeSyncEcho {
            sample_id: 7,
            client_send_tick: 8,
            client_send_time_us: 9,
            server_tick: 10,
            server_echo_time_us: 11,
        };
        let verdicts = ShotVerdictsMessage {
            verdicts: Vec::new(),
        };
        let cases = [
            (
                PreCueServerMessage::TimeSync(echo),
                ServerMessage::TimeSync(echo),
            ),
            (
                PreCueServerMessage::ShotVerdicts(verdicts.clone()),
                ServerMessage::ShotVerdicts(verdicts),
            ),
            (
                PreCueServerMessage::ActivationOutcomes(Vec::new()),
                ServerMessage::ActivationOutcomes(Vec::new()),
            ),
        ];
        for (historical, current) in cases {
            assert_eq!(
                crate::wire::encode(&historical),
                crate::wire::encode(&current),
                "append must preserve existing field/tag order"
            );
        }
    }

    #[test]
    fn observer_cues_and_projectile_facts_refuse_previous_epochs() {
        for received in [
            ProtocolVersion {
                app_protocol_id: PROTOCOL_ID,
                wire_version: 24,
            },
            ProtocolVersion {
                app_protocol_id: 0x5052_4c38,
                wire_version: WIRE_VERSION,
            },
        ] {
            assert!(matches!(
                validate_handshake(protocol_version(), received),
                Err(ClosingCause::Protocol { .. })
            ));
        }
    }

    #[test]
    fn activation_records_and_outcomes_reject_previous_layout_and_vocabulary() {
        assert_eq!(WIRE_VERSION, 26);
        assert_eq!(PROTOCOL_ID, 0x5052_4c41);
        for received in [
            ProtocolVersion {
                app_protocol_id: PROTOCOL_ID,
                wire_version: 23,
            },
            ProtocolVersion {
                app_protocol_id: 0x5052_4c37,
                wire_version: WIRE_VERSION,
            },
        ] {
            assert!(matches!(
                validate_handshake(protocol_version(), received),
                Err(ClosingCause::Protocol { .. })
            ));
        }
    }
}

#[cfg(test)]
mod switch_tick_epoch_tests {
    use super::*;
    use bitcode::{Decode, Encode};

    /// The switch declaration as shipped at wire 25, before its client tick.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
    struct PreTickSwitchDeclaration {
        declaration_id: u32,
        slot: u8,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
    enum PreTickClientControlMessage {
        Admission {
            protocol: ProtocolVersion,
            mod_id: String,
            mod_version: String,
        },
        Parity(crate::wire::ParityDeclaration),
        SwitchDeclaration(PreTickSwitchDeclaration),
    }

    #[test]
    fn switch_tick_advances_only_the_wire_version_and_refuses_wire_25_peers() {
        assert_eq!(WIRE_VERSION, 26);
        assert_eq!(PROTOCOL_ID, 0x5052_4c41, "the vocabulary is unchanged");
        assert_eq!(
            crate::wire::SNAPSHOT_VERSION,
            17,
            "no snapshot record changed"
        );
        assert!(matches!(
            validate_handshake(
                protocol_version(),
                ProtocolVersion {
                    app_protocol_id: PROTOCOL_ID,
                    wire_version: 25,
                },
            ),
            Err(ClosingCause::Protocol { .. })
        ));
    }

    #[test]
    fn switch_tick_changes_the_declaration_layout() {
        use crate::wire::{ClientControlMessage, ClientSwitchDeclaration};
        let before = bitcode::encode(&PreTickClientControlMessage::SwitchDeclaration(
            PreTickSwitchDeclaration {
                declaration_id: 9,
                slot: 2,
            },
        ));
        let after = crate::wire::encode(&ClientControlMessage::SwitchDeclaration(
            ClientSwitchDeclaration {
                declaration_id: 9,
                slot: 2,
                client_tick: 0x0102_0304,
            },
        ));
        assert_ne!(before, after, "the added tick changes the layout");
        assert!(
            crate::wire::decode::<ClientControlMessage>(&before).is_err(),
            "a wire-25 declaration never decodes as a tick-stamped one"
        );
    }
}
