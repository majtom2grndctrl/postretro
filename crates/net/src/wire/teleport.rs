// Teleport wire identity and the explicitly counted latch payload.
// See: context/lib/networking.md §Snapshot apply ordering

use std::sync::{Arc, LazyLock};

use bitcode::{Decode, Encode};

pub const NO_WIRE_TELEPORTER: u16 = u16::MAX;

pub fn teleport_yaw_is_valid(yaw: f32) -> bool {
    yaw.is_finite() && yaw > -std::f32::consts::PI && yaw <= std::f32::consts::PI
}

/// Bitcode transports this record. `arrival_latches` holds exactly a
/// little-endian u16 count followed by that many little-endian u16 teleporter
/// indices in strictly ascending order; bitcode's own byte-vector length is an
/// outer envelope, not the count. The bytes are shared, so the record cloned
/// per pawn per snapshot never allocates, and an unlatched record shares one
/// static empty set.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct WireTeleportState {
    pub generation: u8,
    pub accumulated_yaw: f32,
    pub last_teleporter: u16,
    arrival_latches: Arc<[u8]>,
}

/// The encoded empty latch set: a zero count and no indices.
fn no_latches() -> Arc<[u8]> {
    static NO_LATCHES: LazyLock<Arc<[u8]>> = LazyLock::new(|| Arc::from([0u8; 2].as_slice()));
    NO_LATCHES.clone()
}

impl Default for WireTeleportState {
    fn default() -> Self {
        Self {
            generation: 0,
            accumulated_yaw: 0.0,
            last_teleporter: NO_WIRE_TELEPORTER,
            arrival_latches: no_latches(),
        }
    }
}

impl WireTeleportState {
    /// `None` when the record cannot be represented: more than `u16::MAX`
    /// latches, or a yaw or latch order [`Self::is_valid`] rejects. Allocates
    /// only for a non-empty latch set.
    pub fn new(
        generation: u8,
        accumulated_yaw: f32,
        last_teleporter: u16,
        latches: &[u16],
    ) -> Option<Self> {
        let count = u16::try_from(latches.len()).ok()?;
        let arrival_latches = if latches.is_empty() {
            no_latches()
        } else {
            let mut bytes = Vec::with_capacity(2 + latches.len() * 2);
            bytes.extend_from_slice(&count.to_le_bytes());
            for index in latches {
                bytes.extend_from_slice(&index.to_le_bytes());
            }
            Arc::from(bytes)
        };
        let state = Self {
            generation,
            accumulated_yaw,
            last_teleporter,
            arrival_latches,
        };
        state.is_valid().then_some(state)
    }

    pub fn is_valid(&self) -> bool {
        let Some(count) = self.declared_count() else {
            return false;
        };
        if !teleport_yaw_is_valid(self.accumulated_yaw)
            || self.arrival_latches.len() != 2 + usize::from(count) * 2
        {
            return false;
        }
        let mut previous = None;
        self.latches().all(|index| {
            let valid = index != NO_WIRE_TELEPORTER && previous.is_none_or(|prior| prior < index);
            previous = Some(index);
            valid
        })
    }

    fn declared_count(&self) -> Option<u16> {
        let count = self.arrival_latches.first_chunk::<2>()?;
        Some(u16::from_le_bytes(*count))
    }

    /// The declared latches in ascending order. Only consume after validation:
    /// a malformed tail is ignored here and rejected by `is_valid` before typed
    /// apply.
    pub fn latches(&self) -> impl Iterator<Item = u16> + '_ {
        self.arrival_latches
            .get(2..)
            .unwrap_or_default()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|bytes| u16::from_le_bytes(*bytes))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct WireTeleporterState {
    pub index: u16,
    pub source_entity: u32,
    pub armed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The record as the brief specifies its bytes: the latch field is a plain
    /// byte vector holding the u16 count and then the indices.
    #[derive(Encode)]
    struct SpecifiedLayout {
        generation: u8,
        accumulated_yaw: f32,
        last_teleporter: u16,
        arrival_latches: Vec<u8>,
    }

    #[test]
    fn teleport_wire_latch_bytes_are_exactly_the_count_then_the_indices() {
        let cases: [(&[u16], &[u8]); 4] = [
            (&[], &[0, 0]),
            (&[7], &[1, 0, 7, 0]),
            (&[0, 2, 300, 0xFFFE], &[4, 0, 0, 0, 2, 0, 44, 1, 0xFE, 0xFF]),
            (
                &[1, 2, 3, 4, 5, 6],
                &[6, 0, 1, 0, 2, 0, 3, 0, 4, 0, 5, 0, 6, 0],
            ),
        ];
        for (latches, bytes) in cases {
            let state = WireTeleportState::new(9, 0.5, 3, latches).unwrap();
            assert_eq!(&state.arrival_latches[..], bytes, "{latches:?}");
            let specified = SpecifiedLayout {
                generation: 9,
                accumulated_yaw: 0.5,
                last_teleporter: 3,
                arrival_latches: bytes.to_vec(),
            };
            assert_eq!(
                crate::wire::encode(&state),
                crate::wire::encode(&specified),
                "the encoded record carries nothing beyond count and indices: {latches:?}"
            );
        }
    }

    #[test]
    fn teleport_wire_preserves_u16_count_indices_and_wrapped_yaw() {
        for latches in [&[][..], &[0, 2, u16::MAX - 1], &[1, 3, 5, 7, 9, 11]] {
            let state = WireTeleportState::new(255, std::f32::consts::PI, 2, latches).unwrap();
            let decoded: WireTeleportState =
                crate::wire::decode(&crate::wire::encode(&state)).unwrap();
            assert!(decoded.is_valid());
            assert_eq!(decoded, state);
            assert_eq!(decoded.latches().collect::<Vec<_>>(), latches);
            assert_eq!(decoded.generation, 255);
            assert!((decoded.accumulated_yaw - std::f32::consts::PI).abs() < 1.0e-6);
        }
    }

    #[test]
    fn teleport_wire_clones_share_their_latch_bytes() {
        let empty = WireTeleportState::default();
        assert!(Arc::ptr_eq(
            &empty.arrival_latches,
            &WireTeleportState::new(1, 0.0, 0, &[])
                .unwrap()
                .arrival_latches
        ));
        for count in 0..=5u16 {
            let latches: Vec<u16> = (0..count).collect();
            let state = WireTeleportState::new(1, 0.0, 0, &latches).unwrap();
            let clone = state.clone();
            assert!(
                Arc::ptr_eq(&state.arrival_latches, &clone.arrival_latches),
                "cloning {count} latches allocates nothing"
            );
        }
    }

    #[test]
    fn teleport_wire_rejects_invalid_counts_order_indices_and_yaw() {
        let valid = WireTeleportState::new(1, 0.0, 0, &[0, 3]).unwrap();
        for bytes in [
            vec![],
            vec![0],
            vec![2, 0, 0, 0],
            vec![1, 0, 0, 0, 3, 0],
            vec![1, 0, 0],
            vec![2, 0, 3, 0, 0, 0],
            vec![2, 0, 0, 0, 0, 0],
            vec![1, 0, 255, 255],
        ] {
            let mut bad = valid.clone();
            bad.arrival_latches = Arc::from(bytes);
            assert!(!bad.is_valid(), "{bad:?}");
            let decoded: WireTeleportState =
                crate::wire::decode(&crate::wire::encode(&bad)).unwrap();
            assert!(!decoded.is_valid());
        }
        for yaw in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -std::f32::consts::PI,
            std::f32::consts::TAU,
        ] {
            let mut bad = valid.clone();
            bad.accumulated_yaw = yaw;
            assert!(!bad.is_valid());
            assert!(WireTeleportState::new(1, yaw, 0, &[0]).is_none());
        }
        assert!(WireTeleportState::new(0, 0.0, NO_WIRE_TELEPORTER, &[]).is_some());
        assert!(WireTeleportState::new(0, 0.0, 0, &[2, 1]).is_none());
    }

    #[test]
    fn teleport_wire_uses_the_full_u16_latch_envelope() {
        let latches: Vec<_> = (0..u16::MAX).collect();
        let state = WireTeleportState::new(0, 0.0, u16::MAX - 1, &latches).unwrap();
        assert!(state.is_valid());
        assert_eq!(&state.arrival_latches[..2], &u16::MAX.to_le_bytes());
        assert_eq!(state.latches().count(), usize::from(u16::MAX));
        let decoded: WireTeleportState = crate::wire::decode(&crate::wire::encode(&state)).unwrap();
        assert!(decoded.is_valid());
        let oversized = vec![0; usize::from(u16::MAX) + 1];
        assert!(WireTeleportState::new(0, 0.0, 0, &oversized).is_none());
    }
}
