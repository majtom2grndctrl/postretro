// Host-owned replicated projectile-flight presentation and descriptor materialization.
// See: context/plans/in-progress/E16--projectile-resolution/index.md Task 4

use std::collections::{BTreeSet, HashMap};

use glam::Vec3;
use postretro_entities::components::billboard_emitter::{BillboardEmitterComponent, LifetimeCurve};
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::components::projectile::ProjectileComponent;
use postretro_entities::components::sprite_visual::SpriteVisual;
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::{ProjectileBodyVisual, ProjectileDescriptor};
use postretro_net::replication::ServerReplication;

use crate::sim::RemoteProjectilePresentationLaunch;

use super::{NetworkIdAllocator, OpenAuthorizedShots, ReplicableSet, ShotId};

#[derive(Debug)]
enum PresentationFlight {
    /// The listen host already owns the gameplay projectile. Its hidden replica
    /// copies that source exactly so remote peers see the host's real trajectory.
    FollowGameplay { source: EntityId },
    /// A connected client's authority projectile runs only on that client. The
    /// host advances this visual from the replicated pawn aim until its shot retires.
    StraightLine {
        shot_id: ShotId,
        direction: Vec3,
        speed: f32,
        remaining_range: f32,
        remaining_lifetime: f32,
        spawned: bool,
    },
}

/// Host-owned state for presentation-only flight entities. The entities themselves
/// carry only Transform, provenance, and visual components; this side table is the
/// deliberately non-gameplay flight driver.
#[derive(Debug, Default)]
pub(crate) struct HostProjectilePresentations {
    flights: HashMap<EntityId, PresentationFlight>,
}

impl HostProjectilePresentations {
    /// Spawn and register a remote-fire visual, excluding the firing client before
    /// that recipient's next snapshot can establish a baseline for it.
    pub(crate) fn spawn_remote(
        &mut self,
        registry: &mut EntityRegistry,
        allocator: &mut NetworkIdAllocator,
        replicable: &mut ReplicableSet,
        replication: &mut ServerReplication,
        launch: &RemoteProjectilePresentationLaunch,
    ) {
        let Some(id) = spawn_presentation_entity(
            registry,
            launch.origin,
            &launch.descriptor_class,
            Some(&launch.projectile),
        ) else {
            return;
        };
        let network_id = allocator.stamp(id).0;
        replicable.register(id);
        replication.exclude_entity_for_client(launch.owner_client_id, network_id);
        self.flights.insert(
            id,
            PresentationFlight::StraightLine {
                shot_id: launch.shot_id,
                direction: launch.direction,
                speed: launch.projectile.speed,
                remaining_range: launch.range,
                remaining_lifetime: launch.projectile.lifetime_ms / 1_000.0,
                spawned: true,
            },
        );
    }

    /// Mirror a newly spawned listen-host gameplay projectile for clients. The host
    /// intentionally attaches no visual components to this entity: the host already
    /// renders the gameplay source, while recipients materialize visuals locally
    /// from the replicated descriptor class.
    pub(crate) fn mirror_local_gameplay_projectile(
        &mut self,
        registry: &mut EntityRegistry,
        allocator: &mut NetworkIdAllocator,
        replicable: &mut ReplicableSet,
        projectile_id: EntityId,
    ) {
        let Some((transform, descriptor_class)) =
            local_projectile_presentation_source(registry, projectile_id)
        else {
            return;
        };
        let Some(id) =
            spawn_presentation_entity(registry, transform.position, &descriptor_class, None)
        else {
            return;
        };
        allocator.stamp(id);
        replicable.register(id);
        self.flights.insert(
            id,
            PresentationFlight::FollowGameplay {
                source: projectile_id,
            },
        );
    }

    /// Advance independent remote-fire visuals and mirror host-local gameplay
    /// projectiles. A remote visual ends when its open shot retires or its descriptor
    /// path bound is reached; a host-local mirror ends with its source projectile.
    pub(crate) fn advance(
        &mut self,
        registry: &mut EntityRegistry,
        allocator: &mut NetworkIdAllocator,
        replicable: &mut ReplicableSet,
        open_shots: &OpenAuthorizedShots,
        dt: f32,
    ) {
        let mut despawn = Vec::new();

        for (&id, flight) in &mut self.flights {
            let keep = match flight {
                PresentationFlight::FollowGameplay { source } => {
                    if let Ok(transform) = registry.get_component::<Transform>(*source).cloned() {
                        let _ = registry.set_component(id, transform);
                        true
                    } else {
                        false
                    }
                }
                PresentationFlight::StraightLine {
                    shot_id,
                    direction,
                    speed,
                    remaining_range,
                    remaining_lifetime,
                    spawned,
                } => {
                    if open_shots.get(*shot_id).is_none() {
                        false
                    } else if *spawned {
                        *spawned = false;
                        true
                    } else {
                        advance_straight_line_visual(
                            registry,
                            id,
                            *direction,
                            *speed,
                            remaining_range,
                            remaining_lifetime,
                            dt,
                        )
                    }
                }
            };
            if !keep {
                despawn.push(id);
            }
        }

        for id in despawn {
            self.flights.remove(&id);
            let _ = registry.despawn(id);
            replicable.unregister(id);
            allocator.forget(id);
        }
    }
}

fn local_projectile_presentation_source(
    registry: &EntityRegistry,
    projectile_id: EntityId,
) -> Option<(Transform, String)> {
    let transform = registry
        .get_component::<Transform>(projectile_id)
        .ok()?
        .clone();
    let projectile = registry
        .get_component::<ProjectileComponent>(projectile_id)
        .ok()?;
    let descriptor_class = registry
        .get_component::<DescriptorProvenance>(projectile.owner_weapon)
        .ok()?
        .canonical_name
        .clone();
    (!descriptor_class.is_empty()).then_some((transform, descriptor_class))
}

fn advance_straight_line_visual(
    registry: &mut EntityRegistry,
    id: EntityId,
    direction: Vec3,
    speed: f32,
    remaining_range: &mut f32,
    remaining_lifetime: &mut f32,
    dt: f32,
) -> bool {
    if !dt.is_finite()
        || dt <= 0.0
        || !speed.is_finite()
        || speed <= 0.0
        || !remaining_range.is_finite()
        || !remaining_lifetime.is_finite()
        || *remaining_range <= 0.0
        || *remaining_lifetime <= 0.0
    {
        return false;
    }
    let max_step = (speed * dt)
        .min(*remaining_range)
        .min(speed * *remaining_lifetime);
    if !max_step.is_finite() || max_step <= 0.0 {
        return false;
    }
    let Ok(mut transform) = registry.get_component::<Transform>(id).cloned() else {
        return false;
    };
    transform.position += direction * max_step;
    let _ = registry.set_component(id, transform);
    *remaining_range -= max_step;
    *remaining_lifetime -= dt;

    *remaining_range > 0.0 && *remaining_lifetime > 0.0
}

fn spawn_presentation_entity(
    registry: &mut EntityRegistry,
    origin: Vec3,
    descriptor_class: &str,
    visual: Option<&ProjectileDescriptor>,
) -> Option<EntityId> {
    let Some(id) = registry.try_spawn(
        Transform {
            position: origin,
            ..Transform::default()
        },
        &[],
    ) else {
        log::warn!("[Net] entity registry exhausted; dropping projectile presentation");
        return None;
    };
    let _ = registry.set_component(
        id,
        DescriptorProvenance {
            canonical_name: descriptor_class.to_string(),
            owned_components: BTreeSet::new(),
            map_overrides: BTreeSet::new(),
            spawn_path: DescriptorSpawnPath::ProjectilePresentation,
        },
    );
    if let Some(visual) = visual {
        attach_projectile_visual_components(registry, id, visual);
    }
    Some(id)
}

/// Attach the descriptor's body and optional trail without adding simulation state.
/// Both host remote-observer entities and client materialized copies use this exact
/// visual-only shape.
pub(super) fn attach_projectile_visual_components(
    registry: &mut EntityRegistry,
    id: EntityId,
    projectile: &ProjectileDescriptor,
) {
    match &projectile.visual.body {
        ProjectileBodyVisual::Sprite {
            sprite,
            size,
            opacity,
            rotation,
            tint,
        } => {
            let _ = registry.set_component(
                id,
                SpriteVisual {
                    sprite: sprite.clone(),
                    size: *size,
                    opacity: *opacity,
                    rotation: *rotation,
                    tint: *tint,
                },
            );
        }
        ProjectileBodyVisual::Model { model } => {
            let _ = registry.set_component(id, MeshComponent::stateless(model.clone()));
        }
    }
    if let Some(trail) = projectile.visual.trail.as_ref() {
        let _ = registry.set_component(
            id,
            BillboardEmitterComponent {
                rate: trail.rate,
                burst: trail.burst,
                spread: trail.spread,
                lifetime: trail.lifetime,
                velocity: trail.velocity,
                buoyancy: trail.buoyancy,
                drag: trail.drag,
                size_over_lifetime: LifetimeCurve::from(trail.size_over_lifetime.clone()),
                opacity_over_lifetime: LifetimeCurve::from(trail.opacity_over_lifetime.clone()),
                color: trail.color,
                sprite: trail.sprite.clone(),
                spin_rate: trail.spin_rate,
                spin_animation: None,
            },
        );
    }
}
