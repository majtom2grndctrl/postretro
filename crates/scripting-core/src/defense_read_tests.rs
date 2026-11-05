// Player-layer reads by scope: a plain read in a player-event condition is
// the evaluated player, `byPlayer(on.player)` is the event's player,
// `byPlayer(impact.source)` is the damager, an unregistered layer fails the
// bind, and a pawn without the layer reads absent on each path.
// See: context/lib/scripting.md §11 (Defense layers), §12 (Player events)

use std::sync::Arc;

use postretro_entities::components::defense_layers::{DefenseLayer, DefenseLayersComponent};
use postretro_entities::{EntityId, ScriptCtx, Transform};
use postretro_foundation::defense::DefenseVocabulary;
use postretro_foundation::{
    BakedIr, BindingScope, CURRENT_IR_VERSION, IrNode, IrValue, Seat, bind, eval_value,
};

use crate::components::health::{DamageProducer, ImpactDispatch};
use crate::ir_player_scope::PlayerConditionScope;
use crate::ir_scopes::{DispatchScope, EntityScope};
use log::Level;
use postretro_test_log_capture::LogCapture;

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

// In a player-event condition a plain layer read means the player being
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

// `byPlayer(on.player)` in a player-event reaction reads that player.
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

// `byPlayer(on.player)` of a pawn without the layer reads absent: the scope
// flags the fire missing, so the caller warns and skips the write.
#[test]
fn on_player_layer_read_of_a_pawn_without_the_layer_is_missing() {
    let ctx = ctx();
    let shielded = pawn(&ctx, Seat(1), Some(30.0));
    let bare = pawn(&ctx, Seat(2), None);
    let mut scope = DispatchScope::player_event(ctx.clone(), &[]);
    let current = bind(&read("player.defense.shield", Some("@player")), &scope).expect("binds");

    scope.seed_event_player(&ctx.registry.borrow(), Some(bare));
    let _ = eval_value(&current, &scope);
    assert!(scope.take_missing_event_player_value(), "absent without the layer");

    scope.seed_event_player(&ctx.registry.borrow(), Some(shielded));
    assert_eq!(number(&current, &scope), 30.0);
    assert!(!scope.take_missing_event_player_value());
}

fn impact_from(target: EntityId, source: Option<EntityId>) -> ImpactDispatch {
    ImpactDispatch {
        amount: 1.0,
        raw: 1.0,
        absorbed: 0.0,
        broke: false,
        damage_type: None,
        health_before: 10.0,
        health_after: 9.0,
        max_health: 10.0,
        target,
        source,
        producer: DamageProducer::InTick,
        point: None,
    }
}

// `byPlayer(impact.source)` of a source without the layer reads absent, as an
// absent engine owner read does in an impact policy: zero, warned once per
// read however many hits read it. A source with the layer reads its value.
#[test]
fn impact_source_layer_read_of_a_source_without_the_layer_reads_zero_and_warns_once() {
    let ctx = ctx();
    let shielded = pawn(&ctx, Seat(1), Some(30.0));
    let bare = pawn(&ctx, Seat(2), None);
    let npc = ctx.registry.borrow_mut().spawn(Transform::default());
    let target = ctx.registry.borrow_mut().spawn(Transform::default());
    let mut scope = EntityScope::impact(ctx.clone());
    let current =
        bind(&read("player.defense.shield", Some("@impact.source")), &scope).expect("binds");

    let capture = LogCapture::start();
    for source in [Some(bare), Some(npc), None, Some(bare)] {
        scope
            .seed_impact(&impact_from(target, source))
            .expect("impact fire seeds");
        assert_eq!(number(&current, &scope), 0.0, "absent reads zero");
    }
    capture.assert_logged_once(
        Level::Warn,
        "owner read for slot `player.defense.shield`: the impact source is not a player or has no value; using zero",
    );

    capture.clear();
    scope
        .seed_impact(&impact_from(target, Some(shielded)))
        .expect("impact fire seeds");
    assert_eq!(number(&current, &scope), 30.0);
    capture.assert_not_logged(Level::Warn, "owner read for slot `player.defense.shield`");
}
