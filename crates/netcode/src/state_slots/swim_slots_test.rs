// The swim slots on every machine: each reads, and crosses on, its own player's values.
// See: context/lib/scripting.md §5 (Per-player engine slots), §12 (Own state or host decision)

use postretro_entities::components::immersion::ImmersionComponent;
use postretro_entities::data_descriptors::{CrossingCondition, CrossingDescriptor};
use postretro_entities::{EntityId, ScriptCtx, SlotValue, Transform};
use postretro_foundation::{IrNode, MovementState, Seat};
use postretro_scripting_core::state_crossings::CrossingDetector;
use postretro_sim::scripting_systems::ui_proxy::PlayerHudStatePublisher;

use super::{ClientStateApply, HostStateReplication, ReplicatedSlotIdentity};
use crate::netcode::command_queue::{MovementOwners, WeaponOwners};
use crate::predict_reconcile_harness_test_fixtures::component as movement_component;

const CLIENT_A: u64 = 1;
const CLIENT_B: u64 = 2;
const SPLASH: &str = "splash";

/// One machine's script state: its slot table, and a local
/// `onStateCrossing(read(player.swimming), [splash])` over it.
struct Machine {
    ctx: ScriptCtx,
    crossing: CrossingDetector,
    apply: ClientStateApply,
}

impl Machine {
    fn new(ctx: ScriptCtx) -> Self {
        ctx.data_registry.borrow_mut().populate_level(
            Vec::new(),
            vec![CrossingDescriptor {
                slot: None,
                condition: CrossingCondition::Ir(IrNode::Input {
                    name: "player.swimming".to_string(),
                    owner: None,
                }),
                max: 1.0,
                edge: None,
                fire: vec![SPLASH.to_string()],
            }],
            &[],
        );
        let mut crossing = CrossingDetector::new();
        crossing.initialize(&ctx.data_registry.borrow(), &ctx.slot_table.borrow(), &ctx);
        Self {
            ctx,
            crossing,
            apply: ClientStateApply::new(),
        }
    }

    /// Splashes this machine's crossing fires on its current slot table.
    fn splashes(&mut self) -> usize {
        self.crossing
            .detect(&self.ctx.slot_table.borrow())
            .iter()
            .filter(|fire| fire.reaction == SPLASH)
            .count()
    }

    fn slot(&self, name: &str) -> Option<SlotValue> {
        self.ctx
            .slot_table
            .borrow()
            .get(name)
            .unwrap()
            .value
            .clone()
    }
}

/// A listen host with its own pawn and two connected clients' pawns, plus
/// each client machine.
struct Session {
    host: Machine,
    hud: PlayerHudStatePublisher,
    replication: HostStateReplication,
    owners: MovementOwners,
    client_a: Machine,
    client_b: Machine,
    host_pawn: EntityId,
    pawn_a: EntityId,
    pawn_b: EntityId,
    sequence: u32,
}

impl Session {
    fn new() -> Self {
        let host_ctx = ScriptCtx::new();
        let (host_pawn, pawn_a, pawn_b) = {
            let mut registry = host_ctx.registry.borrow_mut();
            let mut spawn = || {
                let pawn = registry.spawn(Transform::default());
                registry.set_component(pawn, movement_component()).unwrap();
                registry
                    .set_component(pawn, ImmersionComponent::default())
                    .unwrap();
                pawn
            };
            let pawns = (spawn(), spawn(), spawn());
            registry.mark_local_player_pawn(pawns.0).unwrap();
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
        let mut session = Self {
            hud: PlayerHudStatePublisher::new(host_ctx.clone()),
            host: Machine::new(host_ctx),
            replication,
            owners,
            client_a: Machine::new(ScriptCtx::new()),
            client_b: Machine::new(ScriptCtx::new()),
            host_pawn,
            pawn_a,
            pawn_b,
            sequence: 0,
        };
        session.frame();
        session
    }

    /// Set a pawn's movement state and the immersion host movement wrote.
    fn set_swim(&self, pawn: EntityId, swimming: bool, immersion: f32) {
        let mut registry = self.host.ctx.registry.borrow_mut();
        let mut movement = movement_component();
        if swimming {
            movement.movement_state = MovementState::Swim { fluid: 0 };
        }
        registry.set_component(pawn, movement).unwrap();
        registry
            .set_component(pawn, ImmersionComponent { immersion })
            .unwrap();
    }

    /// One host frame: the HUD publishes the host's own player, replication
    /// sends each client its own pawn's values, and each client applies them.
    /// Returns the splashes each machine's local crossing fired, as
    /// `[host, client A, client B]`.
    fn frame(&mut self) -> [usize; 3] {
        let identity = ReplicatedSlotIdentity::default();
        let _ = self
            .hud
            .tick_for_role_and_report_sampled_weapon(false, None);
        let fingerprint = self
            .replication
            .fingerprint(&self.host.ctx.slot_table.borrow(), &identity);
        self.replication.ingest_frame(
            &self.host.ctx.slot_table.borrow(),
            &identity,
            &self.host.ctx.registry.borrow(),
            &self.owners,
            &WeaponOwners::new(),
        );
        let sequence = self.sequence;
        self.sequence += 1;
        for (client_id, machine) in [
            (CLIENT_A, &mut self.client_a),
            (CLIENT_B, &mut self.client_b),
        ] {
            let records = self
                .replication
                .produce_for_client(client_id, sequence)
                .unwrap();
            let outcome = machine.apply.apply_snapshot_state(
                &mut machine.ctx.slot_table.borrow_mut(),
                &identity,
                sequence,
                &fingerprint,
                &records,
            );
            assert_eq!(outcome.slot_baselines.len(), records.len());
            self.replication
                .apply_ack(client_id, sequence, &outcome.slot_baselines, None);
        }
        [
            self.host.splashes(),
            self.client_a.splashes(),
            self.client_b.splashes(),
        ]
    }
}

// Dry and swimming values, each client's HUD reading its own, and a local
// crossing on `player.swimming` firing on each machine for that machine's own
// player only.
#[test]
fn each_machine_reads_and_crosses_on_its_own_players_swim_slots() {
    let mut session = Session::new();
    for machine in [&session.host, &session.client_a, &session.client_b] {
        assert_eq!(
            machine.slot("player.swimming"),
            Some(SlotValue::Boolean(false))
        );
        assert_eq!(
            machine.slot("player.immersion"),
            Some(SlotValue::Number(0.0))
        );
    }

    session.set_swim(session.pawn_a, true, 0.75);
    assert_eq!(
        session.frame(),
        [0, 1, 0],
        "client A's player swims: only client A's crossing fires"
    );
    assert_eq!(
        session.client_a.slot("player.swimming"),
        Some(SlotValue::Boolean(true))
    );
    assert_eq!(
        session.client_a.slot("player.immersion"),
        Some(SlotValue::Number(0.75))
    );
    for machine in [&session.host, &session.client_b] {
        assert_eq!(
            machine.slot("player.swimming"),
            Some(SlotValue::Boolean(false)),
            "another player's swim never reaches this machine's slot"
        );
        assert_eq!(
            machine.slot("player.immersion"),
            Some(SlotValue::Number(0.0))
        );
    }
    assert_eq!(session.frame(), [0, 0, 0], "a held swim fires nothing more");

    session.set_swim(session.host_pawn, true, 0.6);
    session.set_swim(session.pawn_b, false, 0.3);
    assert_eq!(
        session.frame(),
        [1, 0, 0],
        "the host's own player swims: only the host's crossing fires"
    );
    assert_eq!(
        session.host.slot("player.immersion"),
        Some(SlotValue::Number(0.6))
    );
    assert_eq!(
        session.client_b.slot("player.swimming"),
        Some(SlotValue::Boolean(false))
    );
    assert_eq!(
        session.client_b.slot("player.immersion"),
        Some(SlotValue::Number(0.3)),
        "a wading player reads its immersion before it swims"
    );

    session.set_swim(session.pawn_b, true, 0.9);
    assert_eq!(session.frame(), [0, 0, 1]);
}
