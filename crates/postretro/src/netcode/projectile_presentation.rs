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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netcode::replication::produce_owned_snapshots;
    use crate::netcode::{AuthorizedShot, ClientReplication, HostCommandQueues, MovementOwners};
    use postretro_entities::{ComponentKind, EntityTypeDescriptor};
    use postretro_foundation::{
        FireMode, ProjectileBodyVisual, ProjectileVisual, ResolutionMode, WeaponDescriptor,
    };

    const FIRING_CLIENT: u64 = 7;
    const OBSERVING_CLIENT: u64 = 8;

    fn descriptor() -> ProjectileDescriptor {
        ProjectileDescriptor {
            speed: 4.0,
            radius: 0.1,
            lifetime_ms: 1_000.0,
            visual: ProjectileVisual {
                body: ProjectileBodyVisual::Sprite {
                    sprite: "sprites/projectiles/remote-bolt.png".to_string(),
                    size: 0.2,
                    opacity: 1.0,
                    rotation: 0.0,
                    tint: [0.4, 0.8, 1.0],
                },
                trail: None,
            },
        }
    }

    fn projectile_visual_descriptor() -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some("test_remote_projectile".to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: Some(WeaponDescriptor {
                damage: 10.0,
                pellet_count: 1,
                spread_degrees: 0.0,
                range: 12.0,
                cooldown_ms: 1.0,
                fire_mode: FireMode::Semi,
                resolution: ResolutionMode::Projectile,
                projectile: Some(descriptor()),
                credit_source: None,
                third_person_model: None,
                viewmodel: None,
                resource: None,
                lower_ms: 0,
                raise_ms: 0,
                block_during_reload: None,
            }),
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    #[test]
    fn remote_projectile_visual_is_hidden_from_firer_visible_to_observer_and_retires() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let shot_id = ShotId::from_parts(postretro_net::wire::NetworkId(4), 19);
        let mut open_shots = OpenAuthorizedShots::new();
        open_shots.record(
            AuthorizedShot {
                shot_id,
                pawn,
                weapon,
                fire_tick: 1,
                damage: 10.0,
                range: 12.0,
                pellet_count: 1,
                credit_source: "weapon.test.remote".to_string(),
                is_projectile: true,
                fire_origin: Vec3::ZERO,
                timeout_budget_ticks: 180,
            },
            FIRING_CLIENT,
        );
        let launch = RemoteProjectilePresentationLaunch {
            owner_client_id: FIRING_CLIENT,
            shot_id,
            origin: Vec3::new(1.0, 2.0, 3.0),
            direction: Vec3::NEG_Z,
            range: 12.0,
            descriptor_class: "test_remote_projectile".to_string(),
            projectile: descriptor(),
        };
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut replication = ServerReplication::new();
        replication.register_client(FIRING_CLIENT);
        replication.register_client(OBSERVING_CLIENT);
        let mut presentations = HostProjectilePresentations::default();

        presentations.spawn_remote(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &launch,
        );

        let visuals = presentations.flights.keys().copied().collect::<Vec<_>>();
        let [visual] = visuals.as_slice() else {
            panic!("remote projectile fire creates one presentation entity");
        };
        let visual = *visual;
        let visual_network_id = allocator
            .network_id_for_entity(visual)
            .expect("presentation entity is stamped for replication");
        assert!(replicable.contains(visual));
        assert_eq!(
            registry
                .get_component::<SpriteVisual>(visual)
                .expect("host presentation carries its descriptor sprite")
                .sprite,
            "sprites/projectiles/remote-bolt.png"
        );
        let snapshots = produce_owned_snapshots(
            &registry,
            &replicable,
            &mut allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
        replication.ingest_tick(snapshots);
        let sequence = replication.begin_batch();
        let firer_snapshot = replication
            .encode_in_batch(FIRING_CLIENT, 1, sequence)
            .expect("firing recipient is registered");
        assert!(
            firer_snapshot
                .records
                .iter()
                .all(|record| record.network_id != visual_network_id.0),
            "the firing client keeps its predicted projectile and receives no duplicate baseline"
        );
        let observer_snapshot = replication
            .encode_in_batch(OBSERVING_CLIENT, 1, sequence)
            .expect("observing recipient is registered");
        assert!(
            observer_snapshot
                .records
                .iter()
                .any(|record| record.network_id == visual_network_id.0),
            "other peers receive the Transform plus entity-class visual"
        );

        let observer_snapshot = observer_snapshot
            .validate()
            .expect("host observer snapshot is structurally valid");
        let mut observer_registry = EntityRegistry::new();
        let mut observer_replication = ClientReplication::new();
        let observer_outcome =
            observer_replication.apply_snapshot(&mut observer_registry, &observer_snapshot);
        let observer_visual = {
            let [remote] = observer_outcome.remote_entities.as_slice() else {
                panic!("observer baseline requests one descriptor-backed materialization");
            };
            assert_eq!(remote.network_id, visual_network_id);
            assert!(
                super::super::remote_materialize::materialize_armed_remote_projectile(
                    remote,
                    &[projectile_visual_descriptor()],
                    &mut observer_registry,
                )
            );
            remote.entity_id
        };
        assert!(observer_registry.exists(observer_visual));
        assert!(
            observer_registry
                .get_component::<SpriteVisual>(observer_visual)
                .is_ok()
        );
        for kind in [
            ComponentKind::Projectile,
            ComponentKind::Health,
            ComponentKind::Weapon,
            ComponentKind::PlayerMovement,
        ] {
            assert_eq!(
                observer_registry.has_component_kind(observer_visual, kind),
                Ok(false),
                "observer projectile is visual-only and carries no {kind:?} gameplay state"
            );
        }
        let observer_ack = observer_outcome
            .ack
            .expect("applied observer baseline produces an ack");
        replication.apply_ack(
            OBSERVING_CLIENT,
            observer_ack.latest_snapshot_sequence,
            &observer_ack.entity_baselines,
            &observer_ack.despawn_tombstones,
        );

        presentations.advance(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &open_shots,
            1.0 / 60.0,
        );
        assert!(
            registry.exists(visual),
            "the launch pass does not retire the visual"
        );
        open_shots.retire(shot_id);
        presentations.advance(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &open_shots,
            1.0 / 60.0,
        );
        assert!(!registry.exists(visual));
        assert!(!replicable.contains(visual));
        assert!(!allocator.maps_entity(visual));

        replication.ingest_tick(Vec::new());
        let despawn_sequence = replication.begin_batch();
        let observer_despawn = replication
            .encode_in_batch(OBSERVING_CLIENT, 2, despawn_sequence)
            .expect("observer remains registered for projectile retirement");
        let observer_despawn = observer_despawn
            .validate()
            .expect("host projectile retirement snapshot is structurally valid");
        let observer_despawn_outcome =
            observer_replication.apply_snapshot(&mut observer_registry, &observer_despawn);
        assert!(
            !observer_registry.exists(observer_visual),
            "the observer applies the replicated retirement and removes its visual"
        );
        assert!(
            observer_despawn_outcome
                .ack
                .expect("applied despawn produces an ack")
                .despawn_tombstones
                .iter()
                .any(|(network_id, _)| *network_id == visual_network_id.0),
            "client acknowledges the remote projectile tombstone"
        );
    }
}
