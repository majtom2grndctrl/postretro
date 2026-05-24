// Weapon impact effect, routed through one spawn chokepoint.
// See: context/lib/entity_model.md §5

use glam::Vec3;

use crate::scripting::components::particle::ParticleState;
use crate::scripting::components::sprite_visual::SpriteVisual;
use crate::scripting::registry::{EntityRegistry, Transform};

const IMPACT_SPRITE_COLLECTION: &str = "impact";
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
        size_curve: vec![0.18, 0.12, 0.0],
        opacity_curve: vec![1.0, 0.7, 0.0],
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
    use crate::scripting::registry::{ComponentKind, ComponentValue};
    use crate::scripting_systems::particle_sim;

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

        particle_sim::tick(&mut registry, IMPACT_LIFETIME * 2.0, -9.81);

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
}
