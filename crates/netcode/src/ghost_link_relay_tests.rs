// Ghost-link records across a participation reset, over the real relay transport.
// See: context/lib/networking.md §Current contract

use std::collections::HashSet;

use postretro_entities::{EntityId, EntityRegistry, GhostLinkComponent, Transform};
use postretro_net::replication::ServerReplication;

use crate::client::ClientReplication;
use crate::command_queue::{HostCommandQueues, MovementOwners};
use crate::replication::{
    ReplicableSet, host_register_loaded_ghost_links, produce_owned_snapshots,
};
use crate::teleport_tests::RelaySession;

/// A registry holding one installed link (map index 0, source entity 10) at
/// `armed`, as level install spawns it on every peer.
fn level_with_link(armed: bool) -> (EntityRegistry, EntityId) {
    let mut registry = EntityRegistry::new();
    let link = registry.spawn(Transform::default());
    registry
        .set_component(
            link,
            GhostLinkComponent {
                index: 0,
                source_entity: 10,
                armed,
                members: 0b1111,
            },
        )
        .unwrap();
    (registry, link)
}

/// The host side: the installed level plus the connection-scoped replication
/// state that survives a level change, as the endpoint keeps it.
struct Host {
    registry: EntityRegistry,
    link: EntityId,
    allocator: crate::NetworkIdAllocator,
    set: ReplicableSet,
    replication: ServerReplication,
    tick: u32,
}

impl Host {
    fn new() -> Self {
        let (registry, link) = level_with_link(false);
        let mut host = Self {
            registry,
            link,
            allocator: crate::NetworkIdAllocator::new(),
            set: ReplicableSet::new(),
            replication: ServerReplication::new(),
            tick: 0,
        };
        host.install();
        host
    }

    /// Level install: register the links and the participating client.
    fn install(&mut self) {
        host_register_loaded_ghost_links(
            &self.registry,
            &mut self.allocator,
            &mut self.set,
            &mut HashSet::new(),
        );
        self.replication.register_client(RelaySession::CLIENT);
    }

    /// Unload and install the next level, whose link starts disarmed. Sequence
    /// and id allocators are connection-scoped and carry on.
    fn change_level(&mut self) {
        self.allocator.reset_for_level_unload();
        self.replication.reset_for_level_unload();
        self.set = ReplicableSet::new();
        (self.registry, self.link) = level_with_link(false);
        self.install();
    }

    fn arm(&mut self) {
        let mut link = *self
            .registry
            .get_component::<GhostLinkComponent>(self.link)
            .unwrap();
        link.armed = true;
        self.registry.set_component(self.link, link).unwrap();
    }

    fn send_snapshot(&mut self, session: &mut RelaySession) {
        self.tick += 2;
        let owned = produce_owned_snapshots(
            &self.registry,
            &self.set,
            &mut self.allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
        self.replication.ingest_tick(owned);
        let sequence = self.replication.begin_batch();
        let raw = self
            .replication
            .encode_in_batch(RelaySession::CLIENT, self.tick, sequence)
            .expect("registered client");
        assert!(
            session
                .server
                .send_snapshot(RelaySession::CLIENT, postretro_net::wire::encode(&raw))
        );
    }
}

/// Apply every snapshot the client transport hands over; returns how many.
fn apply_received(
    session: &mut RelaySession,
    client: &mut ClientReplication,
    registry: &mut EntityRegistry,
) -> usize {
    let snapshots = session.client.drain_snapshots();
    for bytes in &snapshots {
        let snapshot = crate::decode_snapshot(bytes).expect("valid snapshot");
        client.apply_snapshot(registry, &snapshot);
    }
    snapshots.len()
}

fn received_armed(client: &mut ClientReplication) -> Vec<bool> {
    client
        .drain_ghost_link_updates()
        .map(|update| update.state.armed)
        .collect()
}

// P14 / P37: a link record from the old level is in flight when the host changes
// level, or restarts the same map, and re-promotes the client. Index and source
// entity repeat across a restart, so binding alone cannot tell the record is
// stale; the participation epoch drops it, and the new link keeps its spawn
// state until the new level's first record.
#[test]
fn ghost_link_stale_old_level_record_never_binds_the_new_link() {
    for restart_same_map in [false, true] {
        let mut session = RelaySession::participating();
        let mut host = Host::new();
        let mut client = ClientReplication::new();
        let (mut client_registry, _) = level_with_link(false);
        host.send_snapshot(&mut session);
        session.relay_to_client();
        assert_eq!(
            apply_received(&mut session, &mut client, &mut client_registry),
            1
        );
        assert_eq!(received_armed(&mut client), [false]);

        // The old level arms the link; that record is in flight when the host
        // leaves the level.
        host.arm();
        host.send_snapshot(&mut session);
        session.server.update_connections(RelaySession::STEP);
        let stale = session.server.packets_to_send(RelaySession::CLIENT);
        assert!(!stale.is_empty(), "the armed record is in flight");

        if restart_same_map {
            session.server.set_revealed_level(None);
            let _ = session.server.poll_handshakes();
            session.relay_to_client();
            session
                .server
                .set_revealed_level(Some("test-level".to_string()));
        } else {
            session.server.set_level_parity(None);
            let _ = session.server.poll_handshakes();
            session.relay_to_client();
            session
                .server
                .set_level_parity(Some(("test-level".to_string(), RelaySession::FINGERPRINT)));
        }
        let _ = session.server.poll_handshakes();
        session.relay_to_client();
        assert!(session.server.is_participating(RelaySession::CLIENT));

        // Both peers install the new level: the same link index and source
        // entity, disarmed at spawn.
        host.change_level();
        let _ = client.reset_for_level_unload();
        let (mut client_registry, new_link) = level_with_link(false);

        for packet in &stale {
            session.client.process_packet(packet);
        }
        assert_eq!(
            apply_received(&mut session, &mut client, &mut client_registry),
            0,
            "restart={restart_same_map}: the retired epoch's record never reaches apply"
        );
        assert!(received_armed(&mut client).is_empty());
        assert!(client.network_id_for_entity(new_link).is_none());

        // The new level's first record binds and carries the new level's state.
        host.send_snapshot(&mut session);
        session.relay_to_client();
        assert_eq!(
            apply_received(&mut session, &mut client, &mut client_registry),
            1
        );
        assert_eq!(
            received_armed(&mut client),
            [false],
            "restart={restart_same_map}: the new link's first record is the new level's"
        );
        assert!(client.network_id_for_entity(new_link).is_some());
    }
}
