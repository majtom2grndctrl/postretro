// Immutable-admission comparison and protocol constants.
// See: context/lib/networking.md

use crate::wire::{ProtocolVersion, WireError};

pub use crate::wire::{ClosingCause, DivergenceReason, HoldingCause};

/// E15's tagged-control vocabulary.
///
/// `SessionRoster` extends the server Control message vocabulary, so this app
/// protocol id changes even though its appended enum tag leaves the measured
/// bitcode layout of the five shipped variants intact.
pub const PROTOCOL_ID: u32 = 0x_5052_4C36; // "PRL6"
/// E15's admission/parity envelopes and participation-framed traffic layouts.
/// E16 adds `drop_pressed` to `WireMovementInput`, changing the per-tick Input
/// channel layout. The independent tuning-payload epoch remains unchanged here.
pub const WIRE_VERSION: u32 = 16;

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

    #[test]
    fn validate_handshake_accepts_only_matching_protocol_constants() {
        let version = protocol_version();
        assert_eq!(validate_handshake(version, version), Ok(()));
    }

    #[test]
    fn drop_pressed_input_layout_refuses_previous_wire_version() {
        const PRE_DROP_PRESSED_WIRE_VERSION: u32 = 15;
        assert_eq!(
            PROTOCOL_ID, 0x_5052_4C36,
            "session roster requires application protocol PRL6"
        );
        assert_eq!(
            WIRE_VERSION, 16,
            "drop_pressed changes the Input command bitcode layout"
        );
        assert_ne!(
            transport_protocol_id(),
            ((PROTOCOL_ID as u64) << 32) | u64::from(PRE_DROP_PRESSED_WIRE_VERSION),
            "gate 1 rejects the previous layout before app decode"
        );
        let previous = ProtocolVersion {
            app_protocol_id: PROTOCOL_ID,
            wire_version: PRE_DROP_PRESSED_WIRE_VERSION,
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
