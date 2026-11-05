// `impact.source.grantLayer`: binding against the committed vocabulary, the
// capacity evaluated when the planned effect applies, and Number amounts read
// from the impact's facts.
// See: context/lib/scripting.md §2 (Impact-policy evaluation), §11 (Defense layers)

use super::tests::{evaluate_pending, event, input, number};
use super::*;
use crate::sim::defense_parameter_tests::{
    expr, health, input as ir_input, layer, lit, session, shield, spawn,
};
use postretro_entities::components::health::{DamageContext, apply_damage_with_context};
use postretro_foundation::DamagePayload;
use postretro_foundation::defense::HandleRef;
use serde_json::json;

fn shield_handle() -> Value {
    serde_json::to_value(HandleRef::DefenseLayer {
        name: "shield".to_string(),
    })
    .unwrap()
}

fn grant_layer(layer: Value, amount: Option<Value>) -> Value {
    let mut args = json!({ "layer": layer });
    if let Some(amount) = amount {
        args["amount"] = amount;
    }
    json!({ "primitive": "grantLayer", "target": "@impact.source", "args": args })
}

fn global_set(slot: &str, value: Value) -> Value {
    json!({ "primitive": "slot.set", "args": { "slot": slot, "value": value } })
}

/// A seated-pawn-shaped damager with an expression-capacity shield, and an
/// `enemy` target carrying a literal 100-point shield.
fn rig(ctx: &ScriptCtx) -> (EntityId, EntityId) {
    let source = spawn(ctx, "pawn");
    let target = spawn(ctx, "enemy");
    ctx.registry
        .borrow_mut()
        .set_tags(target, vec!["enemy".to_string()])
        .unwrap();
    (source, target)
}

fn world() -> ScriptCtx {
    session(vec![
        (
            "pawn",
            true,
            health(vec![layer(expr(ir_input("tune.cap")))]),
        ),
        ("enemy", false, health(vec![layer(lit(100.0))])),
    ])
}

fn strike(ctx: &ScriptCtx, target: EntityId, source: Option<EntityId>, amount: f32) {
    let mut context = DamageContext::new("impact-grant-test", DamageProducer::InTick);
    context.attacker = source;
    apply_damage_with_context(
        &mut ctx.registry.borrow_mut(),
        target,
        &DamagePayload {
            amount,
            ..Default::default()
        },
        context,
    );
}

#[test]
fn impact_policy_grant_evaluates_capacity_when_applied() {
    let ctx = world();
    let (source, target) = rig(&ctx);
    let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
    runtime.replace_global_events(vec![event(
        "level-up",
        "enemy",
        vec![
            global_set("tune.cap", number(150.0)),
            grant_layer(shield_handle(), None),
        ],
    )]);
    assert_eq!(shield(&ctx, source), (100.0, 100.0));

    strike(&ctx, target, Some(source), 1.0);
    evaluate_pending(&ctx, &mut runtime);

    assert_eq!(
        shield(&ctx, source),
        (150.0, 150.0),
        "the grant reads the raise written earlier in the same policy"
    );
}

#[test]
fn impact_source_grant_layer_reads_absorbed() {
    let ctx = world();
    let (source, target) = rig(&ctx);
    let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
    runtime.replace_global_events(vec![event(
        "siphon",
        "enemy",
        vec![grant_layer(
            shield_handle(),
            Some(json!({ "op": "mul", "a": input("@impact.absorbed"), "b": number(0.25) })),
        )],
    )]);
    strike(&ctx, source, None, 50.0);
    assert_eq!(shield(&ctx, source), (50.0, 100.0));

    strike(&ctx, target, Some(source), 40.0);
    evaluate_pending(&ctx, &mut runtime);

    assert_eq!(shield(&ctx, target), (60.0, 100.0));
    assert_eq!(
        shield(&ctx, source),
        (60.0, 100.0),
        "40 absorbed × 0.25 grants 10"
    );
}

#[test]
fn impact_grant_layer_binds_only_a_declared_layer_handle() {
    let ctx = world();
    let scope = EntityScope::impact(ctx.clone());
    let bind = |effect: Value| bind_effect(&effect, &scope).map(|_| ());

    assert!(bind(grant_layer(shield_handle(), None)).is_ok());
    assert!(bind(grant_layer(shield_handle(), Some(number(5.0)))).is_ok());

    let undeclared = json!({ "kind": "defenseLayer", "name": "armor" });
    let wrong_kind = json!({ "kind": "damageType", "name": "shock" });
    for (layer, expected) in [
        (undeclared, "`armor` is not a declared defense layer"),
        (
            wrong_kind,
            "expected a defense layer handle, got damage type `shock`",
        ),
        (json!("shield"), "grantLayer `layer`"),
    ] {
        let error = bind(grant_layer(layer, None)).expect_err("a bad reference drops the policy");
        assert!(error.contains(expected), "{error}");
    }
    let error = bind(json!({
        "primitive": "grantLayer",
        "target": "@impact.target",
        "args": { "layer": shield_handle() },
    }))
    .unwrap_err();
    assert_eq!(error, "grantLayer must target @impact.source");
    let error = bind(grant_layer(
        shield_handle(),
        Some(json!({ "op": "const", "value": true })),
    ))
    .unwrap_err();
    assert_eq!(error, "grantLayer `amount` must evaluate to a number");
}

// G14 / R-P7: one tick's grants and hits, in stage order, each seeing the
// last: a trigger grant, an NPC melee, a hitscan whose policy grants the
// damager, then frame-end script damage, which runs no policy but still
// reports the break on its dispatch.
#[test]
fn grants_and_hits_apply_in_tick_order() {
    let ctx = world();
    let (pawn, enemy) = rig(&ctx);
    let npc = ctx.registry.borrow_mut().spawn(Transform::default());
    let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
    runtime.replace_global_events(vec![event(
        "refund",
        "enemy",
        vec![grant_layer(shield_handle(), Some(number(10.0)))],
    )]);
    // Broken ten ticks ago; its delay is still running.
    strike(&ctx, pawn, Some(npc), 100.0);
    ctx.registry.borrow_mut().take_impact_dispatches();
    for _ in 0..10 {
        crate::sim::defense_parameter_tests::tick(&ctx);
    }
    crate::sim::defense_parameter_tests::tick(&ctx);
    let tick_n = ctx.registry.borrow().host_tick();
    assert_eq!(shield(&ctx, pawn), (0.0, 100.0));

    // Trigger stage: an activator grant of 25.
    let args = crate::grant::GrantLayerArgs {
        layer: postretro_foundation::AuthoredHandle(shield_handle()),
        amount: Some(25.0),
    };
    crate::grant::dispatch_layer(&mut ctx.registry.borrow_mut(), &[pawn], &args).unwrap();
    assert_eq!(shield(&ctx, pawn).0, 25.0);

    // AI stage: a 20-point melee takes it to 5, with no break.
    strike(&ctx, pawn, Some(npc), 20.0);
    let melee = ctx.registry.borrow_mut().take_impact_dispatches();
    assert_eq!(shield(&ctx, pawn).0, 5.0);
    assert!(!melee[0].broke);

    // Weapon stage: the pawn's hitscan; the enemy's policy grants it 10.
    strike(&ctx, enemy, Some(pawn), 1.0);
    evaluate_pending(&ctx, &mut runtime);
    assert_eq!(shield(&ctx, pawn).0, 15.0);

    // Frame end: `players().damage(20)` empties it, breaks it, and spills
    // the raw remainder to health.
    let mut context = DamageContext::new("players.damage", DamageProducer::AppDrain);
    context.attacker = None;
    apply_damage_with_context(
        &mut ctx.registry.borrow_mut(),
        pawn,
        &DamagePayload {
            amount: 20.0,
            ..Default::default()
        },
        context,
    );
    let frame_end = ctx.registry.borrow_mut().take_impact_dispatches();
    assert_eq!(shield(&ctx, pawn).0, 0.0);
    assert!(frame_end[0].broke, "the script hit reports the break");
    assert_eq!(frame_end[0].producer, DamageProducer::AppDrain);
    let registry = ctx.registry.borrow();
    assert_eq!(
        registry
            .get_component::<postretro_entities::components::health::HealthComponent>(pawn)
            .unwrap()
            .current,
        95.0
    );
    assert_eq!(
        registry
            .get_component::<postretro_entities::components::defense_layers::DefenseLayersComponent>(pawn)
            .unwrap()
            .last_hit_tick,
        Some(tick_n),
        "the stamp is this tick's; no grant moved it"
    );
}
