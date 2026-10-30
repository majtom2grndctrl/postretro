// Immutable-admission comparison and protocol constants.
// See: context/lib/networking.md

use crate::wire::{ProtocolVersion, WireError};

pub use crate::wire::{ClosingCause, DivergenceReason, HoldingCause};

/// The application vocabulary: a new message or channel layout bumps it.
/// Weapon activation outcomes advanced it to PRL8 and observer cues to PRL9;
/// the client's revealed-level declaration and the two revealed holding
/// causes advance it to protocol 10. The id is four ASCII bytes, so the
/// counter byte continues as a hex digit: protocol 10 is "PRLA". Player-addressed
/// presentation commands advance it to protocol 11, "PRLB". The gameplay
/// channel layout, with weapon switches moving from Control to Input, advances
/// it to protocol 12, "PRLC". Independent of the baked PRL file format.
pub const PROTOCOL_ID: u32 = 0x_5052_4C43; // "PRLC"
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
/// Shared enemy overlay rosters and participation-framed Presentation advance it to 28.
/// The protected push-drift layer in player movement state advances it to 29;
/// `WireMovementState::Swim` and the swim re-entry latch share that bump.
/// Removal of the particle component renumbers later component kinds at 30.
/// `WireMovementState::Vault` advances it to 31.
/// Teleporters advance it to 32: the pawn teleport record in player movement
/// state, the replicated teleporter armed state, and the input command's
/// teleport yaw stamp share the bump.
/// Moving weapon switches from Control to Input advances it to 33: removing the
/// switch variants from the middle of the Control enums renumbers the later
/// positional tags (`ClientControlMessage` `JoinSeed` 3 to 2, `Revealed` 4 to 3;
/// `ServerControlMessage` `SessionRoster` 5 to 3). Input's sub-tick resend is a
/// sender-side setting and does not affect wire compatibility.
/// Ghost links advance it to 34: component payloads gained the link-state record
/// and the snapshot envelope the level-scoped ghost-audio record (`SNAPSHOT_VERSION` 22).
/// Projectile traps reserve component discriminant 25 but never replicate, so
/// they need no bump of their own; the ghost-link record takes discriminant 26.
/// A declaration kind separates a timed switch from an immediate active-slot resync.
/// The snapshot envelope's host level tick advances it to 36 (`SNAPSHOT_VERSION` 23).
/// `Relevel`'s host level life advances it to 37.
pub const WIRE_VERSION: u32 = 37;

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

    /// The server Control variants before [`ServerControlMessage::SessionRoster`],
    /// in the current layout (the switch outcomes left Control at wire 33).
    /// Keeping this local mirror lets the test measure bitcode's enum-tag layout
    /// rather than assuming that the appended variant leaves it unchanged.
    #[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
    enum PreRosterServerControlMessage {
        Divergence(DivergenceReason),
        Tuning(Vec<u8>),
        Relevel { catalog_id: String, level_life: u64 },
    }

    /// Client Control variants before `JoinSeed`, in the current layout (the
    /// switch declaration left Control at wire 33). This measures positional
    /// bitcode tags directly, guarding the admission/parity discriminants
    /// against an accidental insertion.
    #[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
    enum PreJoinSeedClientControlMessage {
        Admission {
            protocol: ProtocolVersion,
            mod_id: String,
            mod_version: String,
        },
        Parity(crate::wire::ParityDeclaration),
    }

    /// Client Control variants before `Revealed`, in the current layout.
    #[derive(Debug, Clone, PartialEq, Encode, Decode)]
    enum PreRevealedClientControlMessage {
        Admission {
            protocol: ProtocolVersion,
            mod_id: String,
            mod_version: String,
        },
        Parity(crate::wire::ParityDeclaration),
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
        const {
            assert!(
                PROTOCOL_ID >= 0x_5052_4C42,
                "player-addressed presentation commands require application protocol PRLB (protocol 11) or later"
            );
        };
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
        use crate::wire::ServerControlMessage;

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
                PreRosterServerControlMessage::Relevel {
                    catalog_id: "campaign-test".to_owned(),
                    level_life: 3,
                },
                ServerControlMessage::Relevel {
                    catalog_id: "campaign-test".to_owned(),
                    level_life: 3,
                },
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
        use crate::wire::{ClientControlMessage, ParityDeclaration};

        let protocol = ProtocolVersion {
            app_protocol_id: 0x5052_4c36,
            wire_version: 17,
        };
        let parity = ParityDeclaration {
            mod_digest: [0x17; 32],
            level: Some(("campaign-test".to_owned(), [0x18; 32])),
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
        use crate::wire::{ClientControlMessage, JoinSeedValue, ParityDeclaration};
        use std::collections::BTreeMap;

        let protocol = ProtocolVersion {
            app_protocol_id: 0x5052_4c39,
            wire_version: 25,
        };
        let parity = ParityDeclaration {
            mod_digest: [0x25; 32],
            level: Some(("campaign-test".to_owned(), [0x26; 32])),
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

    // A previous-build peer cannot decode a presentation command, so gate 1
    // must refuse it before any decode.
    #[test]
    fn presentation_commands_refuse_the_previous_protocol_id() {
        const PRE_COMMAND_PROTOCOL_ID: u32 = 0x_5052_4C41; // "PRLA"
        const {
            assert!(
                PROTOCOL_ID > PRE_COMMAND_PROTOCOL_ID,
                "player-addressed presentation advances the application protocol"
            );
        };
        let previous = ProtocolVersion {
            app_protocol_id: PRE_COMMAND_PROTOCOL_ID,
            wire_version: WIRE_VERSION,
        };
        assert!(matches!(
            validate_handshake(protocol_version(), previous),
            Err(ClosingCause::Protocol { .. })
        ));
        assert_ne!(
            transport_protocol_id(),
            ((PRE_COMMAND_PROTOCOL_ID as u64) << 32) | u64::from(WIRE_VERSION),
            "gate 1 refuses a peer that predates presentation commands"
        );
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
        const {
            assert!(WIRE_VERSION >= 26);
            assert!(PROTOCOL_ID >= 0x5052_4c42);
        }
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

    #[test]
    fn shared_overlay_roster_refuses_wire_27_without_changing_snapshot_layout() {
        const {
            assert!(WIRE_VERSION >= 28);
            assert!(PROTOCOL_ID >= 0x5052_4c42);
        }
        let previous = ProtocolVersion {
            app_protocol_id: PROTOCOL_ID,
            wire_version: 27,
        };
        assert!(matches!(
            validate_handshake(protocol_version(), previous),
            Err(ClosingCause::Protocol { .. })
        ));
        assert_ne!(transport_protocol_id(), ((PROTOCOL_ID as u64) << 32) | 27);
    }

    #[test]
    fn push_drift_layer_refuses_wire_28_peers() {
        const { assert!(WIRE_VERSION > 28) };
        let previous = ProtocolVersion {
            app_protocol_id: PROTOCOL_ID,
            wire_version: 28,
        };
        assert!(matches!(
            validate_handshake(protocol_version(), previous),
            Err(ClosingCause::Protocol { .. })
        ));
        assert_ne!(transport_protocol_id(), ((PROTOCOL_ID as u64) << 32) | 28);
    }

    #[test]
    fn particle_component_renumbering_refuses_wire_29_peers() {
        const { assert!(WIRE_VERSION > 29) };
        let previous = ProtocolVersion {
            app_protocol_id: PROTOCOL_ID,
            wire_version: 29,
        };
        assert!(matches!(
            validate_handshake(protocol_version(), previous),
            Err(ClosingCause::Protocol { .. })
        ));
        assert_ne!(transport_protocol_id(), ((PROTOCOL_ID as u64) << 32) | 29);
    }

    // A pre-vault peer cannot decode `WireMovementState::Vault`, so gate 1
    // refuses it before any snapshot decode.
    #[test]
    fn vault_movement_state_refuses_wire_30_peers() {
        const { assert!(WIRE_VERSION > 30) };
        let previous = ProtocolVersion {
            app_protocol_id: PROTOCOL_ID,
            wire_version: 30,
        };
        assert!(matches!(
            validate_handshake(protocol_version(), previous),
            Err(ClosingCause::Protocol { .. })
        ));
        assert_ne!(transport_protocol_id(), ((PROTOCOL_ID as u64) << 32) | 30);
    }

    // A pre-teleporter peer cannot decode the pawn teleport record, teleporter
    // arming, or the input yaw stamp, so gate 1 refuses it before any decode.
    #[test]
    fn teleport_record_refuses_wire_31_peers() {
        const {
            assert!(WIRE_VERSION >= 32);
            assert!(crate::wire::SNAPSHOT_VERSION >= 21);
        }
        let previous = ProtocolVersion {
            app_protocol_id: PROTOCOL_ID,
            wire_version: 31,
        };
        assert!(matches!(
            validate_handshake(protocol_version(), previous),
            Err(ClosingCause::Protocol { .. })
        ));
        assert_ne!(transport_protocol_id(), ((PROTOCOL_ID as u64) << 32) | 31);
    }

    // A wire-33 peer cannot decode the ghost-link record riding entity
    // records, so both gates refuse it across the bump; the vocabulary did not
    // change, so the application protocol stays PRLC.
    #[test]
    fn ghost_link_record_refuses_wire_33_peers_and_admits_current_wire() {
        const PRE_GHOST_WIRE_VERSION: u32 = 33;
        const {
            assert!(WIRE_VERSION >= 34);
            assert!(crate::wire::SNAPSHOT_VERSION >= 22);
        }
        assert_eq!(
            PROTOCOL_ID, 0x_5052_4C43,
            "no vocabulary change: still PRLC"
        );
        let peer = |wire_version| ProtocolVersion {
            app_protocol_id: PROTOCOL_ID,
            wire_version,
        };
        assert_eq!(
            validate_handshake(protocol_version(), peer(WIRE_VERSION)),
            Ok(())
        );
        assert!(matches!(
            validate_handshake(protocol_version(), peer(PRE_GHOST_WIRE_VERSION)),
            Err(ClosingCause::Protocol { .. })
        ));
        // A wire-33 host refuses this peer just the same.
        assert!(matches!(
            validate_handshake(peer(PRE_GHOST_WIRE_VERSION), protocol_version()),
            Err(ClosingCause::Protocol { .. })
        ));
        assert_ne!(
            transport_protocol_id(),
            ((PROTOCOL_ID as u64) << 32) | u64::from(PRE_GHOST_WIRE_VERSION),
            "gate 1 refuses the pre-ghost layout before any decode"
        );
    }

    // A wire-34 declaration lacks the kind that distinguishes immediate resyncs
    // from switches with authored lower/raise timing.
    #[test]
    fn switch_kind_refuses_wire_34_peers_and_admits_current_wire() {
        const PRE_KIND_WIRE_VERSION: u32 = 34;
        const {
            assert!(WIRE_VERSION >= 35);
        }
        assert_eq!(PROTOCOL_ID, 0x_5052_4C43);
        let old_peer = ProtocolVersion {
            app_protocol_id: PROTOCOL_ID,
            wire_version: PRE_KIND_WIRE_VERSION,
        };
        assert_eq!(
            validate_handshake(protocol_version(), protocol_version()),
            Ok(())
        );
        assert!(matches!(
            validate_handshake(protocol_version(), old_peer),
            Err(ClosingCause::Protocol { .. })
        ));
        assert!(matches!(
            validate_handshake(old_peer, protocol_version()),
            Err(ClosingCause::Protocol { .. })
        ));
        assert_ne!(
            transport_protocol_id(),
            ((PROTOCOL_ID as u64) << 32) | u64::from(PRE_KIND_WIRE_VERSION),
            "gate 1 refuses the declaration layout before decode"
        );
    }

    // A wire-35 snapshot envelope lacks the host level tick, so a client
    // cannot land `levelLoad` tails on the host's fixed ticks: both gates
    // refuse it across the bump, and the application protocol stays PRLC.
    #[test]
    fn level_tick_refuses_wire_35_peers() {
        const PRE_LEVEL_TICK_WIRE_VERSION: u32 = 35;
        const {
            assert!(
                WIRE_VERSION > PRE_LEVEL_TICK_WIRE_VERSION,
                "the snapshot envelope gained the host level tick"
            );
        };
        assert_eq!(crate::wire::SNAPSHOT_VERSION, 23);
        assert_eq!(
            PROTOCOL_ID, 0x_5052_4C43,
            "no vocabulary change: still PRLC"
        );
        let old_peer = ProtocolVersion {
            app_protocol_id: PROTOCOL_ID,
            wire_version: PRE_LEVEL_TICK_WIRE_VERSION,
        };
        assert_eq!(
            validate_handshake(protocol_version(), protocol_version()),
            Ok(())
        );
        assert!(matches!(
            validate_handshake(protocol_version(), old_peer),
            Err(ClosingCause::Protocol { .. })
        ));
        assert!(matches!(
            validate_handshake(old_peer, protocol_version()),
            Err(ClosingCause::Protocol { .. })
        ));
        assert_ne!(
            transport_protocol_id(),
            ((PROTOCOL_ID as u64) << 32) | u64::from(PRE_LEVEL_TICK_WIRE_VERSION),
            "gate 1 refuses the pre-level-tick envelope before decode"
        );
    }

    // A wire-36 `Relevel` carries no host level life, so its client cannot
    // tell a host restart of its active map from a duplicate. Both gates
    // refuse it; the snapshot layout and the vocabulary are unchanged.
    #[test]
    fn relevel_level_life_refuses_wire_36_peers_and_admits_wire_37() {
        const PRE_LEVEL_LIFE_WIRE_VERSION: u32 = 36;
        assert_eq!(WIRE_VERSION, 37);
        assert_eq!(crate::wire::SNAPSHOT_VERSION, 23);
        assert_eq!(
            PROTOCOL_ID, 0x_5052_4C43,
            "no vocabulary change: still PRLC"
        );
        let old_peer = ProtocolVersion {
            app_protocol_id: PROTOCOL_ID,
            wire_version: PRE_LEVEL_LIFE_WIRE_VERSION,
        };
        assert_eq!(
            validate_handshake(protocol_version(), protocol_version()),
            Ok(())
        );
        assert!(matches!(
            validate_handshake(protocol_version(), old_peer),
            Err(ClosingCause::Protocol { .. })
        ));
        assert!(matches!(
            validate_handshake(old_peer, protocol_version()),
            Err(ClosingCause::Protocol { .. })
        ));
        assert_ne!(
            transport_protocol_id(),
            ((PROTOCOL_ID as u64) << 32) | u64::from(PRE_LEVEL_LIFE_WIRE_VERSION),
            "gate 1 refuses the pre-level-life Relevel before decode"
        );
    }

    /// The switch declaration as shipped at wire 25, before its client tick.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
    struct PreTickSwitchDeclaration {
        declaration_id: u32,
        slot: u8,
    }

    // A PRLB / wire-32 peer carries weapon switches on Control, so it numbers
    // the later Control tags differently (`JoinSeed`, `Revealed`, `SessionRoster`)
    // and cannot share the gameplay channel layout. Both gates refuse it; the
    // snapshot layout did not change, so `SNAPSHOT_VERSION` stays.
    #[test]
    fn gameplay_channel_layout_refuses_prlb_and_wire_32_peers() {
        const PRE_LAYOUT_PROTOCOL_ID: u32 = 0x_5052_4C42; // "PRLB"
        const PRE_LAYOUT_WIRE_VERSION: u32 = 32;
        assert_eq!(PROTOCOL_ID, 0x_5052_4C43, "the layout is protocol 12, PRLC");
        const { assert!(WIRE_VERSION > PRE_LAYOUT_WIRE_VERSION) };
        assert_eq!(
            validate_handshake(
                protocol_version(),
                ProtocolVersion {
                    app_protocol_id: 0x_5052_4C43,
                    wire_version: WIRE_VERSION,
                },
            ),
            Ok(()),
            "a PRLC peer at the current wire version is admitted"
        );
        for (app_protocol_id, wire_version) in [
            (PRE_LAYOUT_PROTOCOL_ID, WIRE_VERSION),
            (PROTOCOL_ID, PRE_LAYOUT_WIRE_VERSION),
            (PRE_LAYOUT_PROTOCOL_ID, PRE_LAYOUT_WIRE_VERSION),
        ] {
            assert!(matches!(
                validate_handshake(
                    protocol_version(),
                    ProtocolVersion {
                        app_protocol_id,
                        wire_version,
                    },
                ),
                Err(ClosingCause::Protocol { .. })
            ));
            assert_ne!(
                transport_protocol_id(),
                (u64::from(app_protocol_id) << 32) | u64::from(wire_version),
                "gate 1 refuses the pre-layout pair before any decode"
            );
        }
    }

    #[test]
    fn switch_tick_advances_only_the_wire_version_and_refuses_wire_25_peers() {
        const {
            assert!(WIRE_VERSION >= 26);
        }
        const {
            assert!(
                PROTOCOL_ID >= 0x5052_4c42,
                "the switch tick changed no vocabulary; later vocabulary advanced it to PRLB and past"
            );
        }
        const {
            assert!(
                crate::wire::SNAPSHOT_VERSION >= 18,
                "presentation state later extended snapshot records"
            );
        }
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

    // The declaration left Control at wire 33, so the wire-25 envelope it
    // shipped in no longer exists; the tick still pins the payload layout.
    #[test]
    fn switch_tick_changes_the_declaration_layout() {
        use crate::wire::ClientSwitchDeclaration;
        let before = bitcode::encode(&PreTickSwitchDeclaration {
            declaration_id: 9,
            slot: 2,
        });
        let after = crate::wire::encode(&ClientSwitchDeclaration {
            declaration_id: 9,
            slot: 2,
            client_tick: 0x0102_0304,
            kind: crate::wire::SwitchDeclarationKind::Switch,
        });
        assert_ne!(before, after, "the added tick changes the layout");
        assert!(
            crate::wire::decode::<ClientSwitchDeclaration>(&before).is_err(),
            "a wire-25 declaration never decodes as a tick-stamped one"
        );
    }

    /// The Input envelopes before the switch variants were appended at wire 33.
    #[derive(Debug, Clone, PartialEq, Encode, Decode)]
    enum PreSwitchClientMessage {
        Input(crate::wire::InputCommand),
        Ack(crate::wire::AckMessage),
        BaselineRefresh(crate::wire::BaselineRefreshRequest),
        TimeSync(crate::timesync::TimeSyncRequest),
        StateBaselineRefresh(crate::wire::StateBaselineRefreshRequest),
        HitDeclaration(crate::wire::HitDeclaration),
    }

    #[derive(Debug, Clone, PartialEq, Encode, Decode)]
    enum PreSwitchServerMessage {
        TimeSync(crate::timesync::TimeSyncEcho),
        ShotVerdicts(crate::wire::ShotVerdictsMessage),
        ActivationOutcomes(Vec<crate::wire::ActivationOutcome>),
        WeaponCues(crate::wire::WeaponCuesMessage),
        HitRefused(crate::wire::WireShotId),
    }

    #[test]
    fn switch_append_preserves_shipped_input_message_tags() {
        use crate::wire::{ClientMessage, HitDeclaration, ServerMessage, WireShotId};
        let probe = crate::timesync::TimeSyncRequest {
            sample_id: 3,
            client_send_tick: 4,
            client_send_time_us: 5,
        };
        let shot_id = WireShotId {
            pawn: 1,
            start_tick: 2,
            lane: 0,
            ordinal: 0,
        };
        let declaration = HitDeclaration {
            shot_id,
            records: Vec::new(),
        };
        let client_cases = [
            (
                PreSwitchClientMessage::TimeSync(probe),
                ClientMessage::TimeSync(probe),
            ),
            (
                PreSwitchClientMessage::HitDeclaration(declaration.clone()),
                ClientMessage::HitDeclaration(declaration),
            ),
        ];
        for (historical, current) in client_cases {
            assert_eq!(
                bitcode::encode(&historical),
                crate::wire::encode(&current),
                "appending SwitchDeclaration changed a shipped client Input tag"
            );
        }
        let echo = probe.echo(10, 11);
        let server_cases = [
            (
                PreSwitchServerMessage::TimeSync(echo),
                ServerMessage::TimeSync(echo),
            ),
            (
                PreSwitchServerMessage::HitRefused(shot_id),
                ServerMessage::HitRefused(shot_id),
            ),
        ];
        for (historical, current) in server_cases {
            assert_eq!(
                bitcode::encode(&historical),
                crate::wire::encode(&current),
                "appending the switch outcomes changed a shipped server Input tag"
            );
        }
    }
}
