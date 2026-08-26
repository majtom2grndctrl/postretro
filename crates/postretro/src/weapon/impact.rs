// Weapon impact effect, routed through one spawn chokepoint.
// See: context/lib/entity_model.md §5

use std::sync::LazyLock;

use glam::Vec3;
use postretro_entities::FalloffKind;
use postretro_entities::components::billboard_emitter::LifetimeCurve;
use postretro_entities::components::light::{LightAnimation, LightComponent, LightKind};
use postretro_entities::components::particle::ParticleState;
use postretro_entities::components::sprite_visual::SpriteVisual;
use postretro_entities::registry::{EntityRegistry, Transform};
use postretro_foundation::ProjectileImpactLight;

use crate::impact_effects;

const IMPACT_SPRITE_COLLECTION: &str = "impact";

// Impact curves are identical for every burst particle and every shot. Build
// each `Arc<[f32]>` once and hand every spawned particle a refcount bump
// (`LifetimeCurve::clone`) instead of allocating a fresh `Vec` per particle —
// the same curve-sharing model the emitter/particle hot paths use.
static IMPACT_SIZE_CURVE: LazyLock<LifetimeCurve> =
    LazyLock::new(|| LifetimeCurve::from([0.18, 0.12, 0.0]));
static IMPACT_OPACITY_CURVE: LazyLock<LifetimeCurve> =
    LazyLock::new(|| LifetimeCurve::from([1.0, 0.7, 0.0]));
const IMPACT_LIFETIME: f32 = 0.18;
const IMPACT_PARTICLE_COUNT: usize = 9;
const SURFACE_OFFSET: f32 = 0.03;

pub(crate) fn sprite_collection() -> &'static str {
    IMPACT_SPRITE_COLLECTION
}

pub(crate) fn lifetime() -> f32 {
    IMPACT_LIFETIME
}

/// Spawn the M10 default world-hit burst at `point`, oriented to eject away
/// from `normal`. Future data-defined impact descriptors replace the body of
/// this function; callers stay on this named effect chokepoint.
pub(crate) fn spawn_impact_effect_at(registry: &mut EntityRegistry, point: Vec3, normal: Vec3) {
    let (normal, tangent, bitangent) = impact_frame(normal);
    let origin = point + normal * SURFACE_OFFSET;

    for index in 0..IMPACT_PARTICLE_COUNT {
        let angle = std::f32::consts::TAU * index as f32 / IMPACT_PARTICLE_COUNT as f32;
        let ring = tangent * angle.cos() + bitangent * angle.sin();
        let fan = if index == 0 {
            normal
        } else {
            (normal * 0.82 + ring * 0.58).normalize_or_zero()
        };
        let speed = 4.5 + (index % 3) as f32 * 1.35;
        spawn_particle(registry, origin, fan * speed, index);
    }
}

/// Materialize the descriptor-owned transient flash beside the ordinary impact
/// particles. The bridge reads non-following light positions from `origin`, so
/// keep it equal to the hit-point Transform instead of relying on the entity pose.
pub(crate) fn spawn_projectile_impact_light(
    registry: &mut EntityRegistry,
    point: Vec3,
    config: &ProjectileImpactLight,
) {
    let Some(id) = registry.try_spawn(
        Transform {
            position: point,
            ..Transform::default()
        },
        &[],
    ) else {
        log::warn!("[WeaponImpact] entity registry exhausted; dropping impact flash");
        return;
    };

    let _ = registry.set_component(
        id,
        LightComponent {
            origin: point.to_array(),
            light_type: LightKind::Point,
            intensity: config.intensity,
            color: config.color,
            falloff_model: FalloffKind::InverseSquared,
            falloff_range: config.radius,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            is_dynamic: true,
            animated_slot: None,
            follow_transform: false,
            carrier: None,
            animation: Some(LightAnimation {
                period_ms: config.fade_ms,
                phase: None,
                play_count: Some(1),
                start_active: None,
                brightness: Some(vec![1.0, 0.0]),
                color: None,
                direction: None,
                radius: config
                    .peak_radius
                    .map(|peak_radius| vec![config.radius, peak_radius]),
            }),
        },
    );
    impact_effects::despawn(registry, id, Some(config.fade_ms));
}

fn impact_frame(normal: Vec3) -> (Vec3, Vec3, Vec3) {
    let normal = normal.normalize_or_zero();
    let normal = if normal == Vec3::ZERO {
        Vec3::Y
    } else {
        normal
    };
    let helper = if normal.y.abs() < 0.9 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let tangent = helper.cross(normal).normalize_or_zero();
    let tangent = if tangent == Vec3::ZERO {
        Vec3::Z
    } else {
        tangent
    };
    let bitangent = normal.cross(tangent).normalize_or_zero();
    (normal, tangent, bitangent)
}

fn spawn_particle(registry: &mut EntityRegistry, position: Vec3, velocity: Vec3, index: usize) {
    let Some(id) = registry.try_spawn(
        Transform {
            position,
            ..Transform::default()
        },
        &[],
    ) else {
        log::warn!("[WeaponImpact] entity registry exhausted; dropping impact particle");
        return;
    };

    let lifetime = IMPACT_LIFETIME * (0.8 + index as f32 * 0.025);
    let particle = ParticleState {
        velocity: velocity.to_array(),
        age: 0.0,
        lifetime,
        buoyancy: 0.0,
        drag: 4.0,
        size_curve: IMPACT_SIZE_CURVE.clone(),
        opacity_curve: IMPACT_OPACITY_CURVE.clone(),
        emitter: None,
    };
    let visual = SpriteVisual {
        sprite: IMPACT_SPRITE_COLLECTION.to_string(),
        size: 0.0,
        opacity: 0.0,
        rotation: index as f32 * 0.73,
        tint: [1.0, 0.88, 0.45],
    };

    let _ = registry.set_component(id, particle);
    let _ = registry.set_component(id, visual);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting_systems::particle_sim;
    use postretro_entities::components::light::LightComponent;
    use postretro_entities::registry::{ComponentKind, ComponentValue};
    use postretro_foundation::ProjectileImpactLight;

    const EPSILON: f32 = 1.0e-5;

    fn count_particles(registry: &EntityRegistry) -> usize {
        registry
            .iter_with_kind(ComponentKind::ParticleState)
            .count()
    }

    #[test]
    fn impact_chokepoint_spawns_particles_oriented_from_surface_normal() {
        let mut registry = EntityRegistry::new();
        let point = Vec3::new(1.0, 2.0, 3.0);
        let normal = Vec3::Z;

        spawn_impact_effect_at(&mut registry, point, normal);

        assert_eq!(count_particles(&registry), IMPACT_PARTICLE_COUNT);
        assert!(
            registry
                .iter_with_kind(ComponentKind::BillboardEmitter)
                .next()
                .is_none(),
            "impact burst should not leave behind a persistent emitter"
        );

        for (id, value) in registry.iter_with_kind(ComponentKind::ParticleState) {
            let ComponentValue::ParticleState(particle) = value else {
                continue;
            };
            let transform = registry.get_component::<Transform>(id).unwrap();
            assert!(
                (transform.position - (point + normal * SURFACE_OFFSET)).length() < EPSILON,
                "impact particles should spawn just off the hit surface"
            );
            let velocity = Vec3::from_array(particle.velocity);
            assert!(
                velocity.dot(normal) > 0.0,
                "impact velocity should point away from surface normal: {velocity:?}"
            );
        }
    }

    #[test]
    fn impact_particles_clean_up_after_lifetime() {
        let mut registry = EntityRegistry::new();
        spawn_impact_effect_at(&mut registry, Vec3::ZERO, Vec3::Y);

        let mut live_counts = std::collections::HashMap::new();
        particle_sim::tick(
            &mut registry,
            IMPACT_LIFETIME * 2.0,
            -9.81,
            &mut live_counts,
        );

        assert_eq!(
            count_particles(&registry),
            0,
            "impact particles should despawn through the particle sim"
        );
    }

    #[test]
    fn zero_normal_falls_back_to_upward_burst() {
        let mut registry = EntityRegistry::new();
        spawn_impact_effect_at(&mut registry, Vec3::ZERO, Vec3::ZERO);

        for (_id, value) in registry.iter_with_kind(ComponentKind::ParticleState) {
            let ComponentValue::ParticleState(particle) = value else {
                continue;
            };
            let velocity = Vec3::from_array(particle.velocity);
            assert!(
                velocity.y > 0.0,
                "zero normal should produce upward impact velocity: {velocity:?}"
            );
        }
    }

    #[test]
    fn impact_flash_without_peak_keeps_its_authored_radius_static() {
        let mut registry = EntityRegistry::new();
        let config = ProjectileImpactLight {
            color: [0.6, 0.9, 1.0],
            intensity: 3.0,
            radius: 4.5,
            peak_radius: None,
            fade_ms: 150.0,
        };

        spawn_projectile_impact_light(&mut registry, Vec3::new(2.0, 3.0, 4.0), &config);

        let flashes = registry
            .iter_with_kind(ComponentKind::Light)
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        let [flash] = flashes.as_slice() else {
            panic!("impact flash creates exactly one point light");
        };
        let light = registry
            .get_component::<LightComponent>(*flash)
            .expect("flash light attaches");
        assert_eq!(light.origin, [2.0, 3.0, 4.0]);
        assert!((light.falloff_range - config.radius).abs() <= f32::EPSILON);
        assert_eq!(light.animation.as_ref().unwrap().radius, None);
    }
}
