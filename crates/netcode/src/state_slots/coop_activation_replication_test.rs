// Co-op activation facts on host and clients: the player counts, the
// `occupancy` families and the player list converge to the host's, crossings
// over them fire once per machine, and every peer derives the same families.
// See: context/lib/scripting.md §5 (Session players, Occupancy), §12

use std::collections::BTreeSet;

use postretro_entities::data_descriptors::{CrossingCondition, CrossingDescriptor};
use postretro_entities::engine_state_collections::OCCUPANCY_FAMILY;
use postretro_entities::{ScriptCtx, SlotValue, Transform};
use postretro_foundation::Seat;
use postretro_scripting_core::state_crossings::CrossingDetector;

use super::{ClientStateApply, HostStateReplication, ReplicatedSlotIdentity};
use crate::SeatTable;
use crate::netcode::command_queue::{MovementOwners, WeaponOwners};
use crate::session_players::{
    CapturedPawns, PlayerCounts, PlayerRowsWriter, derive_seat_statuses, roster_rows,
    write_player_counts,
};

fn tags(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn threshold(slot: &str, condition: CrossingCondition, fire: &str) -> CrossingDescriptor {
    CrossingDescriptor {
        slot: Some(slot.to_string()),
        condition,
        max: 1.0,
        edge: None,
        fire: vec![fire.to_string()],
    }
}

/// One machine's state: its slot table with the families its scripts
/// reference, and a crossing pass over them.
struct Machine {
    ctx: ScriptCtx,
    crossings: Vec<CrossingDescriptor>,
    crossing: CrossingDetector,
    apply: ClientStateApply,
    rows: PlayerRowsWriter,
}

impl Machine {
    /// Install: declare the families at 0, then initialize crossings.
    fn install(families: &[&str], crossings: Vec<CrossingDescriptor>) -> Self {
        let ctx = ScriptCtx::new();
        let mut machine = Self {
            ctx,
            crossings,
            crossing: CrossingDetector::new(),
            apply: ClientStateApply::new(),
            rows: PlayerRowsWriter::default(),
        };
        machine.reinstall(families);
        machine
    }

    /// A level change or a reload: the family set becomes `families`, every
    /// family and count reads 0, and crossings re-arm on those values.
    fn reinstall(&mut self, families: &[&str]) {
        {
            let mut table = self.ctx.slot_table.borrow_mut();
            table.replace_family_members(&OCCUPANCY_FAMILY, &tags(families));
            table.reset_family_values(&OCCUPANCY_FAMILY);
            write_player_counts(&mut table, PlayerCounts::default());
        }
        // Unload clears the level's registry; install composes it again.
        let mut registry = self.ctx.data_registry.borrow_mut();
        registry.clear();
        registry.populate_level(Vec::new(), self.crossings.clone(), &[]);
        drop(registry);
        self.crossing = CrossingDetector::new();
        self.crossing.initialize(
            &self.ctx.data_registry.borrow(),
            &self.ctx.slot_table.borrow(),
            &self.ctx,
        );
    }

    fn fires(&mut self) -> Vec<String> {
        self.crossing
            .detect(&self.ctx.slot_table.borrow())
            .into_iter()
            .map(|fire| fire.reaction)
            .collect()
    }

    fn read(&self, slot: &str) -> Option<SlotValue> {
        self.ctx.slot_table.borrow().get(slot).and_then(|s| s.value.clone())
    }

    fn write(&self, slot: &str, value: f32) {
        self.ctx
            .slot_table
            .borrow_mut()
            .get_mut(slot)
            .unwrap()
            .write_value(Some(SlotValue::Number(value)));
    }
}

/// A listen host and its clients, replicating through the state-slot path.
struct Session {
    host: Machine,
    replication: HostStateReplication,
    clients: Vec<(u64, Machine)>,
    sequence: u32,
}

impl Session {
    fn new(host: Machine) -> Self {
        Self {
            host,
            replication: HostStateReplication::new(),
            clients: Vec::new(),
            sequence: 0,
        }
    }

    fn connect(&mut self, client_id: u64, machine: Machine) {
        self.replication.register_client(client_id);
        self.clients.push((client_id, machine));
    }

    fn client(&self, client_id: u64) -> &Machine {
        &self.clients.iter().find(|(id, _)| *id == client_id).unwrap().1
    }

    /// One host frame: replicate, apply on each client, then every machine's
    /// crossing pass. Returns `(host fires, each client's fires)`; a client
    /// whose schema disagrees with the host's drops its whole batch.
    fn frame(&mut self) -> (Vec<String>, Vec<Vec<String>>, Vec<bool>) {
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
        let host_fires = self.host.fires();
        let mut client_fires = Vec::new();
        let mut applied = Vec::new();
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
            applied.push(outcome.slot_baselines.len() == records.len());
            self.replication
                .apply_ack(*client_id, sequence, &outcome.slot_baselines, None);
            client_fires.push(machine.fires());
        }
        (host_fires, client_fires, applied)
    }
}

const HELD: &str = "occupancy.vault-plates.occupiedCount";

fn solve() -> Vec<CrossingDescriptor> {
    vec![threshold(
        HELD,
        CrossingCondition::Above { threshold: 1.0 },
        "vault.solve",
    )]
}

fn count(fires: &[String], name: &str) -> usize {
    fires.iter().filter(|fire| *fire == name).count()
}

// N1: each client's occupancy and counts converge to the host's, and its
// host-mirrored crossing fires once when the set completes, as the host's does.
#[test]
fn client_occupancy_converges_and_mirrored_crossing_fires_once() {
    let mut session = Session::new(Machine::install(&["vault-plates"], solve()));
    session.connect(1, Machine::install(&["vault-plates"], solve()));
    session.connect(2, Machine::install(&["vault-plates"], solve()));
    session.host.write("session.playerCounts.inPlay", 3.0);
    session.host.write("session.playerCounts.connected", 3.0);

    let mut fires = vec![0usize; 3];
    for held in [0.0, 1.0, 2.0, 2.0, 2.0] {
        session.host.write(HELD, held);
        let (host, clients, applied) = session.frame();
        assert!(applied.iter().all(|ok| *ok));
        fires[0] += count(&host, "vault.solve");
        for (index, client) in clients.iter().enumerate() {
            fires[index + 1] += count(client, "vault.solve");
        }
    }
    assert_eq!(fires, vec![1, 1, 1], "every machine fires once");
    for client in [1, 2] {
        assert_eq!(session.client(client).read(HELD), Some(SlotValue::Number(2.0)));
        assert_eq!(
            session.client(client).read("session.playerCounts.inPlay"),
            Some(SlotValue::Number(3.0))
        );
    }
}

// N2 (P10): a client joining while the set is held installs the family at 0,
// arms its crossing there, and fires once in the frame its baseline lands; the
// host fires nothing again.
#[test]
fn mid_hold_joiner_fires_once_from_baseline() {
    let mut session = Session::new(Machine::install(&["vault-plates"], solve()));
    session.host.write(HELD, 2.0);
    let (host, _, _) = session.frame();
    assert_eq!(count(&host, "vault.solve"), 1);

    session.connect(7, Machine::install(&["vault-plates"], solve()));
    let (host, clients, applied) = session.frame();
    assert!(applied[0], "the joiner's first snapshot is a full baseline that applies");
    assert_eq!(count(&clients[0], "vault.solve"), 1, "fires once in the baseline frame");
    assert!(host.is_empty(), "the host does not fire again");
    let (_, clients, _) = session.frame();
    assert!(clients[0].is_empty());
}

// N3 (P8): a client joining after the host visited other levels derives the
// host's exact family set (the mod's plus the current level's) from the same
// scripts, so its batches apply from its first snapshot. A peer holding a
// stale level's family would mismatch and drop every batch.
#[test]
fn late_joiner_derives_host_family_set() {
    let mut host = Machine::install(&["hill", "a"], Vec::new());
    host.reinstall(&["hill", "b"]);
    let mut session = Session::new(host);
    session.host.write("occupancy.b.occupants", 1.0);
    session.connect(3, Machine::install(&["hill", "b"], Vec::new()));
    session.connect(4, Machine::install(&["hill", "a", "b"], Vec::new()));
    let (_, _, applied) = session.frame();
    assert_eq!(applied, vec![true, false]);
    assert_eq!(
        session.client(3).read("occupancy.b.occupants"),
        Some(SlotValue::Number(1.0))
    );
}

// C8 (P20): a staged reload that changes the mod's references redeclares the
// family on host and client and resets both schemas; batches keep applying and
// kept values keep flowing.
#[test]
fn staged_reload_redeclares_mod_families_and_batches_apply() {
    let mut session = Session::new(Machine::install(&["hill"], Vec::new()));
    session.connect(5, Machine::install(&["hill"], Vec::new()));
    session.host.write("occupancy.hill.occupants", 2.0);
    assert_eq!(session.frame().2, vec![true]);

    for (_, machine) in std::iter::once((0, &mut session.host)).chain(
        session
            .clients
            .iter_mut()
            .map(|(id, machine)| (*id, machine)),
    ) {
        machine
            .ctx
            .slot_table
            .borrow_mut()
            .replace_family_members(&OCCUPANCY_FAMILY, &tags(&["hill", "arena"]));
        machine.apply.reset_schema();
    }
    session.replication.reset_schema_for_clients([5]);
    session.host.write("occupancy.arena.occupiedCount", 1.0);
    let (_, _, applied) = session.frame();
    assert_eq!(applied, vec![true], "batches keep applying after the reload");
    let client = session.client(5);
    assert_eq!(client.read("occupancy.hill.occupants"), Some(SlotValue::Number(2.0)));
    assert_eq!(
        client.read("occupancy.arena.occupiedCount"),
        Some(SlotValue::Number(1.0))
    );
}

// C11 (P21): after a level change every family, mod-scope included, and every
// count reads 0 on host and client from unload until the host's install writes
// them. A threshold crossing over a family the lone host stands on crosses in
// the first authoritative frame; a below crossing over an empty mod-scope
// family fires nothing.
#[test]
fn families_and_counts_read_zero_from_unload_through_install() {
    let crossings = || {
        vec![
            threshold(
                "occupancy.airlock.occupiedCount",
                CrossingCondition::Above { threshold: 0.0 },
                "airlock.held",
            ),
            threshold(
                "occupancy.hill.occupants",
                CrossingCondition::Below { threshold: 0.5 },
                "hill.empty",
            ),
        ]
    };
    let mut session = Session::new(Machine::install(&["hill", "airlock"], crossings()));
    session.connect(6, Machine::install(&["hill", "airlock"], crossings()));
    session.host.write("occupancy.hill.occupants", 2.0);
    session.host.write("session.playerCounts.inPlay", 2.0);
    session.frame();
    assert_eq!(
        session.client(6).read("occupancy.hill.occupants"),
        Some(SlotValue::Number(2.0))
    );

    // Unload and reinstall on every machine: everything reads 0.
    session.host.reinstall(&["hill", "airlock"]);
    for (_, machine) in &mut session.clients {
        machine.reinstall(&["hill", "airlock"]);
        machine.apply.reset_schema();
    }
    session.replication.reset_schema_for_clients([6]);
    for machine in std::iter::once(&session.host).chain(session.clients.iter().map(|(_, m)| m)) {
        for slot in [
            "occupancy.hill.occupants",
            "occupancy.airlock.occupiedCount",
            "session.playerCounts.inPlay",
        ] {
            assert_eq!(machine.read(slot), Some(SlotValue::Number(0.0)), "{slot}");
        }
    }

    // The lone host stands on the airlock: its first frame crosses.
    session.host.write("occupancy.airlock.occupiedCount", 1.0);
    let (host, clients, _) = session.frame();
    assert_eq!(count(&host, "airlock.held"), 1);
    assert_eq!(count(&clients[0], "airlock.held"), 1);
    assert_eq!(count(&host, "hill.empty"), 0, "an empty family fires no below crossing");
    assert_eq!(count(&clients[0], "hill.empty"), 0);
}

// L1 and L5 (P24, P25): each client's rows, filled from the host's roster,
// match the seats in seat order with `isLocal` on its own; once the roster
// lands, the rows by status equal the replicated counts.
#[test]
fn rows_by_status_match_counts_on_host_and_clients() {
    let mut session = Session::new(Machine::install(&[], Vec::new()));
    let mut seats = SeatTable::local_only();
    let (host_pawn, a_pawn) = {
        let mut registry = session.host.ctx.registry.borrow_mut();
        let host_pawn = registry.spawn(Transform::default());
        let a_pawn = registry.spawn(Transform::default());
        (host_pawn, a_pawn)
    };
    seats.bind_pawn(&mut session.host.ctx.registry.borrow_mut(), Seat(0), host_pawn);
    for client in [11, 12, 13] {
        seats.admit_or_reclaim(client, None, false).unwrap();
        session.connect(client, Machine::install(&[], Vec::new()));
    }
    let a_seat = seats.seat_for_client(11).unwrap();
    seats.bind_pawn(&mut session.host.ctx.registry.borrow_mut(), a_seat, a_pawn);
    seats.hold_disconnected_client(&mut session.host.ctx.registry.borrow_mut(), 13);

    // The host: host in play, client 11 down, client 12 loading, 13 held.
    let mut statuses = Vec::new();
    let counts = derive_seat_statuses(
        &seats,
        &session.host.ctx.registry.borrow(),
        CapturedPawns {
            captured: &[host_pawn, a_pawn],
            counted: &[host_pawn],
        },
        &mut statuses,
    );
    write_player_counts(&mut session.host.ctx.slot_table.borrow_mut(), counts);
    seats.record_statuses(&statuses);
    session.frame();

    for (client_id, local_row) in [(11u64, 1usize), (12, 2)] {
        let roster = seats.roster_message_for(client_id);
        let mut status = crate::ClientSessionStatus::default();
        status.retain(roster);
        let machine = &mut session
            .clients
            .iter_mut()
            .find(|(id, _)| *id == client_id)
            .unwrap()
            .1;
        machine.rows.write(
            &mut machine.ctx.slot_table.borrow_mut(),
            roster_rows(status.entries(), status.local_seat()),
        );
        let table = machine.ctx.slot_table.borrow();
        let row = |index: usize, field: &str| {
            table
                .get(&format!("session.players.{index}.{field}"))
                .unwrap()
                .value
                .clone()
                .unwrap()
        };
        let mut by_status = std::collections::BTreeMap::<String, f32>::new();
        for index in 0..postretro_foundation::seat::SESSION_PLAYER_CAPACITY {
            assert_eq!(
                row(index, "isLocal"),
                SlotValue::Boolean(index == local_row),
                "client {client_id} row {index}"
            );
            if row(index, "present") == SlotValue::Boolean(true)
                && let SlotValue::Enum(status) = row(index, "status")
            {
                *by_status.entry(status).or_default() += 1.0;
            }
        }
        for (name, slot) in [
            ("inPlay", "session.playerCounts.inPlay"),
            ("down", "session.playerCounts.down"),
            ("loading", "session.playerCounts.loading"),
            ("reconnecting", "session.playerCounts.reconnecting"),
        ] {
            assert_eq!(
                Some(SlotValue::Number(by_status.get(name).copied().unwrap_or(0.0))),
                table.get(slot).unwrap().value.clone(),
                "client {client_id}: rows reading `{name}` equal the replicated count"
            );
        }
    }
}
