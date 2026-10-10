//! App bridge adapter for stable authored presentation identities.

use super::{fog_volume_bridge::FogVolumeBridge, light_bridge::LightBridge};
use glam::Vec3;
use postretro_entities::{
    Component, EntityId, EntityRegistry,
    components::map_ordinal::MapOrdinal,
    presentation_binding::{
        PresentationBindingKey, PresentationBindingKind, PresentationBindingResolver,
    },
};
use postretro_level_format::map_entity::MapEntityRecord;

pub(crate) struct PresentationBindings<'a> {
    pub(crate) lights: &'a LightBridge,
    pub(crate) fog: &'a FogVolumeBridge,
    pub(crate) map_entities: &'a [MapEntityRecord],
}

impl PresentationBindingResolver for PresentationBindings<'_> {
    fn resolve(
        &self,
        registry: &EntityRegistry,
        key: PresentationBindingKey,
    ) -> Option<(EntityId, Vec3)> {
        let index = key.index as usize;
        let result = match key.kind {
            PresentationBindingKind::MapEntity => {
                let origin = Vec3::from(self.map_entities.get(index)?.origin);
                let id = registry
                    .iter_with_kind(MapOrdinal::KIND)
                    .map(|(id, _)| id)
                    .find(|id| {
                        registry.get_component::<MapOrdinal>(*id).ok()
                            == Some(&MapOrdinal(key.index))
                    })?;
                (id, origin)
            }
            PresentationBindingKind::MapLight => {
                let id = self.lights.entity_for_map_index(index)?;
                (id, self.lights.map_origin_for_entity(id)?)
            }
            PresentationBindingKind::FogVolume => {
                let id = self.fog.entity_for_map_index(index)?;
                (id, self.fog.map_origin_for_entity(id)?)
            }
        };
        registry.exists(result.0).then_some(result)
    }

    fn key_for_entity(
        &self,
        registry: &EntityRegistry,
        entity: EntityId,
    ) -> Option<(PresentationBindingKey, Vec3)> {
        if !registry.exists(entity) {
            return None;
        }
        let (kind, index, origin) =
            if let Ok(ordinal) = registry.get_component::<MapOrdinal>(entity) {
                (
                    PresentationBindingKind::MapEntity,
                    ordinal.0,
                    Vec3::from(self.map_entities.get(ordinal.0 as usize)?.origin),
                )
            } else if let Some(index) = self.lights.map_index_for_entity(entity) {
                (
                    PresentationBindingKind::MapLight,
                    index as u32,
                    self.lights.map_origin_for_entity(entity)?,
                )
            } else {
                let index = self.fog.map_index_for_entity(entity)?;
                (
                    PresentationBindingKind::FogVolume,
                    index as u32,
                    self.fog.map_origin_for_entity(entity)?,
                )
            };
        Some((PresentationBindingKey { kind, index }, origin))
    }
}

/// Mutable receive adapter also seeds the presentation bridges before their
/// same-frame update. Identity still comes from the immutable authored roster.
pub(crate) struct PresentationBindingsMut<'a> {
    pub(crate) lights: &'a mut LightBridge,
    pub(crate) fog: &'a mut FogVolumeBridge,
    pub(crate) emitters: &'a mut super::emitter_bridge::EmitterBridge,
    pub(crate) map_entities: &'a [MapEntityRecord],
}
impl PresentationBindingResolver for PresentationBindingsMut<'_> {
    fn resolve(
        &self,
        registry: &EntityRegistry,
        key: PresentationBindingKey,
    ) -> Option<(EntityId, Vec3)> {
        PresentationBindings {
            lights: self.lights,
            fog: self.fog,
            map_entities: self.map_entities,
        }
        .resolve(registry, key)
    }
    fn key_for_entity(
        &self,
        registry: &EntityRegistry,
        entity: EntityId,
    ) -> Option<(PresentationBindingKey, Vec3)> {
        PresentationBindings {
            lights: self.lights,
            fog: self.fog,
            map_entities: self.map_entities,
        }
        .key_for_entity(registry, entity)
    }
    fn seed_light(&mut self, registry: &EntityRegistry, id: EntityId, start: Option<f64>) -> bool {
        self.lights.seed_replicated_state(registry, id, start)
    }
    fn seed_fog(&mut self, registry: &EntityRegistry, id: EntityId, start: Option<f64>) -> bool {
        self.fog.seed_replicated_state(registry, id, start)
    }
    fn seed_emitter(
        &mut self,
        registry: &EntityRegistry,
        id: EntityId,
        elapsed: Option<f64>,
    ) -> bool {
        self.emitters.seed_replicated_state(registry, id, elapsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::Transform;
    use postretro_entities::components::light::{FalloffKind, LightComponent, LightKind};
    use postretro_level_format::fog_volumes::FogVolumeRecord;
    use postretro_level_loader::{FalloffModel, LightType, MapLight, ShadowType};

    fn light() -> MapLight {
        MapLight {
            origin: [1.0, 2.0, 3.0],
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0; 3],
            falloff_model: FalloffModel::InverseSquared,
            falloff_range: 10.0,
            cone_angle_inner: 0.0,
            cone_angle_outer: 0.0,
            cone_direction: [0.0; 3],
            is_dynamic: true,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: vec![],
            cell_index: 0,
            shadow_type: ShadowType::StaticLightMap,
        }
    }

    fn fog() -> FogVolumeRecord {
        FogVolumeRecord {
            min: [-1.0; 3],
            max: [1.0; 3],
            center: [0.0; 3],
            inv_half_ext: [1.0; 3],
            half_diag: 3.0_f32.sqrt(),
            shape_mode: 0.0,
            plane_count: 0,
            planes: vec![],
            density: 0.5,
            edge_softness: 1.0,
            glow: 0.0,
            radial_falloff: 2.0,
            tint: [1.0; 3],
            saturation: 1.0,
            min_brightness: 0.0,
            light_range: 1.0,
            anisotropy: 0.0,
            ambient_scatter: 1.0,
            tags: vec![],
        }
    }

    #[test]
    fn presentation_binding_actual_bridges_roundtrip_authored_identity_after_live_changes() {
        let mut registry = EntityRegistry::new();
        let mut lights = LightBridge::new();
        let mut fog_bridge = FogVolumeBridge::new();
        lights.populate_from_level(&[light()], &mut registry, 0);
        fog_bridge.populate_from_level(&mut registry, &[fog()]);
        let map_entities = [MapEntityRecord {
            origin: [4.0, 5.0, 6.0],
            classname: "light_only".into(),
            ..Default::default()
        }];
        let descriptor_light = registry.spawn(Transform {
            position: Vec3::splat(90.0),
            ..Default::default()
        });
        registry
            .set_component(descriptor_light, MapOrdinal(0))
            .unwrap();
        registry
            .set_component(
                descriptor_light,
                LightComponent {
                    origin: [90.0; 3],
                    light_type: LightKind::Point,
                    intensity: 1.0,
                    color: [1.0; 3],
                    falloff_model: FalloffKind::InverseSquared,
                    falloff_range: 2.0,
                    cone_angle_inner: None,
                    cone_angle_outer: None,
                    cone_direction: None,
                    is_dynamic: true,
                    animated_slot: None,
                    follow_transform: false,
                    carrier: None,
                    animation: None,
                },
            )
            .unwrap();
        lights.absorb_dynamic_lights(&registry);
        let light_id = lights.entity_for_map_index(0).unwrap();
        let fog_id = fog_bridge.entity_for_map_index(0).unwrap();
        registry
            .set_component(
                light_id,
                Transform {
                    position: Vec3::splat(80.0),
                    ..Default::default()
                },
            )
            .unwrap();
        registry
            .set_component(
                fog_id,
                Transform {
                    position: Vec3::splat(70.0),
                    ..Default::default()
                },
            )
            .unwrap();
        let resolver = PresentationBindings {
            lights: &lights,
            fog: &fog_bridge,
            map_entities: &map_entities,
        };
        for (entity, kind, origin) in [
            (
                light_id,
                PresentationBindingKind::MapLight,
                Vec3::new(1.0, 2.0, 3.0),
            ),
            (fog_id, PresentationBindingKind::FogVolume, Vec3::ZERO),
            (
                descriptor_light,
                PresentationBindingKind::MapEntity,
                Vec3::new(4.0, 5.0, 6.0),
            ),
        ] {
            let (key, actual_origin) = resolver.key_for_entity(&registry, entity).unwrap();
            assert_eq!(key, PresentationBindingKey { kind, index: 0 });
            assert!(actual_origin.distance(origin) < 1e-5);
            let (resolved, resolved_origin) = resolver.resolve(&registry, key).unwrap();
            assert_eq!(resolved, entity);
            assert!(resolved_origin.distance(origin) < 1e-5);
            assert!(
                resolver
                    .resolve(&registry, PresentationBindingKey { kind, index: 1 })
                    .is_none()
            );
        }
        assert_eq!(lights.map_index_for_entity(descriptor_light), None);
        assert_eq!(lights.entity_for_map_index(1), None);
    }

    #[test]
    fn presentation_binding_rejects_stale_authored_ids_and_unenrolled_replacements() {
        let mut registry = EntityRegistry::new();
        let mut lights = LightBridge::new();
        let mut fog_bridge = FogVolumeBridge::new();
        lights.populate_from_level(&[light()], &mut registry, 0);
        fog_bridge.populate_from_level(&mut registry, &[fog()]);
        let light_id = lights.entity_for_map_index(0).unwrap();
        let fog_id = fog_bridge.entity_for_map_index(0).unwrap();
        let map_id = registry.spawn(Transform::default());
        registry.set_component(map_id, MapOrdinal(0)).unwrap();
        let map_entities = [MapEntityRecord::default()];
        let resolver = PresentationBindings {
            lights: &lights,
            fog: &fog_bridge,
            map_entities: &map_entities,
        };
        for (id, kind) in [
            (light_id, PresentationBindingKind::MapLight),
            (fog_id, PresentationBindingKind::FogVolume),
            (map_id, PresentationBindingKind::MapEntity),
        ] {
            registry.despawn(id).unwrap();
            let replacement = registry.spawn(Transform::default());
            assert!(
                resolver
                    .resolve(&registry, PresentationBindingKey { kind, index: 0 })
                    .is_none()
            );
            assert!(resolver.key_for_entity(&registry, id).is_none());
            assert!(resolver.key_for_entity(&registry, replacement).is_none());
        }
        let bad_ordinal = registry.spawn(Transform::default());
        registry.set_component(bad_ordinal, MapOrdinal(4)).unwrap();
        assert!(resolver.key_for_entity(&registry, bad_ordinal).is_none());
    }
}
