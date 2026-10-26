// Parked equip state crosses real host/client participation and level teardown.
// See: context/lib/networking.md §Slot lifecycle
use super::*;
use postretro_net::slots::SlotEvent;

impl Fixture {
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
                    if *client_id == CLIENT =>
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
                    if *client_id == CLIENT
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
                        CLIENT,
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
                    self.replication.register_client(CLIENT);
                    self.slots.register_client(CLIENT);
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
        if boundary == "disconnect" {
            let set = &mut fixture.receive.as_mut().unwrap().set;
            netcode::host_handle_transport_disconnect(
                &mut fixture.host.borrow_mut(),
                &mut fixture.allocator,
                set,
                &mut fixture.replication,
                &mut fixture.slots,
                &mut fixture.slot_pawns,
                &mut fixture.queues,
                &mut fixture.owners,
                &mut fixture.weapon_owners,
                &mut fixture.open,
                &mut fixture.pending,
                &mut fixture.weaponless,
                &mut fixture.switch_receive.as_mut().unwrap().last_tuning,
                None,
                CLIENT,
                Some(retired),
            );
            fixture
                .receive
                .as_mut()
                .unwrap()
                .endpoint
                .demote_client_state(&mut fixture.local.borrow_mut());
        }
        fixture.server.set_level_parity(None);
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
        assert!(fixture.server.is_participating(CLIENT));
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
        fixture.idle(272, 140);
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
            fixture.queues.queue_state_for_test(CLIENT)
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
            [(1, true)]
        );
        assert!(
            fixture
                .resolved_commands
                .iter()
                .any(|(_, resolved)| resolved.client_tick == 306)
        );
    }
}
