//! Local presentation names and authoritative elapsed values use distinct sources.
use super::*;
use postretro_entities::Transform;
use postretro_foundation::{PlayerMovementDescriptor, UNDECLARED_LIFE_STATE};

fn fixture() -> (ScriptCtx, EntityId) {
    let ctx = ScriptCtx::new();
    let descriptor: PlayerMovementDescriptor = serde_json::from_value(serde_json::json!({
        "capsule": { "radius": 0.4, "half_height": 0.8, "eye_height": 0.5 },
        "ground": { "speed": { "walk": 7.0, "run": 11.0, "crouch": 3.0 }, "accel": 10.0, "step_height": 0.3, "max_slope": 45.0, "friction": 8.0, "stop_speed": 2.5 },
        "air": { "forward_steer": 0.0, "accel": 0.7, "max_control_speed": 0.5, "bunny_hop": false, "jumps": 0, "jump_velocity": 5.5, "jump_ceiling": 0.0 },
        "fall": { "terminal_velocity": 40.0 }, "stuck_stop_enabled": true, "stuck_stop_threshold": 0.001
    })).unwrap();
    let mut movement = PlayerMovementComponent::from_descriptor(&descriptor);
    movement.life_state = 1;
    movement.life_state_elapsed_ticks = 900;
    let pawn = {
        let mut registry = ctx.registry.borrow_mut();
        let pawn = registry.spawn(Transform::default());
        registry.set_component(pawn, movement).unwrap();
        registry.mark_local_player_pawn(pawn).unwrap();
        pawn
    };
    ctx.data_registry.borrow_mut().life_state_names = vec!["alive".into(), "down".into()];
    (ctx, pawn)
}

#[test]
fn life_state_host_hud_publishes_name_and_exact_tick_milliseconds() {
    let (ctx, pawn) = fixture();
    let mut publisher = PlayerHudStatePublisher::new(ctx.clone());
    publisher.tick(None);
    assert_eq!(
        ctx.slot_table
            .borrow()
            .get("player.lifeState")
            .unwrap()
            .value,
        Some(SlotValue::String("down".into()))
    );
    assert_eq!(
        ctx.slot_table
            .borrow()
            .get("player.lifeStateMs")
            .unwrap()
            .value,
        Some(SlotValue::Number(15_000.0))
    );
    let mut movement = ctx
        .registry
        .borrow()
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .clone();
    movement.life_state = UNDECLARED_LIFE_STATE;
    movement.life_state_elapsed_ticks = 0;
    ctx.registry
        .borrow_mut()
        .set_component(pawn, movement)
        .unwrap();
    publisher.tick(None);
    assert_eq!(
        ctx.slot_table
            .borrow()
            .get("player.lifeState")
            .unwrap()
            .value,
        Some(SlotValue::String(String::new()))
    );
    assert_eq!(
        ctx.slot_table
            .borrow()
            .get("player.lifeStateMs")
            .unwrap()
            .value,
        Some(SlotValue::Number(0.0))
    );
}

#[test]
fn life_state_client_hud_name_uses_host_history_and_keeps_replicated_elapsed() {
    let (ctx, _) = fixture();
    ctx.slot_table
        .borrow_mut()
        .get_mut("player.lifeStateMs")
        .unwrap()
        .write_value(Some(SlotValue::Number(123.0)));
    ctx.data_registry.borrow_mut().life_state_names = vec!["wrong-local-table".into()];
    let mut publisher = PlayerHudStatePublisher::new(ctx.clone());
    publisher.tick_for_role_and_report_sampled_weapon(true, Some("down"));
    assert_eq!(
        ctx.slot_table
            .borrow()
            .get("player.lifeState")
            .unwrap()
            .value,
        Some(SlotValue::String("down".into()))
    );
    assert_eq!(
        ctx.slot_table
            .borrow()
            .get("player.lifeStateMs")
            .unwrap()
            .value,
        Some(SlotValue::Number(123.0))
    );
}
