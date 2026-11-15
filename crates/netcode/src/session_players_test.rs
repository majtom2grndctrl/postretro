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
