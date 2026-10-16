// Teleport wire identity and the explicitly counted latch payload.
// See: context/lib/networking.md §Snapshot apply ordering

use bitcode::{Decode, Encode};

pub const NO_WIRE_TELEPORTER: u16 = u16::MAX;

pub fn teleport_yaw_is_valid(yaw: f32) -> bool {
    yaw.is_finite() && yaw > -std::f32::consts::PI && yaw <= std::f32::consts::PI
}

/// Bitcode transports this owned record. Its byte-vector envelope contains an
/// explicit little-endian u16 count followed by that many little-endian u16
/// indices; the vector's bitcode length is an outer envelope, not the count.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct WireTeleportState {
    pub generation: u8,
    pub accumulated_yaw: f32,
    pub last_teleporter: u16,
    pub arrival_latches: Vec<u8>,
}

impl Default for WireTeleportState {
    fn default() -> Self {
        Self {
            generation: 0,
            accumulated_yaw: 0.0,
            last_teleporter: NO_WIRE_TELEPORTER,
            arrival_latches: vec![0, 0],
        }
    }
}

impl WireTeleportState {
    pub fn new(
        generation: u8,
        accumulated_yaw: f32,
        last_teleporter: u16,
        latches: &[u16],
    ) -> Option<Self> {
        let count = u16::try_from(latches.len()).ok()?;
        let mut arrival_latches = Vec::with_capacity(2 + latches.len() * 2);
        arrival_latches.extend_from_slice(&count.to_le_bytes());
        for index in latches {
            arrival_latches.extend_from_slice(&index.to_le_bytes());
        }
        let state = Self {
            generation,
            accumulated_yaw,
            last_teleporter,
            arrival_latches,
        };
        state.is_valid().then_some(state)
    }

    pub fn is_valid(&self) -> bool {
        if !teleport_yaw_is_valid(self.accumulated_yaw) || self.arrival_latches.len() < 2 {
            return false;
        }
        let count = u16::from_le_bytes([self.arrival_latches[0], self.arrival_latches[1]]) as usize;
        if self.arrival_latches.len() != 2 + count * 2 {
            return false;
        }
        let mut previous = None;
        self.latches().all(|index| {
            let valid = index != NO_WIRE_TELEPORTER && previous.is_none_or(|prior| prior < index);
            previous = Some(index);
            valid
        })
    }

    /// Only consume after validation; malformed tails are deliberately ignored
    /// here and rejected by `is_valid` before typed apply.
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

    #[test]
    fn teleport_wire_preserves_u16_count_indices_and_wrapped_yaw() {
        let state =
            WireTeleportState::new(255, std::f32::consts::PI, 2, &[0, 2, u16::MAX - 1]).unwrap();
        assert_eq!(state.arrival_latches, vec![3, 0, 0, 0, 2, 0, 254, 255]);
        let decoded: WireTeleportState = crate::wire::decode(&crate::wire::encode(&state)).unwrap();
        assert!(decoded.is_valid());
        assert_eq!(
            decoded.latches().collect::<Vec<_>>(),
            vec![0, 2, u16::MAX - 1]
        );
        assert_eq!(decoded.generation, 255);
        assert!((decoded.accumulated_yaw - std::f32::consts::PI).abs() < 1.0e-6);
    }

    #[test]
    fn teleport_wire_rejects_invalid_counts_order_indices_and_yaw() {
        let valid = WireTeleportState::new(1, 0.0, 0, &[0, 3]).unwrap();
        for bytes in [
            vec![],
            vec![0],
            vec![2, 0, 0, 0],
            vec![1, 0, 0, 0, 3, 0],
            vec![2, 0, 3, 0, 0, 0],
            vec![2, 0, 0, 0, 0, 0],
            vec![1, 0, 255, 255],
        ] {
            let mut bad = valid.clone();
            bad.arrival_latches = bytes;
            assert!(!bad.is_valid());
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
        }
        assert!(WireTeleportState::new(0, 0.0, NO_WIRE_TELEPORTER, &[]).is_some());
    }

    #[test]
    fn teleport_wire_uses_the_full_u16_latch_envelope() {
        let latches: Vec<_> = (0..u16::MAX).collect();
        let state = WireTeleportState::new(0, 0.0, u16::MAX - 1, &latches).unwrap();
        assert!(state.is_valid());
        assert_eq!(state.latches().count(), usize::from(u16::MAX));
        assert_eq!(&state.arrival_latches[..2], &u16::MAX.to_le_bytes());
        let oversized = vec![0; usize::from(u16::MAX) + 1];
        assert!(WireTeleportState::new(0, 0.0, 0, &oversized).is_none());
    }
}
