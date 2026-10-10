//! Stable presentation identity shared by host registration and client apply.

use crate::{EntityId, EntityRegistry};
use glam::Vec3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum PresentationBindingKind {
    MapLight = 0,
    FogVolume = 1,
    MapEntity = 2,
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
