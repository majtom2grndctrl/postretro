// Bind-time validation and evaluated-pawn/local-pawn separation for life reads.
use crate::ctx::ScriptCtx;
use crate::ir::scope::BindingScope;
use crate::ir::{BakedIr, CURRENT_IR_VERSION, IrNode, IrValue, bind, eval_value};
use crate::ir_player_scope::PlayerConditionScope;
use crate::ir_scopes::StoreScope;
use crate::slot_table::{
    ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
};
use postretro_entities::Transform;
use postretro_foundation::{PlayerMovementComponent, PlayerMovementDescriptor};

fn movement(state: u8) -> PlayerMovementComponent {
    let descriptor: PlayerMovementDescriptor = serde_json::from_value(serde_json::json!({
        "capsule": { "radius": 0.4, "half_height": 0.8, "eye_height": 0.5 },
        "ground": { "speed": { "walk": 7.0, "run": 11.0, "crouch": 3.0 },
            "accel": 10.0, "step_height": 0.3, "max_slope": 45.0,
            "friction": 8.0, "stop_speed": 2.5 },
        "air": { "forward_steer": 0.0, "accel": 0.7, "max_control_speed": 0.5,
            "bunny_hop": false, "jumps": 0, "jump_velocity": 5.5, "jump_ceiling": 0.0 },
        "fall": { "terminal_velocity": 40.0 },
        "stuck_stop_enabled": true, "stuck_stop_threshold": 0.001
    }))
    .unwrap();
    let mut movement = PlayerMovementComponent::from_descriptor(&descriptor);
    movement.life_state = state;
    movement
}

fn predicate(name: &str) -> BakedIr {
    BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root: IrNode::Input {
            name: format!("player.inLifeState:{name}"),
            owner: None,
        },
    }
}

fn seeded_ctx() -> ScriptCtx {
    let ctx = ScriptCtx::new();
    ctx.data_registry.borrow_mut().life_state_names = vec!["alive".into(), "down".into()];
    if ctx.slot_table.borrow().get("player.lifeState").is_none() {
        ctx.slot_table
            .borrow_mut()
            .insert(
                "player.lifeState".into(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::String,
                    default: Some(SlotValue::String(String::new())),
                    range: None,
                    persist: false,
                    readonly: true,
                    ownership: SlotOwnership::Engine,
                    network: ReplicationScope::None,
                    per_owner: false,
                    accumulate: None,
                }),
            )
            .unwrap();
    }
    ctx.slot_table
        .borrow_mut()
        .get_mut("player.lifeState")
        .unwrap()
        .write_value(Some(SlotValue::String("alive".into())));
    ctx
}

#[test]
fn life_state_condition_reads_the_evaluated_pawn_while_crossing_reads_local_name() {
    let ctx = seeded_ctx();
    let (down, undeclared) = {
        let mut registry = ctx.registry.borrow_mut();
        let down = registry.spawn(Transform::default());
        registry.set_component(down, movement(1)).unwrap();
        let undeclared = registry.spawn(Transform::default());
        registry
            .set_component(
                undeclared,
                movement(postretro_foundation::UNDECLARED_LIFE_STATE),
            )
            .unwrap();
        (down, undeclared)
    };
    let condition = PlayerConditionScope::new(ctx.clone());
    condition.begin_condition();
    let program = bind(&predicate("down"), &condition).unwrap();
    let inputs = condition.finish_condition();
    condition.seed_player(&ctx.registry.borrow(), &ctx.slot_table.borrow(), down, None);
    assert!(condition.observes(&inputs));
    assert_eq!(eval_value(&program, &condition), IrValue::Bool(true));
    condition.seed_player(
        &ctx.registry.borrow(),
        &ctx.slot_table.borrow(),
        undeclared,
        None,
    );
    assert!(condition.observes(&inputs));
    assert_eq!(eval_value(&program, &condition), IrValue::Bool(false));

    let local = StoreScope::script(ctx.clone());
    let crossing = bind(&predicate("down"), &local).unwrap();
    assert_eq!(eval_value(&crossing, &local), IrValue::Bool(false));
    ctx.slot_table
        .borrow_mut()
        .get_mut("player.lifeState")
        .unwrap()
        .write_value(Some(SlotValue::String("down".into())));
    assert_eq!(eval_value(&crossing, &local), IrValue::Bool(true));
    assert!(local.resolve_output("player.inLifeState:down").is_none());
}

#[test]
fn life_state_unknown_names_fail_both_binds_with_the_state_in_the_diagnostic() {
    let ctx = seeded_ctx();
    let capture = postretro_test_log_capture::LogCapture::start();
    assert!(
        bind(
            &predicate("missing"),
            &PlayerConditionScope::new(ctx.clone())
        )
        .is_err()
    );
    assert!(bind(&predicate("missing"), &StoreScope::script(ctx)).is_err());
    capture.assert_logged(log::Level::Error, "unknown life state `missing`");
}
