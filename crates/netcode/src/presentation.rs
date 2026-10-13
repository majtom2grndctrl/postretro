// Host-to-client routing and engine conversion for passive presentation events.
// See: context/lib/networking.md

use std::collections::{BTreeMap, HashMap};

use glam::Vec3;
#[cfg(test)]
use postretro_combat_model::{AuthorizedShot, MAX_OPEN_SHOT_AGE_TICKS, ShotId};
use postretro_entities::{
    EntityId, EntityRegistry, PresentationFact, PresentationFade, PresentationMotion,
    PresentationSpawn, PresentationTemplateHandle,
};
use postretro_foundation::NavAgentParams;
use postretro_net::transport::NetServer;
use postretro_net::wire::{
    NetworkId, PresentationFact as WirePresentationFact, ServerPresentationMessage,
    ServerPresentationPayload,
};
use postretro_scripting_core::data_descriptors::{
    EntityTypeDescriptor, HitboxDescriptor, PresentationTemplate,
};

use crate::impact_policy::ClientOverlayConfig;
use crate::presentation_pool::PresentationPool;
use crate::scripting::builtins::data_archetype::ai_capsule_center_from_feet_offset;
use crate::scripting_systems::hit_zones::{HitZoneStore, model_matrix};

use super::ClientReplication;
#[cfg(test)]
use super::{MovementOwners, NetworkIdAllocator};

mod overlays;
pub use overlays::{
    ClientOverlayFactState, HostOverlayFactTracker, update_client_overlay_anchors,
    update_host_overlay_roster,
};

#[derive(Debug, Clone, Copy)]
struct ClientOverlayHitbox {
    half_extents: Vec3,
    offset: Vec3,
}

/// Share each new impact-policy transient with every participating client and
/// keep one local copy for the host. The registry's routing cursor consumes the
/// offer even when there are no recipients or rendering skips local intake.
pub fn route_host_presentation_spawns(registry: &mut EntityRegistry, server: &mut NetServer) {
    // Resolve the bounded participant set only when new work exists, once per
    // batch rather than allocating it for every impact or every empty frame.
    let mut clients = None;
    registry.visit_unrouted_presentation_spawns(|spawn| {
        let clients = clients.get_or_insert_with(|| server.participating_clients());
        if clients.is_empty() {
            return;
        }
        let message = presentation_message_from_spawn(spawn);
        for &client_id in clients.iter() {
            // A failed addressed send is intentionally a dropped cosmetic.
            let _ = server.send_presentation(client_id, message.clone());
        }
    });
}

/// Convert received passive presentation events into client-local state. Spawn
/// messages enter registry intake. Complete overlay rosters retain only current
/// host-authored state while an entity baseline or synchronized clock is pending.
#[allow(clippy::too_many_arguments)]
pub fn ingest_client_presentation_messages(
    registry: &mut EntityRegistry,
    messages: Vec<ServerPresentationMessage>,
    descriptors: &[EntityTypeDescriptor],
    templates: &HashMap<String, PresentationTemplate>,
    overlay_state: &mut ClientOverlayFactState,
    replication: &ClientReplication,
    pool: &mut PresentationPool,
    overlay_config: Option<&ClientOverlayConfig>,
    hit_zones: &HitZoneStore,
    agent_params: Option<NavAgentParams>,
    anim_time: f64,
    local_script_time: f64,
    rtt_micros: Option<f64>,
) {
    for message in messages {
        match message.payload {
            // Commands present through the system command queue
            // (`presentation_commands`), split out before this intake.
            ServerPresentationPayload::Command(_) => {
                log::warn!("[Netcode] presentation command reached the passive intake; skipped");
            }
            ServerPresentationPayload::Spawn {
                template_id,
                anchor,
                value,
                facts,
            } => {
                if !anchor.iter().all(|component| component.is_finite()) {
                    log::warn!(
                        "[Netcode] dropped presentation spawn `{template_id}` with a non-finite world anchor"
                    );
                    continue;
                }
                let anchor = Vec3::from_array(anchor);
                let Some(template) = templates.get(&template_id) else {
                    continue;
                };

                let mut facts = facts
                    .into_iter()
                    .map(|(name, fact)| (name, presentation_fact_from_wire(fact)))
                    .collect::<BTreeMap<_, _>>();
                // `value` is the conventional impact fact. Preserve an explicitly
                // stamped same-named fact if a future producer supplies one.
                facts
                    .entry("value".to_string())
                    .or_insert(PresentationFact::Number(value));

                registry.push_presentation_spawn(PresentationSpawn {
                    world_anchor: anchor,
                    template: PresentationTemplateHandle::from(template.id.clone()),
                    facts,
                    presenter: None,
                    lifetime_seconds: template.lifetime_ms as f32 / 1_000.0,
                    motion: PresentationMotion {
                        rise_pixels: template.motion.rise,
                        easing: template.motion.easing,
                    },
                    fade: PresentationFade {
                        duration_seconds: template
                            .lifetime_ms
                            .saturating_sub(template.fade.start_ms)
                            as f32
                            / 1_000.0,
                    },
                    scatter_radius: template.spawn_scatter.radius,
                });
            }
            ServerPresentationPayload::OverlayRoster(roster) => {
                overlay_state.receive(roster);
            }
        }
    }
    update_client_overlay_anchors(
        registry,
        descriptors,
        overlay_state,
        replication,
        pool,
        overlay_config,
        hit_zones,
        agent_params,
        anim_time,
        local_script_time,
        rtt_micros,
    );
}

/// Resolve the visual anchor for an already-replicated entity. Connected
/// clients carry no `HealthComponent`, so the local descriptor supplies the
/// same authored hitbox fallback without synthesizing combat state.
fn client_overlay_anchor(
    registry: &EntityRegistry,
    hit_zones: &HitZoneStore,
    entity: EntityId,
    anchor: &postretro_scripting_core::data_descriptors::PresentationWorldAnchor,
    hitbox: Option<ClientOverlayHitbox>,
    anim_time: f64,
) -> Option<Vec3> {
    let offset = Vec3::Y * anchor.offset_y;
    if let Some(socket) = hit_zones.posed_socket_world(registry, entity, &anchor.socket, anim_time)
    {
        return Some(socket + offset);
    }

    let transform = registry
        .get_component::<postretro_entities::Transform>(entity)
        .ok()?;
    if !transform.position.is_finite() {
        return None;
    }
    if let Some(hitbox) = hitbox
        && hitbox.half_extents.is_finite()
        && hitbox.offset.is_finite()
    {
        let top = transform.position + hitbox.offset + Vec3::Y * hitbox.half_extents.y + offset;
        if top.is_finite() {
            return Some(top);
        }
    }
    let mesh = registry
        .get_component::<postretro_entities::components::mesh::MeshComponent>(entity)
        .ok()?;
    let bound = hit_zones.get_by_name(&mesh.model)?.derived_bound?;
    let model_to_world = model_matrix(transform, mesh.origin_offset)?;
    let local_top = Vec3::new(
        (bound.min.x + bound.max.x) * 0.5,
        bound.max.y,
        (bound.min.z + bound.max.z) * 0.5,
    );
    let top = model_to_world.transform_point3(local_top) + offset;
    top.is_finite().then_some(top)
}

fn client_overlay_hitbox(
    replication: &ClientReplication,
    descriptors: &[EntityTypeDescriptor],
    network_id: NetworkId,
    agent_params: Option<NavAgentParams>,
) -> Option<ClientOverlayHitbox> {
    let entity_class = replication.remote_entity_class(network_id)?;
    let descriptor = descriptors
        .iter()
        .find(|descriptor| descriptor.canonical_name.as_deref() == Some(entity_class))?;
    let hitbox: HitboxDescriptor = descriptor.health.as_ref()?.hitbox?;
    Some(ClientOverlayHitbox {
        half_extents: Vec3::from_array(hitbox.half_extents),
        offset: Vec3::from_array(hitbox.offset.unwrap_or([0.0; 3]))
            - ai_capsule_center_from_feet_offset(descriptor, agent_params),
    })
}

fn presentation_message_from_spawn(spawn: &PresentationSpawn) -> ServerPresentationMessage {
    ServerPresentationMessage {
        payload: ServerPresentationPayload::Spawn {
            template_id: spawn.template.0.clone(),
            anchor: spawn.world_anchor.to_array(),
            value: match spawn.facts.get("value") {
                Some(PresentationFact::Number(value)) => *value,
                _ => 0.0,
            },
            facts: spawn
                .facts
                .iter()
                .map(|(name, fact)| (name.clone(), presentation_fact_to_wire(fact)))
                .collect(),
        },
    }
}

fn presentation_fact_to_wire(fact: &PresentationFact) -> WirePresentationFact {
    match fact {
        PresentationFact::Number(value) => WirePresentationFact::Number(*value),
        PresentationFact::Text(value) => WirePresentationFact::Text(value.clone()),
        PresentationFact::Bool(value) => WirePresentationFact::Bool(*value),
    }
}

fn presentation_fact_from_wire(fact: WirePresentationFact) -> PresentationFact {
    match fact {
        WirePresentationFact::Number(value) => PresentationFact::Number(value),
        WirePresentationFact::Text(value) => PresentationFact::Text(value),
        WirePresentationFact::Bool(value) => PresentationFact::Bool(value),
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, UdpSocket};
    use std::time::Duration;

    use super::*;
    use glam::Vec3;
    use log::Level;
    use postretro_entities::components::health::{HealthComponent, Hitbox};
    use postretro_entities::{PresentationPresenter, PresentationTemplateHandle, Transform};
    use postretro_foundation::{PresentationEasing, SplashDescriptor};
    use postretro_net::harness::{LinkConfig, PacketConditioner};
    use postretro_net::transport::NetClient;
    use postretro_scripting_core::data_descriptors::{
        PresentationTemplateFade, PresentationTemplateMotion, PresentationTemplateSpawnScatter,
    };
    use postretro_test_log_capture::LogCapture;

    const FLOAT_EPSILON: f32 = 1.0e-5;
    const PREDICTED_OWNER_CLIENT: u64 = 41;
    const REMOTE_OBSERVER_CLIENT: u64 = 73;
    /// The id a pre-retirement host put on the Presentation channel for splash.
    const RETIRED_SPLASH_TEMPLATE_ID: &str = "postretro.builtin.splash-impact";

    fn perfect_link(seed: u64) -> LinkConfig {
        LinkConfig {
            delay: 0,
            jitter: 0,
            loss_probability: 0.0,
            seed,
        }
    }

    fn damageable_at(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
        let entity = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        registry
            .set_component(
                entity,
                HealthComponent {
                    max: 100.0,
                    current: 100.0,
                    hitbox: Some(Hitbox {
                        half_extents: Vec3::splat(0.25),
                        offset: Vec3::ZERO,
                    }),
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .expect("test damageable accepts Health");
        entity
    }

    fn splash_wall_at_x(x: f32) -> crate::collision::CollisionWorld {
        crate::collision::CollisionWorld::from_triangles_for_test(
            vec![
                Vec3::new(x, -2.0, -2.0),
                Vec3::new(x, 2.0, -2.0),
                Vec3::new(x, 2.0, 2.0),
                Vec3::new(x, -2.0, 2.0),
            ],
            vec![[0, 1, 2], [0, 2, 3]],
        )
    }

    fn connect_presentation_client(
        server: &mut NetServer,
        server_addr: std::net::SocketAddr,
        client_id: u64,
    ) -> NetClient {
        let client_socket =
            UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind presentation relay client");
        let mut client = NetClient::new(
            client_socket,
            server_addr,
            client_id,
            Duration::from_secs(1),
            None,
            None,
        )
        .expect("construct presentation relay client");
        client.set_mod_identity("postretro.test".to_string(), "1".to_string());
        client.set_mod_digest(Some([7; 32]));
        client.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        postretro_net::harness::reveal_both(server, &mut client, "test-level");
        server.add_relay_connection(client_id, None);
        client.set_connected();

        for _ in 0..8 {
            client.update_connections(Duration::from_millis(16));
            for packet in client.packets_to_send() {
                server.process_packet_from(&packet, client_id);
            }
            let _ = server.poll_handshakes();
            if server.is_participating(client_id) {
                break;
            }
        }
        assert!(
            server.is_participating(client_id),
            "relay client completes the real participation gate"
        );

        // A server participation marker arms client-side presentation ingest.
        let mut control_link = PacketConditioner::new(perfect_link(client_id));
        control_link.enqueue_all(server.packets_to_send(client_id));
        control_link.advance(1);
        for packet in control_link.take_ready() {
            client.process_packet(&packet);
        }
        let _ = client.drain_control();
        assert!(client.is_participating());
        client
    }

    fn relay_server_presentation(
        server: &mut NetServer,
        client: &mut NetClient,
        client_id: u64,
        conditioner: &mut PacketConditioner,
    ) -> Vec<ServerPresentationMessage> {
        conditioner.enqueue_all(server.packets_to_send(client_id));
        conditioner.advance(16);
        for packet in conditioner.take_ready() {
            client.process_packet(&packet);
        }
        let _ = client.drain_control();
        client.drain_presentation()
    }

    /// Reliable observer weapon cues the client received this round trip.
    fn relay_server_weapon_cues(
        server: &mut NetServer,
        client: &mut NetClient,
        client_id: u64,
    ) -> Vec<postretro_net::wire::WeaponCue> {
        for packet in server.packets_to_send(client_id) {
            client.process_packet(&packet);
        }
        let _ = client.drain_control();
        client
            .drain_input()
            .into_iter()
            .filter_map(|bytes| match postretro_net::wire::decode(&bytes) {
                Ok(postretro_net::wire::ServerMessage::WeaponCues(message)) => Some(message.cues),
                _ => None,
            })
            .flatten()
            .collect()
    }

    fn impact_particles(
        registry: &mut EntityRegistry,
    ) -> postretro_sim::particle_pool::ParticlePool {
        let mut pool = postretro_sim::particle_pool::ParticlePool::new();
        pool.configure_impact_collection(0);
        crate::weapon::drain_impact_particle_requests(registry, &mut pool, &mut Vec::new());
        pool
    }

    fn spawn(presenter: Option<EntityId>) -> PresentationSpawn {
        PresentationSpawn {
            world_anchor: Vec3::new(1.0, 2.0, 3.0),
            template: PresentationTemplateHandle::from("damage-number"),
            facts: BTreeMap::from([
                ("value".to_string(), PresentationFact::Number(40.0)),
                ("critical".to_string(), PresentationFact::Bool(true)),
                (
                    "label".to_string(),
                    PresentationFact::Text("critical hit".to_string()),
                ),
            ]),
            presenter: presenter.map(|id| PresentationPresenter(id.to_raw())),
            lifetime_seconds: 0.9,
            motion: PresentationMotion {
                rise_pixels: 12.0,
                easing: PresentationEasing::EaseOut,
            },
            fade: PresentationFade {
                duration_seconds: 0.4,
            },
            scatter_radius: 0.15,
        }
    }

    fn template() -> PresentationTemplate {
        PresentationTemplate {
            id: "damage-number".to_string(),
            root: postretro_scripting_core::ui::descriptor::Widget::Spacer(
                postretro_scripting_core::ui::descriptor::SpacerWidget {
                    flex_grow: 0.0,
                    id: None,
                    visible_when: None,
                    role: None,
                },
            ),
            world_anchor: None,
            lifetime_ms: 900,
            motion: PresentationTemplateMotion {
                rise: 12.0,
                easing: PresentationEasing::EaseOut,
            },
            fade: PresentationTemplateFade { start_ms: 500 },
            spawn_scatter: PresentationTemplateSpawnScatter { radius: 0.15 },
        }
    }

    #[test]
    fn shared_damage_numbers_reach_every_participant_once_and_keep_one_host_copy() {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = socket.local_addr().unwrap();
        let mut server = NetServer::new(socket, address, 8, Duration::from_secs(1), None).unwrap();
        server.set_mod_identity("postretro.test".to_string(), "1".to_string());
        server.set_mod_digest(Some([7; 32]));
        server.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        let mut attacker =
            connect_presentation_client(&mut server, address, PREDICTED_OWNER_CLIENT);
        let mut observer =
            connect_presentation_client(&mut server, address, REMOTE_OBSERVER_CLIENT);
        let mut registry = EntityRegistry::new();
        let host_player = registry.spawn(Transform::default());
        let remote_player = registry.spawn(Transform::default());
        let enemy = registry.spawn(Transform::default());
        let mut originals = Vec::new();
        for (index, source) in [Some(host_player), Some(remote_player), Some(enemy), None]
            .into_iter()
            .enumerate()
        {
            let mut number = spawn(source);
            number.facts.insert(
                "value".into(),
                PresentationFact::Number(index as f32 + 10.0),
            );
            originals.push(number.clone());
            registry.push_presentation_spawn(number);
        }
        route_host_presentation_spawns(&mut registry, &mut server);
        // No renderer drain: re-routing must not repeat old numbers, but must
        // still pick up a subsequent hit in the retained bounded queue.
        route_host_presentation_spawns(&mut registry, &mut server);
        let next = spawn(Some(enemy));
        registry.push_presentation_spawn(next.clone());
        originals.push(next);
        route_host_presentation_spawns(&mut registry, &mut server);
        let expected: Vec<_> = originals
            .iter()
            .map(presentation_message_from_spawn)
            .collect();
        for (id, client) in [
            (PREDICTED_OWNER_CLIENT, &mut attacker),
            (REMOTE_OBSERVER_CLIENT, &mut observer),
        ] {
            let received = relay_server_presentation(
                &mut server,
                client,
                id,
                &mut PacketConditioner::new(perfect_link(id)),
            );
            assert_eq!(
                received, expected,
                "every viewer receives the original frozen facts once"
            );
        }
        assert_eq!(
            registry.take_presentation_spawns(),
            originals,
            "the host retains exactly one local copy of every hit"
        );

        let next_frame = spawn(None);
        registry.push_presentation_spawn(next_frame.clone());
        route_host_presentation_spawns(&mut registry, &mut server);
        assert_eq!(
            relay_server_presentation(
                &mut server,
                &mut attacker,
                PREDICTED_OWNER_CLIENT,
                &mut PacketConditioner::new(perfect_link(4))
            ),
            vec![presentation_message_from_spawn(&next_frame)],
            "local intake resets routing for the next frame"
        );
    }

    #[test]
    fn shared_damage_numbers_skip_held_clients_and_never_replay_on_promotion_or_join() {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = socket.local_addr().unwrap();
        let mut server = NetServer::new(socket, address, 8, Duration::from_secs(1), None).unwrap();
        server.set_mod_identity("postretro.test".to_string(), "1".to_string());
        server.set_mod_digest(Some([7; 32]));
        server.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        let mut client = connect_presentation_client(&mut server, address, PREDICTED_OWNER_CLIENT);
        server.set_mod_digest(None);
        assert!(!server.is_participating(PREDICTED_OWNER_CLIENT));
        let mut registry = EntityRegistry::new();
        registry.push_presentation_spawn(spawn(None));
        route_host_presentation_spawns(&mut registry, &mut server);
        assert!(
            relay_server_presentation(
                &mut server,
                &mut client,
                PREDICTED_OWNER_CLIENT,
                &mut PacketConditioner::new(perfect_link(5))
            )
            .is_empty()
        );

        server.set_mod_digest(Some([7; 32]));
        assert!(server.is_participating(PREDICTED_OWNER_CLIENT));
        let mut late_joiner =
            connect_presentation_client(&mut server, address, REMOTE_OBSERVER_CLIENT);
        // The host still has the old number pending local intake; neither a
        // re-promoted client nor a late joiner should receive that old event.
        route_host_presentation_spawns(&mut registry, &mut server);
        for (id, peer) in [
            (PREDICTED_OWNER_CLIENT, &mut client),
            (REMOTE_OBSERVER_CLIENT, &mut late_joiner),
        ] {
            assert!(
                relay_server_presentation(
                    &mut server,
                    peer,
                    id,
                    &mut PacketConditioner::new(perfect_link(id))
                )
                .is_empty()
            );
        }
        let fresh = spawn(None);
        registry.push_presentation_spawn(fresh.clone());
        route_host_presentation_spawns(&mut registry, &mut server);
        for (id, peer) in [
            (PREDICTED_OWNER_CLIENT, &mut client),
            (REMOTE_OBSERVER_CLIENT, &mut late_joiner),
        ] {
            assert_eq!(
                relay_server_presentation(
                    &mut server,
                    peer,
                    id,
                    &mut PacketConditioner::new(perfect_link(id))
                ),
                vec![presentation_message_from_spawn(&fresh)]
            );
        }
        assert_eq!(registry.take_presentation_spawns().len(), 2);
    }

    #[test]
    fn spawn_message_preserves_template_anchor_value_and_all_facts() {
        let original = spawn(Some(EntityId::from_raw(7)));
        let message = presentation_message_from_spawn(&original);
        let ServerPresentationPayload::Spawn {
            template_id,
            anchor,
            value,
            facts,
        } = message.payload
        else {
            panic!("spawn conversion must produce a Spawn payload");
        };

        assert_eq!(template_id, original.template.0);
        assert_eq!(anchor, original.world_anchor.to_array());
        assert_eq!(value, 40.0);
        assert_eq!(
            facts,
            BTreeMap::from([
                ("value".to_string(), WirePresentationFact::Number(40.0)),
                ("critical".to_string(), WirePresentationFact::Bool(true)),
                (
                    "label".to_string(),
                    WirePresentationFact::Text("critical hit".to_string()),
                ),
            ])
        );
    }

    #[test]
    fn client_spawn_ingest_restores_local_pool_values_without_presenter_identity() {
        let mut registry = EntityRegistry::new();
        let mut overlay_state = ClientOverlayFactState::default();
        let replication = ClientReplication::new();
        let mut pool = PresentationPool::new(1);
        let hit_zones = HitZoneStore::new();
        let templates = HashMap::from([("damage-number".to_string(), template())]);
        ingest_client_presentation_messages(
            &mut registry,
            vec![presentation_message_from_spawn(&spawn(Some(
                EntityId::from_raw(7),
            )))],
            &[],
            &templates,
            &mut overlay_state,
            &replication,
            &mut pool,
            None,
            &hit_zones,
            None,
            0.0,
            0.0,
            None,
        );

        let spawned = registry.take_presentation_spawns();
        assert_eq!(spawned.len(), 1);
        let spawned = &spawned[0];
        assert_eq!(spawned.world_anchor, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(spawned.template.0, "damage-number");
        assert_eq!(
            spawned.facts.get("value"),
            Some(&PresentationFact::Number(40.0))
        );
        assert_eq!(
            spawned.facts.get("critical"),
            Some(&PresentationFact::Bool(true))
        );
        assert_eq!(
            spawned.facts.get("label"),
            Some(&PresentationFact::Text("critical hit".to_string()))
        );
        assert_eq!(spawned.presenter, None);
        assert_eq!(spawned.lifetime_seconds, 0.9);
        assert_eq!(spawned.motion.rise_pixels, 12.0);
        assert_eq!(spawned.fade.duration_seconds, 0.4);
        assert_eq!(spawned.scatter_radius, 0.15);
    }

    // Regression: remote splash lost frozen authority state and omitted the
    // listen host's local impact burst.
    #[test]
    fn remote_splash_contact_resolves_damage_host_burst_and_one_observer_burst() {
        let server_socket =
            UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind splash authority server");
        let server_addr = server_socket
            .local_addr()
            .expect("read splash authority server address");
        let mut server =
            NetServer::new(server_socket, server_addr, 8, Duration::from_secs(1), None)
                .expect("construct splash authority server");
        server.set_mod_identity("postretro.test".to_string(), "1".to_string());
        server.set_mod_digest(Some([7; 32]));
        server.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        let mut predicted_owner =
            connect_presentation_client(&mut server, server_addr, PREDICTED_OWNER_CLIENT);
        let mut observer =
            connect_presentation_client(&mut server, server_addr, REMOTE_OBSERVER_CLIENT);

        let mut registry = EntityRegistry::new();
        let owner = damageable_at(&mut registry, Vec3::new(-0.25, 0.0, 0.0));
        let clear_victim = damageable_at(&mut registry, Vec3::new(1.0, 0.0, 2.25));
        let wall_hidden = damageable_at(&mut registry, Vec3::new(2.25, 0.0, 0.0));
        let weapon = registry.spawn(Transform::default());
        let mut owners = MovementOwners::new();
        owners.set(owner, PREDICTED_OWNER_CLIENT);
        let mut allocator = NetworkIdAllocator::new();
        let owner_network_id = allocator.stamp(owner);
        let shot_id = ShotId::from_parts(
            (owner_network_id).0,
            17,
            postretro_foundation::ActivationLane::Primary,
            0,
        );
        let mut open_shots = crate::netcode::OpenAuthorizedShots::new();
        open_shots.record(
            AuthorizedShot {
                sounds: None,
                action: None,
                source_weapon: None,
                knockback: None,
                shot_id,
                pawn: owner,
                weapon,
                fire_tick: 9,
                damage: 100.0,
                range: 20.0,
                pellet_count: 1,
                credit_source: "weapon.test.remote-splash".to_string(),
                splash: Some(SplashDescriptor {
                    knockback: None,
                    radius: 4.0,
                    min_fraction: 0.2,
                    self_damage: true,
                }),
                projectile_radius: Some(0.0),
                projectile_direction: Some(Vec3::X),
                projectile_speed: Some(60.0),
                projectile_lifetime_seconds: Some(10.0),
                projectile_tick_seconds: Some(1.0 / 60.0),
                projectile_static_contact_distance: None,
                is_projectile: true,
                fire_origin: Vec3::ZERO,
                timeout_budget_ticks: MAX_OPEN_SHOT_AGE_TICKS,
            },
            PREDICTED_OWNER_CLIENT,
        );

        let declaration = postretro_net::wire::ClientMessage::HitDeclaration(
            postretro_net::wire::HitDeclaration {
                shot_id: crate::wire_convert::shot_id_to_wire(shot_id),
                records: vec![postretro_net::wire::HitRecord {
                    normal: [0.0, 1.0, 0.0],
                    target: crate::netcode::PRESENTATION_CONTACT_TARGET,
                    point: Vec3::new(4.0, 0.0, 0.0).to_array(),
                    zone: None,
                }],
            },
        );
        let mut declaration_link = PacketConditioner::new(perfect_link(0xe16_0303));
        declaration_link.enqueue(postretro_net::wire::encode(&declaration));
        declaration_link.advance(16);
        let declaration_packets = declaration_link.take_ready();
        let [packet] = declaration_packets.as_slice() else {
            panic!("conditioned client declaration reaches the host once");
        };
        let postretro_net::wire::ClientMessage::HitDeclaration(delivered) =
            postretro_net::wire::decode(packet).expect("decode conditioned declaration")
        else {
            panic!("conditioned packet retains HitDeclaration");
        };

        let collision_world = splash_wall_at_x(1.0);
        let hit_zones = HitZoneStore::new();
        let mut health_at_impact_drain = None;
        let mut pending = crate::netcode::PendingHitDeclarations::new();
        assert!(pending.push_at(PREDICTED_OWNER_CLIENT, delivered, 10));
        // A splash declaration waits until host travel from FIRE (tick 9, 1 m a
        // tick) covers its declared point 4 m out.
        let intake_tick = 15;
        let ready = crate::netcode::host_take_ready_hit_declarations(
            &crate::netcode::HostCommandQueues::new(),
            &mut open_shots,
            &mut pending,
            intake_tick,
        );
        let mut projectile_contact = None;
        let mut remote_impacts = Vec::new();
        assert!(crate::netcode::host_ingest_ready_hit_declarations(
            &mut server,
            &mut registry,
            &collision_world,
            &hit_zones,
            &allocator,
            &owners,
            &mut open_shots,
            intake_tick,
            0.0,
            ready,
            |registry| {
                health_at_impact_drain = Some((
                    registry
                        .get_component::<HealthComponent>(owner)
                        .expect("owner remains live during impact drain")
                        .current,
                    registry
                        .get_component::<HealthComponent>(clear_victim)
                        .expect("victim remains live during impact drain")
                        .current,
                ));
            },
            |_, point| projectile_contact = Some(point),
            |impact| remote_impacts.push(impact),
        ));
        assert_eq!(projectile_contact, Some(Vec3::X));
        assert_eq!(remote_impacts.len(), 1, "one impact per remote shot");
        let (owner_at_drain, victim_at_drain) =
            health_at_impact_drain.expect("one post-blast impact drain observes settled Health");
        assert!((owner_at_drain - 20.0).abs() <= FLOAT_EPSILON);
        assert!((victim_at_drain - 40.0).abs() <= FLOAT_EPSILON);
        assert_eq!(
            registry
                .get_component::<HealthComponent>(wall_hidden)
                .expect("occluded target remains live")
                .current,
            100.0,
            "static world geometry occludes remote splash"
        );
        let victim_health = registry
            .get_component::<HealthComponent>(clear_victim)
            .expect("clear victim remains live");
        assert!(
            (victim_health.current - 40.0).abs() <= FLOAT_EPSILON,
            "a static-world contact damages the nearby victim through splash"
        );
        let [victim_credit] = victim_health.contributor_ledger.entries() else {
            panic!("victim records exactly one splash contribution");
        };
        assert_eq!(victim_credit.hit_count, 1, "splash reaches the victim once");
        assert_eq!(victim_credit.last_attacker, Some(owner));
        assert_eq!(victim_credit.last_weapon, Some(weapon));
        assert_eq!(victim_credit.source_id, "weapon.test.remote-splash");
        route_host_presentation_spawns(&mut registry, &mut server);
        let host_particles = impact_particles(&mut registry);
        assert_eq!(
            host_particles.len(),
            crate::weapon::IMPACT_PARTICLE_COUNT,
            "the listen host materializes exactly one local impact burst for the remote detonation",
        );
        assert!(
            host_particles
                .iter()
                .all(|particle| particle.velocity.dot(Vec3::NEG_X) > 0.0)
        );

        // The firing owner predicted its own explosion and is excluded from the cue.
        assert!(
            relay_server_weapon_cues(&mut server, &mut predicted_owner, PREDICTED_OWNER_CLIENT)
                .is_empty(),
            "the predicted owner receives neither a cue nor a second burst"
        );
        // An observer takes its one burst from the reliable impact cue alone.
        let cues = relay_server_weapon_cues(&mut server, &mut observer, REMOTE_OBSERVER_CLIENT);
        let [cue] = cues.as_slice() else {
            panic!("the observer receives exactly one splash impact cue");
        };
        assert_eq!(cue.kind, crate::netcode::weapon_cues::WeaponCueKind::Impact);
        let mut observer_registry = EntityRegistry::new();
        let delivery = crate::netcode::weapon_cues::materialize_observer_weapon_cue(
            cue.clone(),
            &ClientReplication::new(),
            &observer_registry,
        )
        .expect("a valid cue materializes");
        crate::netcode::weapon_cues::spawn_observer_impact_bursts(
            &mut observer_registry,
            &[delivery],
        );
        let observer_particles = impact_particles(&mut observer_registry);
        assert_eq!(
            observer_particles.len(),
            crate::weapon::IMPACT_PARTICLE_COUNT,
            "an observer of a splash detonation bursts exactly once"
        );
        assert!(
            observer_particles
                .iter()
                .all(|particle| particle.velocity.dot(Vec3::NEG_X) > 0.0)
        );
        let mut observer_link = PacketConditioner::new(perfect_link(0xe16_0305));
        assert!(
            relay_server_presentation(
                &mut server,
                &mut observer,
                REMOTE_OBSERVER_CLIENT,
                &mut observer_link,
            )
            .is_empty(),
            "the retired presentation-channel splash route sends nothing"
        );
    }

    // A pre-retirement host named the built-in splash id on the Presentation
    // channel. The wire type still decodes it, but no client route spawns from it.
    #[test]
    fn client_ignores_the_retired_builtin_splash_presentation_spawn() {
        let mut registry = EntityRegistry::new();
        let mut overlay_state = ClientOverlayFactState::default();
        let replication = ClientReplication::new();
        let mut pool = PresentationPool::new(1);
        let hit_zones = HitZoneStore::new();

        ingest_client_presentation_messages(
            &mut registry,
            vec![ServerPresentationMessage {
                payload: ServerPresentationPayload::Spawn {
                    template_id: RETIRED_SPLASH_TEMPLATE_ID.to_string(),
                    anchor: [1.0, 2.0, 3.0],
                    value: 0.0,
                    facts: BTreeMap::new(),
                },
            }],
            &[],
            &HashMap::new(),
            &mut overlay_state,
            &replication,
            &mut pool,
            None,
            &hit_zones,
            None,
            0.0,
            0.0,
            None,
        );

        assert_eq!(
            impact_particles(&mut registry).len(),
            0,
            "the impact cue is the only observer burst route"
        );
        assert!(registry.take_presentation_spawns().is_empty());
    }

    // Regression: a non-finite spawn anchor reached particle transforms and
    // poisoned later simulation and render math. The guard runs before any
    // template or built-in id is consulted.
    #[test]
    fn client_presentation_spawn_rejects_non_finite_anchor_before_spawning() {
        let mut registry = EntityRegistry::new();
        let mut overlay_state = ClientOverlayFactState::default();
        let replication = ClientReplication::new();
        let mut pool = PresentationPool::new(1);
        let hit_zones = HitZoneStore::new();
        let message = ServerPresentationMessage {
            payload: ServerPresentationPayload::Spawn {
                template_id: RETIRED_SPLASH_TEMPLATE_ID.to_string(),
                anchor: [f32::NAN, 2.0, 3.0],
                value: 0.0,
                facts: BTreeMap::new(),
            },
        };

        let capture = LogCapture::start();
        ingest_client_presentation_messages(
            &mut registry,
            vec![message],
            &[],
            &HashMap::new(),
            &mut overlay_state,
            &replication,
            &mut pool,
            None,
            &hit_zones,
            None,
            0.0,
            0.0,
            None,
        );

        capture.assert_logged_once(Level::Warn, "with a non-finite world anchor");
        assert_eq!(impact_particles(&mut registry).len(), 0);
        assert!(registry.take_presentation_spawns().is_empty());
    }
}
