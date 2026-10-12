//! Written presentation snapshots and client clock rebasing.

use postretro_entities::components::billboard_emitter::{BillboardEmitterComponent, SpinAnimation};
use postretro_entities::components::fog_volume::FogAnimation;
use postretro_entities::components::light::{LightAnimation, LightComponent};
use postretro_entities::presentation_binding::PresentationBindingResolver;
use postretro_entities::presentation_overrides::{PresentationComponent, PresentationOverride};
use postretro_entities::{EntityId, EntityRegistry, FogVolumeComponent};
use postretro_foundation::Vec3Lit;
use postretro_net::wire::{
    ComponentPayload, WireEmitterState, WireFogAnimation, WireFogState, WireLightAnimation,
    WireLightState, WireSpinAnimation,
};

/// Convert the immutable post-write snapshot, using the same gate as receive.
pub fn presentation_override_payload(record: &PresentationOverride) -> Option<ComponentPayload> {
    Some(match &record.component {
        PresentationComponent::Light(light) => {
            let state = WireLightState {
                intensity: light.intensity,
                color: light.color,
                cone_direction: light.cone_direction,
                animation: light
                    .animation
                    .as_ref()
                    .map(|animation| WireLightAnimation {
                        period_ms: animation.period_ms,
                        phase: animation.phase,
                        play_count: animation.play_count,
                        start_active: animation.start_active,
                        brightness: animation.brightness.clone(),
                        color: animation
                            .color
                            .as_ref()
                            .map(|keys| keys.iter().map(Vec3Lit::as_f32_3).collect()),
                        direction: animation
                            .direction
                            .as_ref()
                            .map(|keys| keys.iter().map(Vec3Lit::as_f32_3).collect()),
                    }),
                anim_start: record.anim_start,
            };
            state.validate().ok()?;
            ComponentPayload::LightState(state)
        }
        PresentationComponent::Fog(fog) => {
            let state = WireFogState {
                density: fog.density,
                glow: fog.glow,
                edge_softness: fog.edge_softness,
                falloff: fog.falloff,
                tint: fog.tint,
                saturation: fog.saturation,
                min_brightness: fog.min_brightness,
                light_range: fog.light_range,
                animation: fog.animation.as_ref().map(|animation| WireFogAnimation {
                    period_ms: animation.period_ms,
                    phase: animation.phase,
                    play_count: animation.play_count,
                    density: animation.density.clone(),
                    saturation: animation.saturation.clone(),
                    min_brightness: animation.min_brightness.clone(),
                    light_range: animation.light_range.clone(),
                }),
                anim_start: record.anim_start,
            };
            state.validate().ok()?;
            ComponentPayload::FogState(state)
        }
        PresentationComponent::Emitter(emitter) => {
            let state = WireEmitterState {
                rate: emitter.rate,
                spin_rate: emitter.spin_rate,
                spin_animation: emitter.spin_animation.as_ref().map(|animation| {
                    WireSpinAnimation {
                        duration: animation.duration,
                        rate_curve: animation.rate_curve.clone(),
                    }
                }),
                anim_start: record.anim_start,
            };
            state.validate().ok()?;
            ComponentPayload::EmitterState(state)
        }
    })
}

pub fn validate_presentation_override(record: &PresentationOverride) -> bool {
    presentation_override_payload(record).is_some()
}

/// Apply after display materialization. The caller memoizes the original wire
/// payload, never the locally rebased animation, only when this returns true.
pub(crate) fn apply_presentation_payload(
    registry: &mut EntityRegistry,
    id: EntityId,
    payload: &ComponentPayload,
    local_script_time: f64,
    host_now: f64,
    resolver: &mut dyn PresentationBindingResolver,
) -> bool {
    if !local_script_time.is_finite() || !host_now.is_finite() {
        return false;
    }
    let age = |start: f64| (host_now - start).max(0.0);
    match payload {
        ComponentPayload::LightState(state) => {
            if state.validate().is_err() {
                return false;
            }
            let Ok(mut light) = registry.get_component::<LightComponent>(id).cloned() else {
                return false;
            };
            light.intensity = state.intensity;
            light.color = state.color;
            light.cone_direction = state.cone_direction;
            light.animation = state.animation.as_ref().map(|animation| LightAnimation {
                period_ms: animation.period_ms,
                phase: animation.phase,
                play_count: animation.play_count,
                start_active: animation.start_active,
                brightness: animation.brightness.clone(),
                color: animation
                    .color
                    .as_ref()
                    .map(|keys| keys.iter().copied().map(Vec3Lit).collect()),
                direction: animation
                    .direction
                    .as_ref()
                    .map(|keys| keys.iter().copied().map(Vec3Lit).collect()),
                radius: None,
            });
            let mut start = None;
            if let Some(animation) = &mut light.animation {
                if animation.play_count.is_some_and(|count| count > 0) {
                    if animation.start_active != Some(false) {
                        start = state.anim_start.map(|value| local_script_time - age(value));
                    }
                } else {
                    let period = f64::from(animation.period_ms) / 1000.0;
                    animation.phase = Some(
                        (f64::from(animation.phase.unwrap_or(0.0))
                            + (host_now - local_script_time) / period)
                            .rem_euclid(1.0) as f32,
                    );
                }
            }
            if registry.set_component(id, light).is_err() {
                return false;
            }
            resolver.seed_light(registry, id, start);
        }
        ComponentPayload::FogState(state) => {
            if state.validate().is_err() {
                return false;
            }
            let Ok(mut fog) = registry.get_component::<FogVolumeComponent>(id).cloned() else {
                return false;
            };
            fog.density = state.density;
            fog.glow = state.glow;
            fog.edge_softness = state.edge_softness;
            fog.falloff = state.falloff;
            fog.tint = state.tint;
            fog.saturation = state.saturation;
            fog.min_brightness = state.min_brightness;
            fog.light_range = state.light_range;
            fog.animation = state.animation.as_ref().map(|animation| FogAnimation {
                period_ms: animation.period_ms,
                phase: animation.phase,
                play_count: animation.play_count,
                density: animation.density.clone(),
                saturation: animation.saturation.clone(),
                min_brightness: animation.min_brightness.clone(),
                light_range: animation.light_range.clone(),
            });
            if registry.set_component(id, fog).is_err() {
                return false;
            }
            resolver.seed_fog(
                registry,
                id,
                state.anim_start.map(|value| local_script_time - age(value)),
            );
        }
        ComponentPayload::EmitterState(state) => {
            if state.validate().is_err() {
                return false;
            }
            let Ok(mut emitter) = registry
                .get_component::<BillboardEmitterComponent>(id)
                .cloned()
            else {
                return false;
            };
            emitter.rate = state.rate;
            emitter.spin_rate = state.spin_rate;
            emitter.spin_animation = state
                .spin_animation
                .as_ref()
                .map(|animation| SpinAnimation {
                    duration: animation.duration,
                    rate_curve: animation.rate_curve.clone(),
                });
            if registry.set_component(id, emitter).is_err() {
                return false;
            }
            resolver.seed_emitter(registry, id, state.anim_start.map(age));
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use postretro_entities::components::light::LightKind;
    use postretro_entities::presentation_binding::PresentationBindingKey;
    use postretro_entities::{ComponentKind, Transform};
    use postretro_foundation::FalloffKind;
    use postretro_net::wire::MAX_WIRE_CURVE_KEYS;

    #[derive(Default)]
    struct Seeds(Vec<(ComponentKind, EntityId, Option<f64>)>);
    impl PresentationBindingResolver for Seeds {
        fn resolve(
            &self,
            _: &EntityRegistry,
            _: PresentationBindingKey,
        ) -> Option<(EntityId, Vec3)> {
            None
        }
        fn key_for_entity(
            &self,
            _: &EntityRegistry,
            _: EntityId,
        ) -> Option<(PresentationBindingKey, Vec3)> {
            None
        }
        fn seed_light(
            &mut self,
            registry: &EntityRegistry,
            id: EntityId,
            start: Option<f64>,
        ) -> bool {
            assert!(registry.get_component::<LightComponent>(id).is_ok());
            self.0.push((ComponentKind::Light, id, start));
            false // Application success does not depend on bridge ownership.
        }
        fn seed_fog(
            &mut self,
            registry: &EntityRegistry,
            id: EntityId,
            start: Option<f64>,
        ) -> bool {
            assert!(registry.get_component::<FogVolumeComponent>(id).is_ok());
            self.0.push((ComponentKind::FogVolume, id, start));
            true
        }
        fn seed_emitter(
            &mut self,
            registry: &EntityRegistry,
            id: EntityId,
            elapsed: Option<f64>,
        ) -> bool {
            assert!(
                registry
                    .get_component::<BillboardEmitterComponent>(id)
                    .is_ok()
            );
            self.0.push((ComponentKind::BillboardEmitter, id, elapsed));
            true
        }
    }

    fn light() -> LightComponent {
        LightComponent {
            origin: [4.0, 5.0, 6.0],
            light_type: LightKind::Spot,
            intensity: 2.0,
            color: [0.2, 0.4, 0.6],
            falloff_model: FalloffKind::Linear,
            falloff_range: 12.0,
            cone_angle_inner: Some(0.2),
            cone_angle_outer: Some(0.8),
            cone_direction: Some([0.0, -1.0, 0.0]),
            is_dynamic: true,
            animated_slot: Some(7),
            follow_transform: true,
            carrier: None,
            animation: Some(LightAnimation {
                period_ms: 2000.0,
                phase: Some(0.25),
                play_count: Some(3),
                start_active: Some(true),
                brightness: Some(vec![0.25, 1.0]),
                color: Some(vec![Vec3Lit([1.0, 0.0, 0.0])]),
                direction: Some(vec![Vec3Lit([0.0, -1.0, 0.0])]),
                radius: None,
            }),
        }
    }
    fn fog() -> FogVolumeComponent {
        FogVolumeComponent {
            density: 0.3,
            glow: 0.2,
            edge_softness: 0.5,
            falloff: 0.7,
            tint: [0.2, 0.4, 0.6],
            saturation: 0.8,
            min_brightness: 0.1,
            light_range: 4.0,
            animation: Some(FogAnimation {
                period_ms: 5000.0,
                phase: Some(0.25),
                play_count: Some(1),
                density: Some(vec![0.3, 0.6]),
                saturation: Some(vec![1.0, 0.0]),
                min_brightness: Some(vec![0.1, 0.2]),
                light_range: Some(vec![4.0, 2.0]),
            }),
        }
    }
    fn emitter() -> BillboardEmitterComponent {
        BillboardEmitterComponent {
            rate: 5.0,
            burst: Some(4),
            spread: 0.3,
            lifetime: 3.0,
            velocity: [0.0, 1.0, 0.0],
            buoyancy: 0.2,
            drag: 0.1,
            size_over_lifetime: [0.5, 1.0].into(),
            opacity_over_lifetime: [1.0, 0.0].into(),
            color: [1.0, 0.5, 0.0],
            sprite: "smoke".into(),
            spin_rate: 2.0,
            spin_animation: Some(SpinAnimation {
                duration: 5.0,
                rate_curve: vec![0.0, 2.0],
            }),
        }
    }
    fn record(component: PresentationComponent, anim_start: Option<f64>) -> PresentationOverride {
        PresentationOverride {
            component,
            anim_start,
        }
    }
    fn spawn(registry: &mut EntityRegistry) -> EntityId {
        registry.try_spawn(Transform::default(), &[]).unwrap()
    }
    fn apply(
        registry: &mut EntityRegistry,
        id: EntityId,
        record: &PresentationOverride,
        local: f64,
        host: f64,
        seeds: &mut Seeds,
    ) -> bool {
        apply_presentation_payload(
            registry,
            id,
            &presentation_override_payload(record).unwrap(),
            local,
            host,
            seeds,
        )
    }

    #[test]
    fn presentation_state_conversion_roundtrips_all_written_fields_and_preserves_placement() {
        let mut registry = EntityRegistry::new();
        let id = spawn(&mut registry);
        let mut local_light = light();
        local_light.origin = [9.0; 3];
        local_light.falloff_range = 99.0;
        local_light.animated_slot = Some(42);
        local_light.intensity = 0.0;
        let mut local_emitter = emitter();
        local_emitter.rate = 0.0;
        local_emitter.sprite = "local".into();
        local_emitter.spread = 9.0;
        registry.set_component(id, local_light.clone()).unwrap();
        registry.set_component(id, local_emitter.clone()).unwrap();
        registry.set_component(id, fog()).unwrap();
        let mut seeds = Seeds::default();
        for component in [
            PresentationComponent::Light(light()),
            PresentationComponent::Fog(fog()),
            PresentationComponent::Emitter(emitter()),
        ] {
            let before = record(component, Some(8.0));
            assert!(validate_presentation_override(&before));
            assert!(apply(&mut registry, id, &before, 20.0, 10.0, &mut seeds));
            let after = record(
                PresentationComponent::snapshot(&registry, id, before.component.kind()).unwrap(),
                Some(8.0),
            );
            assert_eq!(
                presentation_override_payload(&before),
                presentation_override_payload(&after)
            );
        }
        let applied = registry.get_component::<LightComponent>(id).unwrap();
        assert_eq!(applied.origin, local_light.origin);
        assert_eq!(applied.falloff_range, 99.0);
        assert_eq!(applied.animated_slot, Some(42));
        assert_eq!(applied.light_type, local_light.light_type);
        assert_eq!(applied.follow_transform, local_light.follow_transform);
        assert_eq!(applied.cone_angle_outer, local_light.cone_angle_outer);
        let applied = registry
            .get_component::<BillboardEmitterComponent>(id)
            .unwrap();
        assert_eq!(applied.sprite, "local");
        assert_eq!(applied.spread, 9.0);
        assert_eq!(
            seeds.0,
            vec![
                (ComponentKind::Light, id, Some(18.0)),
                (ComponentKind::FogVolume, id, Some(18.0)),
                (ComponentKind::BillboardEmitter, id, Some(2.0))
            ]
        );
    }

    #[test]
    fn presentation_state_age_clamps_future_starts_and_none_still_seeds_cached_values() {
        let mut registry = EntityRegistry::new();
        let id = spawn(&mut registry);
        registry.set_component(id, light()).unwrap();
        registry.set_component(id, fog()).unwrap();
        registry.set_component(id, emitter()).unwrap();
        let mut seeds = Seeds::default();
        for start in [Some(12.0), None] {
            for component in [
                PresentationComponent::Light(light()),
                PresentationComponent::Fog(fog()),
                PresentationComponent::Emitter(emitter()),
            ] {
                assert!(apply(
                    &mut registry,
                    id,
                    &record(component, start),
                    20.0,
                    10.0,
                    &mut seeds
                ));
            }
        }
        assert_eq!(
            seeds.0.iter().map(|seed| seed.2).collect::<Vec<_>>(),
            vec![Some(20.0), Some(20.0), Some(0.0), None, None, None]
        );
    }

    #[test]
    fn presentation_state_looping_light_rebases_phase_even_without_start() {
        let mut registry = EntityRegistry::new();
        let id = spawn(&mut registry);
        registry.set_component(id, light()).unwrap();
        let mut seeds = Seeds::default();
        for (count, start) in [(None, None), (Some(0), Some(2.0))] {
            let mut source = light();
            source.animation.as_mut().unwrap().play_count = count;
            let record = record(PresentationComponent::Light(source), start);
            let original = presentation_override_payload(&record).unwrap();
            assert!(apply_presentation_payload(
                &mut registry,
                id,
                &original,
                10.0,
                13.0,
                &mut seeds
            ));
            assert_eq!(
                registry
                    .get_component::<LightComponent>(id)
                    .unwrap()
                    .animation
                    .as_ref()
                    .unwrap()
                    .phase,
                Some(0.75)
            );
            assert_eq!(original, presentation_override_payload(&record).unwrap());
            assert_eq!(seeds.0.last().unwrap().2, None);
        }
        let mut source = light();
        source.animation.as_mut().unwrap().play_count = None;
        assert!(apply(
            &mut registry,
            id,
            &record(PresentationComponent::Light(source), None),
            13.0,
            10.0,
            &mut seeds
        ));
        assert_eq!(
            registry
                .get_component::<LightComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .phase,
            Some(0.75)
        );
    }

    #[test]
    fn presentation_state_zero_fog_play_count_is_refused_without_mutation_or_seed() {
        // Regression: validated wire fog could install zero and panic in the CPU bridge.
        let mut registry = EntityRegistry::new();
        let id = spawn(&mut registry);
        let original = fog();
        registry.set_component(id, original.clone()).unwrap();
        let mut seeds = Seeds::default();
        let source = record(PresentationComponent::Fog(fog()), Some(8.0));
        let ComponentPayload::FogState(mut invalid) =
            presentation_override_payload(&source).unwrap()
        else {
            unreachable!()
        };
        invalid.density = 0.9;
        invalid.animation.as_mut().unwrap().play_count = Some(0);
        assert!(!apply_presentation_payload(
            &mut registry,
            id,
            &ComponentPayload::FogState(invalid),
            20.0,
            10.0,
            &mut seeds
        ));
        assert_eq!(
            registry.get_component::<FogVolumeComponent>(id).unwrap(),
            &original
        );
        assert!(seeds.0.is_empty());

        let mut invalid_producer = original.clone();
        invalid_producer.animation.as_mut().unwrap().play_count = Some(0);
        assert!(
            presentation_override_payload(&record(
                PresentationComponent::Fog(invalid_producer),
                Some(8.0)
            ))
            .is_none()
        );

        for count in [Some(1), None] {
            let mut valid = fog();
            valid.density = 0.7;
            valid.animation.as_mut().unwrap().play_count = count;
            assert!(apply(
                &mut registry,
                id,
                &record(PresentationComponent::Fog(valid.clone()), Some(8.0)),
                20.0,
                10.0,
                &mut seeds
            ));
            assert_eq!(
                registry.get_component::<FogVolumeComponent>(id).unwrap(),
                &valid
            );
            assert_eq!(
                seeds.0.last(),
                Some(&(ComponentKind::FogVolume, id, Some(18.0)))
            );
        }
    }

    #[test]
    fn presentation_state_inactive_finite_light_does_not_seed_start() {
        let mut registry = EntityRegistry::new();
        let id = spawn(&mut registry);
        registry.set_component(id, light()).unwrap();
        let mut seeds = Seeds::default();
        let mut source = light();
        source.animation.as_mut().unwrap().start_active = Some(false);
        assert!(apply(
            &mut registry,
            id,
            &record(PresentationComponent::Light(source), Some(1.0)),
            20.0,
            10.0,
            &mut seeds
        ));
        assert_eq!(seeds.0, vec![(ComponentKind::Light, id, None)]);
    }

    #[test]
    fn presentation_state_missing_components_are_skipped_without_seed() {
        let mut registry = EntityRegistry::new();
        let id = spawn(&mut registry);
        let mut seeds = Seeds::default();
        for component in [
            PresentationComponent::Light(light()),
            PresentationComponent::Fog(fog()),
            PresentationComponent::Emitter(emitter()),
        ] {
            assert!(!apply(
                &mut registry,
                id,
                &record(component, None),
                0.0,
                0.0,
                &mut seeds
            ));
        }
        assert!(seeds.0.is_empty());
        assert!(registry.get_component::<LightComponent>(id).is_err());
        assert!(registry.get_component::<FogVolumeComponent>(id).is_err());
        assert!(
            registry
                .get_component::<BillboardEmitterComponent>(id)
                .is_err()
        );
    }

    #[test]
    fn presentation_state_static_values_clear_animations_and_invalid_receive_does_not_write() {
        let mut registry = EntityRegistry::new();
        let id = spawn(&mut registry);
        registry.set_component(id, light()).unwrap();
        registry.set_component(id, fog()).unwrap();
        registry.set_component(id, emitter()).unwrap();
        let mut seeds = Seeds::default();
        let mut light = light();
        light.animation = None;
        let mut fog = fog();
        fog.animation = None;
        let mut emitter = emitter();
        emitter.spin_animation = None;
        for component in [
            PresentationComponent::Light(light.clone()),
            PresentationComponent::Fog(fog),
            PresentationComponent::Emitter(emitter),
        ] {
            assert!(apply(
                &mut registry,
                id,
                &record(component.clone(), None),
                1.0,
                1.0,
                &mut seeds
            ));
            assert_eq!(
                PresentationComponent::snapshot(&registry, id, component.kind()),
                Some(component)
            );
        }
        assert!(seeds.0.iter().all(|entry| entry.2.is_none()));
        let ComponentPayload::LightState(mut invalid) = presentation_override_payload(&record(
            PresentationComponent::Light(light.clone()),
            None,
        ))
        .unwrap() else {
            unreachable!()
        };
        invalid.intensity = f32::NAN;
        assert!(!apply_presentation_payload(
            &mut registry,
            id,
            &ComponentPayload::LightState(invalid),
            1.0,
            1.0,
            &mut seeds
        ));
        assert_eq!(
            registry.get_component::<LightComponent>(id).unwrap(),
            &light
        );
        assert_eq!(seeds.0.len(), 3);
    }

    #[test]
    fn presentation_state_validation_rejects_nonfinite_and_invalid_curves() {
        let mut source = light();
        source.animation.as_mut().unwrap().brightness = Some(vec![1.0; MAX_WIRE_CURVE_KEYS]);
        assert!(validate_presentation_override(&record(
            PresentationComponent::Light(source.clone()),
            None
        )));
        source
            .animation
            .as_mut()
            .unwrap()
            .brightness
            .as_mut()
            .unwrap()
            .push(1.0);
        assert!(!validate_presentation_override(&record(
            PresentationComponent::Light(source),
            None
        )));
        let mut source = fog();
        source.animation.as_mut().unwrap().density = Some(vec![]);
        assert!(!validate_presentation_override(&record(
            PresentationComponent::Fog(source),
            None
        )));
        let mut source = emitter();
        source.spin_animation.as_mut().unwrap().duration = 0.0;
        assert!(!validate_presentation_override(&record(
            PresentationComponent::Emitter(source),
            None
        )));
        assert!(!validate_presentation_override(&record(
            PresentationComponent::Light(light()),
            Some(f64::NAN)
        )));
        let mut source = light();
        source.animation.as_mut().unwrap().direction =
            Some(vec![Vec3Lit([f32::INFINITY, 0.0, 0.0])]);
        assert!(!validate_presentation_override(&record(
            PresentationComponent::Light(source),
            None
        )));
        let mut source = fog();
        source.tint[1] = f32::NAN;
        assert!(!validate_presentation_override(&record(
            PresentationComponent::Fog(source),
            None
        )));
        let mut source = emitter();
        source.spin_rate = f32::INFINITY;
        assert!(!validate_presentation_override(&record(
            PresentationComponent::Emitter(source),
            None
        )));
    }
}
