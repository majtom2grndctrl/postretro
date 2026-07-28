// Pure application-handshake gate shared by the socket transport and tests.
// See: context/lib/networking.md
//
// This module stays registry- and transport-blind: it compares only opaque wire
// values. `ProtocolVersion` remains a wire type in `wire.rs`.

use crate::wire::{ProtocolVersion, WireError};

/// App protocol identity. Hand-bumped on any change that breaks cross-version
/// compatibility of the message *vocabulary* (a new control message, a changed
/// channel layout). Carried as `ProtocolVersion::app_protocol_id` and folded into
/// the transport-level `protocol_id`.
pub const PROTOCOL_ID: u32 = 0x_5052_4C34; // "PRL4" — E16 adds hit declarations + owner-private shot verdicts

/// Wire-format version. Hand-bumped whenever the bitcode byte layout of any wire
/// type changes (added field, reordered enum, bumped bitcode major). Carried as
/// `ProtocolVersion::wire_version` and folded into the transport-level
/// `protocol_id` so a wire-incompatible peer is refused at the netcode layer.
pub const WIRE_VERSION: u32 = 13; // mover replay tick provenance

/// Transport-level gate fed to renet_netcode as the netcode `protocol_id: u64`.
/// Packs both hand-bumped consts so the encrypted handshake itself fails for any
/// peer whose `(PROTOCOL_ID, WIRE_VERSION)` pair differs — the connection never
/// establishes. The app-level `ProtocolVersion` (sent over the control channel)
/// carries the same two values for the second, app-level gate.
#[must_use]
pub const fn transport_protocol_id() -> u64 {
    ((PROTOCOL_ID as u64) << 32) | (WIRE_VERSION as u64)
}

/// The app-level handshake value built from this build's protocol consts. Sent by
/// the client as its first control message and validated by the server.
#[must_use]
pub const fn protocol_version(kinematic_static_fingerprint: [u8; 32]) -> ProtocolVersion {
    ProtocolVersion {
        app_protocol_id: PROTOCOL_ID,
        wire_version: WIRE_VERSION,
        kinematic_static_fingerprint,
    }
}

/// Why the server refused a joining client at the app-level handshake gate. Carries
/// both versions so a test (and the operator log) can see exactly what diverged.
/// Distinct from the transport gate, which rejects wire-incompatible peers before a
/// connection ever forms — this is the second gate, applied to an *established*
/// connection's first control message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RejectReason {
    /// The `ProtocolVersion` this server expects.
    pub expected: ProtocolVersion,
    /// The `ProtocolVersion` the client actually sent.
    pub received: ProtocolVersion,
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "protocol/content mismatch: expected app_protocol_id={:#010x} wire_version={} \
             kinematic_static_fingerprint={}, received app_protocol_id={:#010x} wire_version={} \
             kinematic_static_fingerprint={}",
            self.expected.app_protocol_id,
            self.expected.wire_version,
            fingerprint_hex(&self.expected.kinematic_static_fingerprint),
            self.received.app_protocol_id,
            self.received.wire_version,
            fingerprint_hex(&self.received.kinematic_static_fingerprint),
        )
    }
}

impl std::error::Error for RejectReason {}

/// Pure handshake gate: the app-level second gate, independent of sockets so the
/// reject reason is unit-assertable. A match is `Ok(())`; any divergence yields the
/// typed `RejectReason` carrying expected vs received.
pub fn validate_handshake(
    expected: ProtocolVersion,
    received: ProtocolVersion,
) -> Result<(), RejectReason> {
    if expected == received {
        Ok(())
    } else {
        Err(RejectReason { expected, received })
    }
}

/// Reconstruct a best-effort `ProtocolVersion` from a decode failure for logging.
/// The bytes did not decode, so there is no real received version — surface the
/// all-zero sentinel, which cannot equal a configured live handshake.
#[must_use]
pub fn malformed_version(_err: &WireError) -> ProtocolVersion {
    ProtocolVersion {
        app_protocol_id: 0,
        wire_version: 0,
        kinematic_static_fingerprint: [0; 32],
    }
}

#[must_use]
pub fn fingerprint_hex(fingerprint: &[u8; 32]) -> String {
    use std::fmt::Write as _;

    let mut hex = String::with_capacity(64);
    for byte in fingerprint {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_KINEMATIC_STATIC_FINGERPRINT: [u8; 32] = [0x5a; 32];

    #[test]
    fn validate_handshake_accepts_matching_versions() {
        let v = protocol_version(TEST_KINEMATIC_STATIC_FINGERPRINT);
        assert_eq!(validate_handshake(v, v), Ok(()));
    }

    #[test]
    fn validate_handshake_rejects_divergent_wire_version() {
        let expected = protocol_version(TEST_KINEMATIC_STATIC_FINGERPRINT);
        let received = ProtocolVersion {
            app_protocol_id: expected.app_protocol_id,
            wire_version: expected.wire_version + 1,
            kinematic_static_fingerprint: expected.kinematic_static_fingerprint,
        };
        let err =
            validate_handshake(expected, received).expect_err("divergent version must reject");
        assert_eq!(err.expected, expected);
        assert_eq!(err.received, received);
    }

    #[test]
    fn validate_handshake_rejects_divergent_protocol_id() {
        let expected = protocol_version(TEST_KINEMATIC_STATIC_FINGERPRINT);
        let received = ProtocolVersion {
            app_protocol_id: expected.app_protocol_id ^ 0xFFFF,
            wire_version: expected.wire_version,
            kinematic_static_fingerprint: expected.kinematic_static_fingerprint,
        };
        let err = validate_handshake(expected, received).expect_err("divergent id must reject");
        assert_eq!(err.expected, expected);
        assert_eq!(err.received, received);
    }

    #[test]
    fn transport_protocol_id_packs_both_consts() {
        let id = transport_protocol_id();
        assert_eq!((id >> 32) as u32, PROTOCOL_ID);
        assert_eq!((id & 0xFFFF_FFFF) as u32, WIRE_VERSION);
    }

    #[test]
    fn mover_replay_provenance_wire_version_refuses_previous_peer_on_both_gates() {
        const PRE_MOVER_REPLAY_PROVENANCE_WIRE_VERSION: u32 = 12;

        assert_eq!(
            PROTOCOL_ID, 0x_5052_4C34,
            "E16 message-vocabulary changes require the PRL4 app protocol id"
        );
        assert_eq!(
            WIRE_VERSION, 13,
            "mover replay provenance requires wire version 13"
        );
        assert_ne!(
            transport_protocol_id(),
            ((PROTOCOL_ID as u64) << 32) | (PRE_MOVER_REPLAY_PROVENANCE_WIRE_VERSION as u64),
            "gate 1 rejects the previous mover layout before app decode"
        );

        let expected = protocol_version(TEST_KINEMATIC_STATIC_FINGERPRINT);
        let previous = ProtocolVersion {
            app_protocol_id: PROTOCOL_ID,
            wire_version: PRE_MOVER_REPLAY_PROVENANCE_WIRE_VERSION,
            kinematic_static_fingerprint: [0; 32],
        };
        let err = validate_handshake(expected, previous)
            .expect_err("gate 2 rejects the previous mover layout");
        assert_eq!(err.expected, expected);
        assert_eq!(err.received, previous);
    }

    #[test]
    fn validate_handshake_rejects_divergent_kinematic_static_fingerprint() {
        let expected = protocol_version(TEST_KINEMATIC_STATIC_FINGERPRINT);
        let received = ProtocolVersion {
            kinematic_static_fingerprint: [0xa5; 32],
            ..expected
        };

        let reason = validate_handshake(expected, received)
            .expect_err("different static mover authoring must reject");

        assert_eq!(reason.expected, expected);
        assert_eq!(reason.received, received);
        assert!(reason.to_string().contains("kinematic_static_fingerprint"));
    }
}
