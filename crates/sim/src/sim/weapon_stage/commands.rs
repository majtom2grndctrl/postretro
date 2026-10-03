mod local;
mod remote;
pub(in crate::sim) use local::{run_local_weapon_command, run_local_weapon_command_with_content};
pub(in crate::sim) use remote::run_remote_weapon_commands;

use std::cell::RefCell;
use std::rc::Rc;

use glam::{Quat, Vec3};

use crate::collision::CollisionWorld;
use crate::emission::{WeaponEmission, descriptor_name, entity_emitter, reload_emission};
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::sprite_collection::derive_collection_id;
use crate::weapon::{self, FireButtonState, WeaponFireAuthorization, WeaponFireCommand};
use postretro_combat_model::{
    AuthorizedShot, MAX_OPEN_SHOT_AGE_TICKS, OpenAuthorizedShot, projectile_timeout_budget_ticks,
};
use postretro_entities::components::billboard_emitter::{BillboardEmitterComponent, LifetimeCurve};
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::light::{LightComponent, LightKind};
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::components::player_movement::PlayerMovementComponent;
use postretro_entities::components::projectile::ProjectileComponent;
use postretro_entities::components::sprite_visual::SpriteVisual;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::components::wieldable_state::WieldableState;
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{EntityId, EntityRegistry, EntityTypeDescriptor, Transform};
use postretro_foundation::{ProjectileBodyVisual, ResolutionMode, WeaponPlacementDescriptor};

use super::super::{
    PostMovementCommand, ReloadDelivery, RemotePawnCommand, RemoteProjectileFireRejection,
    RemoteProjectilePresentationLaunch,
};
use super::impact::apply_authorized_weapon_impact_damage;
use super::machine::{tick_weapon_machine, tick_weapon_machine_activation};
use super::state::{
    WieldableStateEvent, begin_raising, finish_lowering, transition_wieldable_state,
};

#[derive(Debug, Default)]
pub(in crate::sim) struct LocalWeaponCommandResult {
    pub(in crate::sim) reload_deliveries: Vec<ReloadDelivery>,
    /// `reload_deliveries` stamped for presentation at this tick, in order.
    pub(in crate::sim) reload_emissions: Vec<WeaponEmission>,
    pub(in crate::sim) weapon_events: Vec<WeaponEmission>,
    pub(in crate::sim) repointed_pawn: Option<EntityId>,
    pub(in crate::sim) projectile_spawns: Vec<EntityId>,
    #[cfg(test)]
    pub(in crate::sim) weapon_impact_points: Vec<Vec3>,
}

#[derive(Debug, Default)]
pub(in crate::sim) struct RemoteWeaponCommandResult {
    pub(in crate::sim) activation_progress: Vec<super::super::RemoteActivationProgress>,
    pub(in crate::sim) authorized_shots: Vec<OpenAuthorizedShot>,
    pub(in crate::sim) projectile_presentation_launches: Vec<RemoteProjectilePresentationLaunch>,
    pub(in crate::sim) rejected_projectile_fires: Vec<RemoteProjectileFireRejection>,
    pub(in crate::sim) reload_deliveries: Vec<ReloadDelivery>,
    /// `reload_deliveries` stamped for presentation at this tick, in order.
    pub(in crate::sim) reload_emissions: Vec<WeaponEmission>,
    pub(in crate::sim) weapon_events: Vec<WeaponEmission>,
}

pub(in crate::sim) fn weapon_fire_command(
    button: FireButtonState,
    post_movement: PostMovementCommand,
) -> WeaponFireCommand {
    // The aim normalization and `can_fire` gate below are degenerate-input guards.
    // `camera.aim_ray()` already returns normalized, finite values in normal operation;
    // these checks protect against NaN/zero vectors from headless or mocked callers.
    if post_movement.aim_origin.is_finite()
        && let Some(aim_direction) = normalize_aim_direction(post_movement.aim_direction)
    {
        return WeaponFireCommand {
            button,
            aim_origin: post_movement.aim_origin,
            aim_direction,
            can_fire: true,
        };
    }

    WeaponFireCommand {
        button,
        aim_origin: Vec3::ZERO,
        aim_direction: Vec3::Z,
        can_fire: false,
    }
}

fn normalize_aim_direction(direction: Vec3) -> Option<Vec3> {
    if !direction.is_finite() {
        return None;
    }
    let length_squared = direction.length_squared();
    if !length_squared.is_finite() || length_squared <= 1.0e-12 {
        return None;
    }
    Some(direction / length_squared.sqrt())
}

/// Where a projectile came from, recorded at spawn for its contact
/// presentation: the weapon descriptor it was fired from, and the first
/// projectile of its activation (`None` for the first, or a lone projectile).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectileSource {
    pub weapon: Option<String>,
    pub activation: Option<EntityId>,
}

pub fn spawn_projectile(
    registry: &mut EntityRegistry,
    owner_pawn: EntityId,
    owner_weapon: EntityId,
    launch: weapon::ProjectileLaunch,
    predicted_shot_id: Option<postretro_foundation::ShotId>,
    source: ProjectileSource,
) -> Option<EntityId> {
    // Resolve every hit-time visual before moving the body out of the descriptor
    // below. Impact resolution must not consult the owner weapon, which can be
    // gone before a long-lived projectile makes contact.
    let impact_light = launch.descriptor.visual.impact_light.clone();
    let Some(projectile_id) = registry.try_spawn(
        Transform {
            position: launch.origin,
            scale: Vec3::splat(launch.model_scale),
            rotation: projectile_model_body_rotation(
                &launch.descriptor.visual.body,
                launch.direction,
            ),
            ..Transform::default()
        },
        &[],
    ) else {
        log::warn!("[Weapon] entity registry exhausted; dropping projectile launch");
        return None;
    };

    let component = ProjectileComponent {
        source_action: launch.action,
        source_shot: launch.shot_id,
        direction: launch.direction.to_array(),
        speed: launch.speed,
        radius: launch.radius,
        remaining_range: launch.range,
        remaining_lifetime: launch.lifetime,
        damage: launch.damage,
        knockback_impulse: launch.knockback_impulse.to_array(),
        credit_source: launch.credit_source,
        owner_pawn,
        owner_weapon,
        spawned: true,
        predicted_shot_id,
        elapsed_flight_age: 0.0,
        flipbook_active: matches!(
            &launch.descriptor.visual.body,
            ProjectileBodyVisual::Sprite {
                frame_duration_ms: Some(_),
                ..
            }
        ),
        impact_light,
        splash: launch.splash,
        source_weapon: source.weapon,
        activation: source.activation,
    };
    let _ = registry.set_component(projectile_id, component);

    if let Some(light) = launch.descriptor.visual.light.clone() {
        let _ = registry.set_component(
            projectile_id,
            LightComponent {
                origin: launch.origin.to_array(),
                light_type: LightKind::Point,
                intensity: light.intensity,
                color: light.color,
                falloff_model: light.falloff_model,
                falloff_range: light.falloff_range,
                cone_angle_inner: None,
                cone_angle_outer: None,
                cone_direction: None,
                is_dynamic: true,
                animated_slot: None,
                follow_transform: true,
                carrier: None,
                animation: None,
            },
        );
    }

    match launch.descriptor.visual.body {
        ProjectileBodyVisual::Sprite {
            sprite,
            size,
            opacity,
            rotation,
            tint,
            emissive,
            frame_duration_ms,
        } => {
            let _ = registry.set_component(
                projectile_id,
                SpriteVisual {
                    collection: derive_collection_id(
                        &sprite,
                        None,
                        frame_duration_ms,
                        emissive,
                        None,
                        None,
                    ),
                    size,
                    opacity,
                    rotation,
                    tint,
                },
            );
        }
        ProjectileBodyVisual::Model { model } => {
            let _ = registry.set_component(projectile_id, MeshComponent::stateless(model));
        }
    }
    if let Some(trail) = launch.descriptor.visual.trail {
        let _ = registry.set_component(
            projectile_id,
            BillboardEmitterComponent {
                rate: trail.rate,
                burst: trail.burst,
                spread: trail.spread,
                lifetime: trail.lifetime,
                velocity: trail.velocity,
                buoyancy: trail.buoyancy,
                drag: trail.drag,
                size_over_lifetime: LifetimeCurve::from(trail.size_over_lifetime),
                opacity_over_lifetime: LifetimeCurve::from(trail.opacity_over_lifetime),
                color: trail.color,
                sprite: trail.sprite,
                spin_rate: trail.spin_rate,
                spin_animation: trail.spin_animation.map(|animation| {
                    postretro_entities::components::billboard_emitter::SpinAnimation {
                        duration: animation.duration,
                        rate_curve: animation.rate_curve,
                    }
                }),
            },
        );
    }

    Some(projectile_id)
}

/// The renderer applies entity rotation directly to rigid glTF geometry. Projectile
/// models therefore use the same authored mesh-forward convention as other meshes:
/// local `+Z` faces their travel direction. Sprite bodies remain camera-facing and
/// intentionally retain the identity transform rotation.
pub fn projectile_model_body_rotation(body: &ProjectileBodyVisual, direction: Vec3) -> Quat {
    if !matches!(body, ProjectileBodyVisual::Model { .. }) || !direction.is_finite() {
        return Quat::IDENTITY;
    }

    let length_squared = direction.length_squared();
    if !length_squared.is_finite() || length_squared <= 1.0e-12 {
        return Quat::IDENTITY;
    }

    Quat::from_rotation_arc(Vec3::Z, direction / length_squared.sqrt())
}

pub(crate) fn normalize_inventory_liveness(
    registry: &mut EntityRegistry,
    pawn: EntityId,
) -> Option<(Inventory, bool)> {
    let mut inventory = registry.get_component::<Inventory>(pawn).ok()?.clone();
    let original_active = inventory.active_wieldable();
    let mut changed = false;
    for wieldable in &mut inventory.wieldables {
        if wieldable.is_some_and(|id| {
            !registry.exists(id)
                || registry.has_component_kind(id, postretro_entities::ComponentKind::Weapon)
                    != Ok(true)
        }) {
            *wieldable = None;
            changed = true;
        }
    }

    let active_is_live = inventory
        .wieldables
        .get(inventory.active_slot)
        .copied()
        .flatten()
        .is_some();
    let target_is_live = inventory
        .switch_target
        .is_none_or(|slot| inventory.wieldables.get(slot).copied().flatten().is_some());
    if !active_is_live || !target_is_live {
        inventory.active_slot = inventory
            .wieldables
            .iter()
            .position(Option::is_some)
            .unwrap_or_default();
        inventory.switch_target = None;
        inventory.switch_origin = None;
        for weapon in inventory.wieldables.iter().flatten().copied() {
            let Ok(mut component) = registry.get_component::<WeaponComponent>(weapon).cloned()
            else {
                continue;
            };
            if matches!(
                component.state,
                WieldableState::Lowering | WieldableState::Raising
            ) {
                finish_lowering(&mut component);
                let _ = registry.set_component(weapon, component);
            }
        }
        changed = true;
    }

    if changed {
        let _ = registry.set_component(pawn, inventory.clone());
    }
    let active_changed = inventory.active_wieldable() != original_active;
    Some((inventory, active_changed))
}

/// Normalize every live pawn inventory independently of command arrival. Host-owned
/// remote pawns can go several ticks without a command, but a despawned sibling must
/// still abandon its equip transition and update presentation in that interval.
pub(in crate::sim) fn normalize_all_inventory_liveness(
    registry: &mut EntityRegistry,
) -> Vec<EntityId> {
    let pawns = registry
        .iter_with_kind(postretro_entities::ComponentKind::Inventory)
        .map(|(pawn, _)| pawn)
        .collect::<Vec<_>>();
    pawns
        .into_iter()
        .filter(|pawn| {
            normalize_inventory_liveness(registry, *pawn)
                .is_some_and(|(_, active_changed)| active_changed)
        })
        .collect()
}

/// Apply a host refusal to the locally-running switch machine. A refusal can
/// arrive after the local lower already repointed, so the inventory retains the
/// original slot until this path settles it. Equip state is presentation-only on
/// the client and must not survive the correction as a second visible transition.
pub(crate) fn refuse_local_switch(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    refused_slot: usize,
    rollback_slot: usize,
) -> bool {
    let Ok(mut inventory) = registry.get_component::<Inventory>(pawn).cloned() else {
        return false;
    };

    let refused_in_flight = inventory.switch_target == Some(refused_slot);
    let refused_after_repoint = inventory.active_slot == refused_slot;
    if !refused_in_flight && !refused_after_repoint {
        return false;
    }
    if inventory
        .wieldables
        .get(rollback_slot)
        .copied()
        .flatten()
        .is_none()
    {
        return false;
    }
    inventory.active_slot = rollback_slot;
    inventory.switch_target = None;
    inventory.switch_origin = None;

    for weapon in inventory.wieldables.iter().flatten().copied() {
        let Ok(mut component) = registry.get_component::<WeaponComponent>(weapon).cloned() else {
            continue;
        };
        if matches!(
            component.state,
            WieldableState::Lowering | WieldableState::Raising
        ) {
            component.state = WieldableState::Idle;
            component.state_total_ms = 0;
            component.state_remaining_ms = 0;
            component.state_elapsed_sub_ms = 0.0;
            let _ = registry.set_component(weapon, component);
        }
    }

    let _ = registry.set_component(pawn, inventory);
    true
}

#[cfg(test)]
mod projectile_spawn_tests {
    use super::*;
    use postretro_foundation::{
        ProjectileBodyVisual, ProjectileDescriptor, ProjectileImpactLight, ProjectileLight,
        ProjectileTrailSpinAnimation, ProjectileTrailVisual, ProjectileVisual, SplashDescriptor,
    };

    fn launch(visual: ProjectileVisual) -> weapon::ProjectileLaunch {
        weapon::ProjectileLaunch {
            action: None,
            shot_id: None,
            model_scale: 1.0,
            knockback_impulse: glam::Vec3::ZERO,
            origin: Vec3::new(1.0, 2.0, 3.0),
            direction: Vec3::NEG_Z,
            speed: 40.0,
            radius: 0.2,
            range: 64.0,
            lifetime: 2.0,
            damage: 25.0,
            credit_source: "plasma.primary".to_string(),
            descriptor: ProjectileDescriptor {
                speed: 40.0,
                radius: 0.2,
                lifetime_ms: 2000.0,
                visual,
            },
            splash: None,
        }
    }

    #[test]
    fn projectile_spawn_attaches_sprite_body_and_optional_trail() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let visual = ProjectileVisual {
            body: ProjectileBodyVisual::Sprite {
                sprite: "sprites/plasma.png".to_string(),
                size: 0.4,
                opacity: 0.9,
                rotation: 0.25,
                tint: [0.2, 0.8, 1.0],
                emissive: 0.0,
                frame_duration_ms: None,
            },
            trail: Some(ProjectileTrailVisual {
                sprite: "sprites/trail.png".to_string(),
                rate: 60.0,
                lifetime: 0.5,
                burst: None,
                spread: 0.1,
                velocity: [0.0, 0.0, 0.0],
                buoyancy: 0.0,
                drag: 0.0,
                size_over_lifetime: vec![0.2, 0.0],
                opacity_over_lifetime: vec![1.0, 0.0],
                color: [1.0, 1.0, 1.0],
                spin_rate: 0.0,
                spin_animation: Some(ProjectileTrailSpinAnimation {
                    duration: 0.75,
                    rate_curve: vec![0.0, 2.0, -1.0],
                }),
            }),
            light: None,
            impact_light: None,
        };

        let projectile = spawn_projectile(
            &mut registry,
            pawn,
            weapon,
            launch(visual),
            None,
            ProjectileSource::default(),
        )
        .expect("projectile spawns");
        assert!(
            registry
                .get_component::<ProjectileComponent>(projectile)
                .is_ok()
        );
        assert_eq!(
            registry
                .get_component::<SpriteVisual>(projectile)
                .expect("sprite body attaches")
                .collection,
            derive_collection_id("sprites/plasma.png", None, None, 0.0, None, None)
        );
        let trail = registry
            .get_component::<BillboardEmitterComponent>(projectile)
            .expect("trail emitter attaches");
        assert_eq!(trail.sprite, "sprites/trail.png");
        let animation = trail
            .spin_animation
            .as_ref()
            .expect("trail spin animation materializes locally");
        assert!((animation.duration - 0.75).abs() <= f32::EPSILON);
        assert_eq!(animation.rate_curve, [0.0, 2.0, -1.0]);
    }

    #[test]
    fn projectile_spawn_attaches_rigid_mesh_body() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let visual = ProjectileVisual {
            body: ProjectileBodyVisual::Model {
                model: "models/rocket.gltf".to_string(),
            },
            trail: None,
            light: None,
            impact_light: None,
        };
        let mut launch = launch(visual);
        launch.direction = Vec3::new(3.0, 2.0, -4.0).normalize();

        let projectile = spawn_projectile(
            &mut registry,
            pawn,
            weapon,
            launch.clone(),
            None,
            ProjectileSource::default(),
        )
        .expect("projectile spawns");
        let mesh = registry
            .get_component::<MeshComponent>(projectile)
            .expect("rigid mesh body attaches");
        assert_eq!(mesh.model, "models/rocket.gltf");
        assert!(mesh.animation.is_none(), "projectile mesh is rigid");
        let transform = registry
            .get_component::<Transform>(projectile)
            .expect("projectile carries its launch transform");
        let rendered_forward = transform.rotation * Vec3::Z;
        assert!(
            rendered_forward.distance(launch.direction) <= 1.0e-6,
            "a rigid projectile model faces the aim direction captured at fire time"
        );
    }

    #[test]
    fn projectile_spawn_attaches_a_following_dynamic_point_light() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let visual = ProjectileVisual {
            body: ProjectileBodyVisual::Sprite {
                sprite: "sprites/plasma.png".to_string(),
                size: 0.4,
                opacity: 0.9,
                rotation: 0.25,
                tint: [0.2, 0.8, 1.0],
                emissive: 0.0,
                frame_duration_ms: None,
            },
            trail: None,
            light: Some(ProjectileLight {
                color: [0.2, 0.7, 1.0],
                intensity: 2.5,
                falloff_range: 6.0,
                falloff_model: postretro_foundation::FalloffKind::InverseDistance,
            }),
            impact_light: None,
        };

        let projectile = spawn_projectile(
            &mut registry,
            pawn,
            weapon,
            launch(visual),
            None,
            ProjectileSource::default(),
        )
        .expect("projectile spawns");
        let light = registry
            .get_component::<LightComponent>(projectile)
            .expect("descriptor light materializes with its projectile");
        assert_eq!(light.light_type, LightKind::Point);
        assert!(light.is_dynamic);
        assert!(light.follow_transform);
        assert!(Vec3::from_array(light.origin).distance(Vec3::new(1.0, 2.0, 3.0)) <= 1.0e-6);
        assert!(Vec3::from_array(light.color).distance(Vec3::new(0.2, 0.7, 1.0)) <= 1.0e-6);
        assert!((light.intensity - 2.5).abs() <= f32::EPSILON);
        assert!((light.falloff_range - 6.0).abs() <= f32::EPSILON);
        assert_eq!(
            light.falloff_model,
            postretro_foundation::FalloffKind::InverseDistance
        );
    }

    #[test]
    fn projectile_spawn_retains_resolved_impact_light_before_moving_body_visual() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let impact_light = ProjectileImpactLight {
            color: [0.5, 0.8, 1.0],
            intensity: 4.0,
            radius: 5.0,
            peak_radius: Some(10.0),
            fade_ms: 200.0,
        };
        let visual = ProjectileVisual {
            body: ProjectileBodyVisual::Model {
                model: "models/rocket.gltf".to_string(),
            },
            trail: None,
            light: None,
            impact_light: Some(impact_light.clone()),
        };

        let splash = SplashDescriptor {
            knockback: None,
            radius: 8.0,
            min_fraction: 0.25,
            self_damage: true,
        };
        let mut resolved_launch = launch(visual);
        resolved_launch.splash = Some(splash.clone());
        let projectile = spawn_projectile(
            &mut registry,
            pawn,
            weapon,
            resolved_launch,
            None,
            ProjectileSource::default(),
        )
        .expect("projectile spawns");
        let component = registry
            .get_component::<ProjectileComponent>(projectile)
            .expect("projectile state survives body materialization");
        assert_eq!(
            component.impact_light,
            Some(impact_light),
            "the later contact path never has to resolve the owner weapon"
        );
        assert_eq!(
            component.splash,
            Some(splash),
            "the later contact path snapshots splash tuning instead of rereading the weapon"
        );
    }

    #[test]
    fn projectile_model_rotation_handles_vertical_and_invalid_directions() {
        let body = ProjectileBodyVisual::Model {
            model: "models/rocket.gltf".to_string(),
        };
        let vertical = projectile_model_body_rotation(&body, Vec3::Y);
        assert!(vertical.is_finite());
        assert!(
            (vertical * Vec3::Z).distance(Vec3::Y) <= 1.0e-6,
            "vertical aim remains a valid model orientation"
        );

        for invalid in [Vec3::ZERO, Vec3::new(f32::NAN, 0.0, 1.0)] {
            assert_eq!(
                projectile_model_body_rotation(&body, invalid),
                Quat::IDENTITY,
                "invalid launch direction preserves the safe identity fallback"
            );
        }
    }
}
