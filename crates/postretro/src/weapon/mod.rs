// Weapon fire system: resolves equipped wieldables and runs Rust-side shot logic.
// See: context/lib/entity_model.md §5, §7

use glam::Vec3;
use parry3d::math::{Point, Vector};

use crate::camera::Camera;
use crate::collision::{CollisionWorld, cast_ray};
use crate::input::{Action, ActionSnapshot, ButtonState};
use crate::scripting::components::health::HealthComponent;
use crate::scripting::components::weapon::WeaponComponent;
use crate::scripting::data_descriptors::{FireMode, ResolutionMode};
use crate::scripting::registry::{ComponentKind, ComponentValue, EntityId, EntityRegistry};

mod damage;
mod impact;

pub(crate) use damage::DamagePayload;
pub(crate) use impact::sprite_collection as impact_sprite_collection;
pub(crate) use impact::{lifetime as impact_lifetime, spawn_impact_effect_at};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ActivationOutcome {
    Hit(DamagePayload),
    Effect,
    Spawned(EntityId),
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WeaponActivation {
    pub(crate) origin: Vec3,
    pub(crate) direction: Vec3,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WeaponImpact {
    pub(crate) point: Vec3,
    pub(crate) normal: Vec3,
    /// The entity struck, when the nearest hit along the ray is an entity
    /// hitbox rather than world geometry. `None` for a world-only hit or when
    /// no hitbox entity lies along the ray within range. Spatial targeting
    /// rides here, beside the payload — never inside [`DamagePayload`]. The
    /// caller (the death/damage sweep) consumes this to route `apply_damage`.
    pub(crate) target: Option<EntityId>,
    pub(crate) outcome: ActivationOutcome,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct WeaponFireEvents {
    pub(crate) activate: Option<WeaponActivation>,
    pub(crate) impact: Option<WeaponImpact>,
}

impl WeaponFireEvents {
    pub(crate) fn event_names(&self) -> Vec<&'static str> {
        let mut names = Vec::with_capacity(2);
        if self.activate.is_some() {
            names.push("activate");
        }
        if self.impact.is_some() {
            names.push("impact");
        }
        names
    }
}

pub(crate) fn tick(
    registry: &mut EntityRegistry,
    active_wieldable: Option<EntityId>,
    snapshot: &ActionSnapshot,
    camera: &Camera,
    collision_world: &CollisionWorld,
    tick_dt: f32,
) -> WeaponFireEvents {
    let Some(weapon_id) = active_wieldable else {
        return WeaponFireEvents::default();
    };

    let Ok(existing) = registry.get_component::<WeaponComponent>(weapon_id) else {
        return WeaponFireEvents::default();
    };
    let mut weapon = existing.clone();

    let dt_ms = (tick_dt.max(0.0)) * 1000.0;
    weapon.cooldown_remaining_ms = (weapon.cooldown_remaining_ms - dt_ms).max(0.0);

    let stats = weapon.effective();
    let shoot = snapshot.button(Action::Shoot);
    let wants_fire = match stats.fire_mode {
        FireMode::Semi => shoot == ButtonState::Pressed && !weapon.shoot_press_consumed,
        FireMode::Auto => shoot.is_active(),
    };
    if stats.fire_mode == FireMode::Semi && shoot == ButtonState::Pressed {
        weapon.shoot_press_consumed = true;
    } else if !shoot.is_active() {
        weapon.shoot_press_consumed = false;
    }

    let events = if wants_fire && weapon.cooldown_remaining_ms <= 0.0 {
        weapon.cooldown_remaining_ms = stats.cooldown_ms;
        fire_hitscan(
            camera,
            collision_world,
            registry,
            stats.damage,
            stats.range,
            stats.resolution,
        )
    } else {
        WeaponFireEvents::default()
    };

    let _ = registry.set_component(weapon_id, weapon);
    events
}

fn fire_hitscan(
    camera: &Camera,
    collision_world: &CollisionWorld,
    registry: &EntityRegistry,
    damage: f32,
    range: f32,
    resolution: ResolutionMode,
) -> WeaponFireEvents {
    let (origin, direction) = camera.aim_ray();
    let mut events = WeaponFireEvents {
        activate: Some(WeaponActivation { origin, direction }),
        impact: None,
    };

    match resolution {
        ResolutionMode::Hitscan => {
            // World geometry hit, clamped to weapon range. parry returns the
            // nearest triangle intersection along the ray.
            let world_hit = cast_ray(
                collision_world,
                Point::new(origin.x, origin.y, origin.z),
                Vector::new(direction.x, direction.y, direction.z),
                range,
            )
            .map(|hit| RayHit {
                toi: hit.time_of_impact,
                point: origin + direction * hit.time_of_impact,
                normal: Vec3::new(hit.normal.x, hit.normal.y, hit.normal.z),
                target: None,
            });

            // Nearest entity hitbox hit, clamped to the same range. A world hit
            // nearer than every hitbox still wins; a hitbox behind the wall is
            // never reached because its toi exceeds the wall's.
            let entity_hit = nearest_entity_hit(registry, origin, direction, range);

            // Resolve nearest-of(world, entity). On a tie or no contender,
            // prefer whichever exists; entity ties to world keep the wall.
            let resolved = match (world_hit, entity_hit) {
                (Some(world), Some(entity)) => {
                    if entity.toi < world.toi {
                        Some(entity)
                    } else {
                        Some(world)
                    }
                }
                (Some(world), None) => Some(world),
                (None, Some(entity)) => Some(entity),
                (None, None) => None,
            };

            if let Some(hit) = resolved {
                events.impact = Some(WeaponImpact {
                    point: hit.point,
                    normal: hit.normal,
                    target: hit.target,
                    outcome: ActivationOutcome::Hit(DamagePayload { amount: damage }),
                });
            }
        }
    }

    events
}

/// A resolved point along the fire ray: world geometry or an entity hitbox.
/// `target` is `Some` only for an entity hit. `toi` is the ray parameter
/// (distance, since `direction` is unit length) used to pick the nearest.
#[derive(Debug, Clone, Copy)]
struct RayHit {
    toi: f32,
    point: Vec3,
    normal: Vec3,
    target: Option<EntityId>,
}

/// Walk every `Health` entity carrying a hitbox, ray-vs-AABB test each (AABB
/// centered at `transform.position + offset`, world-aligned — entity rotation
/// ignored), and return the nearest hit within `range`. `None` when no hitbox
/// entity lies along the ray within range.
fn nearest_entity_hit(
    registry: &EntityRegistry,
    origin: Vec3,
    direction: Vec3,
    range: f32,
) -> Option<RayHit> {
    let mut nearest: Option<RayHit> = None;

    for (id, value) in registry.iter_with_kind(ComponentKind::Health) {
        let ComponentValue::Health(HealthComponent {
            hitbox: Some(hitbox),
            ..
        }) = value
        else {
            continue;
        };

        let Ok(transform) = registry.get_component::<crate::scripting::registry::Transform>(id)
        else {
            continue;
        };

        let center = transform.position + hitbox.offset;
        let aabb_min = center - hitbox.half_extents;
        let aabb_max = center + hitbox.half_extents;

        let Some((toi, normal)) = ray_aabb_slab(origin, direction, aabb_min, aabb_max, range)
        else {
            continue;
        };

        if nearest.is_none_or(|n| toi < n.toi) {
            nearest = Some(RayHit {
                toi,
                point: origin + direction * toi,
                normal,
                target: Some(id),
            });
        }
    }

    nearest
}

/// Ray-vs-AABB slab test. Returns the entry time-of-impact (clamped to
/// `[0, range]`) and the face normal of the entered slab — the axis whose
/// near plane the ray crossed last, signed toward the ray origin so the impact
/// burst ejects back along the shot. Returns `None` on a miss, when the box is
/// entirely behind the origin, or when entry lies beyond `range`.
///
/// A degenerate (zero-thickness) slab on an axis the ray runs parallel to is
/// handled by the IEEE-754 infinity arithmetic of `1.0 / 0.0`: an origin
/// outside the slab on that axis yields `±inf` bounds that fail the overlap
/// test (miss), and inside the slab yields a `-inf..inf` span that never
/// constrains entry.
fn ray_aabb_slab(
    origin: Vec3,
    direction: Vec3,
    aabb_min: Vec3,
    aabb_max: Vec3,
    range: f32,
) -> Option<(f32, Vec3)> {
    let inv = Vec3::ONE / direction;

    // Per-axis slab entry/exit times. `t1`/`t2` are the unordered crossings;
    // `near`/`far` reorder them so `near <= far` regardless of ray direction.
    let t1 = (aabb_min - origin) * inv;
    let t2 = (aabb_max - origin) * inv;
    let near = t1.min(t2);
    let far = t1.max(t2);

    // Latest entry across all three slabs, earliest exit across all three.
    let t_entry = near.x.max(near.y).max(near.z);
    let t_exit = far.x.min(far.y).min(far.z);

    // Miss: the slabs do not overlap, or the box is entirely behind the origin,
    // or entry is beyond weapon range.
    if t_entry > t_exit || t_exit < 0.0 || t_entry > range {
        return None;
    }

    // An origin inside the box has a negative entry; clamp the reported hit to
    // the origin (toi 0). The struck face is the axis of the latest entry slab.
    let toi = t_entry.max(0.0);

    let axis = if near.x >= near.y && near.x >= near.z {
        Vec3::X
    } else if near.y >= near.z {
        Vec3::Y
    } else {
        Vec3::Z
    };
    // Sign the normal toward the ray origin (against the ray on that axis).
    let normal = axis * -direction.dot(axis).signum();

    Some((toi, normal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Binding, InputSystem, PhysicalInput};
    use crate::scripting::components::health::{HealthComponent, Hitbox};
    use crate::scripting::data_descriptors::WeaponDescriptor;
    use crate::scripting::registry::Transform;
    use parry3d::math::Isometry;
    use parry3d::shape::TriMesh;
    use winit::event::MouseButton;

    const EPSILON: f32 = 1.0e-5;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < EPSILON
    }

    fn assert_vec3_approx(actual: Vec3, expected: Vec3) {
        assert!(
            approx_eq(actual.x, expected.x)
                && approx_eq(actual.y, expected.y)
                && approx_eq(actual.z, expected.z),
            "expected ({:.5}, {:.5}, {:.5}), got ({:.5}, {:.5}, {:.5})",
            expected.x,
            expected.y,
            expected.z,
            actual.x,
            actual.y,
            actual.z,
        );
    }

    fn weapon_component(fire_mode: FireMode, cooldown_ms: f32) -> WeaponComponent {
        WeaponComponent::from_descriptor(&WeaponDescriptor {
            damage: 25.0,
            range: 10.0,
            cooldown_ms,
            fire_mode,
            resolution: ResolutionMode::Hitscan,
        })
    }

    fn spawn_weapon(registry: &mut EntityRegistry, component: WeaponComponent) -> EntityId {
        let id = registry.spawn(Transform::default());
        registry
            .set_component(id, component)
            .expect("weapon component should attach");
        id
    }

    /// Spawn a `Health` entity carrying a hitbox at a world position. Default
    /// `half_extents` make a unit cube (0.5 in each axis); `offset` defaults to
    /// zero so the AABB centers on `position`.
    fn spawn_hitbox_entity(
        registry: &mut EntityRegistry,
        position: Vec3,
        half_extents: Vec3,
        offset: Vec3,
    ) -> EntityId {
        let id = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        registry
            .set_component(
                id,
                HealthComponent {
                    max: 100.0,
                    current: 100.0,
                    hitbox: Some(Hitbox {
                        half_extents,
                        offset,
                    }),
                    death_handled: false,
                },
            )
            .expect("health component should attach");
        id
    }

    fn input_system() -> InputSystem {
        InputSystem::new(vec![Binding::new(
            PhysicalInput::MouseButton(MouseButton::Left),
            Action::Shoot,
        )])
    }

    fn shoot_snapshot(input: &mut InputSystem, active: bool) -> ActionSnapshot {
        input.set_physical_input(PhysicalInput::MouseButton(MouseButton::Left), active);
        input.snapshot()
    }

    fn wall_world() -> CollisionWorld {
        let points = vec![
            Point::new(-1.0, -1.0, -5.0),
            Point::new(1.0, -1.0, -5.0),
            Point::new(1.0, 1.0, -5.0),
            Point::new(-1.0, 1.0, -5.0),
        ];
        let triangles = vec![[0u32, 1, 2], [0, 2, 3]];
        CollisionWorld {
            mesh: TriMesh::new(points, triangles),
            isometry: Isometry::identity(),
        }
    }

    #[test]
    fn semi_weapon_fires_once_per_press() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = CollisionWorld::new();
        let mut input = input_system();

        let pressed = shoot_snapshot(&mut input, true);
        let events = tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );
        assert_eq!(events.event_names(), vec!["activate"]);

        let same_pressed_snapshot = tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            0.2,
        );
        assert!(same_pressed_snapshot.event_names().is_empty());

        let held = shoot_snapshot(&mut input, true);
        let events = tick(&mut registry, Some(weapon_id), &held, &camera, &world, 0.2);
        assert!(events.event_names().is_empty());

        let _released = shoot_snapshot(&mut input, false);
        let inactive = shoot_snapshot(&mut input, false);
        let _ = tick(
            &mut registry,
            Some(weapon_id),
            &inactive,
            &camera,
            &world,
            0.2,
        );

        let pressed_again = shoot_snapshot(&mut input, true);
        let events = tick(
            &mut registry,
            Some(weapon_id),
            &pressed_again,
            &camera,
            &world,
            1.0 / 60.0,
        );
        assert_eq!(events.event_names(), vec!["activate"]);
    }

    #[test]
    fn auto_weapon_fires_repeatedly_when_held_after_cooldown() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Auto, 30.0));
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = CollisionWorld::new();
        let mut input = input_system();

        let pressed = shoot_snapshot(&mut input, true);
        let first = tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            0.016,
        );
        assert_eq!(first.event_names(), vec!["activate"]);

        let held = shoot_snapshot(&mut input, true);
        let blocked = tick(
            &mut registry,
            Some(weapon_id),
            &held,
            &camera,
            &world,
            0.016,
        );
        assert!(blocked.event_names().is_empty());

        let still_held = shoot_snapshot(&mut input, true);
        let second = tick(
            &mut registry,
            Some(weapon_id),
            &still_held,
            &camera,
            &world,
            0.016,
        );
        assert_eq!(second.event_names(), vec!["activate"]);
    }

    #[test]
    fn hitscan_world_hit_returns_impact_point_normal_and_damage_payload() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = wall_world();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );

        assert_eq!(events.event_names(), vec!["activate", "impact"]);
        let impact = events.impact.expect("world hit should emit impact");
        assert_vec3_approx(impact.point, Vec3::new(0.0, 0.0, -5.0));
        assert_vec3_approx(impact.normal, Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(
            impact.outcome,
            ActivationOutcome::Hit(DamagePayload { amount: 25.0 })
        );
    }

    #[test]
    fn open_space_shot_consumes_cooldown_without_impact() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = CollisionWorld::new();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );

        assert_eq!(events.event_names(), vec!["activate"]);
        assert!(events.impact.is_none());
        let weapon = registry
            .get_component::<WeaponComponent>(weapon_id)
            .expect("weapon component should still exist");
        assert!(approx_eq(weapon.cooldown_remaining_ms, 100.0));
    }

    #[test]
    fn inactive_or_missing_wieldable_does_not_fire() {
        let mut registry = EntityRegistry::new();
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = CollisionWorld::new();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = tick(&mut registry, None, &pressed, &camera, &world, 1.0 / 60.0);
        assert!(events.event_names().is_empty());

        let non_weapon = registry.spawn(Transform::default());
        let events = tick(
            &mut registry,
            Some(non_weapon),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );
        assert!(events.event_names().is_empty());
        assert!(
            registry
                .iter_with_kind(ComponentKind::Weapon)
                .next()
                .is_none()
        );
    }

    // --- Ray-vs-AABB slab unit tests ------------------------------------

    #[test]
    fn ray_aabb_slab_hits_box_dead_ahead() {
        // Ray down -Z toward a unit box centered at (0, 0, -3). Entry face is
        // the +Z (near) face; normal points back toward the origin.
        let origin = Vec3::ZERO;
        let direction = Vec3::new(0.0, 0.0, -1.0);
        let min = Vec3::new(-0.5, -0.5, -3.5);
        let max = Vec3::new(0.5, 0.5, -2.5);

        let (toi, normal) =
            ray_aabb_slab(origin, direction, min, max, 10.0).expect("ray should hit the box");
        assert!(approx_eq(toi, 2.5), "entry toi is the near face distance");
        assert_vec3_approx(normal, Vec3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn ray_aabb_slab_misses_off_axis_box() {
        // Box shifted off the ray's path on +X: the ray never overlaps its X
        // slab, so the slabs do not intersect.
        let origin = Vec3::ZERO;
        let direction = Vec3::new(0.0, 0.0, -1.0);
        let min = Vec3::new(2.5, -0.5, -3.5);
        let max = Vec3::new(3.5, 0.5, -2.5);

        assert!(ray_aabb_slab(origin, direction, min, max, 10.0).is_none());
    }

    #[test]
    fn ray_aabb_slab_rejects_box_behind_origin() {
        // Box entirely behind the origin (+Z while the ray travels -Z): exit
        // time is negative, so it is never struck.
        let origin = Vec3::ZERO;
        let direction = Vec3::new(0.0, 0.0, -1.0);
        let min = Vec3::new(-0.5, -0.5, 2.5);
        let max = Vec3::new(0.5, 0.5, 3.5);

        assert!(ray_aabb_slab(origin, direction, min, max, 10.0).is_none());
    }

    #[test]
    fn ray_aabb_slab_rejects_box_beyond_range() {
        // Box is dead ahead but its entry exceeds the weapon range.
        let origin = Vec3::ZERO;
        let direction = Vec3::new(0.0, 0.0, -1.0);
        let min = Vec3::new(-0.5, -0.5, -8.5);
        let max = Vec3::new(0.5, 0.5, -7.5);

        // Entry at 7.5 sits beyond a range of 5.0.
        assert!(ray_aabb_slab(origin, direction, min, max, 5.0).is_none());
        // ...but is reachable when the range covers it.
        assert!(ray_aabb_slab(origin, direction, min, max, 10.0).is_some());
    }

    #[test]
    fn ray_aabb_slab_face_normal_tracks_struck_side() {
        // Shooting along +X strikes the box's -X (near) face; normal points
        // back toward -X, against the ray.
        let origin = Vec3::ZERO;
        let direction = Vec3::new(1.0, 0.0, 0.0);
        let min = Vec3::new(2.5, -0.5, -0.5);
        let max = Vec3::new(3.5, 0.5, 0.5);

        let (toi, normal) =
            ray_aabb_slab(origin, direction, min, max, 10.0).expect("ray should hit the box");
        assert!(approx_eq(toi, 2.5));
        assert_vec3_approx(normal, Vec3::new(-1.0, 0.0, 0.0));
    }

    // --- nearest_entity_hit / nearest-of(world, entity) -----------------

    #[test]
    fn nearest_entity_hit_selects_box_along_ray() {
        let mut registry = EntityRegistry::new();
        let id = spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.0, -4.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );

        let hit = nearest_entity_hit(&registry, Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), 10.0)
            .expect("hitbox entity along the ray should be hit");
        assert_eq!(hit.target, Some(id));
        assert!(approx_eq(hit.toi, 3.5), "near face is at z = -3.5");
        assert_vec3_approx(hit.point, Vec3::new(0.0, 0.0, -3.5));
        assert_vec3_approx(hit.normal, Vec3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn nearest_entity_hit_keeps_nearest_of_two_boxes() {
        let mut registry = EntityRegistry::new();
        let far = spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.0, -8.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        let near = spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.0, -3.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        let _ = far;

        let hit = nearest_entity_hit(&registry, Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), 10.0)
            .expect("a box lies along the ray");
        assert_eq!(hit.target, Some(near), "nearest box wins");
    }

    #[test]
    fn nearest_entity_hit_misses_when_no_box_on_ray() {
        let mut registry = EntityRegistry::new();
        spawn_hitbox_entity(
            &mut registry,
            Vec3::new(5.0, 0.0, -4.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );

        assert!(
            nearest_entity_hit(&registry, Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), 10.0).is_none()
        );
    }

    #[test]
    fn nearest_entity_hit_respects_offset() {
        // Box transform sits off-axis on +X, but its hitbox offset shifts the
        // AABB back onto the ray's path.
        let mut registry = EntityRegistry::new();
        let id = spawn_hitbox_entity(
            &mut registry,
            Vec3::new(2.0, 0.0, -4.0),
            Vec3::splat(0.5),
            Vec3::new(-2.0, 0.0, 0.0),
        );

        let hit = nearest_entity_hit(&registry, Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), 10.0)
            .expect("offset recenters the AABB onto the ray");
        assert_eq!(hit.target, Some(id));
    }

    #[test]
    fn entity_hit_reported_through_weapon_impact() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        let target = spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.0, -4.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        // Empty world: no wall, so the entity is the only contender.
        let world = CollisionWorld::new();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );

        let impact = events.impact.expect("entity hit should emit impact");
        assert_eq!(
            impact.target,
            Some(target),
            "spatial target rides beside payload"
        );
        assert_vec3_approx(impact.point, Vec3::new(0.0, 0.0, -3.5));
        assert_vec3_approx(impact.normal, Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(
            impact.outcome,
            ActivationOutcome::Hit(DamagePayload { amount: 25.0 })
        );
    }

    #[test]
    fn world_wins_when_wall_is_nearer_than_entity() {
        // Wall sits at z = -5; entity box behind it at z = -8. The wall is
        // nearer, so it is selected and no entity target is reported.
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.0, -8.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = wall_world();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );

        let impact = events.impact.expect("wall hit should emit impact");
        assert_eq!(impact.target, None, "wall wins; no entity target");
        assert_vec3_approx(impact.point, Vec3::new(0.0, 0.0, -5.0));
    }

    #[test]
    fn entity_wins_when_nearer_than_wall() {
        // Entity box at z = -3, in front of the wall at z = -5. The entity is
        // nearer and is selected over the wall.
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        let target = spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.0, -3.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = wall_world();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );

        let impact = events.impact.expect("entity hit should emit impact");
        assert_eq!(impact.target, Some(target), "nearer entity beats the wall");
        assert_vec3_approx(impact.point, Vec3::new(0.0, 0.0, -2.5));
    }

    #[test]
    fn entity_beyond_range_is_not_targeted() {
        // Weapon range is 10.0 (see `weapon_component`). The entity sits at
        // z = -12, beyond range, and there is no wall: nothing is hit.
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.0, -12.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = CollisionWorld::new();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );

        assert!(
            events.impact.is_none(),
            "entity beyond weapon range is not targeted"
        );
    }

    #[test]
    fn near_miss_resolves_to_wall_behind() {
        // A hitbox entity sits just off the ray (a near miss) while the wall
        // lies behind it; the shot passes the entity and strikes the wall.
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        spawn_hitbox_entity(
            &mut registry,
            Vec3::new(2.0, 0.0, -3.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = wall_world();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );

        let impact = events.impact.expect("wall hit should emit impact");
        assert_eq!(impact.target, None, "near miss falls through to the wall");
        assert_vec3_approx(impact.point, Vec3::new(0.0, 0.0, -5.0));
    }
}
