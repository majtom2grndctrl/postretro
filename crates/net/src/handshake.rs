// Pure immutable-admission comparison and protocol constants.

use crate::wire::{ProtocolVersion, WireError};

pub use crate::wire::{ClosingCause, DivergenceReason, HoldingCause};

/// E15's tagged-control vocabulary.
pub const PROTOCOL_ID: u32 = 0x_5052_4C35; // "PRL5"
/// E15's admission/parity envelope and slot-state layout.
pub const WIRE_VERSION: u32 = 14;

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

    #[test]
    fn validate_handshake_accepts_only_matching_protocol_constants() {
        let version = protocol_version();
        assert_eq!(validate_handshake(version, version), Ok(()));
    }

    #[test]
    fn mover_replay_provenance_wire_version_refuses_previous_peer_on_both_gates() {
        const PRE_E15_PROTOCOL_ID: u32 = 0x_5052_4C34;
        const PRE_E15_WIRE_VERSION: u32 = 13;
        assert_eq!(
            PROTOCOL_ID, 0x_5052_4C35,
            "tagged E15 control requires PRL5"
        );
        assert_eq!(WIRE_VERSION, 14, "E15 wire layout requires wire version 14");
        assert_ne!(
            transport_protocol_id(),
            ((PRE_E15_PROTOCOL_ID as u64) << 32) | u64::from(PRE_E15_WIRE_VERSION),
            "gate 1 rejects the previous layout before app decode"
        );
        let previous = ProtocolVersion {
            app_protocol_id: PRE_E15_PROTOCOL_ID,
            wire_version: PRE_E15_WIRE_VERSION,
        };
        assert!(matches!(
            validate_handshake(protocol_version(), previous),
            Err(ClosingCause::Protocol { .. })
        ));
    }
}
