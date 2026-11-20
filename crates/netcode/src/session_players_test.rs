use super::*;
use postretro_entities::registry::Transform;

const FIRST: u64 = 11;
const SECOND: u64 = 12;

fn admit(seats: &mut SeatTable, client_id: u64) -> Seat {
    seats
        .admit_or_reclaim(client_id, None, false)
        .expect("fixture seat namespace has room")
        .seat
}

fn spawn_bound(seats: &mut SeatTable, registry: &mut EntityRegistry, seat: Seat) -> EntityId {
    let pawn = registry.spawn(Transform::default());
    seats.bind_pawn(registry, seat, pawn);
    pawn
}

#[test]
fn seat_status_reads_live_pawn_capture_and_hold() {
    let mut seats = SeatTable::local_only();
    let mut registry = EntityRegistry::new();
    let host = spawn_bound(&mut seats, &mut registry, Seat(0));
    let in_play = admit(&mut seats, FIRST);
    let in_play_pawn = spawn_bound(&mut seats, &mut registry, in_play);
    let down = admit(&mut seats, SECOND);
    let down_pawn = spawn_bound(&mut seats, &mut registry, down);
    let loading = admit(&mut seats, 13);
    let held = admit(&mut seats, 14);
    seats.hold_disconnected_client(&mut registry, 14);

    let captured = [host, in_play_pawn, down_pawn];
    let counted = [host, in_play_pawn];
    let pawns = CapturedPawns {
        captured: &captured,
        counted: &counted,
    };
    let mut statuses = Vec::new();
    let counts = derive_seat_statuses(&seats, &registry, pawns, &mut statuses);

    assert_eq!(
        statuses,
        vec![
            (Seat(0), SeatStatus::InPlay),
            (in_play, SeatStatus::InPlay),
            (down, SeatStatus::Down),
            (loading, SeatStatus::Loading),
            (held, SeatStatus::Reconnecting),
        ]
    );
    assert_eq!(
        counts,
        PlayerCounts {
            loading: 1,
            in_play: 2,
            down: 1,
            reconnecting: 1,
        }
    );
    assert_eq!(counts.connected(), 4);
}

// P26: status follows the live pawn, not the seat table's binding.
#[test]
fn seat_without_live_pawn_reads_loading() {
    let mut seats = SeatTable::local_only();
    let mut registry = EntityRegistry::new();
    let client = admit(&mut seats, FIRST);
    let pawn = spawn_bound(&mut seats, &mut registry, client);
    let captured = [pawn];
    let pawns = CapturedPawns {
        captured: &captured,
        counted: &captured,
    };
    assert_eq!(
        seat_status(&seats, &registry, pawns, client),
        SeatStatus::InPlay
    );

    // A demotion despawns the pawn and keeps the seat's client binding.
    registry.despawn(pawn).unwrap();
    assert_eq!(seats.client_for_seat(client), Some(FIRST));
    assert_eq!(
        seat_status(&seats, &registry, pawns, client),
        SeatStatus::Loading
    );

    // The host's seat between install and its pawn spawning.
    assert_eq!(
        seat_status(&seats, &registry, CapturedPawns::NONE, Seat(0)),
        SeatStatus::Loading
    );
}

#[test]
fn pawn_spawned_after_the_capture_reads_loading_until_captured() {
    let mut seats = SeatTable::local_only();
    let mut registry = EntityRegistry::new();
    spawn_bound(&mut seats, &mut registry, Seat(0));
    assert_eq!(
        seat_status(&seats, &registry, CapturedPawns::NONE, Seat(0)),
        SeatStatus::Loading
    );
}

#[test]
fn player_counts_write_shared_engine_slots_only_on_change() {
    // `SlotTable::default` installs the engine catalog, counts included.
    let mut table = SlotTable::default();
    let counts = PlayerCounts {
        loading: 1,
        in_play: 2,
        down: 0,
        reconnecting: 1,
    };
    write_player_counts(&mut table, counts);
    let read = |table: &mut SlotTable, name: &str| {
        let slot = table.get_mut(name).unwrap();
        (slot.value.clone(), slot.write_generation())
    };
    let (connected, generation) = read(&mut table, "session.playerCounts.connected");
    assert_eq!(connected, Some(SlotValue::Number(3.0)));
    assert_eq!(
        read(&mut table, "session.playerCounts.inPlay").0,
        Some(SlotValue::Number(2.0))
    );
    assert_eq!(
        read(&mut table, "session.playerCounts.reconnecting").0,
        Some(SlotValue::Number(1.0))
    );

    write_player_counts(&mut table, counts);
    assert_eq!(
        read(&mut table, "session.playerCounts.connected").1,
        generation,
        "an unchanged count is not rewritten"
    );
}

fn row(table: &SlotTable, index: usize) -> (SlotValue, SlotValue, SlotValue, SlotValue) {
    let get = |field: &str| {
        table
            .get(&format!("session.players.{index}.{field}"))
            .unwrap()
            .value
            .clone()
            .unwrap()
    };
    (get("present"), get("name"), get("status"), get("isLocal"))
}

fn shown(
    present: bool,
    name: &str,
    status: &str,
    is_local: bool,
) -> (SlotValue, SlotValue, SlotValue, SlotValue) {
    (
        SlotValue::Boolean(present),
        SlotValue::String(name.to_string()),
        SlotValue::Enum(status.to_string()),
        SlotValue::Boolean(is_local),
    )
}

// L2 and P25: rows fill densely in seat order; an empty name reads `Player N`
// from the seat, unchanged when an earlier seat leaves and later rows shift up;
// a sent name reads as sent; `isLocal` follows the local seat to its new row.
#[test]
fn player_rows_follow_seats_names_and_local_seat() {
    use postretro_net::wire::{RosterEntry, roster_status};
    let mut table = SlotTable::default();
    let mut writer = PlayerRowsWriter::default();
    let entry = |seat: u16, name: &str, status: u8| RosterEntry {
        seat,
        connected: true,
        name: name.to_string(),
        status,
    };
    let three = vec![
        entry(0, "", roster_status::IN_PLAY),
        entry(1, "Ash", roster_status::DOWN),
        entry(2, "", roster_status::LOADING),
    ];
    writer.write(&mut table, roster_rows(&three, Some(Seat(2))));
    assert_eq!(row(&table, 0), shown(true, "Player 1", "inPlay", false));
    assert_eq!(row(&table, 1), shown(true, "Ash", "down", false));
    assert_eq!(row(&table, 2), shown(true, "Player 3", "loading", true));
    assert_eq!(row(&table, 3), shown(false, "", "loading", false));

    let generation = |table: &SlotTable| {
        table
            .get("session.players.0.name")
            .unwrap()
            .write_generation()
    };
    let before = generation(&table);
    writer.write(&mut table, roster_rows(&three, Some(Seat(2))));
    assert_eq!(
        generation(&table),
        before,
        "an unchanged row is not rewritten"
    );

    // Seat 1's hold expires: seat 2 moves up to row 1 and keeps its name.
    let two = vec![three[0].clone(), three[2].clone()];
    writer.write(&mut table, roster_rows(&two, Some(Seat(2))));
    assert_eq!(row(&table, 1), shown(true, "Player 3", "loading", true));
    assert_eq!(row(&table, 2), shown(false, "", "loading", false));

    // A client returning after its hold mints seat 3.
    let rejoined = vec![
        three[0].clone(),
        three[2].clone(),
        entry(3, "", roster_status::RECONNECTING),
    ];
    writer.write(&mut table, roster_rows(&rejoined, Some(Seat(3))));
    assert_eq!(row(&table, 1), shown(true, "Player 3", "loading", false));
    assert_eq!(
        row(&table, 2),
        shown(true, "Player 4", "reconnecting", true)
    );

    writer.clear(&mut table);
    for index in 0..postretro_foundation::seat::SESSION_PLAYER_CAPACITY {
        assert_eq!(row(&table, index).0, SlotValue::Boolean(false));
    }
}

// The host's rows come from its statuses and its seats' claims; its own seat
// is the local row.
#[test]
fn host_rows_read_statuses_and_claim_names() {
    let mut seats = SeatTable::local_only();
    let client = seats
        .admit_or_reclaim(
            FIRST,
            Some(postretro_net::wire::ConnectClaim {
                player_id: postretro_net::wire::PlayerClaimId([7; 16]),
                display_name: "Wren".to_string(),
            }),
            false,
        )
        .unwrap()
        .seat;
    let statuses = [(Seat(0), SeatStatus::InPlay), (client, SeatStatus::Loading)];
    let mut table = SlotTable::default();
    PlayerRowsWriter::default().write(&mut table, seat_rows(&seats, &statuses));
    assert_eq!(row(&table, 0), shown(true, "Player 1", "inPlay", true));
    assert_eq!(row(&table, 1), shown(true, "Wren", "loading", false));
}

// Seat 0 asserts no claim: its name is the local player's character, in its
// own row and in the roster every client fills its rows from. A new hosted
// run keeps it, and an overlong name cuts as a claim's does.
#[test]
fn host_seat_reads_the_local_name_in_rows_and_roster() {
    use postretro_net::wire::DISPLAY_NAME_MAX_BYTES;
    let mut seats = SeatTable::local_only();
    seats.set_local_display_name("Vex");
    seats
        .remint_for_hosting(|bytes| {
            bytes.fill(9);
            Ok(())
        })
        .unwrap();
    let statuses = [(Seat(0), SeatStatus::InPlay)];
    let mut table = SlotTable::default();
    PlayerRowsWriter::default().write(&mut table, seat_rows(&seats, &statuses));
    assert_eq!(row(&table, 0), shown(true, "Vex", "inPlay", true));
    assert_eq!(seats.roster_entries()[0].name, "Vex");

    seats.set_local_display_name(&"a".repeat(DISPLAY_NAME_MAX_BYTES + 5));
    assert_eq!(seats.display_name(Seat(0)).len(), DISPLAY_NAME_MAX_BYTES);
}
