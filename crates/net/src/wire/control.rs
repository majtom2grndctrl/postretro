use bitcode::{Decode, Encode};

/// The fixed size of renetcode's client-authentication user-data field.
pub const NETCODE_USER_DATA_BYTES: usize = 256;
const CONNECT_CLAIM_MAGIC: &[u8; 4] = b"PRSC";
const CONNECT_CLAIM_VERSION: u8 = 1;
const CONNECT_CLAIM_HEADER_BYTES: usize = 7;
/// The largest UTF-8 display name accepted in a connection claim.
pub const DISPLAY_NAME_MAX_BYTES: usize = 200;

/// A player-controlled durable identity carried in a connection claim.
///
/// The bytes are opaque. Consumers may retain or compare the whole value, but
/// must not parse, slice, order, or derive from them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct PlayerClaimId(pub [u8; 16]);

/// A host-minted identity for one running session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct SessionId(pub [u8; 16]);

/// Immutable client assertion included in the netcode connection token.
///
/// New fields must be appended: bitcode encodes struct fields positionally.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct ConnectClaim {
    pub player_id: PlayerClaimId,
    pub display_name: String,
}

/// Encode a claim into renetcode's fixed-width authentication user-data field.
#[must_use]
pub fn encode_connect_claim(claim: &ConnectClaim) -> [u8; NETCODE_USER_DATA_BYTES] {
    let display_name = truncate_display_name(&claim.display_name);
    let payload = bitcode::encode(&ConnectClaim {
        player_id: claim.player_id,
        display_name,
    });
    debug_assert!(payload.len() <= NETCODE_USER_DATA_BYTES - CONNECT_CLAIM_HEADER_BYTES);

    let mut user_data = [0; NETCODE_USER_DATA_BYTES];
    user_data[..CONNECT_CLAIM_MAGIC.len()].copy_from_slice(CONNECT_CLAIM_MAGIC);
    user_data[4] = CONNECT_CLAIM_VERSION;
    user_data[5..CONNECT_CLAIM_HEADER_BYTES].copy_from_slice(&(payload.len() as u16).to_le_bytes());
    user_data[CONNECT_CLAIM_HEADER_BYTES..CONNECT_CLAIM_HEADER_BYTES + payload.len()]
        .copy_from_slice(&payload);
    user_data
}

/// Decode a claim from renetcode's user-data field.
///
/// Missing user data is random bytes in renetcode, so malformed, stale, or
/// absent envelopes all degrade to an anonymous connection without error.
#[must_use]
pub fn decode_connect_claim(user_data: &[u8; NETCODE_USER_DATA_BYTES]) -> Option<ConnectClaim> {
    if user_data[..CONNECT_CLAIM_MAGIC.len()] != *CONNECT_CLAIM_MAGIC
        || user_data[4] != CONNECT_CLAIM_VERSION
    {
        return None;
    }
    let payload_len = usize::from(u16::from_le_bytes([user_data[5], user_data[6]]));
    let payload_end = CONNECT_CLAIM_HEADER_BYTES.checked_add(payload_len)?;
    let payload = user_data.get(CONNECT_CLAIM_HEADER_BYTES..payload_end)?;
    let claim = bitcode::decode::<ConnectClaim>(payload).ok()?;
    (claim.display_name.len() <= DISPLAY_NAME_MAX_BYTES).then_some(claim)
}

fn truncate_display_name(display_name: &str) -> String {
    if display_name.len() <= DISPLAY_NAME_MAX_BYTES {
        return display_name.to_owned();
    }
    let mut end = DISPLAY_NAME_MAX_BYTES;
    while !display_name.is_char_boundary(end) {
        end -= 1;
    }
    display_name[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(display_name: &str) -> ConnectClaim {
        ConnectClaim {
            player_id: PlayerClaimId([0x5a; 16]),
            display_name: display_name.to_owned(),
        }
    }

    #[test]
    fn connect_claim_envelope_roundtrips_and_zero_fills() {
        let encoded = encode_connect_claim(&claim("Neon Runner"));

        assert_eq!(encoded.len(), NETCODE_USER_DATA_BYTES);
        assert_eq!(&encoded[..4], CONNECT_CLAIM_MAGIC);
        assert_eq!(encoded[4], CONNECT_CLAIM_VERSION);
        let payload_len = usize::from(u16::from_le_bytes([encoded[5], encoded[6]]));
        assert!(
            encoded[CONNECT_CLAIM_HEADER_BYTES + payload_len..]
                .iter()
                .all(|byte| *byte == 0)
        );
        assert_eq!(decode_connect_claim(&encoded), Some(claim("Neon Runner")));
    }

    #[test]
    fn connect_claim_envelope_truncates_display_name_on_utf8_boundary() {
        let name = format!("{}é", "a".repeat(DISPLAY_NAME_MAX_BYTES - 1));
        let decoded = decode_connect_claim(&encode_connect_claim(&claim(&name)))
            .expect("encoded claim decodes");

        assert_eq!(decoded.display_name, "a".repeat(DISPLAY_NAME_MAX_BYTES - 1));
        assert!(
            decoded
                .display_name
                .is_char_boundary(decoded.display_name.len())
        );
    }

    #[test]
    fn connect_claim_envelope_rejects_random_or_mismatched_headers() {
        assert_eq!(decode_connect_claim(&[0xa5; NETCODE_USER_DATA_BYTES]), None);

        let mut wrong_magic = encode_connect_claim(&claim("Neon Runner"));
        wrong_magic[0] = b'X';
        assert_eq!(decode_connect_claim(&wrong_magic), None);

        let mut wrong_version = encode_connect_claim(&claim("Neon Runner"));
        wrong_version[4] = CONNECT_CLAIM_VERSION + 1;
        assert_eq!(decode_connect_claim(&wrong_version), None);
    }

    #[test]
    fn connect_claim_envelope_rejects_invalid_payload_lengths_and_bitcode() {
        let mut overlong_length = encode_connect_claim(&claim("Neon Runner"));
        let too_long = (NETCODE_USER_DATA_BYTES - CONNECT_CLAIM_HEADER_BYTES + 1) as u16;
        overlong_length[5..CONNECT_CLAIM_HEADER_BYTES].copy_from_slice(&too_long.to_le_bytes());
        assert_eq!(decode_connect_claim(&overlong_length), None);

        let mut malformed_payload = encode_connect_claim(&claim("Neon Runner"));
        malformed_payload[5..CONNECT_CLAIM_HEADER_BYTES].copy_from_slice(&1_u16.to_le_bytes());
        malformed_payload[CONNECT_CLAIM_HEADER_BYTES] = 0xff;
        assert_eq!(decode_connect_claim(&malformed_payload), None);
    }

    #[test]
    fn connect_claim_envelope_rejects_overlong_decoded_display_name() {
        let claim = claim(&"a".repeat(DISPLAY_NAME_MAX_BYTES + 1));
        let payload = bitcode::encode(&claim);
        assert!(payload.len() <= NETCODE_USER_DATA_BYTES - CONNECT_CLAIM_HEADER_BYTES);

        let mut user_data = [0; NETCODE_USER_DATA_BYTES];
        user_data[..CONNECT_CLAIM_MAGIC.len()].copy_from_slice(CONNECT_CLAIM_MAGIC);
        user_data[4] = CONNECT_CLAIM_VERSION;
        user_data[5..CONNECT_CLAIM_HEADER_BYTES]
            .copy_from_slice(&(payload.len() as u16).to_le_bytes());
        user_data[CONNECT_CLAIM_HEADER_BYTES..CONNECT_CLAIM_HEADER_BYTES + payload.len()]
            .copy_from_slice(&payload);

        assert_eq!(decode_connect_claim(&user_data), None);
    }
}

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
    /// Session-lifetime player roster. This stays deliberately registry-blind:
    /// seats are bare wire integers and carried seat contents remain host-local.
    ///
    /// New variants must be appended. bitcode encodes enum tags positionally, so
    /// inserting this before an existing variant would renumber shipped messages.
    SessionRoster(SessionRosterMessage),
}

/// One seat's session-visible connection state.
///
/// Claims remain in the host's seat table for future rejoin handling. They are
/// deliberately absent from this wire type: a roster exposes only host-minted
/// seats and the connection lifecycle fact. `connected` does not imply
/// participation, since admitted peers receive the roster too.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct RosterEntry {
    /// Host-minted seat, carried on the wire as its bare `u16` value.
    pub seat: u16,
    /// Whether the host currently has a live connection bound to this seat.
    pub connected: bool,
}

/// Per-recipient session roster publication.
///
/// The session id accompanies every publication so an arriving client can
/// distinguish a new hosted run from an update to the prior run. `your_seat` is
/// encoded separately for each recipient; reusing one encoded frame would leak a
/// different recipient's own-seat identity. `open_seats` is the remaining
/// monotonic seat namespace, letting peers distinguish a full session from a
/// roster with merely disconnected held seats.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct SessionRosterMessage {
    pub session_id: SessionId,
    /// `None` means this admitted recipient could not be assigned a seat.
    pub your_seat: Option<u16>,
    /// Number of fresh seats that can still be minted during this session.
    pub open_seats: u32,
    pub entries: Vec<RosterEntry>,
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
