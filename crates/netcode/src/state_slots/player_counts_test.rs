// Session player counts reach every client as host-computed shared slots.
// See: context/lib/scripting.md §5 (Session players), §12 (host-mirrored crossings)

use postretro_entities::data_descriptors::{CrossingCondition, CrossingDescriptor};
use postretro_entities::{EntityId, MachineSource, ScriptCtx, SlotValue, Transform};
use postretro_foundation::{IrNode, IrValue, Seat};
use postretro_scripting_core::state_crossings::{CrossingDetector, crossing_machine_source};

use super::{ClientStateApply, HostStateReplication, ReplicatedSlotIdentity};
use crate::SeatTable;
use crate::netcode::command_queue::{MovementOwners, WeaponOwners};
use crate::session_players::{CapturedPawns, derive_seat_statuses, write_player_counts};

const CLIENT_A: u64 = 1;
const CLIENT_B: u64 = 2;
const CLIENT_LATE: u64 = 3;
const READY: &str = "party.ready";

fn input(name: &str) -> Box<IrNode> {
    Box::new(IrNode::Input {
        name: name.to_string(),
        owner: None,
    })
}

fn number(value: f32) -> Box<IrNode> {
    Box::new(IrNode::Const {
        value: IrValue::Number(value),
    })
}

/// `loading == 0 && inPlay > 0`, the party-ready half of the all-players gate.
fn party_ready() -> CrossingDescriptor {
    CrossingDescriptor {
        slot: None,
        condition: CrossingCondition::Ir(IrNode::And {
            a: Box::new(IrNode::Eq {
                a: input("session.playerCounts.loading"),
                b: number(0.0),
            }),
            b: Box::new(IrNode::Gt {
                a: input("session.playerCounts.inPlay"),
                b: number(0.0),
            }),
        }),
        max: 1.0,
        edge: None,
        fire: vec![READY.to_string()],
    }
}

struct Machine {
    ctx: ScriptCtx,
    crossing: CrossingDetector,
    apply: ClientStateApply,
}

impl Machine {
    fn new() -> Self {
        let ctx = ScriptCtx::new();
        ctx.data_registry
            .borrow_mut()
            .populate_level(Vec::new(), vec![party_ready()], &[]);
        let mut crossing = CrossingDetector::new();
        crossing.initialize(&ctx.data_registry.borrow(), &ctx.slot_table.borrow(), &ctx);
        Self {
            ctx,
            crossing,
            apply: ClientStateApply::new(),
        }
    }

    fn ready_fires(&mut self) -> usize {
        self.crossing
            .detect(&self.ctx.slot_table.borrow())
            .iter()
            .filter(|fire| fire.reaction == READY)
            .count()
    }

    fn count(&self, name: &str) -> Option<SlotValue> {
        self.ctx
            .slot_table
            .borrow()
            .get(&format!("session.playerCounts.{name}"))
            .unwrap()
            .value
            .clone()
    }
}

struct Session {
    host: Machine,
    seats: SeatTable,
    replication: HostStateReplication,
    clients: Vec<(u64, Machine)>,
    captured: Vec<EntityId>,
    sequence: u32,
}

impl Session {
    fn new() -> Self {
        let host = Machine::new();
        let mut seats = SeatTable::local_only();
        let host_pawn = host.ctx.registry.borrow_mut().spawn(Transform::default());
        seats.bind_pawn(&mut host.ctx.registry.borrow_mut(), Seat(0), host_pawn);
        let mut replication = HostStateReplication::new();
        let mut clients = Vec::new();
        for client_id in [CLIENT_A, CLIENT_B] {
            seats.admit_or_reclaim(client_id, None, false).unwrap();
            replication.register_client(client_id);
            clients.push((client_id, Machine::new()));
        }
        Self {
            host,
            seats,
            replication,
            clients,
            captured: vec![host_pawn],
            sequence: 0,
        }
    }

    /// A client's pawn spawns and the next tick captures it, counted.
    fn spawn(&mut self, client_id: u64) {
        let seat = self.seats.seat_for_client(client_id).unwrap();
        let pawn = self
            .host
            .ctx
            .registry
            .borrow_mut()
            .spawn(Transform::default());
        self.seats
            .bind_pawn(&mut self.host.ctx.registry.borrow_mut(), seat, pawn);
        self.captured.push(pawn);
    }

    /// One authoritative host tick and frame: the host writes counts from the
    /// capture, replication sends them, each client applies, and every machine
    /// runs its crossing pass. Returns each machine's ready fires, host first.
    fn frame(&mut self) -> Vec<usize> {
        let mut statuses = Vec::new();
        let counts = derive_seat_statuses(
            &self.seats,
            &self.host.ctx.registry.borrow(),
            CapturedPawns {
                captured: &self.captured,
                counted: &self.captured,
            },
            &mut statuses,
        );
        write_player_counts(&mut self.host.ctx.slot_table.borrow_mut(), counts);

        let identity = ReplicatedSlotIdentity::default();
        let fingerprint = self
            .replication
            .fingerprint(&self.host.ctx.slot_table.borrow(), &identity);
        self.replication.ingest_frame(
            &self.host.ctx.slot_table.borrow(),
            &identity,
            &self.host.ctx.registry.borrow(),
            &MovementOwners::new(),
            &WeaponOwners::new(),
        );
        let sequence = self.sequence;
        self.sequence += 1;
        let mut fires = vec![self.host.ready_fires()];
        for (client_id, machine) in &mut self.clients {
            let records = self
                .replication
                .produce_for_client(*client_id, sequence)
                .unwrap();
            let outcome = machine.apply.apply_snapshot_state(
                &mut machine.ctx.slot_table.borrow_mut(),
                &identity,
                sequence,
                &fingerprint,
                &records,
            );
            assert_eq!(
                outcome.slot_baselines.len(),
                records.len(),
                "the client's schema matches the host's: no batch drops"
            );
            self.replication
                .apply_ack(*client_id, sequence, &outcome.slot_baselines, None);
            fires.push(machine.ready_fires());
        }
        fires
    }
}

// The counts are engine-owned shared slots: every client converges to the
// host's values, a joiner starts from them, and a crossing reading only them is
// host-mirrored, firing once on each machine when the party is ready.
#[test]
fn player_counts_replicate_to_every_client_and_late_joiner() {
    assert_eq!(
        crossing_machine_source(&party_ready(), &ScriptCtx::new().slot_table.borrow()),
        MachineSource::HostMirrored,
    );

    let mut session = Session::new();
    // Host in play, both clients loading: nobody is ready.
    assert_eq!(session.frame(), vec![0, 0, 0]);
    for (_, machine) in &session.clients {
        assert_eq!(machine.count("connected"), Some(SlotValue::Number(3.0)));
        assert_eq!(machine.count("inPlay"), Some(SlotValue::Number(1.0)));
        assert_eq!(machine.count("loading"), Some(SlotValue::Number(2.0)));
    }

    session.spawn(CLIENT_A);
    assert_eq!(session.frame(), vec![0, 0, 0]);
    session.spawn(CLIENT_B);
    assert_eq!(
        session.frame(),
        vec![1, 1, 1],
        "the last client's spawn makes the party ready on every machine at once"
    );
    for machine in std::iter::once(&session.host).chain(session.clients.iter().map(|(_, m)| m)) {
        assert_eq!(machine.count("inPlay"), Some(SlotValue::Number(3.0)));
        assert_eq!(machine.count("loading"), Some(SlotValue::Number(0.0)));
    }

    // A late joiner: admission raises loading everywhere; its first snapshot is
    // a full baseline of the host's current counts.
    session
        .seats
        .admit_or_reclaim(CLIENT_LATE, None, false)
        .unwrap();
    session.replication.register_client(CLIENT_LATE);
    session.clients.push((CLIENT_LATE, Machine::new()));
    session.frame();
    let (_, late) = session.clients.last().unwrap();
    assert_eq!(late.count("connected"), Some(SlotValue::Number(4.0)));
    assert_eq!(late.count("loading"), Some(SlotValue::Number(1.0)));
    assert_eq!(late.count("inPlay"), Some(SlotValue::Number(3.0)));
}
