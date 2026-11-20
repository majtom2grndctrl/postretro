//! Stable presentation identity shared by host registration and client apply.

use crate::{EntityId, EntityRegistry};
use glam::Vec3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum PresentationBindingKind {
    MapLight = 0,
    FogVolume = 1,
    MapEntity = 2,
    EmissiveVolume = 3,
}

impl PresentationBindingKind {
    /// The kind a wire binding's `kind` byte names, or `None` for a byte no
    /// kind owns.
    pub fn from_wire(kind: u8) -> Option<Self> {
        match kind {
            0 => Some(Self::MapLight),
            1 => Some(Self::FogVolume),
            2 => Some(Self::MapEntity),
            3 => Some(Self::EmissiveVolume),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PresentationBindingKey {
    pub kind: PresentationBindingKind,
    pub index: u32,
}

/// App-owned bridges resolve authored indices; netcode never names those bridges.
pub trait PresentationBindingResolver {
    fn seed_light(
        &mut self,
        _registry: &EntityRegistry,
        _id: EntityId,
        _start: Option<f64>,
    ) -> bool {
        false
    }
    fn seed_fog(&mut self, _registry: &EntityRegistry, _id: EntityId, _start: Option<f64>) -> bool {
        false
    }
    fn seed_emissive(
        &mut self,
        _registry: &EntityRegistry,
        _id: EntityId,
        _start: Option<f64>,
    ) -> bool {
        false
    }
    fn seed_emitter(
        &mut self,
        _registry: &EntityRegistry,
        _id: EntityId,
        _elapsed: Option<f64>,
    ) -> bool {
        false
    }
    fn resolve(
        &self,
        registry: &EntityRegistry,
        key: PresentationBindingKey,
    ) -> Option<(EntityId, Vec3)>;
    fn key_for_entity(
        &self,
        registry: &EntityRegistry,
        entity: EntityId,
    ) -> Option<(PresentationBindingKey, Vec3)>;
}

#[cfg(test)]
mod tests {
    use super::PresentationBindingKind;

    // Drift guard: every kind's wire byte names that kind, and no byte past
    // the last kind names one. The exhaustive match makes a new kind a compile
    // error here until its byte is declared.
    #[test]
    fn binding_kind_wire_byte_roundtrips_every_kind() {
        let kinds = [
            PresentationBindingKind::MapLight,
            PresentationBindingKind::FogVolume,
            PresentationBindingKind::MapEntity,
            PresentationBindingKind::EmissiveVolume,
        ];
        for kind in kinds {
            let byte = match kind {
                PresentationBindingKind::MapLight => 0,
                PresentationBindingKind::FogVolume => 1,
                PresentationBindingKind::MapEntity => 2,
                PresentationBindingKind::EmissiveVolume => 3,
            };
            assert_eq!(kind as u8, byte);
            assert_eq!(PresentationBindingKind::from_wire(byte), Some(kind));
        }
        assert_eq!(PresentationBindingKind::from_wire(4), None);
    }
}
