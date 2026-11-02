//! The assist progress slots: the host publishes the own pawn's values; a
//! connected client writes neither, so a committed host sample survives.
//! See: context/lib/scripting.md §12 (Assist)

use super::*;
use postretro_entities::Transform;
use postretro_foundation::PlayerMovementDescriptor;

const PROGRESS: &str = "player.assistProgress";
const RECEIVED: &str = "player.assistReceivedProgress";

fn fixture(assistable: bool) -> ScriptCtx {
    let ctx = ScriptCtx::new();
    let descriptor: PlayerMovementDescriptor = serde_json::from_value(serde_json::json!({
        "capsule": { "radius": 0.4, "half_height": 0.8, "eye_height": 0.5 },
        "ground": { "speed": { "walk": 7.0, "run": 11.0, "crouch": 3.0 }, "accel": 10.0, "step_height": 0.3, "max_slope": 45.0, "friction": 8.0, "stop_speed": 2.5 },
        "air": { "forward_steer": 0.0, "accel": 0.7, "max_control_speed": 0.5, "bunny_hop": false, "jumps": 0, "jump_velocity": 5.5, "jump_ceiling": 0.0 },
        "fall": { "terminal_velocity": 40.0 }, "stuck_stop_enabled": true, "stuck_stop_threshold": 0.001
    })).unwrap();
    let mut movement = PlayerMovementComponent::from_descriptor(&descriptor);
    movement.assistable = assistable;
    movement.assist_progress = 0.25;
    movement.assist_received_progress = 0.5;
    {
        let mut registry = ctx.registry.borrow_mut();
        let pawn = registry.spawn(Transform::default());
        registry.set_component(pawn, movement).unwrap();
        registry.mark_local_player_pawn(pawn).unwrap();
    }
    ctx
}

fn slot(ctx: &ScriptCtx, name: &str) -> Option<SlotValue> {
    ctx.slot_table.borrow().get(name).unwrap().value.clone()
}

#[test]
fn assist_host_hud_publishes_both_progress_slots_and_received_reads_zero_off_assistable() {
    let ctx = fixture(true);
    let mut publisher = PlayerHudStatePublisher::new(ctx.clone());
    publisher.tick(None);
    assert_eq!(slot(&ctx, PROGRESS), Some(SlotValue::Number(0.25)));
    assert_eq!(slot(&ctx, RECEIVED), Some(SlotValue::Number(0.5)));

    // O50: a pawn whose state declares no `assistable` reads 0 received.
    let ctx = fixture(false);
    let mut publisher = PlayerHudStatePublisher::new(ctx.clone());
    publisher.tick(None);
    assert_eq!(slot(&ctx, PROGRESS), Some(SlotValue::Number(0.25)));
    assert_eq!(slot(&ctx, RECEIVED), Some(SlotValue::Number(0.0)));
}

/// A connected client's component carries no accumulator output: the
/// publisher writes neither slot, so the committed host sample survives the
/// next frame's publish (as `player.lifeStateMs` does).
#[test]
fn assist_client_hud_writes_neither_progress_slot() {
    let ctx = fixture(true);
    for (name, value) in [(PROGRESS, 0.75), (RECEIVED, 0.125)] {
        ctx.slot_table
            .borrow_mut()
            .get_mut(name)
            .unwrap()
            .write_value(Some(SlotValue::Number(value)));
    }
    let mut publisher = PlayerHudStatePublisher::new(ctx.clone());
    publisher.tick_for_role_and_report_sampled_weapon(true, None);
    assert_eq!(slot(&ctx, PROGRESS), Some(SlotValue::Number(0.75)));
    assert_eq!(slot(&ctx, RECEIVED), Some(SlotValue::Number(0.125)));
}
