// Client switch settlement and a second peer's received selection under conditioning.
// See: context/lib/networking.md §Combat authority

use super::*;
use receive_alternate::{ReceiveSetup, descriptor_named, dev_mod_descriptors};
use std::collections::HashMap;

const OBSERVER: u64 = 8;

pub(in crate::host_activations::conditioned_tests) struct SwitchReceive {
    pub(in crate::host_activations::conditioned_tests) latch: crate::input::GameplayInputLatch,
    pub(in crate::host_activations::conditioned_tests) hold_declarations: bool,
    pub(in crate::host_activations::conditioned_tests) last_tuning:
        HashMap<u64, postretro_combat_model::TuningPayload>,
    pub(in crate::host_activations::conditioned_tests) resolutions:
        Vec<netcode::CurrentSwitchResolution>,
    pub(in crate::host_activations::conditioned_tests) observer: Observer,
    pub(in crate::host_activations::conditioned_tests) owner_slots: Vec<(u32, Option<usize>)>,
}

pub(in crate::host_activations::conditioned_tests) struct Observer {
    pub(in crate::host_activations::conditioned_tests) client: NetClient,
    pub(in crate::host_activations::conditioned_tests) up: PacketConditioner,
    pub(in crate::host_activations::conditioned_tests) down: PacketConditioner,
    pub(in crate::host_activations::conditioned_tests) registry: EntityRegistry,
    pub(in crate::host_activations::conditioned_tests) slots:
        Rc<RefCell<postretro_entities::SlotTable>>,
    pub(in crate::host_activations::conditioned_tests) replication: netcode::ClientReplication,
    pub(in crate::host_activations::conditioned_tests) state: netcode::ClientStateApply,
    pub(in crate::host_activations::conditioned_tests) prediction: netcode::ClientPrediction,
    pub(in crate::host_activations::conditioned_tests) selections: Vec<(u32, String)>,
}

impl Fixture {
    /// All inventory changes originate in host composition and arrive through
    /// production tuning installation. Existing zero-duration fixtures opt out.
    pub(in crate::host_activations::conditioned_tests) fn with_switch_timing(
        lower_ms: u32,
        raise_ms: u32,
        link: LinkConfig,
    ) -> Self {
        let (ctx, entities) = dev_mod_descriptors();
        let mut first = descriptor_named(&entities, "reference_rifle");
        let mut second = first.clone();
        first.canonical_name = Some("switch-origin".into());
        second.canonical_name = Some("switch-target".into());
        for descriptor in [&mut first, &mut second] {
            descriptor.touchable = Some(postretro_foundation::TouchableDescriptor {
                mode: postretro_foundation::TouchMode::Press,
                radius: 0.3,
            });
            let weapon = descriptor.weapon.as_mut().unwrap();
            weapon.lower_ms = lower_ms;
            weapon.raise_ms = raise_ms;
        }
        let mut third = second.clone();
        third.canonical_name = Some("switch-third".into());
        let mut fourth = second.clone();
        fourth.canonical_name = Some("switch-fourth".into());
        let mut player = descriptor_named(&entities, "player");
        player.inventory.as_mut().unwrap().loadout = vec![
            "switch-origin".into(),
            "switch-target".into(),
            "switch-third".into(),
            "switch-fourth".into(),
        ];
        let descriptors = vec![player, first.clone(), second, third, fourth];
        let mut fixture = Self::with_snapshot_receive(
            ctx,
            ReceiveSetup {
                host_descriptors: descriptors.clone(),
                client_descriptors: descriptors,
                carried: None,
                weapon: first.weapon.unwrap(),
                hold_tuning: false,
            },
        );
        let address = fixture.server.local_addr()[0];
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut client = NetClient::new(
            socket,
            address,
            OBSERVER,
            Duration::from_secs(1),
            Some([0x5a; 32]),
            None,
        )
        .unwrap();
        client.set_mod_identity("test.mod".into(), "1.0.0".into());
        client.set_mod_digest(Some([0x5a; 32]));
        client.set_level_parity(Some(("test-level".into(), [0x5a; 32])));
        postretro_net::harness::reveal_both(&mut fixture.server, &mut client, "test-level");
        fixture.server.add_relay_connection(OBSERVER, None);
        client.set_connected();
        fixture.switch_receive = Some(SwitchReceive {
            latch: Default::default(),
            hold_declarations: false,
            last_tuning: Default::default(),
            resolutions: Vec::new(),
            owner_slots: Vec::new(),
            observer: Observer {
                client,
                up: PacketConditioner::new(link),
                down: PacketConditioner::new(link),
                registry: EntityRegistry::new(),
                slots: dev_mod_descriptors().0.slot_table,
                replication: Default::default(),
                state: Default::default(),
                prediction: Default::default(),
                selections: Vec::new(),
            },
        });
        fixture.up = PacketConditioner::new(link);
        let mut down = link;
        down.seed ^= 0x5a5a;
        fixture.down = PacketConditioner::new(down);
        let floor = {
            let registry = fixture.host.borrow();
            let center = registry
                .get_component::<Transform>(fixture.host_actors.pawn)
                .unwrap()
                .position
                .y;
            let capsule = &registry
                .get_component::<PlayerMovementComponent>(fixture.host_actors.pawn)
                .unwrap()
                .capsule;
            center - capsule.radius - capsule.half_height - 0.05
        };
        fixture.world = crate::collision::CollisionWorld::from_triangles_for_test(
            vec![
                Vec3::new(-100.0, floor, -100.0),
                Vec3::new(100.0, floor, -100.0),
                Vec3::new(100.0, floor, 100.0),
                Vec3::new(-100.0, floor, 100.0),
            ],
            vec![[0, 1, 2], [0, 2, 3]],
        );
        fixture.idle(0, 90);
        fixture
    }

    pub(in crate::host_activations::conditioned_tests) fn register_switch_pending(
        &mut self,
        slot: usize,
    ) {
        let Some(switch) = self.switch_receive.as_mut() else {
            return;
        };
        let last_weapon = switch.latch.wieldable_selection_mut().last_weapon_slot();
        let netcode::NetEndpoint::Client {
            pending_switch_declarations,
            ..
        } = &mut self.receive.as_mut().unwrap().endpoint
        else {
            unreachable!()
        };
        pending_switch_declarations.push_back(netcode::PendingSwitchDeclaration {
            declaration_id: self.next_switch_id,
            target_slot: slot as u8,
            held_origin_slot: 0,
            rollback_slot: 0,
            rollback_last_weapon_slot: last_weapon,
        });
    }

    pub(in crate::host_activations::conditioned_tests) fn correct_switch_origin(
        &mut self,
        origin: usize,
    ) {
        if self.switch_receive.is_none() {
            return;
        }
        let netcode::NetEndpoint::Client {
            pending_switch_declarations,
            ..
        } = &mut self.receive.as_mut().unwrap().endpoint
        else {
            unreachable!()
        };
        let pending = pending_switch_declarations.back_mut().unwrap();
        pending.held_origin_slot = origin;
        pending.rollback_slot = origin;
    }

    /// Adversarial declaration latency, independent of the ordinary Input stream.
    pub(in crate::host_activations::conditioned_tests) fn release_switch_declarations(&mut self) {
        self.switch_receive.as_mut().unwrap().hold_declarations = false;
        for (_, declaration) in std::mem::take(&mut self.unsent_switches) {
            self.send_switch_declaration(declaration);
        }
    }

    pub(in crate::host_activations::conditioned_tests) fn receive_switch_messages(
        &mut self,
    ) -> Vec<wire::ServerMessage> {
        let Some(switch) = self.switch_receive.as_mut() else {
            return self
                .client
                .drain_input()
                .into_iter()
                .map(|bytes| wire::decode(&bytes).unwrap())
                .collect();
        };
        let netcode::NetEndpoint::Client {
            time_sync,
            pending_switch_declarations,
            ..
        } = &mut self.receive.as_mut().unwrap().endpoint
        else {
            unreachable!()
        };
        let facts = netcode::client_drive_time_sync(&mut self.client, time_sync, self.tick);
        for resolution in
            netcode::client_resolve_switch_outcomes(time_sync, pending_switch_declarations)
        {
            crate::client_weapon::settle_switch_resolution(
                &mut self.local.borrow_mut(),
                &mut switch.latch,
                resolution,
            );
            switch.resolutions.push(resolution);
        }
        let mut messages = vec![wire::ServerMessage::ActivationOutcomes(
            time_sync.drain_activation_outcomes().collect(),
        )];
        for fact in facts {
            messages.push(match fact {
                netcode::ClientShotFact::Verdict(verdict) => {
                    wire::ServerMessage::ShotVerdicts(wire::ShotVerdictsMessage {
                        verdicts: vec![verdict],
                    })
                }
                netcode::ClientShotFact::HitRefused(shot) => {
                    wire::ServerMessage::HitRefused(netcode::wire_convert::shot_id_to_wire(shot))
                }
            });
        }
        messages
    }

    pub(in crate::host_activations::conditioned_tests) fn resend_switch_tuning(&mut self) {
        let Some(switch) = self.switch_receive.as_mut() else {
            return;
        };
        let payload = netcode::tuning_payload_for_pawn(
            &self.host.borrow(),
            self.host_actors.pawn,
            &self.descriptors,
            None,
            &postretro_combat_model::EnvironmentTuning {
                level_gravity: 0.0,
                fluids: Default::default(),
            },
        );
        netcode::host_send_tuning_if_changed(
            &mut self.server,
            &mut switch.last_tuning,
            CLIENT,
            payload,
        );
    }

    pub(in crate::host_activations::conditioned_tests) fn receive_switch_observer(&mut self) {
        let network = wire::NetworkId(self.pawn_network());
        let Some(switch) = self.switch_receive.as_mut() else {
            return;
        };
        let observer = &mut switch.observer;
        observer
            .client
            .update_connections(Duration::from_millis(17));
        observer.up.enqueue_all(observer.client.packets_to_send());
        observer
            .down
            .enqueue_all(self.server.packets_to_send(OBSERVER));
        observer.up.advance(17);
        observer.down.advance(17);
        for packet in observer.up.take_ready() {
            self.server.process_packet_from(&packet, OBSERVER);
        }
        for packet in observer.down.take_ready() {
            observer.client.process_packet(&packet);
        }
        let _ = observer.client.drain_control();
        netcode::host_handle_client_messages(
            &mut self.server,
            &mut self.replication,
            &mut self.slots,
            &mut self.queues,
            &self.owners,
            &mut self.pending,
            &mut self.open,
            OBSERVER,
            self.tick,
            u64::from(self.tick) * 16_667,
        );
        netcode::client_receive_and_apply(
            &mut observer.registry,
            &mut observer.slots.borrow_mut(),
            &Default::default(),
            &Default::default(),
            &Default::default(),
            &mut observer.client,
            &mut observer.replication,
            &mut observer.state,
            &mut observer.prediction,
            &self.descriptors,
            &self.zones,
            None,
            &self.world,
            &postretro_collision::EnvironmentVolumes::empty(),
            DT,
            Duration::from_secs_f32(DT),
            None,
            None,
            None,
            false,
            &mut receive_alternate::NoBindings,
            f64::from(self.tick) * f64::from(DT),
            Some(0.0),
        );
        if let Some(selected) = observer
            .replication
            .observed_active_weapon_archetype(network)
            && observer
                .selections
                .last()
                .is_none_or(|(_, previous)| previous != selected)
        {
            observer.selections.push((self.tick, selected.to_owned()));
        }
    }
}

// Regression: begin repointed the host early, so dropping during a nonzero lower
// released the incoming weapon while the owner was still holding the outgoing one.

impl Fixture {
    pub(in crate::host_activations::conditioned_tests) fn assert_settled_selection(
        &self,
        slot: usize,
        observer_changes: &[&str],
    ) {
        for (registry, pawn) in [
            (&self.host, self.host_actors.pawn),
            (&self.local, self.local_actors.pawn),
        ] {
            let registry = registry.borrow();
            let inventory = registry.get_component::<Inventory>(pawn).unwrap();
            assert_eq!(inventory.active_slot, slot);
            assert_eq!(inventory.switch_target, None);
        }
        assert!(
            !self
                .receive
                .as_ref()
                .unwrap()
                .endpoint
                .has_pending_switch_declarations()
        );
        let observed: Vec<_> = self
            .switch_receive
            .as_ref()
            .unwrap()
            .observer
            .selections
            .iter()
            .map(|(_, selected)| selected.as_str())
            .collect();
        assert_eq!(
            observed, observer_changes,
            "the second client's actual received selections"
        );
    }

    pub(in crate::host_activations::conditioned_tests) fn host_selection_changes(
        &self,
    ) -> Vec<(u32, usize)> {
        self.host_active
            .windows(2)
            .filter(|pair| pair[0].1 != pair[1].1)
            .map(|pair| {
                let (host_tick, slot) = pair[1];
                let resolved = self
                    .resolved_commands
                    .iter()
                    .find(|(tick, _)| *tick == host_tick)
                    .unwrap();
                (resolved.1.client_tick, slot)
            })
            .collect()
    }
}

impl Fixture {
    /// Connected owners receive reload state as a projection, so seed only the
    /// host's authoritative machine; subsequent switching uses the real lanes.
    pub(in crate::host_activations::conditioned_tests) fn seed_origin_reload(
        &mut self,
        remaining_ms: u32,
        blocking: bool,
    ) {
        let mut registry = self.host.borrow_mut();
        let mut weapon = registry
            .get_component::<WeaponComponent>(self.host_actors.weapon)
            .unwrap()
            .clone();
        let ammo_type = weapon.ammo.as_ref().unwrap().ammo_type.clone();
        weapon.magazine = 20;
        weapon.state = postretro_entities::components::wieldable_state::WieldableState::Reloading;
        weapon.state_remaining_ms = remaining_ms;
        weapon.state_total_ms = remaining_ms;
        weapon.state_elapsed_sub_ms = 0.0;
        weapon.reload_credited = 0;
        weapon.block_during_reload = Some(blocking);
        registry
            .set_component(self.host_actors.weapon, weapon)
            .unwrap();
        let mut reserve = registry
            .get_component::<postretro_entities::AmmoReserve>(self.host_actors.pawn)
            .unwrap()
            .clone();
        reserve.set_exact(&ammo_type, 100);
        registry
            .set_component(self.host_actors.pawn, reserve)
            .unwrap();
    }

    pub(in crate::host_activations::conditioned_tests) fn origin_reload_balance(
        &self,
    ) -> (u32, u32) {
        let registry = self.host.borrow();
        let weapon = registry
            .get_component::<WeaponComponent>(self.host_actors.weapon)
            .unwrap();
        let reserve = registry
            .get_component::<postretro_entities::AmmoReserve>(self.host_actors.pawn)
            .unwrap();
        (
            weapon.magazine,
            reserve.available(&weapon.ammo.as_ref().unwrap().ammo_type),
        )
    }
}
