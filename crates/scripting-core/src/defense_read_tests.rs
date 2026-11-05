// Player-layer reads by scope: a plain read in a player-event condition is
// the evaluated player, `byPlayer(on.player)` is the event's player, an
// unregistered layer fails the bind.
// See: context/lib/scripting.md §11 (Defense layers), §12 (Player events)

use std::sync::Arc;

use postretro_entities::components::defense_layers::{DefenseLayer, DefenseLayersComponent};
use postretro_entities::{EntityId, ScriptCtx, Transform};
use postretro_foundation::defense::DefenseVocabulary;
use postretro_foundation::{
    BakedIr, BindingScope, CURRENT_IR_VERSION, IrNode, IrValue, Seat, bind, eval_value,
};

use crate::ir_player_scope::PlayerConditionScope;
use crate::ir_scopes::DispatchScope;

fn ctx() -> ScriptCtx {
    let ctx = ScriptCtx::new();
    ctx.registry
        .borrow_mut()
        .set_defense_vocabulary(Arc::new(DefenseVocabulary {
            layers: vec!["shield".to_string()],
            ..Default::default()
        }));
    ctx
}

fn pawn(ctx: &ScriptCtx, seat: Seat, value: Option<f32>) -> EntityId {
    let mut registry = ctx.registry.borrow_mut();
    let pawn = registry.spawn(Transform::default());
    if let Some(value) = value {
        registry
            .set_component(
                pawn,
                DefenseLayersComponent {
                    descriptor: None,
                    layers: vec![DefenseLayer {
                        name: "shield".to_string(),
                        value,
                        capacity: 100.0,
                        recharge_delay_ms: 0.0,
                        recharge_rate: 0.0,
                        resist: Vec::new(),
                        refill: Default::default(),
                    }],
                    health_resist: Vec::new(),
                    last_hit_tick: None,
                },
            )
            .unwrap();
    }
    registry.bind_pawn_seat(pawn, seat);
    pawn
}

fn read(name: &str, owner: Option<&str>) -> BakedIr {
    BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root: IrNode::Input {
            name: name.to_string(),
            owner: owner.map(str::to_string),
        },
    }
}

fn number<S: BindingScope>(program: &postretro_foundation::BoundProgram<S>, scope: &S) -> f32 {
    match eval_value(program, scope) {
        IrValue::Number(value) => value,
        other => panic!("expected a number, got {other:?}"),
    }
}

// Q6: in a player-event condition a plain layer read means the player being
// evaluated; a pawn without the layer has no value, so it is not observed.
#[test]
fn player_event_condition_layer_read_means_evaluated_player() {
    let ctx = ctx();
    let first = pawn(&ctx, Seat(1), Some(30.0));
    let second = pawn(&ctx, Seat(2), Some(80.0));
    let bare = pawn(&ctx, Seat(3), None);
    let scope = PlayerConditionScope::new(ctx.clone());
    scope.begin_condition();
    let program = bind(&read("player.defense.shield", None), &scope).expect("binds");
    let inputs = scope.finish_condition();
    let slots = ctx.slot_table.borrow();
    let registry = ctx.registry.borrow();
    for (pawn, seat, expected) in [(first, Seat(1), 30.0), (second, Seat(2), 80.0)] {
        scope.seed_player(&registry, &slots, pawn, Some(seat));
        assert!(scope.observes(&inputs));
        assert_eq!(number(&program, &scope), expected);
    }
    scope.seed_player(&registry, &slots, bare, Some(Seat(3)));
    assert!(!scope.observes(&inputs), "absent without the layer");
    assert!(
        bind(&read("player.defense.armor", None), &scope).is_err(),
        "an unregistered layer fails the bind"
    );
}

// Q6: `byPlayer(on.player)` in a player-event reaction reads that player.
#[test]
fn on_player_layer_read_reads_the_event_player() {
    let ctx = ctx();
    let first = pawn(&ctx, Seat(1), Some(30.0));
    let second = pawn(&ctx, Seat(2), Some(80.0));
    let mut scope = DispatchScope::player_event(ctx.clone(), &[]);
    let program = bind(
        &read("player.defense.shield.capacity", Some("@player")),
        &scope,
    )
    .expect("binds");
    let current = bind(&read("player.defense.shield", Some("@player")), &scope).expect("binds");
    for (pawn, expected) in [(first, 30.0), (second, 80.0)] {
        scope.seed_event_player(&ctx.registry.borrow(), Some(pawn));
        assert_eq!(number(&current, &scope), expected);
        assert_eq!(number(&program, &scope), 100.0);
    }
    assert!(bind(&read("player.defense.armor", Some("@player")), &scope).is_err());
}
