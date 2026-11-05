//! Named life-state carry observes component presence and seat lifetime.
use super::*;
use postretro_entities::{ComponentKind, Transform};
use postretro_foundation::{
    LifeStateProfile, PlayerMovementComponent, UNDECLARED_LIFE_STATE, enter_life_state,
};
use postretro_net::wire::{ConnectClaim, PlayerClaimId};

fn names() -> Vec<String> {
    vec!["alive".into(), "down".into(), "spectating".into()]
}

fn life_pawn(registry: &mut EntityRegistry, state: u8) -> EntityId {
    let pawn = registry.spawn(Transform::default());
    let mut movement = crate::predict_reconcile_harness_test_fixtures::component();
    enter_life_state(&mut movement, state, &LifeStateProfile::default(), 0);
    movement.life_state_elapsed_ticks = 700;
    registry.set_component(pawn, movement).unwrap();
    pawn
}

fn health(current: f32) -> HealthComponent {
    let mut health = HealthComponent::from_descriptor(&postretro_foundation::HealthDescriptor {
        max: 100.0,
        hitbox: None,
        zone_multipliers: Default::default(),
        ..Default::default()
    });
    health.current = current;
    health
}

fn claim() -> ConnectClaim {
    ConnectClaim {
        player_id: PlayerClaimId([0x86; 16]),
        display_name: "Life-state carrier".into(),
    }
}

#[test]
fn life_state_harvest_movement_only_creates_named_carried_record() {
    let mut seats = SeatTable::local_only();
    let mut registry = EntityRegistry::new();
    let pawn = life_pawn(&mut registry, 1);
    seats.bind_pawn(&mut registry, Seat(0), pawn);
    assert!(seats.carried_state(Seat(0)).is_none());
    seats.harvest_pawn(&registry, pawn, &names());
    let carried = seats.carried_state(Seat(0)).unwrap();
    assert_eq!(carried.life_state.as_deref(), Some("down"));
    assert!(carried.health_current.is_none());
    assert!(carried.wieldables.iter().all(Option::is_none));
}

#[test]
fn life_state_harvest_absent_movement_preserves_but_undeclared_or_unknown_state_clears() {
    let mut seats = SeatTable::local_only();
    let mut registry = EntityRegistry::new();
    let pawn = life_pawn(&mut registry, 1);
    seats.bind_pawn(&mut registry, Seat(0), pawn);
    registry.set_component(pawn, health(37.0)).unwrap();
    seats.harvest_pawn(&registry, pawn, &names());
    let live = registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .clone();
    registry
        .remove_component::<PlayerMovementComponent>(pawn)
        .unwrap();
    registry.set_component(pawn, health(42.0)).unwrap();
    seats.harvest_pawn(&registry, pawn, &names());
    let carried = seats.carried_state(Seat(0)).unwrap();
    assert_eq!(carried.life_state.as_deref(), Some("down"));
    assert!((carried.health_current.unwrap() - 42.0).abs() < f32::EPSILON);
    registry
        .remove_component_kind(pawn, ComponentKind::Health)
        .unwrap();
    seats.harvest_pawn(&registry, pawn, &names());
    assert_eq!(
        seats.carried_state(Seat(0)).unwrap().life_state.as_deref(),
        Some("down")
    );
    for state in [UNDECLARED_LIFE_STATE, 7] {
        registry.set_component(pawn, live.clone()).unwrap();
        seats.harvest_pawn(&registry, pawn, &names());
        assert_eq!(
            seats.carried_state(Seat(0)).unwrap().life_state.as_deref(),
            Some("down")
        );
        let mut changed = live.clone();
        changed.life_state = state;
        registry.set_component(pawn, changed).unwrap();
        seats.harvest_pawn(&registry, pawn, &names());
        let carried = seats.carried_state(Seat(0)).unwrap();
        assert!(
            carried.life_state.is_none(),
            "present state {state} must replace stale name with absence"
        );
        assert!((carried.health_current.unwrap() - 42.0).abs() < f32::EPSILON);
    }
    // A kept-name hot reload remaps the live id before harvest; the departing
    // record must take that current state rather than the seat's prior one.
    let mut remapped = live;
    remapped.life_state = 0;
    registry.set_component(pawn, remapped).unwrap();
    seats.harvest_pawn(&registry, pawn, &names());
    assert_eq!(
        seats.carried_state(Seat(0)).unwrap().life_state.as_deref(),
        Some("alive")
    );
}

#[test]
fn life_state_harvest_suspend_keeps_live_and_held_names_then_level_unload_clears_both() {
    let mut seats = SeatTable::local_only();
    let mut registry = EntityRegistry::new();
    let host = life_pawn(&mut registry, 1);
    registry.set_component(host, health(41.0)).unwrap();
    seats.bind_pawn(&mut registry, Seat(0), host);
    let remote_seat = seats
        .admit_or_reclaim(70, Some(claim()), false)
        .unwrap()
        .seat;
    let remote = life_pawn(&mut registry, 1);
    registry.set_component(remote, health(0.0)).unwrap();
    seats.bind_pawn(&mut registry, remote_seat, remote);
    seats.harvest_pawn(&registry, remote, &names());
    seats.hold_disconnected_client(&mut registry, 70);
    registry.despawn(remote).unwrap();
    seats.advance_hold_clock(Duration::from_secs(5));
    // This is the host suspend harvest. A held seat has no bound pawn, so it
    // retains its prior name while the still-materialized host is harvested.
    seats.harvest_bound_pawns(&registry, &names());
    for seat in [Seat(0), remote_seat] {
        assert_eq!(
            seats.carried_state(seat).unwrap().life_state.as_deref(),
            Some("down")
        );
    }
    assert_eq!(
        registry
            .get_component::<PlayerMovementComponent>(host)
            .unwrap()
            .life_state_elapsed_ticks,
        700,
        "harvesting on suspend mutates no live state or clock"
    );
    assert!(seats.pawn_for_client(70).is_none());
    // Real level retirement clears names in every record, not only the host
    // record reached by the last bound-pawn loop. Other carry fields survive.
    registry.clear_for_level_unload();
    seats.clear_pawn_bindings_for_level_unload(&mut registry);
    for seat in [Seat(0), remote_seat] {
        assert!(seats.carried_state(seat).unwrap().life_state.is_none());
    }
    assert!(
        (seats
            .carried_state(Seat(0))
            .unwrap()
            .health_current
            .unwrap()
            - 41.0)
            .abs()
            < f32::EPSILON
    );
    assert_eq!(
        seats
            .admit_or_reclaim(71, Some(claim()), false)
            .unwrap()
            .seat,
        remote_seat,
        "the held seat itself survives the level change, with initial-state carry semantics"
    );
}

#[test]
fn life_state_harvest_hold_expiry_discards_named_record_before_fresh_admission() {
    let mut seats = SeatTable::local_only();
    let mut registry = EntityRegistry::new();
    let expired = seats
        .admit_or_reclaim(80, Some(claim()), false)
        .unwrap()
        .seat;
    let pawn = life_pawn(&mut registry, 1);
    seats.bind_pawn(&mut registry, expired, pawn);
    seats.harvest_pawn(&registry, pawn, &names());
    seats.hold_disconnected_client(&mut registry, 80);
    registry.despawn(pawn).unwrap();
    seats.advance_hold_clock(HOLD_WINDOW + Duration::from_nanos(1));
    assert_eq!(seats.release_expired_holds(), [expired]);
    assert!(!seats.contains_seat(expired));
    assert!(seats.carried_state(expired).is_none());
    let fresh = seats.admit_or_reclaim(81, Some(claim()), false).unwrap();
    assert_ne!(fresh.seat, expired);
    assert!(!fresh.reclaimed);
    assert!(seats.carried_state(fresh.seat).is_none());
}
