//! Opt-in AC13 measurement: freshly baked content, real CPU install, host
//! trigger capture, host snapshot production and stock transport admission.

use crate::coop_fixture_tests::initialize_dev_mod;
use crate::scripting_systems::presentation_bindings::PresentationBindings;
use crate::startup::lifecycle::tests::{install_capture_world, script_ctx, test_app};
use postretro_entities::{
    ComponentKind, NamedReaction, PrimitiveDescriptor, ReactionDescriptor, Transform,
    TriggerActivation, TriggerCommand, TriggerFireMode, TriggerVolumeComponent,
};
use postretro_net::replication::ServerReplication;
use postretro_net::transport::{NetClient, NetServer, connection_config};
use postretro_net::wire::{self, RECORD_KIND_FULL_BASELINE, RawSnapshotMessage};
use postretro_netcode::{
    HostCommandQueues, HostStateReplication, MovementOwners, NetworkIdAllocator, ReplicableSet,
    ReplicatedSlotIdentity, WeaponOwners, host_register_presentation_overrides, host_replicate,
    produce_owned_snapshots_with_host_aim,
};
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_sim::residual_drain::{ResidualRegistries, drain_frame_residuals};
use postretro_sim::scripting::builtins::data_archetype::find_descriptor;
use postretro_sim::trigger_bindings::TriggerBindingTable;
use postretro_sim::trigger_commands::TriggerFireContext;
use postretro_sim::trigger_system::{PlayerId, TriggerEventEdge, TriggerSystem};
use serde_json::json;
use std::collections::{BTreeSet, HashSet};
use std::net::{Ipv4Addr, UdpSocket};
use std::time::Duration;

const CLIENT: u64 = 7;
const STEP: Duration = Duration::from_millis(34);

fn participating_transport() -> (NetServer, NetClient) {
    let fingerprint = [0x5a; 32];
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = socket.local_addr().unwrap();
    let mut server = NetServer::new(
        socket,
        address,
        1,
        Duration::from_secs(1),
        Some(fingerprint),
    )
    .unwrap();
    server.set_mod_identity("budget.test".into(), "1.0.0".into());
    server.set_mod_digest(Some(fingerprint));
    server.set_level_parity(Some(("stress-budget".into(), fingerprint)));
    server.set_revealed_level(Some("stress-budget".into()));
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let mut client = NetClient::new(
        socket,
        address,
        CLIENT,
        Duration::from_secs(1),
        Some(fingerprint),
        None,
    )
    .unwrap();
    client.set_mod_identity("budget.test".into(), "1.0.0".into());
    client.set_mod_digest(Some(fingerprint));
    client.set_level_parity(Some(("stress-budget".into(), fingerprint)));
    client.set_revealed_level(Some("stress-budget".into()));
    server.add_relay_connection(CLIENT, None);
    client.set_connected();
    for _ in 0..128 {
        client.update_connections(STEP);
        for packet in client.packets_to_send() {
            server.process_packet_from(&packet, CLIENT);
        }
        server.update_connections(STEP);
        let _ = server.poll_handshakes();
        for packet in server.packets_to_send(CLIENT) {
            client.process_packet(&packet);
        }
        if server.is_participating(CLIENT) {
            return (server, client);
        }
    }
    panic!("measurement transport did not participate");
}

#[test]
#[ignore = "freshly bake stress-env-volumes-baseline.prl before measuring; see test module"]
fn presentation_budget_fresh_stress_map_cpu_install_and_host_capture_measurement() {
    // Run the normal prl-build on this map immediately before this test. A stale
    // checked-in PRL must not masquerade as a measurement of today's source.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let maps = root.join("content/dev/maps");
    let prl = maps.join("stress-env-volumes-baseline.prl");
    let baked_time = std::fs::metadata(&prl).unwrap().modified().unwrap();
    for source in [
        "stress-env-volumes-baseline.map",
        "stress-env-volumes.generated.ts",
    ] {
        assert!(
            baked_time
                >= std::fs::metadata(maps.join(source))
                    .unwrap()
                    .modified()
                    .unwrap(),
            "freshly bake the stress PRL before measuring {source}"
        );
    }
    let world = postretro_level_loader::load_prl(prl.to_str().unwrap()).unwrap();
    let mut app = test_app();
    let ctx = script_ctx(&app);
    let mut primitives = PrimitiveRegistry::new();
    crate::scripting::primitives::register_all(&mut primitives, ctx.clone());
    initialize_dev_mod(&mut app, &primitives);

    // AlphaLights omits 436 explicit bake-only lights from the 1,281 authored
    // lights. Fog includes six brush volumes plus the lamp and tube volumes.
    // The exact 1,324-record CI fixture is a separate, unchanged workload.
    let builtin_emitters = world
        .map_entities
        .iter()
        .filter(|entity| entity.classname == "billboard_emitter")
        .count();
    assert_eq!(
        [
            world.lights.len(),
            builtin_emitters,
            world.fog_volumes.len()
        ],
        [845, 40, 8],
        "fresh compiled stress roster changed; reconcile authored and runtime counts"
    );
    let mut descriptor_counts = [0usize; 2];
    {
        let data = ctx.data_registry.borrow();
        for placement in &world.map_entities {
            if let Some(descriptor) = find_descriptor(&data.entities, &placement.classname) {
                descriptor_counts[0] += usize::from(descriptor.light.is_some());
                descriptor_counts[1] += usize::from(descriptor.emitter.is_some());
            }
        }
    }
    let expected_counts = [
        world.lights.len() + descriptor_counts[0],
        builtin_emitters + descriptor_counts[1],
        world.fog_volumes.len(),
    ];
    install_capture_world(&mut app, &world);
    ctx.presentation_script_time.set(2.0);
    ctx.presentation_overrides.borrow_mut().clear();
    let mut counts = [0usize; 3];
    let trigger;
    let pawn;
    {
        let mut registry = ctx.registry.borrow_mut();
        for (index, kind, tag) in [
            (0, ComponentKind::Light, "budget-light"),
            (1, ComponentKind::BillboardEmitter, "budget-emitter"),
            (2, ComponentKind::FogVolume, "budget-fog"),
        ] {
            let ids: Vec<_> = registry.iter_with_kind(kind).map(|(id, _)| id).collect();
            counts[index] = ids.len();
            for id in ids {
                let mut tags = registry.get_tags(id).unwrap().to_vec();
                tags.push(tag.into());
                registry.set_tags(id, tags).unwrap();
            }
        }
        trigger = registry.spawn(Transform::default());
        pawn = registry.spawn(Transform::default());
        registry
            .set_component(
                trigger,
                TriggerVolumeComponent::new(
                    TriggerActivation::Touch,
                    String::new(),
                    "budget.capture".into(),
                    String::new(),
                    TriggerCommand::Start,
                    TriggerFireMode::Once,
                    0.0,
                    true,
                ),
            )
            .unwrap();
    }
    let total: usize = counts.iter().sum();
    assert_eq!(
        counts, expected_counts,
        "CPU install must materialize every compiled and descriptor presentation component"
    );
    // One test-only host-trigger subscriber per mutable component kind. Tags
    // cover every installed member, including any newly added authored lights.
    for (primitive, tag, args) in [
        (
            "setLightAnimation",
            "budget-light",
            json!({"periodMs":5000.0,"playCount":1,"brightness":[0.25,1.0]}),
        ),
        ("setEmitterRate", "budget-emitter", json!({"rate":0.0})),
        ("setFogDensity", "budget-fog", json!({"density":0.6})),
    ] {
        ctx.data_registry
            .borrow_mut()
            .reactions
            .push(NamedReaction {
                name: "budget.capture".into(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: primitive.into(),
                    target: None,
                    kind: None,
                    tag: Some(tag.into()),
                    on_complete: None,
                    args,
                }),
            });
    }
    let bindings = TriggerBindingTable::build_with_script_ctx(
        &ctx.registry.borrow(),
        &ctx.data_registry.borrow(),
        &ctx,
    );
    let residual = bindings
        .execute_with_script_ctx(
            trigger,
            TriggerEventEdge::Enter,
            &mut ctx.registry.borrow_mut(),
            &ctx,
            &TriggerFireContext {
                fired_teleporter: None,
                fired_trigger: Some(trigger),
                activator: Some(pawn),
                occupancy: 1,
                event_player: None,
            },
        )
        .residual()
        .expect("host trigger retains presentation residual");
    {
        let scripting = &app.session.as_ref().unwrap().scripting;
        drain_frame_residuals(
            &[(
                residual,
                trigger,
                PlayerId::Local(pawn),
                vec![PlayerId::Local(pawn)],
            )],
            &bindings,
            &TriggerSystem::default(),
            &[],
            &Default::default(),
            &scripting.scheduler,
            ResidualRegistries {
                sequence: &scripting.sequence_registry,
                reaction: &scripting.reaction_registry,
                system: &scripting.system_registry,
            },
            &ctx,
            &mut Vec::new(),
            |_, _| None,
        );
    }
    assert_eq!(
        ctx.presentation_overrides.borrow().records.len(),
        total,
        "all installed presentation components must be captured, without a fixed-size subset"
    );
    let mut allocator = NetworkIdAllocator::for_test();
    let mut replicable = ReplicableSet::default();
    let mut tracker = ServerReplication::new();
    let owners = MovementOwners::default();
    let weapon_owners = WeaponOwners::default();
    let queues = HostCommandQueues::default();
    let session = app.session.as_ref().unwrap();
    let resolver = PresentationBindings {
        lights: &session.light_bridge,
        fog: &session.fog_volume_bridge,
        map_entities: &world.map_entities,
    };
    host_register_presentation_overrides(
        &ctx.registry.borrow(),
        &mut ctx.presentation_overrides.borrow_mut(),
        &resolver,
        &mut allocator,
        &mut replicable,
        &mut tracker,
    );
    let owned = produce_owned_snapshots_with_host_aim(
        &ctx.registry.borrow(),
        &replicable,
        &mut allocator,
        &owners,
        &queues,
        None,
        &ctx.presentation_overrides.borrow(),
    );
    assert_eq!(
        owned
            .iter()
            .map(|snapshot| snapshot.components.len())
            .sum::<usize>(),
        total
    );
    assert!(
        owned
            .iter()
            .all(|snapshot| snapshot.presentation_binding.is_some())
    );
    let expected: HashSet<_> = owned.iter().map(|snapshot| snapshot.network_id).collect();
    let (mut server, mut client) = participating_transport();
    tracker.register_client(CLIENT);
    let mut slots = HostStateReplication::default();
    slots.register_client(CLIENT);
    let committed = BTreeSet::new();
    let identity = ReplicatedSlotIdentity::borrowed(None, None, &committed);
    let mut last_tick = None;
    let mut seen = HashSet::new();
    let mut total_baseline_bytes = 0;
    let mut max_budgeted = 0;
    let mut max_snapshot = 0;
    let mut max_packet_bytes = 0;
    let mut max_packets = 0;
    let mut windows = 0;
    for step in 0..1024u32 {
        client.update_connections(STEP);
        for packet in client.packets_to_send() {
            server.process_packet_from(&packet, CLIENT);
        }
        server.update_connections(STEP);
        host_replicate(
            &ctx.registry.borrow(),
            &ctx.slot_table.borrow(),
            &ctx.faction_sentiment.borrow(),
            &Default::default(),
            &identity,
            &mut server,
            &mut allocator,
            &mut tracker,
            &mut slots,
            &replicable,
            &owners,
            &weapon_owners,
            &queues,
            None,
            step * 2,
            2.0 + f64::from(step) / 30.0,
            0,
            true,
            &mut last_tick,
            &ctx.presentation_overrides.borrow(),
        );
        let packets = server.packets_to_send(CLIENT);
        max_packets = max_packets.max(packets.len());
        max_packet_bytes =
            max_packet_bytes.max(packets.iter().map(|packet| packet.len()).sum::<usize>());
        for packet in packets {
            client.process_packet(&packet);
        }
        // Production applies the reliable participation control before it
        // accepts snapshots from that epoch.
        let _ = client.drain_control();
        let received = client.drain_snapshots();
        assert!(
            !received.is_empty(),
            "stock Renet dropped the full snapshot envelope"
        );
        for bytes in received {
            max_snapshot = max_snapshot.max(bytes.len());
            let raw: RawSnapshotMessage = wire::decode(&bytes).unwrap();
            raw.validate().unwrap();
            let pending: Vec<_> = raw
                .records
                .iter()
                .filter(|record| record.record_kind == RECORD_KIND_FULL_BASELINE)
                .collect();
            let budgeted = pending
                .iter()
                .skip(1)
                .map(|record| wire::encode(*record).len())
                .sum::<usize>();
            max_budgeted = max_budgeted.max(budgeted);
            assert!(budgeted <= postretro_net::JOIN_BASELINE_BUDGET_BYTES);
            for record in &pending {
                assert!(expected.contains(&record.network_id));
                assert!(
                    seen.insert(record.network_id),
                    "unchanged in-flight baseline repeated"
                );
                total_baseline_bytes += wire::encode(*record).len();
            }
            let ack: Vec<_> = pending
                .iter()
                .map(|record| (record.network_id, record.baseline_id_or_ref))
                .collect();
            tracker.apply_ack(CLIENT, raw.sequence, &ack, &[]);
        }
        windows += 1;
        if seen == expected {
            break;
        }
    }
    assert_eq!(seen, expected);
    let bound = total_baseline_bytes.div_ceil(postretro_net::JOIN_BASELINE_BUDGET_BYTES) + 2;
    assert!(windows <= bound);
    println!(
        "stress-map actual lights={} emitters={} fog={} descriptor_lights={} descriptor_emitters={} components={} entities={} baseline_bytes={} max_budget_subtotal={} max_full_snapshot_bytes={} max_actual_transport_packet_bytes={} max_renet_packets={} transport_tick_allowance={} convergence_snapshots={} bound={} modeled_rtt_snapshots=0",
        counts[0],
        counts[1],
        counts[2],
        descriptor_counts[0],
        descriptor_counts[1],
        total,
        owned.len(),
        total_baseline_bytes,
        max_budgeted,
        max_snapshot,
        max_packet_bytes,
        max_packets,
        connection_config().available_bytes_per_tick,
        windows,
        bound
    );
}
