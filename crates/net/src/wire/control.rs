use bitcode::{Decode, Encode};

/// Build constants carried by the immutable admission declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct ProtocolVersion {
    pub app_protocol_id: u32,
    pub wire_version: u32,
}

/// Current content declaration retained for one client slot. `level: None`
/// explicitly means the client has no installed level.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct ParityDeclaration {
    pub mod_digest: [u8; 32],
    pub level: Option<(String, [u8; 32])>,
}

/// Tagged client -> server Control envelope. New variants must be appended.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum ClientControlMessage {
    Admission {
        protocol: ProtocolVersion,
        mod_id: String,
        /// Diagnostic-only: a mod version intentionally never gates admission.
        mod_version: String,
    },
    Parity(ParityDeclaration),
    /// A client-authoritative inventory switch declaration. The transport only
    /// carries its slot; engine code validates the owned pawn and its inventory.
    SwitchDeclaration(ClientSwitchDeclaration),
}

/// Reliable client -> host declaration of the inventory slot the client switched to.
/// This stays registry-blind so it may cross the transport Control gate directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct ClientSwitchDeclaration {
    pub declaration_id: u32,
    pub slot: u8,
}

/// A terminal immutable-admission mismatch.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum ClosingCause {
    Protocol {
        expected: ProtocolVersion,
        received: ProtocolVersion,
    },
    ModId {
        expected: String,
        received: String,
        expected_version: String,
        received_version: String,
    },
}

/// A recoverable content mismatch. Variant order is also diagnostic precedence.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum HoldingCause {
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

/// A divergence is statically either terminal or recoverable.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum DivergenceReason {
    Closing(ClosingCause),
    Holding(HoldingCause),
}

impl std::fmt::Display for DivergenceReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closing(ClosingCause::Protocol { expected, received }) => write!(
                f,
                "protocol mismatch: expected app_protocol_id={:#010x} wire_version={}, received app_protocol_id={:#010x} wire_version={}",
                expected.app_protocol_id,
                expected.wire_version,
                received.app_protocol_id,
                received.wire_version,
            ),
            Self::Closing(ClosingCause::ModId {
                expected,
                received,
                expected_version,
                received_version,
            }) => write!(
                f,
                "mod id mismatch: expected {expected} ({expected_version}), received {received} ({received_version})"
            ),
            Self::Holding(HoldingCause::ModDigest { expected, received }) => write!(
                f,
                "mod digest differs: expected {}, received {}",
                digest_hex(expected),
                digest_hex(received)
            ),
            Self::Holding(HoldingCause::HostLevelAbsent) => write!(
                f,
                "host has no level installed; this slot will re-participate on the host's next install"
            ),
            Self::Holding(HoldingCause::LevelAbsent { expected_identity }) => {
                write!(f, "no level installed; host is running {expected_identity}")
            }
            Self::Holding(HoldingCause::LevelIdentity { expected, received }) => write!(
                f,
                "level identity differs: expected {expected}, received {received}"
            ),
            Self::Holding(HoldingCause::LevelDigest {
                identity,
                expected,
                received,
            }) => write!(
                f,
                "level content differs for {identity}: expected {}, received {}",
                digest_hex(expected),
                digest_hex(received)
            ),
        }
    }
}

impl std::error::Error for DivergenceReason {}

fn digest_hex(digest: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Tagged server -> client Control envelope for divergence causes, opaque
/// engine-serialized tuning, and host-selected catalog map ids.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum ServerControlMessage {
    Divergence(DivergenceReason),
    /// Opaque engine-serialized tuning. Net is a registry-blind courier and must
    /// never decode, compare, or validate this descriptor payload.
    Tuning(Vec<u8>),
    /// Host-selected map catalog id. The engine resolves it against the local
    /// catalog and follows through its normal queued level-load path.
    Relevel(String),
    /// The host refused a client switch declaration. The client restores its
    /// previous active slot locally; no snapshot correction is required.
    SwitchRefused(ServerSwitchRefused),
    /// The host accepted a client switch declaration. Explicit acknowledgements
    /// bound the client's rollback chain even when every declaration succeeds.
    SwitchAccepted(ServerSwitchAccepted),
}

/// Reliable host -> client acknowledgement for one accepted inventory slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct ServerSwitchAccepted {
    pub declaration_id: u32,
    pub slot: u8,
}

/// Reliable host -> client refusal for one requested inventory slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct ServerSwitchRefused {
    pub declaration_id: u32,
    pub slot: u8,
}

/// Transport-owned frame around one server Control payload. The optional epoch
/// names the slot participation generation in which the payload was produced.
/// A holding diagnostic retires that generation on the client even when no
/// snapshot from it arrived.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub(crate) struct ServerControlFrame {
    pub participation_epoch: Option<u64>,
    /// `None` is a transport-only participation-start marker and the only frame
    /// that may arm an epoch. Engine Control messages always carry `Some`.
    pub payload: Option<Vec<u8>>,
}

/// Transport-owned frame around Snapshot and client Input payloads. The engine
/// payload remains opaque so participation lifecycle gating stays registry-blind.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub(crate) struct ParticipationFrame {
    pub participation_epoch: u64,
    pub payload: Vec<u8>,
}
