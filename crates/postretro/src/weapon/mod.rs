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
            let hit = cast_ray(
                collision_world,
                Point::new(origin.x, origin.y, origin.z),
                Vector::new(direction.x, direction.y, direction.z),
                range,
            );
            if let Some(hit) = hit {
                let point = origin + direction * hit.time_of_impact;
                let normal = Vec3::new(hit.normal.x, hit.normal.y, hit.normal.z);
                events.impact = Some(WeaponImpact {
                    point,
                    normal,
                    outcome: ActivationOutcome::Hit(DamagePayload { amount: damage }),
                });
            }
        }
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Binding, InputSystem, PhysicalInput};
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

    fn spawn_weapon(registry: &mut EntityRegistry, component: WeaponComponent) -> EntityId {
        let id = registry.spawn(Transform::default());
        registry
            .set_component(id, component)
            .expect("weapon component should attach");
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
}
