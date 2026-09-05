// Host-owned replicated projectile-flight presentation and descriptor materialization.
// See: context/lib/networking.md §Game-logic-owned apply invariant, §Phase boundaries

use std::collections::{BTreeSet, HashMap};

use glam::Vec3;
use postretro_entities::components::billboard_emitter::{BillboardEmitterComponent, LifetimeCurve};
use postretro_entities::components::light::{LightComponent, LightKind};
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::components::projectile::ProjectileComponent;
use postretro_entities::components::sprite_visual::SpriteVisual;
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::{ProjectileBodyVisual, ProjectileDescriptor, ProjectileImpactLight};
use postretro_net::replication::ServerReplication;

use crate::sim::{RemoteProjectilePresentationLaunch, projectile_model_body_rotation};
use crate::weapon;

use super::{
    NetworkIdAllocator, OpenAuthorizedShots, PROJECTILE_CONTACT_DESPAWN_REASON, ReplicableSet,
    ShotId,
};

#[derive(Debug)]
enum PresentationFlight {
    /// The listen host already owns the gameplay projectile. Its hidden replica
    /// copies that source exactly so remote peers see the host's real trajectory.
    FollowGameplay {
        source: EntityId,
        pose_dirty: bool,
        contact_point: Option<Vec3>,
        endpoint: Option<EndpointPublication>,
    },
    /// A connected client's authority projectile runs only on that client. The
    /// host advances this visual from the replicated pawn aim until its shot retires.
    StraightLine {
        shot_id: ShotId,
        direction: Vec3,
        speed: f32,
        remaining_range: f32,
        remaining_lifetime: f32,
        path_finished: bool,
        pose_dirty: bool,
        contact_point: Option<Vec3>,
        endpoint: Option<EndpointPublication>,
        impact_light: Option<ProjectileImpactLight>,
    },
}

#[derive(Debug, Clone, Copy)]
struct EndpointPublication {
    reason: u8,
    baseline_before: Option<u32>,
    requires_new_baseline: bool,
    required_baseline: Option<u32>,
}

/// Host-owned state for presentation-only flight entities. The entities themselves
/// carry only Transform, provenance, and visual components; this side table is the
/// deliberately non-gameplay flight driver.
#[derive(Debug, Default)]
pub(crate) struct HostProjectilePresentations {
    flights: HashMap<EntityId, PresentationFlight>,
}

impl HostProjectilePresentations {
    /// Retain a validated contact endpoint on the matching remote-fire visual.
    /// Unknown or already-retired shots are ignored, so contact state cannot leak
    /// forward into a later presentation.
    pub(crate) fn note_contact(&mut self, shot_id: ShotId, point: Vec3) {
        for flight in self.flights.values_mut() {
            if let PresentationFlight::StraightLine {
                shot_id: live,
                contact_point,
                ..
            } = flight
                && *live == shot_id
            {
                *contact_point = Some(point);
                return;
            }
        }
    }

    /// Record that a listen-host gameplay projectile resolved a real contact.
    /// Only a live matching mirror retains it; a host with no participants has no
    /// mirror and therefore no historic marker to replay after a later join.
    pub(crate) fn note_gameplay_contact(&mut self, projectile: EntityId, point: Vec3) {
        for flight in self.flights.values_mut() {
            if let PresentationFlight::FollowGameplay {
                source,
                contact_point,
                ..
            } = flight
                && *source == projectile
            {
                *contact_point = Some(point);
                return;
            }
        }
    }

    /// Spawn and register a remote-fire visual, excluding the firing client before
    /// that recipient's next snapshot can establish a baseline for it.
    pub(crate) fn spawn_remote(
        &mut self,
        registry: &mut EntityRegistry,
        allocator: &mut NetworkIdAllocator,
        replicable: &mut ReplicableSet,
        replication: &mut ServerReplication,
        launch: &RemoteProjectilePresentationLaunch,
        spawn_tick: u32,
    ) {
        let Some(id) = spawn_presentation_entity(
            registry,
            Transform {
                position: launch.origin,
                rotation: projectile_model_body_rotation(
                    &launch.projectile.visual.body,
                    launch.direction,
                ),
                ..Transform::default()
            },
            &launch.descriptor_class,
            Some(&launch.projectile),
            spawn_tick,
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
                path_finished: false,
                pose_dirty: true,
                contact_point: None,
                endpoint: None,
                impact_light: launch.projectile.visual.impact_light.clone(),
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
        replication: &ServerReplication,
        projectile_id: EntityId,
        spawn_tick: u32,
    ) {
        let Some((transform, descriptor_class)) =
            local_projectile_presentation_source(registry, projectile_id)
        else {
            return;
        };
        self.mirror_gameplay_projectile_from_source(
            registry,
            allocator,
            replicable,
            replication,
            projectile_id,
            transform,
            &descriptor_class,
            spawn_tick,
        );
    }

    /// Mirror a host-authoritative gameplay projectile whose descriptor class is
    /// supplied by the producer. Enemy projectiles deliberately use the resolved
    /// weapon name here: their `owner_weapon` is the enemy damage-provenance id,
    /// not a descriptor-backed wieldable entity.
    pub(crate) fn mirror_gameplay_projectile_with_descriptor_class(
        &mut self,
        registry: &mut EntityRegistry,
        allocator: &mut NetworkIdAllocator,
        replicable: &mut ReplicableSet,
        replication: &ServerReplication,
        projectile_id: EntityId,
        descriptor_class: &str,
        spawn_tick: u32,
    ) {
        let Some(transform) = gameplay_projectile_transform(registry, projectile_id) else {
            return;
        };
        self.mirror_gameplay_projectile_from_source(
            registry,
            allocator,
            replicable,
            replication,
            projectile_id,
            transform,
            descriptor_class,
            spawn_tick,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn mirror_gameplay_projectile_from_source(
        &mut self,
        registry: &mut EntityRegistry,
        allocator: &mut NetworkIdAllocator,
        replicable: &mut ReplicableSet,
        replication: &ServerReplication,
        projectile_id: EntityId,
        transform: Transform,
        descriptor_class: &str,
        spawn_tick: u32,
    ) {
        if !replication.has_registered_clients() || descriptor_class.is_empty() {
            return;
        }
        let Some(id) =
            spawn_presentation_entity(registry, transform, descriptor_class, None, spawn_tick)
        else {
            return;
        };
        allocator.stamp(id);
        replicable.register(id);
        self.flights.insert(
            id,
            PresentationFlight::FollowGameplay {
                source: projectile_id,
                pose_dirty: true,
                contact_point: None,
                endpoint: None,
            },
        );
    }

    /// Mark each live pose as represented by the baseline replication just
    /// ingested. This is not a delivery signal: endpoint retirement still waits
    /// for recipient acknowledgments of that exact-or-newer baseline.
    pub(crate) fn mark_current_poses_ingested(&mut self) {
        for flight in self.flights.values_mut() {
            match flight {
                PresentationFlight::FollowGameplay { pose_dirty, .. }
                | PresentationFlight::StraightLine { pose_dirty, .. } => {
                    *pose_dirty = false;
                }
            }
        }
    }

    /// Advance independent remote-fire visuals and host-local mirrors. Nominal path
    /// completion holds while shot authorization remains open. Every terminal pose
    /// then remains replicated until all intended recipients acknowledge its current
    /// baseline; only the following tombstone may retire it.
    pub(crate) fn advance(
        &mut self,
        registry: &mut EntityRegistry,
        allocator: &mut NetworkIdAllocator,
        replicable: &mut ReplicableSet,
        replication: &mut ServerReplication,
        open_shots: &OpenAuthorizedShots,
        dt: f32,
    ) {
        let mut despawn = Vec::new();
        let mut host_flashes = Vec::new();

        for (&id, flight) in &mut self.flights {
            let Some(network_id) = allocator.network_id_for_entity(id).map(|id| id.0) else {
                despawn.push((id, 0));
                continue;
            };
            let retire_reason = match flight {
                PresentationFlight::FollowGameplay {
                    source,
                    pose_dirty,
                    contact_point,
                    endpoint,
                    ..
                } => {
                    if endpoint.is_none() {
                        if let Some(point) = contact_point.take() {
                            let changed = set_presentation_endpoint(registry, id, point);
                            *pose_dirty |= changed;
                            *endpoint = Some(begin_endpoint_publication(
                                replication,
                                network_id,
                                PROJECTILE_CONTACT_DESPAWN_REASON,
                                *pose_dirty,
                            ));
                        } else if let Ok(transform) =
                            registry.get_component::<Transform>(*source).cloned()
                        {
                            let changed = registry
                                .get_component::<Transform>(id)
                                .is_ok_and(|current| *current != transform);
                            let _ = registry.set_component(id, transform);
                            *pose_dirty |= changed;
                        } else {
                            *endpoint = Some(begin_endpoint_publication(
                                replication,
                                network_id,
                                0,
                                *pose_dirty,
                            ));
                        }
                    }
                    endpoint
                        .as_mut()
                        .and_then(|endpoint| endpoint_ready(replication, network_id, endpoint))
                }
                PresentationFlight::StraightLine {
                    shot_id,
                    direction,
                    speed,
                    remaining_range,
                    remaining_lifetime,
                    path_finished,
                    pose_dirty,
                    contact_point,
                    endpoint,
                    impact_light,
                    ..
                } => {
                    if endpoint.is_none()
                        && let Some(point) = contact_point.take()
                    {
                        let changed = set_presentation_endpoint(registry, id, point);
                        *pose_dirty |= changed;
                        *path_finished = true;
                        if let Some(config) = impact_light.clone() {
                            host_flashes.push((point, config));
                        }
                        *endpoint = Some(begin_endpoint_publication(
                            replication,
                            network_id,
                            PROJECTILE_CONTACT_DESPAWN_REASON,
                            *pose_dirty,
                        ));
                    }

                    if endpoint.is_none() && !*path_finished {
                        let before = registry
                            .get_component::<Transform>(id)
                            .ok()
                            .map(|transform| transform.position);
                        *path_finished = !advance_straight_line_visual(
                            registry,
                            id,
                            *direction,
                            *speed,
                            remaining_range,
                            remaining_lifetime,
                            dt,
                        );
                        let after = registry
                            .get_component::<Transform>(id)
                            .ok()
                            .map(|transform| transform.position);
                        *pose_dirty |= before != after;
                    }

                    // A finished path remains live while its authorized shot is
                    // retained. A later valid contact may still replace this nominal
                    // endpoint and must not lose its observer flash.
                    if endpoint.is_none() && open_shots.get(*shot_id).is_none() {
                        *endpoint = Some(begin_endpoint_publication(
                            replication,
                            network_id,
                            0,
                            *pose_dirty,
                        ));
                    }
                    endpoint
                        .as_mut()
                        .and_then(|endpoint| endpoint_ready(replication, network_id, endpoint))
                }
            };
            if let Some(reason) = retire_reason {
                despawn.push((id, reason));
            }
        }

        for (point, config) in host_flashes {
            weapon::spawn_projectile_impact_light(registry, point, &config);
        }
        for (id, despawn_reason) in despawn {
            self.flights.remove(&id);
            if despawn_reason != 0
                && let Some(network_id) = allocator.network_id_for_entity(id)
            {
                replication.set_next_despawn_reason(network_id.0, despawn_reason);
            }
            let _ = registry.despawn(id);
            replicable.unregister(id);
            allocator.forget(id);
        }
    }
}

fn begin_endpoint_publication(
    replication: &ServerReplication,
    network_id: u32,
    reason: u8,
    requires_new_baseline: bool,
) -> EndpointPublication {
    EndpointPublication {
        reason,
        baseline_before: replication.current_baseline_id(network_id),
        requires_new_baseline,
        required_baseline: None,
    }
}

fn endpoint_ready(
    replication: &ServerReplication,
    network_id: u32,
    endpoint: &mut EndpointPublication,
) -> Option<u8> {
    let baseline_id = match endpoint.required_baseline {
        Some(baseline_id) => baseline_id,
        None => {
            let baseline_id = replication.current_baseline_id(network_id)?;
            if endpoint.requires_new_baseline && endpoint.baseline_before == Some(baseline_id) {
                return None;
            }
            endpoint.required_baseline = Some(baseline_id);
            baseline_id
        }
    };
    replication
        .baseline_acked_by_all_recipients(network_id, baseline_id)
        .then_some(endpoint.reason)
}

fn set_presentation_endpoint(registry: &mut EntityRegistry, id: EntityId, point: Vec3) -> bool {
    let Ok(mut transform) = registry.get_component::<Transform>(id).cloned() else {
        return false;
    };
    let changed = transform.position != point;
    transform.position = point;
    let _ = registry.set_component(id, transform);
    changed
}

fn local_projectile_presentation_source(
    registry: &EntityRegistry,
    projectile_id: EntityId,
) -> Option<(Transform, String)> {
    let transform = gameplay_projectile_transform(registry, projectile_id)?;
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

fn gameplay_projectile_transform(
    registry: &EntityRegistry,
    projectile_id: EntityId,
) -> Option<Transform> {
    registry
        .get_component::<ProjectileComponent>(projectile_id)
        .ok()?;
    registry
        .get_component::<Transform>(projectile_id)
        .ok()
        .copied()
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
    transform: Transform,
    descriptor_class: &str,
    visual: Option<&ProjectileDescriptor>,
    spawn_tick: u32,
) -> Option<EntityId> {
    let Some(id) = registry.try_spawn(transform, &[]) else {
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
        attach_projectile_visual_components(registry, id, visual, spawn_tick);
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
    spawn_tick: u32,
) {
    match &projectile.visual.body {
        ProjectileBodyVisual::Sprite {
            sprite,
            size,
            opacity,
            rotation,
            tint,
            ..
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
                spin_animation: trail.spin_animation.as_ref().map(|animation| {
                    postretro_entities::components::billboard_emitter::SpinAnimation {
                        duration: animation.duration,
                        rate_curve: animation.rate_curve.clone(),
                    }
                }),
            },
        );
    }
    if let Some(light) = projectile.visual.light.as_ref() {
        let origin = registry
            .get_component::<Transform>(id)
            .map(|transform| transform.position.to_array())
            .unwrap_or([0.0; 3]);
        let _ = registry.set_component(
            id,
            LightComponent {
                origin,
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

    // This state is presentation-local: the body cadence comes from shared
    // descriptor content, while elapsed age is derived from the host's fixed-tick
    // epoch. Keeping it in the registry side table avoids a wire-format field.
    if registry.projectile_presentation_age(id).is_err() {
        let flipbook_active = matches!(
            &projectile.visual.body,
            ProjectileBodyVisual::Sprite {
                frame_duration_ms: Some(_),
                ..
            }
        );
        let _ = registry.set_projectile_presentation_age(
            id,
            postretro_entities::components::projectile::ProjectilePresentationAge {
                spawn_tick,
                flipbook_active,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netcode::replication::produce_owned_snapshots;
    use crate::netcode::{AuthorizedShot, ClientReplication, HostCommandQueues, MovementOwners};
    use postretro_entities::components::deferred_effect::{
        DeferredEffectComponent, DeferredEffectKind,
    };
    use postretro_entities::{ComponentKind, EntityTypeDescriptor};
    use postretro_foundation::{
        FireMode, ProjectileBodyVisual, ProjectileImpactLight, ProjectileLight, ProjectileVisual,
        ResolutionMode, WeaponDescriptor,
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
                    emissive: 0.0,
                    frame_duration_ms: None,
                },
                trail: None,
                light: None,
                impact_light: None,
            },
        }
    }

    fn model_descriptor() -> ProjectileDescriptor {
        let mut descriptor = descriptor();
        descriptor.visual.body = ProjectileBodyVisual::Model {
            model: "models/projectiles/rocket.gltf".to_string(),
        };
        descriptor
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
                placement: None,
                muzzle_offset: None,
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

    fn model_projectile_visual_descriptor() -> EntityTypeDescriptor {
        let mut descriptor = projectile_visual_descriptor();
        descriptor
            .weapon
            .as_mut()
            .expect("test descriptor declares its projectile weapon")
            .projectile = Some(model_descriptor());
        descriptor
    }

    fn ack_current_entity_baseline(
        replication: &mut ServerReplication,
        client_id: u64,
        network_id: u32,
        tick: u32,
    ) {
        let sequence = replication.begin_batch();
        let snapshot = replication
            .encode_in_batch(client_id, tick, sequence)
            .expect("registered test recipient")
            .validate()
            .expect("test snapshot validates");
        let baseline_id = snapshot
            .records
            .iter()
            .find_map(|record| match record {
                postretro_net::wire::EntityRecord::FullBaseline {
                    network_id: id,
                    baseline_id,
                    ..
                } if *id == network_id => Some(*baseline_id),
                postretro_net::wire::EntityRecord::Delta {
                    network_id: id,
                    new_baseline_id,
                    ..
                } if *id == network_id => Some(*new_baseline_id),
                _ => None,
            })
            .expect("current entity baseline is present for its unacked recipient");
        replication.apply_ack(client_id, sequence, &[(network_id, baseline_id)], &[]);
    }

    // Regression: remote projectile presentations started with an identity
    // transform, so rigid projectile models did not face their replicated aim.
    #[test]
    fn remote_model_projectile_presentation_preserves_aim_orientation_through_replication() {
        let mut registry = EntityRegistry::new();
        let shot_id = ShotId::from_parts(postretro_net::wire::NetworkId(4), 31);
        let direction = Vec3::new(2.0, 1.0, -3.0).normalize();
        let origin = Vec3::new(1.0, 2.0, 3.0);
        let launch = RemoteProjectilePresentationLaunch {
            owner_client_id: FIRING_CLIENT,
            shot_id,
            origin,
            direction,
            range: 12.0,
            descriptor_class: "test_remote_projectile".to_string(),
            projectile: model_descriptor(),
        };
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut replication = ServerReplication::new();
        replication.register_client(OBSERVING_CLIENT);
        let mut presentations = HostProjectilePresentations::default();

        presentations.spawn_remote(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &launch,
            0,
        );
        let visual = *presentations
            .flights
            .keys()
            .next()
            .expect("remote fire creates one presentation entity");
        let initial = *registry
            .get_component::<Transform>(visual)
            .expect("presentation has a transform");
        assert!(
            (initial.rotation * Vec3::Z).distance(direction) <= 1.0e-6,
            "the host presentation aims its model along the remote launch direction"
        );
        assert_eq!(
            registry
                .get_component::<MeshComponent>(visual)
                .expect("host presentation materializes the model body")
                .model,
            "models/projectiles/rocket.gltf"
        );

        let mut open_shots = OpenAuthorizedShots::new();
        open_shots.record(
            AuthorizedShot {
                shot_id,
                pawn: EntityId::from_raw(1),
                weapon: EntityId::from_raw(2),
                fire_tick: 1,
                damage: 10.0,
                range: 12.0,
                pellet_count: 1,
                credit_source: "weapon.test.remote".to_string(),
                is_projectile: true,
                fire_origin: origin,
                timeout_budget_ticks: 180,
            },
            FIRING_CLIENT,
        );
        presentations.advance(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &open_shots,
            0.25,
        );
        let advanced = *registry
            .get_component::<Transform>(visual)
            .expect("presentation remains live after a flight step");
        assert!(
            advanced.position.distance(origin + direction) <= 1.0e-6,
            "the observer presentation advances along the same direction it renders"
        );
        assert!(
            (advanced.rotation * Vec3::Z).distance(direction) <= 1.0e-6,
            "straight-line presentation retains its launch orientation while flying"
        );

        let snapshots = produce_owned_snapshots(
            &registry,
            &replicable,
            &mut allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
        replication.ingest_tick(snapshots);
        presentations.mark_current_poses_ingested();
        let sequence = replication.begin_batch();
        let observer_snapshot = replication
            .encode_in_batch(OBSERVING_CLIENT, 1, sequence)
            .expect("observer is registered")
            .validate()
            .expect("observer snapshot is valid");
        let mut observer_registry = EntityRegistry::new();
        let mut observer_replication = ClientReplication::new();
        let outcome =
            observer_replication.apply_snapshot(&mut observer_registry, &observer_snapshot);
        let remote = outcome
            .remote_entities
            .into_iter()
            .next()
            .expect("observer materializes the presentation descriptor");
        assert!(
            super::super::remote_materialize::materialize_armed_remote_projectile(
                &remote,
                &[model_projectile_visual_descriptor()],
                &mut observer_registry,
                0,
            )
        );
        let observer_transform = observer_registry
            .get_component::<Transform>(remote.entity_id)
            .expect("observer applies the host transform");
        assert!(
            (observer_transform.rotation * Vec3::Z).distance(direction) <= 1.0e-6,
            "the replicated visual keeps the model aligned with the firing aim"
        );
    }

    #[test]
    fn enemy_gameplay_mirror_uses_the_resolved_weapon_class_not_enemy_provenance() {
        let mut registry = EntityRegistry::new();
        let enemy = registry.spawn(Transform::default());
        registry
            .set_component(
                enemy,
                DescriptorProvenance {
                    canonical_name: "limitator".to_string(),
                    owned_components: BTreeSet::new(),
                    map_overrides: BTreeSet::new(),
                    spawn_path: DescriptorSpawnPath::MapPlacement,
                },
            )
            .expect("enemy accepts its authored provenance");
        let source = registry.spawn(Transform {
            position: Vec3::new(1.0, 2.0, 3.0),
            ..Transform::default()
        });
        registry
            .set_component(
                source,
                ProjectileComponent {
                    direction: Vec3::NEG_Z.to_array(),
                    speed: 4.0,
                    radius: 0.1,
                    remaining_range: 12.0,
                    remaining_lifetime: 1.0,
                    damage: 10.0,
                    credit_source: "enemy.rifle".to_string(),
                    owner_pawn: enemy,
                    // Enemy projectiles preserve the enemy id for common impact
                    // damage provenance; it must never choose the visual class.
                    owner_weapon: enemy,
                    spawned: true,
                    predicted_shot_id: None,
                    elapsed_flight_age: 0.0,
                    flipbook_active: false,
                    impact_light: None,
                },
            )
            .expect("gameplay projectile accepts the common component");
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut replication = ServerReplication::new();
        replication.register_client(OBSERVING_CLIENT);
        let mut presentations = HostProjectilePresentations::default();

        presentations.mirror_gameplay_projectile_with_descriptor_class(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &replication,
            source,
            "enemy_rifle",
            0,
        );

        let visuals = presentations.flights.keys().copied().collect::<Vec<_>>();
        let [visual] = visuals.as_slice() else {
            panic!("enemy gameplay projectile creates one presentation mirror");
        };
        let host_provenance = registry
            .get_component::<DescriptorProvenance>(*visual)
            .expect("presentation carries a descriptor class");
        assert_eq!(host_provenance.canonical_name, "enemy_rifle");
        assert_ne!(host_provenance.canonical_name, "limitator");

        let snapshots = produce_owned_snapshots(
            &registry,
            &replicable,
            &mut allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
        replication.ingest_tick(snapshots);
        let sequence = replication.begin_batch();
        let snapshot = replication
            .encode_in_batch(OBSERVING_CLIENT, 1, sequence)
            .expect("connected observer receives the mirror baseline")
            .validate()
            .expect("the existing snapshot layout accepts the mirror");
        assert!(snapshot.records.iter().any(|record| {
            matches!(
                record,
                postretro_net::wire::EntityRecord::FullBaseline {
                    entity_class: Some(entity_class),
                    ..
                } if entity_class == "projectile:enemy_rifle"
            )
        }));

        let mut observer_registry = EntityRegistry::new();
        let mut observer_replication = ClientReplication::new();
        let outcome = observer_replication.apply_snapshot(&mut observer_registry, &snapshot);
        let [remote] = outcome.remote_entities.as_slice() else {
            panic!("the existing Transform plus entity-class baseline creates one remote");
        };
        let mut weapon_descriptor = projectile_visual_descriptor();
        weapon_descriptor.canonical_name = Some("enemy_rifle".to_string());
        assert!(
            super::super::remote_materialize::materialize_armed_remote_projectile(
                remote,
                &[weapon_descriptor],
                &mut observer_registry,
                0,
            )
        );
        assert_eq!(
            observer_registry
                .get_component::<SpriteVisual>(remote.entity_id)
                .expect("client materializes the weapon projectile visual")
                .sprite,
            "sprites/projectiles/remote-bolt.png"
        );
        assert_eq!(
            observer_registry
                .get_component::<DescriptorProvenance>(remote.entity_id)
                .expect("remote mirror retains its class")
                .canonical_name,
            "enemy_rifle"
        );
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
        let mut projectile = descriptor();
        let ProjectileBodyVisual::Sprite {
            emissive,
            frame_duration_ms,
            ..
        } = &mut projectile.visual.body
        else {
            unreachable!("the shared remote fixture starts with a sprite body");
        };
        *emissive = 3.0;
        *frame_duration_ms = Some(60.0);
        projectile.visual.light = Some(ProjectileLight {
            color: [0.2, 0.7, 1.0],
            intensity: 2.5,
            falloff_range: 6.0,
            falloff_model: postretro_foundation::FalloffKind::InverseSquared,
        });
        projectile.visual.impact_light = Some(ProjectileImpactLight {
            color: [0.55, 0.85, 1.0],
            intensity: 4.0,
            radius: 5.0,
            peak_radius: Some(9.0),
            fade_ms: 180.0,
        });
        let launch = RemoteProjectilePresentationLaunch {
            owner_client_id: FIRING_CLIENT,
            shot_id,
            origin: Vec3::new(1.0, 2.0, 3.0),
            direction: Vec3::NEG_Z,
            range: 12.0,
            descriptor_class: "test_remote_projectile".to_string(),
            projectile: projectile.clone(),
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
            0,
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
        presentations.mark_current_poses_ingested();
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
            let mut descriptor = projectile_visual_descriptor();
            descriptor
                .weapon
                .as_mut()
                .expect("test descriptor carries a weapon")
                .projectile = Some(projectile.clone());
            assert!(
                super::super::remote_materialize::materialize_armed_remote_projectile(
                    remote,
                    &[descriptor],
                    &mut observer_registry,
                    0,
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
        let observer_light = observer_registry
            .get_component::<LightComponent>(observer_visual)
            .expect("observer materializes the descriptor travel light locally");
        assert!(observer_light.is_dynamic);
        assert!(observer_light.follow_transform);
        assert!(
            Vec3::from_array(observer_light.color).distance(Vec3::new(0.2, 0.7, 1.0)) <= 1.0e-6
        );
        let observer_timing = observer_registry
            .projectile_presentation_age(observer_visual)
            .expect("observer records the cadence-owned presentation clock");
        assert!(observer_timing.flipbook_active);
        assert_eq!(observer_timing.spawn_tick, 0);
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
        let provenance = observer_registry
            .get_component::<DescriptorProvenance>(observer_visual)
            .expect("remote materialization marks the exact projectile-presentation shape");
        assert_eq!(
            provenance.spawn_path,
            DescriptorSpawnPath::ProjectilePresentation
        );
        let mut collector =
            crate::scripting_systems::particle_render::ParticleRenderCollector::new();
        collector.register_sprite("sprites/projectiles/remote-bolt.png");
        collector.collect_at_tick(
            &observer_registry,
            None,
            &postretro_visibility::VisibleCells::DrawAll,
            11.0,
        );
        let collected = collector.iter_collections().collect::<Vec<_>>();
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].0, "sprites/projectiles/remote-bolt.png");
        assert_eq!(
            collected[0].1.len(),
            postretro_render_cpu::smoke::SPRITE_INSTANCE_SIZE,
            "visual-only remote materialization is eligible for the sprite collector"
        );
        let packed_age = f32::from_ne_bytes(collected[0].1[12..16].try_into().unwrap());
        assert!(
            (packed_age - 11.0 * crate::frame_timing::TICK_DURATION.as_secs_f32()).abs() < 1.0e-6,
            "the remote flipbook uses the shared fixed-tick epoch rather than frame time"
        );
        let mut observer_bridge = crate::scripting_systems::light_bridge::LightBridge::new();
        observer_bridge.populate_from_level(&[], &mut observer_registry, 0);
        observer_bridge.absorb_dynamic_lights(&observer_registry);
        let observer_lights = observer_bridge
            .update(&mut observer_registry, 0.18, 1.0)
            .expect("the client-side absorb path enrolls presentation lights");
        assert_eq!(
            observer_lights.lights_bytes.len(),
            postretro_lighting::GPU_LIGHT_SIZE,
            "one observer presentation produces one dynamic light record"
        );
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
            &mut replication,
            &open_shots,
            1.0 / 60.0,
        );
        assert!(
            registry.exists(visual),
            "the launch pass does not retire the visual"
        );
        let advanced_snapshots = produce_owned_snapshots(
            &registry,
            &replicable,
            &mut allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
        replication.ingest_tick(advanced_snapshots);
        presentations.mark_current_poses_ingested();
        let advanced_sequence = replication.begin_batch();
        let observer_advanced = replication
            .encode_in_batch(OBSERVING_CLIENT, 2, advanced_sequence)
            .expect("observer receives the advanced projectile state")
            .validate()
            .expect("advanced observer snapshot validates");
        assert!(
            observer_advanced.records.iter().any(|record| match record {
                postretro_net::wire::EntityRecord::FullBaseline { network_id, .. }
                | postretro_net::wire::EntityRecord::Delta { network_id, .. } => {
                    *network_id == visual_network_id.0
                }
                postretro_net::wire::EntityRecord::Despawn { .. } => false,
            }),
            "the observer sees flight motion before retirement"
        );
        let advanced_outcome =
            observer_replication.apply_snapshot(&mut observer_registry, &observer_advanced);
        let advanced_ack = advanced_outcome
            .ack
            .expect("applied endpoint baseline produces an ack");
        replication.apply_ack(
            OBSERVING_CLIENT,
            advanced_ack.latest_snapshot_sequence,
            &advanced_ack.entity_baselines,
            &advanced_ack.despawn_tombstones,
        );
        let contact_point = registry
            .get_component::<Transform>(visual)
            .expect("advanced presentation has a contact endpoint")
            .position;
        presentations.note_contact(shot_id, contact_point);
        open_shots.retire(shot_id);
        presentations.advance(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &open_shots,
            1.0 / 60.0,
        );
        assert!(!registry.exists(visual));
        assert!(!replicable.contains(visual));
        assert!(!allocator.maps_entity(visual));

        replication.ingest_tick(Vec::new());
        let despawn_sequence = replication.begin_batch();
        let observer_despawn = replication
            .encode_in_batch(OBSERVING_CLIENT, 3, despawn_sequence)
            .expect("observer remains registered for projectile retirement");
        let observer_despawn = observer_despawn
            .validate()
            .expect("host projectile retirement snapshot is structurally valid");
        assert!(
            observer_despawn.records.iter().any(|record| {
                matches!(
                    record,
                    postretro_net::wire::EntityRecord::Despawn {
                        network_id,
                        reason,
                        ..
                    } if *network_id == visual_network_id.0
                        && *reason == PROJECTILE_CONTACT_DESPAWN_REASON
                )
            }),
            "contact retirement rides the existing despawn reason byte"
        );
        let observer_despawn_outcome =
            observer_replication.apply_snapshot(&mut observer_registry, &observer_despawn);
        let retired = &observer_despawn_outcome.retired_projectile_presentations;
        let [retired] = retired.as_slice() else {
            panic!("contact despawn surfaces one retired projectile presentation");
        };
        assert_eq!(retired.entity_class, "test_remote_projectile");
        assert!(
            retired
                .transform
                .position
                .distance(Vec3::new(1.0, 2.0, 3.0 - 4.0 / 60.0))
                <= 1.0e-6
        );
        let mut descriptor = projectile_visual_descriptor();
        descriptor
            .weapon
            .as_mut()
            .expect("test descriptor carries a weapon")
            .projectile = Some(projectile.clone());
        super::super::spawn_retired_projectile_presentation_flashes(
            &mut observer_registry,
            &[descriptor],
            &observer_despawn_outcome.retired_projectile_presentations,
        );
        assert_eq!(
            observer_registry
                .iter_with_kind(ComponentKind::Light)
                .count(),
            1,
            "observer client spawns its impact flash locally from the contact tombstone"
        );
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

    #[test]
    fn contact_remote_presentation_retirement_spawns_configured_endpoint_flash() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let shot_id = ShotId::from_parts(postretro_net::wire::NetworkId(4), 42);
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
        let impact_light = ProjectileImpactLight {
            color: [1.0, 0.5, 0.2],
            intensity: 6.0,
            radius: 7.0,
            peak_radius: Some(16.0),
            fade_ms: 240.0,
        };
        let mut projectile = descriptor();
        projectile.visual.impact_light = Some(impact_light.clone());
        let launch = RemoteProjectilePresentationLaunch {
            owner_client_id: FIRING_CLIENT,
            shot_id,
            origin: Vec3::ZERO,
            direction: Vec3::NEG_Z,
            range: 12.0,
            descriptor_class: "test_remote_projectile".to_string(),
            projectile,
        };
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut replication = ServerReplication::new();
        replication.register_client(OBSERVING_CLIENT);
        let mut presentations = HostProjectilePresentations::default();
        presentations.spawn_remote(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &launch,
            0,
        );
        let visual = *presentations
            .flights
            .keys()
            .next()
            .expect("remote fire creates one presentation entity");

        presentations.advance(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &open_shots,
            0.25,
        );
        let visual_network_id = allocator
            .network_id_for_entity(visual)
            .expect("presentation entity is stamped");
        replication.ingest_tick(produce_owned_snapshots(
            &registry,
            &replicable,
            &mut allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        ));
        presentations.mark_current_poses_ingested();
        ack_current_entity_baseline(&mut replication, OBSERVING_CLIENT, visual_network_id.0, 1);
        let contact_point = registry
            .get_component::<Transform>(visual)
            .expect("advanced presentation has a contact endpoint")
            .position;
        presentations.note_contact(shot_id, contact_point);
        open_shots.retire(shot_id);
        presentations.advance(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &open_shots,
            0.0,
        );

        assert!(!registry.exists(visual));
        let flashes = registry
            .iter_with_kind(ComponentKind::Light)
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        let [flash] = flashes.as_slice() else {
            panic!("presentation retirement creates exactly one endpoint flash");
        };
        let light = registry
            .get_component::<LightComponent>(*flash)
            .expect("endpoint flash carries a dynamic point light");
        assert!(Vec3::from_array(light.origin).distance(Vec3::new(0.0, 0.0, -1.0)) <= 1.0e-6);
        assert!(!light.follow_transform);
        for (actual, expected) in light
            .animation
            .as_ref()
            .expect("endpoint flash animates")
            .radius
            .as_deref()
            .expect("endpoint flash expands")
            .iter()
            .zip([7.0, 16.0])
        {
            assert!((*actual - expected).abs() <= f32::EPSILON);
        }
        let deferred = registry
            .get_component::<DeferredEffectComponent>(*flash)
            .expect("endpoint flash self-despawns through deferred effects");
        assert_eq!(deferred.pending[0].kind, DeferredEffectKind::Despawn);
    }

    #[test]
    fn travel_bound_presentation_retirement_spawns_no_endpoint_flash_or_contact_reason() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let shot_id = ShotId::from_parts(postretro_net::wire::NetworkId(4), 43);
        let mut open_shots = OpenAuthorizedShots::new();
        open_shots.record(
            AuthorizedShot {
                shot_id,
                pawn,
                weapon,
                fire_tick: 1,
                damage: 10.0,
                range: 0.5,
                pellet_count: 1,
                credit_source: "weapon.test.remote".to_string(),
                is_projectile: true,
                fire_origin: Vec3::ZERO,
                timeout_budget_ticks: 180,
            },
            FIRING_CLIENT,
        );
        let mut projectile = descriptor();
        projectile.visual.impact_light = Some(ProjectileImpactLight {
            color: [1.0, 0.5, 0.2],
            intensity: 6.0,
            radius: 7.0,
            peak_radius: Some(16.0),
            fade_ms: 240.0,
        });
        let launch = RemoteProjectilePresentationLaunch {
            owner_client_id: FIRING_CLIENT,
            shot_id,
            origin: Vec3::ZERO,
            direction: Vec3::NEG_Z,
            range: 0.5,
            descriptor_class: "test_remote_projectile".to_string(),
            projectile,
        };
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut replication = ServerReplication::new();
        replication.register_client(OBSERVING_CLIENT);
        let mut presentations = HostProjectilePresentations::default();
        presentations.spawn_remote(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &launch,
            0,
        );
        let visual = *presentations
            .flights
            .keys()
            .next()
            .expect("remote fire creates one presentation entity");
        let visual_network_id = allocator
            .network_id_for_entity(visual)
            .expect("presentation entity is stamped");

        presentations.advance(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &open_shots,
            0.25,
        );
        assert!(
            registry.exists(visual),
            "path endpoint is held while its shot authorization remains open"
        );
        open_shots.retire(shot_id);
        presentations.advance(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &open_shots,
            0.0,
        );
        let snapshots = produce_owned_snapshots(
            &registry,
            &replicable,
            &mut allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
        replication.ingest_tick(snapshots);
        presentations.mark_current_poses_ingested();
        presentations.advance(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &open_shots,
            0.0,
        );
        assert!(
            registry.exists(visual),
            "an attempted endpoint snapshot is not treated as delivery"
        );
        ack_current_entity_baseline(&mut replication, OBSERVING_CLIENT, visual_network_id.0, 2);
        presentations.advance(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &open_shots,
            0.0,
        );

        assert!(!registry.exists(visual));
        assert_eq!(
            registry.iter_with_kind(ComponentKind::Light).count(),
            0,
            "travel-bound expiry must not spawn an impact flash"
        );
        replication.ingest_tick(Vec::new());
        let sequence = replication.begin_batch();
        let despawn = replication
            .encode_in_batch(OBSERVING_CLIENT, 3, sequence)
            .expect("observer receives retirement")
            .validate()
            .expect("retirement snapshot validates");
        assert!(
            despawn.records.iter().any(|record| {
                matches!(
                    record,
                    postretro_net::wire::EntityRecord::Despawn {
                        network_id,
                        reason: 0,
                        ..
                    } if *network_id == visual_network_id.0
                )
            }),
            "travel-bound expiry keeps the default despawn reason"
        );
    }

    #[test]
    fn local_gameplay_contact_mirror_retires_with_reason_without_duplicate_host_flash() {
        let mut registry = EntityRegistry::new();
        let weapon = registry.spawn(Transform::default());
        let _ = registry.set_component(
            weapon,
            DescriptorProvenance {
                canonical_name: "test_remote_projectile".to_string(),
                owned_components: BTreeSet::new(),
                map_overrides: BTreeSet::new(),
                spawn_path: DescriptorSpawnPath::DefaultWeapon,
            },
        );
        let source = registry.spawn(Transform::default());
        let owner_pawn = registry.spawn(Transform::default());
        let impact_light = ProjectileImpactLight {
            color: [1.0, 0.5, 0.2],
            intensity: 6.0,
            radius: 7.0,
            peak_radius: Some(16.0),
            fade_ms: 240.0,
        };
        let _ = registry.set_component(
            source,
            ProjectileComponent {
                direction: Vec3::NEG_Z.to_array(),
                speed: 4.0,
                radius: 0.1,
                remaining_range: 12.0,
                remaining_lifetime: 1.0,
                damage: 10.0,
                credit_source: "weapon.test.local".to_string(),
                owner_pawn,
                owner_weapon: weapon,
                spawned: false,
                predicted_shot_id: None,
                elapsed_flight_age: 0.0,
                flipbook_active: false,
                impact_light: Some(impact_light),
            },
        );
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut replication = ServerReplication::new();
        replication.register_client(OBSERVING_CLIENT);
        let mut presentations = HostProjectilePresentations::default();
        presentations.mirror_local_gameplay_projectile(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &replication,
            source,
            0,
        );
        let visual = *presentations
            .flights
            .keys()
            .next()
            .expect("local gameplay projectile creates one mirror");
        let visual_network_id = allocator
            .network_id_for_entity(visual)
            .expect("mirror is stamped for replication");
        let snapshots = produce_owned_snapshots(
            &registry,
            &replicable,
            &mut allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
        replication.ingest_tick(snapshots);
        presentations.mark_current_poses_ingested();

        let contact_point = Vec3::new(2.0, 3.0, -4.0);
        presentations.note_gameplay_contact(source, contact_point);
        let _ = registry.despawn(source);
        presentations.advance(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &OpenAuthorizedShots::new(),
            0.0,
        );
        assert!(registry.exists(visual));
        assert!(
            registry
                .get_component::<Transform>(visual)
                .expect("mirror keeps a contact endpoint")
                .position
                .distance(contact_point)
                <= 1.0e-6
        );
        let snapshots = produce_owned_snapshots(
            &registry,
            &replicable,
            &mut allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
        replication.ingest_tick(snapshots);
        presentations.mark_current_poses_ingested();
        ack_current_entity_baseline(&mut replication, OBSERVING_CLIENT, visual_network_id.0, 2);
        presentations.advance(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &OpenAuthorizedShots::new(),
            0.0,
        );

        assert!(!registry.exists(visual));
        assert_eq!(
            registry.iter_with_kind(ComponentKind::Light).count(),
            0,
            "authoritative host flash already exists; mirror retirement must not duplicate it"
        );
        replication.ingest_tick(Vec::new());
        let sequence = replication.begin_batch();
        let despawn = replication
            .encode_in_batch(OBSERVING_CLIENT, 3, sequence)
            .expect("observer receives mirror retirement")
            .validate()
            .expect("retirement snapshot validates");
        assert!(
            despawn.records.iter().any(|record| {
                matches!(
                    record,
                    postretro_net::wire::EntityRecord::Despawn {
                        network_id,
                        reason,
                        ..
                    } if *network_id == visual_network_id.0
                        && *reason == PROJECTILE_CONTACT_DESPAWN_REASON
                )
            }),
            "local gameplay contact mirror uses the contact despawn reason"
        );
    }

    #[test]
    fn listen_host_without_participants_keeps_no_mirror_or_historic_contact() {
        let mut registry = EntityRegistry::new();
        let weapon = registry.spawn(Transform::default());
        registry
            .set_component(
                weapon,
                DescriptorProvenance {
                    canonical_name: "test_remote_projectile".to_string(),
                    owned_components: BTreeSet::new(),
                    map_overrides: BTreeSet::new(),
                    spawn_path: DescriptorSpawnPath::DefaultWeapon,
                },
            )
            .unwrap();
        let owner_pawn = registry.spawn(Transform::default());
        let source = registry.spawn(Transform::default());
        registry
            .set_component(
                source,
                ProjectileComponent {
                    direction: Vec3::NEG_Z.to_array(),
                    speed: 4.0,
                    radius: 0.1,
                    remaining_range: 12.0,
                    remaining_lifetime: 1.0,
                    damage: 10.0,
                    credit_source: "weapon.test.local".to_string(),
                    owner_pawn,
                    owner_weapon: weapon,
                    spawned: false,
                    predicted_shot_id: None,
                    elapsed_flight_age: 0.0,
                    flipbook_active: false,
                    impact_light: None,
                },
            )
            .unwrap();
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut replication = ServerReplication::new();
        let mut presentations = HostProjectilePresentations::default();

        presentations.mirror_local_gameplay_projectile(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &replication,
            source,
            0,
        );
        presentations.note_gameplay_contact(source, Vec3::new(1.0, 2.0, 3.0));
        replication.register_client(OBSERVING_CLIENT);
        presentations.advance(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &OpenAuthorizedShots::new(),
            0.0,
        );

        assert!(presentations.flights.is_empty());
        assert_eq!(replicable.iter().count(), 0);
        assert_eq!(registry.iter_with_kind(ComponentKind::Light).count(), 0);
    }

    // Regression: a valid contact arriving after nominal visual completion was
    // erased as ordinary expiry before observers could materialize its flash.
    #[test]
    fn late_contact_after_nominal_path_end_survives_until_endpoint_ack() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let shot_id = ShotId::from_parts(postretro_net::wire::NetworkId(4), 21);
        let mut open_shots = OpenAuthorizedShots::new();
        open_shots.record(
            AuthorizedShot {
                shot_id,
                pawn,
                weapon,
                fire_tick: 1,
                damage: 10.0,
                range: 0.01,
                pellet_count: 1,
                credit_source: "weapon.test.short".to_string(),
                is_projectile: true,
                fire_origin: Vec3::ZERO,
                timeout_budget_ticks: 180,
            },
            FIRING_CLIENT,
        );
        let mut short_descriptor = descriptor();
        short_descriptor.speed = 100.0;
        short_descriptor.lifetime_ms = 1.0;
        short_descriptor.visual.impact_light = Some(ProjectileImpactLight {
            color: [1.0, 0.5, 0.2],
            intensity: 6.0,
            radius: 7.0,
            peak_radius: None,
            fade_ms: 240.0,
        });
        let launch = RemoteProjectilePresentationLaunch {
            owner_client_id: FIRING_CLIENT,
            shot_id,
            origin: Vec3::ZERO,
            direction: Vec3::NEG_Z,
            range: 0.01,
            descriptor_class: "test_remote_projectile".to_string(),
            projectile: short_descriptor,
        };
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut replication = ServerReplication::new();
        replication.register_client(OBSERVING_CLIENT);
        let mut presentations = HostProjectilePresentations::default();
        presentations.spawn_remote(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &launch,
            0,
        );
        let visual = *presentations
            .flights
            .keys()
            .next()
            .expect("short shot spawns one observer visual");
        let network_id = allocator
            .network_id_for_entity(visual)
            .expect("short observer visual is stamped");

        presentations.advance(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &open_shots,
            1.0 / 60.0,
        );
        assert!(registry.exists(visual));
        assert!(
            registry
                .get_component::<Transform>(visual)
                .expect("short shot endpoint remains live")
                .position
                .z
                < 0.0,
            "the presentation advances to its nominal endpoint"
        );
        presentations.advance(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &open_shots,
            1.0 / 60.0,
        );
        assert!(
            registry.exists(visual),
            "the finished endpoint survives catch-up ticks before serialization"
        );

        let contact_point = registry
            .get_component::<Transform>(visual)
            .expect("finished path retains its endpoint")
            .position;
        presentations.note_contact(shot_id, contact_point);
        open_shots.retire(shot_id);
        presentations.advance(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &open_shots,
            0.0,
        );
        assert_eq!(
            registry.iter_with_kind(ComponentKind::Light).count(),
            1,
            "late contact still produces the host observer's configured flash"
        );

        let snapshots = produce_owned_snapshots(
            &registry,
            &replicable,
            &mut allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
        replication.ingest_tick(snapshots);
        presentations.mark_current_poses_ingested();
        let sequence = replication.begin_batch();
        let observer_snapshot = replication
            .encode_in_batch(OBSERVING_CLIENT, 2, sequence)
            .expect("observer receives the held short-shot endpoint");
        assert!(
            observer_snapshot
                .records
                .iter()
                .any(|record| record.network_id == network_id.0)
        );

        presentations.advance(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &open_shots,
            1.0 / 60.0,
        );
        assert!(
            registry.exists(visual),
            "sending one endpoint snapshot does not prove recipient delivery"
        );
        ack_current_entity_baseline(&mut replication, OBSERVING_CLIENT, network_id.0, 3);
        presentations.advance(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut replication,
            &open_shots,
            1.0 / 60.0,
        );
        assert!(!registry.exists(visual));
        assert!(!replicable.contains(visual));
    }
}
