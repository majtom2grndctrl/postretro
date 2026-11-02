//! Join and reconnect placement judge live pawns by the occupancy rule respawn shares.
use super::*;
use glam::Vec3;
use postretro_entities::Transform;
use postretro_net::wire::{ConnectClaim, PlayerClaimId};
use postretro_sim::placement_occupancy::standing_centre;

fn movement_sites(indices: &[usize]) -> Vec<PlacementSite> {
    let capsule = crate::predict_reconcile_harness_test_fixtures::component().capsule;
    indices
        .iter()
        .map(|&map_index| PlacementSite {
            map_index,
            foot_anchor: Vec3::X * (8.0 * map_index as f32),
            capsule: Some(capsule.clone()),
        })
        .collect()
}

/// The host's pawn: seat zero holds `placement`, and the pawn stands on `stands_on`.
fn host_pawn_on(
    seats: &mut SeatTable,
    registry: &mut EntityRegistry,
    sites: &[PlacementSite],
    placement: usize,
    stands_on: usize,
) -> EntityId {
    let movement = crate::predict_reconcile_harness_test_fixtures::component();
    let position = standing_centre(sites[stands_on].foot_anchor, &movement.capsule);
    let pawn = registry.spawn(Transform {
        position,
        ..Transform::default()
    });
    registry.set_component(pawn, movement).unwrap();
    seats.bind_level_spawn_placement(pawn, placement);
    seats.bind_pawn(registry, Seat(0), pawn);
    pawn
}

fn claim() -> ConnectClaim {
    ConnectClaim {
        player_id: PlayerClaimId([9; 16]),
        display_name: "Returning".to_string(),
    }
}

// Regression: reconnect recalled the held placement without a live check, so a
// returning player spawned inside a pawn that had respawned onto it.
#[test]
fn placement_on_reconnect_moves_off_a_respawned_pawn_standing_on_it() {
    let mut seats = SeatTable::from_test_session_id([21; 16]);
    let mut registry = EntityRegistry::new();
    let sites = movement_sites(&[0, 1, 2]);
    let seat = seats
        .admit_or_reclaim(61, Some(claim()), false)
        .unwrap()
        .seat;
    // Seat zero holds 0 at install; the joiner takes 1.
    let host = host_pawn_on(&mut seats, &mut registry, &sites, 0, 0);
    assert_eq!(
        seats.assign_joiner_placement(&registry, seat, &sites),
        Some(1)
    );
    assert_eq!(
        seats.hold_disconnected_client(&mut registry, 61),
        Some(seat)
    );

    // The host respawns onto the held seat's placement.
    let onto_held = standing_centre(sites[1].foot_anchor, &sites[1].capsule.clone().unwrap());
    registry
        .set_component(
            host,
            Transform {
                position: onto_held,
                ..Transform::default()
            },
        )
        .unwrap();
    let admission = seats.admit_or_reclaim(62, Some(claim()), false).unwrap();
    assert!(admission.reclaimed);
    assert_eq!(admission.seat, seat);
    assert_eq!(
        seats.assign_joiner_placement(&registry, seat, &sites),
        Some(2),
        "the reclaimed seat skips its occupied placement and seat zero's"
    );
    assert_eq!(seats.placement_for_seat(seat), Some(2));
}

#[test]
fn placement_on_reconnect_recalls_its_own_spawn_while_no_pawn_stands_there() {
    let mut seats = SeatTable::from_test_session_id([22; 16]);
    let mut registry = EntityRegistry::new();
    let sites = movement_sites(&[0, 1, 2]);
    host_pawn_on(&mut seats, &mut registry, &sites, 0, 0);
    let seat = seats
        .admit_or_reclaim(61, Some(claim()), false)
        .unwrap()
        .seat;
    assert_eq!(
        seats.assign_joiner_placement(&registry, seat, &sites),
        Some(1)
    );
    seats.hold_disconnected_client(&mut registry, 61).unwrap();
    assert!(
        seats
            .admit_or_reclaim(62, Some(claim()), false)
            .unwrap()
            .reclaimed
    );
    assert_eq!(
        seats.assign_joiner_placement(&registry, seat, &sites),
        Some(1)
    );
}

#[test]
fn placement_for_a_fresh_join_skips_a_live_occupied_unheld_spawn() {
    let mut seats = SeatTable::from_test_session_id([23; 16]);
    let mut registry = EntityRegistry::new();
    let sites = movement_sites(&[0, 1, 2]);
    // Seat zero holds 0 but its pawn stands on unheld 1.
    host_pawn_on(&mut seats, &mut registry, &sites, 0, 1);
    let joiner = seats.admit_or_reclaim(61, None, false).unwrap().seat;
    assert_eq!(
        seats.assign_joiner_placement(&registry, joiner, &sites),
        Some(2)
    );
}
