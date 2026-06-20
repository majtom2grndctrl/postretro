// Wire codec: bitcode-serialized message types and the protocol/version handshake.
// See: context/lib/networking.md
//
// Every type that crosses the wire derives native `bitcode::Encode`/`Decode`.
// These are dedicated *wire-mirror* types: the engine's `ComponentValue` is a
// serde internally-tagged enum (`#[serde(tag = "kind")]`) which bitcode cannot
// round-trip (`DeserializeAnyNotSupported`). So the component payload carries an
// explicit `u16` discriminant — numeric-equal to the engine `ComponentKind` —
// plus its inner payload, and no serde-internally-tagged enum ever crosses here.
//
// This crate is `postretro`-free and glam-free by design: mirror types use plain
// `[f32; N]` / `f32` / `bool`, never the engine or glam types they shadow. The
// engine-side conversions (`ComponentValue::Transform` <-> `WireTransform`,
// `SimCommand` <-> `InputCommand`) live in `crate::netcode` in the engine, not
// here.

use bitcode::{Decode, Encode};

/// Network-stable entity identity. A `u32` newtype assigned by the host; the wire
/// carries it as a bare `u32` (bitcode encodes the inner field transparently).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Encode, Decode)]
pub struct NetworkId(pub u32);

/// Wire mirror of the engine `Transform`'s networked fields. Scale is not sent —
/// only `position` and `rotation` participate in replication.
///
/// `rotation` mirrors the engine quaternion in **`[x, y, z, w]` order**. The
/// engine-side conversion (which knows glam's `Quat` component order) lives in
/// `crate::netcode`; here it is just four floats in that fixed order.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WireTransform {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
}

/// Per-entity component payload, tagged with an explicit `u16` discriminant that
/// is numeric-equal to the engine `ComponentKind` (Transform = 0). This replaces
/// serde's internal `"kind"` tag so the value can round-trip on bitcode.
///
/// Phase 1 binds **only** `Transform`. Phase 2 grows by adding variants in the
/// same `ComponentKind` numeric order (`Light(WireLight) = 1`, ...) and extending
/// the `kind()` match — the envelope shape does not change, because bitcode
/// encodes the enum variant index itself and `kind()` exposes the engine-aligned
/// discriminant for cross-checks. Keep `kind()` aligned with `ComponentKind` as
/// variants are added (see the drift-guard test below).
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub enum ComponentPayload {
    Transform(WireTransform),
}

impl ComponentPayload {
    /// Engine-aligned `u16` discriminant for this payload, numeric-equal to
    /// `ComponentKind as u16` in the engine. Drift here desyncs replication, so
    /// the mapping is pinned by `transform_discriminant_pinned_to_zero`.
    #[must_use]
    pub fn kind(&self) -> u16 {
        match self {
            ComponentPayload::Transform(_) => 0,
        }
    }
}

/// Full-state snapshot envelope: a server tick stamp plus a count-prefixed list
/// of `(NetworkId, ComponentPayload)`. bitcode length-prefixes the `Vec`, which
/// is the "count-prefixed entries" on the wire; an empty list encodes as count 0.
/// Phase 1 sends full state every snapshot (no delta).
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct Snapshot {
    pub tick: u32,
    pub entries: Vec<(NetworkId, ComponentPayload)>,
}

/// Wire mirror of the engine `MovementInput` fields the input command carries.
/// `wish_dir` is `[right, forward]` (mirroring glam `Vec2` x = right, y = forward).
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WireMovementInput {
    pub wish_dir: [f32; 2],
    pub jump_pressed: bool,
    pub dash_pressed: bool,
    pub running: bool,
    pub crouch_intent: bool,
    pub facing_yaw: f32,
}

/// Wire mirror of the engine `FireButtonState`.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct WireFireButtonState {
    pub pressed: bool,
    pub active: bool,
}

/// Input-command envelope: the client's per-tick intent, mirroring the engine
/// `SimCommand` (movement + fire button). Round-tripped in Phase 1; applied to
/// gameplay in Phase 2.
#[derive(Debug, Clone, Copy, PartialEq, Encode, Decode)]
pub struct InputCommand {
    pub movement: WireMovementInput,
    pub fire_button: WireFireButtonState,
}

/// Handshake message. Every connection is gated on a matching `ProtocolVersion`
/// before any other bitcode payload is decoded — the bitcode byte format is
/// unstable across crate majors, so a mismatch must be rejected up front.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct ProtocolVersion {
    pub app_protocol_id: u32,
    pub wire_version: u32,
}

/// Wire codec failure. Today the only failure mode is a bitcode decode error
/// (short or corrupted buffer); a typed wrapper keeps callers from depending on
/// bitcode's error type directly and leaves room for handshake/version errors.
#[derive(Debug)]
pub enum WireError {
    /// The buffer could not be decoded into the requested type (truncated,
    /// corrupted, or trailing bytes). Never a panic — always this `Err`.
    Decode(bitcode::Error),
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::Decode(e) => write!(f, "wire decode failed: {e}"),
        }
    }
}

impl std::error::Error for WireError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WireError::Decode(e) => Some(e),
        }
    }
}

/// Encode a wire type to a fresh byte buffer. Infallible — bitcode encoding of
/// these owned, finite types cannot fail.
#[must_use]
pub fn encode<T: Encode + ?Sized>(value: &T) -> Vec<u8> {
    bitcode::encode(value)
}

/// Decode a wire type from a byte buffer. A short, corrupted, or over-long buffer
/// yields `Err(WireError::Decode(_))` — never a panic.
pub fn decode<'a, T: Decode<'a>>(bytes: &'a [u8]) -> Result<T, WireError> {
    bitcode::decode::<T>(bytes).map_err(WireError::Decode)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_transform() -> WireTransform {
        WireTransform {
            position: [1.5, -2.0, 3.25],
            // A non-axis-aligned unit quaternion in [x, y, z, w] order.
            rotation: [0.182_574_2, 0.365_148_4, 0.547_722_6, 0.730_296_8],
        }
    }

    fn sample_input() -> InputCommand {
        InputCommand {
            movement: WireMovementInput {
                wish_dir: [0.5, -0.75],
                jump_pressed: true,
                dash_pressed: false,
                running: true,
                crouch_intent: false,
                facing_yaw: 1.234_5,
            },
            fire_button: WireFireButtonState {
                pressed: true,
                active: false,
            },
        }
    }

    // Round-trip a control value: encode then decode must reproduce it. These
    // are finite floats we author directly and never transform, so exact
    // value-equality is the correct assertion (testing_guide §Floating-point:
    // approximate comparison guards *computed* floats, not a byte round-trip of
    // a finite value).
    fn round_trips<T>(value: &T) -> bool
    where
        T: Encode + for<'de> Decode<'de> + PartialEq,
    {
        let bytes = encode(value);
        let decoded: T = decode(&bytes).expect("valid buffer must decode");
        &decoded == value
    }

    #[test]
    fn snapshot_empty_entries_round_trips() {
        let snapshot = Snapshot {
            tick: 0,
            entries: Vec::new(),
        };
        let bytes = encode(&snapshot);
        let decoded: Snapshot = decode(&bytes).expect("valid buffer must decode");
        assert_eq!(decoded, snapshot);
        assert!(decoded.entries.is_empty());
    }

    #[test]
    fn snapshot_multi_entry_round_trips() {
        let snapshot = Snapshot {
            tick: 42,
            entries: vec![
                (
                    NetworkId(1),
                    ComponentPayload::Transform(sample_transform()),
                ),
                (
                    NetworkId(7),
                    ComponentPayload::Transform(WireTransform {
                        position: [0.0, 0.0, 0.0],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                    }),
                ),
            ],
        };
        let bytes = encode(&snapshot);
        let decoded: Snapshot = decode(&bytes).expect("valid buffer must decode");
        assert_eq!(decoded, snapshot);
        // Floats survive byte-identically; assert the exact transform fields too.
        assert_eq!(
            decoded.entries[0].1,
            ComponentPayload::Transform(sample_transform())
        );
    }

    #[test]
    fn input_command_round_trips() {
        assert!(round_trips(&sample_input()));
    }

    #[test]
    fn handshake_round_trips() {
        let handshake = ProtocolVersion {
            app_protocol_id: 0xCAFE_BABE,
            wire_version: 1,
        };
        assert!(round_trips(&handshake));
    }

    #[test]
    fn truncated_buffer_decodes_to_err_not_panic() {
        let snapshot = Snapshot {
            tick: 9,
            entries: vec![(
                NetworkId(3),
                ComponentPayload::Transform(sample_transform()),
            )],
        };
        let bytes = encode(&snapshot);
        // Drop the final byte — a short buffer must be a clean Err, never a panic.
        let truncated = &bytes[..bytes.len() - 1];
        assert!(decode::<Snapshot>(truncated).is_err());
    }

    #[test]
    fn empty_buffer_decodes_to_err_not_panic() {
        assert!(decode::<Snapshot>(&[]).is_err());
        assert!(decode::<ProtocolVersion>(&[]).is_err());
        assert!(decode::<InputCommand>(&[]).is_err());
    }

    #[test]
    fn corrupted_buffer_decodes_to_err_not_panic() {
        // Random bytes are extremely unlikely to be a valid encoding of any of
        // these fixed-shape types; the codec must reject them, not panic.
        let garbage = [0xFFu8, 0x00, 0xAB, 0x12, 0x9C, 0x7D, 0x55, 0x01];
        let _ = decode::<Snapshot>(&garbage);
        let _ = decode::<InputCommand>(&garbage);
        let _ = decode::<ProtocolVersion>(&garbage);
        // No assertion on the variant — the contract is "no panic". Reaching
        // here means each call returned rather than unwinding.
    }

    // Drift guard: the Transform wire discriminant MUST stay 0, numeric-equal to
    // the engine `ComponentKind::Transform as u16` (crates/postretro/src/
    // scripting/registry.rs). Phase 2 adds variants in the same numeric order —
    // if this fails, the wire/engine discriminant mapping has diverged and
    // replication will mis-tag components. Keep `ComponentPayload::kind()`
    // aligned with `ComponentKind` here.
    #[test]
    fn transform_discriminant_pinned_to_zero() {
        let payload = ComponentPayload::Transform(sample_transform());
        assert_eq!(payload.kind(), 0);
    }
}
