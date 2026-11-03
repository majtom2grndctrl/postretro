//! The client consumes refused reload input without cancelling a live charge.

use super::*;
use postretro_foundation::{LifeStateProfile, enter_life_state};

#[test]
fn life_state_client_reload_refusal_preserves_charge_and_consumes_held_edge() {
    let (mut registry, pawn, weapon) = fixture(
        serde_json::json!({
            "trigger":"press", "recoveryMs":10, "charge":{"minMs":10,"fullMs":500}, "steps":[{"kind":"shot"}]
        }),
        false,
    );
    let mut movement = registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .clone();
    enter_life_state(
        &mut movement,
        0,
        &LifeStateProfile {
            reload: false,
            ..Default::default()
        },
        0,
    );
    registry.set_component(pawn, movement).unwrap();
    let mut health = registry
        .get_component::<HealthComponent>(pawn)
        .unwrap()
        .clone();
    health.current = 0.0;
    registry.set_component(pawn, health).unwrap();
    let mut frame = ClientWeaponFrame::default();
    predict(&mut frame, &mut registry, &mut command(true, true), 1);
    let token = registry
        .get_component::<WeaponComponent>(weapon)
        .unwrap()
        .state
        .activation_cursor()
        .unwrap()
        .token;
    let mut held = command(false, true);
    held.reload = true;
    predict(&mut frame, &mut registry, &mut held, 2);
    assert_eq!(
        registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .state
            .activation_cursor()
            .unwrap()
            .token,
        token
    );
    assert!(
        registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .reload_press_consumed
    );
    let mut movement = registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .clone();
    enter_life_state(&mut movement, 1, &LifeStateProfile::default(), 0);
    registry.set_component(pawn, movement).unwrap();
    predict(&mut frame, &mut registry, &mut held, 3);
    assert!(
        registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .state
            .activation_cursor()
            .is_some(),
        "reopening reload cannot create a delayed press"
    );
    held.reload = false;
    predict(&mut frame, &mut registry, &mut held, 4);
    held.reload = true;
    predict(&mut frame, &mut registry, &mut held, 5);
    assert!(
        registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .state
            .activation_cursor()
            .is_none(),
        "a new permitted reload press still cancels the charge"
    );
}
