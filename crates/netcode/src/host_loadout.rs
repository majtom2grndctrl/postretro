// The host returns each seat's carried loadout to that seat's own client: at
// level starts and checkpoints, and as a final character on request.
// See: context/lib/networking.md §Session-state ledger · §Leaving and ending a session

use std::collections::BTreeMap;

use postretro_entities::EntityRegistry;
use postretro_foundation::Seat;
use postretro_net::transport::NetServer;
use postretro_net::wire::{JoinSeedValue, ServerControlMessage};

use crate::{HostJoinSeeds, NetEndpoint, SeatTable, carried_to_wire};

/// Send every connection that has participated, held between levels
/// included, its own seat's carried loadout, and no one else's. A seat
/// carrying nothing is skipped. Returns how many were sent.
pub fn send_character_loadouts(
    server: &mut NetServer,
    join_seeds: &HostJoinSeeds,
    seats: &SeatTable,
) -> usize {
    let mut sent = 0;
    for client_id in server.connected_clients() {
        if server.is_closed(client_id) || !join_seeds.has_participated(client_id) {
            continue;
        }
        let Some(seat) = seats.seat_for_client(client_id) else {
            continue;
        };
        let Some(loadout) = carried_to_wire(seats.carried_state(seat)) else {
            continue;
        };
        server.send_control(
            client_id,
            postretro_net::wire::encode(&ServerControlMessage::CharacterLoadout { loadout }),
        );
        sent += 1;
    }
    sent
}

impl NetEndpoint {
    /// [`send_character_loadouts`] on a host endpoint; a client or a host
    /// without a seat table sends nothing.
    pub fn send_character_loadouts(&mut self, seats: Option<&SeatTable>) -> usize {
        match (self, seats) {
            (
                Self::Host {
                    server, join_seeds, ..
                },
                Some(seats),
            ) => send_character_loadouts(server, join_seeds, seats),
            _ => 0,
        }
    }
}

/// Answer each connection that asked for its final character at its own
/// leave: harvest its seat's live pawn now, then send its per-owner values
/// (`final_character`) and that loadout. A connection that never
/// participated holds no character of the host's making, so it is not
/// answered and keeps its saved one.
pub fn answer_final_character_requests(
    server: &mut NetServer,
    join_seeds: &HostJoinSeeds,
    seats: &mut SeatTable,
    registry: &EntityRegistry,
    life_state_names: &[String],
    requests: &[u64],
    mut final_character: impl FnMut(Seat) -> BTreeMap<String, JoinSeedValue>,
) {
    for &client_id in requests {
        if server.is_closed(client_id) || !join_seeds.has_participated(client_id) {
            log::debug!(
                "[Net] client {client_id} asked for its final character before participating; not answered"
            );
            continue;
        }
        let Some(seat) = seats.seat_for_client(client_id) else {
            continue;
        };
        seats.harvest_seat(registry, seat, life_state_names);
        let loadout = carried_to_wire(seats.carried_state(seat));
        server.send_control(
            client_id,
            postretro_net::wire::encode(&ServerControlMessage::FinalCharacter {
                slots: final_character(seat),
                loadout,
            }),
        );
    }
}
