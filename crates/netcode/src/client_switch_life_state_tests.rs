//! Switch refusal travels through the ordinary correlated Input outcome lane.

use super::*;
use postretro_foundation::{LifeStateProfile, PlayerMovementComponent, enter_life_state};

#[test]
fn life_state_remote_switch_refuses_then_permits_at_zero_hp_without_blocking_active_resync() {
    let mut relay = SwitchRelay::joined();
    let pawn = relay.host_pawn().unwrap();
    // The transport/equip fixture intentionally spawns a component-only pawn;
    // life capabilities require the real movement substrate used by net pawns.
    relay
        .host_registry
        .set_component(
            pawn,
            crate::predict_reconcile_harness_test_fixtures::component(),
        )
        .unwrap();
    let mut movement = relay
        .host_registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .clone();
    enter_life_state(
        &mut movement,
        0,
        &LifeStateProfile {
            switch_weapon: false,
            ..Default::default()
        },
        0,
    );
    relay.host_registry.set_component(pawn, movement).unwrap();
    let mut health = postretro_entities::components::health::HealthComponent::from_descriptor(
        &postretro_foundation::HealthDescriptor {
            max: 100.0,
            hitbox: None,
            zone_multipliers: Default::default(),
            ..Default::default()
        },
    );
    health.current = 0.0;
    relay.host_registry.set_component(pawn, health).unwrap();
    relay.declare(1);
    relay.frames(10);
    assert_eq!(relay.resolutions.len(), 1);
    assert!(
        !is_accepted(&relay.resolutions[0]),
        "refusal must resolve the client's declaration"
    );
    assert!(!relay.pending_declarations());
    assert_eq!(relay.host_active_slot(), Some(0));
    relay.declare(0);
    relay.frames(10);
    assert!(
        is_accepted(relay.resolutions.last().unwrap()),
        "an already-held active slot is acknowledged while switching is blocked"
    );
    let mut movement = relay
        .host_registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .clone();
    enter_life_state(&mut movement, 1, &LifeStateProfile::default(), 0);
    relay.host_registry.set_component(pawn, movement).unwrap();
    relay.declare(1);
    relay.frames(10);
    assert!(is_accepted(relay.resolutions.last().unwrap()));
    assert_eq!(relay.host_active_slot(), Some(1));
}
