// Frozen reliable observer weapon cues and impact-burst spawning from them; assets and sound playback remain local.
// See: context/lib/networking.md · context/lib/audio.md · context/lib/scripting.md §12

use glam::Vec3;
use postretro_entities::reactions::emitter::{ContactHit, Emitter, ImpactContact};
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::{ShotId, WeaponActivationDescriptor};
use postretro_net::wire::{self, NetworkId, WeaponCue, WeaponCueAnchor, WeaponCueContact};

use super::{ClientReplication, NetEndpoint, NetServer, NetworkIdAllocator};
pub use postretro_net::wire::WeaponCueKind;

/// Normalize an explicitly identified producer pawn once at publication. Callers
/// retain the returned id with delayed contacts instead of reinterpreting raw ids.
pub fn observer_shot_id(
    allocator: &mut NetworkIdAllocator,
    owner_pawn: EntityId,
    mut shot_id: ShotId,
) -> ShotId {
    shot_id.pawn = allocator.stamp(owner_pawn).0;
    shot_id
}

/// Freeze the exact keys/alias and captured anchor while the originating action
/// is available. `shot_pawn` is the producer's retained network identity, including
/// on impacts whose firing entity has already despawned. Owner routing is separate.
#[allow(clippy::too_many_arguments)]
pub fn freeze_observer_weapon_cue(
    shot_id: ShotId,
    shot_pawn: NetworkId,
    kind: WeaponCueKind,
    action: Option<&WeaponActivationDescriptor>,
    fallback_sound: Option<&str>,
    additional_sound: Option<&str>,
    emitter: &Emitter,
    allocator: &NetworkIdAllocator,
) -> Option<WeaponCue> {
    let action_sound =
        action
            .and_then(|action| action.sounds.as_ref())
            .and_then(|sounds| match kind {
                WeaponCueKind::Activate => sounds.fire.as_deref(),
                WeaponCueKind::Impact => sounds.impact.as_deref(),
            });
    let alias = action
        .and_then(|action| action.emits.as_ref())
        .and_then(|emits| match kind {
            WeaponCueKind::Activate => emits.activate.as_deref(),
            WeaponCueKind::Impact => emits.impact.as_deref(),
        });
    let anchor = match emitter {
        Emitter::Entity { id, origin } => match allocator.network_id_for_entity(*id) {
            Some(entity) => WeaponCueAnchor::Entity {
                entity,
                origin: origin.to_array(),
            },
            None => WeaponCueAnchor::Contacts(vec![WeaponCueContact {
                point: origin.to_array(),
                normal: [0.0; 3],
                target: None,
            }]),
        },
        Emitter::Contacts(contacts) => {
            if contacts.is_empty() || contacts.len() > wire::MAX_WEAPON_CUE_CONTACTS {
                return None;
            }
            WeaponCueAnchor::Contacts(
                contacts
                    .iter()
                    .map(|contact| WeaponCueContact {
                        point: contact.point.to_array(),
                        normal: contact.normal.to_array(),
                        target: match contact.hit {
                            ContactHit::Entity(id) => allocator.network_id_for_entity(id),
                            ContactHit::World => None,
                        },
                    })
                    .collect(),
            )
        }
    };
    let mut wire_id = super::wire_convert::shot_id_to_wire(shot_id);
    wire_id.pawn = shot_pawn.0;
    let cue = WeaponCue {
        shot_id: wire_id,
        kind,
        sound: action_sound.or(fallback_sound).map(str::to_owned),
        additional_sound: additional_sound.map(str::to_owned),
        alias: alias.map(str::to_owned),
        anchor,
    };
    cue.is_valid().then_some(cue)
}

/// Send immediately for every accepted shot/contact, independently of snapshot
/// cadence. Reliable transport is the only retained outbound cue queue.
pub fn send_observer_weapon_cue(
    endpoint: &mut NetEndpoint,
    firing_owner_client_id: Option<u64>,
    cue: WeaponCue,
) -> usize {
    let NetEndpoint::Host { server, .. } = endpoint else {
        return 0;
    };
    send_observer_weapon_cue_to_server(server, firing_owner_client_id, cue)
}

/// Server-borrow variant for host HIT ingestion and fixed-tick binary glue.
pub fn send_observer_weapon_cue_to_server(
    server: &mut NetServer,
    firing_owner_client_id: Option<u64>,
    cue: WeaponCue,
) -> usize {
    if !cue.is_valid() {
        return 0;
    }
    let mut recipients = 0;
    for client_id in server.participating_clients() {
        if Some(client_id) == firing_owner_client_id {
            continue;
        }
        recipients += usize::from(server.send_weapon_cues(client_id, vec![cue.clone()]));
    }
    recipients
}

/// Ready for the binary's local sound/alias delivery, without a descriptor lookup.
pub struct ObserverWeaponCueDelivery {
    pub shot_id: ShotId,
    pub kind: WeaponCueKind,
    pub sound: Option<String>,
    pub additional_sound: Option<String>,
    pub alias: Option<String>,
    pub emitter: Emitter,
}

/// Missing entities keep the producer's captured world position. Contacts keep
/// their original positions/normals even when their optional target retired.
pub fn materialize_observer_weapon_cue(
    cue: WeaponCue,
    replication: &ClientReplication,
    registry: &EntityRegistry,
) -> Option<ObserverWeaponCueDelivery> {
    if !cue.is_valid() {
        return None;
    }
    let emitter = match cue.anchor {
        WeaponCueAnchor::Entity { entity, origin } => {
            let origin = Vec3::from_array(origin);
            match replication
                .entity_for_network_id(entity)
                .filter(|id| registry.exists(*id))
            {
                Some(id) => Emitter::Entity { id, origin },
                None => Emitter::Contacts(vec![ImpactContact::new(origin, Vec3::ZERO, None)]),
            }
        }
        WeaponCueAnchor::Contacts(contacts) => Emitter::Contacts(
            contacts
                .into_iter()
                .map(|contact| {
                    ImpactContact::new(
                        Vec3::from_array(contact.point),
                        Vec3::from_array(contact.normal),
                        contact
                            .target
                            .and_then(|target| replication.entity_for_network_id(target))
                            .filter(|id| registry.exists(*id)),
                    )
                })
                .collect(),
        ),
    };
    Some(ObserverWeaponCueDelivery {
        shot_id: super::wire_convert::shot_id_from_wire(cue.shot_id),
        kind: cue.kind,
        sound: cue.sound,
        additional_sound: cue.additional_sound,
        alias: cue.alias,
        emitter,
    })
}

/// Spawn the built-in impact burst for each observed impact cue's contacts.
/// This is the only route by which a peer that did not simulate the shot sees
/// its burst: the firing peer's own simulation spawns it, and the firing owner
/// is excluded from cue delivery, so nothing spawns twice. Only an impact cue
/// anchored at contacts has a surface to burst from; activation cues spawn none.
///
/// Invariant: every `impact` producer emits contact anchors. A delivery cannot
/// tell a contact anchor from an unresolved entity anchor, which
/// `materialize_observer_weapon_cue` turns into one zero-normal contact at the
/// producer's origin. An entity-anchored impact cue would burst there. Producers
/// guarantee it never occurs; this function does not enforce it.
pub fn spawn_observer_impact_bursts(
    registry: &mut EntityRegistry,
    cues: &[ObserverWeaponCueDelivery],
) {
    for cue in cues {
        if cue.kind != WeaponCueKind::Impact {
            continue;
        }
        if let Emitter::Contacts(contacts) = &cue.emitter {
            crate::weapon::spawn_impact_effects_for_contacts(registry, contacts);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_foundation::{
        ActivationEmits, ActivationLane, ActivationSounds, ActivationTrigger,
    };

    fn connect_observer(
        server: &mut NetServer,
        addr: std::net::SocketAddr,
        client_id: u64,
    ) -> super::super::NetClient {
        use std::{
            net::{Ipv4Addr, UdpSocket},
            time::Duration,
        };
        let mut client = super::super::NetClient::new(
            UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap(),
            addr,
            client_id,
            Duration::from_secs(1),
            None,
            None,
        )
        .unwrap();
        client.set_mod_identity("test".into(), "1".into());
        client.set_mod_digest(Some([7; 32]));
        client.set_level_parity(Some(("map".into(), [9; 32])));
        server.add_relay_connection(client_id, None);
        client.set_connected();
        client.update_connections(Duration::from_millis(16));
        for packet in client.packets_to_send() {
            server.process_packet_from(&packet, client_id);
        }
        let _ = server.poll_handshakes();
        assert!(server.is_participating(client_id));
        for packet in server.packets_to_send(client_id) {
            client.process_packet(&packet);
        }
        let _ = client.drain_control();
        client
    }

    #[test]
    fn weapon_cues_authored_and_wire_name_bounds_match() {
        assert_eq!(
            postretro_foundation::MAX_DESCRIPTOR_CUE_NAME_BYTES,
            wire::MAX_WEAPON_CUE_NAME_BYTES
        );
        for length in [256, 257] {
            let key = "a".repeat(length);
            assert_eq!(
                postretro_foundation::validate_sound_key("weapon.sounds.fire", &key).is_ok(),
                length == 256
            );
            assert_eq!(
                postretro_foundation::validate_sound_key("behavior.attacks.shoot.sound", &key)
                    .is_ok(),
                length == 256
            );
            let mut action = WeaponActivationDescriptor::single(
                postretro_foundation::ActivationTrigger::Press,
                0.0,
            );
            action.emits = Some(postretro_foundation::ActivationEmits {
                activate: Some(key.clone()),
                impact: None,
            });
            assert_eq!(action.validate("primary").is_ok(), length == 256);
            let allocator = NetworkIdAllocator::new();
            let cue = freeze_observer_weapon_cue(
                ShotId {
                    pawn: 1,
                    start_tick: 1,
                    lane: postretro_foundation::ActivationLane::Primary,
                    ordinal: 0,
                },
                NetworkId(1),
                WeaponCueKind::Activate,
                Some(&action),
                Some(&key),
                Some(&key),
                &Emitter::Contacts(vec![ImpactContact::new(Vec3::ZERO, Vec3::Z, None)]),
                &allocator,
            );
            assert_eq!(cue.is_some(), length == 256);
        }
    }
    #[test]
    fn weapon_cues_route_local_ai_and_remote_primary_secondary_without_owner_duplicate() {
        use std::{
            net::{Ipv4Addr, UdpSocket},
            time::Duration,
        };
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = socket.local_addr().unwrap();
        let mut server = NetServer::new(socket, addr, 8, Duration::from_secs(1), None).unwrap();
        server.set_mod_identity("test".into(), "1".into());
        server.set_mod_digest(Some([7; 32]));
        server.set_level_parity(Some(("map".into(), [9; 32])));
        let mut owner = connect_observer(&mut server, addr, 41);
        let mut observer = connect_observer(&mut server, addr, 42);
        for lane in [ActivationLane::Primary, ActivationLane::Secondary] {
            for kind in [WeaponCueKind::Activate, WeaponCueKind::Impact] {
                for (pawn, firing_owner) in [(7, Some(41)), (8, None), (9, None)] {
                    let cue = WeaponCue {
                        shot_id: wire::WireShotId {
                            pawn,
                            start_tick: 20,
                            lane: lane as u8,
                            ordinal: 0,
                        },
                        kind,
                        sound: Some("frozen_sound".into()),
                        additional_sound: (pawn == 9).then(|| "ai_attack".into()),
                        alias: Some("frozen_alias".into()),
                        anchor: WeaponCueAnchor::Entity {
                            entity: NetworkId(pawn),
                            origin: [1.0, 2.0, 3.0],
                        },
                    };
                    assert_eq!(
                        send_observer_weapon_cue_to_server(&mut server, firing_owner, cue.clone()),
                        if firing_owner.is_some() { 1 } else { 2 }
                    );
                    for (client_id, client) in [(41, &mut owner), (42, &mut observer)] {
                        for packet in server.packets_to_send(client_id) {
                            client.process_packet(&packet);
                        }
                        let messages = client.drain_input();
                        if Some(client_id) == firing_owner {
                            assert!(messages.is_empty());
                        } else {
                            assert_eq!(messages.len(), 1);
                            let wire::ServerMessage::WeaponCues(message) =
                                wire::decode(&messages[0]).unwrap()
                            else {
                                panic!("observer cue");
                            };
                            assert_eq!(message.cues, vec![cue.clone()]);
                        }
                        client.update_connections(Duration::from_millis(16));
                        for packet in client.packets_to_send() {
                            server.process_packet_from(&packet, client_id);
                        }
                    }
                    server.update_connections(Duration::from_millis(16));
                }
            }
        }
        assert_eq!(
            wire::MAX_WEAPON_CUE_CONTACTS as u32,
            postretro_foundation::MAX_PELLET_COUNT
        );
    }

    #[test]
    fn weapon_cues_freeze_originating_secondary_sound_and_alias_after_despawn() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Default::default());
        let mut allocator = NetworkIdAllocator::new();
        let mut action = WeaponActivationDescriptor::single(ActivationTrigger::Press, 300.0);
        action.sounds = Some(ActivationSounds {
            fire: Some("old_fire".into()),
            impact: Some("old_impact".into()),
        });
        action.emits = Some(ActivationEmits {
            activate: Some("alt_fire".into()),
            impact: Some("alt_impact".into()),
        });
        let id = observer_shot_id(
            &mut allocator,
            pawn,
            ShotId::from_parts(pawn.to_raw(), 20, ActivationLane::Secondary, 2),
        );
        let cue = freeze_observer_weapon_cue(
            id,
            NetworkId(id.pawn),
            WeaponCueKind::Impact,
            Some(&action),
            Some("common"),
            None,
            &Emitter::Contacts(vec![ImpactContact::new(
                Vec3::new(2.0, 3.0, 4.0),
                Vec3::Y,
                Some(pawn),
            )]),
            &allocator,
        )
        .unwrap();
        action.sounds.as_mut().unwrap().impact = Some("new_impact".into());
        registry.despawn(pawn).unwrap();
        allocator.forget(pawn);
        let delivery =
            materialize_observer_weapon_cue(cue, &ClientReplication::new(), &registry).unwrap();
        assert_eq!(delivery.sound.as_deref(), Some("old_impact"));
        assert_eq!(delivery.alias.as_deref(), Some("alt_impact"));
        assert_eq!(delivery.shot_id, id);
        let Emitter::Contacts(contacts) = delivery.emitter else {
            panic!("contact emitter");
        };
        assert!(contacts[0].point.distance(Vec3::new(2.0, 3.0, 4.0)) < 1.0e-6);
        assert_eq!(contacts[0].hit, ContactHit::World);
    }

    #[test]
    fn weapon_cues_reject_empty_alias_and_oversized_contact_aggregation() {
        let allocator = NetworkIdAllocator::new();
        let id = ShotId::from_parts(7, 20, ActivationLane::Primary, 0);
        let mut action = WeaponActivationDescriptor::single(ActivationTrigger::Press, 100.0);
        action.emits = Some(ActivationEmits {
            activate: Some("activate".into()),
            impact: None,
        });
        assert!(
            freeze_observer_weapon_cue(
                id,
                NetworkId(7),
                WeaponCueKind::Activate,
                Some(&action),
                None,
                None,
                &Emitter::Contacts(vec![ImpactContact::new(Vec3::ZERO, Vec3::Y, None)]),
                &allocator
            )
            .is_none()
        );
        action.emits.as_mut().unwrap().activate = Some("  ".into());
        assert!(
            freeze_observer_weapon_cue(
                id,
                NetworkId(7),
                WeaponCueKind::Activate,
                Some(&action),
                None,
                None,
                &Emitter::Contacts(vec![ImpactContact::new(Vec3::ZERO, Vec3::Y, None)]),
                &allocator
            )
            .is_none()
        );
        assert!(
            freeze_observer_weapon_cue(
                id,
                NetworkId(7),
                WeaponCueKind::Impact,
                None,
                None,
                None,
                &Emitter::Contacts(vec![
                    ImpactContact::new(Vec3::ZERO, Vec3::Y, None);
                    wire::MAX_WEAPON_CUE_CONTACTS + 1
                ]),
                &allocator
            )
            .is_none()
        );
    }

    fn burst_particles(registry: &EntityRegistry) -> usize {
        registry
            .iter_with_kind(postretro_entities::ComponentKind::ParticleState)
            .count()
    }

    /// Freeze `emitter` as a cue of `kind`, then materialize it as a client would.
    fn delivered(kind: WeaponCueKind, emitter: &Emitter) -> ObserverWeaponCueDelivery {
        let allocator = NetworkIdAllocator::new();
        let id = ShotId::from_parts(7, 20, ActivationLane::Primary, 0);
        let cue = freeze_observer_weapon_cue(
            id,
            NetworkId(7),
            kind,
            None,
            Some("sound"),
            None,
            emitter,
            &allocator,
        )
        .expect("valid cue");
        materialize_observer_weapon_cue(cue, &ClientReplication::new(), &EntityRegistry::new())
            .expect("valid cue materializes")
    }

    #[test]
    fn observer_impact_cue_bursts_once_per_contact() {
        let contacts = vec![
            ImpactContact::new(Vec3::new(1.0, 0.0, 0.0), Vec3::X, None),
            ImpactContact::new(Vec3::new(2.0, 0.0, 0.0), Vec3::Y, None),
            ImpactContact::new(Vec3::new(3.0, 0.0, 0.0), Vec3::ZERO, None),
        ];
        let cue = delivered(WeaponCueKind::Impact, &Emitter::Contacts(contacts));
        let mut registry = EntityRegistry::new();

        spawn_observer_impact_bursts(&mut registry, &[cue]);

        assert_eq!(
            burst_particles(&registry),
            3 * crate::weapon::IMPACT_PARTICLE_COUNT,
            "a zero normal still bursts, along the burst's own fallback"
        );
    }

    #[test]
    fn observer_cues_burst_once_per_cue_and_only_for_impact_contacts() {
        let one_contact = Emitter::Contacts(vec![ImpactContact::new(Vec3::ZERO, Vec3::Y, None)]);
        let impact = delivered(WeaponCueKind::Impact, &one_contact);
        let activate = delivered(WeaponCueKind::Activate, &one_contact);
        // A muzzle cue anchored on a missing entity materializes at its captured
        // origin, but it carries no surface and bursts nothing.
        let entity_anchored = delivered(
            WeaponCueKind::Activate,
            &Emitter::Entity {
                id: EntityId::from_raw(3),
                origin: Vec3::new(1.0, 2.0, 3.0),
            },
        );
        let mut registry = EntityRegistry::new();

        spawn_observer_impact_bursts(&mut registry, &[activate, entity_anchored]);
        assert_eq!(burst_particles(&registry), 0);

        spawn_observer_impact_bursts(&mut registry, &[impact]);
        assert_eq!(
            burst_particles(&registry),
            crate::weapon::IMPACT_PARTICLE_COUNT
        );
    }
}
