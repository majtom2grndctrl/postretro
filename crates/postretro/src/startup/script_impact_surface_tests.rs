// Script damage outside a fixed tick runs its impact policies at the host's
// next evaluation point: the start of the next tick, or frame end before the
// removal pass. A trap kill therefore plays its authored death, despawns and
// reports the kill like an in-tick kill.
// See: context/lib/scripting.md §2 (Impact-policy evaluation)

use postretro_entities::components::health::HealthComponent;
use postretro_entities::{EntityId, ScriptCtx, Transform};
use postretro_foundation::{HealthDescriptor, ImpactEventDescriptor};
use postretro_sim::health::reactions::{ApplyDamageArgs, dispatch};
use serde_json::{Value, json};

use crate::startup::lifecycle::tests::{script_ctx, test_app};

fn input(name: &str) -> Value {
    json!({ "op": "input", "name": name })
}

fn number(value: f32) -> Value {
    json!({ "op": "const", "value": value })
}

fn spawn(ctx: &ScriptCtx, tag: &str) -> EntityId {
    let mut registry = ctx.registry.borrow_mut();
    let entity = registry.spawn(Transform::default());
    registry
        .set_component(
            entity,
            HealthComponent::from_descriptor(&HealthDescriptor {
                max: 100.0,
                hitbox: None,
                zone_multipliers: Default::default(),
                ..Default::default()
            }),
        )
        .unwrap();
    registry.set_tags(entity, vec![tag.to_string()]).unwrap();
    entity
}

fn script_damage(ctx: &ScriptCtx, target: EntityId, amount: f32) {
    dispatch(
        &mut ctx.registry.borrow_mut(),
        &[target],
        &ApplyDamageArgs {
            amount,
            damage_type: None,
        },
    )
    .unwrap();
}

fn install(app: &mut crate::App, events: Vec<ImpactEventDescriptor>) {
    app.session
        .as_mut()
        .unwrap()
        .scripting
        .impact_policy_runtime
        .replace_global_events(events);
}

fn event(tag: &str, policy: Vec<Value>) -> ImpactEventDescriptor {
    ImpactEventDescriptor {
        id: format!("{tag}-policy"),
        is_override: false,
        levels: Vec::new(),
        filter_tag: Some(tag.to_string()),
        policy,
    }
}

/// The frame's end: evaluate pending script impacts, then reap, as
/// `frame_loop::redraw` does. Returns `(entity, is_kill)` per removal.
fn frame_end(app: &mut crate::App, ctx: &ScriptCtx) -> Vec<(EntityId, bool)> {
    app.evaluate_pending_script_impacts();
    let mut removed = Vec::new();
    postretro_sim::impact_effects::run_end_of_frame_removal_pass(
        &mut ctx.registry.borrow_mut(),
        |removal| removed.push((removal.entity, removal.is_kill())),
    );
    removed
}

// A trap kill outside a tick: the authored death runs, the corpse despawns
// and the removal counts as a kill, on the same frame.
#[test]
fn frame_end_script_kill_runs_death_policy_despawns_and_reports() {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    let enemy = spawn(&ctx, "enemy");
    install(
        &mut app,
        vec![event(
            "enemy",
            vec![json!({
                "when": { "op": "le", "a": input("@impact.healthAfter"), "b": number(0.0) },
                "do": [
                    { "primitive": "setState", "target": "@impact.target",
                      "args": { "name": "died", "value": number(1.0) } },
                    { "primitive": "despawn", "target": "@impact.target", "args": {} },
                ],
            })],
        )],
    );

    script_damage(&ctx, enemy, 500.0);
    let removed = frame_end(&mut app, &ctx);

    assert_eq!(removed, vec![(enemy, true)]);
    assert!(!ctx.registry.borrow().exists(enemy));
}

// P16: a frame-end policy reads post-damage `player.health`, and a source
// grant on script damage reaches no one.
#[test]
fn frame_end_script_impact_reads_post_damage_player_health() {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    let pawn = spawn(&ctx, "hero");
    let movement = crate::spawn_pose::tests::movement_descriptor("hero", 0.4, 0.8, 0.5)
        .movement
        .unwrap();
    ctx.registry
        .borrow_mut()
        .set_component(
            pawn,
            postretro_foundation::PlayerMovementComponent::from_descriptor(&movement),
        )
        .unwrap();
    ctx.registry
        .borrow_mut()
        .mark_local_player_pawn(pawn)
        .unwrap();
    install(
        &mut app,
        vec![event(
            "hero",
            vec![
                json!({ "primitive": "setState", "target": "@impact.target",
                        "args": { "name": "seen", "value": input("player.health") } }),
                json!({ "primitive": "grantHealth", "target": "@impact.source",
                        "args": { "amount": number(50.0) } }),
            ],
        )],
    );

    script_damage(&ctx, pawn, 30.0);
    frame_end(&mut app, &ctx);

    let registry = ctx.registry.borrow();
    assert_eq!(
        registry
            .get_component::<postretro_entities::EntityStateComponent>(pawn)
            .unwrap()
            .get("seen"),
        70.0
    );
    assert_eq!(
        registry
            .get_component::<HealthComponent>(pawn)
            .unwrap()
            .current,
        70.0,
        "no damager, so the grant has no recipient"
    );
}

// The frame order the contract rests on: script impacts pending before a
// tick evaluate at its start, before `begin_tick` and any stage that reads
// health; those after the last tick evaluate at frame end, after every
// off-tick producer (player events, drains, `wait` landings, crossings) and
// before the removal pass. UI presses run before the tick loop.
#[test]
fn redraw_evaluates_script_impacts_at_tick_start_and_before_removal() {
    let source = include_str!("../frame_loop/mod.rs");
    let body = source.split("pub(crate) fn redraw(").nth(1).unwrap();
    let at = |needle: &str| {
        body.find(needle)
            .unwrap_or_else(|| panic!("redraw no longer contains `{needle}`"))
    };
    let tick_start = at("app.evaluate_pending_script_impacts();");
    let frame_end = body
        .rfind("app.evaluate_pending_script_impacts();")
        .unwrap();
    assert_ne!(tick_start, frame_end, "two evaluation points");
    assert!(
        at("ui_actions::") < at("for tick_index in"),
        "UI presses precede ticks"
    );
    assert!(
        tick_start > at("if app.is_connected_client() {"),
        "host path only"
    );
    assert!(tick_start < at("sim::begin_tick("));
    for producer in [
        "app.player_events.run_tick(",
        "named_drains::drain_frame_named_events(",
        "drain_landings_resolving_seats(",
        "frame_order::run_crossing_stage(",
    ] {
        assert!(
            at(producer) < frame_end,
            "{producer} lands before frame end"
        );
    }
    assert!(frame_end < at("impact_effects::run_end_of_frame_removal_pass("));
}

// Regression guard: a connected client's raw-tag `applyDamage` is not
// authoritative, so its impact is discarded unevaluated.
#[test]
fn client_raw_tag_apply_damage_evaluates_no_policy() {
    let mut app = test_app();
    crate::startup::lifecycle::tests::connect_test_app_as_client(&mut app);
    let ctx = script_ctx(&app);
    let enemy = spawn(&ctx, "enemy");
    install(
        &mut app,
        vec![event(
            "enemy",
            vec![json!({ "primitive": "despawn", "target": "@impact.target", "args": {} })],
        )],
    );

    script_damage(&ctx, enemy, 500.0);
    let removed = frame_end(&mut app, &ctx);

    assert!(removed.is_empty());
    assert!(ctx.registry.borrow().exists(enemy));
    assert!(
        ctx.registry
            .borrow_mut()
            .take_impact_dispatches()
            .is_empty()
    );
}
