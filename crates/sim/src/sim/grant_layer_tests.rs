// `grantLayer` at the grant chokepoint: fill or add, the on-demand capacity
// evaluation that becomes the last good value, and the skip rules.
// See: context/lib/scripting.md §11 (Defense layers)

use postretro_entities::ScriptCtx;
use postretro_entities::components::defense_layers::DefenseLayersComponent;
use postretro_entities::components::grant::{GrantOutcome, grant_layer};
use postretro_entities::components::health::HealthComponent;
use postretro_entities::registry::{EntityId, Transform};
use postretro_test_log_capture::LogCapture;

use super::defense_parameter_tests::{
    Layer, div, expr, health, hit, input, layer, lit, session, set_global, shield, spawn, tick,
};

fn grant(ctx: &ScriptCtx, id: EntityId, amount: Option<f32>) -> GrantOutcome {
    grant_layer(&mut ctx.registry.borrow_mut(), id, "shield", amount)
}

fn last_hit(ctx: &ScriptCtx, id: EntityId) -> Option<u64> {
    ctx.registry
        .borrow()
        .get_component::<DefenseLayersComponent>(id)
        .unwrap()
        .last_hit_tick
}

fn recharging(capacity: f32) -> Layer {
    Layer {
        delay: Some(lit(0.0)),
        rate: Some(lit(60.0)),
        ..layer(lit(capacity))
    }
}

#[test]
fn grant_layer_fills_or_adds_without_moving_the_stamp() {
    let ctx = session(vec![("enemy", false, health(vec![layer(lit(100.0))]))]);
    let id = spawn(&ctx, "enemy");
    tick(&ctx);
    hit(&ctx, id, 60.0, None);
    let stamp = last_hit(&ctx, id);
    assert!(stamp.is_some(), "the hit stamps the layer");
    tick(&ctx);

    assert_eq!(grant(&ctx, id, Some(25.0)), GrantOutcome::Applied);
    assert_eq!(shield(&ctx, id), (65.0, 100.0), "an amount adds");
    assert_eq!(grant(&ctx, id, Some(50.0)), GrantOutcome::Applied);
    assert_eq!(
        shield(&ctx, id),
        (100.0, 100.0),
        "an add clamps to capacity"
    );
    hit(&ctx, id, 70.0, None);
    let stamp = last_hit(&ctx, id);
    tick(&ctx);
    assert_eq!(grant(&ctx, id, None), GrantOutcome::Applied);
    assert_eq!(shield(&ctx, id), (100.0, 100.0), "no amount fills");
    assert_eq!(last_hit(&ctx, id), stamp, "a grant never moves the stamp");
}

#[test]
fn level_up_raises_capacity_then_fills_on_the_same_tick() {
    let ctx = session(vec![(
        "pawn",
        true,
        health(vec![layer(expr(input("tune.cap")))]),
    )]);
    let id = spawn(&ctx, "pawn");
    hit(&ctx, id, 30.0, None);
    assert_eq!(shield(&ctx, id), (70.0, 100.0));
    // The level-up raise and the grant land in one tick, before its end.
    set_global(&ctx, "tune.cap", 150.0);
    grant(&ctx, id, None);
    assert_eq!(
        shield(&ctx, id),
        (150.0, 150.0),
        "the grant evaluates the raised capacity"
    );
    tick(&ctx);
    assert_eq!(shield(&ctx, id), (150.0, 150.0), "capacity stays ≥ current");
}

#[test]
fn grant_clamps_to_evaluated_capacity_then_adds() {
    let capture = LogCapture::start();
    let ctx = session(vec![
        (
            "enemy",
            false,
            health(vec![layer(expr(div(
                input("tune.cap"),
                input("tune.ward"),
            )))]),
        ),
        ("full", false, health(vec![layer(expr(input("tune.cap")))])),
    ]);
    set_global(&ctx, "tune.ward", 1.0);
    let id = spawn(&ctx, "enemy");
    hit(&ctx, id, 20.0, None);
    assert_eq!(shield(&ctx, id), (80.0, 100.0));

    // R-P18: Lower to 40, grant with no amount, then a 30-point hit.
    set_global(&ctx, "tune.cap", 40.0);
    grant(&ctx, id, None);
    assert_eq!(
        shield(&ctx, id),
        (40.0, 40.0),
        "the value clamps, then fills"
    );
    hit(&ctx, id, 30.0, None);
    assert_eq!(
        shield(&ctx, id),
        (10.0, 40.0),
        "the hit sees the grant's capacity"
    );

    // R-P21: The grant's evaluation is the last good value when the
    // expression faults next tick.
    set_global(&ctx, "tune.ward", 0.0);
    tick(&ctx);
    assert_eq!(shield(&ctx, id), (10.0, 40.0), "not 10 / 100");
    capture.assert_logged_once(
        log::Level::Warn,
        "descriptor `enemy` `components.health.layers[0].capacity` faulted",
    );

    // R-P18 variant: A full 100 / 100, capacity raised to 125, then +25.
    set_global(&ctx, "tune.cap", 100.0);
    let full = spawn(&ctx, "full");
    set_global(&ctx, "tune.cap", 125.0);
    grant(&ctx, full, Some(25.0));
    assert_eq!(shield(&ctx, full), (125.0, 125.0));
}

#[test]
fn grant_layer_rejects_negative_and_skips_missing_layer() {
    let capture = LogCapture::start();
    let ctx = session(vec![("enemy", false, health(vec![layer(lit(100.0))]))]);
    let id = spawn(&ctx, "enemy");
    hit(&ctx, id, 50.0, None);

    for amount in [-5.0, f32::NAN, f32::INFINITY] {
        assert_eq!(
            grant(&ctx, id, Some(amount)),
            GrantOutcome::SkippedInvalidAmount
        );
    }
    assert_eq!(
        shield(&ctx, id),
        (50.0, 100.0),
        "an invalid amount is a no-op"
    );
    capture.assert_logged(
        log::Level::Warn,
        "[Grant] grantLayer: amount -5 is negative or non-finite; no-op",
    );

    let bare = ctx.registry.borrow_mut().spawn(Transform::default());
    assert_eq!(grant(&ctx, bare, None), GrantOutcome::SkippedNoComponent);
    capture.assert_logged(
        log::Level::Warn,
        &format!("[Grant] grantLayer: entity {bare} has no `shield` layer; skipping"),
    );
    assert_eq!(
        grant_layer(&mut ctx.registry.borrow_mut(), id, "armor", None),
        GrantOutcome::SkippedNoComponent,
        "a layered entity without that kind skips"
    );
    capture.assert_logged(
        log::Level::Warn,
        &format!("[Grant] grantLayer: entity {id} has no `armor` layer; skipping"),
    );
    assert_eq!(shield(&ctx, id), (50.0, 100.0));
}

#[test]
fn grant_layer_on_a_latched_entity_fills_and_leaves_the_latch() {
    let ctx = session(vec![("pawn", true, health(vec![recharging(100.0)]))]);
    let id = spawn(&ctx, "pawn");
    hit(&ctx, id, 100.0, None);
    {
        let mut registry = ctx.registry.borrow_mut();
        let mut health = registry
            .get_component::<HealthComponent>(id)
            .unwrap()
            .clone();
        health.current = 0.0;
        health.death_handled = true;
        registry.set_component(id, health).unwrap();
    }
    assert_eq!(shield(&ctx, id).0, 0.0);
    grant(&ctx, id, Some(25.0));
    tick(&ctx);
    tick(&ctx);
    assert_eq!(shield(&ctx, id), (25.0, 100.0), "no recharge at health 0");
    let registry = ctx.registry.borrow();
    let health = registry.get_component::<HealthComponent>(id).unwrap();
    assert_eq!(health.current, 0.0);
    assert!(health.death_handled, "the latch stays set");
}
