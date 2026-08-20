// Fixed-tick straight-line projectile advance and direct-impact resolution.
// See: context/lib/entity_model.md §5, §7

use std::cell::RefCell;
use std::rc::Rc;

use glam::Vec3;
use parry3d::math::{Point, Vector};
use parry3d::shape::Capsule;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::projectile::ProjectileComponent;
use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform};

use crate::collision::{CollisionWorld, cast_capsule};
use crate::scripting_systems::hit_zones::{EntityRayHit, HitZoneStore, nearest_entity_hit};
use crate::sim::weapon_stage::apply_authorized_weapon_impact_damage;
use crate::weapon::{self, ActivationOutcome, DamagePayload, WeaponImpact};

enum PendingProjectileAction {
    Update {
        projectile: EntityId,
        transform: Transform,
        component: ProjectileComponent,
    },
    Expire {
        projectile: EntityId,
        component: ProjectileComponent,
    },
    Impact {
        projectile: EntityId,
        component: ProjectileComponent,
        impact: WeaponImpact,
    },
}

enum ProjectileResolution<'a> {
    Impact {
        component: &'a ProjectileComponent,
        impact: &'a WeaponImpact,
    },
    Expire {
        component: &'a ProjectileComponent,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PredictedProjectileResolution {
    Impact { shot_id: u64, impact: WeaponImpact },
    Expired { shot_id: u64 },
}

struct WorldHit {
    toi: f32,
    point: Vec3,
    normal: Vec3,
}

enum NearestProjectileHit {
    World(WorldHit),
    Entity(EntityRayHit),
}

/// Advance all locally authoritative projectiles by `dt`.
///
/// The snapshot/walk phase is deliberately registry-immutable. Contact effects,
/// damage, component writes, and despawns run afterward so one projectile can
/// never invalidate another iterator entry mid-walk. The same body accepts a
/// frame delta for the later connected-client prediction path.
pub(crate) fn advance(
    registry: &Rc<RefCell<EntityRegistry>>,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    dt: f32,
    on_impact: &mut impl FnMut(&mut EntityRegistry),
) {
    advance_matching(
        registry,
        collision_world,
        hit_zone_store,
        anim_time,
        dt,
        |_| true,
        |registry, resolution| {
            let ProjectileResolution::Impact { component, impact } = resolution else {
                return;
            };
            weapon::spawn_impact_effect_at(registry, impact.point, impact.normal);

            let target_is_live = impact.target.is_none_or(|target| {
                registry
                    .get_component::<HealthComponent>(target)
                    .is_ok_and(|health| health.current.is_finite() && health.current > 0.0)
            });
            if target_is_live && let ActivationOutcome::Hit(payload) = &impact.outcome {
                let attacker = registry
                    .exists(component.owner_pawn)
                    .then_some(component.owner_pawn);
                apply_authorized_weapon_impact_damage(
                    registry,
                    component.owner_weapon,
                    attacker,
                    impact,
                    component.credit_source.clone(),
                    payload.amount,
                );
                on_impact(registry);
            }
        },
    );
}

/// Advance only locally-predicted connected-client projectiles. Their collision
/// result is a declaration, never a local Health mutation. Presentation-only
/// projectiles reserve `shot_id == 0` so they cannot enter this authority path.
pub(crate) fn advance_predicted(
    registry: &Rc<RefCell<EntityRegistry>>,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    dt: f32,
    on_resolution: &mut impl FnMut(PredictedProjectileResolution),
) {
    advance_matching(
        registry,
        collision_world,
        hit_zone_store,
        anim_time,
        dt,
        |component| component.shot_id != 0,
        |registry, resolution| match resolution {
            ProjectileResolution::Impact { component, impact } => {
                weapon::spawn_impact_effect_at(registry, impact.point, impact.normal);
                on_resolution(PredictedProjectileResolution::Impact {
                    shot_id: component.shot_id,
                    impact: impact.clone(),
                });
            }
            ProjectileResolution::Expire { component } => {
                on_resolution(PredictedProjectileResolution::Expired {
                    shot_id: component.shot_id,
                });
            }
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn advance_matching(
    registry: &Rc<RefCell<EntityRegistry>>,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    dt: f32,
    should_advance: impl Fn(&ProjectileComponent) -> bool,
    mut on_resolution: impl for<'a> FnMut(&mut EntityRegistry, ProjectileResolution<'a>),
) {
    if !dt.is_finite() || dt < 0.0 {
        return;
    }

    let snapshot: Vec<(EntityId, Transform, ProjectileComponent)> = {
        let registry = registry.borrow();
        registry
            .iter_with_kind(ComponentKind::Projectile)
            .filter_map(|(id, value)| {
                let ComponentValue::Projectile(component) = value else {
                    return None;
                };
                if !should_advance(component) {
                    return None;
                }
                let transform = registry.get_component::<Transform>(id).ok()?;
                Some((id, *transform, component.clone()))
            })
            .collect()
    };

    let mut pending = Vec::with_capacity(snapshot.len());
    for (projectile_id, transform, mut component) in snapshot {
        if component.spawned {
            component.spawned = false;
            pending.push(PendingProjectileAction::Update {
                projectile: projectile_id,
                transform,
                component,
            });
            continue;
        }

        let direction = Vec3::from_array(component.direction);
        if !direction.is_finite()
            || (direction.length_squared() - 1.0).abs() > 1.0e-3
            || !component.speed.is_finite()
            || component.speed <= 0.0
            || !component.radius.is_finite()
            || component.radius < 0.0
            || !component.remaining_range.is_finite()
            || !component.remaining_lifetime.is_finite()
        {
            pending.push(PendingProjectileAction::Expire {
                projectile: projectile_id,
                component,
            });
            continue;
        }

        let remaining_range = component.remaining_range.max(0.0);
        let remaining_lifetime = component.remaining_lifetime.max(0.0);
        let requested_distance = component.speed * dt;
        let lifetime_distance = component.speed * remaining_lifetime;
        let segment_length = requested_distance
            .min(remaining_range)
            .min(lifetime_distance);
        let expires_after_segment = segment_length >= remaining_range
            || segment_length >= lifetime_distance
            || component.remaining_range <= 0.0
            || component.remaining_lifetime <= 0.0;

        if let Some(hit) = nearest_projectile_hit(
            collision_world,
            &registry.borrow(),
            hit_zone_store,
            anim_time,
            transform.position,
            direction,
            segment_length,
            component.radius,
        ) {
            let impact = match hit {
                NearestProjectileHit::World(world) => WeaponImpact {
                    point: world.point,
                    normal: world.normal,
                    target: None,
                    zone: None,
                    outcome: ActivationOutcome::Hit(DamagePayload {
                        amount: component.damage,
                    }),
                },
                NearestProjectileHit::Entity(entity) => WeaponImpact {
                    point: entity.point,
                    normal: entity.normal,
                    target: Some(entity.target),
                    zone: entity.zone,
                    outcome: ActivationOutcome::Hit(DamagePayload {
                        amount: component.damage,
                    }),
                },
            };
            pending.push(PendingProjectileAction::Impact {
                projectile: projectile_id,
                component,
                impact,
            });
            continue;
        }

        if expires_after_segment {
            // The segment was already swept above. A contact at the final range
            // or lifetime boundary wins over expiry; only an empty sweep expires.
            pending.push(PendingProjectileAction::Expire {
                projectile: projectile_id,
                component,
            });
            continue;
        }

        let travel_time = segment_length / component.speed;
        component.remaining_range = (remaining_range - segment_length).max(0.0);
        component.remaining_lifetime = (remaining_lifetime - travel_time).max(0.0);
        pending.push(PendingProjectileAction::Update {
            projectile: projectile_id,
            transform: Transform {
                position: transform.position + direction * segment_length,
                ..transform
            },
            component,
        });
    }

    let mut registry = registry.borrow_mut();
    for action in pending {
        match action {
            PendingProjectileAction::Update {
                projectile,
                transform,
                component,
            } => {
                if registry.exists(projectile) {
                    let _ = registry.set_component(projectile, transform);
                    let _ = registry.set_component(projectile, component);
                }
            }
            PendingProjectileAction::Expire {
                projectile,
                component,
            } => {
                if registry.exists(projectile) {
                    on_resolution(
                        &mut registry,
                        ProjectileResolution::Expire {
                            component: &component,
                        },
                    );
                    let _ = registry.despawn(projectile);
                }
            }
            PendingProjectileAction::Impact {
                projectile,
                component,
                impact,
            } => {
                if !registry.exists(projectile) {
                    continue;
                }
                on_resolution(
                    &mut registry,
                    ProjectileResolution::Impact {
                        component: &component,
                        impact: &impact,
                    },
                );
                let _ = registry.despawn(projectile);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn nearest_projectile_hit(
    collision_world: &CollisionWorld,
    registry: &EntityRegistry,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    origin: Vec3,
    direction: Vec3,
    range: f32,
    radius: f32,
) -> Option<NearestProjectileHit> {
    let sphere = Capsule::new(Point::origin(), Point::origin(), radius);
    let world_hit = cast_capsule(
        collision_world,
        Point::new(origin.x, origin.y, origin.z),
        &sphere,
        Vector::new(direction.x, direction.y, direction.z),
        range,
    )
    .map(|hit| WorldHit {
        toi: hit.time_of_impact.max(0.0),
        point: origin + direction * hit.time_of_impact.max(0.0),
        normal: Vec3::new(hit.normal2.x, hit.normal2.y, hit.normal2.z),
    });
    let entity_hit = nearest_entity_hit(
        registry,
        hit_zone_store,
        anim_time,
        origin,
        direction,
        range,
        radius,
    );

    match (world_hit, entity_hit) {
        (Some(world), Some(entity)) if entity.toi < world.toi => {
            Some(NearestProjectileHit::Entity(entity))
        }
        (Some(world), _) => Some(NearestProjectileHit::World(world)),
        (None, Some(entity)) => Some(NearestProjectileHit::Entity(entity)),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::components::health::Hitbox;

    fn spawn_target(registry: &mut EntityRegistry, position: Vec3, half_extents: Vec3) -> EntityId {
        let target = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        registry
            .set_component(
                target,
                HealthComponent {
                    max: 20.0,
                    current: 20.0,
                    hitbox: Some(Hitbox {
                        half_extents,
                        offset: Vec3::ZERO,
                    }),
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .expect("target health attaches");
        target
    }

    fn spawn_projectile(
        registry: &mut EntityRegistry,
        range: f32,
        radius: f32,
        damage: f32,
    ) -> EntityId {
        let owner_pawn = registry.spawn(Transform::default());
        let owner_weapon = registry.spawn(Transform::default());
        let projectile = registry.spawn(Transform::default());
        registry
            .set_component(
                projectile,
                ProjectileComponent {
                    direction: Vec3::NEG_Z.to_array(),
                    speed: 1.0,
                    radius,
                    remaining_range: range,
                    remaining_lifetime: 10.0,
                    damage,
                    credit_source: "test.projectile".to_string(),
                    owner_pawn,
                    owner_weapon,
                    spawned: true,
                    shot_id: 0,
                },
            )
            .expect("projectile component attaches");
        projectile
    }

    fn advance_once(registry: &Rc<RefCell<EntityRegistry>>, dt: f32) {
        let world = CollisionWorld::default();
        let zones = HitZoneStore::new();
        let mut ignore_impact = |_: &mut EntityRegistry| {};
        advance(registry, &world, &zones, 0.0, dt, &mut ignore_impact);
    }

    #[test]
    fn spawned_projectile_skips_fire_pass_then_damages_on_later_impact_pass() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let target = spawn_target(
            &mut registry.borrow_mut(),
            Vec3::new(0.0, 0.0, -0.75),
            Vec3::splat(0.1),
        );
        let projectile = spawn_projectile(&mut registry.borrow_mut(), 2.0, 0.0, 5.0);

        advance_once(&registry, 1.0);
        assert_eq!(
            registry
                .borrow()
                .get_component::<HealthComponent>(target)
                .expect("target remains live")
                .current,
            20.0,
            "the spawned marker forbids a fire-pass impact"
        );
        assert!(registry.borrow().exists(projectile));

        advance_once(&registry, 1.0);
        assert_eq!(
            registry
                .borrow()
                .get_component::<HealthComponent>(target)
                .expect("target remains live")
                .current,
            15.0,
            "damage lands only on the later impact pass"
        );
        assert!(!registry.borrow().exists(projectile));
    }

    #[test]
    fn projectile_expiring_at_range_limit_applies_no_damage() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let target = spawn_target(
            &mut registry.borrow_mut(),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::splat(0.05),
        );
        let projectile = spawn_projectile(&mut registry.borrow_mut(), 0.5, 0.0, 5.0);

        advance_once(&registry, 1.0);
        advance_once(&registry, 1.0);

        assert_eq!(
            registry
                .borrow()
                .get_component::<HealthComponent>(target)
                .expect("target remains live")
                .current,
            20.0
        );
        assert!(!registry.borrow().exists(projectile));
    }

    #[test]
    fn projectile_radius_hits_an_aabb_missed_by_its_center_ray() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let target = spawn_target(
            &mut registry.borrow_mut(),
            Vec3::new(0.35, 0.0, -0.5),
            Vec3::splat(0.1),
        );
        let projectile = spawn_projectile(&mut registry.borrow_mut(), 2.0, 0.3, 5.0);

        advance_once(&registry, 1.0);
        advance_once(&registry, 1.0);

        assert_eq!(
            registry
                .borrow()
                .get_component::<HealthComponent>(target)
                .expect("target remains live")
                .current,
            15.0,
            "the swept width should reach the expanded hitbox"
        );
        assert!(!registry.borrow().exists(projectile));
    }

    #[test]
    fn predicted_projectile_declares_later_impact_without_mutating_target_health() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let target = spawn_target(
            &mut registry.borrow_mut(),
            Vec3::new(0.0, 0.0, -0.75),
            Vec3::splat(0.1),
        );
        let projectile = spawn_projectile(&mut registry.borrow_mut(), 2.0, 0.0, 5.0);
        let mut component = registry
            .borrow()
            .get_component::<ProjectileComponent>(projectile)
            .expect("projectile component attaches")
            .clone();
        component.shot_id = 77;
        registry
            .borrow_mut()
            .set_component(projectile, component)
            .expect("prediction shot id attaches");

        let world = CollisionWorld::default();
        let zones = HitZoneStore::new();
        let mut resolutions = Vec::new();
        advance_predicted(&registry, &world, &zones, 0.0, 1.0, &mut |resolution| {
            resolutions.push(resolution)
        });
        assert!(
            resolutions.is_empty(),
            "spawn pass never resolves an impact"
        );

        advance_predicted(&registry, &world, &zones, 0.0, 1.0, &mut |resolution| {
            resolutions.push(resolution)
        });

        assert_eq!(resolutions.len(), 1);
        match &resolutions[0] {
            PredictedProjectileResolution::Impact { shot_id, impact } => {
                assert_eq!(*shot_id, 77);
                assert_eq!(impact.target, Some(target));
            }
            PredictedProjectileResolution::Expired { .. } => {
                panic!("the target contact must declare an impact, not expiry")
            }
        }
        assert_eq!(
            registry
                .borrow()
                .get_component::<HealthComponent>(target)
                .expect("target remains live")
                .current,
            20.0,
            "connected-client prediction declares the hit; it never writes enemy Health"
        );
    }
}
