// The owner-private defense-layer slots: each machine reads its own player's
// layer, a pawn without one reads absent, and a layer is a fingerprint change.
// See: context/lib/scripting.md §5 (Defense layer slots) · context/lib/networking.md §Version gates

use postretro_entities::components::defense_layers::{DefenseLayer, DefenseLayersComponent};
use postretro_entities::{EntityId, ScriptCtx, SlotTable, SlotValue, Transform};
use postretro_foundation::Seat;
use postretro_sim::scripting_systems::ui_proxy::PlayerHudStatePublisher;

use super::{ClientStateApply, HostStateReplication, ReplicatedSlotIdentity};
use crate::netcode::command_queue::{MovementOwners, WeaponOwners};
use crate::predict_reconcile_harness_test_fixtures::component as movement_component;

const CLIENT_A: u64 = 1;
const CLIENT_B: u64 = 2;

fn shield(value: f32, capacity: f32) -> DefenseLayersComponent {
    DefenseLayersComponent {
        descriptor: None,
        layers: vec![DefenseLayer {
            name: "shield".to_string(),
            value,
            capacity,
            recharge_delay_ms: 3_000.0,
            recharge_rate: 0.0,
            resist: Vec::new(),
            refill: Default::default(),
        }],
        health_resist: Vec::new(),
        last_hit_tick: None,
    }
}

fn layered_ctx() -> ScriptCtx {
    let ctx = ScriptCtx::new();
    ctx.slot_table
        .borrow_mut()
        .replace_player_defense_slots(&["shield".to_string()]);
    ctx
}

fn slot(ctx: &ScriptCtx, name: &str) -> Option<SlotValue> {
    ctx.slot_table.borrow().get(name).unwrap().value.clone()
}

/// A listen host whose own pawn has a 70 / 100 shield, client A's pawn a
/// 30 / 50 shield and client B's pawn no layer at all.
struct Session {
    host: ScriptCtx,
    hud: PlayerHudStatePublisher,
    replication: HostStateReplication,
    owners: MovementOwners,
    clients: [(u64, ScriptCtx, ClientStateApply); 2],
    host_pawn: EntityId,
    sequence: u32,
}

impl Session {
    fn new() -> Self {
        let host = layered_ctx();
        let (host_pawn, pawn_a, pawn_b) = {
            let mut registry = host.registry.borrow_mut();
            let mut spawn = || {
                let pawn = registry.spawn(Transform::default());
                registry.set_component(pawn, movement_component()).unwrap();
                pawn
            };
            let pawns = (spawn(), spawn(), spawn());
            registry.mark_local_player_pawn(pawns.0).unwrap();
            registry
                .set_component(pawns.0, shield(70.0, 100.0))
                .unwrap();
            registry.set_component(pawns.1, shield(30.0, 50.0)).unwrap();
            registry.bind_pawn_seat(pawns.1, Seat(1));
            registry.bind_pawn_seat(pawns.2, Seat(2));
            pawns
        };
        let mut owners = MovementOwners::new();
        owners.set(pawn_a, CLIENT_A);
        owners.set(pawn_b, CLIENT_B);
        let mut replication = HostStateReplication::new();
        replication.register_client(CLIENT_A);
        replication.register_client(CLIENT_B);
        Self {
            hud: PlayerHudStatePublisher::new(host.clone()),
            host,
            replication,
            owners,
            clients: [
                (CLIENT_A, layered_ctx(), ClientStateApply::new()),
                (CLIENT_B, layered_ctx(), ClientStateApply::new()),
            ],
            host_pawn,
            sequence: 0,
        }
    }

    /// One host frame: the HUD publishes the host's own player, replication
    /// sends each client its own pawn's values, and each client applies them.
    fn frame(&mut self) {
        let identity = ReplicatedSlotIdentity::default();
        let _ = self
            .hud
            .tick_for_role_and_report_sampled_weapon(false, None);
        let fingerprint = self
            .replication
            .fingerprint(&self.host.slot_table.borrow(), &identity);
        self.replication.ingest_frame(
            &self.host.slot_table.borrow(),
            &identity,
            &self.host.registry.borrow(),
            &self.owners,
            &WeaponOwners::new(),
        );
        let sequence = self.sequence;
        self.sequence += 1;
        for (client_id, ctx, apply) in &mut self.clients {
            let records = self
                .replication
                .produce_for_client(*client_id, sequence)
                .unwrap();
            let outcome = apply.apply_snapshot_state(
                &mut ctx.slot_table.borrow_mut(),
                &identity,
                sequence,
                &fingerprint,
                &records,
            );
            self.replication
                .apply_ack(*client_id, sequence, &outcome.slot_baselines, None);
        }
    }
}

#[test]
fn player_layer_slots_read_host_value_or_absent() {
    let mut session = Session::new();
    session.frame();

    assert_eq!(
        slot(&session.host, "player.defense.shield"),
        Some(SlotValue::Number(70.0))
    );
    assert_eq!(
        slot(&session.host, "player.defense.shield.capacity"),
        Some(SlotValue::Number(100.0))
    );
    let [(_, client_a, _), (_, client_b, _)] = &session.clients;
    assert_eq!(
        slot(client_a, "player.defense.shield"),
        Some(SlotValue::Number(30.0))
    );
    assert_eq!(
        slot(client_a, "player.defense.shield.capacity"),
        Some(SlotValue::Number(50.0))
    );
    // Client B's pawn has no shield: it reads absent, never the host pawn's 70.
    assert_eq!(slot(client_b, "player.defense.shield"), None);
    assert_eq!(slot(client_b, "player.defense.shield.capacity"), None);

    // A drained host value reaches its owner on the next frame.
    session
        .host
        .registry
        .borrow_mut()
        .set_component(session.host_pawn, shield(10.0, 100.0))
        .unwrap();
    session.frame();
    assert_eq!(
        slot(&session.host, "player.defense.shield"),
        Some(SlotValue::Number(10.0))
    );
}

#[test]
fn defense_slots_reach_only_their_owner() {
    let mut session = Session::new();
    session.frame();
    for _ in 0..3 {
        session.frame();
    }
    let [(_, client_a, _), (_, client_b, _)] = &session.clients;
    assert_eq!(
        slot(client_a, "player.defense.shield"),
        Some(SlotValue::Number(30.0))
    );
    assert_eq!(slot(client_b, "player.defense.shield"), None);
}

#[test]
fn registering_a_layer_changes_fingerprint_not_wire_version() {
    let identity = ReplicatedSlotIdentity::default();
    let plain = SlotTable::new();
    let mut layered = SlotTable::new();
    layered.replace_player_defense_slots(&["shield".to_string()]);
    let plain_fingerprint = HostStateReplication::new().fingerprint(&plain, &identity);
    let layered_fingerprint = HostStateReplication::new().fingerprint(&layered, &identity);
    assert_ne!(plain_fingerprint, layered_fingerprint);
    for name in ["player.defense.shield", "player.defense.shield.capacity"] {
        let record = layered.get(name).expect("the layer adds its slot pair");
        assert_eq!(
            record.schema.network,
            postretro_entities::ReplicationScope::OwnerPrivatePlayer
        );
        assert!(record.schema.readonly);
    }
    // A layer is a schema change, not a wire change.
    assert_eq!(postretro_net::handshake::WIRE_VERSION, 39);

    // Re-registering without the layer removes its slots.
    layered.replace_player_defense_slots(&[]);
    assert!(layered.get("player.defense.shield").is_none());
    assert_eq!(
        HostStateReplication::new().fingerprint(&layered, &identity),
        plain_fingerprint
    );
}

// R16: a connected client runs no layer step of its own; each owner reads
// its layer from the host's frames alone, so the value moves only when the
// host's recharge moves it.
#[test]
fn connected_client_reads_layers_only_from_host() {
    let mut session = Session::new();
    let pawn_a = session
        .host
        .registry
        .borrow()
        .pawn_for_seat(Seat(1))
        .expect("client A's pawn holds seat 1");
    let mut recharging = shield(30.0, 50.0);
    recharging.layers[0].recharge_delay_ms = 0.0;
    recharging.layers[0].recharge_rate = 60.0;
    session
        .host
        .registry
        .borrow_mut()
        .set_component(pawn_a, recharging)
        .unwrap();
    // Recharge runs only for a pawn above zero health.
    session
        .host
        .registry
        .borrow_mut()
        .set_component(
            pawn_a,
            postretro_entities::components::health::HealthComponent::from_descriptor(
                &postretro_foundation::HealthDescriptor {
                    max: 100.0,
                    ..Default::default()
                },
            ),
        )
        .unwrap();
    let client_a = |session: &Session| slot(&session.clients[0].1, "player.defense.shield");
    session.frame();
    assert_eq!(client_a(&session), Some(SlotValue::Number(30.0)));
    for expected in [31.0, 32.0, 33.0] {
        {
            let mut registry = session.host.registry.borrow_mut();
            registry.advance_host_tick();
            postretro_sim::sim::defense_layers::recharge_defense_layers(&mut registry);
        }
        session.frame();
        match client_a(&session) {
            Some(SlotValue::Number(value)) => assert!(
                (value - expected).abs() < 1.0e-3,
                "the host's recharge reaches the owner: {value} vs {expected}"
            ),
            other => panic!("client A reads {other:?}"),
        }
    }
    // A host frame with no host tick moves nothing on the client.
    session.frame();
    match client_a(&session) {
        Some(SlotValue::Number(value)) => assert!((value - 33.0).abs() < 1.0e-3),
        other => panic!("client A reads {other:?}"),
    }
}
