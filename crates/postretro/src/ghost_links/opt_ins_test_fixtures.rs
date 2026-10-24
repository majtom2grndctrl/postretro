// Test fixtures: descriptor snapshots that opt archetypes into ghosting.
// See: context/lib/entity_model.md §7d

use postretro_entities::{DataRegistry, EntityTypeDescriptor};
use postretro_foundation::{
    ActivationTrigger, ProjectileBodyVisual, ProjectileDescriptor, ProjectileVisual,
    ResolutionMode, WeaponActivationDescriptor, WeaponDescriptor,
};

/// A component-less descriptor named `name` whose top-level `ghost` is `ghost`.
pub(crate) fn descriptor(name: &str, ghost: bool) -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        faction: None,
        tolerance: None,
        canonical_name: Some(name.to_string()),
        ghost,
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: None,
        touchable: None,
        mesh: None,
        health: None,
        behavior: None,
    }
}

/// A projectile weapon descriptor named `name` whose `projectile.ghost` is
/// `ghost`; the descriptor itself never sets `ghost`.
pub(crate) fn projectile_weapon(name: &str, ghost: bool) -> EntityTypeDescriptor {
    let mut weapon = descriptor(name, false);
    weapon.weapon = Some(WeaponDescriptor {
        sounds: None,
        knockback: None,
        damage: 10.0,
        pellet_count: 1,
        spread_degrees: 0.0,
        bloom_per_shot_degrees: 0.0,
        bloom_max_degrees: 0.0,
        bloom_decay_degrees_per_second: 0.0,
        bloom_decay_delay_ms: 0.0,
        movement_spread_degrees: 0.0,
        spread_vertical_bias: 0.0,
        range: 64.0,
        primary: WeaponActivationDescriptor::single(ActivationTrigger::Press, 100.0),
        secondary: None,
        resolution: ResolutionMode::Projectile,
        projectile: Some(ProjectileDescriptor {
            speed: 30.0,
            radius: 0.1,
            lifetime_ms: 2_000.0,
            visual: ProjectileVisual {
                body: ProjectileBodyVisual::Model {
                    model: "models/rocket.glb".to_string(),
                },
                trail: None,
                light: None,
                impact_light: None,
            },
            ghost,
        }),
        splash: None,
        credit_source: None,
        third_person_model: None,
        viewmodel: None,
        placement: None,
        muzzle_offset: None,
        resource: None,
        lower_ms: 0,
        raise_ms: 0,
        block_during_reload: None,
    });
    weapon
}

/// A committed descriptor snapshot, as mod init or a staged reload commits it.
pub(crate) fn data_registry(descriptors: Vec<EntityTypeDescriptor>) -> DataRegistry {
    let mut data = DataRegistry::new();
    data.replace_entity_types(descriptors);
    data
}

/// The usual co-op fixture: the player descriptor opts in, nothing else does.
pub(crate) fn player_opted_in() -> DataRegistry {
    data_registry(vec![descriptor("player", true)])
}
