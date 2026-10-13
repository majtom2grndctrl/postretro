// Weapon impact effect, routed through one spawn chokepoint.
// See: context/lib/entity_model.md §5

use std::sync::LazyLock;

use glam::Vec3;
use postretro_entities::FalloffKind;
use postretro_entities::components::light::{LightAnimation, LightComponent, LightKind};
use postretro_entities::reactions::emitter::ImpactContact;
use postretro_entities::registry::{EntityRegistry, Transform};
use postretro_foundation::{ImpactParticleRequest, ProjectileImpactLight};

use crate::impact_effects;
use crate::sprite_collection::derive_collection_id;

const IMPACT_SPRITE_COLLECTION: &str = "impact";

const IMPACT_LIFETIME: f32 = 0.18;
const IMPACT_EMISSIVE: f32 = 0.0;
const IMPACT_SPEC_INTENSITY: f32 = 0.45;
const IMPACT_SPEC_EXPONENT: f32 = 4.0;
/// Spark particles in one impact burst. Tests count in multiples of this.
pub use crate::particle_pool::IMPACT_PARTICLE_COUNT;
const SURFACE_OFFSET: f32 = 0.03;

static IMPACT_COLLECTION_ID: LazyLock<String> = LazyLock::new(|| {
    derive_collection_id(
        IMPACT_SPRITE_COLLECTION,
        Some(IMPACT_LIFETIME),
        None,
        IMPACT_EMISSIVE,
        Some(IMPACT_SPEC_INTENSITY),
        Some(IMPACT_SPEC_EXPONENT),
    )
});

pub fn sprite_collection() -> &'static str {
    IMPACT_SPRITE_COLLECTION
}

pub fn lifetime() -> f32 {
    IMPACT_LIFETIME
}

pub fn emissive() -> f32 {
    IMPACT_EMISSIVE
}

pub fn spec_intensity() -> f32 {
    IMPACT_SPEC_INTENSITY
}

pub fn spec_exponent() -> f32 {
    IMPACT_SPEC_EXPONENT
}

pub fn collection_id() -> &'static str {
    &IMPACT_COLLECTION_ID
}

/// Spawn the M10 default world-hit burst at `point`, oriented to eject away
/// from `normal`. Future data-defined impact descriptors replace the body of
/// this function; callers stay on this named effect chokepoint.
pub fn spawn_impact_effect_at(registry: &mut EntityRegistry, point: Vec3, normal: Vec3) {
    registry.push_impact_particle_request(ImpactParticleRequest { point, normal });
}

/// Spawn one impact burst per contact, each along that contact's own normal.
/// A zero or non-finite normal takes the burst's upward fallback. The
/// multi-contact routes (client-predicted hitscan, host ingest of a remote
/// shot, and observer impact cues) spawn through here so each contact gets
/// exactly one burst. Host-local fire and projectile advance spawn per impact
/// through `spawn_impact_effect_at` instead.
pub fn spawn_impact_effects_for_contacts(
    registry: &mut EntityRegistry,
    contacts: &[ImpactContact],
) {
    for contact in contacts {
        spawn_impact_effect_at(registry, contact.point, contact.normal);
    }
}

/// Materialize the descriptor-owned transient flash beside the ordinary impact
/// particles. The bridge reads non-following light positions from `origin`, so
/// keep it equal to the hit-point Transform instead of relying on the entity pose.
pub fn spawn_projectile_impact_light(
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
            falloff_model: FalloffKind::Linear,
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

/// Drain local burst intake in raise order before frame-time emitter spawning.
/// The caller retains `requests` so warmed frames allocate no request storage.
pub fn drain_impact_particle_requests(
    registry: &mut EntityRegistry,
    pool: &mut crate::particle_pool::ParticlePool,
    requests: &mut Vec<ImpactParticleRequest>,
) {
    registry.drain_impact_particle_requests_into(requests);
    for request in requests.iter() {
        let (normal, tangent, bitangent) = impact_frame(request.normal);
        let origin = request.point + normal * SURFACE_OFFSET;
        for index in 0..IMPACT_PARTICLE_COUNT {
            let angle = std::f32::consts::TAU * index as f32 / IMPACT_PARTICLE_COUNT as f32;
            let ring = tangent * angle.cos() + bitangent * angle.sin();
            let fan = if index == 0 {
                normal
            } else {
                (normal * 0.82 + ring * 0.58).normalize_or_zero()
            };
            let speed = 4.5 + (index % 3) as f32 * 1.35;
            if !pool.spawn_impact(origin, fan * speed, index) {
                break;
            }
        }
    }
    requests.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting_systems::particle_sim;
    use postretro_entities::components::light::LightComponent;
    use postretro_entities::registry::ComponentKind;
    use postretro_foundation::ProjectileImpactLight;

    const EPSILON: f32 = 1.0e-5;

    fn drain(registry: &mut EntityRegistry) -> crate::particle_pool::ParticlePool {
        let mut pool = crate::particle_pool::ParticlePool::new();
        pool.configure_impact_collection(7);
        drain_impact_particle_requests(registry, &mut pool, &mut Vec::new());
        pool
    }

    #[test]
    fn impact_chokepoint_spawns_particles_oriented_from_surface_normal() {
        let mut registry = EntityRegistry::new();
        let point = Vec3::new(1.0, 2.0, 3.0);
        let normal = Vec3::Z;
        spawn_impact_effect_at(&mut registry, point, normal);
        let pool = drain(&mut registry);
        assert_eq!(pool.len(), IMPACT_PARTICLE_COUNT);
        assert_eq!(registry.iter_with_kind(ComponentKind::Transform).count(), 0);
        for (index, p) in pool.iter().enumerate() {
            assert!((p.position - (point + normal * SURFACE_OFFSET)).length() < EPSILON);
            assert!(p.velocity.dot(normal) > 0.0);
            assert_eq!(p.collection, 7);
            assert!((p.rotation - index as f32 * 0.73).abs() < EPSILON);
            assert!((p.lifetime - IMPACT_LIFETIME * (0.8 + index as f32 * 0.025)).abs() < EPSILON);
        }
    }

    #[test]
    fn contact_bursts_spawn_one_burst_per_contact_along_its_own_normal() {
        let mut registry = EntityRegistry::new();
        let contacts = [
            ImpactContact::new(Vec3::ZERO, Vec3::X, None),
            ImpactContact::new(Vec3::new(10.0, 0.0, 0.0), Vec3::NEG_X, None),
            ImpactContact::new(Vec3::new(20.0, 0.0, 0.0), Vec3::Z, None),
        ];
        spawn_impact_effects_for_contacts(&mut registry, &contacts);
        let pool = drain(&mut registry);
        assert_eq!(pool.len(), 3 * IMPACT_PARTICLE_COUNT);
        for contact in &contacts {
            let particles = pool
                .iter()
                .filter(|p| {
                    (p.position - (contact.point + contact.normal * SURFACE_OFFSET)).length()
                        < EPSILON
                })
                .collect::<Vec<_>>();
            assert_eq!(particles.len(), IMPACT_PARTICLE_COUNT);
            assert!(
                particles
                    .iter()
                    .any(|p| p.velocity.normalize().dot(contact.normal) > 0.9999)
            );
        }
    }

    #[test]
    fn contact_bursts_take_the_upward_fallback_for_degenerate_normals() {
        for normal in [Vec3::ZERO, Vec3::new(f32::NAN, 0.0, 1.0), Vec3::INFINITY] {
            let mut registry = EntityRegistry::new();
            spawn_impact_effect_at(&mut registry, Vec3::ZERO, normal);
            let pool = drain(&mut registry);
            assert_eq!(pool.len(), IMPACT_PARTICLE_COUNT);
            assert!(
                pool.iter()
                    .all(|p| p.velocity.is_finite() && p.velocity.y > 0.0)
            );
        }
    }

    #[test]
    fn impact_particles_clean_up_after_lifetime() {
        let mut registry = EntityRegistry::new();
        spawn_impact_effect_at(&mut registry, Vec3::ZERO, Vec3::Y);
        let mut pool = drain(&mut registry);
        particle_sim::tick(&mut pool, &registry, IMPACT_LIFETIME * 2.0, |_| {
            (-9.81, Vec3::ZERO)
        });
        assert!(pool.is_empty());
        assert_eq!(pool.parameter_block_count(), 9);
    }

    #[test]
    fn zero_normal_falls_back_to_upward_burst() {
        let mut registry = EntityRegistry::new();
        spawn_impact_effect_at(&mut registry, Vec3::ZERO, Vec3::ZERO);
        let pool = drain(&mut registry);
        assert_eq!(pool.len(), IMPACT_PARTICLE_COUNT);
        assert!(pool.iter().all(|p| p.velocity.y > 0.0));
    }

    #[test]
    fn requests_from_several_ticks_drain_in_raise_order_and_step_in_the_frame() {
        let mut registry = EntityRegistry::new();
        for tick in 0..3 {
            spawn_impact_effect_at(&mut registry, Vec3::X * tick as f32 * 10.0, Vec3::Y);
        }
        let mut pool = drain(&mut registry);
        let before = pool
            .iter()
            .map(|p| (p.position, p.velocity))
            .collect::<Vec<_>>();
        assert_eq!(before.len(), 3 * IMPACT_PARTICLE_COUNT);
        for tick in 0..3 {
            assert!(
                (before[tick * IMPACT_PARTICLE_COUNT].0.x - tick as f32 * 10.0).abs() < EPSILON
            );
        }
        particle_sim::tick(&mut pool, &registry, 0.02, |_| (-9.81, Vec3::ZERO));
        for (p, (position, velocity)) in pool.iter().zip(before) {
            assert!((p.age - 0.02).abs() < EPSILON);
            assert!((p.position - (position + velocity * 0.02)).length() < EPSILON);
        }
    }

    #[test]
    fn request_queued_before_clear_never_enters_reloaded_pool() {
        let mut registry = EntityRegistry::new();
        spawn_impact_effect_at(&mut registry, Vec3::ZERO, Vec3::Y);
        registry.clear_for_level_unload();
        let mut pool = drain(&mut registry);
        assert!(pool.is_empty());
        spawn_impact_effect_at(&mut registry, Vec3::X, Vec3::Y);
        drain_impact_particle_requests(&mut registry, &mut pool, &mut Vec::new());
        assert_eq!(pool.len(), IMPACT_PARTICLE_COUNT);
        assert_eq!(pool.parameter_block_count(), 9);
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
