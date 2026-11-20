//! Immutable written presentation state; bridge sampling never mutates this table.

use crate::components::{billboard_emitter::BillboardEmitterComponent, light::LightComponent};
use crate::{ComponentKind, EmissiveVolumeComponent, EntityId, EntityRegistry, FogVolumeComponent};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub enum PresentationComponent {
    Light(LightComponent),
    Fog(FogVolumeComponent),
    Emitter(BillboardEmitterComponent),
    Emissive(EmissiveVolumeComponent),
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
            ComponentKind::EmissiveVolume => registry
                .get_component::<EmissiveVolumeComponent>(id)
                .ok()
                .cloned()
                .map(Self::Emissive),
            _ => None,
        }
    }
    pub fn kind(&self) -> ComponentKind {
        match self {
            Self::Light(_) => ComponentKind::Light,
            Self::Fog(_) => ComponentKind::FogVolume,
            Self::Emitter(_) => ComponentKind::BillboardEmitter,
            Self::Emissive(_) => ComponentKind::EmissiveVolume,
        }
    }
    /// Whether `self` continues `other`'s animation run, so a capture keeps its
    /// start. An emissive run also needs an unchanged install serial: a
    /// brightness write clears the curve, so an identical re-install in the same
    /// drain is a new run although the curves compare equal.
    pub fn same_animation(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Light(a), Self::Light(b)) => a.animation == b.animation,
            (Self::Fog(a), Self::Fog(b)) => a.animation == b.animation,
            (Self::Emitter(a), Self::Emitter(b)) => a.spin_animation == b.spin_animation,
            (Self::Emissive(a), Self::Emissive(b)) => {
                a.animation == b.animation && a.animation_serial == b.animation_serial
            }
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

#[cfg(test)]
mod tests {
    use super::PresentationComponent;
    use crate::{EmissiveAnimation, EmissiveVolumeComponent};

    // P30: capture keeps an animation start only across the same run. A
    // brightness write then the identical curve re-installed in one drain
    // leaves equal curves but a new install serial: a new run.
    #[test]
    fn emissive_same_animation_requires_the_same_install_serial() {
        let curve = EmissiveAnimation {
            period_ms: 500.0,
            play_count: None,
            brightness: vec![0.0, 1.0],
        };
        let mut volume = EmissiveVolumeComponent::authored(0.0, 1.0, [1.0; 3]);
        volume.install_animation(curve.clone());
        let running = PresentationComponent::Emissive(volume.clone());

        let mut recoloured = volume.clone();
        recoloured.color = [1.0, 0.0, 0.0];
        assert!(PresentationComponent::Emissive(recoloured).same_animation(&running));

        volume.hold_brightness(0.0);
        volume.install_animation(curve);
        let reinstalled = PresentationComponent::Emissive(volume);
        assert!(!reinstalled.same_animation(&running));
    }
}
