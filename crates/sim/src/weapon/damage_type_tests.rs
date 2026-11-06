// A weapon's damage type through the real fire path: every hitscan impact and
// every projectile launch carries the type its weapon names.
// See: context/lib/scripting.md §10.5 (Typed damage)

use std::sync::Arc;

use super::tests::weapon_component;
use super::*;
use postretro_collision::EnvironmentVolumes;
use postretro_entities::registry::Transform;
use postretro_foundation::defense::{DamageTypeDeclaration, DefenseVocabulary, PoolMultiplier};

fn registry_with_heavy() -> EntityRegistry {
    let mut registry = EntityRegistry::new();
    registry.set_defense_vocabulary(Arc::new(DefenseVocabulary {
        layers: Vec::new(),
        damage_types: vec![DamageTypeDeclaration {
            name: "heavy".to_string(),
            matrix: std::collections::BTreeMap::from([(
                "health".to_string(),
                PoolMultiplier::Scale(2.0),
            )]),
        }],
        default_damage_type: None,
    }));
    registry
}

fn shot(registry: &EntityRegistry, weapon: &mut WeaponComponent) -> WeaponFireEvents {
    tick_resolved_component(
        registry,
        None,
        weapon,
        "weapon.test.typed",
        0,
        &WeaponFireCommand {
            button: FireButtonState {
                pressed: true,
                active: true,
            },
            aim_origin: Vec3::ZERO,
            aim_direction: Vec3::NEG_Z,
            can_fire: true,
        },
        &WeaponPlacementDescriptor::default(),
        &CollisionWorld::new(),
        EnvironmentView::new(-9.81, &EnvironmentVolumes::default(), &[]),
        &HitZoneStore::new(),
        0.0,
        WeaponFireAuthorization::Accepted,
    )
}

#[test]
fn hitscan_impacts_carry_their_weapons_damage_type() {
    let mut registry = registry_with_heavy();
    let target = registry.spawn(Transform {
        position: Vec3::new(0.0, 0.0, -5.0),
        ..Transform::default()
    });
    registry
        .set_component(
            target,
            postretro_entities::components::health::HealthComponent::from_descriptor(
                &postretro_foundation::HealthDescriptor {
                    max: 100.0,
                    hitbox: Some(postretro_foundation::HitboxDescriptor {
                        half_extents: [1.0, 1.0, 1.0],
                        offset: None,
                    }),
                    ..Default::default()
                },
            ),
        )
        .unwrap();
    let mut weapon = weapon_component(FireMode::Semi, 0.0);
    weapon.pellet_count = 3;
    weapon.damage_type = Some(Arc::from("heavy"));
    let events = shot(&registry, &mut weapon);
    assert_eq!(events.impacts.len(), 3, "every pellet strikes the target");
    for impact in &events.impacts {
        assert_eq!(impact.target, Some(target));
        let ActivationOutcome::Hit(payload) = impact.outcome else {
            panic!("a struck target is a hit");
        };
        assert_eq!(payload.damage_type, Some(0));
    }
    // A weapon naming no type leaves the payload untyped for the default.
    let mut plain = weapon_component(FireMode::Semi, 0.0);
    let events = shot(&registry, &mut plain);
    assert!(!events.impacts.is_empty());
    for impact in &events.impacts {
        let ActivationOutcome::Hit(payload) = impact.outcome else {
            panic!("a struck target is a hit");
        };
        assert_eq!(payload.damage_type, None);
    }
}

#[test]
fn projectile_launches_freeze_their_weapons_damage_type() {
    let registry = registry_with_heavy();
    let mut weapon = weapon_component(FireMode::Semi, 0.0);
    weapon.resolution = ResolutionMode::Projectile;
    weapon.projectile = Some(postretro_foundation::ProjectileDescriptor {
        ghost: false,
        speed: 40.0,
        radius: 0.1,
        lifetime_ms: 1_000.0,
        visual: postretro_foundation::ProjectileVisual {
            body: postretro_foundation::ProjectileBodyVisual::Sprite {
                sprite: "sprites/projectiles/test.png".to_string(),
                size: 0.2,
                opacity: 1.0,
                rotation: 0.0,
                tint: [1.0, 1.0, 1.0],
                emissive: 0.0,
                frame_duration_ms: None,
            },
            trail: None,
            light: None,
            impact_light: None,
        },
    });
    weapon.damage_type = Some(Arc::from("heavy"));
    let events = shot(&registry, &mut weapon);
    assert_eq!(events.projectile_launches.len(), 1);
    assert_eq!(events.projectile_launches[0].damage_type, Some(0));
}
