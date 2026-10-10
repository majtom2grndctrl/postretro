//! Immutable written presentation state; bridge sampling never mutates this table.

use crate::components::{billboard_emitter::BillboardEmitterComponent, light::LightComponent};
use crate::{ComponentKind, EntityId, EntityRegistry, FogVolumeComponent};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub enum PresentationComponent {
    Light(LightComponent),
    Fog(FogVolumeComponent),
    Emitter(BillboardEmitterComponent),
}

impl PresentationComponent {
    pub fn snapshot(registry: &EntityRegistry, id: EntityId, kind: ComponentKind) -> Option<Self> {
        match kind {
            ComponentKind::Light => registry
                .get_component::<LightComponent>(id)
                .ok()
                .cloned()
                .map(Self::Light),
            ComponentKind::FogVolume => registry
                .get_component::<FogVolumeComponent>(id)
                .ok()
                .cloned()
                .map(Self::Fog),
            ComponentKind::BillboardEmitter => registry
                .get_component::<BillboardEmitterComponent>(id)
                .ok()
                .cloned()
                .map(Self::Emitter),
            _ => None,
        }
    }
    pub fn kind(&self) -> ComponentKind {
        match self {
            Self::Light(_) => ComponentKind::Light,
            Self::Fog(_) => ComponentKind::FogVolume,
            Self::Emitter(_) => ComponentKind::BillboardEmitter,
        }
    }
    pub fn same_animation(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Light(a), Self::Light(b)) => a.animation == b.animation,
            (Self::Fog(a), Self::Fog(b)) => a.animation == b.animation,
            (Self::Emitter(a), Self::Emitter(b)) => a.spin_animation == b.spin_animation,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PresentationOverride {
    pub component: PresentationComponent,
    pub anim_start: Option<f64>,
}

#[derive(Debug, Default)]
pub struct PresentationOverrides {
    pub records: HashMap<(EntityId, ComponentKind), PresentationOverride>,
    /// First-dispatch baseline, including writes that do not publish an override.
    pub frame_start: HashMap<(EntityId, ComponentKind), PresentationOverride>,
    pub warned_writes: HashSet<(EntityId, String)>,
    pub warned_unresolvable: HashSet<EntityId>,
}
impl PresentationOverrides {
    pub fn begin_frame(&mut self) {
        self.frame_start.clear();
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn remove_entity(&mut self, id: EntityId) {
        self.records.retain(|(entity, _), _| *entity != id);
        self.frame_start.retain(|(entity, _), _| *entity != id);
    }
}
