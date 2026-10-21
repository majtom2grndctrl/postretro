// Wire-mirrored pawn relocation identity and shared teleport yaw representation.
// See: context/lib/movement.md §6 (Teleport)

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use smallvec::SmallVec;

pub const NO_TELEPORTER: u16 = u16::MAX;

/// Latches held without a heap allocation. An arrival usually sits inside at
/// most the return direction of a two-way pair; more spill to the heap.
const INLINE_LATCHES: usize = 4;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TeleportState {
    pub generation: u8,
    pub accumulated_yaw: f32,
    pub last_teleporter: u16,
    /// Sorted map-order indices; arrival can latch several overlapping regions.
    pub arrival_latches: TeleportLatches,
}

impl Default for TeleportState {
    fn default() -> Self {
        Self {
            generation: 0,
            accumulated_yaw: 0.0,
            last_teleporter: NO_TELEPORTER,
            arrival_latches: TeleportLatches::default(),
        }
    }
}

/// A pawn's arrival latches: sorted teleporter indices, uncapped.
///
/// The movement component is cloned every tick by host snapshots, prediction
/// history and reconcile replay, so the common few latches live inline and a
/// clone allocates nothing; more spill to the heap. Reads go through the slice
/// it derefs to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TeleportLatches(SmallVec<[u16; INLINE_LATCHES]>);

impl TeleportLatches {
    pub fn from_slice(latches: &[u16]) -> Self {
        Self(SmallVec::from_slice(latches))
    }
    pub fn push(&mut self, index: u16) {
        self.0.push(index);
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&u16) -> bool) {
        self.0.retain(|index| keep(index));
    }
}

impl std::ops::Deref for TeleportLatches {
    type Target = [u16];
    fn deref(&self) -> &[u16] {
        &self.0
    }
}

impl std::ops::DerefMut for TeleportLatches {
    fn deref_mut(&mut self) -> &mut [u16] {
        &mut self.0
    }
}

impl Extend<u16> for TeleportLatches {
    fn extend<I: IntoIterator<Item = u16>>(&mut self, latches: I) {
        self.0.extend(latches);
    }
}

impl PartialEq<[u16]> for TeleportLatches {
    fn eq(&self, other: &[u16]) -> bool {
        self.0[..] == *other
    }
}

impl<const N: usize> PartialEq<[u16; N]> for TeleportLatches {
    fn eq(&self, other: &[u16; N]) -> bool {
        self.0[..] == other[..]
    }
}

impl PartialEq<Vec<u16>> for TeleportLatches {
    fn eq(&self, other: &Vec<u16>) -> bool {
        self.0[..] == other[..]
    }
}

// A plain index sequence, the shape a `Vec<u16>` serializes to.
impl Serialize for TeleportLatches {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0[..].serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TeleportLatches {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<u16>::deserialize(deserializer).map(|latches| Self::from_slice(&latches))
    }
}

/// The stored representation is (-pi, pi], on every machine, before encoding.
pub fn wrap_teleport_yaw(yaw: f32) -> f32 {
    let yaw = yaw.rem_euclid(std::f32::consts::TAU);
    if yaw > std::f32::consts::PI {
        yaw - std::f32::consts::TAU
    } else {
        yaw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teleport_latches_clone_inline_and_spill_past_capacity_without_a_cap() {
        let mut latches = TeleportLatches::default();
        for index in 0..INLINE_LATCHES as u16 {
            latches.push(index);
        }
        assert!(!latches.0.spilled(), "the common few latches stay inline");
        assert!(!latches.clone().0.spilled(), "so a clone allocates nothing");
        latches.push(INLINE_LATCHES as u16);
        assert!(latches.0.spilled());
        assert_eq!(latches, [0, 1, 2, 3, 4], "a spill keeps every latch");
        latches.retain(|index| index % 2 == 0);
        assert_eq!(latches, vec![0, 2, 4]);
    }

    #[test]
    fn teleport_latches_serialize_as_a_plain_index_sequence() {
        let state = TeleportState {
            arrival_latches: TeleportLatches::from_slice(&[2, 7]),
            ..Default::default()
        };
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["arrival_latches"], serde_json::json!([2, 7]));
        let back: TeleportState = serde_json::from_value(json).unwrap();
        assert_eq!(back, state);
    }
}
