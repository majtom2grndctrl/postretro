// Trap weapon descriptors shared by trap, trigger and tick tests.
// See: context/lib/entity_model.md §Components (ProjectileTrap)

use postretro_foundation::{
    ActivationStepDescriptor, ActivationTrigger, ProjectileBodyVisual, ProjectileDescriptor,
    ProjectileVisual, ResolutionMode, WeaponActivationDescriptor, WeaponDescriptor,
};
use postretro_scripting_core::data_descriptors::EntityTypeDescriptor;

pub(crate) fn projectile(speed: f32) -> ProjectileDescriptor {
    ProjectileDescriptor {
        speed,
        radius: 0.0,
        lifetime_ms: 5_000.0,
        visual: ProjectileVisual {
            body: ProjectileBodyVisual::Sprite {
                sprite: "sprites/projectiles/test-bolt.png".to_string(),
                size: 0.25,
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
    }
}

pub(crate) fn action(
    trigger: ActivationTrigger,
    recovery_ms: f32,
    shots: usize,
    gap_ms: f32,
) -> WeaponActivationDescriptor {
    let mut steps = Vec::new();
    for index in 0..shots {
        if index > 0 {
            steps.push(ActivationStepDescriptor::Wait {
                duration_ms: gap_ms,
            });
        }
        steps.push(ActivationStepDescriptor::Shot {
            scale: Box::default(),
        });
    }
    WeaponActivationDescriptor {
        steps,
        ..WeaponActivationDescriptor::single(trigger, recovery_ms)
    }
}

pub(crate) fn weapon(primary: WeaponActivationDescriptor) -> WeaponDescriptor {
    WeaponDescriptor {
        sounds: None,
        knockback: None,
        damage: 7.0,
        pellet_count: 1,
        spread_degrees: 0.0,
        bloom_per_shot_degrees: 0.0,
        bloom_max_degrees: 0.0,
        bloom_decay_degrees_per_second: 0.0,
        bloom_decay_delay_ms: 0.0,
        movement_spread_degrees: 0.0,
        spread_vertical_bias: 0.0,
        range: 50.0,
        primary,
        secondary: None,
        resolution: ResolutionMode::Projectile,
        projectile: Some(projectile(12.0)),
        splash: None,
        credit_source: Some("trap.test-bolt".to_string()),
        third_person_model: None,
        viewmodel: None,
        placement: None,
        muzzle_offset: None,
        resource: None,
        lower_ms: 0,
        raise_ms: 0,
        block_during_reload: None,
    }
}

/// A single-shot press weapon with `recovery_ms`.
pub(crate) fn bolt(recovery_ms: f32) -> WeaponDescriptor {
    weapon(action(ActivationTrigger::Press, recovery_ms, 1, 0.0))
}

pub(crate) fn entity(name: &str, weapon: WeaponDescriptor) -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        faction: None,
        tolerance: None,
        canonical_name: Some(name.to_string()),
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: Some(weapon),
        touchable: None,
        mesh: None,
        health: None,
        behavior: None,
    }
}
