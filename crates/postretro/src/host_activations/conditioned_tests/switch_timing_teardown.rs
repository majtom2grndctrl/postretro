// Parked equip state crosses real host/client participation and level teardown.
// See: context/lib/networking.md §Slot lifecycle
use super::*;
use postretro_net::slots::{CloseCause, SlotEvent};

impl Fixture {
    pub(in crate::host_activations::conditioned_tests) fn drive_switch_disconnects(
        &mut self,
        disconnects: &[u64],
    ) {
        for &client_id in disconnects {
            netcode::host_handle_transport_disconnect(
                &mut self.host.borrow_mut(),
                &mut self.allocator,
                &mut self.receive.as_mut().unwrap().set,
                &mut self.replication,
                &mut self.slots,
                &mut self.slot_pawns,
                &mut self.queues,
                &mut self.owners,
                &mut self.weapon_owners,
                &mut self.open,
                &mut self.pending,
                &mut self.weaponless,
                &mut self.switch_receive.as_mut().unwrap().last_tuning,
                None,
                client_id,
                None,
            );
        }
    }

    /// A disconnected session is discarded, including its pending chain. Both
    /// the receiving endpoint and relay transport are fresh on the next join.
    fn replace_disconnected_switch_client(&mut self, client_id: u64) {
        let receive = self.receive.as_mut().unwrap();
        assert!(receive.endpoint.has_pending_switch_declarations());
        receive.endpoint = netcode::NetEndpoint::from_role(
            &netcode::NetRole::Connect {
                addr: self.server.local_addr()[0],
            },
            None,
        )
        .unwrap()
        .unwrap();
        self.local.borrow_mut().clear_for_level_unload();
        self.frame = Default::default();
        self.predicted = weapon::ClientPredictedShots::new();
        self.projection = Default::default();
        self.next_switch_id = 0;
        self.unsent_switches.clear();
        self.switch_receive.as_mut().unwrap().latch = Default::default();
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        self.client = NetClient::new(
            socket,
            self.server.local_addr()[0],
            client_id,
            Duration::from_secs(1),
            Some([0x5a; 32]),
            None,
        )
        .unwrap();
        self.client
            .set_mod_identity("test.mod".into(), "1.0.0".into());
        self.client.set_mod_digest(Some([0x5a; 32]));
        self.client
            .set_level_parity(Some(("test-level".into(), [0x5a; 32])));
        // Retired datagrams must never be delivered to the replacement peer.
        self.up = PacketConditioner::new(mandated_link());
        let mut down = mandated_link();
        down.seed ^= 0x5a5a;
        self.down = PacketConditioner::new(down);
        self.shooter_client_id = client_id;
        self.server.add_relay_connection(client_id, None);
        self.client.set_connected();
        assert!(!self.server.is_participating(client_id));
        assert!(!self.client.is_participating());
        assert!(
            !self
                .receive
                .as_ref()
                .unwrap()
                .endpoint
                .has_pending_switch_declarations()
        );
    }

    pub(in crate::host_activations::conditioned_tests) fn drive_switch_lifecycle(
        &mut self,
        events: &[SlotEvent],
    ) {
        let Some(receive) = self.receive.as_mut() else {
            return;
        };
        for event in events {
            match event {
                SlotEvent::Demoted { client_id, .. } | SlotEvent::Closed { client_id, .. }
                    if *client_id == self.shooter_client_id =>
                {
                    netcode::host_handle_lifecycle(
                        &mut self.host.borrow_mut(),
                        &mut self.allocator,
                        &mut receive.set,
                        &mut self.replication,
                        &mut self.slots,
                        &mut self.slot_pawns,
                        &mut self.queues,
                        &mut self.owners,
                        &mut self.weapon_owners,
                        &mut self.open,
                        &mut self.pending,
                        &mut self.weaponless,
                        &mut self.switch_receive.as_mut().unwrap().last_tuning,
                        None,
                        std::slice::from_ref(event),
                    );
                }
                SlotEvent::Participating { client_id }
                    if *client_id == self.shooter_client_id
                        && self.server.is_current_participation_entry(event) =>
                {
                    let pawn = netcode::host_handle_accept_descriptor_at_placement(
                        &mut self.host.borrow_mut(),
                        &mut self.allocator,
                        &mut receive.set,
                        &mut self.slot_pawns,
                        &mut self.queues,
                        &mut self.owners,
                        &mut self.weapon_owners,
                        &mut self.open,
                        &mut self.pending,
                        &mut self.weaponless,
                        self.shooter_client_id,
                        &[crate::scripting::map_entity::MapEntity {
                            map_ordinal: None,
                            classname: "player_spawn".into(),
                            origin: Vec3::ZERO,
                            angles: Vec3::ZERO,
                            key_values: Default::default(),
                            tags: Vec::new(),
                        }],
                        0,
                        &self.descriptors,
                        None,
                        None,
                    )
                    .expect("re-entry composes a fresh authoritative inventory");
                    self.host_actors.pawn = pawn;
                    self.host_actors.weapon = self
                        .host
                        .borrow()
                        .get_component::<Inventory>(pawn)
                        .unwrap()
                        .active_wieldable()
                        .unwrap();
                    self.replication.register_client(self.shooter_client_id);
                    self.slots.register_client(self.shooter_client_id);
                }
                _ => {}
            }
        }
    }

    fn reset_host_level_through_endpoint(&mut self) {
        let mut endpoint =
            netcode::NetEndpoint::from_role(&netcode::NetRole::Host { port: 0 }, None)
                .unwrap()
                .unwrap();
        // Move the fixture's actual endpoint-owned state through the production
        // reset. The registry is level-install state and is retired separately.
        macro_rules! swap_state {
            () => {
                if let netcode::NetEndpoint::Host {
                    server,
                    allocator,
                    replication,
                    replicable,
                    slot_pawns,
                    command_queues,
                    owners,
                    weapon_owners,
                    open_shots,
                    pending_hit_declarations,
                    weaponless_fire_logged,
                    state_slots,
                    last_sent_tuning,
                    last_emitted_snapshot_tick,
                    ..
                } = &mut endpoint
                {
                    let receive = self.receive.as_mut().unwrap();
                    std::mem::swap(server.as_mut(), &mut self.server);
                    std::mem::swap(allocator.as_mut(), &mut self.allocator);
                    std::mem::swap(replication.as_mut(), &mut self.replication);
                    std::mem::swap(replicable, &mut receive.set);
                    std::mem::swap(slot_pawns, &mut self.slot_pawns);
                    std::mem::swap(command_queues, &mut self.queues);
                    std::mem::swap(owners, &mut self.owners);
                    std::mem::swap(weapon_owners, &mut self.weapon_owners);
                    std::mem::swap(open_shots, &mut self.open);
                    std::mem::swap(pending_hit_declarations, &mut self.pending);
                    std::mem::swap(weaponless_fire_logged, &mut self.weaponless);
                    std::mem::swap(state_slots.as_mut(), &mut self.slots);
                    std::mem::swap(
                        last_sent_tuning,
                        &mut self.switch_receive.as_mut().unwrap().last_tuning,
                    );
                    std::mem::swap(last_emitted_snapshot_tick, &mut receive.last_snapshot_tick);
                }
            };
        }
        swap_state!();
        endpoint.reset_level_scoped_host_state();
        swap_state!();
        self.host.borrow_mut().clear_for_level_unload();
        mark_unarmed_host_seat(&mut self.host.borrow_mut());
        let receive = self.receive.as_mut().unwrap();
        if let netcode::NetEndpoint::Client { client, .. } = &mut receive.endpoint {
            std::mem::swap(client.as_mut(), &mut self.client);
        }
        receive.endpoint.reset_level_scoped_client_state();
        if let netcode::NetEndpoint::Client { client, .. } = &mut receive.endpoint {
            std::mem::swap(client.as_mut(), &mut self.client);
        }
    }
}

#[test]
fn conditioned_parked_lower_is_discarded_at_demotion_level_reset_and_disconnect() {
    for boundary in ["demotion", "level", "disconnect"] {
        let mut fixture = Fixture::with_switch_timing(600, 100, mandated_link());
        fixture.weapon_only_input = true;
        fixture.lifecycle_drive = true;
        fixture.step(90, selecting(1));
        fixture.idle(91, 20);
        assert_eq!(
            fixture
                .queues
                .queue_state_for_test(CLIENT)
                .unwrap()
                .lower_repoint_tick,
            Some(125)
        );
        let retired = fixture.host_actors.pawn;
        fixture.retired_pawn_network = Some(fixture.pawn_network());
        let retirement_history = fixture.host_active.len();
        if boundary == "disconnect" {
            let closed = fixture
                .server
                .close_relay_connection(CLIENT, CloseCause::Disconnect)
                .expect("a participating relay closes its actual slot");
            assert!(matches!(
                closed,
                SlotEvent::Closed {
                    client_id: CLIENT,
                    ..
                }
            ));
            // Poll the real transport disconnect first, exactly as the host's
            // production boundary does before lifecycle cleanup.
            fixture.transport();
            fixture.drive_switch_lifecycle(std::slice::from_ref(&closed));
            assert!(fixture.server.is_closed(CLIENT));
            assert!(!fixture.server.connected_clients().contains(&CLIENT));
            assert!(fixture.queues.queue_state_for_test(CLIENT).is_none());
            assert_eq!(fixture.owners.owner_of(retired), None);
            fixture.server.set_level_parity(None);
            fixture.replace_disconnected_switch_client(9);
        } else {
            fixture.server.set_level_parity(None);
        }
        if boundary == "level" {
            fixture.reset_host_level_through_endpoint();
        }
        fixture.idle(111, 70);
        assert!(
            fixture.queues.queue_state_for_test(CLIENT).is_none(),
            "{boundary}: retired lower was discarded"
        );
        assert!(
            !fixture
                .receive
                .as_ref()
                .unwrap()
                .endpoint
                .has_pending_switch_declarations()
        );
        assert!(
            fixture
                .switch_receive
                .as_ref()
                .unwrap()
                .raw_outcomes
                .is_empty(),
            "{boundary}: teardown owes no outcome"
        );
        assert!(
            fixture.host_active[retirement_history..]
                .iter()
                .all(|(_, slot)| *slot != 1),
            "{boundary}: the retired switch never repoints, including its old tick125"
        );
        fixture
            .server
            .set_level_parity(Some(("test-level".into(), [0x5a; 32])));
        postretro_net::harness::reveal_both(&mut fixture.server, &mut fixture.client, "test-level");
        let observer = &mut fixture.switch_receive.as_mut().unwrap().observer;
        postretro_net::harness::reveal_both(
            &mut fixture.server,
            &mut observer.client,
            "test-level",
        );
        if boundary == "level" {
            observer.registry = EntityRegistry::new();
            observer.replication = Default::default();
            observer.state = Default::default();
            observer.prediction = Default::default();
        }
        fixture.idle(181, 90);
        assert!(fixture.server.is_participating(fixture.shooter_client_id));
        assert!(fixture.client.is_participating());
        if boundary == "disconnect" {
            assert_ne!(fixture.shooter_client_id, CLIENT);
            assert!(fixture.server.is_closed(CLIENT));
            assert!(!fixture.server.connected_clients().contains(&CLIENT));
        }
        assert_eq!(fixture.owners.owner_of(retired), None);
        assert!(
            fixture
                .local
                .borrow()
                .get_component::<Inventory>(fixture.local_actors.pawn)
                .is_ok(),
            "{boundary}: rejoined owner is armed; pawn={:?}",
            fixture.local_actors.pawn
        );
        assert_eq!(
            fixture
                .host
                .borrow()
                .get_component::<Inventory>(fixture.host_actors.pawn)
                .unwrap()
                .active_slot,
            0
        );
        fixture.host_active.clear();
        fixture.resolved_commands.clear();
        fixture.host_active.push((fixture.tick.wrapping_sub(1), 0));
        let observer_history = fixture
            .switch_receive
            .as_ref()
            .unwrap()
            .observer
            .selections
            .len();
        let mut lower_resolutions = Vec::new();
        fixture.step(271, selecting(1));
        assert!(
            fixture
                .receive
                .as_ref()
                .unwrap()
                .endpoint
                .has_pending_switch_declarations(),
            "{boundary}: fresh local selection declares a switch"
        );
        for tick in 272..412 {
            fixture.step(tick, neutral());
            let Some((host_tick, resolved)) = fixture.resolved_commands.last() else {
                continue;
            };
            if *host_tick != fixture.tick.wrapping_sub(1) {
                continue;
            }
            let inventory = fixture
                .host
                .borrow()
                .get_component::<Inventory>(fixture.host_actors.pawn)
                .unwrap()
                .clone();
            if (271..306).contains(&resolved.client_tick) {
                assert_eq!(
                    inventory.active_slot, 0,
                    "{boundary}: fresh switch stays on origin through305"
                );
                assert_eq!(inventory.switch_target, Some(1));
                assert_eq!(
                    fixture
                        .host
                        .borrow()
                        .get_component::<WeaponComponent>(fixture.host_actors.weapon)
                        .unwrap()
                        .state,
                    postretro_entities::components::wieldable_state::WieldableState::Lowering
                );
                lower_resolutions.push(resolved.client_tick);
            }
        }
        lower_resolutions.dedup();
        assert_eq!(
            lower_resolutions,
            (271..306).collect::<Vec<_>>(),
            "{boundary}: every fresh lower tick resolves without skipping"
        );
        assert_eq!(
            fixture.host_selection_changes(),
            [(306, 1)],
            "{boundary}: the fresh switch repoints exactly once at306"
        );
        assert_eq!(
            fixture.switch_receive.as_ref().unwrap().resolutions.len(),
            1
        );
        assert!(matches!(
            fixture.switch_receive.as_ref().unwrap().resolutions[0],
            netcode::CurrentSwitchResolution::Accepted { .. }
        ));
        let observer_changes: Vec<_> = fixture.switch_receive.as_ref().unwrap().observer.selections
            [observer_history..]
            .iter()
            .map(|(_, name)| name.as_str())
            .collect();
        assert_eq!(
            observer_changes,
            ["switch-target"],
            "{boundary}: fresh observer sees one repoint"
        );
        assert_eq!(
            fixture
                .host
                .borrow()
                .get_component::<Inventory>(fixture.host_actors.pawn)
                .unwrap()
                .active_slot,
            1,
            "{boundary}: fresh host switch; outcomes={:?}, queue={:?}",
            fixture.switch_receive.as_ref().unwrap().raw_outcomes,
            fixture
                .queues
                .queue_state_for_test(fixture.shooter_client_id)
        );
        assert_eq!(
            fixture
                .local
                .borrow()
                .get_component::<Inventory>(fixture.local_actors.pawn)
                .unwrap()
                .active_slot,
            1
        );
        assert!(
            !fixture
                .receive
                .as_ref()
                .unwrap()
                .endpoint
                .has_pending_switch_declarations()
        );
        assert_eq!(
            fixture
                .switch_receive
                .as_ref()
                .unwrap()
                .raw_outcomes
                .iter()
                .map(|(_, id, accepted)| (*id, *accepted))
                .collect::<Vec<_>>(),
            [(if boundary == "disconnect" { 0 } else { 1 }, true)]
        );
        assert!(
            fixture
                .resolved_commands
                .iter()
                .any(|(_, resolved)| resolved.client_tick == 306)
        );
    }
}
