// Weapon fire system: resolves equipped wieldables and runs Rust-side shot logic.
// See: context/lib/entity_model.md §5, §7

use glam::Vec3;
use parry3d::math::{Point, Vector};

use crate::camera::Camera;
use crate::collision::{CollisionWorld, cast_ray};
use crate::input::{Action, ActionSnapshot, ButtonState};
use crate::scripting::components::weapon::WeaponComponent;
use crate::scripting::data_descriptors::{FireMode, ResolutionMode};
use crate::scripting::registry::{EntityId, EntityRegistry};
use crate::scripting_systems::hit_zones::{EntityRayHit, HitZoneStore, nearest_entity_hit};

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

// Not `Copy`: `zone: Option<String>` carries a heap-backed tag for skeletal
// hit-zone hits, so `WeaponImpact` (and `WeaponFireEvents`, which embeds it)
// move/borrow rather than copy. Audited call sites: `fire_hitscan` constructs it
// (the sole literal site, production), and `run_weapon_fire_tick` borrows
// `events.impact` rather than copying it out.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WeaponImpact {
    pub(crate) point: Vec3,
    pub(crate) normal: Vec3,
    /// The entity struck, when the nearest hit along the ray is an entity
    /// hitbox rather than world geometry. `None` for a world-only hit or when
    /// no hitbox entity lies along the ray within range. Spatial targeting
    /// rides here, beside the payload — never inside [`DamagePayload`]. The
    /// caller (the death/damage sweep) consumes this to route `apply_damage`.
    pub(crate) target: Option<EntityId>,
    /// The authored skeletal hit-zone tag the shot landed on (e.g. "head"),
    /// surfaced for an entity hit that struck a bone-posed capsule. `None` for a
    /// world hit or an authored-AABB entity hit. Task 5 consumes this to apply
    /// the descriptor's per-zone damage multiplier; here it is only surfaced.
    pub(crate) zone: Option<String>,
    pub(crate) outcome: ActivationOutcome,
}

#[derive(Debug, Clone, Default, PartialEq)]
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

#[allow(clippy::too_many_arguments)] // weapon fire genuinely needs all of these inputs.
pub(crate) fn tick(
    registry: &mut EntityRegistry,
    active_wieldable: Option<EntityId>,
    snapshot: &ActionSnapshot,
    camera: &Camera,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
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
            hit_zone_store,
            anim_time,
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

#[allow(clippy::too_many_arguments)] // weapon fire genuinely needs all of these inputs.
fn fire_hitscan(
    camera: &Camera,
    collision_world: &CollisionWorld,
    registry: &EntityRegistry,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
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
            .map(|hit| WorldHit {
                toi: hit.time_of_impact,
                point: origin + direction * hit.time_of_impact,
                normal: Vec3::new(hit.normal.x, hit.normal.y, hit.normal.z),
            });

            // Nearest entity hit (authored AABB or bone-posed capsule), clamped
            // to the same range — resolved entirely by the standalone facility. A
            // world hit nearer than every entity still wins; an entity behind the
            // wall is never reached because its toi exceeds the wall's.
            let entity_hit = nearest_entity_hit(
                registry,
                hit_zone_store,
                anim_time,
                origin,
                direction,
                range,
            );

            // World-vs-entity nearest-of resolution stays in the weapon. On a tie
            // (entity toi == world toi) the wall wins (`entity.toi < world.toi`).
            let impact = match (world_hit, entity_hit) {
                (Some(world), Some(entity)) if entity.toi < world.toi => {
                    impact_from_entity(entity, damage)
                }
                (Some(world), _) => WeaponImpact {
                    point: world.point,
                    normal: world.normal,
                    target: None,
                    zone: None,
                    outcome: ActivationOutcome::Hit(DamagePayload { amount: damage }),
                },
                (None, Some(entity)) => impact_from_entity(entity, damage),
                (None, None) => return events,
            };
            events.impact = Some(impact);
        }
    }

    events
}

/// A resolved world-geometry point along the fire ray. `toi` is the ray
/// parameter (distance, since `direction` is unit length) used to pick the
/// nearest of world vs. entity. Entity hits are resolved by the hit-zone
/// facility, which owns the AABB/capsule narrow phases and returns its own type.
#[derive(Debug, Clone, Copy)]
struct WorldHit {
    toi: f32,
    point: Vec3,
    normal: Vec3,
}

/// Build a [`WeaponImpact`] from a facility entity hit, attaching the damage
/// payload and carrying the struck zone tag (if any) through to the caller.
fn impact_from_entity(entity: EntityRayHit, damage: f32) -> WeaponImpact {
    WeaponImpact {
        point: entity.point,
        normal: entity.normal,
        target: Some(entity.target),
        zone: entity.zone,
        outcome: ActivationOutcome::Hit(DamagePayload { amount: damage }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Binding, InputSystem, PhysicalInput};
    use crate::scripting::components::health::{HealthComponent, Hitbox};
    use crate::scripting::data_descriptors::WeaponDescriptor;
    use crate::scripting::registry::{ComponentKind, Transform};
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

    /// Run a weapon `tick` with an EMPTY hit-zone store and a zero animation
    /// clock — the no-skeletal-zones configuration, so these tests exercise the
    /// authored-AABB path exactly as before the facility landed (byte-identical
    /// behavior: an empty store routes every health+hitbox entity through the
    /// AABB narrow phase). Keeps the existing test bodies a one-word rename.
    fn fire_tick(
        registry: &mut EntityRegistry,
        active_wieldable: Option<EntityId>,
        snapshot: &ActionSnapshot,
        camera: &Camera,
        world: &CollisionWorld,
        tick_dt: f32,
    ) -> WeaponFireEvents {
        let store = HitZoneStore::new();
        tick(
            registry,
            active_wieldable,
            snapshot,
            camera,
            world,
            &store,
            0.0,
            tick_dt,
        )
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
                    zone_multipliers: std::collections::HashMap::new(),
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
        let events = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );
        assert_eq!(events.event_names(), vec!["activate"]);

        let same_pressed_snapshot = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            0.2,
        );
        assert!(same_pressed_snapshot.event_names().is_empty());

        let held = shoot_snapshot(&mut input, true);
        let events = fire_tick(&mut registry, Some(weapon_id), &held, &camera, &world, 0.2);
        assert!(events.event_names().is_empty());

        let _released = shoot_snapshot(&mut input, false);
        let inactive = shoot_snapshot(&mut input, false);
        let _ = fire_tick(
            &mut registry,
            Some(weapon_id),
            &inactive,
            &camera,
            &world,
            0.2,
        );

        let pressed_again = shoot_snapshot(&mut input, true);
        let events = fire_tick(
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
        let first = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            0.016,
        );
        assert_eq!(first.event_names(), vec!["activate"]);

        let held = shoot_snapshot(&mut input, true);
        let blocked = fire_tick(
            &mut registry,
            Some(weapon_id),
            &held,
            &camera,
            &world,
            0.016,
        );
        assert!(blocked.event_names().is_empty());

        let still_held = shoot_snapshot(&mut input, true);
        let second = fire_tick(
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

        let events = fire_tick(
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

        let events = fire_tick(
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

        let events = fire_tick(&mut registry, None, &pressed, &camera, &world, 1.0 / 60.0);
        assert!(events.event_names().is_empty());

        let non_weapon = registry.spawn(Transform::default());
        let events = fire_tick(
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

    // The AABB slab test and the entity-hit walk relocated to the hit-zone
    // facility (`scripting/systems/hit_zones.rs`) along with `ray_aabb_slab` /
    // `nearest_entity_hit`; their unit tests live there now. The weapon-level
    // tests below cover the delegation + world-vs-entity nearest-of resolution.

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

        let events = fire_tick(
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

        let events = fire_tick(
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

        let events = fire_tick(
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

        let events = fire_tick(
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

        let events = fire_tick(
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

    // Regression: a zero-HP entity that has not yet been swept from the registry
    // (death sweep runs after weapon fire) was absorbing shots for one frame,
    // blocking the wall behind it.
    #[test]
    fn zero_hp_entity_on_ray_is_not_targeted_wall_behind_wins() {
        // Entity with current == 0.0 sits directly on the ray in front of the
        // wall. The wall should win; the corpse must not absorb the shot.
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        let corpse = spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.0, -3.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        // Drive health to zero to simulate the pending-despawn state.
        let mut health = registry
            .get_component::<HealthComponent>(corpse)
            .expect("health component should exist")
            .clone();
        health.current = 0.0;
        registry
            .set_component(corpse, health)
            .expect("health component update should succeed");

        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = wall_world();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );

        let impact = events.impact.expect("wall hit should emit impact");
        assert_eq!(impact.target, None, "zero-HP corpse is skipped; wall wins");
        assert_vec3_approx(impact.point, Vec3::new(0.0, 0.0, -5.0));
    }

    // --- Skeletal hit-zone delegation (Task 4) ------------------------------

    use crate::lighting::cone_frustum::Aabb;
    use crate::model::skeleton::{Joint, RestLocal, Skeleton};
    use crate::scripting::components::mesh::MeshComponent;
    use crate::scripting_systems::hit_zones::ModelHitZones;
    use std::sync::Arc;

    /// Build a store holding one model with a single TAGGED LEAF joint at the
    /// model origin — a sphere of `radius`. The derived bound is the sphere's box
    /// so the broad phase admits it. Static (no clip), so any anim_time poses the
    /// joint to the origin.
    fn head_zone_store(
        handle: &str,
        radius: f32,
    ) -> crate::scripting_systems::hit_zones::HitZoneStore {
        let skeleton = Skeleton {
            joints: vec![Joint {
                parent: None,
                inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
                rest_local: RestLocal::default(),
            }],
        };
        let model = ModelHitZones {
            skeleton: Arc::new(skeleton),
            clips: Arc::new(vec![]),
            joint_zones: vec![Some(crate::model::gltf_loader::JointZone {
                tag: "head".to_string(),
                radius: Some(radius),
            })],
            derived_bound: Some(Aabb {
                min: Vec3::splat(-radius),
                max: Vec3::splat(radius),
            }),
        };
        let mut store = HitZoneStore::new();
        store.insert_for_test(crate::model::ModelHandle::from(handle), model);
        store
    }

    /// Run `tick` with a populated hit-zone store and animation clock.
    fn fire_tick_with(
        registry: &mut EntityRegistry,
        active_wieldable: Option<EntityId>,
        snapshot: &ActionSnapshot,
        camera: &Camera,
        world: &CollisionWorld,
        store: &HitZoneStore,
        anim_time: f64,
        tick_dt: f32,
    ) -> WeaponFireEvents {
        tick(
            registry,
            active_wieldable,
            snapshot,
            camera,
            world,
            store,
            anim_time,
            tick_dt,
        )
    }

    /// Spawn a health + stateless-mesh entity that uses a zone-bearing model.
    fn spawn_zone_entity(registry: &mut EntityRegistry, model: &str, position: Vec3) -> EntityId {
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
                    hitbox: None,
                    death_handled: false,
                    zone_multipliers: std::collections::HashMap::new(),
                },
            )
            .unwrap();
        registry
            .set_component(id, MeshComponent::stateless(model.to_string()))
            .unwrap();
        id
    }

    /// A zone hit through the full weapon path surfaces its zone tag on the
    /// impact (Task 5 reads `impact.zone`; here we only surface it).
    #[test]
    fn zone_hit_reports_zone_tag_through_weapon_impact() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        // Head sphere (r=0.5) at the entity, placed on the -Z ray at z=-4.
        let store = head_zone_store("mob", 0.5);
        let target = spawn_zone_entity(&mut registry, "mob", Vec3::new(0.0, 0.0, -4.0));
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = CollisionWorld::new(); // empty world: the zone is the only contender
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = fire_tick_with(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            &store,
            0.0,
            1.0 / 60.0,
        );

        let impact = events.impact.expect("zone hit should emit impact");
        assert_eq!(impact.target, Some(target), "zone entity is targeted");
        assert_eq!(
            impact.zone.as_deref(),
            Some("head"),
            "the struck zone tag rides on the impact"
        );
    }

    /// A wall in front of a zone-bearing entity still wins the nearest-of: the
    /// world hit is nearer, so no entity target / zone is reported.
    #[test]
    fn wall_in_front_of_zone_still_wins() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        let store = head_zone_store("mob", 0.5);
        // Zone entity BEHIND the wall (wall at z=-5; entity at z=-8).
        spawn_zone_entity(&mut registry, "mob", Vec3::new(0.0, 0.0, -8.0));
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = wall_world();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = fire_tick_with(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            &store,
            0.0,
            1.0 / 60.0,
        );

        let impact = events.impact.expect("wall hit should emit impact");
        assert_eq!(impact.target, None, "wall wins; no zone entity targeted");
        assert_eq!(impact.zone, None, "no zone tag for a world hit");
        assert_vec3_approx(impact.point, Vec3::new(0.0, 0.0, -5.0));
    }

    /// The facility, called directly with an arbitrary ray (no weapon, no
    /// camera), reports the SAME nearest entity hit the weapon path reports for
    /// that ray — proving the weapon merely delegates.
    #[test]
    fn facility_direct_call_matches_weapon_entity_hit() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        let store = head_zone_store("mob", 0.5);
        let target = spawn_zone_entity(&mut registry, "mob", Vec3::new(0.0, 0.0, -4.0));

        // The weapon fires straight down -Z (camera at origin, yaw/pitch 0).
        let origin = Vec3::ZERO;
        let direction = Vec3::new(0.0, 0.0, -1.0);

        // Direct facility call with the same ray + range (weapon range = 10).
        let direct = nearest_entity_hit(&registry, &store, 0.0, origin, direction, 10.0)
            .expect("facility resolves the entity directly");

        // The weapon path for the same ray.
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = CollisionWorld::new();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);
        let events = fire_tick_with(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            &store,
            0.0,
            1.0 / 60.0,
        );
        let impact = events.impact.expect("weapon reports the entity hit");

        assert_eq!(Some(direct.target), impact.target, "same target");
        assert_eq!(direct.zone, impact.zone, "same zone tag");
        assert_vec3_approx(direct.point, impact.point);
        assert_eq!(direct.target, target);
    }
}
